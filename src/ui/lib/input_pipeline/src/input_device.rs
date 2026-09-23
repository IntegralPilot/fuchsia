// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    Dispatcher, Transport, consumer_controls_binding, keyboard_binding, light_sensor_binding,
    metrics, mouse_binding, touch_binding,
};
use anyhow::Error;
use async_trait::async_trait;
use fidl_next_fuchsia_input_report::InputDevice;
use fuchsia_inspect::health::Reporter;
use fuchsia_inspect::{
    ExponentialHistogramParams, HistogramProperty as _, NumericProperty, Property,
};
use fuchsia_trace as ftrace;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::stream::StreamExt;
use metrics_registry::*;
use sorted_vec_map::SortedVecSet;
use strum_macros::{Display, EnumCount};

pub use input_device_constants::InputDeviceType;

#[derive(Debug, Clone, Default)]
pub struct InputPipelineFeatureFlags {
    /// Merge touch events in same InputReport frame if they are same contact and movement only.
    pub enable_merge_touch_events: bool,
}

const LATENCY_HISTOGRAM_PROPERTIES: ExponentialHistogramParams<i64> = ExponentialHistogramParams {
    floor: 0,
    initial_step: 1,
    step_multiplier: 10,
    // Seven buckets allows us to report
    // *      < 0 msec (added automatically by Inspect)
    // *      0-1 msec
    // *     1-10 msec
    // *   10-100 msec
    // * 100-1000 msec
    // *     1-10 sec
    // *   10-100 sec
    // * 100-1000 sec
    // *    >1000 sec (added automatically by Inspect)
    buckets: 7,
};

/// An [`InputDeviceStatus`] is tied to an [`InputDeviceBinding`] and provides properties
/// detailing its Inspect status.
pub struct InputDeviceStatus {
    /// Function for getting the current timestamp. Enables unit testing
    /// of the latency histogram.
    now: Box<dyn Fn() -> zx::MonotonicInstant>,

    /// A node that contains the state below.
    _node: fuchsia_inspect::Node,

    /// The total number of reports received by the device driver.
    reports_received_count: fuchsia_inspect::UintProperty,

    /// The number of reports received by the device driver that did
    /// not get converted into InputEvents processed by InputPipeline.
    reports_filtered_count: fuchsia_inspect::UintProperty,

    /// The total number of events generated from received
    /// InputReports that were sent to InputPipeline.
    events_generated: fuchsia_inspect::UintProperty,

    /// The event time the last received InputReport was generated.
    last_received_timestamp_ns: fuchsia_inspect::UintProperty,

    /// The event time the last InputEvent was generated.
    last_generated_timestamp_ns: fuchsia_inspect::UintProperty,

    // This node records the health status of the `InputDevice`.
    pub health_node: fuchsia_inspect::health::Node,

    /// Histogram of latency from the driver timestamp for an `InputReport` until
    /// the time at which the report was seen by the respective binding. Reported
    /// in milliseconds, because values less than 1 msec aren't especially
    /// interesting.
    driver_to_binding_latency_ms: fuchsia_inspect::IntExponentialHistogramProperty,

    /// The number of times a wake lease was leaked by this device.
    wake_lease_leak_count: fuchsia_inspect::UintProperty,
}

impl InputDeviceStatus {
    pub fn new(device_node: fuchsia_inspect::Node) -> Self {
        Self::new_internal(device_node, Box::new(zx::MonotonicInstant::get))
    }

    fn new_internal(
        device_node: fuchsia_inspect::Node,
        now: Box<dyn Fn() -> zx::MonotonicInstant>,
    ) -> Self {
        let mut health_node = fuchsia_inspect::health::Node::new(&device_node);
        health_node.set_starting_up();

        let reports_received_count = device_node.create_uint("reports_received_count", 0);
        let reports_filtered_count = device_node.create_uint("reports_filtered_count", 0);
        let events_generated = device_node.create_uint("events_generated", 0);
        let last_received_timestamp_ns = device_node.create_uint("last_received_timestamp_ns", 0);
        let last_generated_timestamp_ns = device_node.create_uint("last_generated_timestamp_ns", 0);
        let driver_to_binding_latency_ms = device_node.create_int_exponential_histogram(
            "driver_to_binding_latency_ms",
            LATENCY_HISTOGRAM_PROPERTIES,
        );
        let wake_lease_leak_count = device_node.create_uint("wake_lease_leak_count", 0);

        Self {
            now,
            _node: device_node,
            reports_received_count,
            reports_filtered_count,
            events_generated,
            last_received_timestamp_ns,
            last_generated_timestamp_ns,
            health_node,
            driver_to_binding_latency_ms,
            wake_lease_leak_count,
        }
    }

    pub fn count_received_report_wire(
        &self,
        report: &fidl_next_fuchsia_input_report::wire::InputReport<'_>,
    ) {
        self.reports_received_count.add(1);
        match report.event_time() {
            Some(event_time) => {
                self.driver_to_binding_latency_ms.insert(
                    ((self.now)() - zx::MonotonicInstant::from_nanos(event_time.0)).into_millis(),
                );
                self.last_received_timestamp_ns.set(event_time.0.try_into().unwrap());
            }
            None => (),
        }
    }

    pub fn count_filtered_report(&self) {
        self.reports_filtered_count.add(1);
    }

