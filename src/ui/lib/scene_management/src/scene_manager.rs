// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::lib::{Position, Size};
use crate::pointerinjector_config::{
    InjectorViewportChangeFn, InjectorViewportHangingGet, InjectorViewportPublisher,
    InjectorViewportSpec, InjectorViewportSubscriber,
};
use crate::{DisplayMetrics, ViewingDistance};
use anyhow::{Context, Error, Result};
use async_trait::async_trait;
use async_utils::hanging_get::server as hanging_get;
use fidl::endpoints::{Proxy, create_proxy};
use fidl_fuchsia_accessibility_scene as a11y_scene;
use fidl_fuchsia_math as math;
use fidl_fuchsia_ui_app as ui_app;
use fidl_fuchsia_ui_composition::{self as ui_comp, ContentId, TransformId};
use fidl_fuchsia_ui_display_singleton as singleton_display;
use fidl_fuchsia_ui_pointerinjector_configuration::{
    SetupRequest as PointerInjectorConfigurationSetupRequest,
    SetupRequestStream as PointerInjectorConfigurationSetupRequestStream,
};
use fidl_fuchsia_ui_views as ui_views;
use flatland_frame_scheduling_lib::*;
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_scenic as scenic;
use fuchsia_trace as trace;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use futures::prelude::*;
use log::{error, info, warn};
use math as fmath;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::CStr;
use std::process;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use zx;

/// Presentation messages.
pub enum PresentationMessage {
    /// Request a present call.
    RequestPresent,
    // Requests a present call; also, provides a channel that will get a ping back
    // when the next frame has been presented on screen.
    RequestPresentWithPingback(oneshot::Sender<()>),
}

/// Unbounded sender used for presentation messages.
pub type PresentationSender = UnboundedSender<PresentationMessage>;

/// Unbounded receiver used for presentation messages.
pub type PresentationReceiver = UnboundedReceiver<PresentationMessage>;

const _CURSOR_SIZE: (u32, u32) = (18, 29);
const CURSOR_HOTSPOT: (u32, u32) = (2, 4);

// TODO(https://fxbug.dev/42158302): Remove hardcoded scale when Flatland provides
// what is needed to determine the cursor scale factor.
const CURSOR_SCALE_MULTIPLIER: u32 = 5;
const CURSOR_SCALE_DIVIDER: u32 = 4;

const DEFAULT_VIEW_CONNECTION_TIMEOUT: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(15);

// Converts a cursor size to physical pixels.
fn physical_cursor_size(value: u32) -> u32 {
    (CURSOR_SCALE_MULTIPLIER * value) / CURSOR_SCALE_DIVIDER
}

pub type FlatlandPtr = Arc<ui_comp::FlatlandProxy>;

#[derive(Clone)]
struct TransformContentIdPair {
    transform_id: TransformId,
    content_id: ContentId,
}

/// FlatlandInstance encapsulates a FIDL connection to a Flatland instance, along with some other
/// state resulting from initializing the instance in a standard way; see FlatlandInstance::new().
/// For example, a view is created during initialization, and so FlatlandInstance stores the
/// corresponding ViewRef and a ParentViewportWatcher FIDL connection.
struct FlatlandInstance {
    flatland: FlatlandPtr,
    view_ref: ui_views::ViewRef,
    root_transform_id: TransformId,
    parent_viewport_watcher: ui_comp::ParentViewportWatcherProxy,
    focuser: ui_views::FocuserProxy,
}

impl FlatlandInstance {
    fn new(
        flatland: ui_comp::FlatlandProxy,
        view_creation_token: ui_views::ViewCreationToken,
        id_generator: &mut scenic::flatland::IdGenerator,
    ) -> Result<FlatlandInstance, Error> {
        let (parent_viewport_watcher, parent_viewport_watcher_request) =
            create_proxy::<ui_comp::ParentViewportWatcherMarker>();

        let (focuser, focuser_request) = create_proxy::<ui_views::FocuserMarker>();

        let view_bound_protocols = ui_comp::ViewBoundProtocols {
            view_focuser: Some(focuser_request),
            ..Default::default()
        };

        let view_identity = ui_views::ViewIdentityOnCreation::from(scenic::ViewRefPair::new()?);
        let view_ref = scenic::duplicate_view_ref(&view_identity.view_ref)?;
        flatland.create_view2(
            view_creation_token,
            view_identity,
            view_bound_protocols,
            parent_viewport_watcher_request,
        )?;

        let root_transform_id = id_generator.next_transform_id();
        flatland.create_transform(&root_transform_id)?;
        flatland.set_root_transform(&root_transform_id)?;
        flatland.set_hit_regions(&root_transform_id, &[])?;

        Ok(FlatlandInstance {
            flatland: Arc::new(flatland),
            view_ref,
            root_transform_id,
            parent_viewport_watcher,
            focuser,
        })
    }
}

fn request_present_with_pingback(
    presentation_sender: &PresentationSender,
) -> Result<oneshot::Receiver<()>, Error> {
    let (sender, receiver) = oneshot::channel::<()>();
    presentation_sender.unbounded_send(PresentationMessage::RequestPresentWithPingback(sender))?;
    Ok(receiver)
}

