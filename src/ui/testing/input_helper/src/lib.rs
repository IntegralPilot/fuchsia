// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::event::Event as AsyncEvent;
use fidl_fuchsia_input::Key;
use fidl_fuchsia_input_injection::InputDeviceRegistryMarker;
use fidl_fuchsia_input_report::{
    ConsumerControlInputReport, ContactInputReport, DeviceInformation, InputReport,
    KeyboardInputReport, MouseInputReport, TouchInputReport,
};
use fidl_fuchsia_math as math;
use fidl_fuchsia_power_system as fps;
use fidl_fuchsia_time_alarms as fta;
use fidl_fuchsia_ui_display_singleton as display_info;
use fidl_fuchsia_ui_input::KeyboardReport;
use fidl_fuchsia_ui_test_input::{
    CoordinateUnit, KeyboardMarker, KeyboardRequest, KeyboardRequestStream,
    MediaButtonsDeviceMarker, MediaButtonsDeviceRequest, MediaButtonsDeviceRequestStream,
    MouseMarker, MouseRequest, MouseRequestStream,
    RegistryRegisterKeyboardAndGetDeviceInfoResponse,
    RegistryRegisterMediaButtonsDeviceAndGetDeviceInfoResponse,
    RegistryRegisterMouseAndGetDeviceInfoResponse,
    RegistryRegisterTouchScreenAndGetDeviceInfoResponse, RegistryRequest, RegistryRequestStream,
    TouchScreenMarker, TouchScreenRequest, TouchScreenRequestStream,
};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_trace::Scope;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use keymaps::inverse_keymap::{InverseKeymap, Shift};
use keymaps::usages::{Usages, hid_usage_to_input3_key};
use log::{error, info, warn};
use std::time::Duration;

mod input_device;
mod input_device_registry;
mod input_reports_reader;

pub(crate) static INPUT_CATEGORY: &str = "input";

/// Use this to place required DeviceInformation into DeviceDescriptor.
fn new_fake_device_info() -> DeviceInformation {
    DeviceInformation {
        product_id: Some(42),
        vendor_id: Some(43),
        version: Some(u32::MAX),
        polling_rate: Some(1000),
        ..Default::default()
    }
}

/// Converts the `input` string into a key sequence under the `InverseKeymap` derived from `keymap`.
///
/// This is intended for end-to-end and input testing only; for production use cases and general
/// testing, IME injection should be used instead.
///
/// A translation from `input` to a sequence of keystrokes is not guaranteed to exist. If a
/// translation does not exist, `None` is returned.
///
/// The sequence does not contain pauses except between repeated keys or to clear a shift state,
/// though the sequence does terminate with an empty report (no keys pressed). A shift key
/// transition is sent in advance of each series of keys that needs it.
///
/// Note that there is currently no way to distinguish between particular key releases. As such,
/// only one key release report is generated even in combinations, e.g. Shift + A.
///
/// # Example
///
/// ```
/// let key_sequence = derive_key_sequence(&keymaps::US_QWERTY, "A").unwrap();
///
/// // [shift, A, clear]
/// assert_eq!(key_sequence.len(), 3);
/// ```
///
/// TODO(https://fxbug.dev/42059899): Simplify the logic in this test.
fn derive_key_sequence(keymap: &keymaps::Keymap<'_>, input: &str) -> Option<Vec<KeyboardReport>> {
    let inverse_keymap = InverseKeymap::new(keymap);
    let mut reports = vec![];
    let mut shift_pressed = false;
    let mut last_usage = None;

    for ch in input.chars() {
        let key_stroke = inverse_keymap.get(&ch)?;

        match key_stroke.shift {
            Shift::Yes if !shift_pressed => {
                shift_pressed = true;
                last_usage = Some(0);
            }
            Shift::No if shift_pressed => {
                shift_pressed = false;
                last_usage = Some(0);
            }
            _ => {
                if last_usage == Some(key_stroke.usage) {
                    last_usage = Some(0);
                }
            }
        }

        if let Some(0) = last_usage {
            reports.push(KeyboardReport {
                pressed_keys: if shift_pressed {
                    vec![Usages::HidUsageKeyLeftShift as u32]
                } else {
                    vec![]
                },
            });
        }

        last_usage = Some(key_stroke.usage);

        reports.push(KeyboardReport {
            pressed_keys: if shift_pressed {
                vec![key_stroke.usage, Usages::HidUsageKeyLeftShift as u32]
            } else {
                vec![key_stroke.usage]
            },
        });
    }

    reports.push(KeyboardReport { pressed_keys: vec![] });

    Some(reports)
}

fn convert_keyboard_report_to_keys(report: &KeyboardReport) -> Vec<Key> {
    report
        .pressed_keys
        .iter()
        .map(|&usage| {
            hid_usage_to_input3_key(usage.try_into().expect("failed to convert usage to u16"))
                .unwrap_or_else(|| panic!("no Key for usage {:?}", usage))
        })
        .collect()
}

async fn register_touch_screen(
    mut registry: input_device_registry::InputDeviceRegistry,
    scope: &fasync::Scope,
    device_server_end: fidl::endpoints::ServerEnd<TouchScreenMarker>,
    got_input_reports_reader: AsyncEvent,
    min_x: i64,
    max_x: i64,
    min_y: i64,
    max_y: i64,
) -> input_device::DeviceId {
    let device = registry
        .add_touchscreen_device(min_x, max_x, min_y, max_y)
        .await
        .expect("failed to create fake touchscreen device");
    let device_id = device.device_id;

    scope.spawn(async move {
        handle_touchscreen_request_stream(device, device_server_end.into_stream()).await;
    });

    info!("wait for input-pipeline setup input-reader");
    let _ = got_input_reports_reader.wait().await;
    info!("input-pipeline setup input-reader.");

    device_id
}

