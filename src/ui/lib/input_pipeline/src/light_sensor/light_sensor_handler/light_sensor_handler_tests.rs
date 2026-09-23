// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{
    ActiveSetting, ActiveSettingState, LightSensorHandler, MAX_SATURATION_BLUE,
    MAX_SATURATION_CLEAR, MAX_SATURATION_GREEN, MAX_SATURATION_RED, SaturatedError,
    correlated_color_temperature, div_round_closest, div_round_up, process_reading, saturated,
    to_us,
};
use crate::Transport;
use crate::input_device::{Handled, InputDeviceDescriptor, InputDeviceEvent, InputEvent};
use crate::input_handler::{InputHandler, InputHandlerStatus};
use crate::light_sensor::calibrator::Calibrate;
use crate::light_sensor::types::{AdjustmentSetting, Rgbc, SensorConfiguration};
use crate::light_sensor_binding::{LightSensorDeviceDescriptor, LightSensorEvent};
use crate::testing_utilities::next_client_old_stream;
use assert_matches::assert_matches;
use diagnostics_assertions::AnyProperty;
use fasync::Task;
use fidl::endpoints::create_proxy_and_stream;
use fidl_fuchsia_input_report::{
    FeatureReport, InputDeviceGetFeatureReportResult, InputDeviceMarker, InputDeviceRequest,
    InputDeviceSetFeatureReportResult, SensorFeatureReport, SensorReportingState,
};
use fidl_fuchsia_lightsensor::{SensorMarker, SensorProxy, SensorRequestStream};
use fidl_next_fuchsia_input_report::InputDevice;
use fuchsia_async as fasync;
use futures::StreamExt;
use std::cell::RefCell;
use std::rc::Rc;
use test_case::test_case;
use zx::MonotonicInstant;

const VENDOR_ID: u32 = 1;
const PRODUCT_ID: u32 = 2;

fn get_adjustment_settings() -> Vec<AdjustmentSetting> {
    vec![
        AdjustmentSetting { atime: 100, gain: 1 },
        AdjustmentSetting { atime: 100, gain: 4 },
        AdjustmentSetting { atime: 100, gain: 16 },
        AdjustmentSetting { atime: 100, gain: 64 },
        AdjustmentSetting { atime: 0, gain: 64 },
    ]
}

#[fuchsia::test]
fn to_us_converts_atime_to_microseconds() {
    let atime = 112;
    let us = to_us(atime);
    assert_eq!(us, 400_320);
}

#[test_case(11, 10 => 2; "1.1 rounds to 2")]
#[test_case(19, 10 => 2; "1.9 rounds to 2")]
#[fuchsia::test]
fn div_round_up_returns_ceil_of_div(n: u32, d: u32) -> u32 {
    div_round_up(n, d)
}

#[test_case(14, 10 => 1; "1.4 rounds to 1")]
#[test_case(15, 10 => 2; "1.5 rounds to 2")]
#[fuchsia::test]
fn div_round_closest_returns_half_rounding(n: u32, d: u32) -> u32 {
    div_round_closest(n, d)
}

#[test_case(Rgbc {
    red: MAX_SATURATION_RED,
    green: MAX_SATURATION_GREEN,
    blue: MAX_SATURATION_BLUE,
    clear: MAX_SATURATION_CLEAR,
} => true; "all max is saturated")]
#[test_case(Rgbc {
    red: MAX_SATURATION_RED,
    green: 0,
    blue: 0,
    clear: 0,
} => false; "only red max is not saturated")]
#[test_case(Rgbc {
    red: 0,
    green: MAX_SATURATION_GREEN,
    blue: 0,
    clear: 0,
} => false; "only green max is not saturated")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: MAX_SATURATION_BLUE,
    clear: 0,
} => false; "only blue max is not saturated")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: 0,
    clear: MAX_SATURATION_CLEAR,
} => false; "only clear max is not saturated")]
#[fuchsia::test]
fn saturated_cases(rgbc: Rgbc<u16>) -> bool {
    saturated(rgbc)
}

#[fuchsia::test]
fn cct() {
    let rgbc = Rgbc { red: 1.0, green: 2.0, blue: 3.0, clear: 4.0 };
    let cct = correlated_color_temperature(rgbc).expect("should not saturate");
    // See doc-comment for `correlated_color_temperature`.
    // let big_x = -0.7687 * 1.0 + 9.7764 * 2.0 + -7.4164 * 3.0 = -3.4651;
    // let big_y = -1.7475 * 1.0 + 9.9603 * 2.0 + -5.6755 * 3.0 = 1.1466;
    // let big_z = -3.6709 * 1.0 + 4.8637 * 2.0 + 4.3682 * 3.0 = 19.1611;

    // let div = big_x + big_y + big_z = 16.8426;
    // let x = big_x / div = -0.20573426905584646;
    // let y = big_y / div = 0.06807737522710271;
    // let n = (x - 0.3320) / (0.1858 - y)
    //       = (-0.20573426905584646 - 0.3320) / (0.1858 - 0.06807737522710271) = -4.567807336042735
    // Ok(449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33)
    //  = 5108.754
    const EXPECTED_COLOR_TEMPERATURE: f32 = 5108.754;
    assert!((cct - EXPECTED_COLOR_TEMPERATURE).abs() <= f32::EPSILON);
}

