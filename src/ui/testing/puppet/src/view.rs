// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::presentation_loop;
use async_utils::event::Event as AsyncEvent;
use async_utils::hanging_get::client::HangingGetStream;
use euclid::{Point2D, Transform2D};
use fidl::endpoints::{ServerEnd, create_proxy, create_request_stream};
use fidl_fuchsia_ui_input3::{self as ui_input3, KeyEvent};
use fidl_fuchsia_ui_pointer::{
    self as ui_pointer, MouseEvent, TouchEvent, TouchInteractionId, TouchInteractionStatus,
    TouchResponse,
};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use log::info;
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::slice::Iter;
use {
    fidl_fuchsia_math as fmath, fidl_fuchsia_ui_composition as ui_comp,
    fidl_fuchsia_ui_test_conformance as ui_conformance, fidl_fuchsia_ui_test_input as test_input,
    fidl_fuchsia_ui_views as ui_views, fuchsia_async as fasync, fuchsia_scenic as scenic,
};

pub type FlatlandPtr = Rc<ui_comp::FlatlandProxy>;

/// Helper function to request to present a set of changes to flatland.
async fn request_present(presentation_sender: &presentation_loop::PresentationSender) {
    let (sender, receiver) = oneshot::channel::<()>();
    presentation_sender.unbounded_send(sender).expect("failed to request present");
    _ = receiver.await;
}

// An interaction is a add-change-remove sequence for a single "finger" on a particular
// device.  Until the interaction status has been settled (i.e. the entire interaction is
// either denied or granted), events are buffered.  When the interaction is granted, the
// buffered events are sent to the app via `touch_input_listener`, and subsequent events
// are immediately sent via `touch_input_listener`.  Conversely, when the interaction is
// denied, buffered events and all subsequent events are dropped.
struct TouchInteraction {
    // Only contains InternalMessage::TouchEvents.
    pending_events: Vec<test_input::TouchInputListenerReportTouchInputRequest>,
    status: Option<ui_pointer::TouchInteractionStatus>,
}

/// Identifiers required to manipulate embedded view state.
struct EmbeddedViewIds {
    /// Flatland `TransformId` for the embedding viewport.
    transform_id: ui_comp::TransformId,

    /// Flatland `ContentId` for the embedding viewport.
    content_id: ui_comp::ContentId,
}

/// Encapsulates capabilities and resources associated with a puppet's view.
pub(super) struct View {
    /// Flatland connection scoped to our view.
    flatland: FlatlandPtr,

    /// Task to poll continuously for view events, and respond as necessary.
    view_event_listener: OnceCell<fasync::Task<()>>,

    /// Used to present changes to flatland.
    presentation_sender: presentation_loop::PresentationSender,

    /// Used to generate flatland transform and content IDs.
    id_generator: scenic::flatland::IdGenerator,

    /// Flatland `TransformId` that corresponds to our view's root transform.
    root_transform_id: ui_comp::TransformId,

    /// View dimensions, in its own logical coordinate space.
    logical_size: fmath::SizeU,

    /// DPR used to convert between logical and physical coordinates.
    device_pixel_ratio: f32,

    /// Indicates whether our view is connected to the display.
    connected_to_display: bool,

    /// Task to poll continuously for touch events, and respond as necessary.
    touch_watcher_task: OnceCell<fasync::Task<()>>,

    /// ViewParameters of touch event, need store this value because not all touch
    /// events include this information.
    view_parameters: Option<ui_pointer::ViewParameters>,

    /// Store pending touch interactions.
    touch_interactions: HashMap<TouchInteractionId, TouchInteraction>,

    /// Proxy to forward touch events to test.
    touch_input_listener: Option<test_input::TouchInputListenerProxy>,

    /// Task to poll continuously for mouse events.
    mouse_watched_task: OnceCell<fasync::Task<()>>,

    /// Proxy to forward mouse events to test.
    mouse_input_listener: Option<test_input::MouseInputListenerProxy>,

