// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ui_input::{MediaButtonsEvent, TouchButton, TouchButtonsEvent};
use starnix_types::time::timeval_from_time;
use starnix_uapi::uapi;

// The maximum value of the TouchButton enum.
const MAX_TOUCH_BUTTON_VALUE: u32 = 5;

// Guarantees the TouchButton bitvec can hold raw values of all TouchButton enums.
const TOUCH_BUTTON_BITVEC_SIZE: u32 = MAX_TOUCH_BUTTON_VALUE + 1;

pub fn new_touch_buttons_bitvec() -> bit_vec::BitVec {
    bit_vec::BitVec::from_elem(TOUCH_BUTTON_BITVEC_SIZE as usize, false)
}

pub struct LinuxButtonEventBatch {
    pub events: Vec<uapi::input_event>,

    // Because FIDL button events do not carry a timestamp, we perform a direct
    // clock read during conversion and assign this value as the timestamp for
    // all generated Linux button events in a single batch.
    pub event_time: zx::MonotonicInstant,
    pub power_is_pressed: bool,
    pub function_is_pressed: bool,
    pub volume_up_is_pressed: bool,
    pub volume_down_is_pressed: bool,

    // touch_buttons is of size TOUCH_BUTTON_BITVEC_SIZE to hold all TouchButton enums.
    pub touch_buttons: bit_vec::BitVec,
}

impl LinuxButtonEventBatch {
    pub fn new() -> Self {
        Self {
            events: vec![],
            event_time: zx::MonotonicInstant::get(),
            power_is_pressed: false,
            function_is_pressed: false,
            volume_up_is_pressed: false,
            volume_down_is_pressed: false,
            touch_buttons: new_touch_buttons_bitvec(),
        }
    }
}

pub fn parse_fidl_media_button_event(
    fidl_event: &MediaButtonsEvent,
    power_was_pressed: bool,
    function_was_pressed: bool,
    volume_up_was_pressed: bool,
    volume_down_was_pressed: bool,
) -> LinuxButtonEventBatch {
    let mut batch = LinuxButtonEventBatch::new();
    let time = timeval_from_time(batch.event_time);
    let sync_event = uapi::input_event {
        // See https://www.kernel.org/doc/Documentation/input/event-codes.rst.
        time,
        type_: uapi::EV_SYN as u16,
        code: uapi::SYN_REPORT as u16,
        value: 0,
    };

    batch.power_is_pressed = fidl_event.power.unwrap_or(false);
    batch.function_is_pressed = fidl_event.function.unwrap_or(false);

    let volume = fidl_event.volume.unwrap_or(0);
    batch.volume_up_is_pressed = volume > 0;
    batch.volume_down_is_pressed = volume < 0;

    for (button_was_pressed, button_is_pressed, key_code) in [
        (power_was_pressed, batch.power_is_pressed, uapi::KEY_POWER),
        (function_was_pressed, batch.function_is_pressed, uapi::KEY_VOLUMEDOWN),
        (volume_up_was_pressed, batch.volume_up_is_pressed, uapi::KEY_VOLUMEUP),
        (volume_down_was_pressed, batch.volume_down_is_pressed, uapi::KEY_VOLUMEDOWN),
    ] {
        // Button state changed. Send an event.
        if button_is_pressed != button_was_pressed {
            batch.events.push(uapi::input_event {
                time,
                type_: uapi::EV_KEY as u16,
                code: key_code as u16,
                value: button_is_pressed as i32,
            });
            batch.events.push(sync_event);
        }
    }

    batch
}