async fn setup_child_view(
    parent_flatland: &FlatlandInstance,
    viewport_creation_token: scenic::flatland::ViewportCreationToken,
    id_generator: &mut scenic::flatland::IdGenerator,
    client_viewport_size: math::SizeU,
) -> Result<ui_comp::ChildViewWatcherProxy, Error> {
    let child_viewport_transform_id = id_generator.next_transform_id();
    let child_viewport_content_id = id_generator.next_content_id();

    let (child_view_watcher, child_view_watcher_request) =
        create_proxy::<ui_comp::ChildViewWatcherMarker>();

    {
        let flatland = &parent_flatland.flatland;
        flatland.create_transform(&child_viewport_transform_id)?;
        flatland.add_child(&parent_flatland.root_transform_id, &child_viewport_transform_id)?;

        let link_properties = ui_comp::ViewportProperties {
            logical_size: Some(client_viewport_size),
            ..Default::default()
        };

        flatland.create_viewport(
            &child_viewport_content_id,
            viewport_creation_token,
            &link_properties,
            child_view_watcher_request,
        )?;
        flatland.set_content(&child_viewport_transform_id, &child_viewport_content_id)?;
    }

    Ok(child_view_watcher)
}

/// SceneManager manages the platform/framework-controlled part of the global Scenic scene
/// graph, with the fundamental goal of connecting the physical display to the product-defined user
/// shell.  The part of the scene graph managed by the scene manager is split between three Flatland
/// instances, which are linked by view/viewport pairs.
//
// The scene graph looks like this:
//
//         FD          FD:  FlatlandDisplay
//         |
//         R*          R*:  root transform of |root_flatland|,
//         |                and also the corresponding view/view-ref (see below)
//        / \
//       /   \         Rc:  transform holding whatever is necessary to render the cursor
//     Rpi    Rc
//      |      \       Rpi: transform with viewport linking to |pointerinjector_flatland|
//      |       (etc.)      (see docs on struct field for rationale)
//      |
//      P*             P*:  root transform of |pointerinjector_flatland|,
//      |                   and also the corresponding view/view-ref (see below)
//      |
//      Pa             Pa:  transform with viewport linking to an external Flatland instance
//      |                   owned by a11y manager.
//      |
//      A*             A*:  root transform of |a11y_flatland| (owned by a11y manager),
//      |                   and also the corresponding view/view-ref (see below).
//      |
//      As             As:  transform with viewport linking to |scene_flatland|.
//      |
//      |
//      S*             S*:  root transform of |scene_flatland|,
//      |                   and also the corresponding view/view-ref (see below)
//      |
//      (user shell)   The session uses the SceneManager.SetRootView() FIDL API to attach the user
//                     shell to the scene graph depicted above.
//
// A11y View can be disabled via `attach_a11y_view` flag. If disabled, Pa and A* is removed from the
// scene graph.
//
// There is a reason why the "corresponding view/view-refs" are called out in the diagram above.
// When registering an input device with the fuchsia.ui.pointerinjector.Registry API, the Config
// must specify two ViewRefs, the "context" and the "target"; the former must be a strict ancestor
// or the former (the target denotes the first eligible view to receive input; it will always be
// the root of the focus chain).  The context ViewRef is R* and the target ViewRef is P*.  Note that
// the possibly-inserted accessiblity view is the direct descendant of |pointerinjector_flatland|.
// This gives the accessiblity manager the ability to give itself focus, and therefore receive all
// input.
pub struct SceneManager {
    // Flatland connection between the physical display and the rest of the scene graph.
    _display: ui_comp::FlatlandDisplayProxy,

    // The size that will ultimately be assigned to the View created with the
    // `fuchsia.session.scene.Manager` protocol.
    client_viewport_size: math::SizeU,

    // Flatland instance that connects to |display|.  Hosts a viewport which connects it to
    // to a view in |pointerinjector_flatland|.
    //
    // See the above diagram of SceneManager's scene graph topology.
    root_flatland: FlatlandInstance,

    // Flatland instance that sits beneath |root_flatland| in the scene graph.  The reason that this
    // exists is that two different ViewRefs must be provided when configuring the input pipeline to
    // inject pointer events into Scenic via fuchsia.ui.pointerinjector.Registry; since a Flatland
    // instance can have only a single view, we add an additional Flatland instance into the scene
    // graph to obtain the second view (the "target" view; the "context" view is obtained from
    // |root_flatland|).
    //
    // See the above diagram of SceneManager's scene graph topology.
    _pointerinjector_flatland: FlatlandInstance,

    // Flatland instance that embeds the system shell (i.e. via the SetRootView() FIDL API).  Its
    // root view is attached to a viewport owned by the accessibility manager (via
    // fuchsia.accessibility.scene.Provider/CreateView()).
    scene_flatland: FlatlandInstance,

    // These are the ViewRefs returned by get_pointerinjection_view_refs().  They are used to
    // configure input-pipeline handlers for pointer events.
    context_view_ref: ui_views::ViewRef,
    target_view_ref: ui_views::ViewRef,

    // Used to sent presentation requests for |root_flatand| and |scene_flatland|, respectively.
    root_flatland_presentation_sender: PresentationSender,
    _pointerinjector_flatland_presentation_sender: PresentationSender,
    scene_flatland_presentation_sender: PresentationSender,

    // Holds a pair of IDs that are used to embed the system shell inside |scene_flatland|, a
    // TransformId identifying a transform in the scene graph, and a ContentId which identifies a
    // a viewport that is set as the content of that transform.
    scene_root_viewport_ids: RefCell<Option<TransformContentIdPair>>,