    /// Task to poll continuously for keyboard events.
    keyboard_watched_task: OnceCell<fasync::Task<()>>,

    /// Proxy to forward keyboard events to test.
    keyboard_input_listener: Option<test_input::KeyboardInputListenerProxy>,

    /// Holds a map from user-defined ID to embedded view IDs.
    embedded_views: HashMap<u64, EmbeddedViewIds>,
}

impl View {
    pub async fn new(
        flatland: ui_comp::FlatlandProxy,
        keyboard_client: ui_input3::KeyboardProxy,
        view_creation_token: ui_views::ViewCreationToken,
        touch_input_listener: Option<test_input::TouchInputListenerProxy>,
        mouse_input_listener: Option<test_input::MouseInputListenerProxy>,
        keyboard_input_listener: Option<test_input::KeyboardInputListenerProxy>,
        device_pixel_ratio: f32,
        view_focuser: Option<ServerEnd<ui_views::FocuserMarker>>,
    ) -> (Rc<RefCell<Self>>, ui_views::ViewRef) {
        let flatland = Rc::new(flatland);
        let (presentation_sender, presentation_receiver) = mpsc::unbounded();
        presentation_loop::start_flatland_presentation_loop(
            presentation_receiver,
            Rc::downgrade(&flatland),
        );

        let mut id_generator = scenic::flatland::IdGenerator::new();

        // Create view parameters.
        let (parent_viewport_watcher, parent_viewport_watcher_request) =
            create_proxy::<ui_comp::ParentViewportWatcherMarker>();
        let (touch_source, touch_source_request) = create_proxy::<ui_pointer::TouchSourceMarker>();
        let (mouse_source, mouse_source_request) = create_proxy::<ui_pointer::MouseSourceMarker>();
        let view_bound_protocols = ui_comp::ViewBoundProtocols {
            touch_source: Some(touch_source_request),
            mouse_source: Some(mouse_source_request),
            view_focuser,
            ..Default::default()
        };
        let view_ref_pair = scenic::ViewRefPair::new().expect("failed to create view ref pair");
        let view_ref = scenic::duplicate_view_ref(&view_ref_pair.view_ref)
            .expect("failed to duplicate view ref");
        let view_ref_clone =
            scenic::duplicate_view_ref(&view_ref).expect("failed to duplicate view ref");
        let view_identity = ui_views::ViewIdentityOnCreation::from(view_ref_pair);

        // Create root transform ID.
        let root_transform_id = Self::create_transform(flatland.clone(), &mut id_generator);

        // Create the view and present.
        flatland
            .create_view2(
                view_creation_token,
                view_identity,
                view_bound_protocols,
                parent_viewport_watcher_request,
            )
            .expect("failed to create view");
        flatland.set_root_transform(&root_transform_id).expect("failed to set root transform");
        info!("[b/328261162] Wait for request_present()");
        request_present(&presentation_sender).await;

        let this = Rc::new(RefCell::new(Self {
            flatland,
            view_event_listener: OnceCell::new(),
            presentation_sender,
            id_generator,
            root_transform_id,
            logical_size: fmath::SizeU { width: 0, height: 0 },
            device_pixel_ratio,
            connected_to_display: false,
            touch_watcher_task: OnceCell::new(),
            view_parameters: None,
            touch_interactions: HashMap::new(),
            touch_input_listener,
            mouse_watched_task: OnceCell::new(),
            mouse_input_listener,
            keyboard_watched_task: OnceCell::new(),
            keyboard_input_listener,
            embedded_views: HashMap::new(),
        }));

        let view_initialized = AsyncEvent::new();
        let view_events_task = fasync::Task::local(Self::listen_for_view_events(
            this.clone(),
            parent_viewport_watcher,
            view_initialized.clone(),
        ));
        this.borrow_mut()
            .view_event_listener
            .set(view_events_task)
            .expect("set event listener task more than once");

        // Start the touch watcher task.
        let touch_task =
            fasync::Task::local(Self::listen_for_touch_events(this.clone(), touch_source));
        this.borrow_mut()
            .touch_watcher_task
            .set(touch_task)
            .expect("set touch watcher task more than once");

        let mouse_task =
            fasync::Task::local(Self::listen_for_mouse_events(this.clone(), mouse_source));
        this.borrow_mut()
            .mouse_watched_task
            .set(mouse_task)
            .expect("set mouse watcher task more than once");

        let keyboard_ready = AsyncEvent::new();
        let keyboard_task = fasync::Task::local(Self::listen_for_key_events(
            this.clone(),
            keyboard_client,
            view_ref,
            keyboard_ready.clone(),
        ));
        this.borrow_mut()
            .keyboard_watched_task
            .set(keyboard_task)
            .expect("set keyboard watcher task more than once");

        info!("[b/328261162] Wait for view to be initialized.");
        _ = view_initialized.wait().await;

        info!("[b/328261162] Wait for keyboard listener ready.");
        _ = keyboard_ready.wait().await;

        (this, view_ref_clone)
    }

