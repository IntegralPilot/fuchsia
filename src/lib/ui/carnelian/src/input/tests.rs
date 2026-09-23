// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::drawing::DisplayRotation;
use crate::geometry::{IntPoint, IntSize};
use euclid::size2;

mod mouse_tests {
    use super::*;
    use crate::input::{Button, ButtonSet, Event, EventType, mouse};

    pub fn create_test_mouse_event(button: u8) -> Event {
        let mouse_event = mouse::Event {
            buttons: ButtonSet::default(),
            phase: mouse::Phase::Down(Button(button)),
            location: IntPoint::zero(),
        };
        Event {
            event_time: 0,
            device_id: device_id_tests::create_test_device_id(),
            event_type: EventType::Mouse(mouse_event),
        }
    }
}

mod touch_tests {
    use super::*;
    use crate::input::touch;

    pub fn create_test_contact() -> touch::Contact {
        touch::Contact {
            contact_id: touch::ContactId(100),
            phase: touch::Phase::Down(IntPoint::zero(), IntSize::zero()),
        }
    }
}

mod pointer_tests {
    use super::*;
    use crate::input::{EventType, pointer};

    #[test]
    fn test_pointer_from_mouse() {
        for button in 1..3 {
            let event = mouse_tests::create_test_mouse_event(button);
            match event.event_type {
                EventType::Mouse(mouse_event) => {
                    let pointer_event =
                        pointer::Event::new_from_mouse_event(&event.device_id, &mouse_event);
                    assert_eq!(pointer_event.is_some(), button == 1);
                }
                _ => panic!("I asked for a mouse event"),
            }
        }
    }

    #[test]
    fn test_pointer_from_contact() {
        let contact = touch_tests::create_test_contact();
        let pointer_event = pointer::Event::new_from_contact(&contact);
        match pointer_event.phase {
            pointer::Phase::Down(location) => {
                assert_eq!(location, IntPoint::zero());
            }
            _ => panic!("This should have been a down pointer event"),
        }
    }
}

mod device_id_tests {
    use crate::input::DeviceId;

    pub(crate) fn create_test_device_id() -> DeviceId {
        DeviceId("test-device-id-1".to_string())
    }
}

mod input_report_tests {
    use super::*;
    use crate::app::strategies::framebuffer::AutoRepeatTimer;
    use crate::input::report::{InputReportHandler, TouchScale};
    use crate::input::{DeviceId, EventType, consumer_control, keyboard};
    use itertools::assert_equal;

    struct TestAutoRepeatContext;

    impl AutoRepeatTimer for TestAutoRepeatContext {
        fn schedule_autorepeat_timer(&mut self, _device_id: &DeviceId) {}
        fn continue_autorepeat_timer(&mut self, _device_id: &DeviceId) {}
    }

    fn make_input_handler() -> InputReportHandler<'static> {
        let test_size = size2(1024, 768);
        let touch_scale = TouchScale {
            target_size: test_size,
            x: fidl_fuchsia_input::Range { min: 0, max: 4095 },
            x_span: 4095.0,
            y: fidl_fuchsia_input::Range { min: 0, max: 4095 },
            y_span: 4095.0,
        };
        InputReportHandler::new_with_scale(
            device_id_tests::create_test_device_id(),
            test_size,
            DisplayRotation::Deg0,
            Some(touch_scale),
            &keymaps::US_QWERTY,
        )
    }

    #[test]
    fn test_typed_string() {
        let reports = test_data::hello_world_keyboard_reports();

        let mut context = TestAutoRepeatContext;

        let mut input_handler = make_input_handler();

        let device_id = DeviceId("keyboard-1".to_string());
        let chars_from_events = reports
            .iter()
            .map(|input_report| {
                input_handler.handle_input_report(&device_id, input_report, &mut context)
            })
            .flatten()
            .filter_map(|event| match event.event_type {
                EventType::Keyboard(keyboard_event) => match keyboard_event.phase {
                    keyboard::Phase::Pressed => keyboard_event
                        .code_point
                        .and_then(|code_point| Some(code_point as u8 as char)),
                    _ => None,
                },
                _ => None,
            });

        assert_equal("Hello World".chars(), chars_from_events);
    }

    #[test]
    fn test_consumer_control() {
        use fidl_fuchsia_input::ConsumerControlButton::{VolumeDown, VolumeUp};
        let reports = test_data::consumer_control_input_reports();

        let device_id = DeviceId("cc-1".to_string());

        let mut context = TestAutoRepeatContext;

        let mut input_handler = make_input_handler();
        let events: Vec<(consumer_control::Phase, fidl_fuchsia_input::ConsumerControlButton)> =
            reports
                .iter()
                .map(|input_report| {
                    input_handler.handle_input_report(&device_id, input_report, &mut context)
                })
                .flatten()
                .filter_map(|event| match event.event_type {
                    EventType::ConsumerControl(consumer_control_event) => {
                        Some((consumer_control_event.phase, consumer_control_event.button))
                    }
                    _ => None,
                })
                .collect();

        let expected = [
            (consumer_control::Phase::Down, VolumeUp),
            (consumer_control::Phase::Up, VolumeUp),
            (consumer_control::Phase::Down, VolumeDown),
            (consumer_control::Phase::Up, VolumeDown),
        ];
        assert_eq!(events, expected);
    }
}