    pub fn count_generated_event(&self, event: InputEvent) {
        self.events_generated.add(1);
        self.last_generated_timestamp_ns.set(event.event_time.into_nanos().try_into().unwrap());
    }

    pub fn count_generated_events(&self, events: &Vec<InputEvent>) {
        self.events_generated.add(events.len() as u64);
        if let Some(last_event) = events.last() {
            self.last_generated_timestamp_ns
                .set(last_event.event_time.into_nanos().try_into().unwrap());
        }
    }

    pub fn count_wake_lease_leak(&self) {
        self.wake_lease_leak_count.add(1);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PreviousDeviceState {
    Keyboard {
        pressed_keys: Vec<fidl_fuchsia_input::Key>,
    },
    Mouse {
        pressed_buttons: SortedVecSet<mouse_binding::MouseButton>,
    },
    TouchScreen {
        active_contacts: Vec<touch_binding::TouchContact>,
        pressed_buttons: Vec<fidl_next_fuchsia_input_report::TouchButton>,
    },
    ConsumerControls {
        pressed_buttons: Vec<fidl_fuchsia_input::ConsumerControlButton>,
    },
    LightSensor,
    #[cfg(test)]
    Fake,
}

/// An [`InputEvent`] holds information about an input event and the device that produced the event.
#[derive(Clone, Debug, PartialEq)]
pub struct InputEvent {
    /// The `device_event` contains the device-specific input event information.
    pub device_event: InputDeviceEvent,

    /// The `device_descriptor` contains static information about the device that generated the
    /// input event.
    pub device_descriptor: InputDeviceDescriptor,

    /// The time in nanoseconds when the event was first recorded.
    pub event_time: zx::MonotonicInstant,

    /// The handled state of the event.
    pub handled: Handled,

    pub trace_id: Option<ftrace::Id>,
}

/// An [`UnhandledInputEvent`] is like an [`InputEvent`], except that the data represents an
/// event that has not been handled.
/// * Event producers must not use this type to carry data for an event that was already
///   handled.
/// * Event consumers should assume that the event has not been handled.
#[derive(Clone, Debug, PartialEq)]
pub struct UnhandledInputEvent {
    /// The `device_event` contains the device-specific input event information.
    pub device_event: InputDeviceEvent,

    /// The `device_descriptor` contains static information about the device that generated the
    /// input event.
    pub device_descriptor: InputDeviceDescriptor,

    /// The time in nanoseconds when the event was first recorded.
    pub event_time: zx::MonotonicInstant,

    pub trace_id: Option<ftrace::Id>,
}

impl UnhandledInputEvent {
    // Returns event type as string.
    pub fn get_event_type(&self) -> &'static str {
        match self.device_event {
            InputDeviceEvent::Keyboard(_) => "keyboard_event",
            InputDeviceEvent::LightSensor(_) => "light_sensor_event",
            InputDeviceEvent::ConsumerControls(_) => "consumer_controls_event",
            InputDeviceEvent::Mouse(_) => "mouse_event",
            InputDeviceEvent::TouchScreen(_) => "touch_screen_event",
            InputDeviceEvent::Touchpad(_) => "touchpad_event",
            #[cfg(test)]
            InputDeviceEvent::Fake => "fake_event",
        }
    }
}

/// An [`InputDeviceEvent`] represents an input event from an input device.
///
/// [`InputDeviceEvent`]s contain more context than the raw [`InputReport`] they are parsed from.
/// For example, [`KeyboardEvent`] contains all the pressed keys, as well as the key's
/// phase (pressed, released, etc.).
///
/// Each [`InputDeviceBinding`] generates the type of [`InputDeviceEvent`]s that are appropriate
/// for their device.
#[derive(Clone, Debug, PartialEq)]
pub enum InputDeviceEvent {
    Keyboard(keyboard_binding::KeyboardEvent),
    LightSensor(light_sensor_binding::LightSensorEvent),
    ConsumerControls(consumer_controls_binding::ConsumerControlsEvent),
    Mouse(mouse_binding::MouseEvent),
    TouchScreen(touch_binding::TouchScreenEvent),
    Touchpad(touch_binding::TouchpadEvent),
    #[cfg(test)]
    Fake,
}

/// An [`InputEventType`] represents the type of an input event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, EnumCount, Display)]
#[strum(serialize_all = "snake_case")]
pub enum InputEventType {
    Keyboard = 0,
    LightSensor = 1,
    ConsumerControls = 2,
    Mouse = 3,
    TouchScreen = 4,
    Touchpad = 5,
    #[cfg(test)]
    Fake = 6,
}

impl From<&InputDeviceEvent> for InputEventType {
    fn from(event: &InputDeviceEvent) -> Self {
        match event {
            InputDeviceEvent::Keyboard(_) => InputEventType::Keyboard,
            InputDeviceEvent::LightSensor(_) => InputEventType::LightSensor,
            InputDeviceEvent::ConsumerControls(_) => InputEventType::ConsumerControls,
            InputDeviceEvent::Mouse(_) => InputEventType::Mouse,
            InputDeviceEvent::TouchScreen(_) => InputEventType::TouchScreen,
            InputDeviceEvent::Touchpad(_) => InputEventType::Touchpad,
            #[cfg(test)]
            InputDeviceEvent::Fake => InputEventType::Fake,
        }
    }
}

/// An [`InputDescriptor`] describes the ranges of values a particular input device can generate.
///
/// For example, a [`InputDescriptor::Keyboard`] contains the keys available on the keyboard,
/// and a [`InputDescriptor::Touch`] contains the maximum number of touch contacts and the
/// range of x- and y-values each contact can take on.
///
/// The descriptor is sent alongside [`InputDeviceEvent`]s so clients can, for example, convert a
/// touch coordinate to a display coordinate. The descriptor is not expected to change for the
/// lifetime of a device binding.
#[derive(Clone, Debug, PartialEq)]
pub enum InputDeviceDescriptor {
    Keyboard(keyboard_binding::KeyboardDeviceDescriptor),
    LightSensor(light_sensor_binding::LightSensorDeviceDescriptor),
    ConsumerControls(consumer_controls_binding::ConsumerControlsDeviceDescriptor),
    Mouse(mouse_binding::MouseDeviceDescriptor),
    TouchScreen(touch_binding::TouchScreenDeviceDescriptor),
    Touchpad(touch_binding::TouchpadDeviceDescriptor),
    #[cfg(test)]
    Fake,
}

impl From<keyboard_binding::KeyboardDeviceDescriptor> for InputDeviceDescriptor {
    fn from(b: keyboard_binding::KeyboardDeviceDescriptor) -> Self {
        InputDeviceDescriptor::Keyboard(b)
    }
}

impl InputDeviceDescriptor {
    pub fn device_id(&self) -> u32 {
        match self {
            InputDeviceDescriptor::Keyboard(b) => b.device_id,
            InputDeviceDescriptor::LightSensor(b) => b.device_id,
            InputDeviceDescriptor::ConsumerControls(b) => b.device_id,
            InputDeviceDescriptor::Mouse(b) => b.device_id,
            InputDeviceDescriptor::TouchScreen(b) => b.device_id,
            InputDeviceDescriptor::Touchpad(b) => b.device_id,
            #[cfg(test)]
            InputDeviceDescriptor::Fake => 0,
        }
    }
}

// Whether the event is consumed by an [`InputHandler`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Handled {
    // The event has been handled.
    Yes,
    // The event has not been handled.
    No,
}