pub fn parse_fidl_touch_button_event(
    fidl_event: &TouchButtonsEvent,
    touch_buttons_were_pressed: &bit_vec::BitVec,
) -> LinuxButtonEventBatch {
    let mut batch = LinuxButtonEventBatch::new();
    let time = timeval_from_time(batch.event_time);
    let sync_event = uapi::input_event {
        // See https://www.kernel.org/doc/Documentation/input/event-codes.rst.
        time,
        type_: uapi::EV_SYN as u16,
        code: uapi::SYN_REPORT as u16,
        value: 0,
    };

    if let Some(buttons) = &fidl_event.pressed_buttons {
        for button in buttons {
            let index = button.into_primitive() as usize;
            if index < batch.touch_buttons.len() {
                batch.touch_buttons.set(index, true);
            }
        }
    };

    for (index, key_code) in [
        (TouchButton::Palm.into_primitive() as usize, uapi::KEY_SLEEP),
        (TouchButton::SwipeUp.into_primitive() as usize, uapi::KEY_UP),
        (TouchButton::SwipeLeft.into_primitive() as usize, uapi::KEY_LEFT),
        (TouchButton::SwipeRight.into_primitive() as usize, uapi::KEY_RIGHT),
        (TouchButton::SwipeDown.into_primitive() as usize, uapi::KEY_DOWN),
    ] {
        let button_was_pressed = touch_buttons_were_pressed.get(index).unwrap_or(false);
        let button_is_pressed = batch.touch_buttons.get(index).unwrap_or(false);

        // Button state changed. Send an event.
        if button_is_pressed != button_was_pressed {
            batch.events.push(uapi::input_event {
                time,
                type_: uapi::EV_KEY as u16,
                code: key_code as u16,
                value: button_is_pressed as i32,
            });
            batch.events.push(sync_event);
        }
    }

    batch
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_ensure_touch_button_max() {
        // If this test fails, it means a new TouchButton enum has been added
        // and the TOUCH_BUTTON_BITVEC_SIZE needs to be updated.
        assert_matches!(TouchButton::from_primitive(MAX_TOUCH_BUTTON_VALUE + 1), None);
    }

    #[test]
    fn test_media_button_press_power() {
        let fidl_event = MediaButtonsEvent { power: Some(true), ..Default::default() };
        let batch = parse_fidl_media_button_event(&fidl_event, false, false, false, false);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, uapi::KEY_POWER as u16);
        assert_eq!(batch.events[0].value, 1);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
    }

    #[test]
    fn test_media_button_release_power() {
        let fidl_event = MediaButtonsEvent { power: Some(false), ..Default::default() };
        let batch = parse_fidl_media_button_event(&fidl_event, true, false, false, false);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, uapi::KEY_POWER as u16);
        assert_eq!(batch.events[0].value, 0);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
    }

    #[test]
    fn test_media_button_no_change() {
        let fidl_event = MediaButtonsEvent { power: Some(true), ..Default::default() };
        let batch = parse_fidl_media_button_event(&fidl_event, true, false, false, false);

        assert_eq!(batch.events.len(), 0);
    }

    #[test]
    fn test_media_button_press_volume_up() {
        let fidl_event = MediaButtonsEvent { volume: Some(1), ..Default::default() };
        let batch = parse_fidl_media_button_event(&fidl_event, false, false, false, false);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, uapi::KEY_VOLUMEUP as u16);
        assert_eq!(batch.events[0].value, 1);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
        assert!(batch.volume_up_is_pressed);
        assert!(!batch.volume_down_is_pressed);
    }

    #[test]
    fn test_media_button_release_volume_up() {
        let fidl_event = MediaButtonsEvent { volume: Some(0), ..Default::default() };
        let batch = parse_fidl_media_button_event(&fidl_event, false, false, true, false);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, uapi::KEY_VOLUMEUP as u16);
        assert_eq!(batch.events[0].value, 0);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
        assert!(!batch.volume_up_is_pressed);
        assert!(!batch.volume_down_is_pressed);
    }

    #[test]
    fn test_media_button_press_volume_down() {
        let fidl_event = MediaButtonsEvent { volume: Some(-1), ..Default::default() };
        let batch = parse_fidl_media_button_event(&fidl_event, false, false, false, false);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, uapi::KEY_VOLUMEDOWN as u16);
        assert_eq!(batch.events[0].value, 1);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
        assert!(!batch.volume_up_is_pressed);
        assert!(batch.volume_down_is_pressed);
    }

    #[test]
    fn test_media_button_release_volume_down() {
        let fidl_event = MediaButtonsEvent { volume: Some(0), ..Default::default() };
        let batch = parse_fidl_media_button_event(&fidl_event, false, false, false, true);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, uapi::KEY_VOLUMEDOWN as u16);
        assert_eq!(batch.events[0].value, 0);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
        assert!(!batch.volume_up_is_pressed);
        assert!(!batch.volume_down_is_pressed);
    }

    use test_case::test_case;

    #[test_case(TouchButton::Palm, uapi::KEY_SLEEP; "palm")]
    #[test_case(TouchButton::SwipeUp, uapi::KEY_UP; "swipe up")]
    #[test_case(TouchButton::SwipeLeft, uapi::KEY_LEFT; "swipe left")]
    #[test_case(TouchButton::SwipeRight, uapi::KEY_RIGHT; "swipe right")]
    #[test_case(TouchButton::SwipeDown, uapi::KEY_DOWN; "swipe down")]
    fn test_touch_button_press(button: TouchButton, expected_key: u32) {
        let fidl_event =
            TouchButtonsEvent { pressed_buttons: Some(vec![button]), ..Default::default() };
        let was_pressed = bit_vec::BitVec::from_elem(6, false);
        let batch = parse_fidl_touch_button_event(&fidl_event, &was_pressed);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, expected_key as u16);
        assert_eq!(batch.events[0].value, 1);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
    }

    #[test_case(TouchButton::Palm, uapi::KEY_SLEEP; "palm")]
    #[test_case(TouchButton::SwipeUp, uapi::KEY_UP; "swipe up")]
    #[test_case(TouchButton::SwipeLeft, uapi::KEY_LEFT; "swipe left")]
    #[test_case(TouchButton::SwipeRight, uapi::KEY_RIGHT; "swipe right")]
    #[test_case(TouchButton::SwipeDown, uapi::KEY_DOWN; "swipe down")]
    fn test_touch_button_release(button: TouchButton, expected_key: u32) {
        let fidl_event = TouchButtonsEvent { pressed_buttons: Some(vec![]), ..Default::default() };
        let mut was_pressed = bit_vec::BitVec::from_elem(6, false);
        was_pressed.set(button.into_primitive() as usize, true);
        let batch = parse_fidl_touch_button_event(&fidl_event, &was_pressed);

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch.events[0].code, expected_key as u16);
        assert_eq!(batch.events[0].value, 0);
        assert_eq!(batch.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch.events[1].code, uapi::SYN_REPORT as u16);
    }

    #[test]
    fn test_touch_button_no_change() {
        let fidl_event = TouchButtonsEvent {
            pressed_buttons: Some(vec![TouchButton::Palm]),
            ..Default::default()
        };
        let mut was_pressed = bit_vec::BitVec::from_elem(6, false);
        was_pressed.set(TouchButton::Palm.into_primitive() as usize, true);
        let batch = parse_fidl_touch_button_event(&fidl_event, &was_pressed);

        assert_eq!(batch.events.len(), 0);
    }

    #[test]
    fn test_touch_button_multi_press() {
        // 1. Press Palm
        let fidl_event1 = TouchButtonsEvent {
            pressed_buttons: Some(vec![TouchButton::Palm]),
            ..Default::default()
        };
        let was_pressed1 = bit_vec::BitVec::from_elem(6, false);
        let batch1 = parse_fidl_touch_button_event(&fidl_event1, &was_pressed1);

        assert_eq!(batch1.events.len(), 2);
        assert_eq!(batch1.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch1.events[0].code, uapi::KEY_SLEEP as u16);
        assert_eq!(batch1.events[0].value, 1);
        assert_eq!(batch1.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch1.events[1].code, uapi::SYN_REPORT as u16);

        // 2. Hold Palm and press SwipeUp
        let fidl_event2 = TouchButtonsEvent {
            pressed_buttons: Some(vec![TouchButton::Palm, TouchButton::SwipeUp]),
            ..Default::default()
        };
        let was_pressed2 = batch1.touch_buttons;
        let batch2 = parse_fidl_touch_button_event(&fidl_event2, &was_pressed2);

        assert_eq!(batch2.events.len(), 2);
        assert_eq!(batch2.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch2.events[0].code, uapi::KEY_UP as u16);
        assert_eq!(batch2.events[0].value, 1);
        assert_eq!(batch2.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch2.events[1].code, uapi::SYN_REPORT as u16);

        // 3. Release Palm, hold SwipeUp
        let fidl_event3 = TouchButtonsEvent {
            pressed_buttons: Some(vec![TouchButton::SwipeUp]),
            ..Default::default()
        };
        let was_pressed3 = batch2.touch_buttons;
        let batch3 = parse_fidl_touch_button_event(&fidl_event3, &was_pressed3);

        assert_eq!(batch3.events.len(), 2);
        assert_eq!(batch3.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch3.events[0].code, uapi::KEY_SLEEP as u16);
        assert_eq!(batch3.events[0].value, 0);
        assert_eq!(batch3.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch3.events[1].code, uapi::SYN_REPORT as u16);

        // 4. Release SwipeUp
        let fidl_event4 = TouchButtonsEvent { pressed_buttons: Some(vec![]), ..Default::default() };
        let was_pressed4 = batch3.touch_buttons;
        let batch4 = parse_fidl_touch_button_event(&fidl_event4, &was_pressed4);

        assert_eq!(batch4.events.len(), 2);
        assert_eq!(batch4.events[0].type_, uapi::EV_KEY as u16);
        assert_eq!(batch4.events[0].code, uapi::KEY_UP as u16);
        assert_eq!(batch4.events[0].value, 0);
        assert_eq!(batch4.events[1].type_, uapi::EV_SYN as u16);
        assert_eq!(batch4.events[1].code, uapi::SYN_REPORT as u16);
    }
}