#[fuchsia::test]
fn cct_saturation() {
    let rgbc = Rgbc { red: 0.0, green: 0.0, blue: 0.0, clear: 1.0 };
    let result = correlated_color_temperature(rgbc);
    assert_matches!(result, None);
}

fn get_mock_device_proxy()
-> (fidl_next::Client<InputDevice, Transport>, Rc<RefCell<Option<FeatureReport>>>, fasync::Task<()>)
{
    get_mock_device_proxy_with_response(None, Ok(()))
}

fn get_mock_device_proxy_with_response(
    get_response: Option<InputDeviceGetFeatureReportResult>,
    response: InputDeviceSetFeatureReportResult,
) -> (fidl_next::Client<InputDevice, Transport>, Rc<RefCell<Option<FeatureReport>>>, fasync::Task<()>)
{
    let (device_proxy, mut stream) = next_client_old_stream::<InputDeviceMarker, InputDevice>();
    let called = Rc::new(RefCell::new(Option::<FeatureReport>::None));
    let task = fasync::Task::local({
        let called = Rc::clone(&called);
        async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    InputDeviceRequest::GetFeatureReport { responder } => match get_response {
                        Some(Ok(ref report)) => responder.send(Ok(report)),
                        Some(Err(e)) => responder.send(Err(e)),
                        None => match called.borrow().as_ref() {
                            Some(report) => responder.send(Ok(report)),
                            None => responder.send(Ok(&FeatureReport {
                                sensor: Some(SensorFeatureReport {
                                    report_interval: Some(1),
                                    sensitivity: Some(vec![16]),
                                    reporting_state: Some(SensorReportingState::ReportAllEvents),
                                    threshold_high: Some(vec![1]),
                                    threshold_low: Some(vec![1]),
                                    sampling_rate: Some(to_us(100) as i64),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })),
                        },
                    }
                    .expect("sending get response to test"),
                    InputDeviceRequest::SetFeatureReport { report, responder } => {
                        *called.borrow_mut() = Some(report);
                        responder.send(response).expect("sending set response to test");
                    }
                    _ => {} // no-op
                }
            }
        }
    });
    (device_proxy, called, task)
}

#[fuchsia::test(allow_stalls = false)]
async fn active_setting_adjusts_down_on_saturation() {
    let (device_proxy, called, task) = get_mock_device_proxy();
    let mut active_setting = ActiveSetting::new(get_adjustment_settings(), 1);
    let result = active_setting
        .adjust(
            Rgbc { red: 21_067, green: 20_395, blue: 20_939, clear: 65_085 },
            &device_proxy,
            |_| std::future::ready(()),
        )
        .await;
    assert_matches!(result, Err(SaturatedError::Saturated));
    assert_matches!(&*called.borrow(), &Some(FeatureReport {
        sensor: Some(SensorFeatureReport {
            sensitivity: Some(ref gains),
            // atime to microseconds: (256 - 100) * 2780
            sampling_rate: Some(433_680),
            ..
        }),
        ..
    }) if gains.len() == 1 && gains.contains(&1));
    drop(device_proxy);
    task.await;
}

#[test_case(Rgbc {
    red: 65_535,
    green: 0,
    blue: 0,
    clear: 0,
}; "red")]
#[test_case(Rgbc {
    red: 0,
    green: 65_535,
    blue: 0,
    clear: 0,
}; "green")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: 65_535,
    clear: 0,
}; "blue")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: 0,
    clear: 65_535,
}; "clear")]
#[fuchsia::test(allow_stalls = false)]
async fn active_setting_adjusts_down_on_single_channel_saturation(rgbc: Rgbc<u16>) {
    let (device_proxy, called, task) = get_mock_device_proxy();
    let mut active_setting = ActiveSetting::new(get_adjustment_settings(), 1);
    let result = active_setting.adjust(rgbc, &device_proxy, |_| std::future::ready(())).await;
    // Result is err because adjusting down occurs due to saturation.
    assert_matches!(result, Err(SaturatedError::Saturated));
    assert_matches!(&*called.borrow(), &Some(FeatureReport {
        sensor: Some(SensorFeatureReport {
            sensitivity: Some(ref gains),
            // atime to microseconds: (256 - 100) * 2780
            sampling_rate: Some(433_680),
            ..
        }),
        ..
    }) if gains.len() == 1 && gains.contains(&1));
    drop(device_proxy);
    task.await;
}

// Calculation for value in test
// let new_us = (256-new_atime)*2780; = (256-100)*2780 = 433_680
// let cur_us = (256-cur_atime)*2780;
// 65_534=v*((new_gain + cur_gain - 1) / cur_gain)*((new_us + cur_us - 1)/cur_us)+65_535/10
// v = (65_534-6_553)/((nagain + cgain - 1)/cgain)
// v = 14_745
#[test_case(Rgbc {
    red: 14_745,
    green: 0,
    blue: 0,
    clear: 0,
}; "red")]
#[test_case(Rgbc {
    red: 0,
    green: 14_745,
    blue: 0,
    clear: 0,
}; "green")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: 14_745,
    clear: 0,
}; "blue")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: 0,
    clear: 14_745,
}; "clear")]
#[fuchsia::test(allow_stalls = false)]
async fn active_setting_adjusts_up_on_low_readings(rgbc: Rgbc<u16>) {
    let (device_proxy, called, task) = get_mock_device_proxy();
    let mut active_setting = ActiveSetting::new(get_adjustment_settings(), 1);
    let result = active_setting.adjust(rgbc, &device_proxy, |_| std::future::ready(())).await;
    // Ok-true signifies the adjustment was pulled up to a higher sensitivity.
    assert_matches!(result, Ok(true));
    assert_matches!(&*called.borrow(), &Some(FeatureReport {
        sensor: Some(SensorFeatureReport {
            sensitivity: Some(ref gains),
            // atime to microseconds: (256 - 100) * 2780
            sampling_rate: Some(433_680),
            ..
        }),
        ..
    }) if gains.len() == 1 && gains.contains(&16));
    drop(device_proxy);
    task.await;
}

