// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device::{self, Handled, InputDeviceStatus, InputEvent};
use crate::utils::{self, Position};
use crate::{Transport, metrics};
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fuchsia_inspect::ArrayProperty;
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use metrics_registry::*;
use sorted_vec_map::SortedVecSet;
use zx;

pub type MouseButton = u8;

/// Flag to indicate the scroll event is from device reporting precision delta.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PrecisionScroll {
    /// Touchpad and some mouse able to report precision delta.
    Yes,
    /// Tick based mouse wheel.
    No,
}

/// A [`MouseLocation`] represents the mouse pointer location at the time of a pointer event.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MouseLocation {
    /// A mouse movement relative to its current position.
    Relative(RelativeLocation),

    /// An absolute position, in device coordinates.
    Absolute(Position),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MousePhase {
    Down,  // One or more buttons were newly pressed.
    Move,  // The mouse moved with no change in button state.
    Up,    // One or more buttons were newly released.
    Wheel, // Mouse wheel is rotating.
}

/// A [`RelativeLocation`] contains the relative mouse pointer location at the time of a pointer event.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RelativeLocation {
    /// A pointer location in counts.
    pub counts: Position,
}

impl Default for RelativeLocation {
    fn default() -> Self {
        RelativeLocation { counts: Position::zero() }
    }
}

/// A [`WheelDelta`] contains raw wheel delta ticks from driver or gesture arena
/// and scaled wheel delta in physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct WheelDelta {
    pub ticks: i64,
    pub physical_pixel: Option<f32>,
}

/// A [`MouseEvent`] represents a pointer event with a specified phase, and the buttons
/// involved in said phase. The supported phases for mice include Up, Down, and Move.
///
/// # Example
/// The following MouseEvent represents a relative movement of 40 units in the x axis
/// and 20 units in the y axis while holding the primary button (1) down.
///
/// ```
/// let mouse_device_event = input_device::InputDeviceEvent::Mouse(MouseEvent::new(
///     MouseLocation::Relative(RelativeLocation {
///       counts: Position { x: 40.0, y: 20.0 },
///     }),
///     None, // wheel_delta_v
///     None, // wheel_delta_h
///     MousePhase::Move,
///     SortedVecSet::from(vec![1]),
///     SortedVecSet::from(vec![1]),
///     None, // is_precision_scroll
///     None, // wake_lease
/// ));
/// ```
#[derive(Debug)]
pub struct MouseEvent {
    /// The mouse location.
    pub location: MouseLocation,

    /// The mouse wheel rotated delta in vertical.
    pub wheel_delta_v: Option<WheelDelta>,

    /// The mouse wheel rotated delta in horizontal.
    pub wheel_delta_h: Option<WheelDelta>,

    /// The mouse device reports precision scroll delta.
    pub is_precision_scroll: Option<PrecisionScroll>,

    /// The phase of the [`buttons`] associated with this input event.
    pub phase: MousePhase,

    /// The buttons relevant to this event.
    pub affected_buttons: SortedVecSet<MouseButton>,

    /// The complete button state including this event.
    pub pressed_buttons: SortedVecSet<MouseButton>,

    /// The wake lease for this event.
    pub wake_lease: Option<zx::EventPair>,
}

impl Clone for MouseEvent {
    fn clone(&self) -> Self {
        log::debug!("MouseEvent cloned without wake lease.");
        Self {
            location: self.location,
            wheel_delta_v: self.wheel_delta_v.clone(),
            wheel_delta_h: self.wheel_delta_h.clone(),
            is_precision_scroll: self.is_precision_scroll,
            phase: self.phase,
            affected_buttons: self.affected_buttons.clone(),
            pressed_buttons: self.pressed_buttons.clone(),
            wake_lease: None,
        }
    }
}

