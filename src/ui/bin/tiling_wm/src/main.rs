// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl::endpoints::{Proxy, RequestStream, create_proxy};
use fidl_fuchsia_element as element;
use fidl_fuchsia_session_scene as scene;
use fidl_fuchsia_session_window as window;
use fidl_fuchsia_ui_composition as ui_comp;
use fidl_fuchsia_ui_views as ui_views;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::{ServiceFs, ServiceObj};
use fuchsia_scenic::ViewRefPair;
use fuchsia_scenic::flatland::{IdGenerator, ViewCreationTokenPair};
use futures::channel::mpsc::UnboundedSender;
use futures::{StreamExt, TryStreamExt};
use log::{error, info, warn};
use rand::distr::{Alphanumeric, SampleString};
use rand::rng;
use std::collections::{HashMap, VecDeque};

// The maximum number of concurrent services to serve.
const NUM_CONCURRENT_REQUESTS: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TileId(pub String);

impl std::fmt::Display for TileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "id={}", self.0)
    }
}

pub enum MessageInternal {
    GraphicalPresenterPresentView {
        view_spec: element::ViewSpec,
        annotation_controller: Option<element::AnnotationControllerProxy>,
        view_controller_request_stream: Option<element::ViewControllerRequestStream>,
        responder: element::GraphicalPresenterPresentViewResponder,
    },
    DismissClient {
        tile_id: TileId,
        control_handle: element::ViewControllerControlHandle,
    },
    ClientDied {
        tile_id: TileId,
    },
    ReceivedClientViewRef {
        tile_id: TileId,
        view_ref: ui_views::ViewRef,
    },
    WindowManagerListViews {
        responder: window::ManagerListResponder,
    },
    WindowManagerSetOrder {
        old_position: usize,
        new_position: usize,
        responder: window::ManagerSetOrderResponder,
    },
    WindowManagerCycle {
        responder: window::ManagerCycleResponder,
    },
    WindowManagerFocusView {
        position: usize,
        responder: window::ManagerFocusResponder,
    },
}

#[derive(Clone, Copy)]
struct ChildView {
    viewport_transform_id: ui_comp::TransformId,
    viewport_content_id: ui_comp::ContentId,
}

pub struct TilingWm {
    internal_sender: UnboundedSender<MessageInternal>,
    flatland: ui_comp::FlatlandProxy,
    id_generator: IdGenerator,
    view_focuser: ui_views::FocuserProxy,
    root_transform_id: ui_comp::TransformId,
    layout_info: ui_comp::LayoutInfo,
    tiles: HashMap<TileId, ChildView>,
    // Maintains ordering of tiles from left-to-right and top-to-bottom, with the first element
    // representing the top-left Tile and the last element representing the bottom-right Tile.
    tile_order: VecDeque<TileId>,
}

impl Drop for TilingWm {
    fn drop(&mut self) {
        info!("dropping TilingWm");
        let flatland = &self.flatland;
        let tiles = &mut self.tiles;
        tiles.retain(|key, tile| {
            if let Err(e) = Self::release_tile_resources(flatland, tile) {
                error!("Error releasing resources for tile {key}: {e}");
            }
            false
        });
        if let Err(e) = flatland.clear() {
            error!("Error clearing Flatland: {e}");
        }
    }
}

