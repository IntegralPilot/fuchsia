// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Real-time touch coordinate report structures and coordinate buffer controls for Goodix GT6853.

use crate::data_types::traits::{AddressableRegister, ReadableRegister, WritableRegister};
use bitfield::bitfield;
use fidl_next_fuchsia_input_report as fidl_input_report;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

bitfield! {
    /// Coordinate Information Buffer status register.
    //
    // @cite(gt6853-hardware-description): sec=3.4 title="Coordinate Information Buffer"
    // @cite(gt6853-programming-guide): sec=3.4 title="Coordinate Information"
    // @cite(gt6853-programming-guide): sec=5 title="Coordinate Reading"
    // @alias(gt6853-hardware-description): theirs="Touch_Status"
    // @alias(gt6853-programming-guide): theirs="Event Status"
    #[derive(Copy, Clone, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
    #[repr(transparent)]
    pub struct EventStatus(u8);
    impl Debug;

    /// True iff a link event is pending.
    pub link_event, _: 4;

    /// True iff a gesture event is pending.
    pub gesture_event, _: 5;

    /// True iff the touch controller has an interrupt request waiting in mailbox register.
    pub controller_request, _: 6;

    /// True iff new touch coordinates are ready.
    ///
    /// Set to false when the host finishes reading the coordinates.
    pub coordinates_ready, set_coordinates_ready: 7;
}

impl AddressableRegister for EventStatus {
    const ADDRESS: u16 = 0x4100;
}

impl ReadableRegister for EventStatus {}
impl WritableRegister for EventStatus {}

impl EventStatus {
    /// Value written to acknowledge the input report and release the buffer lock.
    pub const CLEAR: Self = Self(0x00);
}

bitfield! {
    /// Active touch contact count and status register.
    //
    // @cite(gt6853-hardware-description): sec=3.4 title="Coordinate Information Buffer"
    // @cite(gt6853-programming-guide): sec=3.4 title="Coordinate Information"
    // @cite(gt6853-programming-guide): sec=5 title="Coordinate Reading"
    // @alias(gt6853-hardware-description): theirs="Contact_Count and Status"
    // @alias(gt6853-programming-guide): theirs="Number of Points"
    #[derive(Copy, Clone, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
    #[repr(transparent)]
    pub struct ContactCount(u8);
    impl Debug;

    /// Number of active touch contacts.
    pub u8, contact_count, _: 3, 0;

    /// True iff touch key status is active.
    ///
    /// Note: This field is undocumented by vendor hardware specs.
    pub key_status, _: 4;

    /// True iff large object / palm status is active.
    ///
    /// Note: This field is undocumented by vendor hardware specs.
    pub large_status, _: 5;
}

impl AddressableRegister for ContactCount {
    const ADDRESS: u16 = 0x4101;
}

impl ReadableRegister for ContactCount {}

impl ContactCount {
    /// Returns the number of active touch contacts.
    pub fn count(&self) -> usize {
        self.contact_count() as usize
    }
}

bitfield! {
    /// Tracking ID and status byte for a touch contact point.
    //
    // @cite(gt6853-hardware-description): sec=3.4 title="Coordinate Information Buffer"
    // @cite(gt6853-programming-guide): sec=3.4 title="Coordinate Information"
    // @cite(gt6853-programming-guide): sec=5 title="Coordinate Reading"
    // @alias(gt6853-hardware-description): theirs="Track ID and Touch/Hover Status"
    // @alias(gt6853-programming-guide): theirs="track_id"
    #[derive(Copy, Clone, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
    #[repr(transparent)]
    pub struct TrackIdAndStatus(u8);
    impl Debug;

    /// Contact tracking identifier.
    pub u8, id, _: 3, 0;

    /// Touch or hover status flags.
    ///
    /// Note: This field is undocumented by vendor hardware specs.
    pub u8, status, _: 7, 6;
}