impl PartialEq for MouseEvent {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location
            && self.wheel_delta_v == other.wheel_delta_v
            && self.wheel_delta_h == other.wheel_delta_h
            && self.is_precision_scroll == other.is_precision_scroll
            && self.phase == other.phase
            && self.affected_buttons == other.affected_buttons
            && self.pressed_buttons == other.pressed_buttons
    }
}

impl MouseEvent {
    /// Creates a new [`MouseEvent`].
    ///
    /// # Parameters
    /// - `location`: The mouse location.
    /// - `phase`: The phase of the [`buttons`] associated with this input event.
    /// - `buttons`: The buttons relevant to this event.
    /// - `wake_lease`: The wake lease for this event.
    pub fn new(
        location: MouseLocation,
        wheel_delta_v: Option<WheelDelta>,
        wheel_delta_h: Option<WheelDelta>,
        phase: MousePhase,
        affected_buttons: SortedVecSet<MouseButton>,
        pressed_buttons: SortedVecSet<MouseButton>,
        is_precision_scroll: Option<PrecisionScroll>,
        wake_lease: Option<zx::EventPair>,
    ) -> MouseEvent {
        MouseEvent {
            location,
            wheel_delta_v,
            wheel_delta_h,
            phase,
            affected_buttons,
            pressed_buttons,
            is_precision_scroll,
            wake_lease,
        }
    }

    pub fn record_inspect(&self, node: &fuchsia_inspect::Node) {
        // Note: location coordinates (`self.location`) are omitted to avoid exposing sensitive user behavior.
        if let Some(wheel_delta_v) = &self.wheel_delta_v {
            node.record_child("wheel_delta_v", move |wheel_delta_v_node| {
                wheel_delta_v_node.record_int("ticks", wheel_delta_v.ticks);
                if let Some(physical_pixel) = wheel_delta_v.physical_pixel {
                    wheel_delta_v_node.record_double("physical_pixel", f64::from(physical_pixel));
                }
            });
        }

        if let Some(wheel_delta_h) = &self.wheel_delta_h {
            node.record_child("wheel_delta_h", move |wheel_delta_h_node| {
                wheel_delta_h_node.record_int("ticks", wheel_delta_h.ticks);
                if let Some(physical_pixel) = wheel_delta_h.physical_pixel {
                    wheel_delta_h_node.record_double("physical_pixel", f64::from(physical_pixel));
                }
            });
        }

        if let Some(is_precision_scroll) = self.is_precision_scroll {
            match is_precision_scroll {
                PrecisionScroll::Yes => node.record_string("is_precision_scroll", "yes"),
                PrecisionScroll::No => node.record_string("is_precision_scroll", "no"),
            }
        }

        match self.phase {
            MousePhase::Down => node.record_string("phase", "down"),
            MousePhase::Move => node.record_string("phase", "move"),
            MousePhase::Up => node.record_string("phase", "up"),
            MousePhase::Wheel => node.record_string("phase", "wheel"),
        }

        let affected_buttons_node =
            node.create_uint_array("affected_buttons", self.affected_buttons.len());
        self.affected_buttons.iter().enumerate().for_each(|(i, button)| {
            affected_buttons_node.set(i, *button);
        });
        node.record(affected_buttons_node);

        let pressed_buttons_node =
            node.create_uint_array("pressed_buttons", self.pressed_buttons.len());
        self.pressed_buttons.iter().enumerate().for_each(|(i, button)| {
            pressed_buttons_node.set(i, *button);
        });
        node.record(pressed_buttons_node);
    }
}

/// A [`MouseBinding`] represents a connection to a mouse input device.
///
/// The [`MouseBinding`] parses and exposes mouse descriptor properties (e.g., the range of
/// possible x values) for the device it is associated with. It also parses [`InputReport`]s
/// from the device, and sends them to the device binding owner over `event_sender`.
pub struct MouseBinding {
    /// The channel to stream InputEvents to.
    event_sender: UnboundedSender<Vec<InputEvent>>,