mod input_tests {
    use super::*;
    use crate::input::key3::KeyboardInputHandler;
    use crate::input::{EventType, keyboard};
    use itertools::assert_equal;

    #[test]
    fn test_typed_string() {
        let events = test_data::hello_world_input_events();

        let mut keyboard_input_handler = KeyboardInputHandler::new();
        let chars_from_events = events
            .iter()
            .map(|event| keyboard_input_handler.handle_key_event(event))
            .flatten()
            .filter_map(|event| match event.event_type {
                EventType::Keyboard(keyboard_event) => match keyboard_event.phase {
                    keyboard::Phase::Pressed => keyboard_event
                        .code_point
                        .and_then(|code_point| Some(code_point as u8 as char)),
                    _ => None,
                },
                _ => None,
            });

        assert_equal("Hello World".chars(), chars_from_events);
    }

    #[test]
    fn test_control_r() {
        let events = test_data::control_r_events();

        // make sure there's one and only one keydown even of the r
        // key with the control modifier set.
        let mut keyboard_input_handler = KeyboardInputHandler::new();
        let expected_event_count: usize = events
            .iter()
            .map(|event| keyboard_input_handler.handle_key_event(event))
            .flatten()
            .filter_map(|event| match event.event_type {
                EventType::Keyboard(keyboard_event) => match keyboard_event.phase {
                    keyboard::Phase::Pressed => {
                        if keyboard_event.hid_usage == keymaps::usages::Usages::HidUsageKeyR as u32
                            && keyboard_event.modifiers.control
                        {
                            Some(())
                        } else {
                            None
                        }
                    }
                    _ => None,
                },
                _ => None,
            })
            .count();

        assert_eq!(expected_event_count, 1);
    }
}