    /// Returns true if the parent viewport is connected to the display AND we've received non-zero
    /// layout info.
    fn is_initialized(&self) -> bool {
        info!(
            "connected to display = {} logical size = ({}, {})",
            self.connected_to_display, self.logical_size.width, self.logical_size.height
        );
        self.connected_to_display && self.logical_size.width > 0 && self.logical_size.height > 0
    }

    /// Polls continuously for events reported to the view (parent viewport updates,
    /// touch/mouse/keyboard input, etc.).
    async fn listen_for_view_events(
        this: Rc<RefCell<Self>>,
        parent_viewport_watcher: ui_comp::ParentViewportWatcherProxy,
        view_initialized: AsyncEvent,
    ) {
        let mut view_initialized = Some(view_initialized);

        let mut layout_info_stream = HangingGetStream::new(
            parent_viewport_watcher.clone(),
            ui_comp::ParentViewportWatcherProxy::get_layout,
        );
        let mut status_stream = HangingGetStream::new(
            parent_viewport_watcher,
            ui_comp::ParentViewportWatcherProxy::get_status,
        );

        loop {
            futures::select! {
                parent_status = status_stream.select_next_some() => {
                    match parent_status {
                        Ok(status) => {
                            info!("received parent status update");
                            this.borrow_mut().update_parent_status(status);
                        }
                        // Currently, CTF tests share the same puppet factory, so when one test exits,
                        // all other tests will get a channel closed error.
                        Err(fidl::Error::ClientChannelClosed{..}) => {
                            break;
                        }
                        Err(e) => {
                            panic!("get_status got unexpected error {:?}", e);
                        }
                    }
                }
                layout_info = layout_info_stream.select_next_some() => {
                    match layout_info {
                        Ok(layout_info) => {
                            this.borrow_mut().update_view_parameters(layout_info.logical_size, layout_info.device_pixel_ratio);
                        }
                        // Currently, CTF tests share the same puppet factory, so when one test exits,
                        // all other tests will get a channel closed error.
                        Err(fidl::Error::ClientChannelClosed{..}) => {
                            break;
                        }
                        Err(e) => {
                            panic!("get_layout got unexpected error {:?}", e);
                        }
                    }
                }
            }

            // If the view has become initialized, ping the `view_is_initialized` channel.
            if view_initialized.is_some() && this.borrow().is_initialized() {
                view_initialized.take().expect("failed to take view initialized sender").signal();
            }
        }
    }