async fn register_media_button(
    mut registry: input_device_registry::InputDeviceRegistry,
    scope: &fasync::Scope,
    device_server_end: fidl::endpoints::ServerEnd<MediaButtonsDeviceMarker>,
    got_input_reports_reader: AsyncEvent,
) -> input_device::DeviceId {
    let device = registry
        .add_media_buttons_device()
        .await
        .expect("failed to create fake media buttons device");
    let device_id = device.device_id;

    let activity_governor = connect_to_protocol::<fps::ActivityGovernorMarker>().ok();
    let wake_alarm_proxy = connect_to_protocol::<fta::WakeAlarmsMarker>().ok();
    scope.spawn(async move {
        handle_media_buttons_device_request_stream(
            device,
            device_server_end.into_stream(),
            activity_governor,
            wake_alarm_proxy,
        )
        .await;
    });

    info!("wait for input-pipeline setup input-reader");
    let _ = got_input_reports_reader.wait().await;
    info!("input-pipeline setup input-reader.");

    device_id
}

async fn register_keyboard(
    mut registry: input_device_registry::InputDeviceRegistry,
    scope: &fasync::Scope,
    device_server_end: fidl::endpoints::ServerEnd<KeyboardMarker>,
    got_input_reports_reader: AsyncEvent,
) -> input_device::DeviceId {
    let device = registry.add_keyboard_device().await.expect("failed to create fake keyboard");
    let device_id = device.device_id;

    scope.spawn(async move {
        handle_keyboard_request_stream(device, device_server_end.into_stream()).await;
    });

    info!("wait for input-pipeline setup input-reader");
    let _ = got_input_reports_reader.wait().await;
    info!("input-pipeline setup input-reader.");

    device_id
}

async fn register_mouse(
    mut registry: input_device_registry::InputDeviceRegistry,
    scope: &fasync::Scope,
    device_server_end: fidl::endpoints::ServerEnd<MouseMarker>,
    got_input_reports_reader: AsyncEvent,
) -> input_device::DeviceId {
    let device = registry.add_mouse_device().await.expect("failed to create fake mouse");
    let device_id = device.device_id;

    scope.spawn(async move {
        handle_mouse_request_stream(device, device_server_end.into_stream()).await;
    });

    info!("wait for input-pipeline setup input-reader");
    let _ = got_input_reports_reader.wait().await;
    info!("input-pipeline setup input-reader.");

    device_id
}

