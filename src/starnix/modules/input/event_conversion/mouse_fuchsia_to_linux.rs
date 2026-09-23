// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ui_pointer::MouseEvent as FidlMouseEvent;
use sorted_vec_map::SortedVecSet;
use starnix_types::time::timeval_from_time;
use starnix_uapi::uapi;
use std::collections::VecDeque;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FuchsiaMouseEventToLinuxMouseEventConverter {
    currently_pressed_buttons: SortedVecSet<u8>,
}

pub struct LinuxMouseEventBatch {
    pub events: VecDeque<uapi::input_event>,
    pub count_ignored_events: u64,
    pub count_converted_events: u64,
    pub count_unexpected_events: u64,
    pub last_event_time_ns: i64,
}

/// Maps Fuchsia mouse button IDs to Linux input event key codes.
///
/// Returns None for unsupported or unrecognized button IDs so callers can
/// gracefully ignore them.
pub fn fuchsia_mouse_button_to_linux_button(button: u8) -> Option<u16> {
    match button {
        1 => Some(uapi::BTN_LEFT as u16),
        2 => Some(uapi::BTN_RIGHT as u16),
        3 => Some(uapi::BTN_MIDDLE as u16),
        4 => Some(uapi::BTN_SIDE as u16),
        5 => Some(uapi::BTN_EXTRA as u16),
        _ => None,
    }
}

impl FuchsiaMouseEventToLinuxMouseEventConverter {
    pub fn create() -> Self {
        Self::default()
    }