/// Touch contact point structure.
//
// @cite(gt6853-hardware-description): sec=3.4 title="Coordinate Information Buffer"
// @cite(gt6853-programming-guide): sec=3.4 title="Coordinate Information"
// @cite(gt6853-programming-guide): sec=5 title="Coordinate Reading"
// @alias(gt6853-hardware-description): theirs="Contact Blocks"
// @alias(gt6853-programming-guide): theirs="Touch Point Array"
#[derive(Copy, Clone, Debug, PartialEq, Eq, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct TouchContact {
    pub track_id_and_status: TrackIdAndStatus,
    pub x7_0: u8,
    pub x15_8: u8,
    pub y7_0: u8,
    pub y15_8: u8,
    pub pressure: u8,
    pub reserved: [u8; 2],
}

impl TouchContact {
    pub const BASE_ADDRESS: u16 = 0x4102;
    pub const CONTACT_SIZE: usize = 8;

    /// Computes the register address for the given contact index.
    pub const fn register_address(index: usize) -> u16 {
        Self::BASE_ADDRESS + (index * Self::CONTACT_SIZE) as u16
    }

    /// Returns the contact tracking identifier.
    pub fn id(&self) -> u8 {
        self.track_id_and_status.id()
    }

    /// Returns the X coordinate.
    pub const fn x(&self) -> u16 {
        u16::from_le_bytes([self.x7_0, self.x15_8])
    }

    /// Returns the Y coordinate.
    pub const fn y(&self) -> u16 {
        u16::from_le_bytes([self.y7_0, self.y15_8])
    }

    /// Returns the contact pressure or area value.
    pub const fn pressure(&self) -> u8 {
        self.pressure
    }
}

impl From<&TouchContact> for fidl_input_report::ContactInputReport {
    fn from(contact: &TouchContact) -> Self {
        fidl_input_report::ContactInputReport {
            contact_id: Some(contact.id() as u32),
            position_x: Some(contact.x() as i64),
            position_y: Some(contact.y() as i64),
            ..Default::default()
        }
    }
}

impl From<TouchContact> for fidl_input_report::ContactInputReport {
    fn from(contact: TouchContact) -> Self {
        Self::from(&contact)
    }
}

/// Initial burst read structure combining status, count, and the first contact point.
//
// @cite(gt6853-hardware-description): sec=3.4 title="Coordinate Information Buffer"
// @cite(gt6853-programming-guide): sec=3.4 title="Coordinate Information"
// @cite(gt6853-programming-guide): sec=5 title="Coordinate Reading"
// @alias(gt6853-hardware-description): theirs="Touch_Status"
// @alias(gt6853-programming-guide): theirs="Event Status"
#[derive(Copy, Clone, Debug, PartialEq, Eq, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct InitialTouchReport {
    pub status: EventStatus,
    pub contact_count: ContactCount,
    pub first_contact: TouchContact,
}

impl AddressableRegister for InitialTouchReport {
    const ADDRESS: u16 = 0x4100;
}

impl ReadableRegister for InitialTouchReport {}

/// Generic structure for reading the remaining `N` touch contact points (contacts 1..=N).
//
// @cite(gt6853-hardware-description): sec=3.4 title="Coordinate Information Buffer"
// @cite(gt6853-programming-guide): sec=3.4 title="Coordinate Information"
// @cite(gt6853-programming-guide): sec=5 title="Coordinate Reading"
// @alias(gt6853-hardware-description): theirs="Contact Blocks"
// @alias(gt6853-programming-guide): theirs="Touch Point Array"
#[derive(Copy, Clone, Debug, PartialEq, Eq, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct RemainingTouchContacts<const N: usize> {
    pub contacts: [TouchContact; N],
}

impl<const N: usize> AddressableRegister for RemainingTouchContacts<N> {
    /// Base address for contact index 1 (`0x410A`).
    const ADDRESS: u16 = 0x410A;
}