/// Serves `fuchsia.ui.test.input.Registry`.
pub async fn handle_registry_request_stream(request_stream: RegistryRequestStream) {
    request_stream
        .try_for_each_concurrent(None, |request| async {
            let input_device_registry = connect_to_protocol::<InputDeviceRegistryMarker>()
                .expect("connect to input_device_registry");
            let got_input_reports_reader = AsyncEvent::new();

            let registry = input_device_registry::InputDeviceRegistry::new(
                input_device_registry,
                got_input_reports_reader.clone(),
            );
            let scope = fasync::Scope::new();

            match request {
                RegistryRequest::RegisterTouchScreen { payload, responder, .. } => {
                    info!("register touchscreen");
                    let device = payload
                        .device
                        .expect("no touchscreen device provided in registration request");
                    let (min_x, max_x, min_y, max_y) = match payload.coordinate_unit {
                        Some(CoordinateUnit::PhysicalPixels) => {
                            let display_info_proxy =
                                connect_to_protocol::<display_info::InfoMarker>()
                                    .expect("failed to connect to display info service");
                            let display_dimensions = display_info_proxy
                                .get_metrics()
                                .await
                                .expect("failed to get display metrics")
                                .extent_in_px
                                .expect("display metrics missing extent in px");
                            (
                                0,
                                display_dimensions.width as i64,
                                0,
                                display_dimensions.height as i64,
                            )
                        }
                        Some(CoordinateUnit::RegisteredDimensions) => {
                            let dimensions =
                                payload.display_dimensions.expect("missing dimensions");
                            if dimensions.min_x >= dimensions.max_x
                                || dimensions.min_y >= dimensions.max_y
                            {
                                panic!("invalid dimensions");
                            }
                            (dimensions.min_x, dimensions.max_x, dimensions.min_y, dimensions.max_y)
                        }
                        _ => (-1000, 1000, -1000, 1000),
                    };

                    register_touch_screen(
                        registry,
                        &scope,
                        device,
                        got_input_reports_reader,
                        min_x,
                        max_x,
                        min_y,
                        max_y,
                    )
                    .await;

                    responder.send().expect("Failed to respond to RegisterTouchScreen request");
                }
                RegistryRequest::RegisterTouchScreenAndGetDeviceInfo {
                    payload, responder, ..
                } => {
                    info!("register touchscreen");
                    let device = payload
                        .device
                        .expect("no touchscreen device provided in registration request");
                    let (min_x, max_x, min_y, max_y) = match payload.coordinate_unit {
                        Some(CoordinateUnit::PhysicalPixels) => {
                            let display_info_proxy =
                                connect_to_protocol::<display_info::InfoMarker>()
                                    .expect("failed to connect to display info service");
                            let display_dimensions = display_info_proxy
                                .get_metrics()
                                .await
                                .expect("failed to get display metrics")
                                .extent_in_px
                                .expect("display metrics missing extent in px");
                            (
                                0,
                                display_dimensions.width as i64,
                                0,
                                display_dimensions.height as i64,
                            )
                        }
                        Some(CoordinateUnit::RegisteredDimensions) => {
                            let dimensions =
                                payload.display_dimensions.expect("missing dimensions");
                            if dimensions.min_x >= dimensions.max_x
                                || dimensions.min_y >= dimensions.max_y
                            {
                                panic!("invalid dimensions");
                            }
                            (dimensions.min_x, dimensions.max_x, dimensions.min_y, dimensions.max_y)
                        }
                        _ => (-1000, 1000, -1000, 1000),
                    };

                    let device_id = register_touch_screen(
                        registry,
                        &scope,
                        device,
                        got_input_reports_reader,
                        min_x,
                        max_x,
                        min_y,
                        max_y,
                    )
                    .await;

                    responder
                        .send(RegistryRegisterTouchScreenAndGetDeviceInfoResponse {
                            device_id: Some(device_id),
                            ..Default::default()
                        })
                        .expect("Failed to respond to RegisterTouchScreenAndGetDeviceInfo request");
                }
                RegistryRequest::RegisterMediaButtonsDevice { payload, responder, .. } => {
                    info!("register media buttons device");
                    let device = payload
                        .device
                        .expect("no media buttons device provided in registration request");

                    register_media_button(registry, &scope, device, got_input_reports_reader).await;

                    responder
                        .send()
                        .expect("Failed to respond to RegisterMediaButtonsDevice request");
                }
                RegistryRequest::RegisterMediaButtonsDeviceAndGetDeviceInfo {
                    payload,
                    responder,
                    ..
                } => {
                    info!("register media buttons device");
                    let device = payload
                        .device
                        .expect("no media buttons device provided in registration request");

                    let device_id =
                        register_media_button(registry, &scope, device, got_input_reports_reader)
                            .await;

                    responder.send(RegistryRegisterMediaButtonsDeviceAndGetDeviceInfoResponse {
                        device_id: Some(device_id),
                        ..Default::default()
                    }).expect(
                        "Failed to respond to RegisterMediaButtonsDeviceAndGetDeviceInfo request",
                    );
                }
                RegistryRequest::RegisterKeyboard { payload, responder, .. } => {
                    info!("register keyboard device");
                    let device = payload
                        .device
                        .expect("no keyboard device provided in registration request");

                    register_keyboard(registry, &scope, device, got_input_reports_reader).await;

                    responder.send().expect("Failed to respond to RegisterKeyboard request");
                }
                RegistryRequest::RegisterKeyboardAndGetDeviceInfo {
                    payload, responder, ..
                } => {
                    info!("register keyboard device");
                    let device = payload
                        .device
                        .expect("no keyboard device provided in registration request");

                    let device_id =
                        register_keyboard(registry, &scope, device, got_input_reports_reader).await;

                    responder
                        .send(RegistryRegisterKeyboardAndGetDeviceInfoResponse {
                            device_id: Some(device_id),
                            ..Default::default()
                        })
                        .expect("Failed to respond to RegisterKeyboardAndGetDeviceInfo request");
                }
                RegistryRequest::RegisterMouse { payload, responder } => {
                    info!("register mouse device");
                    let device = payload.device.expect("no mouse provided in registration request");

                    register_mouse(registry, &scope, device, got_input_reports_reader).await;

                    responder.send().expect("Failed to respond to RegisterMouse request");
                }
                RegistryRequest::RegisterMouseAndGetDeviceInfo { payload, responder } => {
                    info!("register mouse device");
                    let device = payload.device.expect("no mouse provided in registration request");

                    let device_id =
                        register_mouse(registry, &scope, device, got_input_reports_reader).await;

                    responder
                        .send(RegistryRegisterMouseAndGetDeviceInfoResponse {
                            device_id: Some(device_id),
                            ..Default::default()
                        })
                        .expect("Failed to respond to RegisterMouse request");
                }
            }

            scope.join().await;
            Ok(())
        })
        .await
        .expect("failed to serve test realm factory request stream");
}

fn input_report_for_touch_contacts(
    contacts: Vec<(u32, math::Vec_)>,
    trace_id: Option<u64>,
) -> InputReport {
    let contact_input_reports = contacts
        .into_iter()
        .map(|(contact_id, location)| ContactInputReport {
            contact_id: Some(contact_id),
            position_x: Some(location.x as i64),
            position_y: Some(location.y as i64),
            contact_width: Some(0),
            contact_height: Some(0),
            ..Default::default()
        })
        .collect();

    let touch_input_report = TouchInputReport {
        contacts: Some(contact_input_reports),
        pressed_buttons: Some(vec![]),
        ..Default::default()
    };

    InputReport {
        event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
        touch: Some(touch_input_report),
        trace_id,
        ..Default::default()
    }
}