// Value is one above the calculation above.
#[test_case(Rgbc {
    red: 14_746,
    green: 0,
    blue: 0,
    clear: 0,
}; "red")]
#[test_case(Rgbc {
    red: 0,
    green: 14_746,
    blue: 0,
    clear: 0,
}; "green")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: 14_746,
    clear: 0,
}; "blue")]
#[test_case(Rgbc {
    red: 0,
    green: 0,
    blue: 0,
    clear: 14_746,
}; "clear")]
#[fuchsia::test(allow_stalls = false)]
async fn active_setting_does_not_adjust_on_high_readings(rgbc: Rgbc<u16>) {
    let (device_proxy, called, task) = get_mock_device_proxy();
    let mut active_setting = ActiveSetting::new(get_adjustment_settings(), 1);
    active_setting
        .adjust(rgbc, &device_proxy, |_| std::future::ready(()))
        .await
        .expect("should succeed");
    assert_matches!(&*called.borrow(), &None);
    drop(device_proxy);
    task.await;
}

#[fuchsia::test(allow_stalls = false)]
async fn active_setting_adjusts_down_on_saturation_reports_error() {
    let (device_proxy, _, task) =
        get_mock_device_proxy_with_response(None, Err(zx::sys::ZX_ERR_CONNECTION_RESET));
    let mut active_setting = ActiveSetting::new(get_adjustment_settings(), 1);
    active_setting
        .adjust(
            Rgbc { red: 21_067, green: 20_395, blue: 20_939, clear: 65_085 },
            &device_proxy,
            |_| std::future::ready(()),
        )
        .await
        .expect_err("should fail");
    drop(device_proxy);
    task.await;
}

#[fuchsia::test(allow_stalls = false)]
async fn active_setting_adjusts_down_on_single_channel_saturation_reports_error() {
    let (device_proxy, _, task) =
        get_mock_device_proxy_with_response(None, Err(zx::sys::ZX_ERR_CONNECTION_RESET));
    let mut active_setting = ActiveSetting::new(get_adjustment_settings(), 1);
    active_setting
        .adjust(Rgbc { red: 65_535, green: 0, blue: 0, clear: 0 }, &device_proxy, |_| {
            std::future::ready(())
        })
        .await
        .expect_err("should fail");
    drop(device_proxy);
    task.await;
}

#[fuchsia::test(allow_stalls = false)]
async fn active_setting_adjusts_up_on_low_readings_reports_error() {
    let (device_proxy, _, task) =
        get_mock_device_proxy_with_response(None, Err(zx::sys::ZX_ERR_CONNECTION_RESET));
    let mut active_setting = ActiveSetting::new(get_adjustment_settings(), 1);
    active_setting
        .adjust(Rgbc { red: 14_745, green: 0, blue: 0, clear: 0 }, &device_proxy, |_| {
            std::future::ready(())
        })
        .await
        .expect_err("should fail");
    drop(device_proxy);
    task.await;
}

#[fuchsia::test]
fn light_sensor_handler_process_reading_lower_gain() {
    let initial_adjustment = AdjustmentSetting { atime: 100, gain: 1 };
    let rgbc = process_reading(Rgbc { red: 1, green: 2, blue: 3, clear: 4 }, initial_adjustment);
    assert_eq!(rgbc, Rgbc { red: 105.0, green: 210.0, blue: 315.0, clear: 420.0 });
}

#[fuchsia::test]
fn light_sensor_handler_process_reading_higher_gain() {
    let initial_adjustment = AdjustmentSetting { atime: 100, gain: 64 };
    let rgbc = process_reading(Rgbc { red: 1, green: 2, blue: 3, clear: 4 }, initial_adjustment);
    assert_eq!(rgbc, Rgbc { red: 2.0, green: 3.0, blue: 5.0, clear: 7.0 });
}

#[fuchsia::test]
fn light_sensor_handler_calculate_lux() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 2.0, green: 3.0, blue: 5.0, clear: 7.0 },
        si_scaling_factors: Rgbc { red: 1.0, green: 1.0, blue: 1.0, clear: 1.0 },
        settings: vec![],
    };

    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");
    let inspect_status = InputHandlerStatus::new(
        &test_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let handler = LightSensorHandler::new((), sensor_configuration, inspect_status);
    let lux = handler.calculate_lux(Rgbc { red: 11.0, green: 13.0, blue: 17.0, clear: 19.0 });
    assert_eq!(lux, 2.0 * 11.0 + 3.0 * 13.0 + 5.0 * 17.0 + 7.0 * 19.0);
}

