// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(clippy::await_holding_refcell_ref)]
use crate::dispatcher::TaskHandle;
use crate::input_handler::{BatchInputHandler, Handler, InputHandlerStatus};
use crate::utils::{self, Position, Size};
use crate::{Dispatcher, Incoming, MonotonicInstant, input_device, metrics, touch_binding};
use anyhow::{Context, Error, Result};
use async_trait::async_trait;
use async_utils::hanging_get::client::HangingGetStream;
use fidl::AsHandleRef;
use fidl::endpoints::Proxy;
use fidl_fuchsia_ui_input as fidl_ui_input;
use fidl_fuchsia_ui_pointerinjector_configuration as pointerinjector_config;
use fidl_fuchsia_ui_policy as fidl_ui_policy;
use fidl_next_fuchsia_ui_pointerinjector as pointerinjector;
#[cfg(feature = "dso")]
use fidl_next_fuchsia_ui_pointerinjector_dso as pointerinjector_dso;
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc;
use futures::stream::StreamExt;
use metrics_registry::*;
use sorted_vec_map::SortedVecMap;
use std::cell::RefCell;
use std::rc::Rc;

#[cfg(feature = "dso")]
use crate::DriverTransport;
#[cfg(not(feature = "dso"))]
use crate::Transport;

/// An input handler that parses touch events and forwards them to Scenic through the
/// fidl_fuchsia_pointerinjector protocols.
pub struct TouchInjectorHandler {
    /// The mutable fields of this handler.
    mutable_state: RefCell<MutableState>,

    /// The scope and coordinate system of injection.
    /// See fidl_fuchsia_pointerinjector::Context for more details.
    context_view_ref: fidl_next_fuchsia_ui_views::ViewRef,

    /// The region where dispatch is attempted for injected events.
    /// See fidl_fuchsia_pointerinjector::Target for more details.
    target_view_ref: fidl_next_fuchsia_ui_views::ViewRef,

    /// The size of the display associated with the touch device, used to convert
    /// coordinates from the touch input report to device coordinates (which is what
    /// Scenic expects).
    display_size: Size,

    /// The FIDL proxy to register new injectors.
    #[cfg(feature = "dso")]
    injector_registry_proxy: fidl_next::Client<pointerinjector_dso::Registry, DriverTransport>,
    #[cfg(not(feature = "dso"))]
    injector_registry_proxy: fidl_next::Client<pointerinjector::Registry, Transport>,

    /// The FIDL proxy used to get configuration details for pointer injection.
    configuration_proxy: pointerinjector_config::SetupProxy,

    /// The inventory of this handler's Inspect status.
    pub inspect_status: InputHandlerStatus,

    /// The metrics logger.
    metrics_logger: metrics::MetricsLogger,
}

struct MutableState {
    /// A rectangular region that directs injected events into a target.
    /// See fidl_fuchsia_pointerinjector::Viewport for more details.
    viewport: Option<pointerinjector::Viewport>,

    /// The injectors registered with Scenic, indexed by their device ids.
    #[cfg(feature = "dso")]
    injectors: SortedVecMap<u32, fidl_next::Client<pointerinjector_dso::Device, DriverTransport>>,
    #[cfg(not(feature = "dso"))]
    injectors: SortedVecMap<u32, fidl_next::Client<pointerinjector::Device, Transport>>,

    /// The touch button listeners, key referenced by proxy channel's raw handle.
    pub listeners: SortedVecMap<u32, fidl_ui_policy::TouchButtonsListenerProxy>,

    /// The last TouchButtonsEvent sent to all listeners.
    /// This is used to send new listeners the state of the touchscreen buttons.
    pub last_button_event: Option<fidl_ui_input::TouchButtonsEvent>,

    pub send_event_task_tracker: LocalTaskTracker,
}

impl Handler for TouchInjectorHandler {
    fn set_handler_healthy(self: std::rc::Rc<Self>) {
        self.inspect_status.health_node.borrow_mut().set_ok();
    }

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, msg: &str) {
        self.inspect_status.health_node.borrow_mut().set_unhealthy(msg);
    }

    fn get_name(&self) -> &'static str {
        "TouchInjectorHandler"
    }

    fn interest(&self) -> Vec<input_device::InputEventType> {
        vec![input_device::InputEventType::TouchScreen]
    }
}

#[async_trait(?Send)]
impl BatchInputHandler for TouchInjectorHandler {
    async fn handle_input_events(
        self: Rc<Self>,
        events: Vec<input_device::InputEvent>,
    ) -> Vec<input_device::InputEvent> {
        if events.is_empty() {
            return events;
        }

        fuchsia_trace::duration!("input", "touch_injector_handler");

        let mut result: Vec<input_device::InputEvent> = Vec::new();
        let mut pending_scenic_events: Vec<pointerinjector::Event> = Vec::new();

        let device_id = events[0].device_descriptor.device_id();
        let has_different_device_events =
            events.iter().any(|e| e.device_descriptor.device_id() != device_id);
        if has_different_device_events {
            self.metrics_logger.log_error(
                InputPipelineErrorMetricDimensionEvent::TouchInjectorReceivedInputFrameContainsEventsFromMultipleDevices,
                std::format!("TouchInjectorHandler: Received events from different devices"),
            );
            return events;
        }

        for event in events {
            let (out_events, scenic_events) = self.clone().handle_single_input_event(event).await;
            result.extend(out_events);
            pending_scenic_events.extend(scenic_events);
        }

        if !pending_scenic_events.is_empty() {
            if let input_device::InputDeviceDescriptor::TouchScreen(ref touch_device_descriptor) =
                result[0].device_descriptor
            {
                if let Err(e) =
                    self.inject_pointer_events(pending_scenic_events, touch_device_descriptor)
                {
                    self.metrics_logger.log_error(
                        InputPipelineErrorMetricDimensionEvent::TouchInjectorSendEventToScenicFailed,
                        std::format!("inject_pointer_events failed: {}", e),
                    );
                }
            }
        }

        result
    }
}