/// An [`InputDeviceBinding`] represents a binding to an input device (e.g., a mouse).
///
/// [`InputDeviceBinding`]s expose information about the bound device. For example, a
/// [`MouseBinding`] exposes the ranges of possible x and y values the device can generate.
///
/// An [`InputPipeline`] manages [`InputDeviceBinding`]s and holds the receiving end of a channel
/// that an [`InputDeviceBinding`]s send [`InputEvent`]s over.
/// ```
#[async_trait]
pub trait InputDeviceBinding: Send {
    /// Returns information about the input device.
    fn get_device_descriptor(&self) -> InputDeviceDescriptor;

    /// Returns the input event stream's sender.
    fn input_event_sender(&self) -> UnboundedSender<Vec<InputEvent>>;
}

/// Initializes the input report stream for the device bound to `device_proxy`.
///
/// Spawns a future which awaits input reports from the device and forwards them to
/// clients via `event_sender`.
///
/// # Parameters
/// - `device_proxy`: The device proxy which is used to get input reports.
/// - `device_descriptor`: The descriptor of the device bound to `device_proxy`.
/// - `event_sender`: The channel to send InputEvents to.
/// - `metrics_logger`: The metrics logger.
/// - `process_reports`: A function that generates InputEvent(s) from an InputReport and the
///                      InputReport that precedes it. Each type of input device defines how it
///                      processes InputReports.
///                      The [`InputReport`] returned by `process_reports` must have no
///                      `wake_lease`.
///
const MAX_UNACKNOWLEDGED_REPORTS_LIMIT: u16 = 120;

struct LocalReaderV2Handler<InputDeviceProcessReportsFn> {
    client: fidl_next::Client<
        fidl_next_fuchsia_input_report::InputReportsReaderV2,
        fidl_next::fuchsia::zx::Channel,
    >,
    previous_state: Option<PreviousDeviceState>,
    device_descriptor: InputDeviceDescriptor,
    event_sender: UnboundedSender<Vec<InputEvent>>,
    inspect_status: std::rc::Rc<InputDeviceStatus>,
    metrics_logger: metrics::MetricsLogger,
    feature_flags: InputPipelineFeatureFlags,
    process_reports: InputDeviceProcessReportsFn,
    ack_threshold: u64,
    last_acknowledged_stamp: u64,
}

impl<InputDeviceProcessReportsFn>
    fidl_next_fuchsia_input_report::InputReportsReaderV2LocalClientHandler<
        fidl_next::fuchsia::zx::Channel,
    > for LocalReaderV2Handler<InputDeviceProcessReportsFn>
