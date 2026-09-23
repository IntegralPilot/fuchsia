// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::keymap::KEY_MAP;
use fidl_fuchsia_input::Key;
use fidl_fuchsia_input_report as fir;
use starnix_logging::log_warn;
use starnix_types::time::time_from_timeval;
use starnix_uapi::errors::Errno;
use starnix_uapi::{error, uapi};

/// A state machine accepts uapi::input_event, produces fir::InputReport
/// when (Key Event + Sync Event) received. It also maintain the currently
/// pressing key list.
///
/// Warning output, clean state and return errno when received events:
/// - unknown keycode.
/// - invalid event.
/// - not follow (Key Event + Sync Event) pattern.
#[derive(Debug, PartialEq)]
pub struct LinuxKeyboardEventParser {
    cached_event: Option<uapi::input_event>,
    pressing_keys: Vec<Key>,
}

impl LinuxKeyboardEventParser {
    pub fn create() -> Self {
        Self { cached_event: None, pressing_keys: vec![] }
    }

    fn reset_state(&mut self) {
        self.cached_event = None;
        self.pressing_keys = vec![];
    }

    fn produce_input_report(
        &mut self,
        e: uapi::input_event,
    ) -> Result<Option<fir::InputReport>, Errno> {
        self.cached_event = None;

        let fkey = KEY_MAP.linux_keycode_to_fuchsia_input_key(e.code as u32);
        // produce no input report for unknown key, there is a warning log from
        // linux_keycode_to_fuchsia_input_key().
        if fkey == Key::Unknown {
            self.reset_state();
            return error!(EINVAL);
        }
        match e.value {
            // Press
            1 => {
                if self.pressing_keys.contains(&fkey) {
                    log_warn!(
                        "keyboard receive a press key event while the key is already pressing, key = {:?}",
                        fkey
                    );
                    self.reset_state();
                    return error!(EINVAL);
                }
                self.pressing_keys.push(fkey);
            }
            // Release
            0 => {
                if !self.pressing_keys.contains(&fkey) {
                    log_warn!(
                        "keyboard receive a release key event while the key is not pressing, key = {:?}",
                        fkey
                    );
                    self.reset_state();
                    return error!(EINVAL);
                }
                // remove the released key.
                self.pressing_keys =
                    self.pressing_keys.clone().into_iter().filter(|x| *x != fkey).collect();
            }
            _ => {
                log_warn!("key event has invalid value field, event = {:?}", e);
                self.reset_state();
                return error!(EINVAL);
            }
        }

        let keyboard_report = fir::KeyboardInputReport {
            pressed_keys3: Some(self.pressing_keys.clone()),
            ..Default::default()
        };

        Ok(Some(fir::InputReport {
            event_time: Some(time_from_timeval::<zx::MonotonicTimeline>(e.time)?.into_nanos()),
            keyboard: Some(keyboard_report),
            ..Default::default()
        }))
    }

    pub fn handle(&mut self, e: uapi::input_event) -> Result<Option<fir::InputReport>, Errno> {
        match self.cached_event {
            Some(key_event) => match e.type_ as u32 {
                uapi::EV_SYN => self.produce_input_report(key_event),
                _ => {
                    self.reset_state();
                    log_warn!("keyboard expect EV_SYN event but got = {:?}", e);
                    error!(EINVAL)
                }
            },
            None => match e.type_ as u32 {
                uapi::EV_KEY => {
                    self.cached_event = Some(e);
                    Ok(None)
                }
                _ => {
                    self.reset_state();
                    log_warn!("keyboard expect EV_KEY event but got = {:?}", e);
                    error!(EINVAL)
                }
            },
        }
    }
}

#[cfg(test)]
mod key_linux_fuchsia_tests {
    use super::*;
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use test_case::test_case;
    use uapi::timeval;

    fn uapi_input_event(ty: u32, code: u32, value: i32) -> uapi::input_event {
        uapi::input_event { time: timeval::default(), type_: ty as u16, code: code as u16, value }
    }