    // Generates a sequential stream of ContentIds and TransformIds.  By guaranteeing
    // uniqueness across all Flatland instances, we avoid potential confusion during debugging.
    id_generator: RefCell<scenic::flatland::IdGenerator>,

    // Supports callers of fuchsia.ui.pointerinjector.configuration.setup.WatchViewport(), allowing
    // each invocation to subscribe to changes in the viewport region.
    viewport_hanging_get: Rc<RefCell<InjectorViewportHangingGet>>,

    // Used to publish viewport changes to subscribers of |viewport_hanging_get|.
    // TODO(https://fxbug.dev/42168647): use this to publish changes to screen resolution.
    _viewport_publisher: Rc<RefCell<InjectorViewportPublisher>>,

    // Used to position the cursor.
    cursor_transform_id: Option<TransformId>,

    // Used to track cursor visibility.
    cursor_visibility: Cell<bool>,

    // Used to track the display metrics for the root scene.
    display_metrics: DisplayMetrics,

    // Used to convert between logical and physical pixels.
    //
    // (physical pixel) = (device_pixel_ratio) * (logical pixel)
    device_pixel_ratio: f32,

    // Lock to serialize set_root_view calls and prevent interleaving race conditions.
    set_root_view_lock: futures::lock::Mutex<()>,

    // Keep spawned tasks alive.
    _tasks: RefCell<Vec<fasync::Task<()>>>,
}

/// A [SceneManager] manages a Scenic scene graph, and allows clients to add views to it.
/// Each [`SceneManager`] can choose how to configure the scene, including lighting, setting the
/// frames of added views, etc.
///
/// # Example
///
/// ```
/// let view_provider = some_apips.connect_to_service::<ViewProviderMarker>()?;
///
/// let scenic = connect_to_service::<ScenicMarker>()?;
/// let mut scene_manager = scene_management::FlatSceneManager::new(scenic).await?;
/// scene_manager.set_root_view(viewport_token).await?;
///
/// ```
#[async_trait(?Send)]
pub trait SceneManagerTrait {
    /// Sets the root view for the scene.
    ///
    /// ViewRef will be unset for Flatland views.
    ///
    /// Removes any previous root view, as well as all of its descendants.
    async fn set_root_view(
        &self,
        viewport_creation_token: ui_views::ViewportCreationToken,
        view_ref: Option<ui_views::ViewRef>,
    ) -> Result<(), Error>;

    /// DEPRECATED: Use ViewportToken version above.
    /// Sets the root view for the scene.
    ///
    /// Removes any previous root view, as well as all of its descendants.
    async fn set_root_view_deprecated(
        &self,
        view_provider: ui_app::ViewProviderProxy,
    ) -> Result<ui_views::ViewRef, Error>;

    /// Requests a new frame be presented in the scene.
    fn present_root_view(&self);

    /// Sets the position of the cursor in the current scene. If no cursor has been created it will
    /// create one using default settings.
    ///
    /// # Parameters
    /// - `position_physical_px`: A [`Position`] struct representing the cursor position, in physical
    ///   pixels.
    ///
    /// # Notes
    /// If a custom cursor has not been set using `set_cursor_image` or `set_cursor_shape` a default
    /// cursor will be created and added to the scene.  The implementation of the `SceneManager` trait
    /// is responsible for translating the raw input position into "pips".
    fn set_cursor_position(&self, position_physical_px: Position);

    /// Sets the visibility of the cursor in the current scene. The cursor is visible by default.
    ///
    /// # Parameters
    /// - `visible`: Boolean value indicating if the cursor should be visible.
    fn set_cursor_visibility(&self, visible: bool);

    // Supports the implementation of fuchsia.ui.pointerinjector.configurator.Setup.GetViewRefs()
    fn get_pointerinjection_view_refs(&self) -> (ui_views::ViewRef, ui_views::ViewRef);

    /// Input pipeline handlers such as TouchInjectorHandler require the display size in order to be
    /// instantiated.  This method exposes that information.
    fn get_pointerinjection_display_size(&self) -> crate::lib::Size;

    // Support the hanging get implementation of
    // fuchsia.ui.pointerinjector.configurator.Setup.WatchViewport().
    fn get_pointerinjector_viewport_watcher_subscription(&self) -> InjectorViewportSubscriber;

    fn get_display_metrics(&self) -> &DisplayMetrics;

    /// Store a task to keep it alive for the lifetime of the SceneManager.
    fn manage_task(&self, task: fasync::Task<()>);
}

#[async_trait(?Send)]
impl SceneManagerTrait for SceneManager {
    /// Sets the root view for the scene.
    ///
    /// ViewRef will be unset for Flatland views.
    ///
    /// Removes any previous root view, as well as all of its descendants.
    async fn set_root_view(
        &self,
        viewport_creation_token: ui_views::ViewportCreationToken,
        _view_ref: Option<ui_views::ViewRef>,
    ) -> Result<(), Error> {
        let _guard = self.set_root_view_lock.lock().await;
        self.set_root_view_internal(viewport_creation_token).await.map(|_view_ref| {})
    }