where
    InputDeviceProcessReportsFn:
        for<'de> FnMut(
            &[fidl_next_fuchsia_input_report::wire::InputReport<'_>],
            Option<PreviousDeviceState>,
            &InputDeviceDescriptor,
            &mut UnboundedSender<Vec<InputEvent>>,
            &InputDeviceStatus,
            &metrics::MetricsLogger,
            &InputPipelineFeatureFlags,
        )
            -> (Option<PreviousDeviceState>, Option<UnboundedReceiver<InputEvent>>),
{
    async fn on_input_reports(
        &mut self,
        request: fidl_next::Request<
            fidl_next_fuchsia_input_report::input_reports_reader_v2::OnInputReports,
            fidl_next::fuchsia::zx::Channel,
        >,
    ) {
        fuchsia_trace::duration!("input", "input-device-process-reports");
        let payload = request.wire_payload();
        // TODO: b/513602239 - use InputEvent instead of InputReport for previous
        // report. To avoid wire to natural type conversion.
        let (prev_state, inspect_receiver) = (self.process_reports)(
            payload.reports.as_slice(),
            self.previous_state.take(),
            &self.device_descriptor,
            &mut self.event_sender,
            &self.inspect_status,
            &self.metrics_logger,
            &self.feature_flags,
        );
        self.previous_state = prev_state;

        let reports_stamp = *payload.last_report_stamp;
        if reports_stamp.saturating_sub(self.last_acknowledged_stamp) >= self.ack_threshold {
            if let Err(e) = self.client.acknowledge_reports(reports_stamp).await {
                log::warn!("failed to send acknowledge_reports: {:?}", e);
            }
            self.last_acknowledged_stamp = reports_stamp;
        }

        // If a report generates multiple events asynchronously, we send them over a mpsc channel
        // to inspect_receiver. We update the event count on inspect_status here since we cannot
        // pass a reference to inspect_status to an async task in process_reports().
        if let Some(mut receiver) = inspect_receiver {
            let inspect_status = self.inspect_status.clone();
            let _task = Dispatcher::spawn_local(async move {
                while let Some(event) = receiver.next().await {
                    inspect_status.count_generated_event(event);
                }
            });
        }
    }
}

pub fn initialize_report_stream<InputDeviceProcessReportsFn>(
    device_proxy: fidl_next::Client<InputDevice, Transport>,
    device_descriptor: InputDeviceDescriptor,
    event_sender: UnboundedSender<Vec<InputEvent>>,
    inspect_status: InputDeviceStatus,
    metrics_logger: metrics::MetricsLogger,
    feature_flags: InputPipelineFeatureFlags,
    process_reports: InputDeviceProcessReportsFn,
) -> crate::dispatcher::TaskHandle<()>
where
    InputDeviceProcessReportsFn: 'static
        + Send
        + for<'de> FnMut(
            &[fidl_next_fuchsia_input_report::wire::InputReport<'_>],
            Option<PreviousDeviceState>,
            &InputDeviceDescriptor,
            &mut UnboundedSender<Vec<InputEvent>>,
            &InputDeviceStatus,
            &metrics::MetricsLogger,
            &InputPipelineFeatureFlags,
        )
            -> (Option<PreviousDeviceState>, Option<UnboundedReceiver<InputEvent>>),
{
    Dispatcher::spawn_local(async move {
        let inspect_status = std::rc::Rc::new(inspect_status);
        let (client_end, server_end) = fidl_next::fuchsia::create_channel();
        let max_unacknowledged_reports = match device_proxy
            .get_input_reports_reader_v2(server_end, MAX_UNACKNOWLEDGED_REPORTS_LIMIT)
            .await
        {
            Ok(response) => {
                response.max_unacknowledged_reports.min(MAX_UNACKNOWLEDGED_REPORTS_LIMIT)
            }
            Err(e) => {
                metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::InputDeviceGetInputReportsReaderError,
                    std::format!("error on GetInputReportsReaderV2: {:?}", e),
                );
                return; // TODO(https://fxbug.dev/42131965): signal error
            }
        };

        let ack_threshold = ((max_unacknowledged_reports / 2).max(1)) as u64;
        let (_client, join_handle) = client_end.spawn_local_handler_full_on_with(
            |client| LocalReaderV2Handler {
                client,
                previous_state: None,
                device_descriptor,
                event_sender,
                inspect_status,
                metrics_logger,
                feature_flags,
                process_reports,
                ack_threshold,
                last_acknowledged_stamp: 0,
            },
            &crate::dispatcher::LocalDriverExecutor::default(),
        );

        let _ = join_handle.await;
        // TODO(https://fxbug.dev/42131965): Add signaling for when this loop exits, since it means the device
        // binding is no longer functional.
        log::warn!("initialize_report_stream exited - device binding no longer works");
    })
}

/// Returns true if the device type of `input_device` matches `device_type`.
///
/// # Parameters
/// - `input_device`: The InputDevice to check the type of.
/// - `device_type`: The type of the device to compare to.
pub async fn is_device_type(
    device_descriptor: &fidl_next_fuchsia_input_report::DeviceDescriptor,
    device_type: InputDeviceType,
) -> bool {
    // Return if the device type matches the desired `device_type`.
    match device_type {
        InputDeviceType::ConsumerControls => device_descriptor.consumer_control.is_some(),
        InputDeviceType::Mouse => device_descriptor.mouse.is_some(),
        InputDeviceType::Touch => device_descriptor.touch.is_some(),
        InputDeviceType::Keyboard => device_descriptor.keyboard.is_some(),
        InputDeviceType::LightSensor => device_descriptor.sensor.is_some(),
    }
}