    #[test]
    fn parse_linux_events_to_fidl_keyboard_event_send_syn_when_no_cached_event() {
        let mut linux_keyboard_event_parser = LinuxKeyboardEventParser::create();
        let e = uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);
        let res = linux_keyboard_event_parser.handle(e);
        assert_eq!(res, error!(EINVAL));
    }

    #[test_case(
        uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 1);
        "press")]
    #[test_case(
        uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 0);
        "release, not fail on this step")]
    #[test_case(
        uapi_input_event(uapi::EV_KEY, uapi::KEY_RESERVED, 1);
        "unknown keycode, not fail on this step")]
    fn parse_linux_events_to_fidl_keyboard_event_send_key_when_no_cached_event_and_no_pressing_keys(
        e: uapi::input_event,
    ) {
        let mut linux_keyboard_event_parser = LinuxKeyboardEventParser::create();
        let res = linux_keyboard_event_parser.handle(e);
        assert_eq!(res, Ok(None));
        assert_eq!(
            linux_keyboard_event_parser,
            LinuxKeyboardEventParser { cached_event: Some(e), pressing_keys: vec![] },
        );
    }

    #[test]
    fn parse_linux_events_to_fidl_keyboard_event_send_syn_when_have_cached_event_and_no_pressing_keys()
     {
        let mut linux_keyboard_event_parser = LinuxKeyboardEventParser::create();
        let e = uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 1);
        let res = linux_keyboard_event_parser.handle(e);
        assert_eq!(res, Ok(None));

        let e = uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);
        let res = linux_keyboard_event_parser.handle(e);
        assert_eq!(
            res,
            Ok(Some(fir::InputReport {
                event_time: Some(0),
                keyboard: Some(fir::KeyboardInputReport {
                    pressed_keys3: Some(vec![Key::A]),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        );
        assert_eq!(
            linux_keyboard_event_parser,
            LinuxKeyboardEventParser { cached_event: None, pressing_keys: vec![Key::A] },
        );
    }

    #[test_case(
        uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 0);
        "release not pressing")]
    #[test_case(
        uapi_input_event(uapi::EV_KEY, uapi::KEY_RESERVED, 1);
        "unknown keycode")]
    fn parse_linux_events_to_fidl_keyboard_event_send_syn_when_have_cached_event_and_no_pressing_keys_failed(
        cached: uapi::input_event,
    ) {
        let mut linux_keyboard_event_parser = LinuxKeyboardEventParser::create();
        let res = linux_keyboard_event_parser.handle(cached);
        assert_eq!(res, Ok(None));

        let e = uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);
        let res = linux_keyboard_event_parser.handle(e);
        assert_eq!(res, error!(EINVAL));
        assert_eq!(
            linux_keyboard_event_parser,
            LinuxKeyboardEventParser { cached_event: None, pressing_keys: vec![] },
        );
    }

    #[test]
    fn parse_linux_events_to_fidl_keyboard_event_send_key_when_have_cached_event() {
        let mut linux_keyboard_event_parser = LinuxKeyboardEventParser::create();
        let e = uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 1);
        let res = linux_keyboard_event_parser.handle(e);
        assert_eq!(res, Ok(None));

        let e = uapi_input_event(uapi::EV_KEY, uapi::KEY_B, 1);
        let res = linux_keyboard_event_parser.handle(e);
        assert_eq!(res, error!(EINVAL));
        assert_eq!(
            linux_keyboard_event_parser,
            LinuxKeyboardEventParser { cached_event: None, pressing_keys: vec![] },
        );
    }

    #[test]
    fn parse_linux_events_to_fidl_keyboard_event_press_pressing_key() {
        let mut linux_keyboard_event_parser = LinuxKeyboardEventParser::create();
        let press_a = uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 1);
        let res = linux_keyboard_event_parser.handle(press_a);
        assert_eq!(res, Ok(None));

        let syn = uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);
        let res = linux_keyboard_event_parser.handle(syn);
        assert_matches!(res, Ok(Some(_)));

        let res = linux_keyboard_event_parser.handle(press_a);
        assert_eq!(res, Ok(None));
        let res = linux_keyboard_event_parser.handle(syn);
        assert_eq!(res, error!(EINVAL));
        assert_eq!(
            linux_keyboard_event_parser,
            LinuxKeyboardEventParser { cached_event: None, pressing_keys: vec![] },
        );
    }

    #[test]
    fn parse_linux_events_to_fidl_keyboard_event_release_key() {
        let mut linux_keyboard_event_parser = LinuxKeyboardEventParser::create();
        let press_a = uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 1);
        let res = linux_keyboard_event_parser.handle(press_a);
        assert_eq!(res, Ok(None));

        let syn = uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);
        let res = linux_keyboard_event_parser.handle(syn);
        assert_matches!(res, Ok(Some(_)));

        let release_a = uapi_input_event(uapi::EV_KEY, uapi::KEY_A, 0);
        let res = linux_keyboard_event_parser.handle(release_a);
        assert_eq!(res, Ok(None));
        let res = linux_keyboard_event_parser.handle(syn);
        assert_eq!(
            res,
            Ok(Some(fir::InputReport {
                event_time: Some(0),
                keyboard: Some(fir::KeyboardInputReport {
                    pressed_keys3: Some(vec![]),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        );
        assert_eq!(
            linux_keyboard_event_parser,
            LinuxKeyboardEventParser { cached_event: None, pressing_keys: vec![] },
        );
    }
}