    /// DEPRECATED: Use ViewportToken version above.
    /// Sets the root view for the scene.
    ///
    /// Removes any previous root view, as well as all of its descendants.
    async fn set_root_view_deprecated(
        &self,
        view_provider: ui_app::ViewProviderProxy,
    ) -> Result<ui_views::ViewRef, Error> {
        let _guard = self.set_root_view_lock.lock().await;
        let link_token_pair = scenic::flatland::ViewCreationTokenPair::new()?;

        // Use view provider to initiate creation of the view which will be connected to the
        // viewport that we create below.
        view_provider.create_view2(ui_app::CreateView2Args {
            view_creation_token: Some(link_token_pair.view_creation_token),
            ..Default::default()
        })?;

        self.set_root_view_internal(link_token_pair.viewport_creation_token).await
    }

    /// Requests a new frame be presented in the scene.
    fn present_root_view(&self) {
        self.root_flatland_presentation_sender
            .unbounded_send(PresentationMessage::RequestPresent)
            .expect("send failed");
    }

    // Supports the implementation of fuchsia.ui.pointerinjector.configurator.Setup.GetViewRefs()
    fn get_pointerinjection_view_refs(&self) -> (ui_views::ViewRef, ui_views::ViewRef) {
        (
            scenic::duplicate_view_ref(&self.context_view_ref).expect("failed to copy ViewRef"),
            scenic::duplicate_view_ref(&self.target_view_ref).expect("failed to copy ViewRef"),
        )
    }

    /// Sets the position of the cursor in the current scene. If no cursor has been created it will
    /// create one using default settings.
    ///
    /// # Parameters
    /// - `position_physical_px`: A [`Position`] struct representing the cursor position, in physical
    ///   pixels.
    ///
    /// # Notes
    /// If a custom cursor has not been set using `set_cursor_image` or `set_cursor_shape` a default
    /// cursor will be created and added to the scene.  The implementation of the `SceneManager` trait
    /// is responsible for translating the raw input position into "pips".
    fn set_cursor_position(&self, position_physical_px: Position) {
        if let Some(cursor_transform_id) = self.cursor_transform_id {
            let position_logical = position_physical_px / self.device_pixel_ratio;
            let x =
                position_logical.x.round() as i32 - physical_cursor_size(CURSOR_HOTSPOT.0) as i32;
            let y =
                position_logical.y.round() as i32 - physical_cursor_size(CURSOR_HOTSPOT.1) as i32;
            self.root_flatland
                .flatland
                .set_translation(&cursor_transform_id, &fmath::Vec_ { x, y })
                .expect("fidl error");
            self.root_flatland_presentation_sender
                .unbounded_send(PresentationMessage::RequestPresent)
                .expect("send failed");
        }
    }

    /// Sets the visibility of the cursor in the current scene. The cursor is visible by default.
    ///
    /// # Parameters
    /// - `visible`: Boolean value indicating if the cursor should be visible.
    fn set_cursor_visibility(&self, visible: bool) {
        if let Some(cursor_transform_id) = self.cursor_transform_id {
            if self.cursor_visibility.get() != visible {
                self.cursor_visibility.set(visible);
                let flatland = &self.root_flatland.flatland;
                if visible {
                    flatland
                        .add_child(&self.root_flatland.root_transform_id, &cursor_transform_id)
                        .expect("failed to add cursor to scene");
                } else {
                    flatland
                        .remove_child(&self.root_flatland.root_transform_id, &cursor_transform_id)
                        .expect("failed to remove cursor from scene");
                }
                self.root_flatland_presentation_sender
                    .unbounded_send(PresentationMessage::RequestPresent)
                    .expect("send failed");
            }
        }
    }

    /// Input pipeline handlers such as TouchInjectorHandler require the display size in order to be
    /// instantiated.  This method exposes that information.
    fn get_pointerinjection_display_size(&self) -> Size {
        // Input pipeline expects size in physical pixels.
        self.display_metrics.size_in_pixels()
    }

    // Support the hanging get implementation of
    // fuchsia.ui.pointerinjector.configurator.Setup.WatchViewport().
    fn get_pointerinjector_viewport_watcher_subscription(&self) -> InjectorViewportSubscriber {
        self.viewport_hanging_get.borrow_mut().new_subscriber()
    }

    fn get_display_metrics(&self) -> &DisplayMetrics {
        &self.display_metrics
    }

    fn manage_task(&self, task: fasync::Task<()>) {
        self._tasks.borrow_mut().push(task);
    }
}

const ROOT_VIEW_DEBUG_NAME: &str = "SceneManager Display";
const POINTER_INJECTOR_DEBUG_NAME: &str = "SceneManager PointerInjector";
const SCENE_DEBUG_NAME: &str = "SceneManager Scene";
const ROOT_VIEW_PRESENT_TRACING_NAME: &CStr = c"Flatland::PerAppPresent[SceneManager Display]";
const POINTER_INJECTOR_PRESENT_TRACING_NAME: &CStr =
    c"Flatland::PerAppPresent[SceneManager PointerInjector]";
const SCENE_TRACING_NAME: &CStr = c"Flatland::PerAppPresent[SceneManager Scene]";