    /// Creates a viewport according to the given `properties`.
    pub async fn embed_remote_view(
        &mut self,
        id: u64,
        properties: ui_conformance::EmbeddedViewProperties,
    ) -> ui_views::ViewCreationToken {
        let view_bounds = properties.bounds.expect("missing embedded view bounds");

        // Create the viewport transform.
        let transform_id = Self::create_transform(self.flatland.clone(), &mut self.id_generator);

        // Create the content id.
        let content_id = self.id_generator.next_content_id();

        // Create the view/viewport token pair.
        let token_pair = scenic::flatland::ViewCreationTokenPair::new()
            .expect("failed to create view creation token pair");

        // Create the embedding viewport.
        let (_, child_view_watcher_request) = create_proxy::<ui_comp::ChildViewWatcherMarker>();
        self.flatland
            .create_viewport(
                &content_id,
                token_pair.viewport_creation_token,
                &ui_comp::ViewportProperties {
                    logical_size: view_bounds.size,
                    ..Default::default()
                },
                child_view_watcher_request,
            )
            .expect("failed to create child viewport");

        // Attach the embedding viewport to its transform.
        self.flatland
            .set_content(&transform_id, &content_id)
            .expect("failed to set viewport content");

        // Position the embedded view.
        if let Some(origin) = view_bounds.origin {
            self.flatland
                .set_translation(&transform_id, &origin)
                .expect("failed to position embedded view");
        }

        // Attach the child view to the view's root transform.
        self.flatland
            .add_child(&self.root_transform_id, &transform_id)
            .expect("failed to attach embedded view to root transform");

        // Present changes.
        request_present(&self.presentation_sender).await;

        self.embedded_views.insert(id, EmbeddedViewIds { transform_id, content_id });

        token_pair.view_creation_token
    }

    pub async fn set_embedded_view_properties(
        &mut self,
        id: u64,
        properties: ui_conformance::EmbeddedViewProperties,
    ) {
        let view_bounds = properties.bounds.expect("missing embedded view bounds");

        // Get embedded view content + transform IDs.
        let embedded_view =
            self.embedded_views.get_mut(&id).expect("no embedded view with specified id");

        // Set viewport properties and translation.
        self.flatland
            .set_viewport_properties(
                &embedded_view.content_id,
                &ui_comp::ViewportProperties {
                    logical_size: view_bounds.size,
                    ..Default::default()
                },
            )
            .expect("failed to set viewport properties");
        if let Some(origin) = view_bounds.origin {
            self.flatland
                .set_translation(&embedded_view.transform_id, &origin)
                .expect("failed to position embedded view");
        }

        // Present changes.
        request_present(&self.presentation_sender).await;
    }

    /// Creates a flatland transform and returns its `TransformId`.
    fn create_transform(
        flatland: FlatlandPtr,
        id_generator: &mut scenic::flatland::IdGenerator,
    ) -> ui_comp::TransformId {
        let flatland_transform_id = id_generator.next_transform_id();

        flatland.create_transform(&flatland_transform_id).expect("failed to create transform");

        flatland_transform_id
    }

    /// Helper method to update our book keeping on our view's spatial parameters.
    fn update_view_parameters(
        &mut self,
        logical_size: Option<fmath::SizeU>,
        device_pixel_ratio: Option<fmath::VecF>,
    ) {
        if let Some(size) = logical_size {
            self.logical_size = size;
        }

        if let Some(dpr) = device_pixel_ratio {
            assert!(dpr.x == dpr.y);
            self.device_pixel_ratio = dpr.x;
        }
    }

    /// Helper method to update our book keeping on the parent viewport's status.
    fn update_parent_status(&mut self, parent_status: ui_comp::ParentViewportStatus) {
        self.connected_to_display = match parent_status {
            ui_comp::ParentViewportStatus::ConnectedToDisplay => true,
            ui_comp::ParentViewportStatus::DisconnectedFromDisplay => false,
        };
    }

    // If no `Interaction` exists for the specified `id`, insert a newly-instantiated one.
    fn ensure_interaction_exists(&mut self, id: &TouchInteractionId) {
        if !self.touch_interactions.contains_key(id) {
            self.touch_interactions
                .insert(id.clone(), TouchInteraction { pending_events: vec![], status: None });
        }
    }

