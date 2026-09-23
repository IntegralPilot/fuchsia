// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module defines mirror types for the FIDL fuchsia.input.report report types.
//! We define these local types because the FIDL-generated types do not currently
//! derive `serde::Serialize`, which we need for outputting device reports as JSON or text.

// TODO(https://fxbug.dev/530323237): Remove this once all types are used.
#![allow(dead_code)]

use crate::common;
use fidl_next_fuchsia_input_report as fidl_input_report;
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct MouseInputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movement_x: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movement_y: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_x: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_y: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_v: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_h: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressed_buttons: Option<Vec<u8>>,
}

impl From<fidl_input_report::MouseInputReport> for MouseInputReport {
    fn from(fidl: fidl_input_report::MouseInputReport) -> Self {
        Self {
            movement_x: fidl.movement_x,
            movement_y: fidl.movement_y,
            position_x: fidl.position_x,
            position_y: fidl.position_y,
            scroll_v: fidl.scroll_v,
            scroll_h: fidl.scroll_h,
            pressed_buttons: fidl.pressed_buttons,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SensorInputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<i64>>,
}

impl From<fidl_input_report::SensorInputReport> for SensorInputReport {
    fn from(fidl: fidl_input_report::SensorInputReport) -> Self {
        Self { values: fidl.values }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SensorFeatureReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_interval: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reporting_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_high: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_low: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_rate: Option<i64>,
}

impl From<fidl_input_report::SensorFeatureReport> for SensorFeatureReport {
    fn from(fidl: fidl_input_report::SensorFeatureReport) -> Self {
        Self {
            report_interval: fidl.report_interval,
            reporting_state: fidl.reporting_state.map(|x| format!("{:?}", x)),
            sensitivity: fidl.sensitivity,
            threshold_high: fidl.threshold_high,
            threshold_low: fidl.threshold_low,
            sampling_rate: fidl.sampling_rate,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct ContactInputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_x: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_y: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<bool>,
}

impl From<fidl_input_report::ContactInputReport> for ContactInputReport {
    fn from(fidl: fidl_input_report::ContactInputReport) -> Self {
        Self {
            contact_id: fidl.contact_id,
            position_x: fidl.position_x,
            position_y: fidl.position_y,
            pressure: fidl.pressure,
            contact_width: fidl.contact_width,
            contact_height: fidl.contact_height,
            confidence: fidl.confidence,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct TouchInputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contacts: Option<Vec<ContactInputReport>>,
}

impl From<fidl_input_report::TouchInputReport> for TouchInputReport {
    fn from(fidl: fidl_input_report::TouchInputReport) -> Self {
        Self { contacts: fidl.contacts.map(common::convert_vec) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SelectiveReportingFeatureReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_switch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button_switch: Option<bool>,
}

impl From<fidl_input_report::SelectiveReportingFeatureReport> for SelectiveReportingFeatureReport {
    fn from(fidl: fidl_input_report::SelectiveReportingFeatureReport) -> Self {
        Self { surface_switch: fidl.surface_switch, button_switch: fidl.button_switch }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct TouchFeatureReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selective_reporting: Option<SelectiveReportingFeatureReport>,
}

impl From<fidl_input_report::TouchFeatureReport> for TouchFeatureReport {
    fn from(fidl: fidl_input_report::TouchFeatureReport) -> Self {
        Self {
            input_mode: fidl.input_mode.map(|x| format!("{:?}", x)),
            selective_reporting: fidl.selective_reporting.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct KeyboardInputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressed_keys3: Option<Vec<String>>,
}

impl From<fidl_input_report::KeyboardInputReport> for KeyboardInputReport {
    fn from(fidl: fidl_input_report::KeyboardInputReport) -> Self {
        Self { pressed_keys3: fidl.pressed_keys3.map(common::debug_vec) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct KeyboardOutputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_leds: Option<Vec<String>>,
}

impl From<fidl_input_report::KeyboardOutputReport> for KeyboardOutputReport {
    fn from(fidl: fidl_input_report::KeyboardOutputReport) -> Self {
        Self { enabled_leds: fidl.enabled_leds.map(common::debug_vec) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct ConsumerControlInputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressed_buttons: Option<Vec<String>>,
}

impl From<fidl_input_report::ConsumerControlInputReport> for ConsumerControlInputReport {
    fn from(fidl: fidl_input_report::ConsumerControlInputReport) -> Self {
        Self { pressed_buttons: fidl.pressed_buttons.map(common::debug_vec) }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct InputReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_id: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse: Option<MouseInputReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SensorInputReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch: Option<TouchInputReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<KeyboardInputReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer_control: Option<ConsumerControlInputReport>,
}

impl From<fidl_input_report::InputReport> for InputReport {
    fn from(fidl: fidl_input_report::InputReport) -> Self {
        Self {
            event_time: fidl.event_time,
            report_id: fidl.report_id,
            mouse: fidl.mouse.map(Into::into),
            sensor: fidl.sensor.map(Into::into),
            touch: fidl.touch.map(Into::into),
            keyboard: fidl.keyboard.map(Into::into),
            consumer_control: fidl.consumer_control.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct InstanceReport {
    pub instance: String,
    pub report: InputReport,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_next_fuchsia_input_report as fidl_input_report;

    #[test]
    fn test_reports_from() {
        // MouseInputReport
        let fidl_mouse_report = fidl_input_report::MouseInputReport {
            movement_x: Some(10),
            movement_y: Some(-10),
            position_x: Some(100),
            position_y: Some(200),
            scroll_v: Some(1),
            scroll_h: Some(-1),
            pressed_buttons: Some(vec![1]),
        };
        let mouse_report = MouseInputReport::from(fidl_mouse_report);
        assert_eq!(mouse_report.movement_x, Some(10));
        assert_eq!(mouse_report.movement_y, Some(-10));
        assert_eq!(mouse_report.position_x, Some(100));
        assert_eq!(mouse_report.position_y, Some(200));
        assert_eq!(mouse_report.scroll_v, Some(1));
        assert_eq!(mouse_report.scroll_h, Some(-1));
        assert_eq!(mouse_report.pressed_buttons.unwrap(), vec![1]);

        // SensorInputReport
        let fidl_sensor_report =
            fidl_input_report::SensorInputReport { values: Some(vec![100, 200]) };
        let sensor_report = SensorInputReport::from(fidl_sensor_report);
        assert_eq!(sensor_report.values.unwrap(), vec![100, 200]);

        // SensorFeatureReport
        let fidl_sensor_feature = fidl_input_report::SensorFeatureReport {
            report_interval: Some(1000),
            reporting_state: Some(fidl_input_report::SensorReportingState::ReportAllEvents),
            sensitivity: Some(vec![1, 2]),
            threshold_high: Some(vec![10, 20]),
            threshold_low: Some(vec![5, 15]),
            sampling_rate: Some(50),
        };
        let sensor_feature = SensorFeatureReport::from(fidl_sensor_feature);
        assert_eq!(sensor_feature.report_interval, Some(1000));
        assert_eq!(sensor_feature.reporting_state.unwrap(), "ReportAllEvents");
        assert_eq!(sensor_feature.sensitivity.unwrap(), vec![1, 2]);
        assert_eq!(sensor_feature.threshold_high.unwrap(), vec![10, 20]);
        assert_eq!(sensor_feature.threshold_low.unwrap(), vec![5, 15]);
        assert_eq!(sensor_feature.sampling_rate, Some(50));

        // TouchInputReport
        let fidl_touch_report = fidl_input_report::TouchInputReport {
            contacts: Some(vec![fidl_input_report::ContactInputReport {
                contact_id: Some(1),
                position_x: Some(100),
                position_y: Some(200),
                pressure: Some(50),
                contact_width: Some(10),
                contact_height: Some(20),
                confidence: Some(true),
            }]),
            pressed_buttons: None,
        };
        let touch_report = TouchInputReport::from(fidl_touch_report);
        let contact = &touch_report.contacts.unwrap()[0];
        assert_eq!(contact.contact_id, Some(1));
        assert_eq!(contact.position_x, Some(100));
        assert_eq!(contact.position_y, Some(200));
        assert_eq!(contact.pressure, Some(50));
        assert_eq!(contact.contact_width, Some(10));
        assert_eq!(contact.contact_height, Some(20));
        assert_eq!(contact.confidence, Some(true));

        // TouchFeatureReport
        let fidl_touch_feature = fidl_input_report::TouchFeatureReport {
            input_mode: Some(
                fidl_input_report::TouchConfigurationInputMode::WindowsPrecisionTouchpadCollection,
            ),
            selective_reporting: Some(fidl_input_report::SelectiveReportingFeatureReport {
                surface_switch: Some(true),
                button_switch: Some(false),
            }),
        };
        let touch_feature = TouchFeatureReport::from(fidl_touch_feature);
        assert_eq!(touch_feature.input_mode.unwrap(), "WindowsPrecisionTouchpadCollection");
        assert_eq!(touch_feature.selective_reporting.unwrap().surface_switch, Some(true));

        // KeyboardInputReport
        let fidl_kb_report = fidl_input_report::KeyboardInputReport {
            pressed_keys3: Some(vec![fidl_next_fuchsia_input::Key::B]),
        };
        let kb_report = KeyboardInputReport::from(fidl_kb_report);
        assert_eq!(kb_report.pressed_keys3.unwrap(), vec!["B".to_string()]);

        // KeyboardOutputReport
        let fidl_kb_out_report = fidl_input_report::KeyboardOutputReport {
            enabled_leds: Some(vec![fidl_input_report::LedType::ScrollLock]),
        };
        let kb_out_report = KeyboardOutputReport::from(fidl_kb_out_report);
        assert_eq!(kb_out_report.enabled_leds.unwrap(), vec!["ScrollLock".to_string()]);

        // ConsumerControlInputReport
        let fidl_cc_report = fidl_input_report::ConsumerControlInputReport {
            pressed_buttons: Some(vec![fidl_input_report::ConsumerControlButton::VolumeDown]),
        };
        let cc_report = ConsumerControlInputReport::from(fidl_cc_report);
        assert_eq!(cc_report.pressed_buttons.unwrap(), vec!["VolumeDown".to_string()]);
    }

    #[test]
    fn test_input_report_from() {
        let fidl_report = fidl_input_report::InputReport {
            event_time: Some(12345),
            report_id: Some(1),
            mouse: Some(fidl_input_report::MouseInputReport {
                movement_x: Some(10),
                movement_y: Some(-20),
                position_x: None,
                position_y: None,
                scroll_v: None,
                scroll_h: None,
                pressed_buttons: None,
            }),
            sensor: Some(fidl_input_report::SensorInputReport { values: Some(vec![1, 2]) }),
            touch: Some(fidl_input_report::TouchInputReport {
                contacts: Some(vec![fidl_input_report::ContactInputReport {
                    contact_id: Some(1),
                    position_x: Some(100),
                    position_y: Some(200),
                    pressure: None,
                    contact_width: None,
                    contact_height: None,
                    confidence: Some(true),
                }]),
                pressed_buttons: None,
            }),
            keyboard: Some(fidl_input_report::KeyboardInputReport {
                pressed_keys3: Some(vec![fidl_next_fuchsia_input::Key::A]),
            }),
            consumer_control: Some(fidl_input_report::ConsumerControlInputReport {
                pressed_buttons: Some(vec![fidl_input_report::ConsumerControlButton::VolumeUp]),
            }),
            trace_id: None,
            wake_lease: None,
        };

        let report = InputReport::from(fidl_report);
        assert_eq!(report.event_time, Some(12345));
        assert_eq!(report.report_id, Some(1));

        let mouse = report.mouse.unwrap();
        assert_eq!(mouse.movement_x, Some(10));
        assert_eq!(mouse.movement_y, Some(-20));

        let sensor = report.sensor.unwrap();
        assert_eq!(sensor.values.unwrap(), vec![1, 2]);

        let touch = report.touch.unwrap();
        assert_eq!(touch.contacts.unwrap()[0].contact_id, Some(1));

        let keyboard = report.keyboard.unwrap();
        assert_eq!(keyboard.pressed_keys3.unwrap(), vec!["A".to_string()]);

        let consumer_control = report.consumer_control.unwrap();
        assert_eq!(consumer_control.pressed_buttons.unwrap(), vec!["VolumeUp".to_string()]);
    }
}
