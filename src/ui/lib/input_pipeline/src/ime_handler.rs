// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_handler::{Handler, InputHandlerStatus, UnhandledInputHandler};
use crate::{Incoming, input_device, keyboard_binding, metrics};
use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_ui_input3::{self as fidl_ui_input3, LockState, Modifiers};
use fuchsia_inspect::health::Reporter;

use keymaps::{LockStateChecker, ModifierChecker};
use metrics_registry::*;
use std::rc::Rc;

#[derive(Debug)]
pub struct FrozenLockState {
    lock_state: LockState,
}

impl From<LockState> for FrozenLockState {
    fn from(lock_state: LockState) -> Self {
        FrozenLockState { lock_state }
    }
}

impl LockStateChecker for FrozenLockState {
    fn test(&self, value: LockState) -> bool {
        self.lock_state.contains(value)
    }
}

/// Modifier state plus a tester method.
#[derive(Debug)]
pub struct FrozenModifierState {
    state: Modifiers,
}

impl From<fidl_fuchsia_ui_input3::Modifiers> for FrozenModifierState {
    fn from(state: Modifiers) -> Self {
        FrozenModifierState { state }
    }
}

impl ModifierChecker for FrozenModifierState {
    fn test(&self, value: Modifiers) -> bool {
        self.state.contains(value)
    }
}

/// [`ImeHandler`] is responsible for dispatching key events to the IME service, thus making sure
/// that key events are delivered to application runtimes (e.g., web, Flutter).
///
/// > NOTE: The [ImeHandler] requires [ModifierHandler] to be installed upstream to apply the keymaps correctly.
pub struct ImeHandler {
    /// The FIDL proxy (client-side stub) to the service for key event injection.
    key_event_injector: fidl_ui_input3::KeyEventInjectorProxy,

    /// The inventory of this handler's Inspect status.
    pub inspect_status: InputHandlerStatus,

    /// The metrics logger.
    metrics_logger: metrics::MetricsLogger,
}

impl Handler for ImeHandler {
    fn set_handler_healthy(self: std::rc::Rc<Self>) {
        self.inspect_status.health_node.borrow_mut().set_ok();
    }

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, msg: &str) {
        self.inspect_status.health_node.borrow_mut().set_unhealthy(msg);
    }

    fn get_name(&self) -> &'static str {
        "ImeHandler"
    }

    fn interest(&self) -> Vec<input_device::InputEventType> {
        vec![input_device::InputEventType::Keyboard]
    }
}

