// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module defines mirror types for the FIDL fuchsia.input.report descriptor types.
//! We define these local types because the FIDL-generated types do not currently
//! derive `serde::Serialize`, which we need for outputting device descriptors as JSON.
//! These mirror types also simplify serialization by flattening options and vectors.

// TODO(https://fxbug.dev/530323237): Remove this once all types are used.
#![allow(dead_code)]

use crate::common;
use fidl_next_fuchsia_input_report as fidl_input_report;
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct Axis {
    pub range: Range,
    pub unit: Unit,
}

impl From<fidl_input_report::Axis> for Axis {
    fn from(fidl: fidl_input_report::Axis) -> Self {
        Self { range: fidl.range.into(), unit: fidl.unit.into() }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Range {
    pub min: i64,
    pub max: i64,
}

impl From<fidl_input_report::Range> for Range {
    fn from(fidl: fidl_input_report::Range) -> Self {
        Self { min: fidl.min, max: fidl.max }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Unit {
    pub type_: String,
    pub exponent: i32,
}

impl From<fidl_input_report::Unit> for Unit {
    fn from(fidl: fidl_input_report::Unit) -> Self {
        Self { type_: format!("{:?}", fidl.type_), exponent: fidl.exponent }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SensorAxis {
    pub axis: Axis,
    pub type_: String,
}

impl From<fidl_input_report::SensorAxis> for SensorAxis {
    fn from(fidl: fidl_input_report::SensorAxis) -> Self {
        Self { axis: fidl.axis.into(), type_: format!("{:?}", fidl.type_) }
    }
}

fn to_hex_option<S>(val: &Option<u32>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match val {
        Some(v) => serializer.serialize_str(&format!("0x{:04x}", v)),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct DeviceInformation {
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "to_hex_option")]
    pub vendor_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "to_hex_option")]
    pub product_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polling_rate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
}

impl From<fidl_input_report::DeviceInformation> for DeviceInformation {
    fn from(fidl: fidl_input_report::DeviceInformation) -> Self {
        Self {
            vendor_id: fidl.vendor_id,
            product_id: fidl.product_id,
            version: fidl.version,
            polling_rate: fidl.polling_rate,
            manufacturer_name: fidl.manufacturer_name,
            product_name: fidl.product_name,
            serial_number: fidl.serial_number,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct DeviceDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_information: Option<DeviceInformation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse: Option<MouseDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SensorDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch: Option<TouchDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<KeyboardDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer_control: Option<ConsumerControlDescriptor>,
}

impl From<fidl_input_report::DeviceDescriptor> for DeviceDescriptor {
    fn from(fidl: fidl_input_report::DeviceDescriptor) -> Self {
        Self {
            mouse: fidl.mouse.map(Into::into),
            sensor: fidl.sensor.map(Into::into),
            touch: fidl.touch.map(Into::into),
            keyboard: fidl.keyboard.map(Into::into),
            consumer_control: fidl.consumer_control.map(Into::into),
            device_information: fidl.device_information.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct OutputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<KeyboardOutputDescriptor>,
}

impl From<fidl_input_report::OutputDescriptor> for OutputDescriptor {
    fn from(fidl: fidl_input_report::OutputDescriptor) -> Self {
        Self { keyboard: fidl.keyboard.map(Into::into) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct MouseInputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movement_x: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movement_y: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_x: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_y: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_v: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_h: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<u8>>,
}

impl From<fidl_input_report::MouseInputDescriptor> for MouseInputDescriptor {
    fn from(fidl: fidl_input_report::MouseInputDescriptor) -> Self {
        Self {
            movement_x: fidl.movement_x.map(Into::into),
            movement_y: fidl.movement_y.map(Into::into),
            position_x: fidl.position_x.map(Into::into),
            position_y: fidl.position_y.map(Into::into),
            scroll_v: fidl.scroll_v.map(Into::into),
            scroll_h: fidl.scroll_h.map(Into::into),
            buttons: fidl.buttons,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct MouseDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<MouseInputDescriptor>,
}

impl From<fidl_input_report::MouseDescriptor> for MouseDescriptor {
    fn from(fidl: fidl_input_report::MouseDescriptor) -> Self {
        Self { input: fidl.input.map(Into::into) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SensorInputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<SensorAxis>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_id: Option<u8>,
}

impl From<fidl_input_report::SensorInputDescriptor> for SensorInputDescriptor {
    fn from(fidl: fidl_input_report::SensorInputDescriptor) -> Self {
        Self { values: fidl.values.map(common::convert_vec), report_id: fidl.report_id }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SensorFeatureDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_interval: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_reporting_state: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<Vec<SensorAxis>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_high: Option<Vec<SensorAxis>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_low: Option<Vec<SensorAxis>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_rate: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_id: Option<u8>,
}

impl From<fidl_input_report::SensorFeatureDescriptor> for SensorFeatureDescriptor {
    fn from(fidl: fidl_input_report::SensorFeatureDescriptor) -> Self {
        Self {
            report_interval: fidl.report_interval.map(Into::into),
            supports_reporting_state: fidl.supports_reporting_state,
            sensitivity: fidl.sensitivity.map(common::convert_vec),
            threshold_high: fidl.threshold_high.map(common::convert_vec),
            threshold_low: fidl.threshold_low.map(common::convert_vec),
            sampling_rate: fidl.sampling_rate.map(Into::into),
            report_id: fidl.report_id,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SensorDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<SensorInputDescriptor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<Vec<SensorFeatureDescriptor>>,
}

impl From<fidl_input_report::SensorDescriptor> for SensorDescriptor {
    fn from(fidl: fidl_input_report::SensorDescriptor) -> Self {
        Self {
            input: fidl.input.map(common::convert_vec),
            feature: fidl.feature.map(common::convert_vec),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct ContactInputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_x: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_y: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_width: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_height: Option<Axis>,
}

impl From<fidl_input_report::ContactInputDescriptor> for ContactInputDescriptor {
    fn from(fidl: fidl_input_report::ContactInputDescriptor) -> Self {
        Self {
            position_x: fidl.position_x.map(Into::into),
            position_y: fidl.position_y.map(Into::into),
            pressure: fidl.pressure.map(Into::into),
            contact_width: fidl.contact_width.map(Into::into),
            contact_height: fidl.contact_height.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct TouchInputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contacts: Option<Vec<ContactInputDescriptor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_contacts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch_type: Option<String>,
}

impl From<fidl_input_report::TouchInputDescriptor> for TouchInputDescriptor {
    fn from(fidl: fidl_input_report::TouchInputDescriptor) -> Self {
        Self {
            contacts: fidl.contacts.map(common::convert_vec),
            max_contacts: fidl.max_contacts,
            touch_type: fidl.touch_type.map(|x| format!("{:?}", x)),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct TouchFeatureDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_input_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_selective_reporting: Option<bool>,
}

impl From<fidl_input_report::TouchFeatureDescriptor> for TouchFeatureDescriptor {
    fn from(fidl: fidl_input_report::TouchFeatureDescriptor) -> Self {
        Self {
            supports_input_mode: fidl.supports_input_mode,
            supports_selective_reporting: fidl.supports_selective_reporting,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct TouchDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<TouchInputDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<TouchFeatureDescriptor>,
}

impl From<fidl_input_report::TouchDescriptor> for TouchDescriptor {
    fn from(fidl: fidl_input_report::TouchDescriptor) -> Self {
        Self { input: fidl.input.map(Into::into), feature: fidl.feature.map(Into::into) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct KeyboardInputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys3: Option<Vec<String>>,
}

impl From<fidl_input_report::KeyboardInputDescriptor> for KeyboardInputDescriptor {
    fn from(fidl: fidl_input_report::KeyboardInputDescriptor) -> Self {
        Self { keys3: fidl.keys3.map(common::debug_vec) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct KeyboardOutputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leds: Option<Vec<String>>,
}

impl From<fidl_input_report::KeyboardOutputDescriptor> for KeyboardOutputDescriptor {
    fn from(fidl: fidl_input_report::KeyboardOutputDescriptor) -> Self {
        Self { leds: fidl.leds.map(common::debug_vec) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct KeyboardDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<KeyboardInputDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<KeyboardOutputDescriptor>,
}

impl From<fidl_input_report::KeyboardDescriptor> for KeyboardDescriptor {
    fn from(fidl: fidl_input_report::KeyboardDescriptor) -> Self {
        Self { input: fidl.input.map(Into::into), output: fidl.output.map(Into::into) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct ConsumerControlInputDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<String>>,
}

impl From<fidl_input_report::ConsumerControlInputDescriptor> for ConsumerControlInputDescriptor {
    fn from(fidl: fidl_input_report::ConsumerControlInputDescriptor) -> Self {
        Self { buttons: fidl.buttons.map(common::debug_vec) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct ConsumerControlDescriptor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<ConsumerControlInputDescriptor>,
}

impl From<fidl_input_report::ConsumerControlDescriptor> for ConsumerControlDescriptor {
    fn from(fidl: fidl_input_report::ConsumerControlDescriptor) -> Self {
        Self { input: fidl.input.map(Into::into) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_next_fuchsia_input_report as fidl_input_report;

    fn create_unitless_axis(min: i64, max: i64) -> fidl_input_report::Axis {
        fidl_input_report::Axis {
            range: fidl_input_report::Range { min, max },
            unit: fidl_input_report::Unit { type_: fidl_input_report::UnitType::None, exponent: 0 },
        }
    }

    fn create_sensor_axis(
        min: i64,
        max: i64,
        sensor_type: fidl_input_report::SensorType,
    ) -> fidl_input_report::SensorAxis {
        fidl_input_report::SensorAxis { axis: create_unitless_axis(min, max), type_: sensor_type }
    }

    #[test]
    fn test_axis_range_unit_from() {
        let axis = Axis::from(create_unitless_axis(-10, 90));
        assert_eq!(axis.range.min, -10);
        assert_eq!(axis.range.max, 90);
        assert_eq!(axis.unit.type_, "None");
        assert_eq!(axis.unit.exponent, 0);
    }

    #[test]
    fn test_sensor_axis_from() {
        let sensor_axis = SensorAxis::from(create_sensor_axis(
            -10,
            90,
            fidl_input_report::SensorType::LightIlluminance,
        ));
        assert_eq!(sensor_axis.axis.range.min, -10);
        assert_eq!(sensor_axis.axis.range.max, 90);
        assert_eq!(sensor_axis.axis.unit.type_, "None");
        assert_eq!(sensor_axis.type_, "LightIlluminance");
    }

    #[test]
    fn test_device_information_from() {
        let fidl_info = fidl_input_report::DeviceInformation {
            vendor_id: Some(0x1234),
            product_id: Some(0x5678),
            version: Some(42),
            polling_rate: Some(1000),
            manufacturer_name: Some("Google".to_string()),
            product_name: Some("Pixel".to_string()),
            serial_number: Some("123456".to_string()),
        };
        let info = DeviceInformation::from(fidl_info);
        assert_eq!(info.vendor_id, Some(0x1234));
        assert_eq!(info.product_id, Some(0x5678));
        assert_eq!(info.version, Some(42));
        assert_eq!(info.polling_rate, Some(1000));
        assert_eq!(info.manufacturer_name.as_deref(), Some("Google"));
        assert_eq!(info.product_name.as_deref(), Some("Pixel"));
        assert_eq!(info.serial_number.as_deref(), Some("123456"));
    }

    #[test]
    fn test_mouse_descriptor_from() {
        let fidl_mouse = fidl_input_report::MouseDescriptor {
            input: Some(fidl_input_report::MouseInputDescriptor {
                movement_x: Some(create_unitless_axis(1, 100)),
                movement_y: Some(create_unitless_axis(2, 200)),
                position_x: Some(create_unitless_axis(3, 300)),
                position_y: Some(create_unitless_axis(4, 400)),
                scroll_v: Some(create_unitless_axis(5, 500)),
                scroll_h: Some(create_unitless_axis(6, 600)),
                buttons: Some(vec![1, 2]),
            }),
        };
        let mouse = MouseDescriptor::from(fidl_mouse);
        let input = mouse.input.unwrap();
        assert_eq!(input.movement_x.as_ref().unwrap().range.min, 1);
        assert_eq!(input.movement_x.as_ref().unwrap().range.max, 100);
        assert_eq!(input.movement_y.as_ref().unwrap().range.min, 2);
        assert_eq!(input.movement_y.as_ref().unwrap().range.max, 200);
        assert_eq!(input.position_x.as_ref().unwrap().range.min, 3);
        assert_eq!(input.position_x.as_ref().unwrap().range.max, 300);
        assert_eq!(input.position_y.as_ref().unwrap().range.min, 4);
        assert_eq!(input.position_y.as_ref().unwrap().range.max, 400);
        assert_eq!(input.scroll_v.as_ref().unwrap().range.min, 5);
        assert_eq!(input.scroll_v.as_ref().unwrap().range.max, 500);
        assert_eq!(input.scroll_h.as_ref().unwrap().range.min, 6);
        assert_eq!(input.scroll_h.as_ref().unwrap().range.max, 600);
        assert_eq!(input.buttons.unwrap(), vec![1, 2]);
    }

    #[test]
    fn test_sensor_descriptor_from() {
        let fidl_sensor = fidl_input_report::SensorDescriptor {
            input: Some(vec![fidl_input_report::SensorInputDescriptor {
                values: Some(vec![create_sensor_axis(
                    10,
                    100,
                    fidl_input_report::SensorType::LightIlluminance,
                )]),
                report_id: Some(1),
            }]),
            feature: Some(vec![fidl_input_report::SensorFeatureDescriptor {
                report_interval: Some(create_unitless_axis(20, 200)),
                supports_reporting_state: Some(true),
                sensitivity: Some(vec![create_sensor_axis(
                    21,
                    210,
                    fidl_input_report::SensorType::LightRed,
                )]),
                threshold_high: Some(vec![create_sensor_axis(
                    22,
                    220,
                    fidl_input_report::SensorType::LightGreen,
                )]),
                threshold_low: Some(vec![create_sensor_axis(
                    23,
                    230,
                    fidl_input_report::SensorType::LightBlue,
                )]),
                sampling_rate: Some(create_unitless_axis(24, 240)),
                report_id: Some(2),
            }]),
        };
        let sensor = SensorDescriptor::from(fidl_sensor);

        let input = &sensor.input.as_ref().unwrap()[0];
        assert_eq!(input.report_id, Some(1));
        assert_eq!(input.values.as_ref().unwrap()[0].axis.range.min, 10);
        assert_eq!(input.values.as_ref().unwrap()[0].axis.range.max, 100);
        assert_eq!(input.values.as_ref().unwrap()[0].type_, "LightIlluminance");

        let feature = &sensor.feature.as_ref().unwrap()[0];
        assert_eq!(feature.report_id, Some(2));
        assert_eq!(feature.report_interval.as_ref().unwrap().range.min, 20);
        assert_eq!(feature.report_interval.as_ref().unwrap().range.max, 200);
        assert_eq!(feature.supports_reporting_state, Some(true));
        assert_eq!(feature.sensitivity.as_ref().unwrap()[0].axis.range.min, 21);
        assert_eq!(feature.sensitivity.as_ref().unwrap()[0].axis.range.max, 210);
        assert_eq!(feature.sensitivity.as_ref().unwrap()[0].type_, "LightRed");
        assert_eq!(feature.threshold_high.as_ref().unwrap()[0].axis.range.min, 22);
        assert_eq!(feature.threshold_high.as_ref().unwrap()[0].axis.range.max, 220);
        assert_eq!(feature.threshold_high.as_ref().unwrap()[0].type_, "LightGreen");
        assert_eq!(feature.threshold_low.as_ref().unwrap()[0].axis.range.min, 23);
        assert_eq!(feature.threshold_low.as_ref().unwrap()[0].axis.range.max, 230);
        assert_eq!(feature.threshold_low.as_ref().unwrap()[0].type_, "LightBlue");
        assert_eq!(feature.sampling_rate.as_ref().unwrap().range.min, 24);
        assert_eq!(feature.sampling_rate.as_ref().unwrap().range.max, 240);
    }

    #[test]
    fn test_touch_descriptor_from() {
        let fidl_touch = fidl_input_report::TouchDescriptor {
            input: Some(fidl_input_report::TouchInputDescriptor {
                contacts: Some(vec![fidl_input_report::ContactInputDescriptor {
                    position_x: Some(create_unitless_axis(10, 100)),
                    position_y: Some(create_unitless_axis(20, 200)),
                    pressure: Some(create_unitless_axis(30, 300)),
                    contact_width: Some(create_unitless_axis(40, 400)),
                    contact_height: Some(create_unitless_axis(50, 500)),
                }]),
                max_contacts: Some(5),
                touch_type: Some(fidl_input_report::TouchType::Touchscreen),
                buttons: None,
            }),
            feature: Some(fidl_input_report::TouchFeatureDescriptor {
                supports_input_mode: Some(true),
                supports_selective_reporting: Some(false),
            }),
        };
        let touch = TouchDescriptor::from(fidl_touch);
        let input = touch.input.unwrap();
        let contact = &input.contacts.unwrap()[0];
        assert_eq!(contact.position_x.as_ref().unwrap().range.min, 10);
        assert_eq!(contact.position_x.as_ref().unwrap().range.max, 100);
        assert_eq!(contact.position_y.as_ref().unwrap().range.min, 20);
        assert_eq!(contact.position_y.as_ref().unwrap().range.max, 200);
        assert_eq!(contact.pressure.as_ref().unwrap().range.min, 30);
        assert_eq!(contact.pressure.as_ref().unwrap().range.max, 300);
        assert_eq!(contact.contact_width.as_ref().unwrap().range.min, 40);
        assert_eq!(contact.contact_width.as_ref().unwrap().range.max, 400);
        assert_eq!(contact.contact_height.as_ref().unwrap().range.min, 50);
        assert_eq!(contact.contact_height.as_ref().unwrap().range.max, 500);
        assert_eq!(input.max_contacts, Some(5));
        assert_eq!(input.touch_type.unwrap(), "Touchscreen");
        assert_eq!(touch.feature.unwrap().supports_input_mode, Some(true));
    }

    #[test]
    fn test_keyboard_descriptor_from() {
        let fidl_keyboard = fidl_input_report::KeyboardDescriptor {
            input: Some(fidl_input_report::KeyboardInputDescriptor {
                keys3: Some(vec![fidl_next_fuchsia_input::Key::A]),
            }),
            output: Some(fidl_input_report::KeyboardOutputDescriptor {
                leds: Some(vec![fidl_input_report::LedType::NumLock]),
            }),
        };
        let keyboard = KeyboardDescriptor::from(fidl_keyboard);
        assert_eq!(keyboard.input.unwrap().keys3.unwrap(), vec!["A".to_string()]);
        assert_eq!(keyboard.output.unwrap().leds.unwrap(), vec!["NumLock".to_string()]);
    }

    #[test]
    fn test_consumer_control_descriptor_from() {
        let fidl_cc = fidl_input_report::ConsumerControlDescriptor {
            input: Some(fidl_input_report::ConsumerControlInputDescriptor {
                buttons: Some(vec![fidl_input_report::ConsumerControlButton::VolumeUp]),
            }),
        };
        let cc = ConsumerControlDescriptor::from(fidl_cc);
        assert_eq!(cc.input.unwrap().buttons.unwrap(), vec!["VolumeUp".to_string()]);
    }

    #[test]
    fn test_device_descriptor_from() {
        let fidl_desc = fidl_input_report::DeviceDescriptor {
            device_information: Some(fidl_input_report::DeviceInformation {
                vendor_id: Some(0x1234),
                product_id: Some(0x5678),
                version: Some(42),
                polling_rate: Some(1000),
                manufacturer_name: Some("Google".to_string()),
                product_name: Some("Pixel".to_string()),
                serial_number: Some("123456".to_string()),
            }),
            mouse: Some(fidl_input_report::MouseDescriptor {
                input: Some(fidl_input_report::MouseInputDescriptor {
                    movement_x: Some(create_unitless_axis(1, 10)),
                    movement_y: None,
                    position_x: None,
                    position_y: None,
                    scroll_v: None,
                    scroll_h: None,
                    buttons: None,
                }),
            }),
            sensor: Some(fidl_input_report::SensorDescriptor {
                input: Some(vec![fidl_input_report::SensorInputDescriptor {
                    values: Some(vec![create_sensor_axis(
                        2,
                        20,
                        fidl_input_report::SensorType::LightIlluminance,
                    )]),
                    report_id: Some(1),
                }]),
                feature: None,
            }),
            touch: Some(fidl_input_report::TouchDescriptor {
                input: Some(fidl_input_report::TouchInputDescriptor {
                    contacts: Some(vec![]),
                    max_contacts: Some(5),
                    touch_type: Some(fidl_input_report::TouchType::Touchscreen),
                    buttons: None,
                }),
                feature: None,
            }),
            keyboard: Some(fidl_input_report::KeyboardDescriptor {
                input: Some(fidl_input_report::KeyboardInputDescriptor {
                    keys3: Some(vec![fidl_next_fuchsia_input::Key::A]),
                }),
                output: None,
            }),
            consumer_control: Some(fidl_input_report::ConsumerControlDescriptor {
                input: Some(fidl_input_report::ConsumerControlInputDescriptor {
                    buttons: Some(vec![fidl_input_report::ConsumerControlButton::VolumeUp]),
                }),
            }),
        };
        let desc = DeviceDescriptor::from(fidl_desc);
        assert_eq!(desc.device_information.unwrap().vendor_id, Some(0x1234));
        assert!(desc.mouse.is_some());
        let mouse_input = desc.mouse.as_ref().unwrap().input.as_ref().unwrap();
        assert_eq!(mouse_input.movement_x.as_ref().unwrap().range.min, 1);
        assert_eq!(mouse_input.movement_x.as_ref().unwrap().range.max, 10);
        assert!(desc.sensor.is_some());
        let sensor_input = &desc.sensor.as_ref().unwrap().input.as_ref().unwrap()[0];
        assert_eq!(sensor_input.values.as_ref().unwrap()[0].axis.range.min, 2);
        assert_eq!(sensor_input.values.as_ref().unwrap()[0].axis.range.max, 20);
        assert!(desc.touch.is_some());
        assert!(desc.keyboard.is_some());
        assert!(desc.consumer_control.is_some());
    }

    #[test]
    fn test_output_descriptor_from() {
        let fidl_desc = fidl_input_report::OutputDescriptor {
            keyboard: Some(fidl_input_report::KeyboardOutputDescriptor {
                leds: Some(vec![fidl_input_report::LedType::CapsLock]),
            }),
        };
        let desc = OutputDescriptor::from(fidl_desc);
        assert_eq!(desc.keyboard.unwrap().leds.unwrap(), vec!["CapsLock".to_string()]);
    }
}