/// Serves `fuchsia.ui.test.input.TouchScreen`.
async fn handle_touchscreen_request_stream(
    touchscreen_device: input_device::InputDevice,
    mut request_stream: TouchScreenRequestStream,
) {
    while let Some(request) = request_stream.next().await {
        match request {
            Ok(TouchScreenRequest::SimulateTap { payload, responder }) => {
                let tap_location = payload.tap_location.expect("missing tap location");
                {
                    fuchsia_trace::duration!("input", "simulate_tap_down");
                    let trace_id = fuchsia_trace::Id::new();
                    fuchsia_trace::flow_begin!("input", "input_report", trace_id);
                    touchscreen_device
                        .send_input_report(input_report_for_touch_contacts(
                            vec![(1, tap_location)],
                            Some(trace_id.into()),
                        ))
                        .expect("Failed to send touch down input report");
                }

                // Send a report with an empty set of touch contacts, so that input
                // pipeline generates a pointer event with phase == UP.
                fuchsia_trace::duration!("input", "simulate_tap_up");
                let trace_id = fuchsia_trace::Id::new();
                fuchsia_trace::flow_begin!("input", "input_report", trace_id);
                touchscreen_device
                    .send_input_report(input_report_for_touch_contacts(
                        vec![],
                        Some(trace_id.into()),
                    ))
                    .expect("failed to send touch up input report");

                responder.send().expect("Failed to send SimulateTap response");
            }
            Ok(TouchScreenRequest::SimulateSwipe { payload, responder }) => {
                // Compute the x- and y- displacements between successive touch events.
                let start_location = payload.start_location.expect("missing start location");
                let end_location = payload.end_location.expect("missing end location");
                let move_event_count = payload.move_event_count.expect("missing move event count");
                assert_ne!(move_event_count, 0);
                let duration_nanos = payload.duration.unwrap_or(0);
                let delay_nanos = duration_nanos / ((move_event_count as i64) + 1);
                let delay = fasync::MonotonicDuration::from_nanos(delay_nanos);

                let start_x_f = start_location.x as f64;
                let start_y_f = start_location.y as f64;
                let end_x_f = end_location.x as f64;
                let end_y_f = end_location.y as f64;
                let move_event_count_f = move_event_count as f64;
                let step_size_x = (end_x_f - start_x_f) / move_event_count_f;
                let step_size_y = (end_y_f - start_y_f) / move_event_count_f;

                // Generate an event at `start_location`, followed by `move_event_count - 1`
                // evenly-spaced events, followed by an event at `end_location`.
                for i in 0..move_event_count + 1 {
                    let i_f = i as f64;
                    let event_x = start_x_f + (i_f * step_size_x);
                    let event_y = start_y_f + (i_f * step_size_y);

                    {
                        fuchsia_trace::duration!("input", "simulate_swipe_move", "idx"=>i);
                        let trace_id = fuchsia_trace::Id::new();
                        fuchsia_trace::flow_begin!("input", "input_report", trace_id);

                        touchscreen_device
                            .send_input_report(input_report_for_touch_contacts(
                                vec![(1, math::Vec_ { x: event_x as i32, y: event_y as i32 })],
                                Some(trace_id.into()),
                            ))
                            .expect("Failed to send tap input report");
                    }
                    fasync::Timer::new(delay).await;
                }

                // Send a report with an empty set of touch contacts, so that input
                // pipeline generates a pointer event with phase == UP.
                fuchsia_trace::duration!("input", "simulate_swipe_up");
                let trace_id = fuchsia_trace::Id::new();
                fuchsia_trace::flow_begin!("input", "input_report", trace_id);
                touchscreen_device
                    .send_input_report(input_report_for_touch_contacts(
                        vec![],
                        Some(trace_id.into()),
                    ))
                    .expect("failed to send empty input report");

                responder.send().expect("Failed to send SimulateSwipe response");
            }
            Ok(TouchScreenRequest::SimulateMultiTap { payload, responder }) => {
                let tap_locations = payload.tap_locations.expect("missing tap locations");

                {
                    fuchsia_trace::duration!("input", "simulate_multi_tap_down");
                    let trace_id = fuchsia_trace::Id::new();
                    fuchsia_trace::flow_begin!("input", "input_report", trace_id);
                    touchscreen_device
                        .send_input_report(input_report_for_touch_contacts(
                            tap_locations
                                .into_iter()
                                .enumerate()
                                .map(|(i, it)| (i as u32, it))
                                .collect(),
                            Some(trace_id.into()),
                        ))
                        .expect("Failed to send tap input report");
                }

                // Send a report with an empty set of touch contacts, so that input
                // pipeline generates a pointer event with phase == UP.
                fuchsia_trace::duration!("input", "simulate_multi_tap_up");
                let trace_id = fuchsia_trace::Id::new();
                fuchsia_trace::flow_begin!("input", "input_report", trace_id);
                touchscreen_device
                    .send_input_report(input_report_for_touch_contacts(
                        vec![],
                        Some(trace_id.into()),
                    ))
                    .expect("failed to send empty input report");
                responder.send().expect("Failed to send SimulateMultiTap response");
            }
            Ok(TouchScreenRequest::SimulateMultiFingerGesture { payload, responder }) => {
                // Compute the x- and y- displacements between successive touch events.
                let start_locations = payload.start_locations.expect("missing start locations");
                let end_locations = payload.end_locations.expect("missing end locations");
                let move_event_count = payload.move_event_count.expect("missing move event count");
                let finger_count = payload.finger_count.expect("missing finger count") as usize;

                let move_event_count_f = move_event_count as f32;

                let mut steps: Vec<math::VecF> = vec![];

                for finger in 0..finger_count {
                    let start_x = start_locations[finger].x as f32;
                    let start_y = start_locations[finger].y as f32;
                    let end_x = end_locations[finger].x as f32;
                    let end_y = end_locations[finger].y as f32;
                    let step_x = (end_x - start_x) / move_event_count_f;
                    let step_y = (end_y - start_y) / move_event_count_f;
                    steps.push(math::VecF { x: step_x, y: step_y });
                }

                // Generate an event at `start_location`, followed by `move_event_count - 1`
                // evenly-spaced events, followed by an event at `end_location`.
                for i in 0..move_event_count {
                    let i_f = i as f32;

                    let mut contacts: Vec<(u32, math::Vec_)> = vec![];

                    for finger in 0..finger_count {
                        let start_x = start_locations[finger].x as f32;
                        let start_y = start_locations[finger].y as f32;
                        let event_x = (start_x + i_f * steps[finger].x) as i32;
                        let event_y = (start_y + i_f * steps[finger].y) as i32;
                        contacts.push((finger as u32, math::Vec_ { x: event_x, y: event_y }));
                    }

                    fuchsia_trace::duration!("input", "simulate_multi_finger_gesture_move", "idx"=>i);
                    let trace_id = fuchsia_trace::Id::new();
                    fuchsia_trace::flow_begin!("input", "input_report", trace_id);

                    touchscreen_device
                        .send_input_report(input_report_for_touch_contacts(
                            contacts,
                            Some(trace_id.into()),
                        ))
                        .expect("Failed to send tap input report");
                }

                // Send a report with an empty set of touch contacts, so that input
                // pipeline generates a pointer event with phase == UP.
                fuchsia_trace::duration!("input", "simulate_multi_finger_gesture_up");
                let trace_id = fuchsia_trace::Id::new();
                fuchsia_trace::flow_begin!("input", "input_report", trace_id);
                touchscreen_device
                    .send_input_report(input_report_for_touch_contacts(
                        vec![],
                        Some(trace_id.into()),
                    ))
                    .expect("failed to send empty input report");

                responder.send().expect("Failed to send SimulateMultiFingerGesture response");
            }
            Ok(TouchScreenRequest::SimulateTouchEvent { report, responder }) => {
                fuchsia_trace::duration!("input", "simulate_touch_event");
                let trace_id = fuchsia_trace::Id::new();
                fuchsia_trace::flow_begin!("input", "input_report", trace_id);
                let touch_report = fidl_fuchsia_input_report::TouchInputReport {
                    contacts: report.contacts.map(|contacts| {
                        contacts
                            .into_iter()
                            .map(|c| fidl_fuchsia_input_report::ContactInputReport {
                                contact_id: c.contact_id,
                                position_x: c.position_x,
                                position_y: c.position_y,
                                ..Default::default()
                            })
                            .collect()
                    }),
                    ..Default::default()
                };
                let input_report = InputReport {
                    event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
                    touch: Some(touch_report),
                    trace_id: Some(trace_id.into()),
                    ..Default::default()
                };
                touchscreen_device
                    .send_input_report(input_report)
                    .expect("failed to send empty input report");
                responder.send().expect("Failed to send SimulateTouchEvent response");
            }
            Err(e) => {
                error!("Error on touchscreen channel: {}", e);
                return;
            }
        }
    }
}