#[fuchsia::test(allow_stalls = false)]
async fn light_sensor_handler_no_calibrator_returns_uncalibrated() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 1.5, green: 1.6, blue: 1.7, clear: 1.8 },
        si_scaling_factors: Rgbc { red: 1.1, green: 1.2, blue: 1.3, clear: 1.4 },
        settings: get_adjustment_settings(),
    };

    let (device_proxy, called, task) = get_mock_device_proxy();
    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");
    let inspect_status = InputHandlerStatus::new(
        &test_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let handler =
        LightSensorHandler::<DoublingCalibrator>::new(None, sensor_configuration, inspect_status);
    // The first reading is always saturated as it initializing the device settings.
    let reading = handler
        .get_calibrated_data(Rgbc { red: 1, green: 2, blue: 3, clear: 14747 }, &device_proxy)
        .await;
    assert_matches!(reading, Err(SaturatedError::Saturated));
    // The call should have adjusted the sensor.
    assert_matches!(&*called.borrow(), &Some(FeatureReport {
            sensor: Some(SensorFeatureReport {
                sensitivity: Some(ref gains),
                // atime to microseconds: (256 - 100) * 2780
                sampling_rate: Some(433_680),
                ..
            }),
            ..
        }) if gains.len() == 1 && gains.contains(&1));

    let reading = handler
        .get_calibrated_data(Rgbc { red: 1, green: 2, blue: 3, clear: 14747 }, &device_proxy)
        .await;
    let reading = reading.expect("calibration should succeed");

    // r = round(1 * 64 * 256 / (256 - 100)) = 105.0
    // g = round(2 * 64 * 256 / (256 - 100)) = 210.0
    // b = round(3 * 64 * 256 / (256 - 100)) = 315.0
    // c = round(4 * 64 * 256 / (256 - 100)) = 1548813.0
    // r / 4 = 26.25
    // g / 4 = 52.5
    // b / 4 = 78.75
    // c / 4 = 387203.25
    assert!((reading.rgbc.red - 26.25).abs() <= f32::EPSILON);
    assert!((reading.rgbc.green - 52.5).abs() <= f32::EPSILON);
    assert!((reading.rgbc.blue - 78.75).abs() <= f32::EPSILON);
    assert!((reading.rgbc.clear - 387203.25).abs() <= f32::EPSILON);
    // si_r = r * 1.1 / (64 * 256) = 0.0070495605
    // si_g = g * 1.2 / (64 * 256) = 0.01538086
    // si_b = b * 1.3 / (64 * 256) = 0.024993896
    // si_c = c * 1.4 / (64 * 256) = 132.34486
    assert!((reading.si_rgbc.red - 0.0070495605).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.green - 0.01538086).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.blue - 0.024993896).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.clear - 132.34486).abs() <= f32::EPSILON);
    // = 0.0070495605 * 1.5 + 0.01538086 * 1.6 + 0.024993896 * 1.7 + 132.34486083984373 * 1.8
    assert!((reading.lux - 238.29842).abs() <= f32::EPSILON);
    // let big_x = -0.7687 * 0.0070495605 + 9.7764 * 0.01538086 + -7.4164 * 0.024993896
    //     = -0.040414304;
    // let big_y = -1.7475 * 0.0070495605 + 9.9603 * 0.01538086 + -5.6755 * 0.024993896
    //     = -0.0009739697;
    // let big_z = -3.6709 * 0.0070495605 + 4.8637 * 0.01538086 + 4.3682 * 0.024993896
    //     = 0.158108;

    // let div = big_x + big_y + big_z = 0.11671972;
    // let x = big_x / div = -0.34625086;
    // let y = big_y / div = -0.008344517;
    // let n = (x - 0.3320) / (0.1858 - y)
    //       = (-0.34625086 - 0.3320) / (0.1858 - -0.008344517) = -3.493536
    // Ok(449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33)
    //  = 5560.375
    assert!((reading.cct.unwrap() - 5560.375).abs() <= f32::EPSILON);
    assert!(!reading.is_calibrated);
    drop(device_proxy);

    // The second call should not have adjusted the sensor.
    assert_matches!(&*called.borrow(), &Some(FeatureReport {
        sensor: Some(SensorFeatureReport {
            sensitivity: Some(ref gains),
            // atime to microseconds: (256 - 100) * 2780
            sampling_rate: Some(433_680),
            ..
        }),
        ..
    }) if gains.len() == 1 && gains.contains(&1));

    task.await;
}

/// Mock calibrator that just multiplies the input by 2.
struct DoublingCalibrator;

impl Calibrate for DoublingCalibrator {
    fn calibrate(&self, rgbc: Rgbc<f32>) -> Rgbc<f32> {
        rgbc.map(|c| c * 2.0)
    }
}