    /// Converts a batch of FIDL mouse events to Linux input events in a single pass.
    ///
    /// TODO(https://fxbug.dev/563345995): Batching was introduced as a performance
    /// enhancement for scroll wheel events. Aggregating pointer and button events the
    /// same way can have unintended effects: rapid click sequences may be collapsed,
    /// and relative motion deltas that negate one another within a batch sum to zero
    /// and are dropped entirely. Revisit before this path becomes load bearing.
    pub fn handle(&mut self, mouse_events: Vec<FidlMouseEvent>) -> LinuxMouseEventBatch {
        let mut count_ignored_events: u64 = 0;
        let mut count_converted_events: u64 = 0;
        let mut count_unexpected_events: u64 = 0;
        let mut last_event_time = zx::MonotonicInstant::get();

        let mut total_rel_x: i32 = 0;
        let mut total_rel_y: i32 = 0;
        let mut total_scroll_v: i32 = 0;
        let mut total_scroll_h: i32 = 0;
        let mut button_events: Vec<uapi::input_event> = Vec::new();

        for event in mouse_events {
            let Some(sample) = event.pointer_sample else {
                if event.stream_info.is_some()
                    || event.view_parameters.is_some()
                    || event.device_info.is_some()
                {
                    count_ignored_events += 1;
                } else {
                    count_unexpected_events += 1;
                }
                continue;
            };

            let event_time = match event.timestamp {
                Some(time) if time > 0 => zx::MonotonicInstant::from_nanos(time),
                _ => zx::MonotonicInstant::get(),
            };
            last_event_time = event_time;
            let time = timeval_from_time(event_time);

            let mut sample_had_data = false;

            // 1. Relative motion
            //
            // TODO(https://fxbug.dev/563345995): Rounding each sample independently
            // discards sub-pixel motion (e.g. 0.4 becomes 0), which can make pointer
            // tracking jittery or unresponsive at very low speeds. Accumulate the
            // fractional remainder across samples on the converter, as standard
            // pointer ballistics handling does.
            if let Some([rx, ry]) = sample.relative_motion {
                let dx = rx.round() as i32;
                let dy = ry.round() as i32;
                if dx != 0 {
                    total_rel_x += dx;
                    sample_had_data = true;
                }
                if dy != 0 {
                    total_rel_y += dy;
                    sample_had_data = true;
                }
            }

            // 2. Scroll wheel (vertical and horizontal)
            if let Some(ticks) = sample.scroll_v {
                if ticks != 0 {
                    total_scroll_v += ticks as i32;
                    sample_had_data = true;
                }
            }
            if let Some(ticks) = sample.scroll_h {
                if ticks != 0 {
                    total_scroll_h += ticks as i32;
                    sample_had_data = true;
                }
            }

            // 3. Buttons (transitions)
            let new_pressed_buttons: SortedVecSet<u8> =
                sample.pressed_buttons.map(|vec| vec.into_iter().collect()).unwrap_or_default();

            for &btn in new_pressed_buttons.difference(&self.currently_pressed_buttons) {
                if let Some(code) = fuchsia_mouse_button_to_linux_button(btn) {
                    button_events.push(uapi::input_event {
                        time,
                        type_: uapi::EV_KEY as u16,
                        code,
                        value: 1,
                    });
                    sample_had_data = true;
                }
            }

            for &btn in self.currently_pressed_buttons.difference(&new_pressed_buttons) {
                if let Some(code) = fuchsia_mouse_button_to_linux_button(btn) {
                    button_events.push(uapi::input_event {
                        time,
                        type_: uapi::EV_KEY as u16,
                        code,
                        value: 0,
                    });
                    sample_had_data = true;
                }
            }

            self.currently_pressed_buttons = new_pressed_buttons;

            if sample_had_data {
                count_converted_events += 1;
            } else {
                count_ignored_events += 1;
            }
        }

        let time = timeval_from_time(last_event_time);
        let mut new_events: VecDeque<uapi::input_event> = VecDeque::new();

        if total_rel_x != 0 {
            new_events.push_back(uapi::input_event {
                time,
                type_: uapi::EV_REL as u16,
                code: uapi::REL_X as u16,
                value: total_rel_x,
            });
        }
        if total_rel_y != 0 {
            new_events.push_back(uapi::input_event {
                time,
                type_: uapi::EV_REL as u16,
                code: uapi::REL_Y as u16,
                value: total_rel_y,
            });
        }
        if total_scroll_v != 0 {
            new_events.push_back(uapi::input_event {
                time,
                type_: uapi::EV_REL as u16,
                code: uapi::REL_WHEEL as u16,
                value: total_scroll_v,
            });
        }
        if total_scroll_h != 0 {
            new_events.push_back(uapi::input_event {
                time,
                type_: uapi::EV_REL as u16,
                code: uapi::REL_HWHEEL as u16,
                value: total_scroll_h,
            });
        }
        new_events.extend(button_events);

        if !new_events.is_empty() {
            new_events.push_back(uapi::input_event {
                time,
                type_: uapi::EV_SYN as u16,
                code: uapi::SYN_REPORT as u16,
                value: 0,
            });
        }

        LinuxMouseEventBatch {
            events: new_events,
            count_ignored_events,
            count_converted_events,
            count_unexpected_events,
            last_event_time_ns: last_event_time.into_nanos(),
        }
    }
}