    /// Holds information about this device.
    device_descriptor: MouseDeviceDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MouseDeviceDescriptor {
    /// The id of the connected mouse input device.
    pub device_id: u32,

    /// The range of possible x values of absolute mouse positions reported by this device.
    pub absolute_x_range: Option<fidl_fuchsia_input::Range>,

    /// The range of possible y values of absolute mouse positions reported by this device.
    pub absolute_y_range: Option<fidl_fuchsia_input::Range>,

    /// The range of possible vertical wheel delta reported by this device.
    pub wheel_v_range: Option<fidl_fuchsia_input::Axis>,

    /// The range of possible horizontal wheel delta reported by this device.
    pub wheel_h_range: Option<fidl_fuchsia_input::Axis>,

    /// This is a vector of ids for the mouse buttons.
    pub buttons: Option<Vec<MouseButton>>,
}

#[async_trait]
impl input_device::InputDeviceBinding for MouseBinding {
    fn input_event_sender(&self) -> UnboundedSender<Vec<InputEvent>> {
        self.event_sender.clone()
    }

    fn get_device_descriptor(&self) -> input_device::InputDeviceDescriptor {
        input_device::InputDeviceDescriptor::Mouse(self.device_descriptor.clone())
    }
}

impl MouseBinding {
    /// Creates a new [`InputDeviceBinding`] from the `device_proxy`.
    ///
    /// The binding will start listening for input reports immediately and send new InputEvents
    /// to the device binding owner over `input_event_sender`.
    ///
    /// # Parameters
    /// - `device_proxy`: The proxy to bind the new [`InputDeviceBinding`] to.
    /// - `device_id`: The id of the connected mouse device.
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
        _feature_flags: input_device::InputPipelineFeatureFlags,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<(Self, crate::dispatcher::TaskHandle<()>), Error> {
        let (device_descriptor, mut inspect_status) =
            Self::bind_device(&device_proxy, device_id, device_node).await?;
        inspect_status.health_node.set_ok();
        let task = input_device::initialize_report_stream(
            device_proxy,
            input_device::InputDeviceDescriptor::Mouse(device_descriptor.clone()),
            input_event_sender.clone(),
            inspect_status,
            metrics_logger,
            _feature_flags,
            Self::process_reports,
        );

        Ok((MouseBinding { event_sender: input_event_sender, device_descriptor }, task))
    }

    /// Binds the provided input device to a new instance of `Self`.
    ///
    /// # Parameters
    /// - `device`: The device to use to initialize the binding.
    /// - `device_id`: The id of the connected mouse device.
    /// - `input_event_sender`: The channel to send new InputEvents to.
    /// - `device_node`: The inspect node for this device binding
    ///
    /// # Errors
    /// If the device descriptor could not be retrieved, or the descriptor could
    /// not be parsed correctly.
    async fn bind_device(
        device: &fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
        device_id: u32,
        device_node: fuchsia_inspect::Node,
    ) -> Result<(MouseDeviceDescriptor, InputDeviceStatus), Error> {
        let mut input_device_status = InputDeviceStatus::new(device_node);
        let device_descriptor: fidl_next_fuchsia_input_report::DeviceDescriptor = match device
            .get_descriptor()
            .await
        {
            Ok(res) => res.descriptor,
            Err(_) => {
                input_device_status.health_node.set_unhealthy("Could not get device descriptor.");
                return Err(format_err!("Could not get descriptor for device_id: {}", device_id));
            }
        };

        let mouse_descriptor = device_descriptor.mouse.ok_or_else(|| {
            input_device_status
                .health_node
                .set_unhealthy("DeviceDescriptor does not have a MouseDescriptor.");
            format_err!("DeviceDescriptor does not have a MouseDescriptor")
        })?;

        let mouse_input_descriptor = mouse_descriptor.input.ok_or_else(|| {
            input_device_status
                .health_node
                .set_unhealthy("MouseDescriptor does not have a MouseInputDescriptor.");
            format_err!("MouseDescriptor does not have a MouseInputDescriptor")
        })?;

        let device_descriptor: MouseDeviceDescriptor = MouseDeviceDescriptor {
            device_id,
            absolute_x_range: mouse_input_descriptor
                .position_x
                .as_ref()
                .map(|axis| utils::range_to_old(&axis.range)),
            absolute_y_range: mouse_input_descriptor
                .position_y
                .as_ref()
                .map(|axis| utils::range_to_old(&axis.range)),
            wheel_v_range: utils::axis_to_old(mouse_input_descriptor.scroll_v.as_ref()),
            wheel_h_range: utils::axis_to_old(mouse_input_descriptor.scroll_h.as_ref()),
            buttons: mouse_input_descriptor.buttons,
        };

        Ok((device_descriptor, input_device_status))
    }