mod test_data {
    pub fn consumer_control_input_reports() -> Vec<fidl_fuchsia_input_report::InputReport> {
        use fidl_fuchsia_input::ConsumerControlButton::{VolumeDown, VolumeUp};
        use fidl_fuchsia_input_report::{ConsumerControlInputReport, InputReport};
        vec![
            InputReport {
                event_time: Some(66268999833),
                mouse: None,
                trace_id: Some(2),
                sensor: None,
                touch: None,
                keyboard: None,
                consumer_control: Some(ConsumerControlInputReport {
                    pressed_buttons: Some([VolumeUp].to_vec()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            InputReport {
                event_time: Some(66434561666),
                mouse: None,
                trace_id: Some(3),
                sensor: None,
                touch: None,
                keyboard: None,
                consumer_control: Some(ConsumerControlInputReport {
                    pressed_buttons: Some([].to_vec()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            InputReport {
                event_time: Some(358153537000),
                mouse: None,
                trace_id: Some(4),
                sensor: None,
                touch: None,
                keyboard: None,
                consumer_control: Some(ConsumerControlInputReport {
                    pressed_buttons: Some([VolumeDown].to_vec()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            InputReport {
                event_time: Some(358376260958),
                mouse: None,
                trace_id: Some(5),
                sensor: None,
                touch: None,
                keyboard: None,
                consumer_control: Some(ConsumerControlInputReport {
                    pressed_buttons: Some([].to_vec()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ]
    }
    pub fn hello_world_keyboard_reports() -> Vec<fidl_fuchsia_input_report::InputReport> {
        use fidl_fuchsia_input::Key::*;
        use fidl_fuchsia_input_report::{InputReport, KeyboardInputReport};
        vec![
            InputReport {
                event_time: Some(85446402710730),
                mouse: None,
                trace_id: Some(189),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![LeftShift]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85446650713601),
                mouse: None,
                trace_id: Some(191),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![LeftShift, H]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85446738712880),
                mouse: None,
                trace_id: Some(193),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![LeftShift]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85446794702907),
                mouse: None,
                trace_id: Some(195),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85446970709193),
                mouse: None,
                trace_id: Some(197),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![E]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85447090710657),
                mouse: None,
                trace_id: Some(199),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85447330708990),
                mouse: None,
                trace_id: Some(201),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![L]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85447394712460),
                mouse: None,
                trace_id: Some(203),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85447508813465),
                mouse: None,
                trace_id: Some(205),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![L]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85447618712982),
                mouse: None,
                trace_id: Some(207),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85447810705156),
                mouse: None,
                trace_id: Some(209),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![O]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85447898703263),
                mouse: None,
                trace_id: Some(211),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85450082706011),
                mouse: None,
                trace_id: Some(213),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![Space]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85450156060503),
                mouse: None,
                trace_id: Some(215),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85450418710803),
                mouse: None,
                trace_id: Some(217),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![LeftShift]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85450594712232),
                mouse: None,
                trace_id: Some(219),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![LeftShift, W]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85450746707982),
                mouse: None,
                trace_id: Some(221),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![W]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85450794706822),
                mouse: None,
                trace_id: Some(223),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85450962706591),
                mouse: None,
                trace_id: Some(225),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![O]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85451050703903),
                mouse: None,
                trace_id: Some(227),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85451282710803),
                mouse: None,
                trace_id: Some(229),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![R]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85451411293149),
                mouse: None,
                trace_id: Some(231),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85451842714565),
                mouse: None,
                trace_id: Some(233),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![L]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85451962704075),
                mouse: None,
                trace_id: Some(235),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85452906710709),
                mouse: None,
                trace_id: Some(237),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![D]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85453034711602),
                mouse: None,
                trace_id: Some(239),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85454778708461),
                mouse: None,
                trace_id: Some(241),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![Enter]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
            InputReport {
                event_time: Some(85454858706151),
                mouse: None,
                trace_id: Some(243),
                sensor: None,
                touch: None,
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                consumer_control: None,
                ..Default::default()
            },
        ]
    }

    pub fn hello_world_input_events() -> Vec<fidl_fuchsia_ui_input3::KeyEvent> {
        use fidl_fuchsia_input::Key::*;
        use fidl_fuchsia_ui_input3::KeyEvent;
        use fidl_fuchsia_ui_input3::KeyEventType::{Pressed, Released};
        vec![
            KeyEvent {
                timestamp: Some(3264387612285),
                type_: Some(Pressed),
                key: Some(LeftShift),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3265130500125),
                type_: Some(Pressed),
                key: Some(H),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3265266507731),
                type_: Some(Released),
                key: Some(H),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3265370503901),
                type_: Some(Released),
                key: Some(LeftShift),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3266834499940),
                type_: Some(Pressed),
                key: Some(E),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3266962508842),
                type_: Some(Released),
                key: Some(E),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3267154500453),
                type_: Some(Pressed),
                key: Some(L),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3267219444859),
                type_: Some(Released),
                key: Some(L),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3267346499392),
                type_: Some(Pressed),
                key: Some(L),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3267458502427),
                type_: Some(Released),
                key: Some(L),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3267690502669),
                type_: Some(Pressed),
                key: Some(O),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3267858501367),
                type_: Some(Released),
                key: Some(O),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275178512511),
                type_: Some(Pressed),
                key: Some(Space),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275274501635),
                type_: Some(Pressed),
                key: Some(LeftShift),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275298499697),
                type_: Some(Released),
                key: Some(Space),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275474504423),
                type_: Some(Pressed),
                key: Some(W),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275586502431),
                type_: Some(Released),
                key: Some(LeftShift),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275634500151),
                type_: Some(Released),
                key: Some(W),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275714502408),
                type_: Some(Pressed),
                key: Some(O),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275834561768),
                type_: Some(Released),
                key: Some(O),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3275858499854),
                type_: Some(Pressed),
                key: Some(R),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3276018509754),
                type_: Some(Released),
                key: Some(R),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3276114540325),
                type_: Some(Pressed),
                key: Some(L),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3276282504845),
                type_: Some(Released),
                key: Some(L),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3276578503737),
                type_: Some(Pressed),
                key: Some(D),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(3276706501366),
                type_: Some(Released),
                key: Some(D),
                modifiers: None,
                ..Default::default()
            },
        ]
    }

    pub fn control_r_events() -> Vec<fidl_fuchsia_ui_input3::KeyEvent> {
        use fidl_fuchsia_input::Key::*;
        use fidl_fuchsia_ui_input3::KeyEvent;
        use fidl_fuchsia_ui_input3::KeyEventType::{Pressed, Released};
        vec![
            KeyEvent {
                timestamp: Some(4453530520711),
                type_: Some(Pressed),
                key: Some(LeftCtrl),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(4454138519645),
                type_: Some(Pressed),
                key: Some(R),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(4454730534107),
                type_: Some(Released),
                key: Some(R),
                modifiers: None,
                ..Default::default()
            },
            KeyEvent {
                timestamp: Some(4454738498944),
                type_: Some(Released),
                key: Some(LeftCtrl),
                modifiers: None,
                ..Default::default()
            },
        ]
    }
}