    fn get_touch_report(
        &self,
        touch_event: &ui_pointer::TouchEvent,
    ) -> test_input::TouchInputListenerReportTouchInputRequest {
        let pointer_sample =
            touch_event.pointer_sample.as_ref().expect("touch event missing pointer_sample");
        let position_in_viewport = pointer_sample
            .position_in_viewport
            .expect("pointer sample missing position_in_viewport");
        let local_position =
            self.get_local_position(Point2D::new(position_in_viewport[0], position_in_viewport[1]));
        let interact = pointer_sample.interaction.expect("interaction is missing");
        let pointer_id = interact.pointer_id;
        let device_id = interact.device_id;

        let local_x: f64 = local_position.x.try_into().expect("failed to convert to f64");
        let local_y: f64 = local_position.y.try_into().expect("failed to convert to f64");
        let view_bounds = self.view_parameters.expect("missing view parameters").view;
        let view_min_x: f64 = view_bounds.min[0].try_into().expect("failed to convert to f64");
        let view_min_y: f64 = view_bounds.min[1].try_into().expect("failed to convert to f64");
        let view_max_x: f64 = view_bounds.max[0].try_into().expect("failed to convert to f64");
        let view_max_y: f64 = view_bounds.max[1].try_into().expect("failed to convert to f64");

        info!("view min ({:?}, {:?})", view_min_x, view_min_y);
        info!("view max ({:?}, {:?})", view_max_x, view_max_y);
        info!("tap received at ({:?}, {:?})", local_x, local_y);

        test_input::TouchInputListenerReportTouchInputRequest {
            local_x: Some(local_x),
            local_y: Some(local_y),
            phase: pointer_sample.phase,
            pointer_id: Some(pointer_id),
            time_received: touch_event.timestamp,
            device_pixel_ratio: Some(self.device_pixel_ratio as f64),
            device_id: Some(device_id),
            ..Default::default()
        }
    }

    fn get_local_position(&self, position_in_viewpoint: Point2D<f32, f32>) -> Point2D<f32, f32> {
        let viewport_to_view_transform =
            self.view_parameters.expect("missing view parameters").viewport_to_view_transform;
        Transform2D::new(
            /* 1, 1 */ viewport_to_view_transform[0],
            /* 1, 2 */ viewport_to_view_transform[3],
            /* 2, 1 */ viewport_to_view_transform[1],
            /* 2, 2 */ viewport_to_view_transform[4],
            /* 3, 1 */ viewport_to_view_transform[6],
            /* 3, 2 */ viewport_to_view_transform[7],
        )
        .transform_point(position_in_viewpoint)
    }

    fn process_touch_events(
        &mut self,
        events: Vec<ui_pointer::TouchEvent>,
    ) -> Vec<ui_pointer::TouchResponse> {
        // Generate the responses which will be sent with the next call to
        // `fuchsia.ui.pointer.TouchSource.Watch()`.
        let pending_responses = Self::generate_touch_event_responses(events.iter());

        for e in events.iter() {
            if let Some(view_parameters) = e.view_parameters {
                self.view_parameters = Some(view_parameters);
            }

            // Handle `pointer_sample` field, if it exists.
            if let Some(ui_pointer::TouchPointerSample { interaction: Some(id), .. }) =
                &e.pointer_sample
            {
                self.ensure_interaction_exists(&id);
                let interaction_status =
                    self.touch_interactions.get(id).expect("interaction does not exist").status;
                match interaction_status {
                    None => {
                        // Queue pending report unil interaction is resolved.
                        let touch_report = self.get_touch_report(&e);
                        self.touch_interactions
                            .get_mut(&id)
                            .unwrap()
                            .pending_events
                            .push(touch_report);
                    }
                    Some(TouchInteractionStatus::Granted) => {
                        match &self.touch_input_listener {
                            Some(listener) => {
                                // Samples received after the interaction is granted are
                                // immediately sent to the listener
                                let touch_report = self.get_touch_report(&e);
                                listener
                                    .report_touch_input(&touch_report)
                                    .expect("failed to send touch input report");
                            }
                            None => {
                                info!("no touch event listener.");
                            }
                        }
                    }
                    Some(TouchInteractionStatus::Denied) => {
                        // Drop the event/msg, and remove the interaction from the map:
                        // we're guaranteed not to receive any further events for this
                        // interaction.
                        self.touch_interactions.remove(&id);
                    }
                }
            }

            // Handle `interaction_result` field, if it exists.
            if let Some(ui_pointer::TouchInteractionResult { interaction: id, status }) =
                &e.interaction_result
            {
                self.ensure_interaction_exists(&id);
                let interaction = self.touch_interactions.get_mut(&id).unwrap();
                if let Some(existing_status) = &interaction.status {
                    // The status of an interaction can only change from None to Some().
                    assert_eq!(status, existing_status);
                } else {
                    // Status was previously None.
                    interaction.status = Some(status.clone());
                }

                match status {
                    ui_pointer::TouchInteractionStatus::Granted => {
                        // Report buffered events to touch listener
                        let mut pending_events = vec![];
                        std::mem::swap(
                            &mut pending_events,
                            &mut self.touch_interactions.get_mut(&id).unwrap().pending_events,
                        );
                        for pending_event in pending_events {
                            match &self.touch_input_listener {
                                Some(listener) => {
                                    listener
                                        .report_touch_input(&pending_event)
                                        .expect("failed to send touch input report");
                                }
                                None => {
                                    info!("no touch event listener.");
                                }
                            }
                        }
                    }
                    ui_pointer::TouchInteractionStatus::Denied => {
                        // Drop any buffered events and remove the interaction from the
                        // map: we're guaranteed not to receive any further events for
                        // this interaction.
                        self.touch_interactions.remove(&id);
                    }
                }
            }
        }

        pending_responses
    }