impl TilingWm {
    async fn handle_message(&mut self, message: MessageInternal) -> Result<(), Error> {
        match message {
            // The ElementManager has asked us (via GraphicalPresenter::PresentView()) to display
            // the view provided by a newly-launched element.
            MessageInternal::GraphicalPresenterPresentView {
                view_spec,
                annotation_controller,
                view_controller_request_stream,
                responder,
            } => {
                // We have either a view holder token OR a viewport_creation_token, but for
                // Flatland we can expect a viewport creation token.
                let viewport_creation_token = match view_spec.viewport_creation_token {
                    Some(token) => token,
                    None => {
                        warn!(
                            "Client attempted to present Gfx component but only Flatland is \
                             supported."
                        );
                        return Ok(());
                    }
                };

                // Create a Viewport that houses the view we are creating.
                let (tile_watcher, tile_watcher_request) =
                    create_proxy::<ui_comp::ChildViewWatcherMarker>();
                let viewport_content_id = self.id_generator.next_content_id();
                let viewport_properties = ui_comp::ViewportProperties {
                    logical_size: Some(self.layout_info.logical_size.unwrap()),
                    ..Default::default()
                };
                self.flatland
                    .create_viewport(
                        &viewport_content_id,
                        viewport_creation_token,
                        &viewport_properties,
                        tile_watcher_request,
                    )
                    .context("GraphicalPresenterPresentView create_viewport")?;

                // Attach the Viewport to the scene graph.
                let viewport_transform_id = self.id_generator.next_transform_id();
                self.flatland
                    .create_transform(&viewport_transform_id)
                    .context("GraphicalPresenterPresentView create_transform")?;
                self.flatland
                    .set_content(&viewport_transform_id, &viewport_content_id)
                    .context("GraphicalPresenterPresentView create_transform")?;
                self.flatland
                    .add_child(&self.root_transform_id, &viewport_transform_id)
                    .context("GraphicalPresenterPresentView add_child")?;

                let mut view_name = Alphanumeric.sample_string(&mut rng(), 16);
                view_name.make_ascii_lowercase();
                let new_tile_id = TileId(view_name);
                let new_tile = ChildView { viewport_transform_id, viewport_content_id };
                self.tiles.insert(new_tile_id.clone(), new_tile);
                self.tile_order.push_front(new_tile_id.clone());

                self.layout_tiles()?;

                // Flush the changes.
                self.flatland
                    .present(ui_comp::PresentArgs {
                        requested_presentation_time: Some(0),
                        ..Default::default()
                    })
                    .context("GraphicalPresenterPresentView present")?;

                // Alert the client that the view has been presented, then begin servicing ViewController requests.
                if view_controller_request_stream.is_some() {
                    let view_controller_request_stream = view_controller_request_stream.unwrap();
                    view_controller_request_stream
                        .control_handle()
                        .send_on_presented()
                        .context("GraphicalPresenterPresentView send_on_presented")?;
                    run_tile_controller_request_stream(
                        new_tile_id.clone(),
                        view_controller_request_stream,
                        self.internal_sender.clone(),
                    );
                }

                // Begin servicing ChildViewWatcher requests.
                Self::watch_tile(new_tile_id, tile_watcher, self.internal_sender.clone());

                // Ignore Annotations for now.
                let _ = annotation_controller;

                // Finally, acknowledge the PresentView request.
                if let Err(e) = responder.send(Ok(())) {
                    error!("Failed to send response for GraphicalPresenter.PresentView(): {}", e);
                }

                Ok(())
            }
            MessageInternal::DismissClient { tile_id, control_handle } => {
                // Explicitly shutting down the handle indicates intentionality, instead of
                // (for example) because this component crashed and the handle was auto-closed.
                control_handle.shutdown_with_epitaph(Ok(()));
                match &mut self.tiles.remove(&tile_id) {
                    Some(tile) => {
                        self.layout_tiles()?;
                        Self::release_tile_resources(&self.flatland, tile)
                            .context("DismissClient release_tile_resources")?;
                    }
                    None => error!("Tile not found after client requested dismiss: {tile_id}"),
                }

                Ok(())
            }
            MessageInternal::ClientDied { tile_id } => {
                match &mut self.tiles.remove(&tile_id) {
                    Some(tile) => {
                        self.layout_tiles()?;
                        Self::release_tile_resources(&self.flatland, tile)
                            .context("ClientDied release_tile_resources")?;
                    }
                    None => error!("Tile not found after client died: {tile_id}"),
                }

                Ok(())
            }
            MessageInternal::ReceivedClientViewRef { tile_id, view_ref, .. } => {
                let result = self.view_focuser.request_focus(view_ref);
                fasync::Task::local(async move {
                    match result.await {
                        Ok(Ok(())) => {
                            info!("Successfully requested focus on child {tile_id}")
                        }
                        Ok(Err(e)) => {
                            error!("Error while requesting focus on child {tile_id}: {e:?}")
                        }
                        Err(e) => {
                            error!("FIDL error while requesting focus on child {tile_id}: {e:?}")
                        }
                    }
                })
                .detach();

                Ok(())
            }
            MessageInternal::WindowManagerListViews { responder } => {
                let views = self.list_views();
                if let Err(e) = responder.send(&views) {
                    error!("Failed to send response for WindowManager Manager.List(): {}", e);
                }
                Ok(())
            }
            MessageInternal::WindowManagerSetOrder { old_position, new_position, responder } => {
                self.set_tile_order(old_position, new_position)?;
                self.flatland
                    .present(ui_comp::PresentArgs {
                        requested_presentation_time: Some(0),
                        ..Default::default()
                    })
                    .context("WindowManagerSetOrder present")?;
                if let Err(e) = responder.send() {
                    error!("Failed to send response for WindowManager Manager.SetOrder(): {}", e);
                }
                Ok(())
            }
            MessageInternal::WindowManagerCycle { responder } => {
                self.cycle_tiles()?;
                self.flatland
                    .present(ui_comp::PresentArgs {
                        requested_presentation_time: Some(0),
                        ..Default::default()
                    })
                    .context("WindowManagerSetOrder present")?;
                if let Err(e) = responder.send() {
                    error!("Failed to send response for WindowManager Manager.Cycle(): {}", e);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub async fn new(internal_sender: UnboundedSender<MessageInternal>) -> Result<TilingWm, Error> {
        // TODO(https://fxbug.dev/42169911): do something like this to instantiate the library component that knows
        // how to generate a Flatland scene to lay views out on a tiled grid.  It will be used in the
        // event loop below.
        // let tiles_helper = tile_helper::TilesHelper::new();

        // Set the root view and then wait for scene_manager to reply with a CreateView2 request.
        // Don't await the result yet, because the future will not resolve until we handle the
        // ViewProvider request below.
        let scene_manager = connect_to_protocol::<scene::ManagerMarker>()
            .expect("failed to connect to fuchsia.scene.Manager");

        // TODO(https://fxbug.dev/42055565): see scene_manager.fidl.  If we awaited the future immediately we
        // would deadlock.  Conversely, if we just dropped the future, then scene_manager would barf
        // because it would try to reply to present_root_view() on a closed channel.  So we kick off
        // the async FIDL request (which is not idiomatic for Rust, where typically the "future
        // doesn't do anything" until awaited), and then call create_wm() so
        // that present_root_view() eventually returns a result.
        let ViewCreationTokenPair { view_creation_token, viewport_creation_token } =
            ViewCreationTokenPair::new()?;
        let fut = scene_manager.present_root_view(viewport_creation_token);
        let wm = Self::create_wm(view_creation_token, internal_sender).await?;
        let _ = fut.await?;
        Ok(wm)
    }

    async fn create_wm(
        view_creation_token: ui_views::ViewCreationToken,
        internal_sender: UnboundedSender<MessageInternal>,
    ) -> Result<TilingWm, Error> {
        let flatland = connect_to_protocol::<ui_comp::FlatlandMarker>()
            .expect("failed to connect to fuchsia.ui.flatland.Flatland");
        flatland.set_debug_name("TilingWM")?;

        let mut id_generator = IdGenerator::new();

        // Create the root transform for tiles.
        let root_transform_id = id_generator.next_transform_id();
        flatland.create_transform(&root_transform_id)?;
        flatland.set_root_transform(&root_transform_id)?;

        // Create the root view for tiles.
        let (parent_viewport_watcher, parent_viewport_watcher_request) =
            create_proxy::<ui_comp::ParentViewportWatcherMarker>();
        let (view_focuser, view_focuser_request) =
            fidl::endpoints::create_proxy::<ui_views::FocuserMarker>();
        let view_identity = ui_views::ViewIdentityOnCreation::from(ViewRefPair::new()?);
        let view_bound_protocols = ui_comp::ViewBoundProtocols {
            view_focuser: Some(view_focuser_request),
            ..Default::default()
        };
        flatland.create_view2(
            view_creation_token,
            view_identity,
            view_bound_protocols,
            parent_viewport_watcher_request,
        )?;

        // Present the root scene.
        flatland.present(ui_comp::PresentArgs {
            requested_presentation_time: Some(0),
            ..Default::default()
        })?;

        // Get initial layout deterministically before proceeding.
        // Begin servicing ParentViewportWatcher requests.
        let layout_info = parent_viewport_watcher.get_layout().await?;
        Self::watch_layout(parent_viewport_watcher, internal_sender.clone());

        Ok(TilingWm {
            internal_sender,
            flatland,
            id_generator,
            view_focuser,
            root_transform_id,
            layout_info,
            tiles: HashMap::new(),
            tile_order: VecDeque::new(),
        })
    }

    fn release_tile_resources(
        flatland: &ui_comp::FlatlandProxy,
        tile: &mut ChildView,
    ) -> Result<(), Error> {
        let _ = flatland.release_viewport(&tile.viewport_content_id);
        flatland.release_transform(&tile.viewport_transform_id)?;
        // Note: While unlikely, a very quick succession of events may result
        // in running out of credits. This possibility is not addressed here.
        Ok(flatland.present(ui_comp::PresentArgs {
            requested_presentation_time: Some(0),
            ..Default::default()
        })?)
    }

    fn set_tile_order(&mut self, old_position: usize, new_position: usize) -> Result<(), Error> {
        if old_position == new_position {
            return Ok(());
        }
        if new_position > self.tile_order.len() - 1 {
            warn!(
                "WindowManager SetOrder failed: cannot move element out-of-bounds to position {}",
                new_position
            );
            return Ok(());
        }
        let tile_id = match self.tile_order.remove(old_position) {
            Some(id) => id,
            None => {
                warn!("WindowManager SetOrder failed: no element at position {}", old_position);
                return Ok(());
            }
        };
        self.tile_order.insert(new_position, tile_id);
        self.layout_tiles()
    }

    fn cycle_tiles(&mut self) -> Result<(), Error> {
        self.set_tile_order(0, self.tile_order.len() - 1)
    }

    fn list_views(&mut self) -> Vec<window::ListedView> {
        let mut list = vec![];
        for (pos, TileId(id)) in self.tile_order.iter().enumerate() {
            let view = window::ListedView { position: pos as u64, id: id.to_string() };
            list.push(view);
        }
        list
    }

    fn layout_tiles(&mut self) -> Result<(), Error> {
        let fullscreen_height = self.layout_info.logical_size.unwrap().height;
        let fullscreen_width = self.layout_info.logical_size.unwrap().width;
        let num_tiles = self.tiles.len() as u32;
        if num_tiles == 0 {
            return Ok(());
        }
        let mut columns = (num_tiles as f32).sqrt().ceil() as u32;
        let mut rows = (columns + num_tiles - 1) / columns;
        if fullscreen_height > fullscreen_width {
            std::mem::swap(&mut columns, &mut rows);
        }
        let tile_height = (fullscreen_height as f32) / (rows as f32);
        let mut tile_idx = 0;
        let mut tiles_in_row = columns;
        for r in 0..rows {
            if r == rows - 1 && (num_tiles % columns) != 0 {
                tiles_in_row = num_tiles % columns;
            }
            let tile_width = (fullscreen_width as f32) / (tiles_in_row as f32);
            let tile_size = fidl_fuchsia_math::SizeU {
                width: (tile_width as u32),
                height: (tile_height as u32),
            };
            let viewport_properties =
                ui_comp::ViewportProperties { logical_size: Some(tile_size), ..Default::default() };
            for c in 0..tiles_in_row {
                // Get next ChildView in order that will be added to this row/column slot
                let tile_name = &self.tile_order.get(tile_idx).expect("Index out of bounds");
                let view = self
                    .tiles
                    .get_mut(tile_name)
                    .unwrap_or_else(|| panic!("{tile_name} not found"));
                tile_idx += 1;
                let viewport_translation = fidl_fuchsia_math::Vec_ {
                    x: (c as i32) * (tile_width as i32),
                    y: (r as i32) * (tile_height as i32),
                };
                self.flatland
                    .set_viewport_properties(&view.viewport_content_id, &viewport_properties)
                    .expect("TilingWM failed to set tile's viewport properties");
                self.flatland
                    .set_translation(&view.viewport_transform_id, &viewport_translation)
                    .expect("TilingWM failed to set tile's translation");
            }
        }
        Ok(())
    }

    fn watch_layout(
        proxy: ui_comp::ParentViewportWatcherProxy,
        _internal_sender: UnboundedSender<MessageInternal>,
    ) {
        // Listen for channel closure.
        // TODO(https://fxbug.dev/42169911): Actually watch for and respond to layout changes.
        fasync::Task::local(async move {
            let _ = proxy.on_closed().await;
        })
        .detach();
    }

    fn watch_tile(
        tile_id: TileId,
        proxy: ui_comp::ChildViewWatcherProxy,
        internal_sender: UnboundedSender<MessageInternal>,
    ) {
        // Get view ref, then listen for channel closure.
        fasync::Task::local(async move {
            match proxy.get_view_ref().await {
                Ok(view_ref) => {
                    internal_sender
                        .unbounded_send(MessageInternal::ReceivedClientViewRef {
                            tile_id: tile_id.clone(),
                            view_ref,
                        })
                        .expect("Failed to send MessageInternal::ReceivedClientViewRef");
                }
                Err(_) => {
                    internal_sender
                        .unbounded_send(MessageInternal::ClientDied { tile_id })
                        .expect("Failed to send MessageInternal::ClientDied");
                    return;
                }
            }

            let _ = proxy.on_closed().await;

            internal_sender
                .unbounded_send(MessageInternal::ClientDied { tile_id })
                .expect("Failed to send MessageInternal::ClientDied");
        })
        .detach();
    }
}

enum ExposedServices {
    GraphicalPresenter(element::GraphicalPresenterRequestStream),
    WindowManager(window::ManagerRequestStream),
}

fn expose_services() -> Result<ServiceFs<ServiceObj<'static, ExposedServices>>, Error> {
    let mut fs = ServiceFs::new();

    // Add services for component outgoing directory.
    fs.dir("svc").add_fidl_service(ExposedServices::GraphicalPresenter);
    fs.dir("svc").add_fidl_service(ExposedServices::WindowManager);
    fs.take_and_serve_directory_handle()?;

    Ok(fs)
}

fn run_services(
    fs: ServiceFs<ServiceObj<'static, ExposedServices>>,
    internal_sender: UnboundedSender<MessageInternal>,
) {
    fasync::Task::local(async move {
        fs.for_each_concurrent(NUM_CONCURRENT_REQUESTS, |service_request: ExposedServices| async {
            match service_request {
                ExposedServices::GraphicalPresenter(request_stream) => {
                    run_graphical_presenter_service(request_stream, internal_sender.clone());
                }
                ExposedServices::WindowManager(request_stream) => {
                    run_window_manager_service(request_stream, internal_sender.clone());
                }
            }
        })
        .await;
    })
    .detach();
}

fn run_graphical_presenter_service(
    mut request_stream: element::GraphicalPresenterRequestStream,
    mut internal_sender: UnboundedSender<MessageInternal>,
) {
    fasync::Task::local(async move {
        loop {
            let result = request_stream.try_next().await;
            match result {
                Ok(Some(request)) => {
                    internal_sender = handle_graphical_presenter_request(request, internal_sender)
                }
                Ok(None) => {
                    info!("GraphicalPresenterRequestStream ended with Ok(None)");
                    return;
                }
                Err(e) => {
                    error!(
                        "Error while retrieving requests from GraphicalPresenterRequestStream: {}",
                        e
                    );
                    return;
                }
            }
        }
    })
    .detach();
}

fn handle_graphical_presenter_request(
    request: element::GraphicalPresenterRequest,
    internal_sender: UnboundedSender<MessageInternal>,
) -> UnboundedSender<MessageInternal> {
    match request {
        element::GraphicalPresenterRequest::PresentView {
            view_spec,
            annotation_controller,
            view_controller_request,
            responder,
        } => {
            // "Unwrap" the optional element::AnnotationControllerProxy.
            let annotation_controller = annotation_controller.map(|proxy| proxy.into_proxy());
            // "Unwrap" the optional element::ViewControllerRequestStream.
            let view_controller_request_stream =
                view_controller_request.map(|request_stream| request_stream.into_stream());
            internal_sender
                .unbounded_send(
                    MessageInternal::GraphicalPresenterPresentView {
                        view_spec,
                        annotation_controller,
                        view_controller_request_stream,
                        responder,
                    },
                    // TODO(https://fxbug.dev/42169911): is this a safe expect()?  I think so, since
                    // we're using Task::local() instead of Task::spawn(), so we're on the
                    // same thread as main(), which will keep the receiver end alive until
                    // it exits, at which time the executor will not tick this task again.
                    // Assuming that we verify this understanding, what is the appropriate
                    // way to document this understanding?  Is it so idiomatic it needs no
                    // comment?  We're all Rust n00bs here, so maybe not?
                )
                .expect("Failed to send MessageInternal.");
        }
    }
    return internal_sender;
}

// Serve the fuchsia.element.ViewController protocol. This merely redispatches
// the requests onto the `MessageInternal` handler, which are handled by
// `TilingWm::handle_message`.
pub fn run_tile_controller_request_stream(
    tile_id: TileId,
    mut request_stream: fidl_fuchsia_element::ViewControllerRequestStream,
    internal_sender: UnboundedSender<MessageInternal>,
) {
    fasync::Task::local(async move {
        if let Some(Ok(fidl_fuchsia_element::ViewControllerRequest::Dismiss { control_handle })) =
            request_stream.next().await
        {
            {
                internal_sender
                    .unbounded_send(MessageInternal::DismissClient { tile_id, control_handle })
                    .expect("Failed to send MessageInternal::DismissClient");
            }
        }
    })
    .detach();
}

fn run_window_manager_service(
    mut request_stream: window::ManagerRequestStream,
    mut internal_sender: UnboundedSender<MessageInternal>,
) {
    fasync::Task::local(async move {
        loop {
            let result = request_stream.try_next().await;
            match result {
                Ok(Some(request)) => {
                    internal_sender = handle_window_manager_request(request, internal_sender);
                }
                Ok(None) => {
                    info!("Window Manager ManagerRequestStream ended with Ok(None)");
                    return;
                }
                Err(e) => {
                    error!(
                        "Error while retrieving requests from Window Manager ManagerRequestStream: {}",
                        e
                    );
                    return;
                }
            }
        }
    })
    .detach();
}

fn handle_window_manager_request(
    request: window::ManagerRequest,
    internal_sender: UnboundedSender<MessageInternal>,
) -> UnboundedSender<MessageInternal> {
    match request {
        window::ManagerRequest::List { responder } => {
            internal_sender
                .unbounded_send(MessageInternal::WindowManagerListViews { responder })
                .expect("Failed to send MessageInternal.");
        }
        window::ManagerRequest::SetOrder { old_position, new_position, responder } => {
            internal_sender
                .unbounded_send(MessageInternal::WindowManagerSetOrder {
                    old_position: old_position as usize,
                    new_position: new_position as usize,
                    responder,
                })
                .expect("Failed to send MessageInternal.");
        }
        window::ManagerRequest::Cycle { responder } => {
            internal_sender
                .unbounded_send(MessageInternal::WindowManagerCycle { responder })
                .expect("Failed to send MessageInternal");
        }
        window::ManagerRequest::Focus { position: _position, responder } => {
            // TODO(https://fxbug.dev/426605973): Request focus on view at `position`. For now, do
            // nothing. TilingWM already displays all views and focus shifts via user interaction
            // with a window.
            if let Err(e) = responder.send() {
                error!("Failed to send response for fuchsia.session.window.Manager.Focus: {}", e);
            }
        }
        _ => {
            unimplemented!()
        }
    }
    internal_sender
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), Error> {
    let (internal_sender, mut internal_receiver) =
        futures::channel::mpsc::unbounded::<MessageInternal>();

    // We start listening for service requests, but don't yet start serving those requests until we
    // we receive confirmation that we are hooked up to the Scene Manager.
    let fs = expose_services()?;

    // Connect to the scene owner and attach our tiles view to it.
    let mut wm = Box::new(TilingWm::new(internal_sender.clone()).await?);

    // Serve the FIDL services on the message loop, proxying them into internal messages.
    run_services(fs, internal_sender.clone());

    // Process internal messages using tiling wm, then cleanup when done.
    while let Some(message) = internal_receiver.next().await {
        if let Err(e) = wm.handle_message(message).await {
            error!("Error handling message: {e}");
            break;
        }
    }

    Ok(())
}