#[fuchsia::test(allow_stalls = false)]
async fn light_sensor_handler_get_calibrated_data() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 1.5, green: 1.6, blue: 1.7, clear: 1.8 },
        si_scaling_factors: Rgbc { red: 1.1, green: 1.2, blue: 1.3, clear: 1.4 },
        settings: get_adjustment_settings(),
    };

    let (device_proxy, called, task) = get_mock_device_proxy();
    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");
    let inspect_status = InputHandlerStatus::new(
        &test_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let handler = LightSensorHandler::new(DoublingCalibrator, sensor_configuration, inspect_status);
    // The first reading is always saturated as it initializing the device settings.
    let reading = handler
        .get_calibrated_data(Rgbc { red: 1, green: 2, blue: 3, clear: 14747 }, &device_proxy)
        .await;
    assert_matches!(reading, Err(SaturatedError::Saturated));

    // The last call should have adjusted the sensor.
    assert_matches!(&*called.borrow(), &Some(FeatureReport {
        sensor: Some(SensorFeatureReport {
            sensitivity: Some(ref gains),
            // atime to microseconds: (256 - 100) * 2780
            sampling_rate: Some(433_680),
            ..
        }),
        ..
    }) if gains.len() == 1 && gains.contains(&1));

    let result = handler
        // Set a high clear reading so the sensor is not adjusted up. This simplifies the test
        // setup below so we don't have to account for skipped readings due to saturated inputs.
        .get_calibrated_data(Rgbc { red: 1, green: 2, blue: 3, clear: 14747 }, &device_proxy)
        .await;
    let reading = result.expect("calibration should succeed");

    // r = round(1 * 64 * 256 / (256 - 100)) = 105
    // g = round(2 * 64 * 256 / (256 - 100)) = 210
    // b = round(3 * 64 * 256 / (256 - 100)) = 315
    // c = round(14747 * 64 * 256 / (256 - 100)) = 1548813

    // r / 4 = 26.25
    // g / 4 = 52.5
    // b / 4 = 78.75
    // c / 4 = 387203.25
    assert!((reading.rgbc.red - 26.25).abs() <= f32::EPSILON);
    assert!((reading.rgbc.green - 52.5).abs() <= f32::EPSILON);
    assert!((reading.rgbc.blue - 78.75).abs() <= f32::EPSILON);
    assert!((reading.rgbc.clear - 387203.25).abs() <= f32::EPSILON);

    // Numbers on left of multiplication are calibrated + scaled for units.
    // 2 {double calibrator} * r * 1.1 {si_scaling_factor} / (64 * 256) (sensor counts to uW/cm^2) =
    //     0.014099121.
    // 2 * g * 1.2 / (64 * 256) = 0.03076172
    // 2 * b * 1.3 / (64 * 256) = 0.049987793
    // 2 * c * 1.4 / (64 * 256) = 264.6897216796875
    // Note readings are doubled compared to uncalibrated test.
    assert!((reading.si_rgbc.red - 0.014099121).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.green - 0.03076172).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.blue - 0.049987793).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.clear - 264.6897216796875).abs() <= f32::EPSILON);
    // = 0.014099121 * 1.5 + 0.03076172 * 1.6 + 0.049987793 * 1.7 + 264.6897216796875 * 1.8
    assert!((reading.lux - 476.59683).abs() <= f32::EPSILON);
    // Note reading matches result from uncalibrated test because doubling is cancelled out.
    // let big_x = -0.7687 * 0.014099121 + 9.7764 * 0.03076172 + -7.4164 * 0.049987793
    //     = -0.08082861;
    // let big_y = -1.7475 * 0.014099121 + 9.9603 * 0.03076172 + -5.6755 * 0.049987793
    //     = -0.0019479394;
    // let big_z = -3.6709 * 0.014099121 + 4.8637 * 0.03076172 + 4.3682 * 0.049987793
    //     = 0.316216;

    // let div = big_x + big_y + big_z = 0.23343945;
    // let x = big_x / div = -0.34625086;
    // let y = big_y / div = -0.008344517;
    // let n = (x - 0.3320) / (0.1858 - y)
    //       = (-0.34625086 - 0.3320) / (0.1858 - -0.008344517) = -3.493536
    // cct = 449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33
    assert!((reading.cct.unwrap() - 5560.375).abs() <= f32::EPSILON);
    assert!(reading.is_calibrated);

    // Attempt to read a low value so the sensor increases the gain.
    let _reading = handler
        .get_calibrated_data(Rgbc { red: 0, green: 0, blue: 0, clear: 0 }, &device_proxy)
        .await;

    // The last call should have adjusted the sensor.
    assert_matches!(&*called.borrow(), &Some(FeatureReport {
        sensor: Some(SensorFeatureReport {
            sensitivity: Some(ref gains),
            // atime to microseconds: (256 - 100) * 2780
            sampling_rate: Some(433_680),
            ..
        }),
        ..
    }) if gains.len() == 1 && gains.contains(&4));

    // Since the sensor is adjusted, reading the same values should now return a different result.
    let reading = handler
        .get_calibrated_data(Rgbc { red: 1, green: 2, blue: 3, clear: 14747 }, &device_proxy)
        .await;
    let reading = reading.expect("calibration should succeed");
    // r = round(1 * 16 * 256 / (256 - 100)) = 26
    // g = round(2 * 16 * 256 / (256 - 100)) = 53
    // b = round(3 * 16 * 256 / (256 - 100)) = 79
    // c = round(14747 * 16 * 256 / (256 - 100)) = 387203

    // r / 4 = 6.5
    // g / 4 = 13.25
    // b / 4 = 19.75
    // c / 4 = 96800.75
    assert!((reading.rgbc.red - 6.5).abs() <= f32::EPSILON);
    assert!((reading.rgbc.green - 13.25).abs() <= f32::EPSILON);
    assert!((reading.rgbc.blue - 19.75).abs() <= f32::EPSILON);
    assert!((reading.rgbc.clear - 96800.75).abs() <= f32::EPSILON);

    // Numbers on left of multiplication are calibrated + scaled for units.
    // 2 {double calibrator} * r * 1.1 {si_scaling_factor} / (64 * 256) (sensor counts to uW/cm^2) =
    //     0.003491211.
    // 2 * g * 1.2 / (64 * 256) = 0.007763672
    // 2 * b * 1.3 / (64 * 256) = 0.012536621
    // 2 * c * 1.4 / (64 * 256) = 66.172386
    // Note readings are doubled compared to uncalibrated test.
    assert!((reading.si_rgbc.red - 0.003491211).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.green - 0.007763672).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.blue - 0.012536621).abs() <= f32::EPSILON);
    assert!((reading.si_rgbc.clear - 66.172386).abs() <= f32::EPSILON);
    // = 0.003491211 * 1.5 + 0.007763672 * 1.6 + 0.012536621 * 1.7 + 66.172386 * 1.8
    assert!((reading.lux - 119.14926).abs() <= f32::EPSILON);
    // let big_x = -0.7687 * 0.003491211 + 9.7764 * 0.007763672 + -7.4164 * 0.012536621
    //     = -0.01975952
    // let big_y = -1.7475 * 0.003491211 + 9.9603 * 0.007763672 + -5.6755 * 0.012536621
    //     = 7.6025724e-5
    // let big_z = -3.6709 * 0.003491211 + 4.8637 * 0.007763672 + 4.3682 * 0.012536621
    //     = 0.07970675
    //
    // let div = big_x + big_y + big_z = 0.060023256;
    // let x = big_x / div = -0.32919776;
    // let y = big_y / div = 0.0012666045;
    // let n = (x - 0.3320) / (0.1858 - y)
    //       = (-0.32919776 - 0.3320) / (0.1858 - 0.0012666045) = -3.583079
    // Ok(449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33)
    //  = 5672.924
    assert!((reading.cct.unwrap() - 5672.924).abs() <= f32::EPSILON);
    assert!(reading.is_calibrated);

    drop(device_proxy);
    task.await;
}