impl<const N: usize> ReadableRegister for RemainingTouchContacts<N> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_touch_contact_size() {
        assert_eq!(core::mem::size_of::<TouchContact>(), 8);
    }

    #[test]
    fn test_initial_touch_report_size() {
        assert_eq!(core::mem::size_of::<InitialTouchReport>(), 10);
    }

    #[test]
    fn test_remaining_touch_contacts_size() {
        assert_eq!(core::mem::size_of::<RemainingTouchContacts<1>>(), 8);
        assert_eq!(core::mem::size_of::<RemainingTouchContacts<9>>(), 72);
    }

    #[test]
    fn test_touch_contact_address_calculation() {
        assert_eq!(TouchContact::register_address(0), 0x4102);
        assert_eq!(TouchContact::register_address(1), 0x410A);
        assert_eq!(TouchContact::register_address(9), 0x414A);
    }

    #[test]
    fn test_touch_contact_coordinate_conversion() {
        // Test with asymmetric bytes where both high and low bytes are non-zero.
        let contact = TouchContact {
            track_id_and_status: TrackIdAndStatus(0x03),
            x7_0: 0x51,
            x15_8: 0x03, // 0x0351 = 849
            y7_0: 0xE8,
            y15_8: 0x04, // 0x04E8 = 1256
            pressure: 0x54,
            reserved: [0; 2],
        };

        assert_eq!(contact.id(), 3);
        assert_eq!(contact.x(), 849);
        assert_eq!(contact.y(), 1256);
        assert_eq!(contact.pressure(), 0x54);
    }

    #[test]
    fn test_initial_touch_report_parsing() {
        let report = InitialTouchReport {
            status: EventStatus(0x80),
            contact_count: ContactCount(1),
            first_contact: TouchContact {
                track_id_and_status: TrackIdAndStatus(0x00),
                x7_0: 0x20,
                x15_8: 0x03, // 0x0320 = 800
                y7_0: 0xB0,
                y15_8: 0x04, // 0x04B0 = 1200
                pressure: 80,
                reserved: [0; 2],
            },
        };

        assert!(report.status.coordinates_ready());
        assert_eq!(report.contact_count.count(), 1);
        assert_eq!(report.first_contact.x(), 800);
        assert_eq!(report.first_contact.y(), 1200);
    }

    #[test]
    fn test_touch_contact_into_fidl() {
        let contact = TouchContact {
            track_id_and_status: TrackIdAndStatus(0x01),
            x7_0: 0x34,
            x15_8: 0x12, // 0x1234
            y7_0: 0x78,
            y15_8: 0x56, // 0x5678
            pressure: 0x9a,
            reserved: [0xbc, 0xde],
        };

        let report = fidl_input_report::ContactInputReport::from(&contact);
        assert_eq!(report.contact_id, Some(1));
        assert_eq!(report.position_x, Some(0x1234));
        assert_eq!(report.position_y, Some(0x5678));
        assert_eq!(report.pressure, None);
        assert_eq!(report.contact_width, None);
        assert_eq!(report.contact_height, None);

        // Also test value Into trait
        let report_into: fidl_input_report::ContactInputReport = contact.into();
        assert_eq!(report, report_into);
    }

    #[test]
    fn test_touch_contact_into_fidl_origin() {
        let origin = TouchContact {
            track_id_and_status: TrackIdAndStatus(0x00),
            x7_0: 0x00,
            x15_8: 0x00,
            y7_0: 0x00,
            y15_8: 0x00,
            pressure: 0,
            reserved: [0; 2],
        };

        let origin_report = fidl_input_report::ContactInputReport::from(&origin);
        assert_eq!(origin_report.contact_id, Some(0));
        assert_eq!(origin_report.position_x, Some(0));
        assert_eq!(origin_report.position_y, Some(0));

        // Also test value Into trait
        let origin_into: fidl_input_report::ContactInputReport = origin.into();
        assert_eq!(origin_report, origin_into);
    }
}
