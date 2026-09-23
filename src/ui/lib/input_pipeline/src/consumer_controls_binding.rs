// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device::{self, Handled, InputDeviceStatus, InputEvent};
use crate::{Transport, metrics, utils};
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fidl_fuchsia_input::ConsumerControlButton;
use fuchsia_inspect::ArrayProperty;
use fuchsia_inspect::health::Reporter;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use metrics_registry::*;

/// A [`ConsumerControlsEvent`] represents an event where one or more consumer control buttons
/// were pressed.
///
/// # Example
/// The following ConsumerControlsEvents represents an event where the volume up button was pressed.
///
/// ```
/// let volume_event = input_device::InputDeviceEvent::ConsumerControls(ConsumerControlsEvent::new(
///     vec![ConsumerControlButton::VOLUME_UP],
/// ));
/// ```
#[derive(Debug)]
pub struct ConsumerControlsEvent {
    pub pressed_buttons: Vec<ConsumerControlButton>,
    pub wake_lease: Option<zx::EventPair>,
}

impl Clone for ConsumerControlsEvent {
    fn clone(&self) -> Self {
        log::debug!("ConsumerControlsEvent cloned without wake lease.");
        Self { pressed_buttons: self.pressed_buttons.clone(), wake_lease: None }
    }
}

impl PartialEq for ConsumerControlsEvent {
    fn eq(&self, other: &Self) -> bool {
        self.pressed_buttons == other.pressed_buttons
            && self.wake_lease.as_ref().map(|h| h.koid())
                == other.wake_lease.as_ref().map(|h| h.koid())
    }
}

impl Drop for ConsumerControlsEvent {
    fn drop(&mut self) {
        log::debug!("ConsumerControlsEvent dropped, had_wake_lease: {:?}", self.wake_lease);
    }
}

impl ConsumerControlsEvent {
    /// Creates a new [`ConsumerControlsEvent`] with the relevant buttons.
    ///
    /// # Parameters
    /// - `pressed_buttons`: The buttons relevant to this event.
    pub fn new(
        pressed_buttons: Vec<ConsumerControlButton>,
        wake_lease: Option<zx::EventPair>,
    ) -> Self {
        Self { pressed_buttons, wake_lease }
    }

    pub fn record_inspect(&self, node: &fuchsia_inspect::Node) {
        let pressed_buttons_node =
            node.create_string_array("pressed_buttons", self.pressed_buttons.len());
        self.pressed_buttons.iter().enumerate().for_each(|(i, button)| {
            let button_name: String = match button {
                ConsumerControlButton::VolumeUp => "volume_up".into(),
                ConsumerControlButton::VolumeDown => "volume_down".into(),
                ConsumerControlButton::Pause => "pause".into(),
                ConsumerControlButton::FactoryReset => "factory_reset".into(),
                ConsumerControlButton::MicMute => "mic_mute".into(),
                ConsumerControlButton::Reboot => "reboot".into(),
                ConsumerControlButton::CameraDisable => "camera_disable".into(),
                ConsumerControlButton::Power => "power".into(),
                ConsumerControlButton::Function => "function".into(),
                unknown_value => {
                    format!("unknown({:?})", unknown_value)
                }
            };
            pressed_buttons_node.set(i, &button_name);
        });
        node.record(pressed_buttons_node);
    }
}

/// A [`ConsumerControlsBinding`] represents a connection to a consumer controls input device with
/// consumer controls. The buttons supported by this binding is returned by `supported_buttons()`.
///
/// The [`ConsumerControlsBinding`] parses and exposes consumer control descriptor properties
/// for the device it is associated with. It also parses [`InputReport`]s
/// from the device, and sends them to the device binding owner over `event_sender`.
pub struct ConsumerControlsBinding {
    /// The channel to stream InputEvents to.
    event_sender: UnboundedSender<Vec<InputEvent>>,

    /// Holds information about this device.
    device_descriptor: ConsumerControlsDeviceDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsumerControlsDeviceDescriptor {
    /// The list of buttons that this device contains.
    pub buttons: Vec<ConsumerControlButton>,
    /// Identifies the device originating this event.
    pub device_id: u32,
    /// True if the device was injected (e.g. by input_device_registry).
    pub is_injected: bool,
}

#[async_trait]
impl input_device::InputDeviceBinding for ConsumerControlsBinding {
    fn input_event_sender(&self) -> UnboundedSender<Vec<InputEvent>> {
        self.event_sender.clone()
    }