#[fuchsia::test(allow_stalls = false)]
async fn light_sensor_handler_get_calibrated_data_should_fallback_on_proxy_error() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 1.5, green: 1.6, blue: 1.7, clear: 1.8 },
        si_scaling_factors: Rgbc { red: 1.1, green: 1.2, blue: 1.3, clear: 1.4 },
        settings: get_adjustment_settings(),
    };

    let (device_proxy, _, task) =
        get_mock_device_proxy_with_response(None, Err(zx::sys::ZX_ERR_CONNECTION_RESET));
    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");
    let inspect_status = InputHandlerStatus::new(
        &test_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let handler = LightSensorHandler::new(DoublingCalibrator, sensor_configuration, inspect_status);
    let reading = handler
        .get_calibrated_data(Rgbc { red: 1, green: 2, blue: 3, clear: 4 }, &device_proxy)
        .await;
    reading.expect("Should process reading");
    assert_matches!(&*handler.active_setting.borrow(), &ActiveSettingState::Static(..));
    drop(device_proxy);
    task.await;
}

#[fuchsia::test(allow_stalls = false)]
async fn light_sensor_handler_input_event_handler() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 1.5, green: 1.6, blue: 1.7, clear: 1.8 },
        si_scaling_factors: Rgbc { red: 1.1, green: 1.2, blue: 1.3, clear: 1.4 },
        settings: get_adjustment_settings(),
    };

    let (device_proxy, _, task) = get_mock_device_proxy();
    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");
    let inspect_status = InputHandlerStatus::new(
        &test_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let handler = LightSensorHandler::new(DoublingCalibrator, sensor_configuration, inspect_status);

    let (sensor_proxy, stream): (SensorProxy, SensorRequestStream) =
        create_proxy_and_stream::<SensorMarker>();
    // Register stream so subscriber is created.
    let request_task = Task::local({
        let handler = Rc::clone(&handler);
        async move {
            handler.handle_light_sensor_request_stream(stream).await.expect("can register");
        }
    });

    let input_event = InputEvent {
        device_event: InputDeviceEvent::LightSensor(LightSensorEvent {
            device_proxy,
            rgbc: Rgbc { red: 1, green: 2, blue: 3, clear: 14747 },
        }),
        device_descriptor: InputDeviceDescriptor::LightSensor(LightSensorDeviceDescriptor {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            device_id: 3,
            sensor_layout: Rgbc { red: 1, green: 2, blue: 3, clear: 4 },
        }),
        event_time: MonotonicInstant::get(),
        handled: Handled::No,
        trace_id: None,
    };

    // Trigger the first event. The first event will trigger an override of the settings on the
    // device, and will not send out any update.
    let events = Rc::clone(&handler).handle_input_event(input_event.clone()).await;

    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.handled, Handled::No);
    drop(events);

    // Trigger the second event. The data should match what was used in
    // `light_sensor_handler_get_calibrated_data` so the same results will be returned.
    let events = handler.handle_input_event(input_event).await;

    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.handled, Handled::Yes);

    // Wait for the results in a separate task while we trigger the event below.
    let reading = sensor_proxy.watch().await.expect("watch called");
    drop(sensor_proxy);

    let rgbc = &reading.rgbc.unwrap();
    // The readings should match the results in the `light_sensor_handler_get_calibrated_data` test.
    assert!((rgbc.red_intensity - 26.25).abs() <= f32::EPSILON);
    assert!((rgbc.green_intensity - 52.5).abs() <= f32::EPSILON);
    assert!((rgbc.blue_intensity - 78.75).abs() <= f32::EPSILON);
    assert!((rgbc.clear_intensity - 387203.25).abs() <= f32::EPSILON);
    assert!((reading.calculated_lux.unwrap() - 476.59683).abs() <= f32::EPSILON);
    assert!((reading.correlated_color_temperature.unwrap() - 5560.375).abs() <= f32::EPSILON);
    drop(events);
    request_task.await;
    task.await;
}

