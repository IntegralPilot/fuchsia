// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
mod internal_message;
mod keyboard;
mod mouse;
mod touch;

use async_utils::hanging_get::client::HangingGetStream;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_element::{
    GraphicalPresenterMarker, GraphicalPresenterProxy, ViewControllerMarker, ViewControllerProxy,
    ViewSpec,
};
use fidl_fuchsia_math as fmath;
use fidl_fuchsia_ui_app as fapp;
use fidl_fuchsia_ui_composition as fland;
use fidl_fuchsia_ui_input3 as fuiinput;
use fidl_fuchsia_ui_pointer as fptr;
use fidl_fuchsia_ui_pointer::EventPhase;
use fidl_fuchsia_ui_views as fviews;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::{self as component};
use fuchsia_scenic::ViewRefPair;
use fuchsia_trace as trace;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures::prelude::*;
use internal_message::InternalMessage;
use log::{error, warn};
use std::env;

const IMAGE_ID: fland::ContentId = fland::ContentId { value: 2 };
const TRANSFORM_ID: fland::TransformId = fland::TransformId { value: 3 };

struct AppModel<'a> {
    flatland: &'a fland::FlatlandProxy,
    graphical_presenter: Option<GraphicalPresenterProxy>,
    internal_sender: UnboundedSender<InternalMessage>,
    view_controller: Option<ViewControllerProxy>,
    color: fland::ColorRgba,
    size: fmath::SizeU,
    keyboard: Option<fuiinput::KeyboardProxy>,
}

