// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::keymap::KEY_MAP;
use fidl_fuchsia_ui_input3 as fuiinput;
use starnix_types::time::timeval_from_time;
use starnix_uapi::uapi;

/// Converts fuchsia KeyEven to a vector of `uapi::input_events`.
///
/// A single `KeyEvent` may translate into multiple `uapi::input_events`.
/// 1 key event and 1 sync event.
///
/// If translation fails an empty vector is returned.
pub fn parse_fidl_keyboard_event_to_linux_input_event(
    e: &fuiinput::KeyEvent,
    is_uinput_running: bool,
) -> Vec<uapi::input_event> {
    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    match e {
        &fuiinput::KeyEvent {
            timestamp: Some(time_nanos),
            type_: Some(event_type),
            key: Some(key),
            ..
        } => {
            let lkey = KEY_MAP.fuchsia_input_key_to_linux_keycode(key);
            // return empty for unknown keycode.
            if lkey == uapi::KEY_RESERVED {
                return vec![];
            }
            let lkey = match lkey {
                // TODO(b/312467059): keep this ESC -> Power workaround for debug.
                uapi::KEY_ESC => {
                    if is_uinput_running {
                        uapi::KEY_ESC
                    } else {
                        uapi::KEY_POWER
                    }
                }
                k => k,
            };

            let time = timeval_from_time(zx::MonotonicInstant::from_nanos(time_nanos));
            let key_event = uapi::input_event {
                time,
                type_: uapi::EV_KEY as u16,
                code: lkey as u16,
                value: if event_type == fuiinput::KeyEventType::Pressed { 1 } else { 0 },
            };

            let sync_event = uapi::input_event {
                // See https://www.kernel.org/doc/Documentation/input/event-codes.rst.
                time,
                type_: uapi::EV_SYN as u16,
                code: uapi::SYN_REPORT as u16,
                value: 0,
            };

            let mut events = vec![];
            events.push(key_event);
            events.push(sync_event);
            events
        }
        _ => vec![],
    }
}