    /// Parses an [`InputReport`] into one or more [`InputEvent`]s.
    ///
    /// The [`InputEvent`]s are sent to the device binding owner via [`input_event_sender`].
    ///
    /// # Parameters
    /// `reports`: The incoming [`InputReport`].
    /// `previous_report`: The previous [`InputReport`] seen for the same device. This can be
    ///                    used to determine, for example, which keys are no longer present in
    ///                    a keyboard report to generate key released events. If `None`, no
    ///                    previous report was found.
    /// `device_descriptor`: The descriptor for the input device generating the input reports.
    /// `input_event_sender`: The sender for the device binding's input event stream.
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
        fuchsia_trace::duration!("input", "mouse-binding-process-reports", "num_reports" => reports.len());
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
        input_event_sender: &mut UnboundedSender<Vec<input_device::InputEvent>>,
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
        // Input devices can have multiple types so ensure `report` is a MouseInputReport.
        let mouse_report = match report.mouse() {
            Some(mouse) => mouse,
            None => {
                inspect_status.count_filtered_report();
                return previous_state;
            }
        };

        let previous_buttons: SortedVecSet<MouseButton> = match previous_state {
            Some(input_device::PreviousDeviceState::Mouse { pressed_buttons }) => pressed_buttons,
            _ => SortedVecSet::new(),
        };
        let current_buttons: SortedVecSet<MouseButton> =
            buttons_from_mouse_report_wire(mouse_report);

        // Send a Down event with:
        // * affected_buttons: the buttons that were pressed since the previous report,
        //   i.e. that are in the current report, but were not in the previous report.
        // * pressed_buttons: the full set of currently pressed buttons, including the
        //   recently pressed ones (affected_buttons).
        send_mouse_event(
            MouseLocation::Relative(Default::default()),
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            MousePhase::Down,
            current_buttons.difference(&previous_buttons).cloned().collect(),
            current_buttons.clone(),
            device_descriptor,
            input_event_sender,
            inspect_status,
            metrics_logger,
            wake_lease.as_ref().map(|lease| {
                lease
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("failed to duplicate event pair")
            }),
        );

        // Create a location for the move event. Use the absolute position if available.
        let location = if let (Some(position_x), Some(position_y)) =
            (mouse_report.position_x(), mouse_report.position_y())
        {
            MouseLocation::Absolute(Position { x: position_x.0 as f32, y: position_y.0 as f32 })
        } else {
            let movement_x = mouse_report.movement_x().map(|x| x.0).unwrap_or_default() as f32;
            let movement_y = mouse_report.movement_y().map(|y| y.0).unwrap_or_default() as f32;
            MouseLocation::Relative(RelativeLocation {
                counts: Position { x: movement_x, y: movement_y },
            })
        };

        // Send a Move event with buttons from both the current report and the previous report.
        // * affected_buttons and pressed_buttons are identical in this case, since the full
        //   set of currently pressed buttons are the same set affected by the event.
        send_mouse_event(
            location,
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            MousePhase::Move,
            current_buttons.union(&previous_buttons).cloned().collect(),
            current_buttons.union(&previous_buttons).cloned().collect(),
            device_descriptor,
            input_event_sender,
            inspect_status,
            metrics_logger,
            wake_lease.as_ref().map(|lease| {
                lease
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("failed to duplicate event pair")
            }),
        );