/// Serves `fuchsia.ui.test.input.MediaButtonsDevice`.
async fn handle_media_buttons_device_request_stream(
    media_buttons_device: input_device::InputDevice,
    request_stream: MediaButtonsDeviceRequestStream,
    activity_governor: Option<fps::ActivityGovernorProxy>,
    wake_alarm_proxy: Option<fta::WakeAlarmsProxy>,
) {
    let mut request_stream = request_stream.fuse();
    let (alarm_sender, mut alarm_receiver) = futures::channel::mpsc::unbounded();

    // Incomplete tasks are dropped when this function exits, avoiding detached orphan tasks.
    let mut scheduled_inputs = FuturesUnordered::new();

    loop {
        futures::select! {
            request = request_stream.next() => {
                match request {
                    Some(Ok(MediaButtonsDeviceRequest::SimulateButtonPress { payload, responder })) => {
                        if let Some(button) = payload.button {
                            let button = fidl_fuchsia_input::ConsumerControlButton::from_primitive(button.into_primitive()).unwrap();
                            let wake_lease = if let Some(ref ag) = activity_governor {
                                acquire_and_deposit_lease(ag, "input-helper-button").await
                            } else {
                                None
                            };
                            send_media_button_press_and_release(&media_buttons_device, button, wake_lease);
                            responder.send().expect("Failed to send SimulateButtonPress response");
                        } else {
                            warn!("SimulateButtonPress request missing button");
                            let _ = responder.send();
                        }
                    }
                    Some(Ok(MediaButtonsDeviceRequest::SendButtonsState { payload, responder })) => {
                        let buttons = match payload.buttons {
                            Some(buttons) => buttons.into_iter().map(|b| fidl_fuchsia_input::ConsumerControlButton::from_primitive(b.into_primitive()).unwrap()).collect(),
                            None => vec![],
                        };
                        let wake_lease = if let Some(ref ag) = activity_governor {
                            acquire_and_deposit_lease(ag, "input-helper-button-state").await
                        } else {
                            None
                        };
                        let input_report = InputReport {
                            event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
                            consumer_control: Some(ConsumerControlInputReport {
                                pressed_buttons: Some(buttons),
                                ..Default::default()
                            }),
                            wake_lease,
                            ..Default::default()
                        };

                        media_buttons_device
                            .send_input_report(input_report)
                            .expect("Failed to send button press input report");

                        responder.send().expect("Failed to send SimulateButtonPress response");
                    }

                    Some(Ok(MediaButtonsDeviceRequest::ScheduleSimulateButtonPress { payload, responder })) => {
                        let button = fidl_fuchsia_input::ConsumerControlButton::from_primitive(payload.button.expect("missing button").into_primitive()).unwrap();
                        let delay = zx::Duration::from_nanos(payload.delay.expect("missing delay"));
                        let sender = alarm_sender.clone();
                        let proxy = wake_alarm_proxy.clone();

                        scheduled_inputs.push(
                        fasync::Task::local(async move {
                            if let Some(alarm_proxy) = proxy {
                                // Use boot instant here since it will increase,
                                // even when the device is suspended.
                                let time = zx::BootInstant::after(delay);
                                info!("scheduling button press for {:?}", time);
                                fuchsia_trace::instant!(
                                    INPUT_CATEGORY,
                                    "WakeupTest:TimerSet",
                                    Scope::Process
                                );
                                let (_lease, peer) = zx::EventPair::create();
                                match alarm_proxy
                                    .set_and_wait(time, fta::SetMode::KeepAlive(peer), "input_helper_button")
                                    .await
                                {
                                    Ok(Ok(lease_token)) => {
                                        let _ = sender.unbounded_send(Some((button, lease_token)));
                                    }
                                    Ok(Err(e)) => {
                                        error!("Failed to set wakeup alarm: {:?}", e);
                                    }
                                    Err(e) => {
                                        error!("FIDL error: {:?}", e);
                                    }
                                }
                            } else {
                                error!("failed to connect to WakeAlarms protocol");
                            }
                        }));
                        responder.send().expect("Failed to send ScheduleSimulateButtonPress response");
                    }
                    Some(Err(e)) => {
                        error!("Error on media buttons device channel: {}", e);
                        break;
                    }
                    None => break,
                }
            },
            // while processing the request stream, keep checking for scheduled tasks that sent the
            // fired alarm info.
            fired_alarm = alarm_receiver.next() => {
                if let Some(Some((button, lease_token))) = fired_alarm {
                    info!("wake alarm fired, sending button event");
                    // Trace how long we spend processing the timer. It should be inconsequential to the resume
                    // latency.
                    fuchsia_trace::duration!(INPUT_CATEGORY,"WakeupTest:OnTimer");

                    // Duplicate for local deposit to mimic hardware safety timeout.
                    if let Ok(local_lease) = lease_token.duplicate_handle(zx::Rights::SAME_RIGHTS) {
                        deposit_wake_lease(local_lease, zx::MonotonicDuration::from_millis(100));
                    }

                    send_media_button_press_and_release(&media_buttons_device, button, Some(lease_token));
                }
            },
            _ = scheduled_inputs.select_next_some() => {},
        }
    }
}