    /// Generate a vector of responses to the input `TouchEvents`, as required by
    /// `fuchsia.ui.pointer.TouchSource.Watch()`.
    fn generate_touch_event_responses(events: Iter<'_, TouchEvent>) -> Vec<TouchResponse> {
        events
            .map(|evt| {
                if let Some(_) = &evt.pointer_sample {
                    return TouchResponse {
                        response_type: Some(ui_pointer::TouchResponseType::Yes),
                        trace_flow_id: evt.trace_flow_id,
                        ..Default::default()
                    };
                }
                TouchResponse::default()
            })
            .collect()
    }

    async fn listen_for_touch_events(
        this: Rc<RefCell<Self>>,
        touch_source: ui_pointer::TouchSourceProxy,
    ) {
        let mut pending_responses: Vec<TouchResponse> = vec![];

        loop {
            let events = touch_source.watch(&pending_responses);

            match events.await {
                Ok(events) => {
                    pending_responses = this.borrow_mut().process_touch_events(events);
                }
                _ => {
                    info!("TouchSource connection closed");
                    return;
                }
            }
        }
    }

    fn get_mouse_report(
        &self,
        mouse_event: &ui_pointer::MouseEvent,
    ) -> test_input::MouseInputListenerReportMouseInputRequest {
        let pointer_sample =
            mouse_event.pointer_sample.as_ref().expect("mouse event missing pointer_sample");
        let position_in_viewport = pointer_sample
            .position_in_viewport
            .expect("pointer sample missing position_in_viewport");
        let local_position =
            self.get_local_position(Point2D::new(position_in_viewport[0], position_in_viewport[1]));
        let local_x: f64 = local_position.x.try_into().expect("failed to convert to f64");
        let local_y: f64 = local_position.y.try_into().expect("failed to convert to f64");
        let device_id = pointer_sample.device_id.expect("pointer sample missing device id");

        let buttons: Option<Vec<test_input::MouseButton>> = match &pointer_sample.pressed_buttons {
            None => None,
            Some(buttons) => Some(
                buttons
                    .iter()
                    .map(|button| {
                        test_input::MouseButton::from_primitive_allow_unknown(*button as u32)
                    })
                    .collect(),
            ),
        };

        test_input::MouseInputListenerReportMouseInputRequest {
            local_x: Some(local_x),
            local_y: Some(local_y),
            time_received: mouse_event.timestamp,
            buttons,
            device_pixel_ratio: Some(self.device_pixel_ratio as f64),
            wheel_x_physical_pixel: pointer_sample.scroll_h_physical_pixel,
            wheel_y_physical_pixel: pointer_sample.scroll_v_physical_pixel,
            device_id: Some(device_id),
            ..Default::default()
        }
    }