impl SceneManager {
    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    pub async fn new(
        display: ui_comp::FlatlandDisplayProxy,
        singleton_display_info: singleton_display::InfoProxy,
        root_flatland: ui_comp::FlatlandProxy,
        pointerinjector_flatland: ui_comp::FlatlandProxy,
        scene_flatland: ui_comp::FlatlandProxy,
        a11y_view_provider: Option<a11y_scene::ProviderProxy>,
        display_rotation: u64,
        display_pixel_density: Option<f32>,
        viewing_distance: Option<ViewingDistance>,
    ) -> Result<Self, Error> {
        // If scenic closes, all the Scenic connections become invalid. This task exits the
        // process in response.
        let mut tasks = vec![start_exit_on_scenic_closed_task(display.clone())];

        let mut id_generator = scenic::flatland::IdGenerator::new();

        // Generate unique transform/content IDs that will be used to create the sub-scenegraphs
        // in the Flatland instances managed by SceneManager.
        let pointerinjector_viewport_transform_id = id_generator.next_transform_id();
        let pointerinjector_viewport_content_id = id_generator.next_content_id();

        root_flatland.set_debug_name(ROOT_VIEW_DEBUG_NAME)?;
        pointerinjector_flatland.set_debug_name(POINTER_INJECTOR_DEBUG_NAME)?;
        scene_flatland.set_debug_name(SCENE_DEBUG_NAME)?;

        let root_view_creation_pair = scenic::flatland::ViewCreationTokenPair::new()?;
        let root_flatland = FlatlandInstance::new(
            root_flatland,
            root_view_creation_pair.view_creation_token,
            &mut id_generator,
        )?;

        let pointerinjector_view_creation_pair = scenic::flatland::ViewCreationTokenPair::new()?;
        let pointerinjector_flatland = FlatlandInstance::new(
            pointerinjector_flatland,
            pointerinjector_view_creation_pair.view_creation_token,
            &mut id_generator,
        )?;

        let scene_view_creation_pair = scenic::flatland::ViewCreationTokenPair::new()?;
        let scene_flatland = FlatlandInstance::new(
            scene_flatland,
            scene_view_creation_pair.view_creation_token,
            &mut id_generator,
        )?;

        // Create display metrics, and set the device pixel ratio of FlatlandDisplay.
        let info = singleton_display_info.get_metrics().await?;
        let extent_in_px =
            info.extent_in_px.ok_or_else(|| anyhow::anyhow!("Did not receive display size"))?;
        let display_metrics = DisplayMetrics::new(
            Size { width: extent_in_px.width as f32, height: extent_in_px.height as f32 },
            display_pixel_density,
            viewing_distance,
            None,
        );

        display.set_device_pixel_ratio(&fmath::VecF {
            x: display_metrics.pixels_per_pip(),
            y: display_metrics.pixels_per_pip(),
        })?;

        // Connect the FlatlandDisplay to |root_flatland|'s view.
        {
            // We don't need to watch the child view, since we also own it. So, we discard the
            // client end of the the channel pair.
            let (_, child_view_watcher_request) = create_proxy::<ui_comp::ChildViewWatcherMarker>();

            display.set_content(
                root_view_creation_pair.viewport_creation_token,
                child_view_watcher_request,
            )?;
        }

        // Obtain layout info from FlatlandDisplay. Logical size may be different from the
        // display size if DPR is applied.
        let layout_info = root_flatland.parent_viewport_watcher.get_layout().await?;
        let root_viewport_size = layout_info
            .logical_size
            .ok_or_else(|| anyhow::anyhow!("Did not receive layout info from the display"))?;

        let (
            display_rotation_enum,
            injector_viewport_translation,
            flip_injector_viewport_dimensions,
        ) = match display_rotation % 360 {
            0 => Ok((ui_comp::Orientation::Ccw0Degrees, math::Vec_ { x: 0, y: 0 }, false)),
            90 => Ok((
                // Rotation is specified in the opposite winding direction to the
                // specified |display_rotation| value. Winding in the opposite direction is equal
                // to -90 degrees, which is equivalent to 270.
                ui_comp::Orientation::Ccw270Degrees,
                math::Vec_ { x: root_viewport_size.width as i32, y: 0 },
                true,
            )),
            180 => Ok((
                ui_comp::Orientation::Ccw180Degrees,
                math::Vec_ {
                    x: root_viewport_size.width as i32,
                    y: root_viewport_size.height as i32,
                },
                false,
            )),
            270 => Ok((
                // Rotation is specified in the opposite winding direction to the
                // specified |display_rotation| value. Winding in the opposite direction is equal
                // to -270 degrees, which is equivalent to 90.
                ui_comp::Orientation::Ccw90Degrees,
                math::Vec_ { x: 0, y: root_viewport_size.height as i32 },
                true,
            )),
            _ => Err(anyhow::anyhow!("Invalid display rotation; must be {{0,90,180,270}}")),
        }?;
        let client_viewport_size = match flip_injector_viewport_dimensions {
            true => {
                math::SizeU { width: root_viewport_size.height, height: root_viewport_size.width }
            }
            false => {
                math::SizeU { width: root_viewport_size.width, height: root_viewport_size.height }
            }
        };

        // Create the pointerinjector view and embed it as a child of the root view.
        {
            let flatland = &root_flatland.flatland;
            flatland.create_transform(&pointerinjector_viewport_transform_id)?;
            flatland.add_child(
                &root_flatland.root_transform_id,
                &pointerinjector_viewport_transform_id,
            )?;
            flatland
                .set_orientation(&pointerinjector_viewport_transform_id, display_rotation_enum)?;
            flatland.set_translation(
                &pointerinjector_viewport_transform_id,
                &injector_viewport_translation,
            )?;

            let link_properties = ui_comp::ViewportProperties {
                logical_size: Some(client_viewport_size),
                ..Default::default()
            };

            let (_, child_view_watcher_request) = create_proxy::<ui_comp::ChildViewWatcherMarker>();

            flatland.create_viewport(
                &pointerinjector_viewport_content_id,
                pointerinjector_view_creation_pair.viewport_creation_token,
                &link_properties,
                child_view_watcher_request,
            )?;
            flatland.set_content(
                &pointerinjector_viewport_transform_id,
                &pointerinjector_viewport_content_id,
            )?;
        }

        let mut a11y_view_watcher: Option<ui_comp::ChildViewWatcherProxy> = None;
        match a11y_view_provider {
            Some(a11y_view_provider) => {
                let a11y_view_creation_pair = scenic::flatland::ViewCreationTokenPair::new()?;

                // Bridge the pointerinjector and a11y Flatland instances.
                a11y_view_watcher = Some(
                    setup_child_view(
                        &pointerinjector_flatland,
                        a11y_view_creation_pair.viewport_creation_token,
                        &mut id_generator,
                        client_viewport_size,
                    )
                    .await?,
                );

                // Request for the a11y manager to create its view.
                a11y_view_provider.create_view(
                    a11y_view_creation_pair.view_creation_token,
                    scene_view_creation_pair.viewport_creation_token,
                )?;
            }
            None => {
                // Bridge the pointerinjector and scene Flatland instances. This skips the A11y View.
                let _ = setup_child_view(
                    &pointerinjector_flatland,
                    scene_view_creation_pair.viewport_creation_token,
                    &mut id_generator,
                    client_viewport_size,
                )
                .await?;
            }
        }

        // Start Present() loops for both Flatland instances, and request that both be presented.
        let (root_flatland_presentation_sender, root_receiver) = unbounded();
        tasks.push(start_flatland_presentation_loop(
            root_receiver,
            Arc::downgrade(&root_flatland.flatland),
            ROOT_VIEW_DEBUG_NAME.to_string(),
        ));
        let (pointerinjector_flatland_presentation_sender, pointerinjector_receiver) = unbounded();
        tasks.push(start_flatland_presentation_loop(
            pointerinjector_receiver,
            Arc::downgrade(&pointerinjector_flatland.flatland),
            POINTER_INJECTOR_DEBUG_NAME.to_string(),
        ));
        let (scene_flatland_presentation_sender, scene_receiver) = unbounded();
        tasks.push(start_flatland_presentation_loop(
            scene_receiver,
            Arc::downgrade(&scene_flatland.flatland),
            SCENE_DEBUG_NAME.to_string(),
        ));

        let mut pingback_channels = Vec::new();
        pingback_channels.push(request_present_with_pingback(&root_flatland_presentation_sender)?);
        pingback_channels
            .push(request_present_with_pingback(&pointerinjector_flatland_presentation_sender)?);
        pingback_channels.push(request_present_with_pingback(&scene_flatland_presentation_sender)?);

        if let Some(a11y_view_watcher) = a11y_view_watcher {
            // Wait for a11y view to attach before proceeding.
            let a11y_view_status = a11y_view_watcher.get_status().await?;
            match a11y_view_status {
                ui_comp::ChildViewStatus::ContentHasPresented => {}
            }
        }

        // Read device pixel ratio from layout info.
        let device_pixel_ratio = display_metrics.pixels_per_pip();
        let viewport_hanging_get: Rc<RefCell<InjectorViewportHangingGet>> =
            create_viewport_hanging_get({
                InjectorViewportSpec {
                    width: display_metrics.width_in_pixels() as f32,
                    height: display_metrics.height_in_pixels() as f32,
                    scale: 1. / device_pixel_ratio,
                    x_offset: 0.,
                    y_offset: 0.,
                }
            });
        let viewport_publisher =
            Rc::new(RefCell::new(viewport_hanging_get.borrow_mut().new_publisher()));

        let context_view_ref = scenic::duplicate_view_ref(&root_flatland.view_ref)?;
        let target_view_ref = scenic::duplicate_view_ref(&pointerinjector_flatland.view_ref)?;

        // Wait for all pingbacks to ensure the scene is fully set up before returning.
        for receiver in pingback_channels {
            _ = receiver.await;
        }

        Ok(SceneManager {
            _display: display,
            client_viewport_size,
            root_flatland,
            _pointerinjector_flatland: pointerinjector_flatland,
            scene_flatland,
            context_view_ref,
            target_view_ref,
            root_flatland_presentation_sender,
            _pointerinjector_flatland_presentation_sender:
                pointerinjector_flatland_presentation_sender,
            scene_flatland_presentation_sender,
            scene_root_viewport_ids: RefCell::new(None),
            id_generator: RefCell::new(id_generator),
            viewport_hanging_get,
            _viewport_publisher: viewport_publisher,
            cursor_transform_id: None,
            cursor_visibility: Cell::new(true),
            display_metrics,
            device_pixel_ratio,
            set_root_view_lock: futures::lock::Mutex::new(()),
            _tasks: RefCell::new(tasks),
        })
    }