// Ensure a default value of 0.0 is not returned for the lux.
#[fuchsia::test(allow_stalls = false)]
async fn light_sensor_handler_subscriber_queue() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 1.5, green: 1.6, blue: 1.7, clear: 1.8 },
        si_scaling_factors: Rgbc { red: 1.1, green: 1.2, blue: 1.3, clear: 1.4 },
        settings: get_adjustment_settings(),
    };

    let (device_proxy, _, task) = get_mock_device_proxy();
    let inspector = fuchsia_inspect::Inspector::default();
    let test_node = inspector.root().create_child("test_node");
    let inspect_status = InputHandlerStatus::new(
        &test_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let handler = LightSensorHandler::new(DoublingCalibrator, sensor_configuration, inspect_status);

    let (sensor_proxy, stream): (SensorProxy, SensorRequestStream) =
        create_proxy_and_stream::<SensorMarker>();
    // Register stream so subscriber is created.
    let request_task = Task::local({
        let handler = Rc::clone(&handler);
        async move {
            handler.handle_light_sensor_request_stream(stream).await.expect("can register");
        }
    });

    let watch_task = Task::local(async move {
        // Wait for the results in a separate task while we trigger the event below.
        let reading = sensor_proxy.watch().await.expect("watch called");
        drop(sensor_proxy);
        reading
    });

    let input_event = InputEvent {
        device_event: InputDeviceEvent::LightSensor(LightSensorEvent {
            device_proxy,
            rgbc: Rgbc { red: 1, green: 2, blue: 3, clear: 14747 },
        }),
        device_descriptor: InputDeviceDescriptor::LightSensor(LightSensorDeviceDescriptor {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            device_id: 3,
            sensor_layout: Rgbc { red: 1, green: 2, blue: 3, clear: 4 },
        }),
        event_time: MonotonicInstant::get(),
        handled: Handled::No,
        trace_id: None,
    };

    // Trigger the first event. The first event will trigger an override of the settings on the
    // device, and will not send out any update.
    let events = Rc::clone(&handler).handle_input_event(input_event.clone()).await;

    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.handled, Handled::No);
    drop(events);

    // Trigger the second event. The data should match what was used in
    // `light_sensor_handler_get_calibrated_data` so the same results will be returned.
    let events = handler.handle_input_event(input_event).await;

    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.handled, Handled::Yes);

    let reading = watch_task.await;
    assert!(reading.calculated_lux != Some(0.0));
    drop(events);
    request_task.await;
    task.await;
}

#[fuchsia::test]
async fn light_sensor_handler_initialized_with_inspect_node() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 2.0, green: 3.0, blue: 5.0, clear: 7.0 },
        si_scaling_factors: Rgbc { red: 1.0, green: 1.0, blue: 1.0, clear: 1.0 },
        settings: vec![],
    };
    let inspector = fuchsia_inspect::Inspector::default();
    let fake_handlers_node = inspector.root().create_child("input_handlers_node");
    let inspect_status = InputHandlerStatus::new(
        &fake_handlers_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let _handler =
        LightSensorHandler::new(DoublingCalibrator, sensor_configuration, inspect_status);
    diagnostics_assertions::assert_data_tree!(inspector, root: {
        input_handlers_node: {
            light_sensor_handler: {
                clients_connected_count: 0u64,
                events_received_count: 0u64,
                events_saturated_count: 0u64,
                events_handled_count: 0u64,
                last_received_timestamp_ns: 0u64,
                recent_feature_events_log: {},
                "fuchsia.inspect.Health": {
                    status: "STARTING_UP",
                    // Timestamp value is unpredictable and not relevant in this context,
                    // so we only assert that the property is present.
                    start_timestamp_nanos: diagnostics_assertions::AnyProperty
                },
            }
        }
    });
}