/// Returns a new [`InputDeviceBinding`] of the given device type.
///
/// # Parameters
/// - `device_type`: The type of the input device.
/// - `device_proxy`: The device proxy which is used to get input reports.
/// - `device_id`: The id of the connected input device.
/// - `input_event_sender`: The channel to send generated InputEvents to.
pub async fn get_device_binding(
    device_type: InputDeviceType,
    device_proxy: fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
    device_id: u32,
    input_event_sender: UnboundedSender<Vec<InputEvent>>,
    device_node: fuchsia_inspect::Node,
    feature_flags: InputPipelineFeatureFlags,
    metrics_logger: metrics::MetricsLogger,
    is_injected: bool,
) -> Result<(Box<dyn InputDeviceBinding>, crate::dispatcher::TaskHandle<()>), Error> {
    match device_type {
        InputDeviceType::ConsumerControls => {
            let (binding, task) = consumer_controls_binding::ConsumerControlsBinding::new(
                device_proxy,
                device_id,
                input_event_sender,
                device_node,
                feature_flags.clone(),
                metrics_logger,
                is_injected,
            )
            .await?;
            Ok((Box::new(binding), task))
        }
        InputDeviceType::Mouse => {
            let (binding, task) = mouse_binding::MouseBinding::new(
                device_proxy,
                device_id,
                input_event_sender,
                device_node,
                feature_flags.clone(),
                metrics_logger,
            )
            .await?;
            Ok((Box::new(binding), task))
        }
        InputDeviceType::Touch => {
            let (binding, task) = touch_binding::TouchBinding::new(
                device_proxy,
                device_id,
                input_event_sender,
                device_node,
                feature_flags.clone(),
                metrics_logger,
            )
            .await?;
            Ok((Box::new(binding), task))
        }
        InputDeviceType::Keyboard => {
            let (binding, task) = keyboard_binding::KeyboardBinding::new(
                device_proxy,
                device_id,
                input_event_sender,
                device_node,
                feature_flags.clone(),
                metrics_logger,
            )
            .await?;
            Ok((Box::new(binding), task))
        }
        InputDeviceType::LightSensor => {
            let (binding, task) = light_sensor_binding::LightSensorBinding::new(
                device_proxy,
                device_id,
                input_event_sender,
                device_node,
                feature_flags.clone(),
                metrics_logger,
            )
            .await?;
            Ok((Box::new(binding), task))
        }
    }
}

/// Returns the event time if it exists, otherwise returns the current time.
///
/// # Parameters
/// - `event_time`: The event time from an InputReport.
pub fn event_time_or_now(event_time: Option<i64>) -> zx::MonotonicInstant {
    match event_time {
        Some(time) => zx::MonotonicInstant::from_nanos(time),
        None => zx::MonotonicInstant::get(),
    }
}

impl std::convert::From<UnhandledInputEvent> for InputEvent {
    fn from(event: UnhandledInputEvent) -> Self {
        Self {
            device_event: event.device_event,
            device_descriptor: event.device_descriptor,
            event_time: event.event_time,
            handled: Handled::No,
            trace_id: event.trace_id,
        }
    }
}

// Fallible conversion from an InputEvent to an UnhandledInputEvent.
//
// Useful to adapt various functions in the [`testing_utilities`] module
// to work with tests for [`UnhandledInputHandler`]s.
//
// Production code however, should probably just match on the [`InputEvent`].
#[cfg(test)]
impl std::convert::TryFrom<InputEvent> for UnhandledInputEvent {
    type Error = anyhow::Error;
    fn try_from(event: InputEvent) -> Result<UnhandledInputEvent, Self::Error> {
        match event.handled {
            Handled::Yes => {
                Err(anyhow::anyhow!("Attempted to treat a handled InputEvent as unhandled"))
            }
            Handled::No => Ok(UnhandledInputEvent {
                device_event: event.device_event,
                device_descriptor: event.device_descriptor,
                event_time: event.event_time,
                trace_id: event.trace_id,
            }),
        }
    }
}

impl InputEvent {
    /// Marks the event as handled, if `predicate` is `true`.
    /// Otherwise, leaves the event unchanged.
    pub(crate) fn into_handled_if(self, predicate: bool) -> Self {
        if predicate { Self { handled: Handled::Yes, ..self } } else { self }
    }

    /// Marks the event as handled.
    pub(crate) fn into_handled(self) -> Self {
        Self { handled: Handled::Yes, ..self }
    }

    /// Returns the same event, with modified event time.
    pub fn into_with_event_time(self, event_time: zx::MonotonicInstant) -> Self {
        Self { event_time, ..self }
    }

    /// Returns the same event, with modified device descriptor.
    #[cfg(test)]
    pub fn into_with_device_descriptor(self, device_descriptor: InputDeviceDescriptor) -> Self {
        Self { device_descriptor, ..self }
    }

    /// Returns true if this event is marked as handled.
    pub fn is_handled(&self) -> bool {
        self.handled == Handled::Yes
    }