    async fn set_root_view_internal(
        &self,
        viewport_creation_token: ui_views::ViewportCreationToken,
    ) -> Result<ui_views::ViewRef> {
        // Remove any existing viewport.
        {
            let mut scene_root_viewport_ids = self.scene_root_viewport_ids.borrow_mut();
            if let Some(ids) = &*scene_root_viewport_ids {
                let flatland = &self.scene_flatland.flatland;
                flatland
                    .set_content(&ids.transform_id, &ContentId { value: 0 })
                    .context("could not set content")?;
                flatland.remove_child(&self.scene_flatland.root_transform_id, &ids.transform_id)?;
                flatland
                    .release_transform(&ids.transform_id)
                    .context("could not release transform")?;
                let _ = flatland.release_viewport(&ids.content_id);
            }
            *scene_root_viewport_ids = None;
        }

        // Create new viewport.
        let ids = {
            let mut id_generator = self.id_generator.borrow_mut();
            TransformContentIdPair {
                transform_id: id_generator.next_transform_id(),
                content_id: id_generator.next_content_id(),
            }
        };
        let (child_view_watcher, child_view_watcher_request) =
            create_proxy::<ui_comp::ChildViewWatcherMarker>();
        {
            let flatland = &self.scene_flatland.flatland;
            let viewport_properties = ui_comp::ViewportProperties {
                logical_size: Some(self.client_viewport_size),
                ..Default::default()
            };
            flatland.create_viewport(
                &ids.content_id,
                viewport_creation_token,
                &viewport_properties,
                child_view_watcher_request,
            )?;
            flatland.create_transform(&ids.transform_id).context("could not create transform")?;
            flatland.add_child(&self.scene_flatland.root_transform_id, &ids.transform_id)?;
            flatland
                .set_content(&ids.transform_id, &ids.content_id)
                .context("could not set content #2")?;
        }
        *self.scene_root_viewport_ids.borrow_mut() = Some(ids);

        // Present the previous scene graph mutations.  This MUST be done before awaiting the result
        // of get_view_ref() below, because otherwise the view won't become attached to the global
        // scene graph topology, and the awaited ViewRef will never come.
        let mut pingback_channels = Vec::new();
        pingback_channels.push(
            request_present_with_pingback(&self.scene_flatland_presentation_sender)
                .context("could not request present with pingback")?,
        );

        let _child_status = child_view_watcher
            .get_status()
            .on_timeout(fasync::MonotonicInstant::after(DEFAULT_VIEW_CONNECTION_TIMEOUT), || {
                make_timeout_error("fuchsia.ui.composition.ChildViewWatcher")
            })
            .await
            .context("could not call get_status")?;
        let child_view_ref = child_view_watcher
            .get_view_ref()
            .on_timeout(fasync::MonotonicInstant::after(DEFAULT_VIEW_CONNECTION_TIMEOUT), || {
                make_timeout_error("fuchsia.ui.composition.ChildViewWatcher")
            })
            .await
            .context("could not get view_ref")?;
        let child_view_ref_copy =
            scenic::duplicate_view_ref(&child_view_ref).context("could not duplicate view_ref")?;

        let request_focus_result = self
            .root_flatland
            .focuser
            .request_focus(child_view_ref)
            .on_timeout(fasync::MonotonicInstant::after(DEFAULT_VIEW_CONNECTION_TIMEOUT), || {
                make_timeout_error("fuchsia.ui.views.Focuser")
            })
            .await;
        match request_focus_result {
            Err(e) => warn!("Request focus failed with err: {}", e),
            Ok(Err(value)) => warn!("Request focus failed with err: {:?}", value),
            Ok(_) => {}
        }
        pingback_channels.push(
            request_present_with_pingback(&self.root_flatland_presentation_sender)
                .context("could not request present with pingback #2")?,
        );

        // Wait for all pingbacks to ensure the scene is fully set up before returning.
        for receiver in pingback_channels {
            _ = receiver.await;
        }

        Ok(child_view_ref_copy)
    }
}

