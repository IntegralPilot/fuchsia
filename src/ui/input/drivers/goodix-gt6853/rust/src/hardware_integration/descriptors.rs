// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Input report device descriptors for Goodix GT6853 touch controller.

use crate::data_types::hardware_limits::{
    MAX_CONTACTS, NELSON_MAX_CONTACT_X, NELSON_MAX_CONTACT_Y,
};
use fidl_next_fuchsia_input_report as fidl_input_report;

/// Constructs the FIDL [`fidl_input_report::DeviceDescriptor`] for the Goodix GT6853
/// touch controller on Nelson devices.
pub fn make_device_descriptor() -> fidl_input_report::DeviceDescriptor {
    let axis_x = fidl_input_report::Axis {
        range: fidl_input_report::Range { min: 0, max: NELSON_MAX_CONTACT_X },
        unit: fidl_input_report::Unit { type_: fidl_input_report::UnitType::None, exponent: 0 },
    };

    let axis_y = fidl_input_report::Axis {
        range: fidl_input_report::Range { min: 0, max: NELSON_MAX_CONTACT_Y },
        unit: fidl_input_report::Unit { type_: fidl_input_report::UnitType::None, exponent: 0 },
    };

    let contacts = (0..MAX_CONTACTS)
        .map(|_| fidl_input_report::ContactInputDescriptor {
            position_x: Some(axis_x),
            position_y: Some(axis_y),
            // Hardware pressure readings are uncalibrated and unused by the input pipeline.
            ..Default::default()
        })
        .collect();

    let touch_input_descriptor = fidl_input_report::TouchInputDescriptor {
        contacts: Some(contacts),
        max_contacts: Some(MAX_CONTACTS as u32),
        touch_type: Some(fidl_input_report::TouchType::Touchscreen),
        ..Default::default()
    };

    let touch_descriptor = fidl_input_report::TouchDescriptor {
        input: Some(touch_input_descriptor),
        ..Default::default()
    };

    let device_info = fidl_input_report::DeviceInformation {
        vendor_id: Some(u32::from(fidl_input_report::VendorId::Google)),
        product_id: Some(u32::from(fidl_input_report::VendorGoogleProductId::GoodixTouchscreen)),
        manufacturer_name: Some("Goodix".to_string()),
        product_name: Some("GT6853 Touchscreen".to_string()),
        ..Default::default()
    };

    fidl_input_report::DeviceDescriptor {
        touch: Some(touch_descriptor),
        device_information: Some(device_info),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_device_descriptor_device_info() {
        let descriptor = make_device_descriptor();
        let device_info = descriptor.device_information.expect("missing device_information");

        assert_eq!(device_info.vendor_id, Some(u32::from(fidl_input_report::VendorId::Google)));
        assert_eq!(
            device_info.product_id,
            Some(u32::from(fidl_input_report::VendorGoogleProductId::GoodixTouchscreen))
        );
        assert_eq!(device_info.manufacturer_name.as_deref(), Some("Goodix"));
        assert_eq!(device_info.product_name.as_deref(), Some("GT6853 Touchscreen"));
    }

    #[test]
    fn test_make_device_descriptor_touch_capabilities() {
        let descriptor = make_device_descriptor();
        let touch_desc = descriptor.touch.expect("missing touch descriptor");
        let input_desc = touch_desc.input.expect("missing touch input descriptor");

        assert_eq!(input_desc.touch_type, Some(fidl_input_report::TouchType::Touchscreen));
        assert_eq!(input_desc.max_contacts, Some(MAX_CONTACTS as u32));
        assert_eq!(input_desc.buttons, None);

        let contacts = input_desc.contacts.expect("missing contacts descriptor");
        assert_eq!(contacts.len(), MAX_CONTACTS);
    }

    #[test]
    fn test_make_device_descriptor_axes_ranges() {
        let descriptor = make_device_descriptor();
        let touch_desc = descriptor.touch.expect("missing touch descriptor");
        let input_desc = touch_desc.input.expect("missing touch input descriptor");
        let contacts = input_desc.contacts.expect("missing contacts descriptor");

        for (i, contact) in contacts.iter().enumerate() {
            let pos_x = contact.position_x.expect("missing position_x");
            assert_eq!(pos_x.range.min, 0, "contact {i} position_x min mismatch");
            assert_eq!(
                pos_x.range.max, NELSON_MAX_CONTACT_X,
                "contact {i} position_x max mismatch"
            );
            assert_eq!(pos_x.unit.type_, fidl_input_report::UnitType::None);
            assert_eq!(pos_x.unit.exponent, 0);

            let pos_y = contact.position_y.expect("missing position_y");
            assert_eq!(pos_y.range.min, 0, "contact {i} position_y min mismatch");
            assert_eq!(
                pos_y.range.max, NELSON_MAX_CONTACT_Y,
                "contact {i} position_y max mismatch"
            );
            assert_eq!(pos_y.unit.type_, fidl_input_report::UnitType::None);
            assert_eq!(pos_y.unit.exponent, 0);

            assert_eq!(contact.pressure, None);
            assert_eq!(contact.contact_width, None);
            assert_eq!(contact.contact_height, None);
        }
    }
}