fn deposit_wake_lease(lease: zx::EventPair, duration: zx::MonotonicDuration) {
    fasync::Task::local(async move {
        fasync::Timer::new(fasync::MonotonicInstant::after(duration)).await;
        std::mem::drop(lease);
    })
    .detach();
}

/// Helper to acquire a wake lease and deposit a local safety copy.
async fn acquire_and_deposit_lease(
    ag: &fps::ActivityGovernorProxy,
    name: &str,
) -> Option<zx::EventPair> {
    match ag.acquire_wake_lease(name).await {
        Ok(Ok(lease)) => {
            if let Ok(local_lease) = lease.duplicate_handle(zx::Rights::SAME_RIGHTS) {
                deposit_wake_lease(local_lease, zx::MonotonicDuration::from_millis(100));
            }
            Some(lease)
        }
        Ok(Err(e)) => {
            error!("Failed to acquire wake lease {name}: {:?}", e);
            None
        }
        Err(e) => {
            error!("FIDL error acquiring wake lease {name}: {:?}", e);
            None
        }
    }
}

fn send_media_button_press_and_release(
    media_buttons_device: &input_device::InputDevice,
    button: fidl_fuchsia_input::ConsumerControlButton,
    wake_lease: Option<zx::EventPair>,
) {
    let media_buttons_input_report =
        ConsumerControlInputReport { pressed_buttons: Some(vec![button]), ..Default::default() };

    let release_wake_lease =
        wake_lease.as_ref().and_then(|lease| lease.duplicate_handle(zx::Rights::SAME_RIGHTS).ok());

    let input_report = InputReport {
        event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
        consumer_control: Some(media_buttons_input_report),
        wake_lease,
        ..Default::default()
    };

    media_buttons_device
        .send_input_report(input_report)
        .expect("Failed to send button press input report");

    // Send a report with an empty set of pressed buttons,
    // so that input pipeline generates a media buttons
    // event with the target button being released.
    let empty_report = InputReport {
        event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
        consumer_control: Some(ConsumerControlInputReport {
            pressed_buttons: Some(vec![]),
            ..Default::default()
        }),
        wake_lease: release_wake_lease,
        ..Default::default()
    };

    media_buttons_device
        .send_input_report(empty_report)
        .expect("Failed to send button release input report");
}