impl<'a> AppModel<'a> {
    fn new(
        flatland: &'a fland::FlatlandProxy,
        graphical_presenter: Option<GraphicalPresenterProxy>,
        internal_sender: UnboundedSender<InternalMessage>,
    ) -> AppModel<'a> {
        AppModel {
            flatland,
            graphical_presenter,
            internal_sender,
            view_controller: None,
            color: fland::ColorRgba { red: 1.0, green: 1.0, blue: 0.0, alpha: 1.0 },
            size: fmath::SizeU { width: 100, height: 100 },
            keyboard: None,
        }
    }

    async fn init_scene(&mut self) {
        // Create a rectangle that will fill the whole screen.
        self.flatland.create_filled_rect(&IMAGE_ID).expect("fidl error");
        self.flatland.set_solid_fill(&IMAGE_ID, &self.color, &self.size).expect("fidl error");

        // Populate the rest of the Flatland scene. There is a single transform which is set as the
        // root transform; the newly-created image is set as the content of that transform.
        self.flatland.create_transform(&TRANSFORM_ID).expect("fidl error");
        self.flatland.set_root_transform(&TRANSFORM_ID).expect("fidl error");
        self.flatland.set_content(&TRANSFORM_ID, &IMAGE_ID).expect("fidl error");
    }

    async fn create_view(&mut self) {
        // Set up the channel to listen for layout changes.
        let (parent_viewport_watcher, parent_viewport_watcher_request) =
            create_proxy::<fland::ParentViewportWatcherMarker>();

        // Keyboard setup
        let keyboard = connect_to_protocol::<fuiinput::KeyboardMarker>()
            .expect("error connecting to Keyboard");

        let (touch, touch_request) = create_proxy::<fptr::TouchSourceMarker>();
        let (mouse, mouse_request) = create_proxy::<fptr::MouseSourceMarker>();
        let (focuser, focuser_request) = create_proxy::<fviews::FocuserMarker>();
        let view_bound_protocols = fland::ViewBoundProtocols {
            touch_source: Some(touch_request),
            mouse_source: Some(mouse_request),
            view_focuser: Some(focuser_request),
            ..Default::default()
        };

        // Create the view.
        let fuchsia_scenic::flatland::ViewCreationTokenPair {
            view_creation_token,
            viewport_creation_token,
        } = fuchsia_scenic::flatland::ViewCreationTokenPair::new()
            .expect("failed to create view tokens");
        let view_identity = fviews::ViewIdentityOnCreation::from(
            ViewRefPair::new().expect("failed viewref creation"),
        );
        let view_ref_for_keyboard = fuchsia_scenic::duplicate_view_ref(&view_identity.view_ref)
            .expect("failed to dup view ref");
        let view_ref_for_focuser = fuchsia_scenic::duplicate_view_ref(&view_identity.view_ref)
            .expect("failed to dup view ref");
        self.flatland
            .create_view2(
                view_creation_token,
                view_identity,
                view_bound_protocols,
                parent_viewport_watcher_request,
            )
            .expect("fidl error");

        // TODO(https://fxbug.dev/42055565): `flatland.present()` is required before
        // calling `present_view()` to avoid scenic deadlock.
        self.flatland.present(fland::PresentArgs::default()).expect("flatland present");

        // Connect to graphical presenter to get the view displayed.
        let (view_controller_proxy, view_controller_request) =
            create_proxy::<ViewControllerMarker>();
        self.view_controller = Some(view_controller_proxy);
        let view_spec = ViewSpec {
            viewport_creation_token: Some(viewport_creation_token),
            ..Default::default()
        };

        let graphical_presenter =
            self.graphical_presenter.clone().expect("run with graphical_presenter");
        graphical_presenter
            .present_view(view_spec, None, Some(view_controller_request))
            .await
            .expect("failed to present view")
            .unwrap_or_else(|e| error!("{:?}", e));

        let (keyboard_listener_client, keyboard_listener_stream) =
            fidl::endpoints::create_request_stream::<fuiinput::KeyboardListenerMarker>();
        keyboard
            .add_listener(view_ref_for_keyboard, keyboard_listener_client)
            .await
            .expect("add_listener");

        if let Err(e) = focuser.request_focus(view_ref_for_focuser).await {
            error!("Failed to request focus: {:?}", e);
        }

        // Listen for updates over channels we just created.
        keyboard::spawn_keyboard_listener(keyboard_listener_stream, self.internal_sender.clone());
        self.keyboard = Some(keyboard);
        Self::spawn_layout_info_watcher(parent_viewport_watcher, self.internal_sender.clone());
        touch::spawn_touch_source_watcher(touch, self.internal_sender.clone());
        mouse::spawn_mouse_source_watcher(mouse, self.internal_sender.clone());
    }

    async fn create_view_with_token_identity(
        &mut self,
        view_creation_token: fviews::ViewCreationToken,
        view_identity: fviews::ViewIdentityOnCreation,
    ) {
        // Set up the channel to listen for layout changes.
        let (parent_viewport_watcher, parent_viewport_watcher_request) =
            create_proxy::<fland::ParentViewportWatcherMarker>();

        // Set up the protocols we care about (currently touch and mouse).
        let (touch, touch_request) = create_proxy::<fptr::TouchSourceMarker>();
        let (mouse, mouse_request) = create_proxy::<fptr::MouseSourceMarker>();
        let (focuser, focuser_request) = create_proxy::<fviews::FocuserMarker>();
        let view_bound_protocols = fland::ViewBoundProtocols {
            touch_source: Some(touch_request),
            mouse_source: Some(mouse_request),
            view_focuser: Some(focuser_request),
            ..Default::default()
        };

        let view_ref_for_keyboard = fuchsia_scenic::duplicate_view_ref(&view_identity.view_ref)
            .expect("failed to dup view ref");
        let view_ref_for_focuser = fuchsia_scenic::duplicate_view_ref(&view_identity.view_ref)
            .expect("failed to dup view ref");

        // Create the view.
        self.flatland
            .create_view2(
                view_creation_token,
                view_identity,
                view_bound_protocols,
                parent_viewport_watcher_request,
            )
            .expect("fidl error");

        // TODO(https://fxbug.dev/42055565): `flatland.present()` is required before
        // calling `present_view()` to avoid scenic deadlock.
        self.flatland.present(fland::PresentArgs::default()).expect("flatland present");

        // Listen for updates over channels we just created.
        Self::spawn_layout_info_watcher(parent_viewport_watcher, self.internal_sender.clone());
        touch::spawn_touch_source_watcher(touch, self.internal_sender.clone());
        mouse::spawn_mouse_source_watcher(mouse, self.internal_sender.clone());

        // Keyboard setup
        let keyboard = connect_to_protocol::<fuiinput::KeyboardMarker>()
            .expect("error connecting to Keyboard");
        let (keyboard_listener_client, keyboard_listener_stream) =
            fidl::endpoints::create_request_stream::<fuiinput::KeyboardListenerMarker>();
        keyboard
            .add_listener(view_ref_for_keyboard, keyboard_listener_client)
            .await
            .expect("add_listener");

        if let Err(e) = focuser.request_focus(view_ref_for_focuser).await {
            warn!("Failed to request focus: {:?}", e);
        }
        keyboard::spawn_keyboard_listener(keyboard_listener_stream, self.internal_sender.clone());
        self.keyboard = Some(keyboard);
    }

    fn spawn_layout_info_watcher(
        parent_viewport_watcher: fland::ParentViewportWatcherProxy,
        sender: UnboundedSender<InternalMessage>,
    ) {
        fasync::Task::spawn(async move {
            let mut layout_info_stream = HangingGetStream::new(
                parent_viewport_watcher,
                fland::ParentViewportWatcherProxy::get_layout,
            );

            while let Some(result) = layout_info_stream.next().await {
                match result {
                    Ok(layout_info) => {
                        sender
                            .unbounded_send(InternalMessage::Relayout {
                                size: layout_info
                                    .logical_size
                                    .unwrap_or(fmath::SizeU { width: 0, height: 0 }),
                            })
                            .expect("failed to send InternalMessage.");
                    }
                    Err(fidl_error) => {
                        warn!("ParentViewportWatcher GetLayout() error: {:?}", fidl_error);
                        return;
                    }
                }
            }
        })
        .detach();
    }

    fn on_relayout(&mut self, size: fmath::SizeU) {
        self.size = size;
        self.flatland.set_solid_fill(&IMAGE_ID, &self.color, &self.size).expect("fidl error");
    }

    fn next_color(&mut self) {
        self.color = fland::ColorRgba {
            red: (self.color.red + 0.0625) % 1.0,
            green: (self.color.green + 0.125) % 1.0,
            blue: (self.color.blue + 0.25) % 1.0,
            alpha: self.color.alpha,
        };
        self.flatland.set_solid_fill(&IMAGE_ID, &self.color, &self.size).expect("fidl error");
    }
}

