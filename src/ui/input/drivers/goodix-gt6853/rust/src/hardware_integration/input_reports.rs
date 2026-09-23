// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL input report conversions and builders for Goodix GT6853.

use crate::data_types::reports as registers;
use fidl_next_fuchsia_input_report as fidl_input_report;

/// Converts active [`registers::TouchContact`]s and an event timestamp into a FIDL
/// [`fidl_input_report::InputReport`].
pub fn make_input_report(
    event_time: zx::MonotonicInstant,
    contacts: &[registers::TouchContact],
) -> fidl_input_report::InputReport {
    let fidl_contacts: Vec<fidl_input_report::ContactInputReport> =
        contacts.iter().map(Into::into).collect();
    let touch =
        fidl_input_report::TouchInputReport { contacts: Some(fidl_contacts), ..Default::default() };
    fidl_input_report::InputReport {
        event_time: Some(event_time.into_nanos()),
        touch: Some(touch),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_TIME_NANOS: i64 = 123_456_789;

    #[test]
    fn test_make_input_report_active_contacts() {
        let timestamp = zx::MonotonicInstant::from_nanos(EXPECTED_TIME_NANOS);
        let contacts = [
            registers::TouchContact {
                track_id_and_status: registers::TrackIdAndStatus(0x00),
                x7_0: 0x14,
                x15_8: 0x00, // 0x0014 = 20
                y7_0: 0x28,
                y15_8: 0x00, // 0x0028 = 40
                pressure: 0x0a,
                reserved: [0; 2],
            },
            registers::TouchContact {
                track_id_and_status: registers::TrackIdAndStatus(0x01),
                x7_0: 0x3c,
                x15_8: 0x00, // 0x003c = 60
                y7_0: 0x50,
                y15_8: 0x00, // 0x0050 = 80
                pressure: 0x14,
                reserved: [0; 2],
            },
        ];

        let input_report = make_input_report(timestamp, &contacts);
        assert_eq!(input_report.event_time, Some(EXPECTED_TIME_NANOS));
        let touch_report = input_report.touch.expect("missing touch report");
        let reported_contacts = touch_report.contacts.expect("missing contacts");
        assert_eq!(reported_contacts.len(), 2);
        assert_eq!(reported_contacts[0].contact_id, Some(0));
        assert_eq!(reported_contacts[0].position_x, Some(0x0014));
        assert_eq!(reported_contacts[0].position_y, Some(0x0028));
        assert_eq!(reported_contacts[1].contact_id, Some(1));
        assert_eq!(reported_contacts[1].position_x, Some(0x003c));
        assert_eq!(reported_contacts[1].position_y, Some(0x0050));
        assert_eq!(touch_report.pressed_buttons, None);
    }

    #[test]
    fn test_make_input_report_empty_release() {
        let timestamp = zx::MonotonicInstant::from_nanos(EXPECTED_TIME_NANOS);
        let input_report = make_input_report(timestamp, &[]);
        assert_eq!(input_report.event_time, Some(EXPECTED_TIME_NANOS));
        let touch_report = input_report.touch.expect("missing touch report");
        let reported_contacts = touch_report.contacts.expect("missing contacts");
        assert!(reported_contacts.is_empty());
        assert_eq!(touch_report.pressed_buttons, None);
    }
}
