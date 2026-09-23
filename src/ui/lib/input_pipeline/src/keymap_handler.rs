// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements applying keymaps to hardware keyboard key events.
//!
//! See [KeymapHandler] for details.

use crate::input_handler::{Handler, InputHandlerStatus, UnhandledInputHandler};
use crate::{input_device, keyboard_binding, metrics};
use async_trait::async_trait;
use fuchsia_inspect::health::Reporter;
use metrics_registry::InputPipelineErrorMetricDimensionEvent;
use std::cell::RefCell;
use std::rc::Rc;

/// `KeymapHandler` applies a keymap to a keyboard event, resolving each key press
/// to a sequence of Unicode code points.  This allows basic keymap application,
/// but does not lend itself to generalized text editing.
///
/// Create a new one with [KeymapHandler::new].
#[derive(Debug)]
pub struct KeymapHandler {
    /// Tracks the state of the modifier keys.
    modifier_state: RefCell<keymaps::ModifierState>,

    /// Tracks the lock state (NumLock, CapsLock).
    lock_state: RefCell<keymaps::LockStateKeys>,

    /// The metrics logger.
    metrics_logger: metrics::MetricsLogger,

    /// The inventory of this handler's Inspect status.
    pub inspect_status: InputHandlerStatus,
}

impl Handler for KeymapHandler {
    fn set_handler_healthy(self: std::rc::Rc<Self>) {
        self.inspect_status.health_node.borrow_mut().set_ok();
    }

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, msg: &str) {
        self.inspect_status.health_node.borrow_mut().set_unhealthy(msg);
    }

    fn get_name(&self) -> &'static str {
        "KeymapHandler"
    }

    fn interest(&self) -> Vec<input_device::InputEventType> {
        vec![input_device::InputEventType::Keyboard]
    }
}

/// This trait implementation allows the [KeymapHandler] to be hooked up into the input
/// pipeline.
#[async_trait(?Send)]
impl UnhandledInputHandler for KeymapHandler {
    async fn handle_unhandled_input_event(
        self: Rc<Self>,
        input_event: input_device::UnhandledInputEvent,
    ) -> Vec<input_device::InputEvent> {
        fuchsia_trace::duration!("input", "keymap_handler");
        match input_event {
            // Decorate a keyboard event with key meaning.
            input_device::UnhandledInputEvent {
                device_event: input_device::InputDeviceEvent::Keyboard(event),
                device_descriptor,
                event_time,
                trace_id,
            } => {
                fuchsia_trace::duration!("input", "keymap_handler[processing]");
                self.inspect_status.count_received_event(&event_time);
                vec![input_device::InputEvent::from(self.process_keyboard_event(
                    event,
                    device_descriptor,
                    event_time,
                    trace_id,
                ))]
            }
            // Pass other events unchanged.
            _ => {
                self.metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::HandlerReceivedUninterestedEvent,
                    std::format!(
                        "{} uninterested input event: {:?}",
                        self.get_name(),
                        input_event.get_event_type()
                    ),
                );
                vec![input_device::InputEvent::from(input_event)]
            }
        }
    }
}