/// Serves `fuchsia.ui.test.input.Keyboard`.
async fn handle_keyboard_request_stream(
    keyboard_device: input_device::InputDevice,
    mut request_stream: KeyboardRequestStream,
) {
    while let Some(request) = request_stream.next().await {
        match request {
            Ok(KeyboardRequest::SimulateUsAsciiTextEntry { payload, responder }) => {
                if let Some(text) = payload.text {
                    let key_sequence = derive_key_sequence(&keymaps::US_QWERTY, &text)
                        .expect("Failed to derive key sequence");

                    let mut key_iter = key_sequence.into_iter().peekable();
                    while let Some(keyboard_report) = key_iter.next() {
                        let input_report = InputReport {
                            event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
                            keyboard: Some(KeyboardInputReport {
                                pressed_keys3: Some(convert_keyboard_report_to_keys(
                                    &keyboard_report,
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        };

                        keyboard_device
                            .send_input_report(input_report)
                            .expect("Failed to send key event report");

                        if key_iter.peek().is_some() {
                            fuchsia_async::Timer::new(Duration::from_millis(100)).await;
                        }
                    }

                    responder.send().expect("Failed to send SimulateTextEntry response");
                } else {
                    warn!("SimulateTextEntry request missing text");
                }
            }
            Ok(KeyboardRequest::SimulateKeyEvent { payload, responder }) => {
                let keyboard_report = fidl_fuchsia_input_report::KeyboardInputReport {
                    pressed_keys3: payload.pressed_keys,
                    ..Default::default()
                };
                let input_report = InputReport {
                    event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
                    keyboard: Some(keyboard_report),
                    ..Default::default()
                };

                keyboard_device
                    .send_input_report(input_report)
                    .expect("Failed to send key event report");

                responder.send().expect("Failed to send SimulateKeyEvent response");
            }
            Ok(KeyboardRequest::SimulateKeyPress { payload, responder }) => {
                let key_code = payload.key_code.expect("no key code");

                let down_report = InputReport {
                    event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
                    keyboard: Some(KeyboardInputReport {
                        pressed_keys3: Some(vec![key_code]),
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                let up_report = InputReport {
                    event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
                    keyboard: Some(KeyboardInputReport {
                        pressed_keys3: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                keyboard_device.send_input_report(down_report).expect("Failed to send key down");

                keyboard_device.send_input_report(up_report).expect("Failed to send key up");

                responder.send().expect("Failed to send SimulateKeyPress response");
            }
            Err(e) => {
                error!("Error on keyboard device channel: {}", e);
                return;
            }
        }
    }
}

/// Serves `fuchsia.ui.test.input.Mouse`.
async fn handle_mouse_request_stream(
    mouse_device: input_device::InputDevice,
    mut request_stream: MouseRequestStream,
) {
    while let Some(request) = request_stream.next().await {
        match request {
            Ok(MouseRequest::SimulateMouseEvent { payload, responder }) => {
                let mut mouse_input_report = MouseInputReport {
                    movement_x: payload.movement_x,
                    movement_y: payload.movement_y,
                    scroll_v: payload.scroll_v_detent,
                    scroll_h: payload.scroll_h_detent,
                    ..Default::default()
                };
                if let Some(pressed_buttons) = payload.pressed_buttons {
                    mouse_input_report.pressed_buttons = Some(
                        pressed_buttons
                            .into_iter()
                            .map(|b| {
                                b.into_primitive()
                                    .try_into()
                                    .expect("failed to convert MouseButton to u8")
                            })
                            .collect(),
                    );
                }

                let input_report = InputReport {
                    event_time: Some(fasync::MonotonicInstant::now().into_nanos()),
                    mouse: Some(mouse_input_report),
                    ..Default::default()
                };

                mouse_device
                    .send_input_report(input_report)
                    .expect("Failed to send key event report");

                responder.send().expect("Failed to send SimulateMouseEvent response");
            }
            Err(e) => {
                error!("Error on keyboard device channel: {}", e);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Most of the functions in this file need to bind to FIDL services in
    // this component's environment to do their work, but a component can't
    // modify its own environment. Hence, we can't validate those functions.
    //
    // However, we can (and do) validate derive_key_sequence().

    use super::{
        KeyboardReport, Usages, derive_key_sequence, handle_media_buttons_device_request_stream,
    };
    use pretty_assertions::assert_eq;

    // TODO(https://fxbug.dev/42059899): Remove this macro.
    macro_rules! reports {
        ( $( [ $( $usages:expr ),* ] ),* $( , )? ) => {
            Some(vec![
                $(
                    KeyboardReport {
                        pressed_keys: vec![$($usages as u32),*]
                    }
                ),*
            ])
        }
    }

    #[test]
    fn lowercase() {
        assert_eq!(
            derive_key_sequence(&keymaps::US_QWERTY, "lowercase"),
            reports![
                [Usages::HidUsageKeyL],
                [Usages::HidUsageKeyO],
                [Usages::HidUsageKeyW],
                [Usages::HidUsageKeyE],
                [Usages::HidUsageKeyR],
                [Usages::HidUsageKeyC],
                [Usages::HidUsageKeyA],
                [Usages::HidUsageKeyS],
                [Usages::HidUsageKeyE],
                [],
            ]
        );
    }

    #[test]
    fn numerics() {
        assert_eq!(
            derive_key_sequence(&keymaps::US_QWERTY, "0123456789"),
            reports![
                [Usages::HidUsageKey0],
                [Usages::HidUsageKey1],
                [Usages::HidUsageKey2],
                [Usages::HidUsageKey3],
                [Usages::HidUsageKey4],
                [Usages::HidUsageKey5],
                [Usages::HidUsageKey6],
                [Usages::HidUsageKey7],
                [Usages::HidUsageKey8],
                [Usages::HidUsageKey9],
                [],
            ]
        );
    }

    #[test]
    fn internet_text_entry() {
        assert_eq!(
            derive_key_sequence(&keymaps::US_QWERTY, "http://127.0.0.1:8080"),
            reports![
                [Usages::HidUsageKeyH],
                [Usages::HidUsageKeyT],
                [],
                [Usages::HidUsageKeyT],
                [Usages::HidUsageKeyP],
                // ':'
                // Shift is actuated first on its own, then together with
                // the key.
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeySemicolon, Usages::HidUsageKeyLeftShift],
                [],
                [Usages::HidUsageKeySlash],
                [],
                [Usages::HidUsageKeySlash],
                [Usages::HidUsageKey1],
                [Usages::HidUsageKey2],
                [Usages::HidUsageKey7],
                [Usages::HidUsageKeyDot],
                [Usages::HidUsageKey0],
                [Usages::HidUsageKeyDot],
                [Usages::HidUsageKey0],
                [Usages::HidUsageKeyDot],
                [Usages::HidUsageKey1],
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeySemicolon, Usages::HidUsageKeyLeftShift],
                [],
                [Usages::HidUsageKey8],
                [Usages::HidUsageKey0],
                [Usages::HidUsageKey8],
                [Usages::HidUsageKey0],
                [],
            ]
        );
    }

    #[test]
    fn sentence() {
        assert_eq!(
            derive_key_sequence(&keymaps::US_QWERTY, "Hello, world!"),
            reports![
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyH, Usages::HidUsageKeyLeftShift],
                [],
                [Usages::HidUsageKeyE],
                [Usages::HidUsageKeyL],
                [],
                [Usages::HidUsageKeyL],
                [Usages::HidUsageKeyO],
                [Usages::HidUsageKeyComma],
                [Usages::HidUsageKeySpace],
                [Usages::HidUsageKeyW],
                [Usages::HidUsageKeyO],
                [Usages::HidUsageKeyR],
                [Usages::HidUsageKeyL],
                [Usages::HidUsageKeyD],
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKey1, Usages::HidUsageKeyLeftShift],
                [],
            ]
        );
    }

    #[test]
    fn hold_shift() {
        assert_eq!(
            derive_key_sequence(&keymaps::US_QWERTY, "ALL'S WELL!"),
            reports![
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyA, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyL, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyL, Usages::HidUsageKeyLeftShift],
                [],
                [Usages::HidUsageKeyApostrophe],
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyS, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeySpace, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyW, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyE, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyL, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyL, Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKey1, Usages::HidUsageKeyLeftShift],
                [],
            ]
        );
    }

    #[test]
    fn tab_and_newline() {
        assert_eq!(
            derive_key_sequence(&keymaps::US_QWERTY, "\tHello\n"),
            reports![
                [Usages::HidUsageKeyTab],
                [Usages::HidUsageKeyLeftShift],
                [Usages::HidUsageKeyH, Usages::HidUsageKeyLeftShift],
                [],
                [Usages::HidUsageKeyE],
                [Usages::HidUsageKeyL],
                [],
                [Usages::HidUsageKeyL],
                [Usages::HidUsageKeyO],
                [Usages::HidUsageKeyEnter],
                [],
            ]
        );
    }

    #[fuchsia::test]
    async fn test_schedule_power_button_press() {
        use fidl_fuchsia_input_report::{ConsumerControlButton, InputReport};
        use fidl_fuchsia_time_alarms as fta;
        use fidl_fuchsia_ui_test_input::MediaButtonsDeviceMarker;
        use futures::{FutureExt, StreamExt};

        let (report_sender, mut report_receiver) =
            futures::channel::mpsc::unbounded::<InputReport>();
        let input_device = crate::input_device::InputDevice::new_for_test(report_sender);

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<MediaButtonsDeviceMarker>();

        let (alarm_proxy, mut alarm_stream) =
            fidl::endpoints::create_proxy_and_stream::<fta::WakeAlarmsMarker>();
        let alarm_proxy_clone = alarm_proxy.clone();

        let handler_fut = handle_media_buttons_device_request_stream(
            input_device,
            stream,
            None,
            Some(alarm_proxy_clone),
        );

        let test_fut = async {
            let schedule_fut = proxy.schedule_simulate_button_press(
                &fidl_fuchsia_ui_test_input::MediaButtonsDeviceScheduleSimulateButtonPressRequest {
                    #[cfg(fuchsia_api_level_at_least = "32")]
                    button: Some(fidl_fuchsia_input::ConsumerControlButton::Power),
                    #[cfg(not(fuchsia_api_level_at_least = "32"))]
                    button: Some(ConsumerControlButton::Power),
                    delay: Some(zx::Duration::<zx::BootTimeline>::from_millis(750).into_nanos()), // 0.75s
                    ..Default::default()
                },
            );

            schedule_fut.await.expect("failed to schedule");

            // Mock WakeAlarms.SetAndWait.
            if let Some(Ok(fta::WakeAlarmsRequest::SetAndWait { responder, .. })) =
                alarm_stream.next().await
            {
                let (lease, _peer) = zx::EventPair::create();
                responder.send(Ok(lease)).expect("failed to respond");
            }

            // Receive press report
            let press_report: InputReport = report_receiver.next().await.expect("no press report");
            assert!(press_report.consumer_control.is_some());
            let cc = press_report.consumer_control.unwrap();
            assert_eq!(cc.pressed_buttons, Some(vec![ConsumerControlButton::Power]));
            assert!(press_report.wake_lease.is_some());

            // Receive release report
            let release_report: InputReport =
                report_receiver.next().await.expect("no release report");
            let cc = release_report.consumer_control.unwrap();
            assert_eq!(cc.pressed_buttons, Some(vec![]));
            assert!(release_report.wake_lease.is_some());
        };

        futures::select! {
            _ = handler_fut.fuse() => panic!("handler exited early"),
            _ = test_fut.fuse() => (),
        }
    }
}