    fn get_device_descriptor(&self) -> input_device::InputDeviceDescriptor {
        input_device::InputDeviceDescriptor::ConsumerControls(self.device_descriptor.clone())
    }
}

impl ConsumerControlsBinding {
    /// Creates a new [`InputDeviceBinding`] from the `device_proxy`.
    ///
    /// The binding will start listening for input reports immediately and send new InputEvents
    /// to the device binding owner over `input_event_sender`.
    ///
    /// # Parameters
    /// - `device_proxy`: The proxy to bind the new [`InputDeviceBinding`] to.
    /// - `device_id`: The id of the connected device.
    /// - `input_event_sender`: The channel to send new InputEvents to.
    /// - `device_node`: The inspect node for this device binding
    /// - `metrics_logger`: The metrics logger.
    ///
    /// # Errors
    /// If there was an error binding to the proxy.
    pub async fn new(
        device_proxy: fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
        device_id: u32,
        input_event_sender: UnboundedSender<Vec<InputEvent>>,
        device_node: fuchsia_inspect::Node,
        feature_flags: input_device::InputPipelineFeatureFlags,
        metrics_logger: metrics::MetricsLogger,
        is_injected: bool,
    ) -> Result<(Self, crate::dispatcher::TaskHandle<()>), Error> {
        let (device_descriptor, mut inspect_status) =
            Self::bind_device(&device_proxy, device_id, device_node, is_injected).await?;
        inspect_status.health_node.set_ok();
        let task = input_device::initialize_report_stream(
            device_proxy,
            input_device::InputDeviceDescriptor::ConsumerControls(device_descriptor.clone()),
            input_event_sender.clone(),
            inspect_status,
            metrics_logger,
            feature_flags,
            Self::process_reports,
        );

        Ok((ConsumerControlsBinding { event_sender: input_event_sender, device_descriptor }, task))
    }

    /// Binds the provided input device to a new instance of `Self`.
    ///
    /// # Parameters
    /// - `device`: The device to use to initialize the binding.
    /// - `device_id`: The id of the connected device.
    /// - `device_node`: The inspect node for this device binding
    ///
    /// # Errors
    /// If the device descriptor could not be retrieved, or the descriptor could
    /// not be parsed correctly.
    async fn bind_device(
        device: &fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
        device_id: u32,
        device_node: fuchsia_inspect::Node,
        is_injected: bool,
    ) -> Result<(ConsumerControlsDeviceDescriptor, InputDeviceStatus), Error> {
        let mut input_device_status = InputDeviceStatus::new(device_node);
        let device_descriptor: fidl_next_fuchsia_input_report::DeviceDescriptor = match device
            .get_descriptor()
            .await
        {
            Ok(descriptor) => descriptor.descriptor,
            Err(_) => {
                input_device_status.health_node.set_unhealthy("Could not get device descriptor.");
                return Err(format_err!("Could not get descriptor for device_id: {}", device_id));
            }
        };

        let consumer_controls_descriptor = device_descriptor.consumer_control.ok_or_else(|| {
            input_device_status
                .health_node
                .set_unhealthy("DeviceDescriptor does not have a ConsumerControlDescriptor.");
            format_err!("DeviceDescriptor does not have a ConsumerControlDescriptor")
        })?;

        let consumer_controls_input_descriptor =
            consumer_controls_descriptor.input.ok_or_else(|| {
                input_device_status.health_node.set_unhealthy(
                    "ConsumerControlDescriptor does not have a ConsumerControlInputDescriptor.",
                );
                format_err!(
                    "ConsumerControlDescriptor does not have a ConsumerControlInputDescriptor"
                )
            })?;

        let device_descriptor: ConsumerControlsDeviceDescriptor =
            ConsumerControlsDeviceDescriptor {
                buttons: consumer_controls_input_descriptor
                    .buttons
                    .unwrap_or_default()
                    .into_iter()
                    .map(|b| utils::consumer_control_button_to_old(&b))
                    .collect(),
                device_id,
                is_injected,
            };

        Ok((device_descriptor, input_device_status))
    }