impl KeymapHandler {
    /// Creates a new instance of the keymap handler.
    pub fn new(
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Rc<Self> {
        let inspect_status = InputHandlerStatus::new(
            input_handlers_node,
            "keymap_handler",
            /* generates_events */ false,
        );
        Rc::new(Self {
            modifier_state: Default::default(),
            lock_state: Default::default(),
            metrics_logger,
            inspect_status,
        })
    }

    /// Attaches a key meaning to each passing keyboard event.
    fn process_keyboard_event(
        self: &Rc<Self>,
        event: keyboard_binding::KeyboardEvent,
        device_descriptor: input_device::InputDeviceDescriptor,
        event_time: zx::MonotonicInstant,
        trace_id: Option<fuchsia_trace::Id>,
    ) -> input_device::UnhandledInputEvent {
        if let Some(trace_id) = trace_id {
            fuchsia_trace::flow_step!("input", "event_in_input_pipeline", trace_id.into());
        }

        let (key, event_type) = (event.get_key(), event.get_event_type());
        log::debug!(
            concat!(
                "Keymap::process_keyboard_event: key:{:?}, ",
                "modifier_state:{:?}, lock_state: {:?}, event_type: {:?}"
            ),
            key,
            self.modifier_state.borrow(),
            self.lock_state.borrow(),
            event_type
        );

        self.modifier_state.borrow_mut().update(event_type, key);
        self.lock_state.borrow_mut().update(event_type, key);
        let key_meaning =
            keymaps::apply(key, &*self.modifier_state.borrow(), &*self.lock_state.borrow());
        input_device::UnhandledInputEvent {
            device_event: input_device::InputDeviceEvent::Keyboard(
                event.into_with_key_meaning(key_meaning),
            ),
            device_descriptor,
            event_time,
            trace_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_handler::InputHandler;
    use crate::{consumer_controls_binding, testing_utilities};
    use fidl_fuchsia_input as finput;
    use fidl_fuchsia_ui_input3 as finput3;
    use pretty_assertions::assert_eq;
    use std::convert::TryFrom as _;
    use zx;

    // A mod-specific version of `testing_utilities::create_keyboard_event`.
    fn create_unhandled_keyboard_event(
        key: finput::Key,
        event_type: finput3::KeyEventType,
        keymap: Option<String>,
    ) -> input_device::UnhandledInputEvent {
        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![finput::Key::A, finput::Key::B],
                ..Default::default()
            },
        );
        let (_, event_time_u64) = testing_utilities::event_times();
        input_device::UnhandledInputEvent::try_from(
            testing_utilities::create_keyboard_event_with_time(
                key,
                event_type,
                /* modifiers= */ None,
                event_time_u64,
                &device_descriptor,
                keymap,
            ),
        )
        .unwrap()
    }

    // A mod-specific version of `testing_utilities::create_consumer_controls_event`.
    fn create_unhandled_consumer_controls_event(
        pressed_buttons: Vec<fidl_fuchsia_input::ConsumerControlButton>,
        event_time: zx::MonotonicInstant,
        device_descriptor: &input_device::InputDeviceDescriptor,
    ) -> input_device::UnhandledInputEvent {
        input_device::UnhandledInputEvent::try_from(
            testing_utilities::create_consumer_controls_event(
                pressed_buttons,
                event_time,
                device_descriptor,
            ),
        )
        .unwrap()
    }

    fn get_key_meaning(event: &input_device::InputEvent) -> Option<finput3::KeyMeaning> {
        match event {
            input_device::InputEvent {
                device_event: input_device::InputDeviceEvent::Keyboard(event),
                ..
            } => event.get_key_meaning(),
            _ => None,
        }
    }

    #[fuchsia::test]
    async fn test_keymap_application() {
        // Not using test_case crate because it does not compose very well with
        // async test execution.
        #[derive(Debug)]
        struct TestCase {
            events: Vec<input_device::UnhandledInputEvent>,
            expected: Vec<Option<finput3::KeyMeaning>>,
        }
        let tests: Vec<TestCase> = vec![
            TestCase {
                events: vec![create_unhandled_keyboard_event(
                    finput::Key::A,
                    finput3::KeyEventType::Pressed,
                    Some("US_QWERTY".into()),
                )],
                expected: vec![
                    Some(finput3::KeyMeaning::Codepoint(97)), // a
                ],
            },
            TestCase {
                // A non-keyboard event.
                events: vec![create_unhandled_consumer_controls_event(
                    vec![],
                    zx::MonotonicInstant::ZERO,
                    &input_device::InputDeviceDescriptor::ConsumerControls(
                        consumer_controls_binding::ConsumerControlsDeviceDescriptor {
                            buttons: vec![fidl_fuchsia_input::ConsumerControlButton::VolumeUp],
                            device_id: 1,
                            is_injected: false,
                        },
                    ),
                )],
                expected: vec![None],
            },
            TestCase {
                events: vec![
                    create_unhandled_keyboard_event(
                        finput::Key::LeftShift,
                        finput3::KeyEventType::Pressed,
                        Some("US_QWERTY".into()),
                    ),
                    create_unhandled_keyboard_event(
                        finput::Key::A,
                        finput3::KeyEventType::Pressed,
                        Some("US_QWERTY".into()),
                    ),
                ],
                expected: vec![
                    Some(finput3::KeyMeaning::NonPrintableKey(finput3::NonPrintableKey::Shift)),
                    Some(finput3::KeyMeaning::Codepoint(65)), // A
                ],
            },
            TestCase {
                events: vec![
                    create_unhandled_keyboard_event(
                        finput::Key::Tab,
                        finput3::KeyEventType::Pressed,
                        Some("US_QWERTY".into()),
                    ),
                    create_unhandled_keyboard_event(
                        finput::Key::A,
                        finput3::KeyEventType::Pressed,
                        Some("US_QWERTY".into()),
                    ),
                ],
                expected: vec![
                    Some(finput3::KeyMeaning::NonPrintableKey(finput3::NonPrintableKey::Tab)),
                    Some(finput3::KeyMeaning::Codepoint(97)), // a
                ],
            },
        ];
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        for test in &tests {
            let mut actual: Vec<Option<finput3::KeyMeaning>> = vec![];
            let handler = KeymapHandler::new(&test_node, metrics::MetricsLogger::default());
            for event in &test.events {
                let mut result = handler
                    .clone()
                    .handle_unhandled_input_event(event.clone())
                    .await
                    .iter()
                    .map(get_key_meaning)
                    .collect();
                actual.append(&mut result);
            }
            assert_eq!(test.expected, actual);
        }
    }

    #[fuchsia::test]
    async fn keymap_handler_initialized_with_inspect_node() {
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let _handler = KeymapHandler::new(&fake_handlers_node, metrics::MetricsLogger::default());
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                keymap_handler: {
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
    async fn keymap_handler_inspect_counts_events() {
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let keymap_handler =
            KeymapHandler::new(&fake_handlers_node, metrics::MetricsLogger::default());
        let device_descriptor = input_device::InputDeviceDescriptor::Keyboard(
            keyboard_binding::KeyboardDeviceDescriptor {
                keys: vec![finput::Key::A, finput::Key::B],
                ..Default::default()
            },
        );
        let (_, event_time_u64) = testing_utilities::event_times();
        let input_events = vec![
            testing_utilities::create_keyboard_event_with_time(
                finput::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            // Should not count received events that have already been handled.
            testing_utilities::create_keyboard_event_with_handled(
                finput::Key::B,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
                /* key_meaning= */ None,
                input_device::Handled::Yes,
            ),
            testing_utilities::create_keyboard_event_with_time(
                finput::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Released,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
            // Should not count non-keyboard input events.
            testing_utilities::create_fake_input_event(event_time_u64),
            testing_utilities::create_keyboard_event_with_time(
                finput::Key::B,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                event_time_u64,
                &device_descriptor,
                /* keymap= */ None,
            ),
        ];

        for input_event in input_events {
            let _ = keymap_handler.clone().handle_input_event(input_event).await;
        }

        let last_event_timestamp: u64 = event_time_u64.into_nanos().try_into().unwrap();

        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                keymap_handler: {
                    events_received_count: 3u64,
                    events_handled_count: 0u64,
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
}