        let wheel_delta_v = mouse_report
            .scroll_v()
            .map(|ticks| WheelDelta { ticks: ticks.0, physical_pixel: None });

        let wheel_delta_h = mouse_report
            .scroll_h()
            .map(|ticks| WheelDelta { ticks: ticks.0, physical_pixel: None });

        // Send a mouse wheel event.
        send_mouse_event(
            MouseLocation::Relative(Default::default()),
            wheel_delta_v,
            wheel_delta_h,
            MousePhase::Wheel,
            current_buttons.union(&previous_buttons).cloned().collect(),
            current_buttons.union(&previous_buttons).cloned().collect(),
            device_descriptor,
            input_event_sender,
            inspect_status,
            metrics_logger,
            wake_lease.as_ref().map(|lease| {
                lease
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("failed to duplicate event pair")
            }),
        );

        // Send an Up event with:
        // * affected_buttons: the buttons that were released since the previous report,
        //   i.e. that were in the previous report, but are not in the current report.
        // * pressed_buttons: the full set of currently pressed buttons, excluding the
        //   recently released ones (affected_buttons).
        send_mouse_event(
            MouseLocation::Relative(Default::default()),
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            MousePhase::Up,
            previous_buttons.difference(&current_buttons).cloned().collect(),
            current_buttons.clone(),
            device_descriptor,
            input_event_sender,
            inspect_status,
            metrics_logger,
            wake_lease,
        );

        Some(input_device::PreviousDeviceState::Mouse { pressed_buttons: current_buttons })
    }
}