    /// Parses an [`InputReport`] into one or more [`InputEvent`]s. Sends the [`InputEvent`]s
    /// to the device binding owner via [`input_event_sender`].
    ///
    /// # Parameters
    /// `reports`: The incoming [`InputReport`].
    /// `previous_report`: The previous [`InputReport`] seen for the same device. This can be
    ///                    used to determine, for example, which keys are no longer present in
    ///                    a keyboard report to generate key released events. If `None`, no
    ///                    previous report was found.
    /// `device_descriptor`: The descriptor for the input device generating the input reports.
    /// `input_event_sender`: The sender for the device binding's input event stream.
    /// `metrics_logger`: The metrics logger.
    ///
    ///
    /// # Returns
    /// An [`InputReport`] which will be passed to the next call to [`process_reports`], as
    /// [`previous_report`]. If `None`, the next call's [`previous_report`] will be `None`.
    /// A [`UnboundedReceiver<InputEvent>`] which will poll asynchronously generated events to be
    /// recorded by `inspect_status` in `input_device::initialize_report_stream()`. If device
    /// binding does not generate InputEvents asynchronously, this will be `None`.
    ///
    /// The returned [`InputReport`] is guaranteed to have no `wake_lease`.
    fn process_reports(
        reports: &[fidl_next_fuchsia_input_report::wire::InputReport<'_>],
        mut previous_state: Option<input_device::PreviousDeviceState>,
        device_descriptor: &input_device::InputDeviceDescriptor,
        input_event_sender: &mut UnboundedSender<Vec<InputEvent>>,
        inspect_status: &InputDeviceStatus,
        metrics_logger: &metrics::MetricsLogger,
        _feature_flags: &input_device::InputPipelineFeatureFlags,
    ) -> (Option<input_device::PreviousDeviceState>, Option<UnboundedReceiver<InputEvent>>) {
        fuchsia_trace::duration!("input", "consumer-controls-binding-process-report", "num_reports" => reports.len());
        for report in reports {
            previous_state = Self::process_report(
                report,
                previous_state,
                device_descriptor,
                input_event_sender,
                inspect_status,
                metrics_logger,
            );
        }
        (previous_state, None)
    }

    fn process_report(
        report: &fidl_next_fuchsia_input_report::wire::InputReport<'_>,
        previous_state: Option<input_device::PreviousDeviceState>,
        device_descriptor: &input_device::InputDeviceDescriptor,
        input_event_sender: &mut UnboundedSender<Vec<InputEvent>>,
        inspect_status: &InputDeviceStatus,
        metrics_logger: &metrics::MetricsLogger,
    ) -> Option<input_device::PreviousDeviceState> {
        if let Some(trace_id) = report.trace_id() {
            fuchsia_trace::flow_end!("input", "input_report", trace_id.0.into());
        }

        // Extract the wake_lease early to prevent it from leaking. If this is moved
        // below an early return, the lease could accidentally be stored inside
        // `previous_report`, which would prevent the system from suspending.
        let wake_lease = utils::duplicate_wake_lease(report.wake_lease());

        inspect_status.count_received_report_wire(report);
        // Input devices can have multiple types so ensure `report` is a ConsumerControlInputReport.
        let pressed_buttons: Vec<ConsumerControlButton> = match report.consumer_control() {
            Some(ref consumer_control_report) => consumer_control_report
                .pressed_buttons()
                .map(|buttons| {
                    buttons
                        .iter()
                        .map(|&b| {
                            let natural_button = fidl_next::FromWire::from_wire(b);
                            utils::consumer_control_button_to_old(&natural_button)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            None => {
                inspect_status.count_filtered_report();
                return previous_state;
            }
        };

        let trace_id = fuchsia_trace::Id::new();
        fuchsia_trace::flow_begin!("input", "event_in_input_pipeline", trace_id);

        send_consumer_controls_event(
            pressed_buttons.clone(),
            wake_lease,
            device_descriptor,
            input_event_sender,
            inspect_status,
            metrics_logger,
            trace_id,
        );

        Some(input_device::PreviousDeviceState::ConsumerControls { pressed_buttons })
    }
}

/// Sends an InputEvent over `sender`.
///
/// # Parameters
/// - `pressed_buttons`: The buttons relevant to the event.
/// - `wake_lease`: The wake lease associated with the event.
/// - `device_descriptor`: The descriptor for the input device generating the input reports.
/// - `sender`: The stream to send the InputEvent to.
/// - `metrics_logger`: The metrics logger.
/// - `trace_id`: The trace_id of this button event.
fn send_consumer_controls_event(
    pressed_buttons: Vec<ConsumerControlButton>,
    wake_lease: Option<zx::EventPair>,
    device_descriptor: &input_device::InputDeviceDescriptor,
    sender: &mut UnboundedSender<Vec<input_device::InputEvent>>,
    inspect_status: &InputDeviceStatus,
    metrics_logger: &metrics::MetricsLogger,
    trace_id: fuchsia_trace::Id,
) {
    let event = input_device::InputEvent {
        device_event: input_device::InputDeviceEvent::ConsumerControls(ConsumerControlsEvent::new(
            pressed_buttons,
            wake_lease,
        )),
        device_descriptor: device_descriptor.clone(),
        event_time: zx::MonotonicInstant::get(),
        handled: Handled::No,
        trace_id: Some(trace_id),
    };
    let events = vec![event];
    inspect_status.count_generated_events(&events);

    if let Err(e) = sender.unbounded_send(events) {
        metrics_logger.log_error(
            InputPipelineErrorMetricDimensionEvent::ConsumerControlsSendEventFailed,
            std::format!("Failed to send ConsumerControlsEvent with error: {:?}", e),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing_utilities;
    use futures::StreamExt;

    // Tests that an InputReport containing one consumer control button generates an InputEvent
    // containing the same consumer control button.
    #[fuchsia::test]
    async fn volume_up_only() {
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let pressed_buttons = vec![ConsumerControlButton::VolumeUp];
        let first_report = testing_utilities::create_consumer_control_input_report(
            pressed_buttons.clone(),
            event_time_i64,
        );
        let descriptor = testing_utilities::consumer_controls_device_descriptor();

        let input_reports = vec![first_report];
        let expected_events = vec![testing_utilities::create_consumer_controls_event(
            pressed_buttons,
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: ConsumerControlsBinding,
        );
    }

    // Tests that an InputReport containing two consumer control buttons generates an InputEvent
    // containing both consumer control buttons.
    #[fuchsia::test]
    async fn volume_up_and_down() {
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let pressed_buttons =
            vec![ConsumerControlButton::VolumeUp, ConsumerControlButton::VolumeDown];
        let first_report = testing_utilities::create_consumer_control_input_report(
            pressed_buttons.clone(),
            event_time_i64,
        );
        let descriptor = testing_utilities::consumer_controls_device_descriptor();

        let input_reports = vec![first_report];
        let expected_events = vec![testing_utilities::create_consumer_controls_event(
            pressed_buttons,
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: ConsumerControlsBinding,
        );
    }

    // Tests that three InputReports containing one consumer control button generates three
    // InputEvents containing the same consumer control button.
    #[fuchsia::test]
    async fn sequence_of_buttons() {
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_consumer_control_input_report(
            vec![ConsumerControlButton::VolumeUp],
            event_time_i64,
        );
        let second_report = testing_utilities::create_consumer_control_input_report(
            vec![ConsumerControlButton::VolumeDown],
            event_time_i64,
        );
        let third_report = testing_utilities::create_consumer_control_input_report(
            vec![ConsumerControlButton::CameraDisable],
            event_time_i64,
        );
        let descriptor = testing_utilities::consumer_controls_device_descriptor();

        let input_reports = vec![first_report, second_report, third_report];
        let expected_events = vec![
            testing_utilities::create_consumer_controls_event(
                vec![ConsumerControlButton::VolumeUp],
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_consumer_controls_event(
                vec![ConsumerControlButton::VolumeDown],
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_consumer_controls_event(
                vec![ConsumerControlButton::CameraDisable],
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: ConsumerControlsBinding,
        );
    }
}
