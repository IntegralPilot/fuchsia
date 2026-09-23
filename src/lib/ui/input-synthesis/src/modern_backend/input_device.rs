// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

use crate::modern_backend::input_reports_reader::InputReportsReaderV2;
use crate::synthesizer;
use crate::usages::hid_usage_to_input3_key;
use anyhow::{Context as _, Error, format_err};
use async_trait::async_trait;
use fidl::Error as FidlError;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_input::Key;
use fidl_fuchsia_input_report::{
    ConsumerControlButton, ConsumerControlInputReport, ContactInputReport, DeviceDescriptor,
    FeatureReport, InputDeviceRequest, InputDeviceRequestStream, InputReport,
    InputReportsReaderV2Marker, KeyboardInputReport, MouseInputReport, TOUCH_MAX_CONTACTS,
    TouchInputReport,
};
use fidl_fuchsia_ui_input::{KeyboardReport, Touch};
use fuchsia_async as fasync;
use futures::{StreamExt, TryFutureExt, future, pin_mut};
use std::convert::TryFrom as _;

/// Implements the `synthesizer::InputDevice` trait, and the server side of the
/// `fuchsia.input.report.InputDevice` FIDL protocol. Used by
/// `modern_backend::InputDeviceRegistry`.
///
/// # Notes
/// * Some of the methods of `fuchsia.input.report.InputDevice` are not relevant to
///   input injection, so this implemnentation does not support them:
///   * `SendOutputReport` provides a way to change keyboard LED state.
///   If these FIDL methods are invoked, `InputDevice::flush()` will resolve to Err.
/// * This implementation does not support multiple calls to `GetInputReportsReaderV2`,
///   since:
///   * The ideal semantics for multiple calls are not obvious, and
///   * Each `InputDevice` has a single FIDL client (an input pipeline implementation),
///     and the current input pipeline implementation is happy to use a single
///     `InputReportsReaderV2` for the lifetime of the `InputDevice`.
pub(super) struct InputDevice {
    /// FIFO queue of reports to be consumed by calls to
    /// `fuchsia.input.report.InputReportsReader.ReadInputReports()`.
    /// Populated by calls to `synthesizer::InputDevice` trait methods.
    report_sender: futures::channel::mpsc::UnboundedSender<InputReport>,

    // `Task` to keep serving the `fuchsia.input.report.InputDevice` protocol.
    input_device_task: fasync::Task<Result<(), Error>>,
}

impl std::convert::From<synthesizer::MediaButton> for ConsumerControlButton {
    fn from(synthesizer_button: synthesizer::MediaButton) -> Self {
        match synthesizer_button {
            synthesizer::MediaButton::VolumeUp => Self::VolumeUp,
            synthesizer::MediaButton::VolumeDown => Self::VolumeDown,
            synthesizer::MediaButton::MicMute => Self::MicMute,
            synthesizer::MediaButton::FactoryReset => Self::FactoryReset,
            synthesizer::MediaButton::Pause => Self::Pause,
            synthesizer::MediaButton::CameraDisable => Self::CameraDisable,
        }
    }
}

