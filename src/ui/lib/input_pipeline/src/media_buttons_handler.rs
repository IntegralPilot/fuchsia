// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::dispatcher::TaskHandle;
use crate::input_handler::{Handler, InputHandlerStatus, UnhandledInputHandler};
use crate::{Dispatcher, consumer_controls_binding, input_device, metrics};
use async_trait::async_trait;
use fidl::endpoints::Proxy;
use fidl_fuchsia_ui_input as fidl_ui_input;
use fidl_fuchsia_ui_policy as fidl_ui_policy;
use fuchsia_inspect::health::Reporter;
use futures::StreamExt;
use futures::channel::mpsc;
use metrics_registry::*;
use sorted_vec_map::SortedVecMap;
use std::cell::RefCell;
use std::rc::Rc;
use zx::AsHandleRef;

/// A [`MediaButtonsHandler`] tracks MediaButtonListeners and sends media button events to them.
pub struct MediaButtonsHandler {
    /// The mutable fields of this handler.
    inner: RefCell<MediaButtonsHandlerInner>,

    /// The inventory of this handler's Inspect status.
    pub inspect_status: InputHandlerStatus,

    metrics_logger: metrics::MetricsLogger,
}

#[derive(Debug)]
struct MediaButtonsHandlerInner {
    /// The media button listeners, key referenced by proxy channel's raw handle.
    pub listeners: SortedVecMap<u32, fidl_ui_policy::MediaButtonsListenerProxy>,

    /// The last MediaButtonsEvent sent to all listeners.
    /// This is used to send new listeners the state of the media buttons.
    pub last_event: Option<fidl_ui_input::MediaButtonsEvent>,

    pub send_event_task_tracker: LocalTaskTracker,
}

impl Handler for MediaButtonsHandler {
    fn set_handler_healthy(self: std::rc::Rc<Self>) {
        self.inspect_status.health_node.borrow_mut().set_ok();
    }

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, msg: &str) {
        self.inspect_status.health_node.borrow_mut().set_unhealthy(msg);
    }

    fn get_name(&self) -> &'static str {
        "MediaButtonsHandler"
    }

    fn interest(&self) -> Vec<input_device::InputEventType> {
        vec![input_device::InputEventType::ConsumerControls]
    }
}

#[async_trait(?Send)]
impl UnhandledInputHandler for MediaButtonsHandler {
    async fn handle_unhandled_input_event(
        self: Rc<Self>,
        mut unhandled_input_event: input_device::UnhandledInputEvent,
    ) -> Vec<input_device::InputEvent> {
        fuchsia_trace::duration!("input", "media_buttons_handler");
        match unhandled_input_event {
            input_device::UnhandledInputEvent {
                device_event:
                    input_device::InputDeviceEvent::ConsumerControls(ref mut consumer_controls_event),
                device_descriptor:
                    input_device::InputDeviceDescriptor::ConsumerControls(ref device_descriptor),
                event_time,
                trace_id,
            } => {
                fuchsia_trace::duration!("input", "media_buttons_handler[processing]");
                if let Some(trace_id) = trace_id {
                    fuchsia_trace::flow_step!("input", "event_in_input_pipeline", trace_id.into());
                }

                self.inspect_status.count_received_event(&event_time);
                let mut media_buttons_event =
                    Self::create_media_buttons_event(consumer_controls_event, device_descriptor);

                // Send the event if the media buttons are supported.
                self.send_event_to_listeners(&media_buttons_event).await;

                // Store the sent event without any wake leases.
                std::mem::drop(media_buttons_event.wake_lease.take());
                self.inner.borrow_mut().last_event = Some(media_buttons_event);

                // Consume the input event.
                self.inspect_status.count_handled_event();
                vec![input_device::InputEvent::from(unhandled_input_event).into_handled()]
            }
            _ => {
                self.metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::HandlerReceivedUninterestedEvent,
                    std::format!(
                        "{} uninterested input event: {:?}",
                        self.get_name(),
                        unhandled_input_event.get_event_type()
                    ),
                );
                vec![input_device::InputEvent::from(unhandled_input_event)]
            }
        }
    }
}