impl TouchInjectorHandler {
    /// Creates a new touch handler that holds touch pointer injectors.
    /// The caller is expected to spawn a task to continually watch for updates to the viewport.
    /// Example:
    /// let handler = TouchInjectorHandler::new(display_size).await?;
    /// fasync::Task::local(handler.clone().watch_viewport()).detach();
    ///
    /// # Parameters
    /// - `display_size`: The size of the associated touch display.
    ///
    /// # Errors
    /// If unable to connect to pointerinjector protocols.
    pub async fn new(
        incoming: &Incoming,
        display_size: Size,
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<Rc<Self>, Error> {
        let configuration_proxy =
            incoming.connect_protocol::<pointerinjector_config::SetupProxy>()?;
        #[cfg(feature = "dso")]
        let injector_registry_proxy =
            incoming.connect_protocol_driver_transport::<pointerinjector_dso::Registry>()?.spawn();
        #[cfg(not(feature = "dso"))]
        let injector_registry_proxy =
            incoming.connect_protocol_next::<pointerinjector::Registry>()?.spawn();

        Self::new_handler(
            configuration_proxy,
            injector_registry_proxy,
            display_size,
            input_handlers_node,
            metrics_logger,
        )
        .await
    }

    /// Creates a new touch handler that holds touch pointer injectors.
    /// The caller is expected to spawn a task to continually watch for updates to the viewport.
    /// Example:
    /// let handler = TouchInjectorHandler::new_with_config_proxy(config_proxy, display_size).await?;
    /// fasync::Task::local(handler.clone().watch_viewport()).detach();
    ///
    /// # Parameters
    /// - `configuration_proxy`: A proxy used to get configuration details for pointer
    ///    injection.
    /// - `display_size`: The size of the associated touch display.
    ///
    /// # Errors
    /// If unable to get injection view refs from `configuration_proxy`.
    /// If unable to connect to pointerinjector Registry protocol.
    pub async fn new_with_config_proxy(
        incoming: &Incoming,
        configuration_proxy: pointerinjector_config::SetupProxy,
        display_size: Size,
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<Rc<Self>, Error> {
        #[cfg(feature = "dso")]
        let injector_registry_proxy =
            incoming.connect_protocol_driver_transport::<pointerinjector_dso::Registry>()?.spawn();
        #[cfg(not(feature = "dso"))]
        let injector_registry_proxy =
            incoming.connect_protocol_next::<pointerinjector::Registry>()?.spawn();

        Self::new_handler(
            configuration_proxy,
            injector_registry_proxy,
            display_size,
            input_handlers_node,
            metrics_logger,
        )
        .await
    }

    /// Creates a new touch handler that holds touch pointer injectors.
    /// The caller is expected to spawn a task to continually watch for updates to the viewport.
    /// Example:
    /// let handler = TouchInjectorHandler::new_handler(None, None, display_size).await?;
    /// fasync::Task::local(handler.clone().watch_viewport()).detach();
    ///
    /// # Parameters
    /// - `configuration_proxy`: A proxy used to get configuration details for pointer
    ///    injection.
    /// - `injector_registry_proxy`: A proxy used to register new pointer injectors.  If
    ///    none is provided, connect to protocol routed to this component.
    /// - `display_size`: The size of the associated touch display.
    ///
    /// # Errors
    /// If unable to get injection view refs from `configuration_proxy`.
    async fn new_handler(
        configuration_proxy: pointerinjector_config::SetupProxy,
        #[cfg(feature = "dso")] injector_registry_proxy: fidl_next::Client<
            pointerinjector_dso::Registry,
            DriverTransport,
        >,
        #[cfg(not(feature = "dso"))] injector_registry_proxy: fidl_next::Client<
            pointerinjector::Registry,
            Transport,
        >,
        display_size: Size,
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<Rc<Self>, Error> {
        // Get the context and target views to inject into.
        let (context_view_ref, target_view_ref) = configuration_proxy.get_view_refs().await?;

        let inspect_status = InputHandlerStatus::new(
            input_handlers_node,
            "touch_injector_handler",
            /* generates_events */ false,
        );
        let handler = Rc::new(Self {
            mutable_state: RefCell::new(MutableState {
                viewport: None,
                injectors: SortedVecMap::new(),
                listeners: SortedVecMap::new(),
                last_button_event: None,
                send_event_task_tracker: LocalTaskTracker::new(),
            }),
            context_view_ref: fidl_next_fuchsia_ui_views::ViewRef {
                reference: context_view_ref.reference,
            },
            target_view_ref: fidl_next_fuchsia_ui_views::ViewRef {
                reference: target_view_ref.reference,
            },
            display_size,
            injector_registry_proxy,
            configuration_proxy,
            inspect_status,
            metrics_logger,
        });

        Ok(handler)
    }

    fn clone_event(event: &fidl_ui_input::TouchButtonsEvent) -> fidl_ui_input::TouchButtonsEvent {
        // each copy of the event should have a unique trace flow id.
        let trace_flow_id = fuchsia_trace::Id::new();
        fuchsia_trace::flow_begin!("input", "dispatch_touch_button_to_listeners", trace_flow_id);

        fidl_ui_input::TouchButtonsEvent {
            event_time: event.event_time,
            device_info: event.device_info.clone(),
            pressed_buttons: event.pressed_buttons.clone(),
            wake_lease: event.wake_lease.as_ref().map(|lease| {
                lease
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("failed to duplicate event pair")
            }),
            trace_flow_id: Some(trace_flow_id.into()),
            ..Default::default()
        }
    }

    async fn handle_single_input_event(
        self: Rc<Self>,
        mut input_event: input_device::InputEvent,
    ) -> (Vec<input_device::InputEvent>, Vec<pointerinjector::Event>) {
        match input_event {
            input_device::InputEvent {
                device_event: input_device::InputDeviceEvent::TouchScreen(ref mut touch_event),
                device_descriptor:
                    input_device::InputDeviceDescriptor::TouchScreen(ref touch_device_descriptor),
                event_time,
                handled: input_device::Handled::No,
                trace_id,
            } => {
                self.inspect_status.count_received_event(&event_time);
                fuchsia_trace::duration!("input", "touch_injector_handler[processing]");
                if let Some(trace_id) = trace_id {
                    fuchsia_trace::flow_step!("input", "event_in_input_pipeline", trace_id.into());
                }

                let mut scenic_events = vec![];
                if touch_event.injector_contacts.iter().all(|(_, vec)| vec.is_empty()) {
                    let mut touch_buttons_event = Self::create_touch_buttons_event(
                        touch_event,
                        event_time,
                        &touch_device_descriptor,
                    );

                    // Send the event if the touch buttons are supported.
                    self.send_event_to_listeners(&touch_buttons_event).await;

                    // Store the sent event without any wake leases.
                    std::mem::drop(touch_buttons_event.wake_lease.take());
                    self.mutable_state.borrow_mut().last_button_event = Some(touch_buttons_event);
                } else if touch_event.pressed_buttons.is_empty() {
                    // Create a new injector if this is the first time seeing device_id.
                    if let Err(e) = self.ensure_injector_registered(&touch_device_descriptor).await
                    {
                        self.metrics_logger.log_error(
                        InputPipelineErrorMetricDimensionEvent::TouchInjectorEnsureInjectorRegisteredFailed,
                        std::format!("ensure_injector_registered failed: {}", e));
                    }

                    // Handle the event.
                    scenic_events = self.create_pointer_events(
                        touch_event,
                        &touch_device_descriptor,
                        event_time,
                    );
                }

                // Consume the input event.
                self.inspect_status.count_handled_event();
                (vec![input_event.into_handled()], scenic_events)
            }
            input_device::InputEvent {
                device_event: input_device::InputDeviceEvent::TouchScreen(_),
                handled: input_device::Handled::Yes,
                ..
            } => {
                // If a touch event is handled but reached to TouchInjectorHandler, it's expected.
                (vec![input_event], vec![])
            }
            _ => {
                log::warn!("Unhandled input event: {:?}", input_event.get_event_type());
                (vec![input_event], vec![])
            }
        }
    }

    /// Adds a new pointer injector and tracks it in `self.injectors` if one doesn't exist at
    /// `touch_descriptor.device_id`.
    ///
    /// # Parameters
    /// - `touch_descriptor`: The descriptor of the new touch device.
    async fn ensure_injector_registered(
        self: &Rc<Self>,
        touch_descriptor: &touch_binding::TouchScreenDeviceDescriptor,
    ) -> Result<(), anyhow::Error> {
        if self.mutable_state.borrow().injectors.contains_key(&touch_descriptor.device_id) {
            return Ok(());
        }

        // Create a new injector.
        let device_proxy;
        let device_server;
        #[cfg(feature = "dso")]
        {
            let (client, server) = DriverTransport::create_with_dispatcher(fdf::CurrentDispatcher);
            device_proxy =
                fidl_next::ClientEnd::<pointerinjector_dso::Device, DriverTransport>::from_untyped(
                    client,
                )
                .spawn();
            device_server =
                fidl_next::ServerEnd::<pointerinjector_dso::Device, DriverTransport>::from_untyped(
                    server,
                );
        }
        #[cfg(not(feature = "dso"))]
        {
            let (client, server) = fidl_next::fuchsia::create_channel::<pointerinjector::Device>();
            device_proxy = Dispatcher::client_from_zx_channel(client).spawn();
            device_server = server;
        }
        let context = utils::duplicate_view_ref_next(&self.context_view_ref)
            .context("Failed to duplicate context view ref.")?;
        let context = fidl_next_fuchsia_ui_views::ViewRef { reference: context.reference };
        let target = utils::duplicate_view_ref_next(&self.target_view_ref)
            .context("Failed to duplicate target view ref.")?;
        let target = fidl_next_fuchsia_ui_views::ViewRef { reference: target.reference };
        let viewport = self.mutable_state.borrow().viewport.clone();
        if viewport.is_none() {
            // An injector without a viewport is not valid. The event will be dropped
            // since the handler will not have a registered injector to inject into.
            return Err(anyhow::format_err!(
                "Received a touch event without a viewport to inject into."
            ));
        }
        let config = pointerinjector::Config {
            device_id: Some(touch_descriptor.device_id),
            device_type: Some(pointerinjector::DeviceType::Touch),
            context: Some(pointerinjector::Context::View(context)),
            target: Some(pointerinjector::Target::View(target)),
            viewport,
            dispatch_policy: Some(pointerinjector::DispatchPolicy::TopHitAndAncestorsInTarget),
            scroll_v_range: None,
            scroll_h_range: None,
            buttons: None,
            ..Default::default()
        };

        // Keep track of the injector.
        self.mutable_state.borrow_mut().injectors.insert(touch_descriptor.device_id, device_proxy);

        // Register the new injector.
        self.injector_registry_proxy
            .register(config, device_server)
            .await
            .context("Failed to register injector.")?;
        log::info!("Registered injector with device id {:?}", touch_descriptor.device_id);

        Ok(())
    }

    /// Converts the given touch event into a list of Scenic events.
    ///
    /// # Parameters
    /// - `touch_event`: The touch event to send to Scenic.
    /// - `touch_descriptor`: The descriptor for the device that sent the touch event.
    /// - `event_time`: The time when the event was first recorded.
    fn create_pointer_events(
        &self,
        touch_event: &mut touch_binding::TouchScreenEvent,
        touch_descriptor: &touch_binding::TouchScreenDeviceDescriptor,
        event_time: zx::MonotonicInstant,
    ) -> Vec<pointerinjector::Event> {
        let ordered_phases = vec![
            pointerinjector::EventPhase::Add,
            pointerinjector::EventPhase::Change,
            pointerinjector::EventPhase::Remove,
        ];

        let mut events: Vec<pointerinjector::Event> = vec![];
        for phase in ordered_phases {
            let contacts: Vec<touch_binding::TouchContact> = touch_event
                .injector_contacts
                .get(&phase)
                .map_or(vec![], |contacts| contacts.to_owned());
            let new_events = contacts.into_iter().map(|contact| {
                Self::create_pointer_sample_event(
                    phase,
                    &contact,
                    touch_descriptor,
                    &self.display_size,
                    event_time,
                    touch_event.wake_lease.take(),
                )
            });
            events.extend(new_events);
        }

        events
    }

    /// Injects the given events into Scenic.
    ///
    /// # Parameters
    /// - `events`: The events to inject.
    /// - `touch_descriptor`: The descriptor for the device that sent the touch event.
    fn inject_pointer_events(
        &self,
        events: Vec<pointerinjector::Event>,
        touch_descriptor: &touch_binding::TouchScreenDeviceDescriptor,
    ) -> Result<(), anyhow::Error> {
        fuchsia_trace::duration!("input", "touch-inject-into-scenic");

        let injector =
            self.mutable_state.borrow().injectors.get(&touch_descriptor.device_id).cloned();
        if let Some(injector) = injector {
            _ = injector.inject_events(events).send_immediately();
            Ok(())
        } else {
            Err(anyhow::format_err!(
                "No injector found for touch device {}.",
                touch_descriptor.device_id
            ))
        }
    }

    /// Creates a [`fidl_next_fuchsia_ui_pointerinjector::Event`] representing the given touch contact.
    ///
    /// # Parameters
    /// - `phase`: The phase of the touch contact.
    /// - `contact`: The touch contact to create the event for.
    /// - `touch_descriptor`: The device descriptor for the device that generated the event.
    /// - `display_size`: The size of the associated touch display.
    /// - `event_time`: The time in nanoseconds when the event was first recorded.
    /// - `wake_lease`: The wake lease for this event.
    fn create_pointer_sample_event(
        phase: pointerinjector::EventPhase,
        contact: &touch_binding::TouchContact,
        touch_descriptor: &touch_binding::TouchScreenDeviceDescriptor,
        display_size: &Size,
        event_time: zx::MonotonicInstant,
        wake_lease: Option<zx::EventPair>,
    ) -> pointerinjector::Event {
        let position =
            Self::display_coordinate_from_contact(&contact, &touch_descriptor, display_size);
        let pointer_sample = pointerinjector::PointerSample {
            pointer_id: Some(contact.id),
            phase: Some(phase),
            position_in_viewport: Some([position.x, position.y]),
            scroll_v: None,
            scroll_h: None,
            pressed_buttons: None,
            ..Default::default()
        };
        let data = pointerinjector::Data::PointerSample(pointer_sample);

        let trace_flow_id = fuchsia_trace::Id::new();
        let event = pointerinjector::Event {
            timestamp: Some(event_time.into_nanos()),
            data: Some(data),
            trace_flow_id: Some(trace_flow_id.into()),
            wake_lease,
            ..Default::default()
        };

        fuchsia_trace::flow_begin!("input", "dispatch_event_to_scenic", trace_flow_id);

        event
    }

    /// Converts an input event touch to a display coordinate, which is the coordinate space in
    /// which Scenic handles events.
    ///
    /// The display coordinate is calculated by normalizing the contact position to the display
    /// size. It does not account for the viewport position, which Scenic handles directly.
    ///
    /// # Parameters
    /// - `contact`: The contact to get the display coordinate from.
    /// - `touch_descriptor`: The device descriptor for the device that generated the event.
    ///                       This is used to compute the device coordinate.
    ///
    /// # Returns
    /// (x, y) coordinates.
    fn display_coordinate_from_contact(
        contact: &touch_binding::TouchContact,
        touch_descriptor: &touch_binding::TouchScreenDeviceDescriptor,
        display_size: &Size,
    ) -> Position {
        if let Some(contact_descriptor) = touch_descriptor.contacts.first() {
            // Scale the x position.
            let x_range: f32 =
                contact_descriptor.x_range.max as f32 - contact_descriptor.x_range.min as f32;
            let x_wrt_range: f32 = contact.position.x - contact_descriptor.x_range.min as f32;
            let x: f32 = (display_size.width * x_wrt_range) / x_range;

            // Scale the y position.
            let y_range: f32 =
                contact_descriptor.y_range.max as f32 - contact_descriptor.y_range.min as f32;
            let y_wrt_range: f32 = contact.position.y - contact_descriptor.y_range.min as f32;
            let y: f32 = (display_size.height * y_wrt_range) / y_range;

            Position { x, y }
        } else {
            return contact.position;
        }
    }

    /// Watches for viewport updates from the scene manager.
    pub async fn watch_viewport(self: Rc<Self>) {
        let configuration_proxy = self.configuration_proxy.clone();
        let mut viewport_stream = HangingGetStream::new(
            configuration_proxy,
            pointerinjector_config::SetupProxy::watch_viewport,
        );
        loop {
            match viewport_stream.next().await {
                Some(Ok(new_viewport)) => {
                    // Update the viewport tracked by this handler.
                    self.mutable_state.borrow_mut().viewport =
                        Some(utils::viewport_to_next(&new_viewport));

                    // Update Scenic with the latest viewport.
                    let injectors: Vec<fidl_next::Client<_, _>> = self
                        .mutable_state
                        .borrow()
                        .injectors
                        .iter()
                        .map(|(_, v)| v)
                        .cloned()
                        .collect();
                    for injector in injectors {
                        let events = vec![pointerinjector::Event {
                            timestamp: Some(MonotonicInstant::now().into_nanos()),
                            data: Some(pointerinjector::Data::Viewport(utils::viewport_to_next(
                                &new_viewport,
                            ))),
                            trace_flow_id: Some(fuchsia_trace::Id::new().into()),
                            ..Default::default()
                        }];
                        injector
                            .inject_events(events)
                            .send_immediately()
                            .expect("Failed to inject updated viewport.");
                    }
                }
                Some(Err(e)) => {
                    self.metrics_logger.log_error(
                        InputPipelineErrorMetricDimensionEvent::TouchInjectorErrorWhileReadingViewportUpdate,
                        std::format!("Error while reading viewport update: {}", e));
                    return;
                }
                None => {
                    self.metrics_logger.log_error(
                        InputPipelineErrorMetricDimensionEvent::TouchInjectorViewportUpdateStreamTerminatedUnexpectedly,
                        "Viewport update stream terminated unexpectedly");
                    return;
                }
            }
        }
    }

    /// Creates a fidl_ui_input::TouchButtonsEvent from a touch_binding::TouchScreenEvent.
    ///
    /// # Parameters
    /// - `event`: The TouchScreenEvent to create a TouchButtonsEvent from.
    /// - `event_time`: The time when the event was first recorded.
    /// - `touch_descriptor`: The descriptor of the new touch device.
    fn create_touch_buttons_event(
        event: &mut touch_binding::TouchScreenEvent,
        event_time: zx::MonotonicInstant,
        touch_descriptor: &touch_binding::TouchScreenDeviceDescriptor,
    ) -> fidl_ui_input::TouchButtonsEvent {
        let pressed_buttons = match event.pressed_buttons.len() {
            0 => None,
            _ => Some(
                event
                    .pressed_buttons
                    .clone()
                    .into_iter()
                    .map(|button| match button {
                        fidl_next_fuchsia_input_report::TouchButton::Palm => {
                            fidl_ui_input::TouchButton::Palm
                        }
                        fidl_next_fuchsia_input_report::TouchButton::SwipeUp => {
                            fidl_ui_input::TouchButton::SwipeUp
                        }
                        fidl_next_fuchsia_input_report::TouchButton::SwipeLeft => {
                            fidl_ui_input::TouchButton::SwipeLeft
                        }
                        fidl_next_fuchsia_input_report::TouchButton::SwipeRight => {
                            fidl_ui_input::TouchButton::SwipeRight
                        }
                        fidl_next_fuchsia_input_report::TouchButton::SwipeDown => {
                            fidl_ui_input::TouchButton::SwipeDown
                        }
                        fidl_next_fuchsia_input_report::TouchButton::UnknownOrdinal_(n) => {
                            fidl_ui_input::TouchButton::__SourceBreaking {
                                unknown_ordinal: n as u32,
                            }
                        }
                    })
                    .collect::<Vec<_>>(),
            ),
        };
        fidl_ui_input::TouchButtonsEvent {
            event_time: Some(event_time),
            device_info: Some(fidl_ui_input::TouchDeviceInfo {
                id: Some(touch_descriptor.device_id),
                ..Default::default()
            }),
            pressed_buttons,
            wake_lease: event.wake_lease.take(),
            ..Default::default()
        }
    }

    /// Sends touch button events to touch button listeners.
    ///
    /// # Parameters
    /// - `event`: The event to send to the listeners.
    async fn send_event_to_listeners(self: &Rc<Self>, event: &fidl_ui_input::TouchButtonsEvent) {
        let tracker = &self.mutable_state.borrow().send_event_task_tracker;

        for (handle, listener) in self.mutable_state.borrow().listeners.iter() {
            let weak_handler = Rc::downgrade(&self);
            let listener_clone = listener.clone();
            let handle_clone = handle.clone();
            let event_to_send = Self::clone_event(event);
            let fut = async move {
                match listener_clone.on_event(event_to_send).await {
                    Ok(_) => {}
                    Err(e) => {
                        if let Some(handler) = weak_handler.upgrade() {
                            handler.mutable_state.borrow_mut().listeners.remove(&handle_clone);
                            log::info!(
                                "Unregistering listener; unable to send TouchButtonsEvent: {:?}",
                                e
                            )
                        }
                    }
                }
            };

            let metrics_logger_clone = self.metrics_logger.clone();
            tracker.track(metrics_logger_clone, Dispatcher::spawn_local(fut));
        }
    }

    // Add the listener to the registry.
    ///
    /// # Parameters
    /// - `proxy`: A new listener proxy to send events to.
    pub async fn register_listener_proxy(
        self: &Rc<Self>,
        proxy: fidl_ui_policy::TouchButtonsListenerProxy,
    ) {
        self.mutable_state
            .borrow_mut()
            .listeners
            .insert(proxy.as_channel().as_handle_ref().raw_handle(), proxy.clone());

        // Send the listener the last touch button event.
        if let Some(event) = &self.mutable_state.borrow().last_button_event {
            let event_to_send = Self::clone_event(event);
            let fut = async move {
                match proxy.on_event(event_to_send).await {
                    Ok(_) => {}
                    Err(e) => {
                        log::info!("Failed to send touch buttons event to listener {:?}", e)
                    }
                }
            };
            let metrics_logger_clone = self.metrics_logger.clone();
            self.mutable_state
                .borrow()
                .send_event_task_tracker
                .track(metrics_logger_clone, Dispatcher::spawn_local(fut));
        }
    }
}

/// Maintains a collection of pending local [`Task`]s, allowing them to be dropped (and cancelled)
/// en masse.
#[derive(Debug)]
pub struct LocalTaskTracker {
    sender: mpsc::UnboundedSender<TaskHandle<()>>,
    _receiver_task: TaskHandle<()>,
}

impl LocalTaskTracker {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded();
        let _receiver_task = Dispatcher::spawn_local(async move {
            // Drop the tasks as they are completed.
            receiver.for_each_concurrent(None, |task: TaskHandle<()>| task).await
        });

        Self { sender, _receiver_task }
    }

    /// Submits a new task to track.
    pub fn track(&self, metrics_logger: metrics::MetricsLogger, task: TaskHandle<()>) {
        match self.sender.unbounded_send(task) {
            Ok(_) => {}
            // `Full` should never happen because this is unbounded.
            // `Disconnected` might happen if the `Service` was dropped. However, it's not clear how
            // to create such a race condition.
            Err(e) => {
                metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::TouchFailedToSendTouchScreenEvent,
                    std::format!("Unexpected {e:?} while pushing task"),
                );
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_handler::BatchInputHandler;
    use crate::testing_utilities::{
        create_fake_input_event, create_touch_contact, create_touch_pointer_sample_event,
        create_touch_screen_event, create_touch_screen_event_with_handled, create_touchpad_event,
        get_touch_screen_device_descriptor, next_client_old_stream,
    };
    use assert_matches::assert_matches;
    use fidl_fuchsia_ui_input as fidl_ui_input;
    use fidl_fuchsia_ui_pointerinjector as pointerinjector;
    use fidl_fuchsia_ui_policy as fidl_ui_policy;
    use fidl_next_fuchsia_ui_pointerinjector as pointerinjector_next;
    use fuchsia_async as fasync;
    use futures::{FutureExt, TryStreamExt};
    use pretty_assertions::assert_eq;
    use sorted_vec_map::SortedVecSet;
    use std::convert::TryFrom as _;
    use std::ops::Add;

    const TOUCH_ID: u32 = 1;
    const DISPLAY_WIDTH: f32 = 100.0;
    const DISPLAY_HEIGHT: f32 = 100.0;

    struct TestFixtures {
        touch_handler: Rc<TouchInjectorHandler>,
        device_listener_proxy: fidl_ui_policy::DeviceListenerRegistryProxy,
        injector_registry_request_stream: pointerinjector::RegistryRequestStream,
        configuration_request_stream: pointerinjector_config::SetupRequestStream,
        inspector: fuchsia_inspect::Inspector,
        _test_node: fuchsia_inspect::Node,
        _server_task: fasync::Task<()>,
    }

    fn spawn_device_listener_registry_server(
        handler: Rc<TouchInjectorHandler>,
    ) -> (fidl_ui_policy::DeviceListenerRegistryProxy, fasync::Task<()>) {
        let (device_listener_proxy, mut device_listener_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_ui_policy::DeviceListenerRegistryMarker>(
            );

        let task = fasync::Task::local(async move {
            loop {
                match device_listener_stream.try_next().await {
                    Ok(Some(
                        fidl_ui_policy::DeviceListenerRegistryRequest::RegisterTouchButtonsListener {
                            listener,
                            responder,
                        },
                    )) => {
                        handler.register_listener_proxy(listener.into_proxy()).await;
                        let _ = responder.send();
                    }
                    Ok(Some(_)) => {
                        panic!("Unexpected registration");
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(e) => {
                        panic!("Error handling device listener registry request stream: {}", e);
                    }
                }
            }
        });

        (device_listener_proxy, task)
    }

    impl TestFixtures {
        async fn new() -> Self {
            let inspector = fuchsia_inspect::Inspector::default();
            let test_node = inspector.root().create_child("test_node");
            let (configuration_proxy, mut configuration_request_stream) =
                fidl::endpoints::create_proxy_and_stream::<pointerinjector_config::SetupMarker>();
            let (injector_registry_proxy, injector_registry_request_stream) =
                next_client_old_stream::<
                    pointerinjector::RegistryMarker,
                    pointerinjector_next::Registry,
                >();

            let touch_handler_fut = TouchInjectorHandler::new_handler(
                configuration_proxy,
                injector_registry_proxy,
                Size { width: DISPLAY_WIDTH, height: DISPLAY_HEIGHT },
                &test_node,
                metrics::MetricsLogger::default(),
            );

            let handle_initial_request_fut = async {
                match configuration_request_stream.next().await {
                    Some(Ok(pointerinjector_config::SetupRequest::GetViewRefs {
                        responder,
                        ..
                    })) => {
                        let context = fuchsia_scenic::ViewRefPair::new()
                            .expect("Failed to create viewrefpair.")
                            .view_ref;
                        let target = fuchsia_scenic::ViewRefPair::new()
                            .expect("Failed to create viewrefpair.")
                            .view_ref;
                        let _ = responder.send(context, target);
                    }
                    other => panic!("Expected GetViewRefs request, got {:?}", other),
                }
            };

            let (touch_handler_res, _) =
                futures::future::join(touch_handler_fut, handle_initial_request_fut).await;

            let touch_handler = touch_handler_res.expect("Failed to create touch handler.");
            let (device_listener_proxy, server_task) =
                spawn_device_listener_registry_server(touch_handler.clone());

            TestFixtures {
                touch_handler,
                device_listener_proxy,
                injector_registry_request_stream,
                configuration_request_stream,
                inspector,
                _test_node: test_node,
                _server_task: server_task,
            }
        }
    }

    /// Returns an |input_device::InputDeviceDescriptor::Touchpad|.
    fn get_touchpad_device_descriptor() -> input_device::InputDeviceDescriptor {
        input_device::InputDeviceDescriptor::Touchpad(touch_binding::TouchpadDeviceDescriptor {
            device_id: 1,
            contacts: vec![touch_binding::ContactDeviceDescriptor {
                x_range: fidl_fuchsia_input::Range { min: 0, max: 100 },
                y_range: fidl_fuchsia_input::Range { min: 0, max: 100 },
                x_unit: fidl_fuchsia_input::Unit {
                    type_: fidl_fuchsia_input::UnitType::Meters,
                    exponent: -6,
                },
                y_unit: fidl_fuchsia_input::Unit {
                    type_: fidl_fuchsia_input::UnitType::Meters,
                    exponent: -6,
                },
                pressure_range: None,
                width_range: None,
                height_range: None,
            }],
        })
    }

    /// Handles |fidl_fuchsia_pointerinjector::DeviceRequest|s by asserting the `injector_stream`
    /// gets `expected_event`.
    async fn handle_device_request_stream(
        mut injector_stream: pointerinjector::DeviceRequestStream,
        expected_event: pointerinjector::Event,
    ) {
        match injector_stream.next().await {
            Some(Ok(pointerinjector::DeviceRequest::Inject { .. })) => {
                panic!("DeviceRequest::Inject is deprecated.");
            }
            Some(Ok(pointerinjector::DeviceRequest::InjectEvents { events, .. })) => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].timestamp, expected_event.timestamp);
                assert_eq!(events[0].data, expected_event.data);
            }
            Some(Err(e)) => panic!("FIDL error {}", e),
            None => panic!("Expected another event."),
        }
    }

    fn create_viewport(min: f32, max: f32) -> pointerinjector::Viewport {
        pointerinjector::Viewport {
            extents: Some([[min, min], [max, max]]),
            viewport_to_context_transform: None,
            ..Default::default()
        }
    }

    fn create_viewport_next(min: f32, max: f32) -> pointerinjector_next::Viewport {
        pointerinjector_next::Viewport {
            extents: Some([[min, min], [max, max]]),
            viewport_to_context_transform: None,
            ..Default::default()
        }
    }

    #[fuchsia::test]
    async fn events_with_pressed_buttons_are_sent_to_listener() {
        let fixtures = TestFixtures::new().await;
        let (listener, mut listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::TouchButtonsListenerMarker>();
        fixtures
            .device_listener_proxy
            .register_touch_buttons_listener(listener)
            .await
            .expect("Failed to register listener.");

        let descriptor = get_touch_screen_device_descriptor();
        let event_time = zx::MonotonicInstant::get();
        let input_event = create_touch_screen_event(SortedVecMap::new(), event_time, &descriptor);

        let _ = fixtures.touch_handler.clone().handle_input_events(vec![input_event]).await;

        let expected_touch_buttons_event = fidl_ui_input::TouchButtonsEvent {
            event_time: Some(event_time),
            device_info: Some(fidl_ui_input::TouchDeviceInfo { id: Some(1), ..Default::default() }),
            ..Default::default()
        };

        assert_matches!(
            listener_stream.next().await,
            Some(Ok(fidl_ui_policy::TouchButtonsListenerRequest::OnEvent {
                event,
                responder,
            })) => {
                assert_eq!(event.event_time, expected_touch_buttons_event.event_time);
                assert_eq!(event.device_info, expected_touch_buttons_event.device_info);
                assert_eq!(event.pressed_buttons, expected_touch_buttons_event.pressed_buttons);
                assert!(event.trace_flow_id.is_some());
                let _ = responder.send();
            }
        );
    }

    #[fuchsia::test]
    async fn events_with_contacts_are_not_sent_to_listener() {
        let fixtures = TestFixtures::new().await;
        let (listener, mut listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::TouchButtonsListenerMarker>();
        fixtures
            .device_listener_proxy
            .register_touch_buttons_listener(listener)
            .await
            .expect("Failed to register listener.");

        let descriptor = get_touch_screen_device_descriptor();
        let event_time = zx::MonotonicInstant::get();
        let contact = create_touch_contact(TOUCH_ID, Position { x: 20.0, y: 40.0 });
        let input_event = create_touch_screen_event(
            SortedVecMap::from_iter(vec![(
                fidl_ui_input::PointerEventPhase::Add,
                vec![contact.clone()],
            )]),
            event_time,
            &descriptor,
        );

        let _ = fixtures.touch_handler.clone().handle_input_events(vec![input_event]).await;

        assert!(listener_stream.next().now_or_never().is_none());
    }

    #[fuchsia::test]
    async fn multiple_listeners_receive_pressed_button_events() {
        let fixtures = TestFixtures::new().await;
        let (first_listener, mut first_listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::TouchButtonsListenerMarker>();
        let (second_listener, mut second_listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::TouchButtonsListenerMarker>();
        fixtures
            .device_listener_proxy
            .register_touch_buttons_listener(first_listener)
            .await
            .expect("Failed to register listener.");
        fixtures
            .device_listener_proxy
            .register_touch_buttons_listener(second_listener)
            .await
            .expect("Failed to register listener.");

        let descriptor = get_touch_screen_device_descriptor();
        let event_time = zx::MonotonicInstant::get();
        let input_event = create_touch_screen_event(SortedVecMap::new(), event_time, &descriptor);

        let _ = fixtures.touch_handler.clone().handle_input_events(vec![input_event]).await;

        let expected_touch_buttons_event = fidl_ui_input::TouchButtonsEvent {
            event_time: Some(event_time),
            device_info: Some(fidl_ui_input::TouchDeviceInfo { id: Some(1), ..Default::default() }),
            ..Default::default()
        };

        assert_matches!(
            first_listener_stream.next().await,
            Some(Ok(fidl_ui_policy::TouchButtonsListenerRequest::OnEvent {
                event,
                responder,
            })) => {
                assert_eq!(event.event_time, expected_touch_buttons_event.event_time);
                assert_eq!(event.device_info, expected_touch_buttons_event.device_info);
                assert_eq!(event.pressed_buttons, expected_touch_buttons_event.pressed_buttons);
                assert!(event.trace_flow_id.is_some());
                let _ = responder.send();
            }
        );
        assert_matches!(
            second_listener_stream.next().await,
            Some(Ok(fidl_ui_policy::TouchButtonsListenerRequest::OnEvent {
                event,
                responder,
            })) => {
                assert_eq!(event.event_time, expected_touch_buttons_event.event_time);
                assert_eq!(event.device_info, expected_touch_buttons_event.device_info);
                assert_eq!(event.pressed_buttons, expected_touch_buttons_event.pressed_buttons);
                assert!(event.trace_flow_id.is_some());
                let _ = responder.send();
            }
        );
    }

    // Tests that TouchInjectorHandler::watch_viewport() tracks viewport updates and notifies
    // injectors about said updates.
    #[fuchsia::test]
    async fn receives_viewport_updates() {
        let mut fixtures = TestFixtures::new().await;

        // Add an injector.
        let (injector_device_proxy, mut injector_device_request_stream) =
            next_client_old_stream::<pointerinjector::DeviceMarker, pointerinjector_next::Device>();
        fixtures
            .touch_handler
            .mutable_state
            .borrow_mut()
            .injectors
            .insert(1, injector_device_proxy);

        // This nested block is used to bound the lifetime of `watch_viewport_fut`.
        {
            // Request a viewport update.
            let _watch_viewport_task =
                fasync::Task::local(fixtures.touch_handler.clone().watch_viewport());

            // Send a viewport update.
            match fixtures.configuration_request_stream.next().await {
                Some(Ok(pointerinjector_config::SetupRequest::WatchViewport {
                    responder, ..
                })) => {
                    responder.send(&create_viewport(0.0, 100.0)).expect("Failed to send viewport.");
                }
                other => panic!("Received unexpected value: {:?}", other),
            };

            // Check that the injector received an updated viewport
            match injector_device_request_stream.next().await {
                Some(Ok(pointerinjector::DeviceRequest::Inject { .. })) => {
                    panic!("DeviceRequest::Inject is deprecated.");
                }
                Some(Ok(pointerinjector::DeviceRequest::InjectEvents { events, .. })) => {
                    assert_eq!(events.len(), 1);
                    assert!(events[0].data.is_some());
                    assert_eq!(
                        events[0].data,
                        Some(pointerinjector::Data::Viewport(create_viewport(0.0, 100.0)))
                    );
                }
                other => panic!("Received unexpected value: {:?}", other),
            }

            // Request viewport update.
            // Send viewport update.
            match fixtures.configuration_request_stream.next().await {
                Some(Ok(pointerinjector_config::SetupRequest::WatchViewport {
                    responder, ..
                })) => {
                    responder
                        .send(&create_viewport(100.0, 200.0))
                        .expect("Failed to send viewport.");
                }
                other => panic!("Received unexpected value: {:?}", other),
            };

            // Check that the injector received an updated viewport
            match injector_device_request_stream.next().await {
                Some(Ok(pointerinjector::DeviceRequest::Inject { .. })) => {
                    panic!("DeviceRequest::Inject is deprecated.");
                }
                Some(Ok(pointerinjector::DeviceRequest::InjectEvents { events, .. })) => {
                    assert_eq!(events.len(), 1);
                    assert!(events[0].data.is_some());
                    assert_eq!(
                        events[0].data,
                        Some(pointerinjector::Data::Viewport(create_viewport(100.0, 200.0)))
                    );
                }
                other => panic!("Received unexpected value: {:?}", other),
            }
        }

        // Check the viewport on the handler is accurate.
        let expected_viewport = create_viewport_next(100.0, 200.0);
        assert_eq!(fixtures.touch_handler.mutable_state.borrow().viewport, Some(expected_viewport));
    }

    // Tests that an add contact event is dropped without a viewport.
    #[fuchsia::test]
    async fn add_contact_drops_without_viewport() {
        let mut fixtures = TestFixtures::new().await;

        // Create touch event.
        let event_time = zx::MonotonicInstant::get();
        let contact = create_touch_contact(TOUCH_ID, Position { x: 20.0, y: 40.0 });
        let descriptor = get_touch_screen_device_descriptor();
        let input_event = input_device::UnhandledInputEvent::try_from(create_touch_screen_event(
            SortedVecMap::from_iter(vec![(
                fidl_ui_input::PointerEventPhase::Add,
                vec![contact.clone()],
            )]),
            event_time,
            &descriptor,
        ))
        .unwrap();

        // Clear the viewport that was set during test fixture setup.
        fixtures.touch_handler.mutable_state.borrow_mut().viewport = None;

        // Try to handle the event.
        let _ = fixtures.touch_handler.clone().handle_input_events(vec![input_event.into()]).await;

        // Injector should not receive anything because the handler has no viewport.
        assert!(fixtures.injector_registry_request_stream.next().now_or_never().is_none());
    }

    // Tests that an add contact event is handled correctly with a viewport.
    #[fuchsia::test]
    async fn add_contact_succeeds_with_viewport() {
        let mut fixtures = TestFixtures::new().await;

        // Add an injector.
        let (injector_device_proxy, mut injector_device_request_stream) =
            next_client_old_stream::<pointerinjector::DeviceMarker, pointerinjector_next::Device>();
        fixtures
            .touch_handler
            .mutable_state
            .borrow_mut()
            .injectors
            .insert(1, injector_device_proxy);

        // Request a viewport update.
        let _watch_viewport_task =
            fasync::Task::local(fixtures.touch_handler.clone().watch_viewport());

        // Send a viewport update.
        match fixtures.configuration_request_stream.next().await {
            Some(Ok(pointerinjector_config::SetupRequest::WatchViewport { responder, .. })) => {
                responder.send(&create_viewport(0.0, 100.0)).expect("Failed to send viewport.");
            }
            other => panic!("Received unexpected value: {:?}", other),
        };

        // Check that the injector received an updated viewport
        match injector_device_request_stream.next().await {
            Some(Ok(pointerinjector::DeviceRequest::Inject { .. })) => {
                panic!("DeviceRequest::Inject is deprecated.");
            }
            Some(Ok(pointerinjector::DeviceRequest::InjectEvents { events, .. })) => {
                assert_eq!(events.len(), 1);
                assert!(events[0].data.is_some());
                assert_eq!(
                    events[0].data,
                    Some(pointerinjector::Data::Viewport(create_viewport(0.0, 100.0)))
                );
            }
            other => panic!("Received unexpected value: {:?}", other),
        }

        // Create touch event.
        let event_time = zx::MonotonicInstant::get();
        let contact = create_touch_contact(TOUCH_ID, Position { x: 20.0, y: 40.0 });
        let descriptor = get_touch_screen_device_descriptor();
        let input_event = input_device::UnhandledInputEvent::try_from(create_touch_screen_event(
            SortedVecMap::from_iter(vec![(
                fidl_ui_input::PointerEventPhase::Add,
                vec![contact.clone()],
            )]),
            event_time,
            &descriptor,
        ))
        .unwrap();

        // Handle event.
        let handle_event_fut =
            fixtures.touch_handler.clone().handle_input_events(vec![input_event.into()]);

        // Declare expected event.
        let expected_event = create_touch_pointer_sample_event(
            pointerinjector::EventPhase::Add,
            &contact,
            Position { x: 20.0, y: 40.0 },
            event_time,
        );

        // Await all futures concurrently. If this completes, then the touch event was handled and
        // matches `expected_event`.
        let device_fut =
            handle_device_request_stream(injector_device_request_stream, expected_event);
        let (handle_result, _) = futures::future::join(handle_event_fut, device_fut).await;

        // No unhandled events.
        assert_matches!(
            handle_result.as_slice(),
            [input_device::InputEvent { handled: input_device::Handled::Yes, .. }]
        );
    }

    // Tests that an add touchpad contact event with viewport is unhandled and not send to scenic.
    #[fuchsia::test]
    async fn add_touchpad_contact_with_viewport() {
        let mut fixtures = TestFixtures::new().await;

        // Add an injector.
        let (injector_device_proxy, mut injector_device_request_stream) =
            next_client_old_stream::<pointerinjector::DeviceMarker, pointerinjector_next::Device>();
        fixtures
            .touch_handler
            .mutable_state
            .borrow_mut()
            .injectors
            .insert(1, injector_device_proxy);

        // Request a viewport update.
        let _watch_viewport_task =
            fasync::Task::local(fixtures.touch_handler.clone().watch_viewport());

        // Send a viewport update.
        match fixtures.configuration_request_stream.next().await {
            Some(Ok(pointerinjector_config::SetupRequest::WatchViewport { responder, .. })) => {
                responder.send(&create_viewport(0.0, 100.0)).expect("Failed to send viewport.");
            }
            other => panic!("Received unexpected value: {:?}", other),
        };

        // Check that the injector received an updated viewport
        match injector_device_request_stream.next().await {
            Some(Ok(pointerinjector::DeviceRequest::Inject { .. })) => {
                panic!("DeviceRequest::Inject is deprecated.");
            }
            Some(Ok(pointerinjector::DeviceRequest::InjectEvents { events, .. })) => {
                assert_eq!(events.len(), 1);
                assert!(events[0].data.is_some());
                assert_eq!(
                    events[0].data,
                    Some(pointerinjector::Data::Viewport(create_viewport(0.0, 100.0)))
                );
            }
            other => panic!("Received unexpected value: {:?}", other),
        }

        // Create touch event.
        let event_time = zx::MonotonicInstant::get();
        let contact = create_touch_contact(TOUCH_ID, Position { x: 20.0, y: 40.0 });
        let descriptor = get_touchpad_device_descriptor();
        let input_event = input_device::UnhandledInputEvent::try_from(create_touchpad_event(
            vec![contact.clone()],
            SortedVecSet::new(),
            event_time,
            &descriptor,
        ))
        .unwrap();

        // Handle event.
        let handle_event_fut =
            fixtures.touch_handler.clone().handle_input_events(vec![input_event.into()]);

        let handle_result = handle_event_fut.await;

        // Event is not handled.
        assert_matches!(
            handle_result.as_slice(),
            [input_device::InputEvent { handled: input_device::Handled::No, .. }]
        );

        // Injector should not receive anything because the handler does not support touchpad yet.
        assert!(fixtures.injector_registry_request_stream.next().now_or_never().is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn touch_injector_handler_initialized_with_inspect_node() {
        let fixtures = TestFixtures::new().await;
        diagnostics_assertions::assert_data_tree!(fixtures.inspector, root: {
            test_node: {
                touch_injector_handler: {
                    events_received_count: 0u64,
                    events_handled_count: 0u64,
                    last_received_timestamp_ns: 0u64,
                    "fuchsia.inspect.Health": {
                        status: "STARTING_UP",
                        // Timestamp value is unpredictable and not relevant in this context,
                        // so we only assert that the property is present.
                        start_timestamp_nanos: diagnostics_assertions::AnyProperty
                    },
                }
            }
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn touch_injector_handler_inspect_counts_events() {
        let fixtures = TestFixtures::new().await;

        let contact = create_touch_contact(TOUCH_ID, Position { x: 20.0, y: 40.0 });
        let descriptor = get_touch_screen_device_descriptor();
        let event_time1 = zx::MonotonicInstant::get();
        let event_time2 = event_time1.add(zx::MonotonicDuration::from_micros(1));
        let event_time3 = event_time2.add(zx::MonotonicDuration::from_micros(1));

        let input_events = vec![
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![(
                    fidl_ui_input::PointerEventPhase::Add,
                    vec![contact.clone()],
                )]),
                event_time1,
                &descriptor,
            ),
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![(
                    fidl_ui_input::PointerEventPhase::Move,
                    vec![contact.clone()],
                )]),
                event_time2,
                &descriptor,
            ),
            // Should not count non-touch input event.
            create_fake_input_event(event_time2),
            // Should not count received event that has already been handled.
            create_touch_screen_event_with_handled(
                SortedVecMap::from_iter(vec![(
                    fidl_ui_input::PointerEventPhase::Move,
                    vec![contact.clone()],
                )]),
                event_time2,
                &descriptor,
                input_device::Handled::Yes,
            ),
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![(
                    fidl_ui_input::PointerEventPhase::Remove,
                    vec![contact.clone()],
                )]),
                event_time3,
                &descriptor,
            ),
        ];

        for input_event in input_events {
            fixtures.touch_handler.clone().handle_input_events(vec![input_event]).await;
        }

        let last_received_event_time: u64 = event_time3.into_nanos().try_into().unwrap();

        diagnostics_assertions::assert_data_tree!(fixtures.inspector, root: {
            test_node: {
                touch_injector_handler: {
                    events_received_count: 3u64,
                    events_handled_count: 3u64,
                    last_received_timestamp_ns: last_received_event_time,
                    "fuchsia.inspect.Health": {
                        status: "STARTING_UP",
                        // Timestamp value is unpredictable and not relevant in this context,
                        // so we only assert that the property is present.
                        start_timestamp_nanos: diagnostics_assertions::AnyProperty
                    },
                }
            }
        });
    }

    #[fuchsia::test]
    async fn clone_event_with_lease_duplicates_lease() {
        let (event_pair, _) = fidl::EventPair::create();
        let event = fidl_ui_input::TouchButtonsEvent {
            event_time: Some(zx::MonotonicInstant::from_nanos(1)),
            device_info: Some(fidl_ui_input::TouchDeviceInfo { id: Some(1), ..Default::default() }),
            pressed_buttons: Some(vec![fidl_ui_input::TouchButton::Palm]),
            wake_lease: Some(event_pair),
            ..Default::default()
        };
        let cloned_event = TouchInjectorHandler::clone_event(&event);
        assert_eq!(event.event_time, cloned_event.event_time);
        assert_eq!(event.device_info, cloned_event.device_info);
        assert_eq!(event.pressed_buttons, cloned_event.pressed_buttons);
        assert!(event.wake_lease.is_some());
        assert!(cloned_event.wake_lease.is_some());
        assert_ne!(
            event.wake_lease.as_ref().unwrap().as_handle_ref().raw_handle(),
            cloned_event.wake_lease.as_ref().unwrap().as_handle_ref().raw_handle()
        );
    }

    #[fuchsia::test]
    async fn clone_event_without_lease_has_no_lease() {
        let event = fidl_ui_input::TouchButtonsEvent {
            event_time: Some(zx::MonotonicInstant::from_nanos(1)),
            device_info: Some(fidl_ui_input::TouchDeviceInfo { id: Some(1), ..Default::default() }),
            pressed_buttons: Some(vec![fidl_ui_input::TouchButton::Palm]),
            wake_lease: None,
            ..Default::default()
        };
        let cloned_event = TouchInjectorHandler::clone_event(&event);
        assert_eq!(event.event_time, cloned_event.event_time);
        assert_eq!(event.device_info, cloned_event.device_info);
        assert_eq!(event.pressed_buttons, cloned_event.pressed_buttons);
        assert!(event.wake_lease.is_none());
        assert!(cloned_event.wake_lease.is_none());
    }

    #[fuchsia::test]
    async fn clone_event_creates_new_trace_id() {
        let event = fidl_ui_input::TouchButtonsEvent {
            event_time: Some(zx::MonotonicInstant::from_nanos(1)),
            device_info: Some(fidl_ui_input::TouchDeviceInfo { id: Some(1), ..Default::default() }),
            pressed_buttons: Some(vec![fidl_ui_input::TouchButton::Palm]),
            trace_flow_id: Some(123),
            ..Default::default()
        };
        let cloned_event = TouchInjectorHandler::clone_event(&event);
        assert_eq!(event.event_time, cloned_event.event_time);
        assert_eq!(event.device_info, cloned_event.device_info);
        assert_eq!(event.pressed_buttons, cloned_event.pressed_buttons);
        assert!(cloned_event.trace_flow_id.is_some());
        assert_ne!(event.trace_flow_id, cloned_event.trace_flow_id);
    }

    #[fuchsia::test]
    async fn handle_input_events_batches_events() {
        let mut fixtures = TestFixtures::new().await;

        // Add an injector.
        let (injector_device_proxy, mut injector_device_request_stream) =
            next_client_old_stream::<pointerinjector::DeviceMarker, pointerinjector_next::Device>();
        fixtures
            .touch_handler
            .mutable_state
            .borrow_mut()
            .injectors
            .insert(1, injector_device_proxy);

        // Request a viewport update.
        let _watch_viewport_task =
            fasync::Task::local(fixtures.touch_handler.clone().watch_viewport());

        // Send a viewport update.
        match fixtures.configuration_request_stream.next().await {
            Some(Ok(pointerinjector_config::SetupRequest::WatchViewport { responder, .. })) => {
                responder.send(&create_viewport(0.0, 100.0)).expect("Failed to send viewport.");
            }
            other => panic!("Received unexpected value: {:?}", other),
        };

        // Check that the injector received an updated viewport
        match injector_device_request_stream.next().await {
            Some(Ok(pointerinjector::DeviceRequest::InjectEvents { events, .. })) => {
                assert_eq!(events.len(), 1);
                assert!(events[0].data.is_some());
                assert_eq!(
                    events[0].data,
                    Some(pointerinjector::Data::Viewport(create_viewport(0.0, 100.0)))
                );
            }
            other => panic!("Received unexpected value: {:?}", other),
        }

        // Create two touch events to be batched.
        let event_time1 = zx::MonotonicInstant::get();
        let contact1 = create_touch_contact(TOUCH_ID, Position { x: 20.0, y: 40.0 });
        let descriptor = get_touch_screen_device_descriptor();
        let input_event1 = input_device::UnhandledInputEvent::try_from(create_touch_screen_event(
            SortedVecMap::from_iter(vec![(
                fidl_ui_input::PointerEventPhase::Add,
                vec![contact1.clone()],
            )]),
            event_time1,
            &descriptor,
        ))
        .unwrap();

        let event_time2 = event_time1 + zx::MonotonicDuration::from_millis(10);
        let contact2 = create_touch_contact(TOUCH_ID, Position { x: 25.0, y: 45.0 });
        let input_event2 = input_device::UnhandledInputEvent::try_from(create_touch_screen_event(
            SortedVecMap::from_iter(vec![(
                fidl_ui_input::PointerEventPhase::Move,
                vec![contact2.clone()],
            )]),
            event_time2,
            &descriptor,
        ))
        .unwrap();

        // Handle events.
        let handle_event_fut = fixtures
            .touch_handler
            .clone()
            .handle_input_events(vec![input_event1.into(), input_event2.into()]);

        // Declare expected events.
        let expected_event1 = create_touch_pointer_sample_event(
            pointerinjector::EventPhase::Add,
            &contact1,
            Position { x: 20.0, y: 40.0 },
            event_time1,
        );
        let expected_event2 = create_touch_pointer_sample_event(
            pointerinjector::EventPhase::Change,
            &contact2,
            Position { x: 25.0, y: 45.0 },
            event_time2,
        );

        let device_fut = async move {
            match injector_device_request_stream.next().await {
                Some(Ok(pointerinjector::DeviceRequest::InjectEvents { events, .. })) => {
                    assert_eq!(events.len(), 2);
                    assert_eq!(events[0].timestamp, expected_event1.timestamp);
                    assert_eq!(events[0].data, expected_event1.data);
                    assert_eq!(events[1].timestamp, expected_event2.timestamp);
                    assert_eq!(events[1].data, expected_event2.data);
                }
                other => panic!("Received unexpected value: {:?}", other),
            }
        };

        let (handle_result, _) = futures::future::join(handle_event_fut, device_fut).await;

        // Verify events were marked handled.
        assert_matches!(
            handle_result.as_slice(),
            [
                input_device::InputEvent { handled: input_device::Handled::Yes, .. },
                input_device::InputEvent { handled: input_device::Handled::Yes, .. }
            ]
        );
    }
}