/// Sends an InputEvent over `sender`.
///
/// When no buttons are present, only [`MousePhase::Move`] events will
/// be sent.
///
/// # Parameters
/// - `location`: The mouse location.
/// - `wheel_delta_v`: The mouse wheel delta in vertical.
/// - `wheel_delta_h`: The mouse wheel delta in horizontal.
/// - `phase`: The phase of the [`buttons`] associated with the input event.
/// - `buttons`: The buttons relevant to the event.
/// - `device_descriptor`: The descriptor for the input device generating the input reports.
/// - `sender`: The stream to send the MouseEvent to.
/// - `wake_lease`: The wake lease to send with the event.
fn send_mouse_event(
    location: MouseLocation,
    wheel_delta_v: Option<WheelDelta>,
    wheel_delta_h: Option<WheelDelta>,
    phase: MousePhase,
    affected_buttons: SortedVecSet<MouseButton>,
    pressed_buttons: SortedVecSet<MouseButton>,
    device_descriptor: &input_device::InputDeviceDescriptor,
    sender: &mut UnboundedSender<Vec<input_device::InputEvent>>,
    inspect_status: &InputDeviceStatus,
    metrics_logger: &metrics::MetricsLogger,
    wake_lease: Option<zx::EventPair>,
) {
    // Only send Down/Up events when there are buttons affected.
    if (phase == MousePhase::Down || phase == MousePhase::Up) && affected_buttons.is_empty() {
        return;
    }

    // Don't send Move events when there is no relative movement.
    // However, absolute movement is always reported.
    if phase == MousePhase::Move && location == MouseLocation::Relative(Default::default()) {
        return;
    }

    // Only send wheel events when the delta has value.
    if phase == MousePhase::Wheel && wheel_delta_v.is_none() && wheel_delta_h.is_none() {
        return;
    }

    let trace_id = fuchsia_trace::Id::new();
    fuchsia_trace::duration!("input", "mouse-binding-send-event");
    fuchsia_trace::flow_begin!("input", "event_in_input_pipeline", trace_id);

    let event = input_device::InputEvent {
        device_event: input_device::InputDeviceEvent::Mouse(MouseEvent::new(
            location,
            wheel_delta_v,
            wheel_delta_h,
            phase,
            affected_buttons,
            pressed_buttons,
            match phase {
                MousePhase::Wheel => Some(PrecisionScroll::No),
                _ => None,
            },
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
            InputPipelineErrorMetricDimensionEvent::MouseFailedToSendEvent,
            std::format!("Failed to send MouseEvent with error: {:?}", e),
        );
    }
}

fn buttons_from_mouse_report_wire(
    mouse_report: &fidl_next_fuchsia_input_report::wire::MouseInputReport<'_>,
) -> SortedVecSet<MouseButton> {
    mouse_report
        .pressed_buttons()
        .map(|buttons| SortedVecSet::from_iter(buttons.iter().copied()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing_utilities;
    use futures::StreamExt;
    use sorted_vec_map::SortedVecSet;

    const DEVICE_ID: u32 = 1;

    fn mouse_device_descriptor(device_id: u32) -> input_device::InputDeviceDescriptor {
        input_device::InputDeviceDescriptor::Mouse(MouseDeviceDescriptor {
            device_id,
            absolute_x_range: None,
            absolute_y_range: None,
            wheel_v_range: Some(fidl_fuchsia_input::Axis {
                range: fidl_fuchsia_input::Range { min: -1, max: 1 },
                unit: fidl_fuchsia_input::Unit {
                    type_: fidl_fuchsia_input::UnitType::Other,
                    exponent: 1,
                },
            }),
            wheel_h_range: Some(fidl_fuchsia_input::Axis {
                range: fidl_fuchsia_input::Range { min: -1, max: 1 },
                unit: fidl_fuchsia_input::Unit {
                    type_: fidl_fuchsia_input::UnitType::Other,
                    exponent: 1,
                },
            }),
            buttons: None,
        })
    }

    fn wheel_delta_ticks(ticks: i64) -> Option<WheelDelta> {
        Some(WheelDelta { ticks, physical_pixel: None })
    }

    /// Tests that a report containing no buttons but with movement generates a move event.
    #[fuchsia::test]
    async fn movement_without_button() {
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position { x: 10.0, y: 16.0 },
            None, /* scroll_v */
            None, /* scroll_h */
            vec![],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report];
        let expected_events = vec![testing_utilities::create_mouse_event(
            MouseLocation::Relative(RelativeLocation { counts: Position { x: 10.0, y: 16.0 } }),
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            None, /* is_precision_scroll */
            MousePhase::Move,
            SortedVecSet::new(),
            SortedVecSet::new(),
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that a report containing a new mouse button generates a down event.
    #[fuchsia::test]
    async fn down_without_movement() {
        let mouse_button: MouseButton = 3;
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![mouse_button],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report];
        let expected_events = vec![testing_utilities::create_mouse_event(
            MouseLocation::Relative(Default::default()),
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            None, /* is_precision_scroll */
            MousePhase::Down,
            SortedVecSet::from(vec![mouse_button]),
            SortedVecSet::from(vec![mouse_button]),
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that a report containing a new mouse button with movement generates a down event and a
    /// move event.
    #[fuchsia::test]
    async fn down_with_movement() {
        let mouse_button: MouseButton = 3;
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position { x: 10.0, y: 16.0 },
            None, /* scroll_v */
            None, /* scroll_h */
            vec![mouse_button],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![mouse_button]),
                SortedVecSet::from(vec![mouse_button]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(RelativeLocation { counts: Position { x: 10.0, y: 16.0 } }),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Move,
                SortedVecSet::from(vec![mouse_button]),
                SortedVecSet::from(vec![mouse_button]),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that a press and release of a mouse button without movement generates a down and up event.
    #[fuchsia::test]
    async fn down_up() {
        let button = 1;
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![button],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![button]),
                SortedVecSet::from(vec![button]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Up,
                SortedVecSet::from(vec![button]),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that a press and release of a mouse button with movement generates down, move, and up events.
    #[fuchsia::test]
    async fn down_up_with_movement() {
        let button = 1;

        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![button],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position { x: 10.0, y: 16.0 },
            None, /* scroll_v */
            None, /* scroll_h */
            vec![],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![button]),
                SortedVecSet::from(vec![button]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(RelativeLocation { counts: Position { x: 10.0, y: 16.0 } }),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Move,
                SortedVecSet::from(vec![button]),
                SortedVecSet::from(vec![button]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Up,
                SortedVecSet::from(vec![button]),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that a press, move, and release of a button generates down, move, and up events.
    /// This specifically tests the separate input report containing the movement, instead of sending
    /// the movement as part of the down or up events.
    #[fuchsia::test]
    async fn down_move_up() {
        let button = 1;

        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![button],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position { x: 10.0, y: 16.0 },
            None, /* scroll_v */
            None, /* scroll_h */
            vec![button],
            event_time_i64,
        );
        let third_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report, third_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![button]),
                SortedVecSet::from(vec![button]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(RelativeLocation { counts: Position { x: 10.0, y: 16.0 } }),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Move,
                SortedVecSet::from(vec![button]),
                SortedVecSet::from(vec![button]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Up,
                SortedVecSet::from(vec![button]),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that a report with absolute movement to {0, 0} generates a move event.
    #[fuchsia::test(allow_stalls = false)]
    async fn absolute_movement_to_origin() {
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![testing_utilities::create_mouse_input_report_absolute(
            Position::zero(),
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            vec![],
            event_time_i64,
        )];
        let expected_events = vec![testing_utilities::create_mouse_event(
            MouseLocation::Absolute(Position { x: 0.0, y: 0.0 }),
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            None, /* is_precision_scroll */
            MousePhase::Move,
            SortedVecSet::new(),
            SortedVecSet::new(),
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that a report that contains both a relative movement and absolute position
    /// generates a move event to the absolute position.
    #[fuchsia::test(allow_stalls = false)]
    async fn report_with_both_movement_and_position() {
        let relative_movement = Position { x: 5.0, y: 5.0 };
        let absolute_position = Position { x: 10.0, y: 10.0 };
        let expected_location = MouseLocation::Absolute(absolute_position);

        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![fidl_next_fuchsia_input_report::InputReport {
            event_time: Some(event_time_i64),
            keyboard: None,
            mouse: Some(fidl_next_fuchsia_input_report::MouseInputReport {
                movement_x: Some(relative_movement.x as i64),
                movement_y: Some(relative_movement.y as i64),
                position_x: Some(absolute_position.x as i64),
                position_y: Some(absolute_position.y as i64),
                scroll_h: None,
                scroll_v: None,
                pressed_buttons: None,
                ..Default::default()
            }),
            touch: None,
            sensor: None,
            consumer_control: None,
            trace_id: None,
            ..Default::default()
        }];
        let expected_events = vec![testing_utilities::create_mouse_event(
            expected_location,
            None, /* wheel_delta_v */
            None, /* wheel_delta_h */
            None, /* is_precision_scroll */
            MousePhase::Move,
            SortedVecSet::new(),
            SortedVecSet::new(),
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that two separate button presses generate two separate down events with differing
    /// sets of `affected_buttons` and `pressed_buttons`.
    #[fuchsia::test]
    async fn down_down() {
        const PRIMARY_BUTTON: u8 = 1;
        const SECONDARY_BUTTON: u8 = 2;

        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![PRIMARY_BUTTON],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![PRIMARY_BUTTON, SECONDARY_BUTTON],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![SECONDARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON, SECONDARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Tests that two staggered button presses followed by stagged releases generate four mouse
    /// events with distinct `affected_buttons` and `pressed_buttons`.
    /// Specifically, we test and expect the following in order:
    /// | Action           | MousePhase | `affected_buttons` | `pressed_buttons` |
    /// | ---------------- | ---------- | ------------------ | ----------------- |
    /// | Press button 1   | Down       | [1]                | [1]               |
    /// | Press button 2   | Down       | [2]                | [1, 2]            |
    /// | Release button 1 | Up         | [1]                | [2]               |
    /// | Release button 2 | Up         | [2]                | []                |
    #[fuchsia::test]
    async fn down_down_up_up() {
        const PRIMARY_BUTTON: u8 = 1;
        const SECONDARY_BUTTON: u8 = 2;

        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![PRIMARY_BUTTON],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![PRIMARY_BUTTON, SECONDARY_BUTTON],
            event_time_i64,
        );
        let third_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![SECONDARY_BUTTON],
            event_time_i64,
        );
        let fourth_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![],
            event_time_i64,
        );
        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report, third_report, fourth_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![SECONDARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON, SECONDARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Up,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![SECONDARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Up,
                SortedVecSet::from(vec![SECONDARY_BUTTON]),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Test simple scroll in vertical and horizontal.
    #[fuchsia::test]
    async fn scroll() {
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            Some(1),
            None,
            vec![],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None,
            Some(1),
            vec![],
            event_time_i64,
        );

        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                wheel_delta_ticks(1),
                None,
                Some(PrecisionScroll::No),
                MousePhase::Wheel,
                SortedVecSet::new(),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None,
                wheel_delta_ticks(1),
                Some(PrecisionScroll::No),
                MousePhase::Wheel,
                SortedVecSet::new(),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Test button down -> scroll -> button up -> continue scroll.
    #[fuchsia::test]
    async fn down_scroll_up_scroll() {
        const PRIMARY_BUTTON: u8 = 1;

        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![PRIMARY_BUTTON],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            Some(1),
            None,
            vec![PRIMARY_BUTTON],
            event_time_i64,
        );
        let third_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            None, /* scroll_v */
            None, /* scroll_h */
            vec![],
            event_time_i64,
        );
        let fourth_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            Some(1),
            None,
            vec![],
            event_time_i64,
        );

        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report, third_report, fourth_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                wheel_delta_ticks(1),
                None,
                Some(PrecisionScroll::No),
                MousePhase::Wheel,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Up,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                wheel_delta_ticks(1),
                None,
                Some(PrecisionScroll::No),
                MousePhase::Wheel,
                SortedVecSet::new(),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }

    /// Test button down with scroll -> button up with scroll -> scroll.
    #[fuchsia::test]
    async fn down_scroll_bundle_up_scroll_bundle() {
        const PRIMARY_BUTTON: u8 = 1;

        let (event_time_i64, event_time_u64) = testing_utilities::event_times();
        let first_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            Some(1),
            None,
            vec![PRIMARY_BUTTON],
            event_time_i64,
        );
        let second_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            Some(1),
            None,
            vec![],
            event_time_i64,
        );
        let third_report = testing_utilities::create_mouse_input_report_relative(
            Position::zero(),
            Some(1),
            None,
            vec![],
            event_time_i64,
        );

        let descriptor = mouse_device_descriptor(DEVICE_ID);

        let input_reports = vec![first_report, second_report, third_report];
        let expected_events = vec![
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Down,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                wheel_delta_ticks(1),
                None,
                Some(PrecisionScroll::No),
                MousePhase::Wheel,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                wheel_delta_ticks(1),
                None,
                Some(PrecisionScroll::No),
                MousePhase::Wheel,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                None, /* is_precision_scroll */
                MousePhase::Up,
                SortedVecSet::from(vec![PRIMARY_BUTTON]),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
            testing_utilities::create_mouse_event(
                MouseLocation::Relative(Default::default()),
                wheel_delta_ticks(1),
                None,
                Some(PrecisionScroll::No),
                MousePhase::Wheel,
                SortedVecSet::new(),
                SortedVecSet::new(),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: input_reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: MouseBinding,
        );
    }
}