impl MediaButtonsHandler {
    /// Creates a new [`MediaButtonsHandler`] that sends media button events to listeners.
    pub fn new(
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Rc<Self> {
        let inspect_status =
            InputHandlerStatus::new(input_handlers_node, "media_buttons_handler", false);
        Self::new_internal(inspect_status, metrics_logger)
    }

    fn clone_event(event: &fidl_ui_input::MediaButtonsEvent) -> fidl_ui_input::MediaButtonsEvent {
        // each copy of the event should have a unique trace flow id.
        let trace_flow_id = fuchsia_trace::Id::new();
        fuchsia_trace::flow_begin!("input", "dispatch_media_buttons_to_listeners", trace_flow_id);

        fidl_ui_input::MediaButtonsEvent {
            volume: event.volume,
            mic_mute: event.mic_mute,
            pause: event.pause,
            camera_disable: event.camera_disable,
            power: event.power,
            function: event.function,
            device_id: event.device_id,
            wake_lease: event.wake_lease.as_ref().map(|lease| {
                lease
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("failed to duplicate event pair")
            }),
            trace_flow_id: Some(trace_flow_id.into()),
            ..Default::default()
        }
    }

    fn new_internal(
        inspect_status: InputHandlerStatus,
        metrics_logger: metrics::MetricsLogger,
    ) -> Rc<Self> {
        let media_buttons_handler = Self {
            inner: RefCell::new(MediaButtonsHandlerInner {
                listeners: SortedVecMap::new(),
                last_event: None,
                send_event_task_tracker: LocalTaskTracker::new(),
            }),
            inspect_status,
            metrics_logger,
        };
        Rc::new(media_buttons_handler)
    }

    /// Creates a fidl_ui_input::MediaButtonsEvent from a media_buttons::MediaButtonEvent.
    ///
    /// # Parameters
    /// -  `event`: The MediaButtonEvent to create a MediaButtonsEvent from.
    fn create_media_buttons_event(
        event: &mut consumer_controls_binding::ConsumerControlsEvent,
        device_descriptor: &consumer_controls_binding::ConsumerControlsDeviceDescriptor,
    ) -> fidl_ui_input::MediaButtonsEvent {
        let mut new_event = fidl_ui_input::MediaButtonsEvent {
            volume: None,
            mic_mute: None,
            pause: None,
            camera_disable: None,
            power: None,
            function: None,
            device_id: Some(device_descriptor.device_id),
            wake_lease: event.wake_lease.take(),
            ..Default::default()
        };

        for button in &device_descriptor.buttons {
            match button {
                fidl_fuchsia_input::ConsumerControlButton::VolumeUp
                | fidl_fuchsia_input::ConsumerControlButton::VolumeDown => {
                    new_event.volume = Some(0)
                }
                fidl_fuchsia_input::ConsumerControlButton::MicMute => {
                    new_event.mic_mute = Some(false)
                }
                fidl_fuchsia_input::ConsumerControlButton::Pause => new_event.pause = Some(false),
                fidl_fuchsia_input::ConsumerControlButton::CameraDisable => {
                    new_event.camera_disable = Some(false)
                }
                fidl_fuchsia_input::ConsumerControlButton::Power => new_event.power = Some(false),
                fidl_fuchsia_input::ConsumerControlButton::Function => {
                    new_event.function = Some(false)
                }
                _ => {}
            }
        }

        for button in &event.pressed_buttons {
            match button {
                fidl_fuchsia_input::ConsumerControlButton::VolumeUp => {
                    new_event.volume = Some(new_event.volume.unwrap_or(0).saturating_add(1));
                }
                fidl_fuchsia_input::ConsumerControlButton::VolumeDown => {
                    new_event.volume = Some(new_event.volume.unwrap_or(0).saturating_sub(1));
                }
                fidl_fuchsia_input::ConsumerControlButton::MicMute => {
                    new_event.mic_mute = Some(true);
                }
                fidl_fuchsia_input::ConsumerControlButton::Pause => {
                    new_event.pause = Some(true);
                }
                fidl_fuchsia_input::ConsumerControlButton::CameraDisable => {
                    new_event.camera_disable = Some(true);
                }
                fidl_fuchsia_input::ConsumerControlButton::Function => {
                    new_event.function = Some(true);
                }
                fidl_fuchsia_input::ConsumerControlButton::Power => {
                    new_event.power = Some(true);
                }
                _ => {}
            }
        }

        new_event
    }

    /// Sends media button events to media button listeners.
    ///
    /// # Parameters
    /// - `event`: The event to send to the listeners.
    async fn send_event_to_listeners(self: &Rc<Self>, event: &fidl_ui_input::MediaButtonsEvent) {
        let tracker = &self.inner.borrow().send_event_task_tracker;

        for (handle, listener) in self.inner.borrow().listeners.iter() {
            let weak_handler = Rc::downgrade(&self);
            let listener_clone = listener.clone();
            let handle_clone = handle.clone();
            let event_to_send = Self::clone_event(event);
            let fut = async move {
                match listener_clone.on_event(event_to_send).await {
                    Ok(_) => {}
                    Err(e) => {
                        if let Some(handler) = weak_handler.upgrade() {
                            handler.inner.borrow_mut().listeners.remove(&handle_clone);
                            log::info!(
                                "Unregistering listener; unable to send MediaButtonsEvent: {:?}",
                                e
                            )
                        }
                    }
                }
            };

            let metrics_logger_clone = self.metrics_logger.clone();
            let task = Dispatcher::spawn_local(fut);
            tracker.track(metrics_logger_clone, task);
        }
    }

    // Add the listener to the registry.
    ///
    /// # Parameters
    /// - `proxy`: A new listener proxy to send events to.
    pub async fn register_listener_proxy(
        self: &Rc<Self>,
        proxy: fidl_ui_policy::MediaButtonsListenerProxy,
    ) {
        self.inner
            .borrow_mut()
            .listeners
            .insert(proxy.as_channel().as_handle_ref().raw_handle(), proxy.clone());

        // Send the listener the last media button event.
        if let Some(event) = &self.inner.borrow().last_event {
            let event_to_send = Self::clone_event(event);
            let fut = async move {
                match proxy.on_event(event_to_send).await {
                    Ok(_) => {}
                    Err(e) => {
                        log::info!("Failed to send media buttons event to listener {:?}", e)
                    }
                }
            };
            let metrics_logger_clone = self.metrics_logger.clone();
            let task = Dispatcher::spawn_local(fut);
            self.inner.borrow().send_event_task_tracker.track(metrics_logger_clone, task);
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
        let receiver_task = Dispatcher::spawn_local(async move {
            // Drop the tasks as they are completed.
            receiver.for_each_concurrent(None, |task: TaskHandle<()>| task).await
        });

        Self { sender, _receiver_task: receiver_task }
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
                    InputPipelineErrorMetricDimensionEvent::MediaButtonErrorWhilePushingTask,
                    std::format!("Unexpected {e:?} while pushing task"),
                );
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_handler::InputHandler;
    use crate::testing_utilities;
    use anyhow::Error;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use futures::channel::oneshot;
    use pretty_assertions::assert_eq;
    use std::task::Poll;

    fn spawn_device_listener_registry_server(
        handler: Rc<MediaButtonsHandler>,
    ) -> (fidl_ui_policy::DeviceListenerRegistryProxy, fasync::Task<()>) {
        let (device_listener_proxy, mut device_listener_stream) =
            create_proxy_and_stream::<fidl_ui_policy::DeviceListenerRegistryMarker>();

        let task = fasync::Task::local(async move {
            loop {
                match device_listener_stream.try_next().await {
                    Ok(Some(fidl_ui_policy::DeviceListenerRegistryRequest::RegisterListener {
                        listener,
                        responder,
                    })) => {
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

    fn create_ui_input_media_buttons_event(
        volume: Option<i8>,
        mic_mute: Option<bool>,
        pause: Option<bool>,
        camera_disable: Option<bool>,
        power: Option<bool>,
        function: Option<bool>,
    ) -> fidl_ui_input::MediaButtonsEvent {
        fidl_ui_input::MediaButtonsEvent {
            volume,
            mic_mute,
            pause,
            camera_disable,
            power,
            function,
            device_id: Some(0),
            ..Default::default()
        }
    }

    /// Makes a `Task` that waits for a `oneshot`'s value to be set, and then forwards that value to
    /// a reference-counted container that can be observed outside the task.
    fn make_signalable_task<T: Default + 'static>()
    -> (oneshot::Sender<T>, TaskHandle<()>, Rc<RefCell<T>>) {
        let (sender, receiver) = oneshot::channel();
        let task_completed = Rc::new(RefCell::new(<T as Default>::default()));
        let task_completed_ = task_completed.clone();
        let task = fasync::Task::local(async move {
            if let Ok(value) = receiver.await {
                *task_completed_.borrow_mut() = value;
            }
        });
        (sender, task.into(), task_completed)
    }

    /// Tests that a media button listener can be registered and is sent the latest event upon
    /// registration.
    #[fuchsia::test]
    async fn register_media_buttons_listener() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let inspect_status = InputHandlerStatus::new(
            &test_node,
            "media_buttons_handler",
            /* generates_events */ false,
        );

        let media_buttons_handler = Rc::new(MediaButtonsHandler {
            inner: RefCell::new(MediaButtonsHandlerInner {
                listeners: SortedVecMap::new(),
                last_event: Some(create_ui_input_media_buttons_event(
                    Some(1),
                    None,
                    None,
                    None,
                    None,
                    None,
                )),
                send_event_task_tracker: LocalTaskTracker::new(),
            }),
            inspect_status,
            metrics_logger: metrics::MetricsLogger::default(),
        });
        let (device_listener_proxy, _server_task) =
            spawn_device_listener_registry_server(media_buttons_handler.clone());

        // Register a listener.
        let (listener, mut listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>();
        let register_listener_fut = async {
            let res = device_listener_proxy.register_listener(listener).await;
            assert!(res.is_ok());
        };

        // Assert listener was registered and received last event.
        let expected_event =
            create_ui_input_media_buttons_event(Some(1), None, None, None, None, None);
        let assert_fut = async {
            match listener_stream.next().await {
                Some(Ok(fidl_ui_policy::MediaButtonsListenerRequest::OnEvent {
                    mut event,
                    responder,
                })) => {
                    event.trace_flow_id = None;
                    assert_eq!(event, expected_event);
                    responder.send().expect("responder failed.");
                }
                _ => assert!(false),
            }
        };
        futures::join!(register_listener_fut, assert_fut);
        assert_eq!(media_buttons_handler.inner.borrow().listeners.len(), 1);
    }

    /// Tests that all supported buttons are sent.
    #[fuchsia::test]
    async fn listener_receives_all_buttons() {
        let event_time = zx::MonotonicInstant::get();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let inspect_status = InputHandlerStatus::new(
            &test_node,
            "media_buttons_handler",
            /* generates_events */ false,
        );
        let media_buttons_handler =
            MediaButtonsHandler::new_internal(inspect_status, metrics::MetricsLogger::default());
        let (device_listener_proxy, _server_task) =
            spawn_device_listener_registry_server(media_buttons_handler.clone());

        // Register a listener.
        let (listener, listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>();
        let _ = device_listener_proxy.register_listener(listener).await;

        // Setup events and expectations.
        let descriptor = testing_utilities::consumer_controls_device_descriptor();
        let input_events = vec![testing_utilities::create_consumer_controls_event(
            vec![
                fidl_fuchsia_input::ConsumerControlButton::VolumeUp,
                fidl_fuchsia_input::ConsumerControlButton::VolumeDown,
                fidl_fuchsia_input::ConsumerControlButton::Pause,
                fidl_fuchsia_input::ConsumerControlButton::MicMute,
                fidl_fuchsia_input::ConsumerControlButton::CameraDisable,
                fidl_fuchsia_input::ConsumerControlButton::Function,
                fidl_fuchsia_input::ConsumerControlButton::Power,
            ],
            event_time,
            &descriptor,
        )];
        let expected_events = vec![create_ui_input_media_buttons_event(
            Some(0),
            Some(true),
            Some(true),
            Some(true),
            Some(true),
            Some(true),
        )];

        // Assert registered listener receives event.
        use crate::input_handler::InputHandler as _; // Adapt UnhandledInputHandler to InputHandler
        assert_input_event_sequence_generates_media_buttons_events!(
            input_handler: media_buttons_handler,
            input_events: input_events,
            expected_events: expected_events,
            media_buttons_listener_request_stream: vec![listener_stream],
        );
    }

    /// Tests that multiple listeners are supported.
    #[fuchsia::test]
    async fn multiple_listeners_receive_event() {
        let event_time = zx::MonotonicInstant::get();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let inspect_status = InputHandlerStatus::new(
            &test_node,
            "media_buttons_handler",
            /* generates_events */ false,
        );
        let media_buttons_handler =
            MediaButtonsHandler::new_internal(inspect_status, metrics::MetricsLogger::default());
        let (device_listener_proxy, _server_task) =
            spawn_device_listener_registry_server(media_buttons_handler.clone());

        // Register two listeners.
        let (first_listener, first_listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>();
        let (second_listener, second_listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>();
        let _ = device_listener_proxy.register_listener(first_listener).await;
        let _ = device_listener_proxy.register_listener(second_listener).await;

        // Setup events and expectations.
        let descriptor = testing_utilities::consumer_controls_device_descriptor();
        let input_events = vec![testing_utilities::create_consumer_controls_event(
            vec![fidl_fuchsia_input::ConsumerControlButton::VolumeUp],
            event_time,
            &descriptor,
        )];
        let expected_events = vec![create_ui_input_media_buttons_event(
            Some(1),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
        )];

        // Assert registered listeners receives event.
        use crate::input_handler::InputHandler as _; // Adapt UnhandledInputHandler to InputHandler
        assert_input_event_sequence_generates_media_buttons_events!(
            input_handler: media_buttons_handler,
            input_events: input_events,
            expected_events: expected_events,
            media_buttons_listener_request_stream:
                vec![first_listener_stream, second_listener_stream],
        );
    }

    /// Tests that listener is unregistered if channel is closed and we try to send input event to listener
    #[fuchsia::test]
    fn unregister_listener_if_channel_closed() {
        let mut exec = fasync::TestExecutor::new();

        let event_time = zx::MonotonicInstant::get();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let inspect_status = InputHandlerStatus::new(
            &test_node,
            "media_buttons_handler",
            /* generates_events */ false,
        );
        let media_buttons_handler =
            MediaButtonsHandler::new_internal(inspect_status, metrics::MetricsLogger::default());
        let media_buttons_handler_clone = media_buttons_handler.clone();

        let mut task = fasync::Task::local(async move {
            let (device_listener_proxy, _server_task) =
                spawn_device_listener_registry_server(media_buttons_handler.clone());

            // Register three listeners.
            let (first_listener, mut first_listener_stream) =
                fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>(
                );
            let (second_listener, mut second_listener_stream) =
                fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>(
                );
            let (third_listener, third_listener_stream) = fidl::endpoints::create_request_stream::<
                fidl_ui_policy::MediaButtonsListenerMarker,
            >();
            let _ = device_listener_proxy.register_listener(first_listener).await;
            let _ = device_listener_proxy.register_listener(second_listener).await;
            let _ = device_listener_proxy.register_listener(third_listener).await;
            assert_eq!(media_buttons_handler.inner.borrow().listeners.len(), 3);

            // Generate input event to be handled by MediaButtonsHandler.
            let descriptor = testing_utilities::consumer_controls_device_descriptor();
            let input_event = testing_utilities::create_consumer_controls_event(
                vec![fidl_fuchsia_input::ConsumerControlButton::VolumeUp],
                event_time,
                &descriptor,
            );

            let expected_media_buttons_event = create_ui_input_media_buttons_event(
                Some(1),
                Some(false),
                Some(false),
                Some(false),
                Some(false),
                Some(false),
            );

            // Drop third registered listener.
            std::mem::drop(third_listener_stream);

            let _ = media_buttons_handler.clone().handle_input_event(input_event).await;
            // First listener stalls, responder doesn't send response - subsequent listeners should still be able receive event.
            if let Some(request) = first_listener_stream.next().await {
                match request {
                    Ok(fidl_ui_policy::MediaButtonsListenerRequest::OnEvent {
                        mut event,
                        responder: _,
                    }) => {
                        event.trace_flow_id = None;
                        pretty_assertions::assert_eq!(event, expected_media_buttons_event);

                        // No need to send response because we want to simulate reader getting stuck.
                    }
                    _ => assert!(false),
                }
            } else {
                assert!(false);
            }

            // Send response from responder on second listener stream
            if let Some(request) = second_listener_stream.next().await {
                match request {
                    Ok(fidl_ui_policy::MediaButtonsListenerRequest::OnEvent {
                        mut event,
                        responder,
                    }) => {
                        event.trace_flow_id = None;
                        pretty_assertions::assert_eq!(event, expected_media_buttons_event);
                        let _ = responder.send();
                    }
                    _ => assert!(false),
                }
            } else {
                assert!(false);
            }
        });

        // Must manually run tasks with executor to ensure all tasks in LocalTaskTracker complete/stall before we call final assertion.
        let _ = exec.run_until_stalled(&mut task);

        // Should only be two listeners still registered in 'inner' after we unregister the listener with closed channel.
        let _ = exec.run_singlethreaded(async {
            assert_eq!(media_buttons_handler_clone.inner.borrow().listeners.len(), 2);
        });
    }

    /// Tests that handle_input_event returns even if reader gets stuck while sending event to listener
    #[fuchsia::test]
    async fn stuck_reader_wont_block_input_pipeline() {
        let event_time = zx::MonotonicInstant::get();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let inspect_status = InputHandlerStatus::new(
            &test_node,
            "media_buttons_handler",
            /* generates_events */ false,
        );
        let media_buttons_handler =
            MediaButtonsHandler::new_internal(inspect_status, metrics::MetricsLogger::default());
        let (device_listener_proxy, _server_task) =
            spawn_device_listener_registry_server(media_buttons_handler.clone());

        let (first_listener, mut first_listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>();
        let (second_listener, mut second_listener_stream) =
            fidl::endpoints::create_request_stream::<fidl_ui_policy::MediaButtonsListenerMarker>();
        let _ = device_listener_proxy.register_listener(first_listener).await;
        let _ = device_listener_proxy.register_listener(second_listener).await;

        // Setup events and expectations.
        let descriptor = testing_utilities::consumer_controls_device_descriptor();
        let first_unhandled_input_event = input_device::UnhandledInputEvent {
            device_event: input_device::InputDeviceEvent::ConsumerControls(
                consumer_controls_binding::ConsumerControlsEvent::new(
                    vec![fidl_fuchsia_input::ConsumerControlButton::VolumeUp],
                    None,
                ),
            ),
            device_descriptor: descriptor.clone(),
            event_time,
            trace_id: None,
        };
        let first_expected_media_buttons_event = create_ui_input_media_buttons_event(
            Some(1),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
        );

        assert_matches!(
            media_buttons_handler
                .clone()
                .handle_unhandled_input_event(first_unhandled_input_event)
                .await
                .as_slice(),
            [input_device::InputEvent { handled: input_device::Handled::Yes, .. }]
        );

        let mut save_responder = None;

        // Ensure handle_input_event attempts to send event to first listener.
        if let Some(request) = first_listener_stream.next().await {
            match request {
                Ok(fidl_ui_policy::MediaButtonsListenerRequest::OnEvent {
                    mut event,
                    responder,
                }) => {
                    event.trace_flow_id = None;
                    pretty_assertions::assert_eq!(event, first_expected_media_buttons_event);

                    // No need to send response because we want to simulate reader getting stuck.

                    // Save responder to send response later
                    save_responder = Some(responder);
                }
                _ => assert!(false),
            }
        } else {
            assert!(false)
        }

        // Ensure handle_input_event still sends event to second listener when reader for first listener is stuck.
        if let Some(request) = second_listener_stream.next().await {
            match request {
                Ok(fidl_ui_policy::MediaButtonsListenerRequest::OnEvent {
                    mut event,
                    responder,
                }) => {
                    event.trace_flow_id = None;
                    pretty_assertions::assert_eq!(event, first_expected_media_buttons_event);
                    let _ = responder.send();
                }
                _ => assert!(false),
            }
        } else {
            assert!(false)
        }

        // Setup second event to handle
        let second_unhandled_input_event = input_device::UnhandledInputEvent {
            device_event: input_device::InputDeviceEvent::ConsumerControls(
                consumer_controls_binding::ConsumerControlsEvent::new(
                    vec![fidl_fuchsia_input::ConsumerControlButton::MicMute],
                    None,
                ),
            ),
            device_descriptor: descriptor.clone(),
            event_time,
            trace_id: None,
        };
        let second_expected_media_buttons_event = create_ui_input_media_buttons_event(
            Some(0),
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
        );

        // Ensure we can handle a subsequent event if listener stalls on first event.
        assert_matches!(
            media_buttons_handler
                .clone()
                .handle_unhandled_input_event(second_unhandled_input_event)
                .await
                .as_slice(),
            [input_device::InputEvent { handled: input_device::Handled::Yes, .. }]
        );

        // Ensure events are still sent to listeners if a listener stalls on a previous event.
        if let Some(request) = second_listener_stream.next().await {
            match request {
                Ok(fidl_ui_policy::MediaButtonsListenerRequest::OnEvent {
                    mut event,
                    responder,
                }) => {
                    event.trace_flow_id = None;
                    pretty_assertions::assert_eq!(event, second_expected_media_buttons_event);
                    let _ = responder.send();
                }
                _ => assert!(false),
            }
        } else {
            assert!(false)
        }

        match save_responder {
            Some(save_responder) => {
                // Simulate delayed response to first listener for first event
                let _ = save_responder.send();
                // First listener should now receive second event after delayed response for first event
                if let Some(request) = first_listener_stream.next().await {
                    match request {
                        Ok(fidl_ui_policy::MediaButtonsListenerRequest::OnEvent {
                            mut event,
                            responder: _,
                        }) => {
                            event.trace_flow_id = None;
                            pretty_assertions::assert_eq!(
                                event,
                                second_expected_media_buttons_event
                            );

                            // No need to send response
                        }
                        _ => assert!(false),
                    }
                } else {
                    assert!(false)
                }
            }
            None => {
                assert!(false)
            }
        }
    }

    // Test for LocalTaskTracker
    #[fuchsia::test]
    fn local_task_tracker_test() -> Result<(), Error> {
        let mut exec = fasync::TestExecutor::new();

        let (mut sender_1, task_1, completed_1) = make_signalable_task::<bool>();
        let (sender_2, task_2, completed_2) = make_signalable_task::<bool>();

        let mut tracker = LocalTaskTracker::new();

        tracker.track(metrics::MetricsLogger::default(), task_1);
        tracker.track(metrics::MetricsLogger::default(), task_2);

        assert_matches!(exec.run_until_stalled(&mut tracker._receiver_task), Poll::Pending);
        assert_eq!(Rc::strong_count(&completed_1), 2);
        assert_eq!(Rc::strong_count(&completed_2), 2);
        assert!(!sender_1.is_canceled());
        assert!(!sender_2.is_canceled());

        assert!(sender_2.send(true).is_ok());
        assert_matches!(exec.run_until_stalled(&mut tracker._receiver_task), Poll::Pending);

        assert_eq!(Rc::strong_count(&completed_1), 2);
        assert_eq!(Rc::strong_count(&completed_2), 1);
        assert_eq!(*completed_1.borrow(), false);
        assert_eq!(*completed_2.borrow(), true);
        assert!(!sender_1.is_canceled());

        drop(tracker);
        let mut sender_1_cancellation = sender_1.cancellation();
        assert_matches!(exec.run_until_stalled(&mut sender_1_cancellation), Poll::Ready(()));
        assert_eq!(Rc::strong_count(&completed_1), 1);
        assert!(sender_1.is_canceled());

        Ok(())
    }

    #[fuchsia::test]
    async fn media_buttons_handler_initialized_with_inspect_node() {
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let _handler =
            MediaButtonsHandler::new(&fake_handlers_node, metrics::MetricsLogger::default());
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                media_buttons_handler: {
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

    #[fuchsia::test]
    async fn media_buttons_handler_inspect_counts_events() {
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let media_buttons_handler =
            MediaButtonsHandler::new(&fake_handlers_node, metrics::MetricsLogger::default());

        // Unhandled input event should be counted by inspect.
        let descriptor = testing_utilities::consumer_controls_device_descriptor();
        let events = vec![
            input_device::InputEvent {
                device_event: input_device::InputDeviceEvent::ConsumerControls(
                    consumer_controls_binding::ConsumerControlsEvent::new(
                        vec![fidl_fuchsia_input::ConsumerControlButton::VolumeUp],
                        None,
                    ),
                ),
                device_descriptor: descriptor.clone(),
                event_time: zx::MonotonicInstant::get(),
                handled: input_device::Handled::No,
                trace_id: None,
            },
            // Handled input event should be ignored.
            input_device::InputEvent {
                device_event: input_device::InputDeviceEvent::ConsumerControls(
                    consumer_controls_binding::ConsumerControlsEvent::new(
                        vec![fidl_fuchsia_input::ConsumerControlButton::VolumeUp],
                        None,
                    ),
                ),
                device_descriptor: descriptor.clone(),
                event_time: zx::MonotonicInstant::get(),
                handled: input_device::Handled::Yes,
                trace_id: None,
            },
            input_device::InputEvent {
                device_event: input_device::InputDeviceEvent::ConsumerControls(
                    consumer_controls_binding::ConsumerControlsEvent::new(
                        vec![fidl_fuchsia_input::ConsumerControlButton::VolumeDown],
                        None,
                    ),
                ),
                device_descriptor: descriptor.clone(),
                event_time: zx::MonotonicInstant::get(),
                handled: input_device::Handled::No,
                trace_id: None,
            },
        ];

        let last_event_timestamp: u64 =
            events[2].clone().event_time.into_nanos().try_into().unwrap();

        for event in events {
            media_buttons_handler.clone().handle_input_event(event).await;
        }

        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                media_buttons_handler: {
                    events_received_count: 2u64,
                    events_handled_count: 2u64,
                    last_received_timestamp_ns: last_event_timestamp,
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
        let event_with_lease = fidl_ui_input::MediaButtonsEvent {
            volume: Some(1),
            mic_mute: Some(true),
            pause: Some(true),
            camera_disable: Some(true),
            power: Some(true),
            function: Some(true),
            device_id: Some(1),
            wake_lease: Some(event_pair),
            ..Default::default()
        };

        // Test cloning an event that has a wake lease.
        // With wake lease argument should duplicate the handle.
        let cloned_event = MediaButtonsHandler::clone_event(&event_with_lease);
        assert_eq!(event_with_lease.volume, cloned_event.volume);
        assert_eq!(event_with_lease.mic_mute, cloned_event.mic_mute);
        assert_eq!(event_with_lease.pause, cloned_event.pause);
        assert_eq!(event_with_lease.camera_disable, cloned_event.camera_disable);
        assert_eq!(event_with_lease.power, cloned_event.power);
        assert_eq!(event_with_lease.function, cloned_event.function);
        assert_eq!(event_with_lease.device_id, cloned_event.device_id);
        assert!(event_with_lease.wake_lease.is_some());
        assert!(cloned_event.wake_lease.is_some());
        assert_ne!(
            event_with_lease.wake_lease.as_ref().unwrap().as_handle_ref().raw_handle(),
            cloned_event.wake_lease.as_ref().unwrap().as_handle_ref().raw_handle()
        );
        assert_eq!(
            event_with_lease.wake_lease.as_ref().unwrap().koid(),
            cloned_event.wake_lease.as_ref().unwrap().koid()
        );
    }

    #[fuchsia::test]
    async fn clone_event_without_lease_has_no_lease() {
        // Test cloning an event that does not have a wake lease.
        let event_without_lease = fidl_ui_input::MediaButtonsEvent {
            volume: Some(1),
            mic_mute: Some(true),
            pause: Some(true),
            camera_disable: Some(true),
            power: Some(true),
            function: Some(true),
            device_id: Some(1),
            ..Default::default()
        };

        // With wake lease argument should result in no wake lease.
        let cloned_event = MediaButtonsHandler::clone_event(&event_without_lease);
        assert_eq!(event_without_lease.volume, cloned_event.volume);
        assert_eq!(event_without_lease.mic_mute, cloned_event.mic_mute);
        assert_eq!(event_without_lease.pause, cloned_event.pause);
        assert_eq!(event_without_lease.camera_disable, cloned_event.camera_disable);
        assert_eq!(event_without_lease.power, cloned_event.power);
        assert_eq!(event_without_lease.function, cloned_event.function);
        assert_eq!(event_without_lease.device_id, cloned_event.device_id);
        assert!(cloned_event.wake_lease.is_none());
    }

    #[fuchsia::test]
    async fn clone_event_sets_trace_flow_id() {
        let event = fidl_ui_input::MediaButtonsEvent { volume: Some(1), ..Default::default() };

        let cloned_event_1 = MediaButtonsHandler::clone_event(&event);
        let cloned_event_2 = MediaButtonsHandler::clone_event(&event);

        assert!(cloned_event_1.trace_flow_id.is_some());
        assert!(cloned_event_2.trace_flow_id.is_some());
        assert_ne!(cloned_event_1.trace_flow_id, cloned_event_2.trace_flow_id);
    }
}