pub fn parse_fidl_mouse_events(mouse_events: Vec<FidlMouseEvent>) -> LinuxMouseEventBatch {
    FuchsiaMouseEventToLinuxMouseEventConverter::create().handle(mouse_events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_ui_pointer::{MouseEventStreamInfo, MousePointerSample, MouseViewStatus};
    use pretty_assertions::assert_eq;

    #[test]
    fn test_mouse_wheel_event() {
        let fidl_event = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample { scroll_v: Some(1), ..Default::default() }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![fidl_event]);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_REL as u16);
        assert_eq!(batch.events[0].code, uapi::REL_WHEEL as u16);
        assert_eq!(batch.events[0].value, 1);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
        assert_eq!(batch.count_converted_events, 1);
        assert_eq!(batch.count_ignored_events, 0);
        assert_eq!(batch.count_unexpected_events, 0);
        assert_eq!(batch.last_event_time_ns, 1000);
    }

    #[test]
    fn test_mouse_horizontal_wheel_event() {
        let fidl_event = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample { scroll_h: Some(3), ..Default::default() }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![fidl_event]);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_REL as u16);
        assert_eq!(batch.events[0].code, uapi::REL_HWHEEL as u16);
        assert_eq!(batch.events[0].value, 3);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
        assert_eq!(batch.count_converted_events, 1);
        assert_eq!(batch.count_ignored_events, 0);
        assert_eq!(batch.count_unexpected_events, 0);
        assert_eq!(batch.last_event_time_ns, 1000);
    }

    #[test]
    fn test_mouse_wheel_merge() {
        let fidl_event1 = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample { scroll_v: Some(1), ..Default::default() }),
            ..Default::default()
        };
        let fidl_event2 = FidlMouseEvent {
            timestamp: Some(2000),
            pointer_sample: Some(MousePointerSample { scroll_v: Some(2), ..Default::default() }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![fidl_event1, fidl_event2]);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_REL as u16);
        assert_eq!(batch.events[0].code, uapi::REL_WHEEL as u16);
        assert_eq!(batch.events[0].value, 3);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
        assert_eq!(batch.count_converted_events, 2);
        assert_eq!(batch.count_ignored_events, 0);
        assert_eq!(batch.count_unexpected_events, 0);
        assert_eq!(batch.last_event_time_ns, 2000);
    }

    #[test]
    fn test_mouse_wheel_merge_to_zero() {
        let fidl_event1 = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample { scroll_v: Some(1), ..Default::default() }),
            ..Default::default()
        };
        let fidl_event2 = FidlMouseEvent {
            timestamp: Some(2000),
            pointer_sample: Some(MousePointerSample { scroll_v: Some(-1), ..Default::default() }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![fidl_event1, fidl_event2]);

        assert_eq!(batch.events.len(), 0);
        assert_eq!(batch.count_converted_events, 2);
        assert_eq!(batch.count_ignored_events, 0);
        assert_eq!(batch.count_unexpected_events, 0);
        assert_eq!(batch.last_event_time_ns, 2000);
    }

    #[test]
    fn test_mouse_wheel_zero_ticks() {
        let fidl_event = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample { scroll_v: Some(0), ..Default::default() }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![fidl_event]);

        assert_eq!(batch.events.len(), 0);
        assert_eq!(batch.count_converted_events, 0);
        assert_eq!(batch.count_ignored_events, 1);
        assert_eq!(batch.count_unexpected_events, 0);
    }

    #[test]
    fn test_mouse_relative_motion() {
        let fidl_event = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample {
                relative_motion: Some([10.4, -5.2]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![fidl_event]);

        assert_eq!(batch.events.len(), 3);
        assert_eq!(batch.events[0].type_, uapi::EV_REL as u16);
        assert_eq!(batch.events[0].code, uapi::REL_X as u16);
        assert_eq!(batch.events[0].value, 10);
        assert_eq!(batch.events[1].type_, uapi::EV_REL as u16);
        assert_eq!(batch.events[1].code, uapi::REL_Y as u16);
        assert_eq!(batch.events[1].value, -5);
        assert_eq!(batch.events[2].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[2].code, uapi::SYN_REPORT as u16);
        assert_eq!(batch.count_converted_events, 1);
        assert_eq!(batch.count_ignored_events, 0);
        assert_eq!(batch.count_unexpected_events, 0);
    }

    #[test]
    fn test_mouse_button_press_and_release() {
        let mut converter = FuchsiaMouseEventToLinuxMouseEventConverter::create();

        // 1. Press Left (1) and Right (2) buttons.
        let press_event = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample {
                pressed_buttons: Some(vec![1, 2]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let batch1 = converter.handle(vec![press_event]);
        assert_eq!(batch1.events.len(), 3);
        assert_eq!(batch1.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch1.events[0].code, uapi::BTN_LEFT as u16);
        assert_eq!(batch1.events[0].value, 1);
        assert_eq!(batch1.events[1].type_, uapi::EV_KEY as u16);
        assert_eq!(batch1.events[1].code, uapi::BTN_RIGHT as u16);
        assert_eq!(batch1.events[1].value, 1);
        assert_eq!(batch1.events[2].type_, uapi::EV_SYN as u16);
        assert_eq!(batch1.events[2].code, uapi::SYN_REPORT as u16);
        assert_eq!(batch1.count_converted_events, 1);

        // 2. Release Left (1) button, keep Right (2) pressed.
        let release_event = FidlMouseEvent {
            timestamp: Some(2000),
            pointer_sample: Some(MousePointerSample {
                pressed_buttons: Some(vec![2]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let batch2 = converter.handle(vec![release_event]);
        assert_eq!(batch2.events.len(), 2);
        assert_eq!(batch2.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch2.events[0].code, uapi::BTN_LEFT as u16);
        assert_eq!(batch2.events[0].value, 0);
        assert_eq!(batch2.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch2.events[1].code, uapi::SYN_REPORT as u16);
        assert_eq!(batch2.count_converted_events, 1);

        // 3. Release Right (2) button.
        let release_all = FidlMouseEvent {
            timestamp: Some(3000),
            pointer_sample: Some(MousePointerSample {
                pressed_buttons: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let batch3 = converter.handle(vec![release_all]);
        assert_eq!(batch3.events.len(), 2);
        assert_eq!(batch3.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch3.events[0].code, uapi::BTN_RIGHT as u16);
        assert_eq!(batch3.events[0].value, 0);
        assert_eq!(batch3.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch3.events[1].code, uapi::SYN_REPORT as u16);
        assert_eq!(batch3.count_converted_events, 1);
    }

    #[test]
    fn test_unsupported_mouse_button_ignored() {
        let mut converter = FuchsiaMouseEventToLinuxMouseEventConverter::create();
        let fidl_event = FidlMouseEvent {
            timestamp: Some(1000),
            pointer_sample: Some(MousePointerSample {
                pressed_buttons: Some(vec![99]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let batch = converter.handle(vec![fidl_event]);
        assert_eq!(batch.events.len(), 0);
        assert_eq!(batch.count_converted_events, 0);
        assert_eq!(batch.count_ignored_events, 1);
    }

    #[test]
    fn test_zero_timestamp_uses_monotonic_instant() {
        let fidl_event = FidlMouseEvent {
            timestamp: Some(0),
            pointer_sample: Some(MousePointerSample { scroll_v: Some(1), ..Default::default() }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![fidl_event]);
        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].code, uapi::REL_WHEEL as u16);
        assert!(batch.events[0].time.tv_sec > 0);
        assert!(batch.last_event_time_ns > 0);
    }

    #[test]
    fn test_metadata_events_ignored() {
        let stream_info_event = FidlMouseEvent {
            stream_info: Some(MouseEventStreamInfo {
                device_id: 1,
                status: MouseViewStatus::Entered,
            }),
            ..Default::default()
        };
        let batch = parse_fidl_mouse_events(vec![stream_info_event]);
        assert_eq!(batch.events.len(), 0);
        assert_eq!(batch.count_ignored_events, 1);
        assert_eq!(batch.count_unexpected_events, 0);
    }

    #[test]
    fn test_mouse_unexpected_event() {
        let fidl_event = FidlMouseEvent { timestamp: Some(1000), ..Default::default() };
        let batch = parse_fidl_mouse_events(vec![fidl_event]);

        assert_eq!(batch.events.len(), 0);
        assert_eq!(batch.count_converted_events, 0);
        assert_eq!(batch.count_ignored_events, 0);
        assert_eq!(batch.count_unexpected_events, 1);
    }
}