fn setup_view_provider_fidl_services(sender: UnboundedSender<InternalMessage>) {
    let view_provider_cb = move |stream: fapp::ViewProviderRequestStream| {
        let sender = sender.clone();
        fasync::Task::local(
            stream
                .try_for_each(move |req| {
                    match req {
                        fapp::ViewProviderRequest::CreateView2 { args, .. } => {
                            let view_creation_token = args.view_creation_token.unwrap();
                            // We do not get passed a view ref so create our own.
                            let view_identity = fviews::ViewIdentityOnCreation::from(
                                ViewRefPair::new().expect("failed to create ViewRefPair"),
                            );
                            sender
                                .unbounded_send(InternalMessage::CreateView(
                                    view_creation_token,
                                    view_identity,
                                ))
                                .expect("failed to send InternalMessage.");
                        }
                        unhandled_req => {
                            warn!("Unhandled ViewProvider request: {:?}", unhandled_req);
                        }
                    };
                    future::ok(())
                })
                .unwrap_or_else(|e| {
                    error!("error running TemporaryFlatlandViewProvider server: {:?}", e)
                }),
        )
        .detach()
    };

    let mut fs = component::server::ServiceFs::new();
    fs.dir("svc").add_fidl_service(view_provider_cb);

    fs.take_and_serve_directory_handle().expect("failed to serve directory handle");
    fasync::Task::local(fs.collect()).detach();
}