#[fuchsia::test]
async fn light_sensor_handler_inspect_counts_events() {
    let sensor_configuration = SensorConfiguration {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        rgbc_to_lux_coefficients: Rgbc { red: 1.5, green: 1.6, blue: 1.7, clear: 1.8 },
        si_scaling_factors: Rgbc { red: 1.1, green: 1.2, blue: 1.3, clear: 1.4 },
        settings: get_adjustment_settings(),
    };

    let (device_proxy, _, _task) = get_mock_device_proxy();
    let inspector = fuchsia_inspect::Inspector::default();
    let fake_handlers_node = inspector.root().create_child("input_handlers_node");
    let inspect_status = InputHandlerStatus::new(
        &fake_handlers_node,
        "light_sensor_handler",
        /* generates_events */ false,
    );
    let handler = LightSensorHandler::new(DoublingCalibrator, sensor_configuration, inspect_status);

    let (sensor_proxy, stream): (SensorProxy, SensorRequestStream) =
        create_proxy_and_stream::<SensorMarker>();
    // Register stream so subscriber is created.
    let request_task = Task::local({
        let handler = Rc::clone(&handler);
        async move {
            handler.handle_light_sensor_request_stream(stream).await.expect("can register");
        }
    });

    // Called so we can initialize ActiveSettingState
    let _ = Rc::clone(&handler)
        .get_calibrated_data(Rgbc { red: 1, green: 1, blue: 1, clear: 1 }, &device_proxy)
        .await;

    let input_event = InputEvent {
        device_event: InputDeviceEvent::LightSensor(LightSensorEvent {
            device_proxy: device_proxy.clone(),
            rgbc: Rgbc { red: 1, green: 2, blue: 3, clear: 14747 },
        }),
        device_descriptor: InputDeviceDescriptor::LightSensor(LightSensorDeviceDescriptor {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            device_id: 3,
            sensor_layout: Rgbc { red: 1, green: 2, blue: 3, clear: 4 },
        }),
        event_time: MonotonicInstant::get(),
        handled: Handled::No,
        trace_id: None,
    };

    // Handle an unhandled input event.
    let _ = Rc::clone(&handler).handle_input_event(input_event.clone()).await;

    // Client connected to handler to watch for updated sensor readings
    let _reading = sensor_proxy.watch().await.expect("watch called");

    // Handled event should be ignored.
    let handled_event = InputEvent {
        device_event: InputDeviceEvent::LightSensor(LightSensorEvent {
            device_proxy: device_proxy.clone(),
            rgbc: Rgbc { red: 1, green: 2, blue: 3, clear: 14747 },
        }),
        device_descriptor: InputDeviceDescriptor::LightSensor(LightSensorDeviceDescriptor {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            device_id: 3,
            sensor_layout: Rgbc { red: 1, green: 2, blue: 3, clear: 4 },
        }),
        event_time: MonotonicInstant::get(),
        handled: Handled::Yes,
        trace_id: None,
    };
    let _ = Rc::clone(&handler).handle_input_event(handled_event).await;

    let input_event2 = InputEvent {
        device_event: InputDeviceEvent::LightSensor(LightSensorEvent {
            device_proxy: device_proxy.clone(),
            rgbc: Rgbc { red: 0, green: 10, blue: 0, clear: 14700 },
        }),
        device_descriptor: InputDeviceDescriptor::LightSensor(LightSensorDeviceDescriptor {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            device_id: 3,
            sensor_layout: Rgbc { red: 1, green: 2, blue: 3, clear: 4 },
        }),
        event_time: MonotonicInstant::get(),
        handled: Handled::No,
        trace_id: None,
    };

    // Handle an unhandled input event.
    let _ = Rc::clone(&handler).handle_input_event(input_event2.clone()).await;

    // Client makes subsequent call to handler
    let _reading2 = sensor_proxy.watch().await.expect("watch called");

    let saturated_input_event = InputEvent {
        device_event: InputDeviceEvent::LightSensor(LightSensorEvent {
            device_proxy: device_proxy.clone(),
            rgbc: Rgbc { red: 21067, green: 20395, blue: 20939, clear: 65085 },
        }),
        device_descriptor: InputDeviceDescriptor::LightSensor(LightSensorDeviceDescriptor {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            device_id: 3,
            sensor_layout: Rgbc { red: 1, green: 2, blue: 3, clear: 4 },
        }),
        event_time: MonotonicInstant::get(),
        handled: Handled::No,
        trace_id: None,
    };
    let last_event_timestamp: u64 =
        saturated_input_event.clone().event_time.into_nanos().try_into().unwrap();

    // Handle an unhandled input event with saturated rgbc reading.
    // Event should be discarded and counted to `events_saturated_count`.
    let _ = Rc::clone(&handler).handle_input_event(saturated_input_event.clone()).await;

    diagnostics_assertions::assert_data_tree!(inspector, root: {
        input_handlers_node: {
            light_sensor_handler: {
                clients_connected_count: 1u64,
                events_received_count: 3u64,
                events_saturated_count: 1u64,
                events_handled_count: 2u64,
                last_received_timestamp_ns: last_event_timestamp,
                recent_feature_events_log: {
                  "000_feature_report_update_event": {
                    event_time: AnyProperty,
                    sampling_rate: 433_680_i64,
                    sensitivity: 1_i64,
                  },
                  "001_feature_report_update_event": {
                    event_time: AnyProperty,
                    sampling_rate: 433_680_i64,
                    sensitivity: 4_i64,
                  },
                  "002_feature_report_update_event": {
                    event_time: AnyProperty,
                    sampling_rate: 433_680_i64,
                    sensitivity: 1_i64,
                  },
                },
                "fuchsia.inspect.Health": {
                    status: "STARTING_UP",
                    // Timestamp value is unpredictable and not relevant in this context,
                    // so we only assert that the property is present.
                    start_timestamp_nanos: diagnostics_assertions::AnyProperty
                },
            }
        }
    });

    drop(sensor_proxy);
    request_task.await;

    // Update clients_connected_count once stream is terminated & client stops sending requests.
    diagnostics_assertions::assert_data_tree!(inspector, root: {
        input_handlers_node: {
            light_sensor_handler: contains {
                clients_connected_count: 0u64,
            }
        }
    });
}