    // Returns event type as string.
    pub fn get_event_type(&self) -> &'static str {
        match self.device_event {
            InputDeviceEvent::Keyboard(_) => "keyboard_event",
            InputDeviceEvent::LightSensor(_) => "light_sensor_event",
            InputDeviceEvent::ConsumerControls(_) => "consumer_controls_event",
            InputDeviceEvent::Mouse(_) => "mouse_event",
            InputDeviceEvent::TouchScreen(_) => "touch_screen_event",
            InputDeviceEvent::Touchpad(_) => "touchpad_event",
            #[cfg(test)]
            InputDeviceEvent::Fake => "fake_event",
        }
    }

    pub fn record_inspect(&self, node: &fuchsia_inspect::Node) {
        node.record_int("event_time", self.event_time.into_nanos());
        match &self.device_event {
            InputDeviceEvent::LightSensor(e) => e.record_inspect(node),
            InputDeviceEvent::ConsumerControls(e) => e.record_inspect(node),
            InputDeviceEvent::Mouse(e) => e.record_inspect(node),
            InputDeviceEvent::TouchScreen(e) => e.record_inspect(node),
            InputDeviceEvent::Touchpad(e) => e.record_inspect(node),
            // No-op for KeyboardEvent, since we don't want to potentially record sensitive information to Inspect.
            InputDeviceEvent::Keyboard(_) => (),
            #[cfg(test)] // No-op for Fake InputDeviceEvent.
            InputDeviceEvent::Fake => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing_utilities::spawn_input_stream_handler;
    use assert_matches::assert_matches;
    use diagnostics_assertions::AnyProperty;
    use fidl_fuchsia_input_report as fidl_input_report;
    use fidl_next_fuchsia_input_report::InputReport;
    use pretty_assertions::assert_eq;
    use std::convert::TryFrom as _;
    use test_case::test_case;

    #[test]
    fn max_event_time() {
        let event_time = event_time_or_now(Some(i64::MAX));
        assert_eq!(event_time, zx::MonotonicInstant::INFINITE);
    }

    #[test]
    fn min_event_time() {
        let event_time = event_time_or_now(Some(i64::MIN));
        assert_eq!(event_time, zx::MonotonicInstant::INFINITE_PAST);
    }

    #[fuchsia::test]
    async fn input_device_status_initialized_with_correct_properties() {
        let inspector = fuchsia_inspect::Inspector::default();
        let input_pipeline_node = inspector.root().create_child("input_pipeline");
        let input_devices_node = input_pipeline_node.create_child("input_devices");
        let device_node = input_devices_node.create_child("001_keyboard");
        let _input_device_status = InputDeviceStatus::new(device_node);
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_pipeline: {
                input_devices: {
                    "001_keyboard": {
                        reports_received_count: 0u64,
                        reports_filtered_count: 0u64,
                        events_generated: 0u64,
                        last_received_timestamp_ns: 0u64,
                        last_generated_timestamp_ns: 0u64,
                        "fuchsia.inspect.Health": {
                            status: "STARTING_UP",
                            // Timestamp value is unpredictable and not relevant in this context,
                            // so we only assert that the property is present.
                            start_timestamp_nanos: AnyProperty
                        },
                        driver_to_binding_latency_ms: diagnostics_assertions::HistogramAssertion::exponential(super::LATENCY_HISTOGRAM_PROPERTIES),
                        wake_lease_leak_count: 0u64,
                    }
                }
            }
        });
    }

    #[test_case(i64::MIN; "min value")]
    #[test_case(-1; "negative value")]
    #[test_case(0; "zero")]
    #[test_case(1; "positive value")]
    #[test_case(i64::MAX; "max value")]
    #[fuchsia::test(allow_stalls = false)]
    async fn input_device_status_updates_latency_histogram_on_count_received_report_wire(
        latency_nsec: i64,
    ) {
        let mut expected_histogram = diagnostics_assertions::HistogramAssertion::exponential(
            super::LATENCY_HISTOGRAM_PROPERTIES,
        );
        let inspector = fuchsia_inspect::Inspector::default();
        let input_device_status = InputDeviceStatus::new_internal(
            inspector.root().clone_weak(),
            Box::new(move || zx::MonotonicInstant::from_nanos(latency_nsec)),
        );
        let decoded = crate::testing_utilities::report_to_wire(InputReport {
            event_time: Some(0),
            ..InputReport::default()
        });
        input_device_status.count_received_report_wire(&decoded);
        expected_histogram.insert_values([latency_nsec / 1000 / 1000]);
        diagnostics_assertions::assert_data_tree!(inspector, root: contains {
            driver_to_binding_latency_ms: expected_histogram,
        });
    }

    // Tests that is_device_type() returns true for InputDeviceType::ConsumerControls when a
    // consumer controls device exists.
    #[fuchsia::test]
    async fn consumer_controls_input_device_exists() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: None,
                            sensor: None,
                            touch: None,
                            keyboard: None,
                            consumer_control: Some(fidl_input_report::ConsumerControlDescriptor {
                                input: Some(fidl_input_report::ConsumerControlInputDescriptor {
                                    buttons: Some(vec![
                                        fidl_fuchsia_input::ConsumerControlButton::VolumeUp,
                                        fidl_fuchsia_input::ConsumerControlButton::VolumeDown,
                                    ]),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        assert!(
            is_device_type(
                &input_device_proxy
                    .get_descriptor()
                    .await
                    .expect("Failed to get device descriptor")
                    .descriptor,
                InputDeviceType::ConsumerControls
            )
            .await
        );
    }

    // Tests that is_device_type() returns true for InputDeviceType::Mouse when a mouse exists.
    #[fuchsia::test]
    async fn mouse_input_device_exists() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: Some(fidl_input_report::MouseDescriptor {
                                input: Some(fidl_input_report::MouseInputDescriptor {
                                    movement_x: None,
                                    movement_y: None,
                                    position_x: None,
                                    position_y: None,
                                    scroll_v: None,
                                    scroll_h: None,
                                    buttons: None,
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            sensor: None,
                            touch: None,
                            keyboard: None,
                            consumer_control: None,
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        assert!(
            is_device_type(
                &input_device_proxy
                    .get_descriptor()
                    .await
                    .expect("Failed to get device descriptor")
                    .descriptor,
                InputDeviceType::Mouse
            )
            .await
        );
    }

    // Tests that is_device_type() returns true for InputDeviceType::Mouse when a mouse doesn't
    // exist.
    #[fuchsia::test]
    async fn mouse_input_device_doesnt_exist() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: None,
                            sensor: None,
                            touch: None,
                            keyboard: None,
                            consumer_control: None,
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        assert!(
            !is_device_type(
                &input_device_proxy
                    .get_descriptor()
                    .await
                    .expect("Failed to get device descriptor")
                    .descriptor,
                InputDeviceType::Mouse
            )
            .await
        );
    }

    // Tests that is_device_type() returns true for InputDeviceType::Touch when a touchscreen
    // exists.
    #[fuchsia::test]
    async fn touch_input_device_exists() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: None,
                            sensor: None,
                            touch: Some(fidl_input_report::TouchDescriptor {
                                input: Some(fidl_input_report::TouchInputDescriptor {
                                    contacts: None,
                                    max_contacts: None,
                                    touch_type: None,
                                    buttons: None,
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            keyboard: None,
                            consumer_control: None,
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        assert!(
            is_device_type(
                &input_device_proxy
                    .get_descriptor()
                    .await
                    .expect("Failed to get device descriptor")
                    .descriptor,
                InputDeviceType::Touch
            )
            .await
        );
    }

    // Tests that is_device_type() returns true for InputDeviceType::Touch when a touchscreen
    // exists.
    #[fuchsia::test]
    async fn touch_input_device_doesnt_exist() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: None,
                            sensor: None,
                            touch: None,
                            keyboard: None,
                            consumer_control: None,
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        assert!(
            !is_device_type(
                &input_device_proxy
                    .get_descriptor()
                    .await
                    .expect("Failed to get device descriptor")
                    .descriptor,
                InputDeviceType::Touch
            )
            .await
        );
    }

    // Tests that is_device_type() returns true for InputDeviceType::Keyboard when a keyboard
    // exists.
    #[fuchsia::test]
    async fn keyboard_input_device_exists() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: None,
                            sensor: None,
                            touch: None,
                            keyboard: Some(fidl_input_report::KeyboardDescriptor {
                                input: Some(fidl_input_report::KeyboardInputDescriptor {
                                    keys3: None,
                                    ..Default::default()
                                }),
                                output: None,
                                ..Default::default()
                            }),
                            consumer_control: None,
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        assert!(
            is_device_type(
                &input_device_proxy
                    .get_descriptor()
                    .await
                    .expect("Failed to get device descriptor")
                    .descriptor,
                InputDeviceType::Keyboard
            )
            .await
        );
    }

    // Tests that is_device_type() returns true for InputDeviceType::Keyboard when a keyboard
    // exists.
    #[fuchsia::test]
    async fn keyboard_input_device_doesnt_exist() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: None,
                            sensor: None,
                            touch: None,
                            keyboard: None,
                            consumer_control: None,
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        assert!(
            !is_device_type(
                &input_device_proxy
                    .get_descriptor()
                    .await
                    .expect("Failed to get device descriptor")
                    .descriptor,
                InputDeviceType::Keyboard
            )
            .await
        );
    }

    // Tests that is_device_type() returns true for every input device type that exists.
    #[fuchsia::test]
    async fn no_input_device_match() {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&fidl_input_report::DeviceDescriptor {
                            device_information: None,
                            mouse: Some(fidl_input_report::MouseDescriptor {
                                input: Some(fidl_input_report::MouseInputDescriptor {
                                    movement_x: None,
                                    movement_y: None,
                                    position_x: None,
                                    position_y: None,
                                    scroll_v: None,
                                    scroll_h: None,
                                    buttons: None,
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            sensor: None,
                            touch: Some(fidl_input_report::TouchDescriptor {
                                input: Some(fidl_input_report::TouchInputDescriptor {
                                    contacts: None,
                                    max_contacts: None,
                                    touch_type: None,
                                    buttons: None,
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            keyboard: Some(fidl_input_report::KeyboardDescriptor {
                                input: Some(fidl_input_report::KeyboardInputDescriptor {
                                    keys3: None,
                                    ..Default::default()
                                }),
                                output: None,
                                ..Default::default()
                            }),
                            consumer_control: Some(fidl_input_report::ConsumerControlDescriptor {
                                input: Some(fidl_input_report::ConsumerControlInputDescriptor {
                                    buttons: Some(vec![
                                        fidl_fuchsia_input::ConsumerControlButton::VolumeUp,
                                        fidl_fuchsia_input::ConsumerControlButton::VolumeDown,
                                    ]),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                    }
                    _ => panic!("InputDevice handler received an unexpected request"),
                }
            });

        let device_descriptor = &input_device_proxy
            .get_descriptor()
            .await
            .expect("Failed to get device descriptor")
            .descriptor;
        assert!(is_device_type(&device_descriptor, InputDeviceType::ConsumerControls).await);
        assert!(is_device_type(&device_descriptor, InputDeviceType::Mouse).await);
        assert!(is_device_type(&device_descriptor, InputDeviceType::Touch).await);
        assert!(is_device_type(&device_descriptor, InputDeviceType::Keyboard).await);
    }

    #[fuchsia::test]
    fn unhandled_to_generic_conversion_sets_handled_flag_to_no() {
        assert_eq!(
            InputEvent::from(UnhandledInputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: zx::MonotonicInstant::from_nanos(1),
                trace_id: None,
            })
            .handled,
            Handled::No
        );
    }

    #[fuchsia::test]
    fn unhandled_to_generic_conversion_preserves_fields() {
        const EVENT_TIME: zx::MonotonicInstant = zx::MonotonicInstant::from_nanos(42);
        let expected_trace_id: Option<ftrace::Id> = Some(1234.into());
        assert_eq!(
            InputEvent::from(UnhandledInputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: EVENT_TIME,
                trace_id: expected_trace_id,
            }),
            InputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: EVENT_TIME,
                handled: Handled::No,
                trace_id: expected_trace_id,
            },
        );
    }

    #[fuchsia::test]
    fn generic_to_unhandled_conversion_fails_for_handled_events() {
        assert_matches!(
            UnhandledInputEvent::try_from(InputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: zx::MonotonicInstant::from_nanos(1),
                handled: Handled::Yes,
                trace_id: None,
            }),
            Err(_)
        )
    }

    #[fuchsia::test]
    fn generic_to_unhandled_conversion_preserves_fields_for_unhandled_events() {
        const EVENT_TIME: zx::MonotonicInstant = zx::MonotonicInstant::from_nanos(42);
        let expected_trace_id: Option<ftrace::Id> = Some(1234.into());
        assert_eq!(
            UnhandledInputEvent::try_from(InputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: EVENT_TIME,
                handled: Handled::No,
                trace_id: expected_trace_id,
            })
            .unwrap(),
            UnhandledInputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: EVENT_TIME,
                trace_id: expected_trace_id,
            },
        )
    }

    #[test_case(Handled::No; "initially not handled")]
    #[test_case(Handled::Yes; "initially handled")]
    fn into_handled_if_yields_handled_yes_on_true(initially_handled: Handled) {
        let event = InputEvent {
            device_event: InputDeviceEvent::Fake,
            device_descriptor: InputDeviceDescriptor::Fake,
            event_time: zx::MonotonicInstant::from_nanos(1),
            handled: initially_handled,
            trace_id: None,
        };
        pretty_assertions::assert_eq!(event.into_handled_if(true).handled, Handled::Yes);
    }

    #[test_case(Handled::No; "initially not handled")]
    #[test_case(Handled::Yes; "initially handled")]
    fn into_handled_if_leaves_handled_unchanged_on_false(initially_handled: Handled) {
        let event = InputEvent {
            device_event: InputDeviceEvent::Fake,
            device_descriptor: InputDeviceDescriptor::Fake,
            event_time: zx::MonotonicInstant::from_nanos(1),
            handled: initially_handled.clone(),
            trace_id: None,
        };
        pretty_assertions::assert_eq!(event.into_handled_if(false).handled, initially_handled);
    }

    #[test_case(Handled::No; "initially not handled")]
    #[test_case(Handled::Yes; "initially handled")]
    fn into_handled_yields_handled_yes(initially_handled: Handled) {
        let event = InputEvent {
            device_event: InputDeviceEvent::Fake,
            device_descriptor: InputDeviceDescriptor::Fake,
            event_time: zx::MonotonicInstant::from_nanos(1),
            handled: initially_handled,
            trace_id: None,
        };
        pretty_assertions::assert_eq!(event.into_handled().handled, Handled::Yes);
    }

    #[fuchsia::test]
    async fn initialize_report_stream_acknowledges_reports() {
        let (ack_sender, mut ack_receiver) = futures::channel::mpsc::unbounded::<u64>();

        let (input_device_proxy, _task) = spawn_input_stream_handler(move |input_device_request| {
            let ack_sender = ack_sender.clone();
            async move {
                match input_device_request {
                    fidl_input_report::InputDeviceRequest::GetInputReportsReaderV2 {
                        reader,
                        max_unacknowledged_reports_limit: _,
                        responder,
                    } => {
                        // Report max_unacknowledged_reports = 2, so ack_threshold is (2/2).max(1) = 1.
                        responder.send(2).unwrap();
                        let (mut request_stream, control_handle) =
                            reader.into_stream_and_control_handle();
                        fuchsia_async::Task::local(async move {
                                // Send report batch with stamp 1.
                                control_handle
                                    .send_on_input_reports(
                                        vec![fidl_input_report::InputReport::default()],
                                        1,
                                    )
                                    .unwrap();

                                while let Some(Ok(req)) = request_stream.next().await {
                                    match req {
                                        fidl_input_report::InputReportsReaderV2Request::AcknowledgeReports {
                                            last_acknowledged_report_stamp,
                                            ..
                                        } => {
                                            ack_sender
                                                .unbounded_send(last_acknowledged_report_stamp)
                                                .unwrap();
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            })
                            .detach();
                    }
                    _ => panic!("unexpected request: {:?}", input_device_request),
                }
            }
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let device_node = inspector.root().create_child("test_device");
        let inspect_status = InputDeviceStatus::new(device_node);
        let (event_sender, _event_receiver) = futures::channel::mpsc::unbounded();

        let _stream_task = initialize_report_stream(
            input_device_proxy,
            InputDeviceDescriptor::Fake,
            event_sender,
            inspect_status,
            metrics::MetricsLogger::default(),
            InputPipelineFeatureFlags::default(),
            |_reports, prev, _desc, _sender, _status, _logger, _flags| (prev, None),
        );

        let acked_stamp = ack_receiver.next().await;
        assert_eq!(acked_stamp, Some(1));
    }
}