    fn process_mouse_events(&mut self, events: Vec<MouseEvent>) {
        for mouse_event in events {
            match &mouse_event {
                &MouseEvent {
                    stream_info:
                        Some(ui_pointer::MouseEventStreamInfo {
                            status: ui_pointer::MouseViewStatus::Exited,
                            ..
                        }),
                    ..
                } => return,
                &MouseEvent {
                    stream_info:
                        Some(ui_pointer::MouseEventStreamInfo {
                            status: ui_pointer::MouseViewStatus::Entered,
                            ..
                        }),
                    ..
                } => {
                    let view_parameters =
                        mouse_event.view_parameters.expect("entered event missing view_parameters");
                    self.view_parameters = Some(view_parameters);
                    continue;
                }
                _ => {}
            }
            let event = self.get_mouse_report(&mouse_event);
            match &self.mouse_input_listener {
                Some(listener) => {
                    listener.report_mouse_input(&event).expect("failed to send mouse input report");
                }
                None => {
                    info!("no mouse event listener");
                }
            }
        }
    }

    async fn listen_for_mouse_events(
        this: Rc<RefCell<Self>>,
        mouse_source: ui_pointer::MouseSourceProxy,
    ) {
        loop {
            let events = mouse_source.watch();
            match events.await {
                Ok(events) => {
                    this.borrow_mut().process_mouse_events(events);
                }
                _ => {
                    info!("MouseSource connection closed");
                    return;
                }
            }
        }
    }

    fn get_key_report(
        &self,
        key_event: KeyEvent,
    ) -> Option<test_input::KeyboardInputListenerReportTextInputRequest> {
        if key_event.type_ != Some(ui_input3::KeyEventType::Pressed) {
            return None;
        }
        match key_event.key_meaning.unwrap() {
            ui_input3::KeyMeaning::Codepoint(code) => {
                let s = char::from_u32(code).expect("key event is not a valid char").to_string();
                info!("Key received {:?}", s);
                Some(test_input::KeyboardInputListenerReportTextInputRequest {
                    text: Some(s),
                    device_id: key_event.device_id,
                    ..Default::default()
                })
            }
            ui_input3::KeyMeaning::NonPrintableKey(key) => {
                info!("NonPrintableKey received {:?}", key);
                Some(test_input::KeyboardInputListenerReportTextInputRequest {
                    non_printable: Some(key),
                    device_id: key_event.device_id,
                    ..Default::default()
                })
            }
        }
    }

    fn process_key_event(&mut self, event: KeyEvent) {
        let report = self.get_key_report(event);
        match &self.keyboard_input_listener {
            Some(listener) => match report {
                Some(event) => {
                    listener
                        .report_text_input(&event)
                        .expect("failed to send keyboard input report");
                }
                None => {}
            },
            None => {
                info!("no keyboard event listener");
            }
        }
    }

    async fn listen_for_key_events(
        this: Rc<RefCell<Self>>,
        keyboard: ui_input3::KeyboardProxy,
        view_ref: ui_views::ViewRef,
        keyboard_ready: AsyncEvent,
    ) {
        let (keyboard_client, mut keyboard_stream) =
            create_request_stream::<ui_input3::KeyboardListenerMarker>();

        keyboard
            .add_listener(view_ref, keyboard_client)
            .await
            .expect("failed to add keyboard listener");

        keyboard_ready.signal();

        loop {
            let listener_request = keyboard_stream.next().await;
            match listener_request {
                Some(Ok(ui_input3::KeyboardListenerRequest::OnKeyEvent {
                    event,
                    responder,
                    ..
                })) => {
                    responder.send(ui_input3::KeyEventStatus::Handled).expect("send");
                    this.borrow_mut().process_key_event(event);
                }
                _ => {
                    info!("keyboard connection closed");
                    return;
                }
            }
        }
    }
}