#[async_trait(?Send)]
impl synthesizer::InputDevice for self::InputDevice {
    fn media_buttons(
        &mut self,
        pressed_buttons: Vec<synthesizer::MediaButton>,
        time: u64,
    ) -> Result<(), Error> {
        self.report_sender
            .unbounded_send(InputReport {
                event_time: Some(i64::try_from(time).context("converting time to i64")?),
                consumer_control: Some(ConsumerControlInputReport {
                    pressed_buttons: Some(pressed_buttons.into_iter().map(Into::into).collect()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .context("sending media button InputReport")
    }

    // TODO(https://fxbug.dev/42142533): remove dependency on HID usage codes.
    fn key_press(&mut self, report: KeyboardReport, time: u64) -> Result<(), Error> {
        self.key_press_internal(report, time, Self::convert_keyboard_report_to_keys)
    }

    fn key_press_raw(&mut self, report: KeyboardReport, time: u64) -> Result<(), Error> {
        self.key_press_internal(report, time, Self::convert_keyboard_report_to_keys_no_transform)
    }

    // TODO(https://fxbug.dev/42142533): remove reference to HID usage codes.
    fn key_press_usage(&mut self, usage: Option<u32>, time: u64) -> Result<(), Error> {
        self.key_press(KeyboardReport { pressed_keys: usage.into_iter().collect() }, time)
    }

    fn tap(&mut self, pos: Option<(u32, u32)>, time: u64) -> Result<(), Error> {
        let fingers = pos.and_then(|(x, y)| {
            Some(vec![Touch { finger_id: 1, x: x as i32, y: y as i32, width: 0, height: 0 }])
        });
        self.multi_finger_tap(fingers, time)
    }

    fn multi_finger_tap(&mut self, fingers: Option<Vec<Touch>>, time: u64) -> Result<(), Error> {
        let num_fingers = match &fingers {
            Some(fingers_vec) => fingers_vec.len(),
            None => 0,
        };
        if num_fingers > usize::try_from(TOUCH_MAX_CONTACTS).context("usize is at least 32 bits")? {
            return Err(format_err!(
                "Got {} fingers, but max is {}",
                num_fingers,
                TOUCH_MAX_CONTACTS
            ));
        }
        self.multi_finger_tap_internal(
            TouchInputReport {
                contacts: Some(fingers.map_or_else(Vec::new, |fingers_vec| {
                    fingers_vec
                        .into_iter()
                        .map(|finger| ContactInputReport {
                            contact_id: Some(finger.finger_id),
                            position_x: Some(i64::from(finger.x)),
                            position_y: Some(i64::from(finger.y)),
                            contact_width: Some(i64::from(finger.width)),
                            contact_height: Some(i64::from(finger.height)),
                            ..Default::default()
                        })
                        .collect()
                })),
                pressed_buttons: Some(vec![]),
                ..Default::default()
            },
            time,
        )
    }

    fn mouse(&mut self, report: MouseInputReport, time: u64) -> Result<(), Error> {
        self.report_sender
            .unbounded_send(InputReport {
                event_time: Some(i64::try_from(time).context("converting time to i64")?),
                mouse: Some(report),
                ..Default::default()
            })
            .context("error sending mouse InputReport")
    }

    async fn flush(self: Box<Self>) -> Result<(), Error> {
        let Self { input_device_task, report_sender } = *self;
        std::mem::drop(report_sender); // Drop `report_sender` to close channel.
        input_device_task.await
    }
}

impl InputDevice {
    /// Creates a new `InputDevice` that will create a task to:
    /// a) process requests from `request_stream`, and
    /// b) respond to `GetDescriptor` calls with the descriptor generated by `descriptor_generator()`
    pub(super) fn new(
        request_stream: InputDeviceRequestStream,
        descriptor: DeviceDescriptor,
    ) -> Self {
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();

        // Create a `Task` to keep serving the `fuchsia.input.report.InputDevice` protocol.
        let input_device_task =
            fasync::Task::local(Self::serve_reports(request_stream, descriptor, report_receiver));

        Self { report_sender, input_device_task }
    }

    /// Returns a `Future` which resolves when all `InputReport`s for this device
    /// have been sent to a `fuchsia.input.report.InputReportsReaderV2` client, or when
    /// an error occurs.
    ///
    /// # Resolves to
    /// * `Ok(())` if all reports were written successfully
    /// * `Err` otherwise. For example:
    ///   * The `fuchsia.input.report.InputDevice` client sent an invalid request.
    ///   * A FIDL error occurred while trying to read a FIDL request.
    ///   * A FIDL error occurred while trying to write a FIDL response.
    ///
    /// # Corner cases
    /// Resolves to `Err` if the `fuchsia.input.report.InputDevice` client did not call
    /// `GetInputReportsReaderV2()`, even if no `InputReport`s were queued.
    ///
    /// # Note
    /// When the `Future` resolves, `InputReports` may still be sitting unread in the
    /// channel to the `fuchsia.input.report.InputReportsReaderV2` client. (The client will
    /// typically be an input pipeline implementation.)
    async fn serve_reports(
        request_stream: InputDeviceRequestStream,
        descriptor: DeviceDescriptor,
        report_receiver: futures::channel::mpsc::UnboundedReceiver<InputReport>,
    ) -> Result<(), Error> {
        // Process `fuchsia.input.report.InputDevice` requests, waiting for the `InputDevice`
        // client to provide a `ServerEnd<InputReportsReaderV2Marker>` by calling `GetInputReportsReaderV2()`.
        let mut input_reports_reader_server_end_stream = request_stream
            .filter_map(|r| future::ready(Self::handle_device_request(r, &descriptor)));
        let input_reports_reader_fut = {
            let (reader_server_end, max_unacknowledged_reports) =
                input_reports_reader_server_end_stream
                    .next()
                    .await
                    .ok_or_else(|| {
                        format_err!("stream ended without a call to GetInputReportsReaderV2")
                    })?
                    .context("handling InputDeviceRequest")?;
            InputReportsReaderV2 {
                request_stream: reader_server_end.into_stream(),
                report_receiver,
                max_unacknowledged_reports,
            }
            .into_future()
        };
        pin_mut!(input_reports_reader_fut);

        // Create a `Future` to keep serving the `fuchsia.input.report.InputDevice` protocol.
        // This time, receiving a `ServerEnd<InputReportsReaderV2Marker>` will be an `Err`.
        let input_device_server_fut = async {
            match input_reports_reader_server_end_stream.next().await {
                Some(Ok(_server_end)) => {
                    // There are no obvious "best" semantics for how to handle multiple
                    // `GetInputReportsReaderV2` calls, and there is no current need to
                    // do so. Instead of taking a guess at what the client might want
                    // in such a case, just return `Err`.
                    Err(format_err!(
                        "InputDevice does not support multiple GetInputReportsReaderV2 calls"
                    ))
                }
                Some(Err(e)) => Err(e.context("handling InputDeviceRequest")),
                None => Ok(()),
            }
        };
        pin_mut!(input_device_server_fut);

        // Now, process both `fuchsia.input.report.InputDevice` requests, and
        // `fuchsia.input.report.InputReportsReaderV2` requests. And keep processing
        // `InputReportsReaderV2` requests even if the `InputDevice` connection
        // is severed.
        future::select(
            input_device_server_fut.and_then(|_: ()| future::pending()),
            input_reports_reader_fut,
        )
        .await
        .factor_first()
        .0
    }

    /// Converts a [KeyboardReport] into a sequence of key presses, using the supplied
    /// key-to-HID usage transformation function.
    fn key_press_internal(
        &mut self,
        report: KeyboardReport,
        time: u64,
        transform: fn(r: &KeyboardReport) -> Result<Vec<Key>, Error>,
    ) -> Result<(), Error> {
        self.report_sender
            .unbounded_send(InputReport {
                event_time: Some(i64::try_from(time).context("converting time to i64")?),
                keyboard: Some(KeyboardInputReport {
                    pressed_keys3: Some(transform(&report)?),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .context("sending key press InputReport")
    }

    fn multi_finger_tap_internal(
        &mut self,
        touch: TouchInputReport,
        time: u64,
    ) -> Result<(), Error> {
        self.report_sender
            .unbounded_send(InputReport {
                event_time: Some(i64::try_from(time).context("converting time to i64")?),
                touch: Some(touch),
                ..Default::default()
            })
            .context("sending touch InputReport")
    }

    /// Processes a single request from an `InputDeviceRequestStream`
    ///
    /// # Returns
    /// * Some(Ok((ServerEnd<InputReportsReaderV2Marker>, u16))) if the request yielded an `InputReportsReaderV2`.
    /// * Some(Err) if the request yielded an `Error`
    /// * None if the request was fully processed by `handle_device_request()`
    fn handle_device_request(
        request: Result<InputDeviceRequest, FidlError>,
        descriptor: &DeviceDescriptor,
    ) -> Option<Result<(ServerEnd<InputReportsReaderV2Marker>, u16), Error>> {
        match request {
            Ok(InputDeviceRequest::GetInputReportsReaderV2 {
                reader: reader_server_end,
                max_unacknowledged_reports_limit,
                responder,
            }) => {
                let max_unacknowledged_reports = max_unacknowledged_reports_limit.max(1);
                if let Err(e) = responder.send(max_unacknowledged_reports) {
                    return Some(Err(
                        anyhow::Error::from(e).context("sending GetInputReportsReaderV2 response")
                    ));
                }
                Some(Ok((reader_server_end, max_unacknowledged_reports)))
            }
            Ok(InputDeviceRequest::GetDescriptor { responder }) => {
                match responder.send(&descriptor) {
                    Ok(()) => None,
                    Err(e) => {
                        Some(Err(anyhow::Error::from(e).context("sending GetDescriptor response")))
                    }
                }
            }
            Ok(InputDeviceRequest::GetFeatureReport { responder }) => {
                match responder.send(Ok(&FeatureReport::default())) {
                    Ok(()) => None,
                    Err(e) => Some(Err(
                        anyhow::Error::from(e).context("sending GetFeatureReport response")
                    )),
                }
            }
            Err(e) => {
                // Fail fast.
                //
                // Panic here, since we don't have a good way to report an error from a
                // background task.  InputDevice::flush() exists, but this is unlikely
                // to be called in tests, and it may get called way too late, after
                // an error in this background task already caused some other error.
                panic!("InputDevice got an error while reading request: {:?}", e);
            }
            _ => {
                // See the previous branch.
                panic!(
                    "InputDevice::handle_device_request does not support this request: {:?}",
                    request
                );
            }
        }
    }

    fn convert_keyboard_report_to_keys(report: &KeyboardReport) -> Result<Vec<Key>, Error> {
        report
            .pressed_keys
            .iter()
            .map(|&usage| {
                hid_usage_to_input3_key(usage as u16)
                    .ok_or_else(|| format_err!("no Key for usage {:?}", usage))
            })
            .collect()
    }

    /// Same as convert_keyboard_report_to_keys, but no additional calls to HID usage mapping.
    ///
    /// The keyboard report in `convert_keyboard_report_to_keys` assumes USB HID usage page 7.
    /// This set of keys is narrower than what Fuchsia supports so that function needs to map
    /// back into Fuchsia USB HID encoding (see [fidl_fuchsia_input::Key]).
    ///
    /// This function, in turn, uses the full range of [fidl_fuchsia_input::Key], so does not
    /// need this conversion.
    fn convert_keyboard_report_to_keys_no_transform(
        report: &KeyboardReport,
    ) -> Result<Vec<Key>, Error> {
        report
            .pressed_keys
            .iter()
            .map(|&usage| {
                Key::from_primitive(usage)
                    .ok_or_else(|| anyhow::anyhow!("could not convert to input::Key: {}", usage))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::synthesizer::InputDevice as _;
    use super::*;
    use fidl::endpoints;
    use fidl_fuchsia_input_report::{
        DeviceDescriptor, InputDeviceMarker, KeyboardDescriptor, KeyboardInputDescriptor,
    };
    use fuchsia_async as fasync;

    const DEFAULT_REPORT_TIMESTAMP: u64 = 0;

    mod responds_to_get_feature_report_request {
        use super::*;

        #[fasync::run_until_stalled(test)]
        async fn single_request_before_call_to_get_feature_report() -> Result<(), Error> {
            let (proxy, request_stream) = endpoints::create_proxy_and_stream::<InputDeviceMarker>();
            let input_device_server_fut =
                Box::new(InputDevice::new(request_stream, DeviceDescriptor::default())).flush();
            let get_feature_report_fut = proxy.get_feature_report();
            std::mem::drop(proxy); // Drop `proxy` to terminate `request_stream`.

            let (_, get_feature_report_result) =
                future::join(input_device_server_fut, get_feature_report_fut).await;
            assert_eq!(
                get_feature_report_result.context("fidl error")?,
                Ok(FeatureReport::default())
            );
            Ok(())
        }
    }

    mod responds_to_get_descriptor_request {
        use super::utils::{make_input_device_proxy_and_struct, make_keyboard_descriptor};
        use super::*;
        use assert_matches::assert_matches;
        use futures::task::Poll;

        #[fasync::run_until_stalled(test)]
        async fn single_request_before_call_to_get_input_reports_reader() -> Result<(), Error> {
            let (proxy, request_stream) = endpoints::create_proxy_and_stream::<InputDeviceMarker>();
            let input_device_server_fut =
                Box::new(InputDevice::new(request_stream, make_keyboard_descriptor(vec![Key::A])))
                    .flush();
            let get_descriptor_fut = proxy.get_descriptor();
            std::mem::drop(proxy); // Drop `proxy` to terminate `request_stream`.

            let (_, get_descriptor_result) =
                future::join(input_device_server_fut, get_descriptor_fut).await;
            assert_eq!(
                get_descriptor_result.context("fidl error")?,
                make_keyboard_descriptor(vec![Key::A])
            );
            Ok(())
        }

        #[test]
        fn multiple_requests_before_call_to_get_input_reports_reader() -> Result<(), Error> {
            let mut executor = fasync::TestExecutor::new();
            let (proxy, request_stream) = endpoints::create_proxy_and_stream::<InputDeviceMarker>();
            let mut input_device_server_fut =
                Box::new(InputDevice::new(request_stream, make_keyboard_descriptor(vec![Key::A])))
                    .flush();

            let mut get_descriptor_fut = proxy.get_descriptor();
            assert_matches!(
                executor.run_until_stalled(&mut input_device_server_fut),
                Poll::Pending
            );
            std::mem::drop(executor.run_until_stalled(&mut get_descriptor_fut));

            let mut get_descriptor_fut = proxy.get_descriptor();
            let _ = executor.run_until_stalled(&mut input_device_server_fut);
            assert_matches!(
                executor.run_until_stalled(&mut get_descriptor_fut),
                Poll::Ready(Ok(_))
            );

            Ok(())
        }

        #[test]
        fn after_call_to_get_input_reports_reader_with_report_pending() -> Result<(), Error> {
            let mut executor = fasync::TestExecutor::new();
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            for _ in 0..2 {
                input_device
                    .key_press(KeyboardReport { pressed_keys: vec![] }, DEFAULT_REPORT_TIMESTAMP)
                    .context("internal error queuing input event")?;
            }

            let input_device_server_fut = input_device.flush();
            pin_mut!(input_device_server_fut);

            let (_input_reports_reader_proxy, input_reports_reader_server_end) =
                endpoints::create_proxy::<InputReportsReaderV2Marker>();

            const MAX_UNACKNOWLEDGED_REPORTS_LIMIT: u16 = 1;
            let _ = input_device_proxy.get_input_reports_reader_v2(
                input_reports_reader_server_end,
                MAX_UNACKNOWLEDGED_REPORTS_LIMIT,
            );
            assert_matches!(
                executor.run_until_stalled(&mut input_device_server_fut),
                Poll::Pending
            );

            let mut get_descriptor_fut = input_device_proxy.get_descriptor();
            assert_matches!(
                executor.run_until_stalled(&mut input_device_server_fut),
                Poll::Pending
            );
            assert_matches!(executor.run_until_stalled(&mut get_descriptor_fut), Poll::Ready(_));
            Ok(())
        }
    }

    mod report_contents {
        use super::utils::{get_input_reports, make_input_device_proxy_and_struct};
        use super::*;
        use crate::usages::Usages;
        use assert_matches::assert_matches;
        use std::convert::TryInto as _;

        #[fasync::run_until_stalled(test)]
        async fn media_buttons_generates_empty_consumer_controls_input_report() -> Result<(), Error>
        {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.media_buttons(vec![], DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    consumer_control: Some(ConsumerControlInputReport {
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn media_buttons_generates_full_consumer_controls_input_report() -> Result<(), Error>
        {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.media_buttons(
                vec![
                    synthesizer::MediaButton::VolumeUp,
                    synthesizer::MediaButton::VolumeDown,
                    synthesizer::MediaButton::MicMute,
                    synthesizer::MediaButton::FactoryReset,
                    synthesizer::MediaButton::Pause,
                    synthesizer::MediaButton::CameraDisable,
                ],
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    consumer_control: Some(ConsumerControlInputReport {
                        pressed_buttons: Some(vec![
                            ConsumerControlButton::VolumeUp,
                            ConsumerControlButton::VolumeDown,
                            ConsumerControlButton::MicMute,
                            ConsumerControlButton::FactoryReset,
                            ConsumerControlButton::Pause,
                            ConsumerControlButton::CameraDisable,
                        ]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn media_buttons_generates_partial_consumer_controls_input_report()
        -> Result<(), Error> {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.media_buttons(
                vec![
                    synthesizer::MediaButton::VolumeUp,
                    synthesizer::MediaButton::MicMute,
                    synthesizer::MediaButton::Pause,
                ],
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    consumer_control: Some(ConsumerControlInputReport {
                        pressed_buttons: Some(vec![
                            ConsumerControlButton::VolumeUp,
                            ConsumerControlButton::MicMute,
                            ConsumerControlButton::Pause,
                        ]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn key_press_generates_expected_keyboard_input_report() -> Result<(), Error> {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.key_press(
                KeyboardReport {
                    pressed_keys: vec![Usages::HidUsageKeyA as u32, Usages::HidUsageKeyB as u32],
                },
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    keyboard: Some(KeyboardInputReport {
                        pressed_keys3: Some(vec![Key::A, Key::B]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn key_press_usage_generates_expected_keyboard_input_report_for_some()
        -> Result<(), Error> {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device
                .key_press_usage(Some(Usages::HidUsageKeyA as u32), DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    keyboard: Some(KeyboardInputReport {
                        pressed_keys3: Some(vec![Key::A]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn key_press_usage_generates_expected_keyboard_input_report_for_none()
        -> Result<(), Error> {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.key_press_usage(None, DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    keyboard: Some(KeyboardInputReport {
                        pressed_keys3: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn key_press_returns_error_if_usage_cannot_be_mapped_to_key() {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            assert_matches!(
                input_device.key_press(
                    KeyboardReport { pressed_keys: vec![0xffff_ffff] },
                    DEFAULT_REPORT_TIMESTAMP
                ),
                Err(_)
            );
        }

        #[fasync::run_until_stalled(test)]
        async fn key_press_usage_returns_error_if_usage_cannot_be_mapped_to_key() {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            assert_matches!(
                input_device.key_press_usage(Some(0xffff_ffff), DEFAULT_REPORT_TIMESTAMP),
                Err(_)
            );
        }

        #[fasync::run_until_stalled(test)]
        async fn key_events_generates_expected_keyboard_response() -> Result<(), Error> {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.key_press_raw(
                KeyboardReport {
                    pressed_keys: vec![Key::A.into_primitive(), Key::B.into_primitive()],
                },
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, _input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    keyboard: Some(KeyboardInputReport {
                        pressed_keys3: Some(vec![Key::A, Key::B]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn tap_generates_expected_report_for_some() -> Result<(), Error> {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.tap(Some((10, 20)), DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, _input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    touch: Some(TouchInputReport {
                        contacts: Some(vec![ContactInputReport {
                            contact_id: Some(1),
                            position_x: Some(10),
                            position_y: Some(20),
                            pressure: None,
                            contact_width: Some(0),
                            contact_height: Some(0),
                            ..Default::default()
                        }]),
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn tap_generates_expected_report_for_none() -> Result<(), Error> {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.tap(None, DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, _input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    touch: Some(TouchInputReport {
                        contacts: Some(vec![]),
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn multi_finger_tap_generates_report_for_single_finger() -> Result<(), Error> {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.multi_finger_tap(
                Some(vec![Touch { finger_id: 5, x: 10, y: 20, width: 100, height: 200 }]),
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, _input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    touch: Some(TouchInputReport {
                        contacts: Some(vec![ContactInputReport {
                            contact_id: Some(5),
                            position_x: Some(10),
                            position_y: Some(20),
                            pressure: None,
                            contact_width: Some(100),
                            contact_height: Some(200),
                            ..Default::default()
                        }]),
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn multi_finger_tap_generates_expected_report_for_two_fingers() -> Result<(), Error> {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.multi_finger_tap(
                Some(vec![
                    Touch { finger_id: 5, x: 10, y: 20, width: 100, height: 200 },
                    Touch { finger_id: 0, x: 30, y: 40, width: 300, height: 400 },
                ]),
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, _input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    touch: Some(TouchInputReport {
                        contacts: Some(vec![
                            ContactInputReport {
                                contact_id: Some(5),
                                position_x: Some(10),
                                position_y: Some(20),
                                pressure: None,
                                contact_width: Some(100),
                                contact_height: Some(200),
                                ..Default::default()
                            },
                            ContactInputReport {
                                contact_id: Some(0),
                                position_x: Some(30),
                                position_y: Some(40),
                                pressure: None,
                                contact_width: Some(300),
                                contact_height: Some(400),
                                ..Default::default()
                            }
                        ]),
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn multi_finger_tap_generates_expected_report_for_zero_fingers() -> Result<(), Error>
        {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.multi_finger_tap(Some(vec![]), DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, _input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    touch: Some(TouchInputReport {
                        contacts: Some(vec![]),
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn multi_finger_tap_generates_expected_report_for_none() -> Result<(), Error> {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.multi_finger_tap(None, DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, _input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    touch: Some(TouchInputReport {
                        contacts: Some(vec![]),
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn multi_finger_tap_returns_error_when_num_fingers_is_to_large() {
            let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            assert_matches!(
                input_device.multi_finger_tap(
                    Some(
                        (0..=TOUCH_MAX_CONTACTS)
                            .map(|i| Touch {
                                finger_id: i,
                                x: i as i32,
                                y: i as i32,
                                width: i,
                                height: i
                            })
                            .collect(),
                    ),
                    DEFAULT_REPORT_TIMESTAMP,
                ),
                Err(_)
            );
        }

        #[fasync::run_until_stalled(test)]
        async fn mouse_generates_empty_mouse_input_report() -> Result<(), Error> {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.mouse(MouseInputReport::default(), DEFAULT_REPORT_TIMESTAMP)?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    mouse: Some(MouseInputReport::default()),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn mouse_generates_full_mouse_input_report() -> Result<(), Error> {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.mouse(
                MouseInputReport {
                    movement_x: Some(10),
                    movement_y: Some(15),
                    pressed_buttons: Some(vec![1, 2, 3]),
                    scroll_v: Some(1),
                    scroll_h: Some(-1),
                    ..Default::default()
                },
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    mouse: Some(MouseInputReport {
                        movement_x: Some(10),
                        movement_y: Some(15),
                        pressed_buttons: Some(vec![1, 2, 3]),
                        scroll_v: Some(1),
                        scroll_h: Some(-1),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }

        #[fasync::run_until_stalled(test)]
        async fn mouse_generates_partial_mouse_input_report() -> Result<(), Error> {
            let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
            input_device.mouse(
                MouseInputReport {
                    movement_x: Some(10),
                    movement_y: Some(15),
                    pressed_buttons: Some(vec![]),
                    ..Default::default()
                },
                DEFAULT_REPORT_TIMESTAMP,
            )?;

            let input_reports = get_input_reports(input_device, input_device_proxy).await;
            assert_eq!(
                input_reports.as_slice(),
                [InputReport {
                    event_time: Some(
                        DEFAULT_REPORT_TIMESTAMP.try_into().expect("converting to i64")
                    ),
                    mouse: Some(MouseInputReport {
                        movement_x: Some(10),
                        movement_y: Some(15),
                        pressed_buttons: Some(vec![]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            );
            Ok(())
        }
    }

    mod future_resolution {
        use super::utils::{make_input_device_proxy_and_struct, make_input_reports_reader_proxy};
        use super::*;
        use futures::task::Poll;

        mod yields_ok_after_all_reports_are_sent_to_input_reports_reader {
            use super::*;
            use assert_matches::assert_matches;

            #[test]
            fn if_device_request_channel_was_closed() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
                let _input_reports_reader_proxy =
                    make_input_reports_reader_proxy(&input_device_proxy);
                input_device
                    .key_press(KeyboardReport { pressed_keys: vec![] }, DEFAULT_REPORT_TIMESTAMP)
                    .expect("queuing input report");

                let mut input_device_fut = input_device.flush();
                std::mem::drop(input_device_proxy); // Close device request channel.
                assert_matches!(
                    executor.run_until_stalled(&mut input_device_fut),
                    Poll::Ready(Ok(()))
                );
            }

            #[test]
            fn even_if_device_request_channel_is_open() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
                let _input_reports_reader_proxy =
                    make_input_reports_reader_proxy(&input_device_proxy);
                input_device
                    .key_press(KeyboardReport { pressed_keys: vec![] }, DEFAULT_REPORT_TIMESTAMP)
                    .expect("queuing input report");

                let mut input_device_fut = input_device.flush();
                assert_matches!(
                    executor.run_until_stalled(&mut input_device_fut),
                    Poll::Ready(Ok(()))
                );
            }

            #[test]
            fn even_if_reports_was_empty_and_device_request_channel_is_open() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, input_device) = make_input_device_proxy_and_struct();
                let _input_reports_reader_proxy =
                    make_input_reports_reader_proxy(&input_device_proxy);
                let mut input_device_fut = input_device.flush();
                assert_matches!(
                    executor.run_until_stalled(&mut input_device_fut),
                    Poll::Ready(Ok(()))
                );
            }
        }

        mod yields_err_if_peer_closed_device_channel_without_calling_get_input_reports_reader {
            use super::*;
            use assert_matches::assert_matches;

            #[test]
            fn if_reports_were_available() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
                input_device
                    .key_press(KeyboardReport { pressed_keys: vec![] }, DEFAULT_REPORT_TIMESTAMP)
                    .expect("queuing input report");

                let mut input_device_fut = input_device.flush();
                std::mem::drop(input_device_proxy);
                assert_matches!(
                    executor.run_until_stalled(&mut input_device_fut),
                    Poll::Ready(Err(_))
                )
            }

            #[test]
            fn even_if_no_reports_were_available() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, input_device) = make_input_device_proxy_and_struct();
                let mut input_device_fut = input_device.flush();
                std::mem::drop(input_device_proxy);
                assert_matches!(
                    executor.run_until_stalled(&mut input_device_fut),
                    Poll::Ready(Err(_))
                )
            }
        }

        mod is_pending_if_peer_has_device_channel_open_and_has_not_called_get_input_reports_reader {
            use super::*;
            use assert_matches::assert_matches;

            #[test]
            fn if_reports_were_available() {
                let mut executor = fasync::TestExecutor::new();
                let (_input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
                input_device
                    .key_press(KeyboardReport { pressed_keys: vec![] }, DEFAULT_REPORT_TIMESTAMP)
                    .expect("queuing input report");

                let mut input_device_fut = input_device.flush();
                assert_matches!(executor.run_until_stalled(&mut input_device_fut), Poll::Pending)
            }

            #[test]
            fn even_if_no_reports_were_available() {
                let mut executor = fasync::TestExecutor::new();
                let (_input_device_proxy, input_device) = make_input_device_proxy_and_struct();
                let mut input_device_fut = input_device.flush();
                assert_matches!(executor.run_until_stalled(&mut input_device_fut), Poll::Pending)
            }

            #[test]
            fn even_if_get_device_descriptor_has_been_called() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, input_device) = make_input_device_proxy_and_struct();
                let mut input_device_fut = input_device.flush();
                let _get_descriptor_fut = input_device_proxy.get_descriptor();
                assert_matches!(executor.run_until_stalled(&mut input_device_fut), Poll::Pending)
            }
        }

        mod is_pending_if_unacknowledged_reports_reach_limit {
            use super::utils::make_input_reports_reader_proxy_with_limit;
            use super::*;
            use assert_matches::assert_matches;

            #[test]
            fn if_device_request_channel_is_open() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
                let _input_reports_reader_proxy =
                    make_input_reports_reader_proxy_with_limit(&input_device_proxy, 1);
                for _ in 0..2 {
                    input_device
                        .key_press(
                            KeyboardReport { pressed_keys: vec![] },
                            DEFAULT_REPORT_TIMESTAMP,
                        )
                        .expect("queuing input report");
                }

                let mut input_device_fut = input_device.flush();
                assert_matches!(executor.run_until_stalled(&mut input_device_fut), Poll::Pending)
            }

            #[test]
            fn even_if_device_request_channel_is_closed() {
                let mut executor = fasync::TestExecutor::new();
                let (input_device_proxy, mut input_device) = make_input_device_proxy_and_struct();
                let _input_reports_reader_proxy =
                    make_input_reports_reader_proxy_with_limit(&input_device_proxy, 1);
                for _ in 0..2 {
                    input_device
                        .key_press(
                            KeyboardReport { pressed_keys: vec![] },
                            DEFAULT_REPORT_TIMESTAMP,
                        )
                        .expect("queuing input report");
                }

                let mut input_device_fut = input_device.flush();
                std::mem::drop(input_device_proxy); // Terminate `InputDeviceRequestStream`.
                assert_matches!(executor.run_until_stalled(&mut input_device_fut), Poll::Pending)
            }
        }
    }

    // Because `input_synthesis` is a library, unsupported use cases should yield `Error`s,
    // rather than panic!()-ing.
    mod unsupported_use_cases {
        use super::utils::make_input_device_proxy_and_struct;
        use super::*;
        use assert_matches::assert_matches;

        #[fasync::run_until_stalled(test)]
        async fn multiple_get_input_reports_reader_requests_yield_error() -> Result<(), Error> {
            let (input_device_proxy, input_device) = make_input_device_proxy_and_struct();

            let (_input_reports_reader_proxy, input_reports_reader_server_end) =
                endpoints::create_proxy::<InputReportsReaderV2Marker>();
            let _ = input_device_proxy
                .get_input_reports_reader_v2(input_reports_reader_server_end, 120);

            let (_input_reports_reader_proxy, input_reports_reader_server_end) =
                endpoints::create_proxy::<InputReportsReaderV2Marker>();
            let _ = input_device_proxy
                .get_input_reports_reader_v2(input_reports_reader_server_end, 120);

            let input_device_fut = input_device.flush();
            assert_matches!(input_device_fut.await, Err(_));
            Ok(())
        }
    }

    mod utils {
        use super::*;
        use fidl_fuchsia_input_report::{
            InputDeviceProxy, InputReportsReaderV2Event, InputReportsReaderV2Proxy,
        };

        /// Creates a `DeviceDescriptor` for a keyboard which has the keys enumerated
        /// in `keys`.
        pub(super) fn make_keyboard_descriptor(keys: Vec<Key>) -> DeviceDescriptor {
            DeviceDescriptor {
                keyboard: Some(KeyboardDescriptor {
                    input: Some(KeyboardInputDescriptor {
                        keys3: Some(keys),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }
        }

        /// Creates an `InputDeviceProxy`, for sending `fuchsia.input.report.InputDevice`
        /// requests, and an `InputDevice` struct that will receive the FIDL requests
        /// from the `InputDeviceProxy`.
        ///
        /// # Returns
        /// A tuple of the proxy and struct. The struct is `Box`-ed so that the caller
        /// can easily invoke `flush()`.
        pub(super) fn make_input_device_proxy_and_struct() -> (InputDeviceProxy, Box<InputDevice>) {
            let (input_device_proxy, input_device_request_stream) =
                endpoints::create_proxy_and_stream::<InputDeviceMarker>();
            let input_device = Box::new(InputDevice::new(
                input_device_request_stream,
                DeviceDescriptor::default(),
            ));
            (input_device_proxy, input_device)
        }

        /// Creates an `InputReportsReaderV2Proxy`, for sending
        /// `fuchsia.input.report.InputReportsReaderV2` requests, and registers that
        /// `InputReportsReaderV2` with the `InputDevice` bound to `InputDeviceProxy`.
        ///
        /// # Returns
        /// The newly created `InputReportsReaderV2Proxy`.
        pub(super) fn make_input_reports_reader_proxy(
            input_device_proxy: &InputDeviceProxy,
        ) -> InputReportsReaderV2Proxy {
            make_input_reports_reader_proxy_with_limit(input_device_proxy, 120)
        }

        pub(super) fn make_input_reports_reader_proxy_with_limit(
            input_device_proxy: &InputDeviceProxy,
            limit: u16,
        ) -> InputReportsReaderV2Proxy {
            let (input_reports_reader_proxy, input_reports_reader_server_end) =
                endpoints::create_proxy::<InputReportsReaderV2Marker>();
            let _ = input_device_proxy
                .get_input_reports_reader_v2(input_reports_reader_server_end, limit);
            input_reports_reader_proxy
        }

        /// Serves `fuchsia.input.report.InputDevice` and `fuchsia.input.report.InputReportsReaderV2`
        /// protocols using `input_device`, and reads `InputReport`s with `InputReportsReaderV2EventStream`.
        /// Then drops the connections to `fuchsia.input.report.InputDevice` and `fuchsia.input.report.InputReportsReaderV2`.
        ///
        /// # Returns
        /// The reports provided by the `InputDevice`.
        pub(super) async fn get_input_reports(
            input_device: Box<InputDevice>,
            input_device_proxy: InputDeviceProxy,
        ) -> Vec<InputReport> {
            let input_reports_reader_proxy = make_input_reports_reader_proxy(&input_device_proxy);
            let mut event_stream = input_reports_reader_proxy.take_event_stream();
            let input_device_server_fut = input_device.flush();
            let read_reports_fut = async {
                let mut reports = Vec::new();
                while let Some(Ok(InputReportsReaderV2Event::OnInputReports {
                    reports: batch,
                    last_report_stamp,
                })) = event_stream.next().await
                {
                    let _ = input_reports_reader_proxy.acknowledge_reports(last_report_stamp);
                    reports.extend(batch);
                }
                reports
            };
            std::mem::drop(input_device_proxy); // Terminate `input_device_request_stream`.
            let (flush_res, reports) =
                future::join(input_device_server_fut, read_reports_fut).await;
            flush_res.expect("flush error");
            reports
        }
    }
}