#[async_trait(?Send)]
impl UnhandledInputHandler for ImeHandler {
    async fn handle_unhandled_input_event(
        self: Rc<Self>,
        unhandled_input_event: input_device::UnhandledInputEvent,
    ) -> Vec<input_device::InputEvent> {
        fuchsia_trace::duration!("input", "ime_handler");
        match unhandled_input_event {
            input_device::UnhandledInputEvent {
                device_event: input_device::InputDeviceEvent::Keyboard(ref keyboard_device_event),
                device_descriptor:
                    input_device::InputDeviceDescriptor::Keyboard(ref keyboard_description),
                event_time,
                trace_id,
            } => {
                fuchsia_trace::duration!("input", "ime_handler[processing]");
                if let Some(trace_id) = trace_id {
                    fuchsia_trace::flow_step!("input", "event_in_input_pipeline", trace_id.into());
                }

                self.inspect_status.count_received_event(&event_time);
                let key_event = create_key_event(
                    &keyboard_device_event,
                    event_time,
                    keyboard_description.device_id,
                );
                self.dispatch_key(key_event).await;
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

impl ImeHandler {
    /// Creates a new [`ImeHandler`] by connecting out to the key event injector.
    pub async fn new(
        incoming: &Incoming,
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<Rc<Self>, Error> {
        let key_event_injector =
            incoming.connect_protocol::<fidl_ui_input3::KeyEventInjectorProxy>()?;

        Self::new_handler(key_event_injector, input_handlers_node, metrics_logger).await
    }

    /// Creates a new [`ImeHandler`].
    ///
    /// # Parameters
    /// `key_event_injector`: A proxy (FIDL client-side stub) to the key event
    /// injector FIDL service.
    async fn new_handler(
        key_event_injector: fidl_ui_input3::KeyEventInjectorProxy,
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<Rc<Self>, Error> {
        let inspect_status = InputHandlerStatus::new(
            input_handlers_node,
            "ime_handler",
            /* generates_events */ false,
        );
        let handler = ImeHandler { key_event_injector, inspect_status, metrics_logger };

        Ok(Rc::new(handler))
    }

    /// Dispatches key events to IME and returns KeyboardEvents for unhandled events.
    ///
    /// # Parameters
    /// `key_events`: The key events to dispatch.
    /// `event_time`: The time in nanoseconds when the events were first recorded.
    async fn dispatch_key(self: &Rc<Self>, key_event: fidl_ui_input3::KeyEvent) {
        assert!(
            key_event.timestamp.is_some(),
            "dispatch_key: got a key_event without a timestamp: {:?}",
            key_event
        );
        match self.key_event_injector.inject(&key_event).await {
            Err(err) => self.metrics_logger.log_error(
                InputPipelineErrorMetricDimensionEvent::ImeFailedToDispatchKeyToIme,
                std::format!("Failed to dispatch key to IME: {:?}", err),
            ),
            _ => {}
        };
    }
}

/// Returns a KeyEvent with the given parameters.
///
/// # Parameters
/// * `event`: The keyboard event to process.
/// * `event_time`: The time in nanoseconds when the event was first recorded.
fn create_key_event(
    event: &keyboard_binding::KeyboardEvent,
    event_time: zx::MonotonicInstant,
    device_id: u32,
) -> fidl_ui_input3::KeyEvent {
    let modifier_state: FrozenModifierState =
        event.get_modifiers().unwrap_or_else(|| Modifiers::from_bits_allow_unknown(0)).into();
    let lock_state: FrozenLockState =
        event.get_lock_state().unwrap_or_else(|| LockState::from_bits_allow_unknown(0)).into();
    log::debug!(
        "ImeHandler::create_key_event: key:{:?}, modifier_state: {:?}, lock_state: {:?}, event_type: {:?}",
        event.get_key(),
        modifier_state,
        lock_state,
        event.get_event_type(),
    );
    // Don't override the key meaning if already set, e.g. by prior stage.
    let key_meaning = event
        .get_key_meaning()
        .or_else(|| keymaps::US_QWERTY.apply(event.get_key(), &modifier_state, &lock_state));

    // Don't insert a spurious Some(0).
    let repeat_sequence = match event.get_repeat_sequence() {
        0 => None,
        s => Some(s),
    };

    fidl_ui_input3::KeyEvent {
        timestamp: Some(event_time.into_nanos()),
        type_: event.get_event_type().into(),
        key: event.get_key().into(),
        modifiers: event.get_modifiers(),
        lock_state: event.get_lock_state(),
        key_meaning,
        repeat_sequence,
        device_id: Some(device_id),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_handler::InputHandler;
    use crate::keyboard_binding::{self, KeyboardEvent};
    use crate::testing_utilities;
    use assert_matches::assert_matches;
    use fidl_fuchsia_input as fidl_input;
    use fidl_fuchsia_ui_input3 as fidl_ui_input3;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::convert::TryFrom as _;
    use test_case::test_case;

    fn handle_events(
        ime_handler: Rc<ImeHandler>,
        input_events: Vec<input_device::UnhandledInputEvent>,
    ) -> fasync::Task<()> {
        fasync::Task::local(async move {
            for input_event in input_events {
                assert_matches!(
                    ime_handler.clone().handle_unhandled_input_event(input_event).await.as_slice(),
                    [input_device::InputEvent { handled: input_device::Handled::Yes, .. }]
                );
            }
        })
    }

    async fn assert_ime_receives_events(
        expected_events: Vec<fidl_ui_input3::KeyEvent>,
        mut request_stream: fidl_ui_input3::KeyEventInjectorRequestStream,
    ) {
        let mut expected_events_iter = expected_events.iter().peekable();
        while let Some(Ok(fidl_ui_input3::KeyEventInjectorRequest::Inject {
            key_event,
            responder,
            ..
        })) = request_stream.next().await
        {
            pretty_assertions::assert_eq!(&key_event, expected_events_iter.next().unwrap());

            // All the expected events have been received, so make sure no more events
            // are present before returning.
            if expected_events_iter.peek().is_none() {
                responder
                    .send(fidl_ui_input3::KeyEventStatus::Handled)
                    .expect("error responding to DispatchKey");
                return;
            }
            responder
                .send(fidl_ui_input3::KeyEventStatus::Handled)
                .expect("error responding to DispatchKey");
        }

        assert!(false);
    }

    fn connect_to_key_event_injector()
    -> (fidl_ui_input3::KeyEventInjectorProxy, fidl_ui_input3::KeyEventInjectorRequestStream) {
        fidl::endpoints::create_proxy_and_stream::<fidl_ui_input3::KeyEventInjectorMarker>()
    }

    fn create_unhandled_keyboard_event(
        key: fidl_fuchsia_input::Key,
        event_type: fidl_fuchsia_ui_input3::KeyEventType,
        modifiers: Option<fidl_ui_input3::Modifiers>,
        event_time: zx::MonotonicInstant,
        device_descriptor: &input_device::InputDeviceDescriptor,
        keymap: Option<String>,
    ) -> input_device::UnhandledInputEvent {
        create_unhandled_keyboard_event_with_key_meaning(
            key,
            event_type,
            modifiers,
            event_time,
            device_descriptor,
            keymap,
            /* key_meaning */ None,
        )
    }

    fn create_unhandled_keyboard_event_with_key_meaning(
        key: fidl_fuchsia_input::Key,
        event_type: fidl_fuchsia_ui_input3::KeyEventType,
        modifiers: Option<fidl_ui_input3::Modifiers>,
        event_time: zx::MonotonicInstant,
        device_descriptor: &input_device::InputDeviceDescriptor,
        keymap: Option<String>,
        key_meaning: Option<fidl_fuchsia_ui_input3::KeyMeaning>,
    ) -> input_device::UnhandledInputEvent {
        input_device::UnhandledInputEvent::try_from(
            testing_utilities::create_keyboard_event_with_key_meaning(
                key,
                event_type,
                modifiers,
                event_time,
                device_descriptor,
                keymap,
                key_meaning,
            ),
        )
        .unwrap()
    }

    fn create_unhandled_input_event(
        keyboard_event: keyboard_binding::KeyboardEvent,
        device_descriptor: &input_device::InputDeviceDescriptor,
        event_time: zx::MonotonicInstant,
    ) -> input_device::UnhandledInputEvent {
        input_device::UnhandledInputEvent {
            device_event: input_device::InputDeviceEvent::Keyboard(keyboard_event),
            device_descriptor: device_descriptor.clone(),
            event_time,
            trace_id: None,
        }
    }

    /// Tests that a pressed key event is dispatched.
    ///
    /// > NOTE: The `device_descriptor` used in this test case and elsewhere
    /// *must* be of type `KeyboardDeviceDescriptor` as this is required by the
    /// pattern matching in `ImeHandler`.
    #[fuchsia::test]
    async fn pressed_key() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![fidl_input::Key::A],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![create_unhandled_keyboard_event(
            fidl_input::Key::A,
            fidl_fuchsia_ui_input3::KeyEventType::Pressed,
            None,
            event_time_u64,
            &device_descriptor,
            /* keymap= */ None,
        )];

        let expected_events = vec![fidl_ui_input3::KeyEvent {
            timestamp: Some(event_time_i64),
            type_: Some(fidl_ui_input3::KeyEventType::Pressed),
            key: Some(fidl_input::Key::A),
            // A key "A" without shift is a lowercase 'a'.
            key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(97)),
            device_id: Some(0),
            ..Default::default()
        }];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    /// Tests that a released key event is dispatched.
    #[fuchsia::test]
    async fn released_key() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![fidl_input::Key::A],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![create_unhandled_keyboard_event(
            fidl_input::Key::A,
            fidl_fuchsia_ui_input3::KeyEventType::Released,
            None,
            event_time_u64,
            &device_descriptor,
            /* keymap= */ None,
        )];

        let expected_events = vec![fidl_ui_input3::KeyEvent {
            timestamp: Some(event_time_i64),
            type_: Some(fidl_ui_input3::KeyEventType::Released),
            key: Some(fidl_input::Key::A),
            key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(97)),
            device_id: Some(0),
            ..Default::default()
        }];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    /// Tests that both pressed and released keys are dispatched appropriately.
    #[fuchsia::test]
    async fn pressed_and_released_key() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![fidl_input::Key::A, fidl_input::Key::B],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![
            create_unhandled_keyboard_event(
                fidl_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event(
                fidl_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Released,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event(
                fidl_input::Key::B,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event_with_key_meaning(
                fidl_input::Key::C,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
                Some(fidl_fuchsia_ui_input3::KeyMeaning::Codepoint(42)),
            ),
        ];

        let expected_events = vec![
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::A),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(97)),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Released),
                key: Some(fidl_input::Key::A),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(97)),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::B),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(98)),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::C),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(42)),
                device_id: Some(0),
                ..Default::default()
            },
        ];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    // Tests that modifier keys are dispatched appropriately.
    //
    // This test depends on the incoming event having correct modifier and lock
    // state.  Typically you'd do this by installing a ModifierHandler upstream
    // of this pipeline stage.
    #[fuchsia::test]
    async fn repeated_modifier_key() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![fidl_input::Key::A, fidl_input::Key::CapsLock],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![
            create_unhandled_input_event(
                KeyboardEvent::new(
                    fidl_input::Key::CapsLock,
                    fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                )
                .into_with_modifiers(Some(fidl_ui_input3::Modifiers::CAPS_LOCK))
                .into_with_lock_state(Some(fidl_ui_input3::LockState::CAPS_LOCK)),
                &device_descriptor,
                event_time_u64,
            ),
            create_unhandled_input_event(
                KeyboardEvent::new(
                    fidl_input::Key::A,
                    fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                )
                .into_with_modifiers(Some(fidl_ui_input3::Modifiers::CAPS_LOCK))
                .into_with_lock_state(Some(fidl_ui_input3::LockState::CAPS_LOCK)),
                &device_descriptor,
                event_time_u64,
            ),
            create_unhandled_input_event(
                KeyboardEvent::new(
                    fidl_input::Key::CapsLock,
                    fidl_fuchsia_ui_input3::KeyEventType::Released,
                )
                .into_with_lock_state(Some(fidl_ui_input3::LockState::CAPS_LOCK)),
                &device_descriptor,
                event_time_u64,
            ),
        ];

        let expected_events = vec![
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::CapsLock),
                modifiers: Some(fidl_ui_input3::Modifiers::CAPS_LOCK),
                lock_state: Some(fidl_ui_input3::LockState::CAPS_LOCK),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::CapsLock,
                )),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::A),
                modifiers: Some(fidl_ui_input3::Modifiers::CAPS_LOCK),
                lock_state: Some(fidl_ui_input3::LockState::CAPS_LOCK),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(65)),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Released),
                key: Some(fidl_input::Key::CapsLock),
                lock_state: Some(fidl_ui_input3::LockState::CAPS_LOCK),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::CapsLock,
                )),
                device_id: Some(0),
                ..Default::default()
            },
        ];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    #[fuchsia::test]
    async fn nonprintable_key_meanings_set_correctly() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![
                    fidl_input::Key::Enter,
                    fidl_input::Key::Tab,
                    fidl_input::Key::Backspace,
                ],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![
            create_unhandled_keyboard_event(
                fidl_input::Key::Enter,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event(
                fidl_input::Key::Tab,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event(
                fidl_input::Key::Backspace,
                fidl_fuchsia_ui_input3::KeyEventType::Released,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
        ];

        let expected_events = vec![
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::Enter),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::Enter,
                )),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::Tab),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::Tab,
                )),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                // Test that things also work when a key is released.
                type_: Some(fidl_ui_input3::KeyEventType::Released),
                key: Some(fidl_input::Key::Backspace),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::Backspace,
                )),
                device_id: Some(0),
                ..Default::default()
            },
        ];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    #[fuchsia::test]
    async fn tab() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![
                    fidl_input::Key::Enter,
                    fidl_input::Key::Tab,
                    fidl_input::Key::Backspace,
                ],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![create_unhandled_keyboard_event(
            fidl_input::Key::Tab,
            fidl_fuchsia_ui_input3::KeyEventType::Pressed,
            None,
            event_time_u64,
            &device_descriptor,
            /* keymap= */ None,
        )];

        let expected_events = vec![fidl_ui_input3::KeyEvent {
            timestamp: Some(event_time_i64),
            type_: Some(fidl_ui_input3::KeyEventType::Pressed),
            key: Some(fidl_input::Key::Tab),
            key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                fidl_ui_input3::NonPrintableKey::Tab,
            )),
            device_id: Some(0),
            ..Default::default()
        }];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    #[fuchsia::test]
    async fn shift_shift_a() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![fidl_input::Key::LeftCtrl, fidl_input::Key::Tab],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![
            create_unhandled_keyboard_event(
                fidl_input::Key::LeftShift,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                Some(Modifiers::LEFT_SHIFT | Modifiers::SHIFT),
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event(
                fidl_input::Key::RightShift,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                Some(Modifiers::LEFT_SHIFT | Modifiers::RIGHT_SHIFT | Modifiers::SHIFT),
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event(
                fidl_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                Some(Modifiers::LEFT_SHIFT | Modifiers::RIGHT_SHIFT | Modifiers::SHIFT),
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
        ];

        let expected_events = vec![
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::LeftShift),
                modifiers: Some(Modifiers::LEFT_SHIFT | Modifiers::SHIFT),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::Shift,
                )),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::RightShift),
                modifiers: Some(Modifiers::RIGHT_SHIFT | Modifiers::LEFT_SHIFT | Modifiers::SHIFT),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::Shift,
                )),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::A),
                modifiers: Some(Modifiers::RIGHT_SHIFT | Modifiers::LEFT_SHIFT | Modifiers::SHIFT),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(65)), // "A"
                device_id: Some(0),
                ..Default::default()
            },
        ];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    #[fuchsia::test]
    async fn ctrl_tab() {
        let (proxy, request_stream) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &test_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");

        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![fidl_input::Key::LeftCtrl, fidl_input::Key::Tab],
                device_id: 0,
                ..Default::default()
            },
        );
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![
            create_unhandled_keyboard_event(
                fidl_input::Key::LeftCtrl,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            create_unhandled_keyboard_event(
                fidl_input::Key::Tab,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
        ];

        let expected_events = vec![
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::LeftCtrl),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::Control,
                )),
                device_id: Some(0),
                ..Default::default()
            },
            fidl_ui_input3::KeyEvent {
                timestamp: Some(event_time_i64),
                type_: Some(fidl_ui_input3::KeyEventType::Pressed),
                key: Some(fidl_input::Key::Tab),
                key_meaning: Some(fidl_ui_input3::KeyMeaning::NonPrintableKey(
                    fidl_ui_input3::NonPrintableKey::Tab,
                )),
                device_id: Some(0),
                ..Default::default()
            },
        ];

        let _task = handle_events(ime_handler, input_events);
        assert_ime_receives_events(expected_events, request_stream).await;
    }

    #[fuchsia::test]
    async fn ime_handler_initialized_with_inspect_node() {
        let (proxy, _) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let _handler =
            ImeHandler::new_handler(proxy, &fake_handlers_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                ime_handler: {
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
    async fn ime_handler_inspect_counts_events() {
        let (proxy, _) = connect_to_key_event_injector();
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let ime_handler =
            ImeHandler::new_handler(proxy, &fake_handlers_node, metrics::MetricsLogger::default())
                .await
                .expect("Failed to create ImeHandler.");
        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![fidl_input::Key::A, fidl_input::Key::B],
                ..Default::default()
            },
        );
        let (_, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![
            testing_utilities::create_keyboard_event_with_time(
                fidl_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            // Should not count received events that have already been handled.
            testing_utilities::create_keyboard_event_with_handled(
                fidl_input::Key::B,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
                /* key_meaning= */ None,
                input_device::Handled::Yes,
            ),
            testing_utilities::create_keyboard_event_with_time(
                fidl_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Released,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            // Should not count non-keyboard input events.
            testing_utilities::create_fake_input_event(event_time_u64),
            testing_utilities::create_keyboard_event_with_time(
                fidl_input::Key::B,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
        ];

        for input_event in input_events {
            let _ = ime_handler.clone().handle_input_event(input_event).await;
        }

        let last_event_timestamp: u64 = event_time_u64.into_nanos().try_into().unwrap();

        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                ime_handler: {
                    events_received_count: 3u64,
                    events_handled_count: 3u64,
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

    #[test_case(
        keyboard_binding::KeyboardEvent::new(
            fidl_input::Key::A,
            fidl_ui_input3::KeyEventType::Pressed),
        zx::MonotonicInstant::from_nanos(42) => fidl_ui_input3::KeyEvent{
            timestamp: Some(42),
            type_: Some(fidl_ui_input3::KeyEventType::Pressed),
            key: Some(fidl_input::Key::A),
            key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(97)),
            device_id: Some(0),
            ..Default::default()}; "basic value copy")]
    #[test_case(
        keyboard_binding::KeyboardEvent::new(
            fidl_input::Key::A,
            fidl_ui_input3::KeyEventType::Pressed)
            .into_with_repeat_sequence(13),
        zx::MonotonicInstant::from_nanos(42) => fidl_ui_input3::KeyEvent{
            timestamp: Some(42),
            type_: Some(fidl_ui_input3::KeyEventType::Pressed),
            key: Some(fidl_input::Key::A),
            key_meaning: Some(fidl_ui_input3::KeyMeaning::Codepoint(97)),
            repeat_sequence: Some(13),
            device_id: Some(0),
            ..Default::default()}; "repeat_sequence is honored")]
    fn test_create_key_event(
        event: keyboard_binding::KeyboardEvent,
        event_time: zx::MonotonicInstant,
    ) -> fidl_ui_input3::KeyEvent {
        super::create_key_event(&event, event_time, 0)
    }
}