fn setup_handle_flatland_events(
    event_stream: fland::FlatlandEventStream,
    sender: UnboundedSender<InternalMessage>,
) {
    fasync::Task::local(
        event_stream
            .try_for_each(move |event| {
                match event {
                    fland::FlatlandEvent::OnNextFrameBegin { .. } => {
                        sender
                            .unbounded_send(InternalMessage::OnNextFrameBegin)
                            .expect("failed to send InternalMessage");
                    }
                    fland::FlatlandEvent::OnFramePresented { .. } => {}
                    fland::FlatlandEvent::OnError { error } => {
                        sender
                            .unbounded_send(InternalMessage::OnPresentError { error })
                            .expect("failed to send InternalMessage.");
                    }
                };
                future::ok(())
            })
            .unwrap_or_else(|e| error!("error listening for Flatland Events: {:?}", e)),
    )
    .detach();
}

#[fuchsia::main]
async fn main() {
    // TODO(https://fxbug.dev/42162229): remove args once the view_provider refactoring is done.
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 || !(args[1] == "view_provider" || args[1] == "graphical_presenter") {
        error!("Param must be 'view_provider' or 'graphical_presenter', got {:?}", args);
    }
    let use_view_provider = if args[1] == "view_provider" { true } else { false };

    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let (internal_sender, mut internal_receiver) = unbounded::<InternalMessage>();

    let flatland =
        connect_to_protocol::<fland::FlatlandMarker>().expect("error connecting to Flatland");
    flatland.set_debug_name("simplest-app-flatland").expect("fidl error");

    setup_handle_flatland_events(flatland.take_event_stream(), internal_sender.clone());
    let mut app = if use_view_provider {
        setup_view_provider_fidl_services(internal_sender.clone());
        AppModel::new(&flatland, None, internal_sender.clone())
    } else {
        let graphical_presenter = connect_to_protocol::<GraphicalPresenterMarker>()
            .expect("error connecting to GraphicalPresenter");
        AppModel::new(&flatland, Some(graphical_presenter), internal_sender.clone())
    };

    app.init_scene().await;

    if !use_view_provider {
        app.create_view().await;
    }

    let mut present_count = 1;

    let mut touch_updates = Vec::<trace::Id>::new();

    loop {
        futures::select! {
          message = internal_receiver.next() => {
            if let Some(message) = message {
              match message {
                  InternalMessage::CreateView(view_creation_token, view_identity) => {
                      app.create_view_with_token_identity(view_creation_token, view_identity).await;
                  }
                  InternalMessage::Relayout { size } => {
                      app.on_relayout(size);
                  }
                  InternalMessage::OnPresentError { error } => {
                      error!("OnPresentError({:?})", error);
                      break;
                  }
                  InternalMessage::OnNextFrameBegin {} => {
                    trace::duration!("gfx", "SimplestApp::OnNextFrameBegin");

                    // End all flows from where the update was applied.
                    for trace_id in touch_updates.drain(..) {
                        trace::flow_end!("input", "touch_update", trace_id);
                    }

                    // Present all pending updates with a trace flow into Scenic based on
                    // present_count.
                    trace::flow_begin!("gfx", "Flatland::PerAppPresent[simplest-app-flatland]", present_count.into());
                    flatland.present(fland::PresentArgs::default()).expect("Present call failed");
                    present_count += 1;
                  }
                  InternalMessage::TouchEvent{ phase, trace_id } => {
                    trace::duration!("input", "OnTouchEvent");
                    trace::flow_end!("input", "touch_in_simplest_app", trace_id);
                    // Change color on every finger down event.
                    if phase == EventPhase::Add {
                        // Trace from now until the update is applied.
                        let trace_id = trace::Id::new();
                        touch_updates.push(trace_id);
                        trace::flow_begin!("input", "touch_update", trace_id);
                        app.next_color();
                    }
                  },
                  InternalMessage::MouseEvent{ trace_id, change_color } => {
                    trace::duration!("input", "OnMouseEvent");
                    trace::flow_end!("input", "mouse_in_simplest_app", trace_id);
                    if change_color {
                        app.next_color();
                    }
                  },
                  InternalMessage::KeyboardEvent{ trace_id, change_color } => {
                    trace::duration!("input", "OnKeyboardEvent");
                    trace::flow_end!("input", "keyboard_in_simplest_app", trace_id);
                    if change_color {
                        app.next_color();
                    }
                  },
                }
            }
          }
        }
    }
}