fn make_timeout_error<T>(protocol_name: &'static str) -> Result<T, fidl::Error> {
    Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, protocol_name })
}

pub fn create_viewport_hanging_get(
    initial_spec: InjectorViewportSpec,
) -> Rc<RefCell<InjectorViewportHangingGet>> {
    let notify_fn: InjectorViewportChangeFn = Box::new(|viewport_spec, responder| {
        if let Err(fidl_error) = responder.send(&(*viewport_spec).into()) {
            info!("Viewport hanging get notification, FIDL error: {}", fidl_error);
        }
        // TODO(https://fxbug.dev/42168817): the HangingGet docs don't explain what value to return.
        true
    });

    Rc::new(RefCell::new(hanging_get::HangingGet::new(initial_spec, notify_fn)))
}

pub fn start_exit_on_scenic_closed_task(
    flatland_proxy: ui_comp::FlatlandDisplayProxy,
) -> fasync::Task<()> {
    fasync::Task::local(async move {
        let _ = flatland_proxy.on_closed().await;
        info!("Scenic died, closing SceneManager too.");
        process::exit(1);
    })
}

pub fn start_flatland_presentation_loop(
    mut receiver: PresentationReceiver,
    weak_flatland: Weak<ui_comp::FlatlandProxy>,
    debug_name: String,
) -> fasync::Task<()> {
    fasync::Task::local(async move {
        let mut present_count = 0;
        let scheduler = ThroughputScheduler::new();
        let mut flatland_event_stream = {
            if let Some(flatland) = weak_flatland.upgrade() {
                flatland.take_event_stream()
            } else {
                warn!(
                    "Failed to upgrade Flatand weak ref; exiting presentation loop for {debug_name}"
                );
                return;
            }
        };

        let mut channels_awaiting_pingback = VecDeque::from([Vec::new()]);

        loop {
            futures::select! {
                    message = receiver.next() => {
                        match message {
                            Some(PresentationMessage::RequestPresent) => {
                                scheduler.request_present();
                            }
                            Some(PresentationMessage::RequestPresentWithPingback(channel)) => {
                                channels_awaiting_pingback.back_mut().unwrap().push(channel);
                                scheduler.request_present();
                            }
                            None => {}
                        }
                    }
                    flatland_event = flatland_event_stream.next() => {
                        match flatland_event {
                            Some(Ok(ui_comp::FlatlandEvent::OnNextFrameBegin{ values })) => {
                                trace::duration!("scene_manager", "SceneManager::OnNextFrameBegin",
                                                 "debug_name" => &*debug_name);
                                let credits = values
                                              .additional_present_credits
                                              .expect("Present credits must exist");
                                let infos = values
                                    .future_presentation_infos
                                    .expect("Future presentation infos must exist")
                                    .iter()
                                    .map(
                                    |x| PresentationInfo{
                                        latch_point: zx::MonotonicInstant::from_nanos(x.latch_point.unwrap()),
                                        presentation_time: zx::MonotonicInstant::from_nanos(
                                                            x.presentation_time.unwrap())
                                    })
                                    .collect();
                                scheduler.on_next_frame_begin(credits, infos);
                            }
                            Some(Ok(ui_comp::FlatlandEvent::OnFramePresented{ frame_presented_info })) => {
                                trace::duration!("scene_manager", "SceneManager::OnFramePresented",
                                                 "debug_name" => &*debug_name);
                                let actual_presentation_time =
                                    zx::MonotonicInstant::from_nanos(frame_presented_info.actual_presentation_time);
                                let presented_infos: Vec<PresentedInfo> =
                                    frame_presented_info.presentation_infos
                                    .into_iter()
                                    .map(|x| x.into())
                                    .collect();

                                // Pingbacks for presented updates. For each presented frame, drain all
                                // of the corresponding pingback channels
                                for _ in 0..presented_infos.len() {
                                    for channel in channels_awaiting_pingback.pop_back().unwrap() {
                                        _ = channel.send(());
                                    }
                                }

                                scheduler.on_frame_presented(actual_presentation_time, presented_infos);
                            }
                            Some(Ok(ui_comp::FlatlandEvent::OnError{ error })) => {
                                error!(
                                    "Received FlatlandError code: {}; exiting listener loop for {debug_name}",
                                    error.into_primitive()
                                );
                                return;
                            }
                            _ => {}
                        }
                    }
                    present_parameters = scheduler.wait_to_update().fuse() => {
                        trace::duration!("scene_manager", "SceneManager::Present",
                                         "debug_name" => &*debug_name);

                        match debug_name.as_str() {
                            ROOT_VIEW_DEBUG_NAME => {
                                trace::flow_begin!("gfx", ROOT_VIEW_PRESENT_TRACING_NAME, present_count.into());
                            }
                            POINTER_INJECTOR_DEBUG_NAME => {
                                trace::flow_begin!("gfx", POINTER_INJECTOR_PRESENT_TRACING_NAME, present_count.into());
                            }
                            SCENE_DEBUG_NAME => {
                                trace::flow_begin!("gfx", SCENE_TRACING_NAME, present_count.into());
                            }
                            _ => {
                                warn!("SceneManager::Present with unknown debug_name {:?}", debug_name);
                            }
                        }
                        present_count += 1;
                        channels_awaiting_pingback.push_front(Vec::new());
                        if let Some(flatland) = weak_flatland.upgrade() {
                            flatland
                                .present(present_parameters.into())
                                .expect("Present failed for {debug_name}");
                        } else {
                            warn!(
                                "Failed to upgrade Flatand weak ref; exiting listener loop for {debug_name}"
                            );
                            return;
                        }
                    }
            }
        }
    })
}

pub fn handle_pointer_injector_configuration_setup_request_stream(
    mut request_stream: PointerInjectorConfigurationSetupRequestStream,
    scene_manager: Rc<dyn SceneManagerTrait>,
) -> fasync::Task<()> {
    fasync::Task::local(async move {
        let subscriber = scene_manager.get_pointerinjector_viewport_watcher_subscription();

        loop {
            let request = request_stream.try_next().await;
            match request {
                Ok(Some(PointerInjectorConfigurationSetupRequest::GetViewRefs { responder })) => {
                    let (context_view_ref, target_view_ref) =
                        scene_manager.get_pointerinjection_view_refs();
                    if let Err(e) = responder.send(context_view_ref, target_view_ref) {
                        warn!("Failed to send GetViewRefs() response: {}", e);
                    }
                }
                Ok(Some(PointerInjectorConfigurationSetupRequest::WatchViewport { responder })) => {
                    if let Err(e) = subscriber.register(responder) {
                        warn!("Failed to register WatchViewport() subscriber: {}", e);
                    }
                }
                Ok(None) => {
                    return;
                }
                Err(e) => {
                    error!("Error obtaining SetupRequest: {}", e);
                    return;
                }
            }
        }
    })
}
