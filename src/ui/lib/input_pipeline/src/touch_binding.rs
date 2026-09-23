// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device::{self, Handled, InputDeviceStatus, InputEvent};
use crate::utils::{self, Position, Size};
use crate::{Transport, metrics, mouse_binding};
use anyhow::{Context, Error, format_err};
use async_trait::async_trait;
use fuchsia_inspect::ArrayProperty;
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use zx;

use fidl_fuchsia_ui_input as fidl_ui_input;
use fidl_next_fuchsia_ui_pointerinjector as pointerinjector;

use metrics_registry::*;
use sorted_vec_map::{SortedVecMap, SortedVecSet};

/// A [`TouchScreenEvent`] represents a set of contacts and the phase those contacts are in.
///
/// For example, when a user touches a touch screen with two fingers, there will be two
/// [`TouchContact`]s. When a user removes one finger, there will still be two contacts
/// but one will be reported as removed.
///
/// The expected sequence for any given contact is:
/// 1. [`fidl_fuchsia_ui_input::PointerEventPhase::Add`]
/// 2. [`fidl_fuchsia_ui_input::PointerEventPhase::Down`]
/// 3. 0 or more [`fidl_fuchsia_ui_input::PointerEventPhase::Move`]
/// 4. [`fidl_fuchsia_ui_input::PointerEventPhase::Up`]
/// 5. [`fidl_fuchsia_ui_input::PointerEventPhase::Remove`]
///
/// Additionally, a [`fidl_fuchsia_ui_input::PointerEventPhase::Cancel`] may be sent at any time
/// signalling that the event is no longer directed towards the receiver.
#[derive(Debug, PartialEq)]
pub struct TouchScreenEvent {
    /// Deprecated. To be removed with https://fxbug.dev/42155652.
    /// The contacts associated with the touch event. For example, a two-finger touch would result
    /// in one touch event with two [`TouchContact`]s.
    ///
    /// Contacts are grouped based on their current phase (e.g., down, move).
    pub contacts: SortedVecMap<fidl_ui_input::PointerEventPhase, Vec<TouchContact>>,

    /// The contacts associated with the touch event. For example, a two-finger touch would result
    /// in one touch event with two [`TouchContact`]s.
    ///
    /// Contacts are grouped based on their current phase (e.g., add, change).
    pub injector_contacts: SortedVecMap<pointerinjector::EventPhase, Vec<TouchContact>>,

    /// Indicates whether any touch buttons are pressed.
    pub pressed_buttons: Vec<fidl_next_fuchsia_input_report::TouchButton>,

    /// The wake lease for this event.
    pub wake_lease: Option<zx::EventPair>,
}

impl Clone for TouchScreenEvent {
    fn clone(&self) -> Self {
        log::debug!("TouchScreenEvent cloned without wake lease.");
        Self {
            contacts: self.contacts.clone(),
            injector_contacts: self.injector_contacts.clone(),
            pressed_buttons: self.pressed_buttons.clone(),
            wake_lease: None,
        }
    }
}

impl Drop for TouchScreenEvent {
    fn drop(&mut self) {
        log::debug!("TouchScreenEvent dropped, had_wake_lease: {:?}", self.wake_lease);
    }
}

impl TouchScreenEvent {
    pub fn record_inspect(&self, node: &fuchsia_inspect::Node) {
        let contacts_clone = self.injector_contacts.clone();
        node.record_child("injector_contacts", move |contacts_node| {
            for (phase, contacts) in contacts_clone.iter() {
                let phase_str = match pointerinjector::EventPhase::try_from(*phase) {
                    Ok(pointerinjector::EventPhase::Add) => "add",
                    Ok(pointerinjector::EventPhase::Change) => "change",
                    Ok(pointerinjector::EventPhase::Remove) => "remove",
                    Ok(pointerinjector::EventPhase::Cancel) => "cancel",
                    Err(_) => unreachable!("invalid phase"),
                };
                contacts_node.record_child(phase_str, move |phase_node| {
                    for contact in contacts.iter() {
                        phase_node.record_child(contact.id.to_string(), move |contact_node| {
                            if let Some(pressure) = contact.pressure {
                                contact_node.record_int("pressure", pressure);
                            }
                            if let Some(contact_size) = contact.contact_size {
                                contact_node.record_double(
                                    "contact_width_mm",
                                    f64::from(contact_size.width),
                                );
                                contact_node.record_double(
                                    "contact_height_mm",
                                    f64::from(contact_size.height),
                                );
                            }
                        });
                    }
                });
            }
        });

        let pressed_buttons_node =
            node.create_string_array("pressed_buttons", self.pressed_buttons.len());
        self.pressed_buttons.iter().enumerate().for_each(|(i, &ref button)| {
            let button_name: String = match button {
                fidl_next_fuchsia_input_report::TouchButton::Palm => "palm".into(),
                unknown_value => {
                    format!("unknown({:?})", unknown_value)
                }
            };
            pressed_buttons_node.set(i, &button_name);
        });
        node.record(pressed_buttons_node);
    }
}

/// A [`TouchpadEvent`] represents a set of contacts.
///
/// For example, when a user touches a touch screen with two fingers, there will be two
/// [`TouchContact`]s in the vector.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchpadEvent {
    /// The contacts associated with the touch event. For example, a two-finger touch would result
    /// in one touch event with two [`TouchContact`]s.
    pub injector_contacts: Vec<TouchContact>,

    /// The complete button state including this event.
    pub pressed_buttons: SortedVecSet<mouse_binding::MouseButton>,
}

impl TouchpadEvent {
    pub fn record_inspect(&self, node: &fuchsia_inspect::Node) {
        let pressed_buttons_node =
            node.create_uint_array("pressed_buttons", self.pressed_buttons.len());
        self.pressed_buttons.iter().enumerate().for_each(|(i, button)| {
            pressed_buttons_node.set(i, *button);
        });
        node.record(pressed_buttons_node);

        // Populate TouchpadEvent contact details.
        let contacts_clone = self.injector_contacts.clone();
        node.record_child("injector_contacts", move |contacts_node| {
            for contact in contacts_clone.iter() {
                contacts_node.record_child(contact.id.to_string(), move |contact_node| {
                    if let Some(pressure) = contact.pressure {
                        contact_node.record_int("pressure", pressure);
                    }
                    if let Some(contact_size) = contact.contact_size {
                        contact_node
                            .record_double("contact_width_mm", f64::from(contact_size.width));
                        contact_node
                            .record_double("contact_height_mm", f64::from(contact_size.height));
                    }
                })
            }
        });
    }
}

/// [`TouchDeviceType`] indicates the type of touch device. Both Touch Screen and Windows Precision
/// Touchpad send touch event from driver but need different process inside input pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TouchDeviceType {
    TouchScreen,
    WindowsPrecisionTouchpad,
}

/// A [`TouchContact`] represents a single contact (e.g., one touch of a multi-touch gesture) related
/// to a touch event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchContact {
    /// The identifier of the contact. Unique per touch device.
    pub id: u32,

    /// The position of the touch event, in the units of the associated
    /// [`ContactDeviceDescriptor`]'s `range`.
    pub position: Position,

    /// The pressure associated with the contact, in the units of the associated
    /// [`ContactDeviceDescriptor`]'s `pressure_range`.
    pub pressure: Option<i64>,

    /// The size of the touch event, in the units of the associated
    /// [`ContactDeviceDescriptor`]'s `range`.
    pub contact_size: Option<Size>,
}

impl Eq for TouchContact {}

impl TryFrom<&fidl_next_fuchsia_input_report::ContactInputReport> for TouchContact {
    type Error = anyhow::Error;

    fn try_from(
        fidl_contact: &fidl_next_fuchsia_input_report::ContactInputReport,
    ) -> anyhow::Result<TouchContact> {
        let contact_size =
            if fidl_contact.contact_width.is_some() && fidl_contact.contact_height.is_some() {
                Some(Size {
                    width: fidl_contact.contact_width.unwrap() as f32,
                    height: fidl_contact.contact_height.unwrap() as f32,
                })
            } else {
                None
            };

        let id = fidl_contact.contact_id.context("contact_id is required")?;
        let position_x = fidl_contact.position_x.context("position_x is required")?;
        let position_y = fidl_contact.position_y.context("position_y is required")?;

        Ok(TouchContact {
            id,
            position: Position { x: position_x as f32, y: position_y as f32 },
            pressure: fidl_contact.pressure,
            contact_size,
        })
    }
}

impl TryFrom<&fidl_next_fuchsia_input_report::wire::ContactInputReport<'_>> for TouchContact {
    type Error = anyhow::Error;

    fn try_from(
        fidl_contact: &fidl_next_fuchsia_input_report::wire::ContactInputReport<'_>,
    ) -> Result<Self, Self::Error> {
        let contact_size =
            if fidl_contact.contact_width().is_some() && fidl_contact.contact_height().is_some() {
                Some(Size {
                    width: fidl_contact.contact_width().map(|w| w.0).unwrap() as f32,
                    height: fidl_contact.contact_height().map(|h| h.0).unwrap() as f32,
                })
            } else {
                None
            };

        let id = fidl_contact.contact_id().map(|id| id.0).context("contact_id is required")?;
        let position_x =
            fidl_contact.position_x().map(|x| x.0).context("position_x is required")?;
        let position_y =
            fidl_contact.position_y().map(|y| y.0).context("position_y is required")?;

        Ok(TouchContact {
            id,
            position: Position { x: position_x as f32, y: position_y as f32 },
            pressure: fidl_contact.pressure().map(|p| p.0),
            contact_size,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TouchScreenDeviceDescriptor {
    /// The id of the connected touch screen input device.
    pub device_id: u32,

    /// The descriptors for the possible contacts associated with the device.
    pub contacts: Vec<ContactDeviceDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TouchpadDeviceDescriptor {
    /// The id of the connected touchpad input device.
    pub device_id: u32,

    /// The descriptors for the possible contacts associated with the device.
    pub contacts: Vec<ContactDeviceDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TouchDeviceDescriptor {
    TouchScreen(TouchScreenDeviceDescriptor),
    Touchpad(TouchpadDeviceDescriptor),
}

/// A [`ContactDeviceDescriptor`] describes the possible values touch contact properties can take on.
///
/// This descriptor can be used, for example, to determine where on a screen a touch made contact.
///
/// # Example
///
/// ```
/// // Determine the scaling factor between the display and the touch device's x range.
/// let scaling_factor =
///     display_width / (contact_descriptor._x_range.end - contact_descriptor._x_range.start);
/// // Use the scaling factor to scale the contact report's x position.
/// let hit_location =
///     scaling_factor * (contact_report.position_x - contact_descriptor._x_range.start);
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContactDeviceDescriptor {
    /// The range of possible x values for this touch contact.
    pub x_range: fidl_fuchsia_input::Range,

    /// The range of possible y values for this touch contact.
    pub y_range: fidl_fuchsia_input::Range,

    /// The unit of measure for `x_range`.
    pub x_unit: fidl_fuchsia_input::Unit,

    /// The unit of measure for `y_range`.
    pub y_unit: fidl_fuchsia_input::Unit,

    /// The range of possible pressure values for this touch contact.
    pub pressure_range: Option<fidl_fuchsia_input::Range>,

    /// The range of possible widths for this touch contact.
    pub width_range: Option<fidl_fuchsia_input::Range>,

    /// The range of possible heights for this touch contact.
    pub height_range: Option<fidl_fuchsia_input::Range>,
}

/// A [`TouchBinding`] represents a connection to a touch input device.
///
/// The [`TouchBinding`] parses and exposes touch descriptor properties (e.g., the range of
/// possible x values for touch contacts) for the device it is associated with.
/// It also parses [`InputReport`]s from the device, and sends them to the device binding owner over
/// `event_sender`.
pub struct TouchBinding {
    /// The channel to stream InputEvents to.
    event_sender: UnboundedSender<Vec<InputEvent>>,

    /// Holds information about this device.
    device_descriptor: TouchDeviceDescriptor,
}

#[async_trait]
impl input_device::InputDeviceBinding for TouchBinding {
    fn input_event_sender(&self) -> UnboundedSender<Vec<InputEvent>> {
        self.event_sender.clone()
    }

    fn get_device_descriptor(&self) -> input_device::InputDeviceDescriptor {
        match self.device_descriptor.clone() {
            TouchDeviceDescriptor::TouchScreen(desc) => {
                input_device::InputDeviceDescriptor::TouchScreen(desc)
            }
            TouchDeviceDescriptor::Touchpad(desc) => {
                input_device::InputDeviceDescriptor::Touchpad(desc)
            }
        }
    }
}

impl TouchBinding {
    /// Creates a new [`InputDeviceBinding`] from the `device_proxy`.
    ///
    /// The binding will start listening for input reports immediately and send new InputEvents
    /// to the device binding owner over `input_event_sender`.
    ///
    /// # Parameters
    /// - `device_proxy`: The proxy to bind the new [`InputDeviceBinding`] to.
    /// - `device_id`: The id of the connected touch device.
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
    ) -> Result<(Self, crate::dispatcher::TaskHandle<()>), Error> {
        let (device_descriptor, touch_device_type, mut inspect_status) =
            Self::bind_device(&device_proxy, device_id, device_node).await?;
        Self::set_touchpad_mode(&device_proxy, touch_device_type, true)
            .await
            .with_context(|| format!("enabling touchpad mode for device {}", device_id))?;
        inspect_status.health_node.set_ok();
        let task = input_device::initialize_report_stream(
            device_proxy.clone(),
            match device_descriptor.clone() {
                TouchDeviceDescriptor::TouchScreen(desc) => {
                    input_device::InputDeviceDescriptor::TouchScreen(desc)
                }
                TouchDeviceDescriptor::Touchpad(desc) => {
                    input_device::InputDeviceDescriptor::Touchpad(desc)
                }
            },
            input_event_sender.clone(),
            inspect_status,
            metrics_logger,
            feature_flags,
            Self::process_reports,
        );

        Ok((TouchBinding { event_sender: input_event_sender, device_descriptor }, task))
    }

    /// Binds the provided input device to a new instance of `Self`.
    ///
    /// # Parameters
    /// - `device`: The device to use to initialize the binding.
    /// - `device_id`: The id of the connected touch device.
    /// - `input_event_sender`: The channel to send new InputEvents to.
    /// - `device_node`: The inspect node for this device binding
    ///
    /// # Errors
    /// If the device descriptor could not be retrieved, or the descriptor could not be parsed
    /// correctly.
    async fn bind_device(
        device_proxy: &fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
        device_id: u32,
        device_node: fuchsia_inspect::Node,
    ) -> Result<(TouchDeviceDescriptor, TouchDeviceType, InputDeviceStatus), Error> {
        let mut input_device_status = InputDeviceStatus::new(device_node);
        let device_descriptor: fidl_next_fuchsia_input_report::DeviceDescriptor = match device_proxy
            .get_descriptor()
            .await
        {
            Ok(res) => res.descriptor,
            Err(_) => {
                input_device_status.health_node.set_unhealthy("Could not get device descriptor.");
                return Err(format_err!("Could not get descriptor for device_id: {}", device_id));
            }
        };

        let touch_device_type = get_device_type(device_proxy).await;

        match device_descriptor.touch {
            Some(fidl_next_fuchsia_input_report::TouchDescriptor {
                input:
                    Some(fidl_next_fuchsia_input_report::TouchInputDescriptor {
                        contacts: Some(contact_descriptors),
                        max_contacts: _,
                        touch_type: _,
                        buttons: _,
                        ..
                    }),
                ..
            }) => Ok((
                match touch_device_type {
                    TouchDeviceType::TouchScreen => {
                        TouchDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                            device_id,
                            contacts: contact_descriptors
                                .iter()
                                .map(TouchBinding::parse_contact_descriptor)
                                .filter_map(Result::ok)
                                .collect(),
                        })
                    }
                    TouchDeviceType::WindowsPrecisionTouchpad => {
                        TouchDeviceDescriptor::Touchpad(TouchpadDeviceDescriptor {
                            device_id,
                            contacts: contact_descriptors
                                .iter()
                                .map(TouchBinding::parse_contact_descriptor)
                                .filter_map(Result::ok)
                                .collect(),
                        })
                    }
                },
                touch_device_type,
                input_device_status,
            )),
            descriptor => {
                input_device_status
                    .health_node
                    .set_unhealthy("Touch Device Descriptor failed to parse.");
                Err(format_err!("Touch Descriptor failed to parse: \n {:?}", descriptor))
            }
        }
    }

    async fn set_touchpad_mode(
        device_proxy: &fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
        touch_device_type: TouchDeviceType,
        enable: bool,
    ) -> Result<(), Error> {
        match touch_device_type {
            TouchDeviceType::TouchScreen => Ok(()),
            TouchDeviceType::WindowsPrecisionTouchpad => {
                // `get_feature_report` to only modify the input_mode and
                // keep other feature as is.
                let mut report = match device_proxy.get_feature_report().await? {
                    Ok(res) => res.report,
                    Err(e) => return Err(format_err!("get_feature_report failed: {e:?}")),
                };
                let mut touch = report
                    .touch
                    .unwrap_or_else(fidl_next_fuchsia_input_report::TouchFeatureReport::default);
                touch.input_mode = match enable {
                            true => Some(fidl_next_fuchsia_input_report::TouchConfigurationInputMode::WindowsPrecisionTouchpadCollection),
                            false => Some(fidl_next_fuchsia_input_report::TouchConfigurationInputMode::MouseCollection),
                        };
                report.touch = Some(touch);
                match device_proxy.set_feature_report(&report).await? {
                    Ok(_) => {
                        // TODO(https://fxbug.dev/42056283): Remove log message.
                        log::info!("touchpad: set touchpad_enabled to {}", enable);
                        Ok(())
                    }
                    Err(e) => Err(format_err!("set_feature_report failed: {e:?}")),
                }
            }
        }
    }

    /// Parses an [`InputReport`] into one or more [`InputEvent`]s.
    ///
    /// The [`InputEvent`]s are sent to the device binding owner via [`input_event_sender`].
    ///
    /// # Parameters
    /// - `reports`: The incoming [`InputReport`].
    /// - `previous_report`: The previous [`InputReport`] seen for the same device. This can be
    ///                    used to determine, for example, which keys are no longer present in
    ///                    a keyboard report to generate key released events. If `None`, no
    ///                    previous report was found.
    /// - `device_descriptor`: The descriptor for the input device generating the input reports.
    /// - `input_event_sender`: The sender for the device binding's input event stream.
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
        previous_state: Option<input_device::PreviousDeviceState>,
        device_descriptor: &input_device::InputDeviceDescriptor,
        input_event_sender: &mut UnboundedSender<Vec<InputEvent>>,
        inspect_status: &InputDeviceStatus,
        metrics_logger: &metrics::MetricsLogger,
        feature_flags: &input_device::InputPipelineFeatureFlags,
    ) -> (Option<input_device::PreviousDeviceState>, Option<UnboundedReceiver<InputEvent>>) {
        fuchsia_trace::duration!(
            "input",
            "touch-binding-process-report",
            "num_reports" => reports.len(),
        );
        match device_descriptor {
            input_device::InputDeviceDescriptor::TouchScreen(_) => process_touch_screen_reports(
                reports,
                previous_state,
                device_descriptor,
                input_event_sender,
                inspect_status,
                metrics_logger,
                feature_flags.enable_merge_touch_events,
            ),
            input_device::InputDeviceDescriptor::Touchpad(_) => {
                // TODO(b/512925135): support touchpad in starnix
                (previous_state, None)
            }
            _ => (previous_state, None),
        }
    }

    /// Parses a fidl_fuchsia_input_report contact descriptor into a [`ContactDeviceDescriptor`]
    ///
    /// # Parameters
    /// - `contact_device_descriptor`: The contact descriptor to parse.
    ///
    /// # Errors
    /// If the contact description fails to parse because required fields aren't present.
    fn parse_contact_descriptor(
        contact_device_descriptor: &fidl_next_fuchsia_input_report::ContactInputDescriptor,
    ) -> Result<ContactDeviceDescriptor, Error> {
        match contact_device_descriptor {
            fidl_next_fuchsia_input_report::ContactInputDescriptor {
                position_x: Some(x_axis),
                position_y: Some(y_axis),
                pressure: pressure_axis,
                contact_width: width_axis,
                contact_height: height_axis,
                ..
            } => Ok(ContactDeviceDescriptor {
                x_range: utils::range_to_old(&x_axis.range),
                y_range: utils::range_to_old(&y_axis.range),
                x_unit: utils::unit_to_old(&x_axis.unit),
                y_unit: utils::unit_to_old(&y_axis.unit),
                pressure_range: pressure_axis.as_ref().map(|axis| utils::range_to_old(&axis.range)),
                width_range: width_axis.as_ref().map(|axis| utils::range_to_old(&axis.range)),
                height_range: height_axis.as_ref().map(|axis| utils::range_to_old(&axis.range)),
            }),
            descriptor => {
                Err(format_err!("Touch Contact Descriptor failed to parse: \n {:?}", descriptor))
            }
        }
    }
}

fn is_move_only(event: &InputEvent) -> bool {
    matches!(
        &event.device_event,
        input_device::InputDeviceEvent::TouchScreen(event)
            if event
                .injector_contacts
                .get(&pointerinjector::EventPhase::Add)
                .map_or(true, |c| c.is_empty())
                && event
                    .injector_contacts
                    .get(&pointerinjector::EventPhase::Remove)
                    .map_or(true, |c| c.is_empty())
                && event
                    .injector_contacts
                    .get(&pointerinjector::EventPhase::Cancel)
                    .map_or(true, |c| c.is_empty())
    )
}

fn has_pressed_buttons(event: &InputEvent) -> bool {
    match &event.device_event {
        input_device::InputDeviceEvent::TouchScreen(event) => !event.pressed_buttons.is_empty(),
        _ => false,
    }
}

fn process_touch_screen_reports(
    reports: &[fidl_next_fuchsia_input_report::wire::InputReport<'_>],
    mut previous_state: Option<input_device::PreviousDeviceState>,
    device_descriptor: &input_device::InputDeviceDescriptor,
    input_event_sender: &mut UnboundedSender<Vec<InputEvent>>,
    inspect_status: &InputDeviceStatus,
    metrics_logger: &metrics::MetricsLogger,
    enable_merge_touch_events: bool,
) -> (Option<input_device::PreviousDeviceState>, Option<UnboundedReceiver<InputEvent>>) {
    let num_reports = reports.len();
    let mut batch: Vec<InputEvent> = Vec::with_capacity(num_reports);
    for report in reports {
        inspect_status.count_received_report_wire(report);
        let (prev_state, event) = process_single_touch_screen_report(
            report,
            previous_state,
            device_descriptor,
            inspect_status,
            metrics_logger,
        );
        previous_state = prev_state;
        if let Some(event) = event {
            batch.push(event);
        }
    }

    if !batch.is_empty() {
        if enable_merge_touch_events {
            // Pre-calculate move-only status for all events
            let mut is_event_move_only: Vec<bool> = Vec::with_capacity(batch.len());
            let mut pressed_buttons: Vec<bool> = Vec::with_capacity(batch.len());
            for event in &batch {
                is_event_move_only.push(is_move_only(event));
                pressed_buttons.push(has_pressed_buttons(event));
            }
            let size_of_batch = batch.len();

            // Merge consecutive move-only events into a single event.
            let mut merged_batch = Vec::with_capacity(size_of_batch);

            // Use into_iter().enumerate() to move elements without cloning
            for (i, current_event) in batch.into_iter().enumerate() {
                let current_is_move = is_event_move_only[i];
                let current_pressed_buttons = pressed_buttons[i];
                let is_last_event = i == size_of_batch - 1;

                // Check if the NEXT event is also move-only
                let next_is_move =
                    if i + 1 < size_of_batch { is_event_move_only[i + 1] } else { false };

                let next_pressed_buttons = if i + 1 < size_of_batch {
                    pressed_buttons[i + 1]
                } else {
                    current_pressed_buttons
                };

                // If both are move-only, skip the current one (it's redundant).
                // always keep the last event
                if !is_last_event
                    // both are move-only
                    && (current_is_move && next_is_move)
                    // same pressed buttons
                    && (current_pressed_buttons == next_pressed_buttons)
                {
                    continue;
                }

                merged_batch.push(current_event);
            }

            batch = merged_batch;
        }

        let events_to_send: Vec<InputEvent> = {
            fuchsia_trace::duration!("input", "prepare_events_to_send");
            batch
                .into_iter()
                .map(|event| {
                    // Unwrap is safe because trace_id is set when the event is created.
                    // This unwrap will not move the trace_id out of the event because trace_id has
                    // Copy trait.
                    let trace_id: fuchsia_trace::Id = event.trace_id.unwrap();
                    fuchsia_trace::flow_begin!("input", "event_in_input_pipeline", trace_id);
                    event
                })
                .collect()
        };
        fuchsia_trace::instant!(
            "input",
            "events_to_input_handlers",
            fuchsia_trace::Scope::Thread,
            "num_reports" => num_reports,
            "num_events_generated" => events_to_send.len()
        );

        // Record inspect data before sending, as unbounded_send consumes the vector.
        inspect_status.count_generated_events(&events_to_send);

        if let Err(e) = input_event_sender.unbounded_send(events_to_send) {
            metrics_logger.log_error(
                InputPipelineErrorMetricDimensionEvent::TouchFailedToSendTouchScreenEvent,
                std::format!("Failed to send TouchScreenEvent with error: {:?}", e),
            );
        }
    }
    (previous_state, None)
}

fn process_single_touch_screen_report(
    report: &fidl_next_fuchsia_input_report::wire::InputReport<'_>,
    previous_state: Option<input_device::PreviousDeviceState>,
    device_descriptor: &input_device::InputDeviceDescriptor,
    inspect_status: &InputDeviceStatus,
    metrics_logger: &metrics::MetricsLogger,
) -> (Option<input_device::PreviousDeviceState>, Option<InputEvent>) {
    fuchsia_trace::flow_end!(
        "input",
        "input_report",
        report.trace_id().map(|x| x.0).unwrap_or(0).into()
    );

    // Extract the wake_lease early to prevent it from leaking. If this is moved
    // below an early return, the lease could accidentally be stored inside
    // `previous_report`, which would prevent the system from suspending.
    let wake_lease = utils::duplicate_wake_lease(report.wake_lease());

    // Input devices can have multiple types so ensure `report` is a TouchInputReport.
    let touch_report = match report.touch() {
        Some(touch) => touch,
        None => {
            inspect_status.count_filtered_report();
            return (previous_state, None);
        }
    };

    let (previous_contacts, previous_buttons): (
        SortedVecMap<u32, TouchContact>,
        Vec<fidl_next_fuchsia_input_report::TouchButton>,
    ) = match &previous_state {
        Some(input_device::PreviousDeviceState::TouchScreen {
            active_contacts,
            pressed_buttons,
        }) => {
            let contacts =
                SortedVecMap::from_iter(active_contacts.iter().map(|c| (c.id, c.clone())));
            (contacts, pressed_buttons.clone())
        }
        _ => (SortedVecMap::new(), vec![]),
    };
    let (current_contacts, current_buttons): (
        SortedVecMap<u32, TouchContact>,
        Vec<fidl_next_fuchsia_input_report::TouchButton>,
    ) = touch_contacts_and_buttons_from_touch_report_wire(touch_report, metrics_logger);

    if previous_contacts.is_empty()
        && current_contacts.is_empty()
        && previous_buttons.is_empty()
        && current_buttons.is_empty()
    {
        inspect_status.count_filtered_report();
        return (previous_state, None);
    }

    // Contacts which exist only in current.
    let added_contacts: Vec<TouchContact> = Vec::from_iter(
        current_contacts
            .iter()
            .map(|(_, v)| v.clone())
            .filter(|contact| !previous_contacts.contains_key(&contact.id)),
    );
    // Contacts which exist in both previous and current.
    let moved_contacts: Vec<TouchContact> = Vec::from_iter(
        current_contacts
            .iter()
            .map(|(_, v)| v.clone())
            .filter(|contact| previous_contacts.contains_key(&contact.id)),
    );
    // Contacts which exist only in previous.
    let removed_contacts: Vec<TouchContact> =
        Vec::from_iter(previous_contacts.iter().map(|(_, v)| v.clone()).filter(|contact| {
            current_buttons.is_empty()
                && previous_buttons.is_empty()
                && !current_contacts.contains_key(&contact.id)
        }));

    let active_contacts: Vec<TouchContact> = if current_contacts.is_empty()
        && !previous_contacts.is_empty()
        && (!current_buttons.is_empty() || !previous_buttons.is_empty())
    {
        previous_contacts.values().cloned().collect()
    } else {
        added_contacts.iter().chain(moved_contacts.iter()).cloned().collect()
    };

    let trace_id = fuchsia_trace::Id::new();
    let event = create_touch_screen_event(
        SortedVecMap::from_iter(vec![
            (fidl_ui_input::PointerEventPhase::Add, added_contacts.clone()),
            (fidl_ui_input::PointerEventPhase::Down, added_contacts.clone()),
            (fidl_ui_input::PointerEventPhase::Move, moved_contacts.clone()),
            (fidl_ui_input::PointerEventPhase::Up, removed_contacts.clone()),
            (fidl_ui_input::PointerEventPhase::Remove, removed_contacts.clone()),
        ]),
        SortedVecMap::from_iter(vec![
            (pointerinjector::EventPhase::Add, added_contacts),
            (pointerinjector::EventPhase::Change, moved_contacts),
            (pointerinjector::EventPhase::Remove, removed_contacts),
        ]),
        current_buttons.clone(),
        device_descriptor,
        trace_id,
        wake_lease,
    );

    let next_previous_state = input_device::PreviousDeviceState::TouchScreen {
        active_contacts,
        pressed_buttons: current_buttons,
    };

    (Some(next_previous_state), Some(event))
}

fn touch_contacts_and_buttons_from_touch_report_wire(
    touch_report: &fidl_next_fuchsia_input_report::wire::TouchInputReport<'_>,
    metrics_logger: &metrics::MetricsLogger,
) -> (SortedVecMap<u32, TouchContact>, Vec<fidl_next_fuchsia_input_report::TouchButton>) {
    let mut contacts = Vec::new();
    if let Some(unwrapped_contacts) = touch_report.contacts() {
        for contact in unwrapped_contacts.iter() {
            match TouchContact::try_from(contact) {
                Ok(c) => contacts.push(c),
                Err(e) => {
                    metrics_logger.log_warn(
                        InputPipelineErrorMetricDimensionEvent::TouchReportContactMissingField,
                        std::format!("failed to convert touch contact: {:?}", e),
                    );
                }
            }
        }
    } else {
        metrics_logger.log_warn(
            InputPipelineErrorMetricDimensionEvent::TouchReportMissingContact,
            "contacts missing in touch input report",
        );
    }

    let pressed_buttons = touch_report
        .pressed_buttons()
        .map(|buttons| buttons.iter().map(|&b| fidl_next::FromWire::from_wire(b)).collect())
        .unwrap_or_default();

    (
        SortedVecMap::from_iter(contacts.into_iter().map(|contact| (contact.id, contact))),
        pressed_buttons,
    )
}

/// Create a TouchScreenEvent.
///
/// # Parameters
/// - `contacts`: The contact points relevant to the new TouchScreenEvent.
/// - `injector_contacts`: The contact points relevant to the new TouchScreenEvent, used to send
///                        pointer events into Scenic.
/// - `device_descriptor`: The descriptor for the input device generating the input reports.
/// - `trace_id`: The trace id to distinguish the event.
/// - `wake_lease`: The wake lease to send with the event.
fn create_touch_screen_event(
    contacts: SortedVecMap<fidl_ui_input::PointerEventPhase, Vec<TouchContact>>,
    injector_contacts: SortedVecMap<pointerinjector::EventPhase, Vec<TouchContact>>,
    pressed_buttons: Vec<fidl_next_fuchsia_input_report::TouchButton>,
    device_descriptor: &input_device::InputDeviceDescriptor,
    trace_id: fuchsia_trace::Id,
    wake_lease: Option<zx::EventPair>,
) -> InputEvent {
    input_device::InputEvent {
        device_event: input_device::InputDeviceEvent::TouchScreen(TouchScreenEvent {
            contacts,
            injector_contacts,
            pressed_buttons,
            wake_lease,
        }),
        device_descriptor: device_descriptor.clone(),
        event_time: zx::MonotonicInstant::get(),
        handled: Handled::No,
        trace_id: Some(trace_id),
    }
}

/// [`get_device_type`] check if the touch device is a touchscreen or Windows Precision Touchpad.
///
/// Windows Precision Touchpad reports `MouseCollection` or `WindowsPrecisionTouchpadCollection`
/// in `TouchFeatureReport`. Fallback all error responses on `get_feature_report` to TouchScreen
/// because some touch screen does not report this method.
async fn get_device_type(
    input_device: &fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
) -> TouchDeviceType {
    match input_device.get_feature_report().await {
        Ok(Ok(fidl_next_fuchsia_input_report::InputDeviceGetFeatureReportResponse {
            report: fidl_next_fuchsia_input_report::FeatureReport {
                touch:
                    Some(fidl_next_fuchsia_input_report::TouchFeatureReport {
                        input_mode:
                            Some(
                                fidl_next_fuchsia_input_report::TouchConfigurationInputMode::MouseCollection
                                | fidl_next_fuchsia_input_report::TouchConfigurationInputMode::WindowsPrecisionTouchpadCollection,
                            ),
                        ..
                    }),
                ..
            }
        })) => TouchDeviceType::WindowsPrecisionTouchpad,
        _ => TouchDeviceType::TouchScreen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing_utilities::{
        self, create_touch_contact, create_touch_input_report, create_touch_screen_event,
        create_touch_screen_event_with_buttons, spawn_input_stream_handler,
    };
    use crate::utils::Position;
    use assert_matches::assert_matches;
    use diagnostics_assertions::AnyProperty;
    use futures::StreamExt;
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    #[fuchsia::test]
    async fn process_empty_reports() {
        let report_time = zx::MonotonicInstant::get().into_nanos();
        let report =
            create_touch_input_report(vec![], /* pressed_buttons= */ None, report_time);

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (mut event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();

        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("TestDevice_Touch");
        let mut inspect_status = InputDeviceStatus::new(test_node);
        inspect_status.health_node.set_ok();

        let previous_state = input_device::PreviousDeviceState::TouchScreen {
            active_contacts: vec![],
            pressed_buttons: vec![],
        };

        let reports_wire = crate::testing_utilities::reports_to_wire(vec![report]);
        let (returned_state, _) = TouchBinding::process_reports(
            &reports_wire,
            Some(previous_state),
            &descriptor,
            &mut event_sender,
            &inspect_status,
            &metrics::MetricsLogger::default(),
            &input_device::InputPipelineFeatureFlags::default(),
        );
        assert!(returned_state.is_some());
        assert_eq!(
            returned_state.unwrap(),
            input_device::PreviousDeviceState::TouchScreen {
                active_contacts: vec![],
                pressed_buttons: vec![]
            }
        );

        // Assert there are no pending events on the receiver.
        let event = event_receiver.try_next();
        assert!(event.is_err());

        diagnostics_assertions::assert_data_tree!(inspector, root: {
            "TestDevice_Touch": contains {
                reports_received_count: 1u64,
                reports_filtered_count: 1u64,
                events_generated: 0u64,
                last_received_timestamp_ns: report_time as u64,
                last_generated_timestamp_ns: 0u64,
                "fuchsia.inspect.Health": {
                    status: "OK",
                    // Timestamp value is unpredictable and not relevant in this context,
                    // so we only assert that the property is present.
                    start_timestamp_nanos: AnyProperty
                },
            }
        });
    }

    // Tests that a input report with a new contact generates an event with an add and a down.
    #[fuchsia::test]
    async fn add_and_down() {
        const TOUCH_ID: u32 = 2;

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();

        let contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            pressure: None,
            contact_width: None,
            contact_height: None,
            ..Default::default()
        };
        let reports = vec![create_touch_input_report(
            vec![contact],
            /* pressed_buttons= */ None,
            event_time_i64,
        )];

        let expected_events = vec![create_touch_screen_event(
            SortedVecMap::from_iter(vec![
                (
                    fidl_ui_input::PointerEventPhase::Add,
                    vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                ),
                (
                    fidl_ui_input::PointerEventPhase::Down,
                    vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                ),
            ]),
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
        );
    }

    // Tests that up and remove events are sent when a touch is released.
    #[fuchsia::test]
    async fn up_and_remove() {
        const TOUCH_ID: u32 = 2;

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();

        let contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            pressure: None,
            contact_width: None,
            contact_height: None,
            ..Default::default()
        };
        let reports = vec![
            create_touch_input_report(
                vec![contact],
                /* pressed_buttons= */ None,
                event_time_i64,
            ),
            create_touch_input_report(vec![], /* pressed_buttons= */ None, event_time_i64),
        ];

        let expected_events = vec![
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![
                    (
                        fidl_ui_input::PointerEventPhase::Add,
                        vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                    ),
                    (
                        fidl_ui_input::PointerEventPhase::Down,
                        vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                    ),
                ]),
                event_time_u64,
                &descriptor,
            ),
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![
                    (
                        fidl_ui_input::PointerEventPhase::Up,
                        vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                    ),
                    (
                        fidl_ui_input::PointerEventPhase::Remove,
                        vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                    ),
                ]),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
        );
    }

    // Tests that a move generates the correct event.
    #[fuchsia::test]
    async fn add_down_move() {
        const TOUCH_ID: u32 = 2;
        let first = Position { x: 10.0, y: 30.0 };
        let second = Position { x: first.x * 2.0, y: first.y * 2.0 };

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();

        let first_contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(first.x as i64),
            position_y: Some(first.y as i64),
            pressure: None,
            contact_width: None,
            contact_height: None,
            ..Default::default()
        };
        let second_contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(first.x as i64 * 2),
            position_y: Some(first.y as i64 * 2),
            pressure: None,
            contact_width: None,
            contact_height: None,
            ..Default::default()
        };

        let reports = vec![
            create_touch_input_report(
                vec![first_contact],
                /* pressed_buttons= */ None,
                event_time_i64,
            ),
            create_touch_input_report(
                vec![second_contact],
                /* pressed_buttons= */ None,
                event_time_i64,
            ),
        ];

        let expected_events = vec![
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![
                    (
                        fidl_ui_input::PointerEventPhase::Add,
                        vec![create_touch_contact(TOUCH_ID, first)],
                    ),
                    (
                        fidl_ui_input::PointerEventPhase::Down,
                        vec![create_touch_contact(TOUCH_ID, first)],
                    ),
                ]),
                event_time_u64,
                &descriptor,
            ),
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![(
                    fidl_ui_input::PointerEventPhase::Move,
                    vec![create_touch_contact(TOUCH_ID, second)],
                )]),
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
        );
    }

    #[fuchsia::test]
    async fn sent_event_has_trace_id() {
        let report_time = zx::MonotonicInstant::get().into_nanos();
        let contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(222),
            position_x: Some(333),
            position_y: Some(444),
            ..Default::default()
        };
        let report =
            create_touch_input_report(vec![contact], /* pressed_buttons= */ None, report_time);

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (mut event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();

        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("TestDevice_Touch");
        let mut inspect_status = InputDeviceStatus::new(test_node);
        inspect_status.health_node.set_ok();

        let previous_state = input_device::PreviousDeviceState::TouchScreen {
            active_contacts: vec![],
            pressed_buttons: vec![],
        };

        let reports_wire = crate::testing_utilities::reports_to_wire(vec![report]);
        let _ = TouchBinding::process_reports(
            &reports_wire,
            Some(previous_state),
            &descriptor,
            &mut event_sender,
            &inspect_status,
            &metrics::MetricsLogger::default(),
            &input_device::InputPipelineFeatureFlags::default(),
        );
        assert_matches!(event_receiver.try_next(), Ok(Some(events)) if events.len() == 1 && events[0].trace_id.is_some());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn enables_touchpad_mode_automatically() {
        let (set_feature_report_sender, set_feature_report_receiver) =
            futures::channel::mpsc::unbounded();
        let (input_device_proxy, _task) = spawn_input_stream_handler(move |input_device_request| {
            let set_feature_report_sender = set_feature_report_sender.clone();
            async move {
                match input_device_request {
                    fidl_fuchsia_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ = responder.send(&get_touchpad_device_descriptor(
                            true, /* has_mouse_descriptor */
                        ));
                    }
                    fidl_fuchsia_input_report::InputDeviceRequest::GetFeatureReport {
                        responder,
                    } => {
                        let _ = responder.send(Ok(&fidl_fuchsia_input_report::FeatureReport {
                            touch: Some(fidl_fuchsia_input_report::TouchFeatureReport {
                                input_mode: Some(
                                    fidl_fuchsia_input_report::TouchConfigurationInputMode::MouseCollection,
                                ),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }));
                    }
                    fidl_fuchsia_input_report::InputDeviceRequest::SetFeatureReport {
                        responder,
                        report,
                    } => {
                        match set_feature_report_sender.unbounded_send(report) {
                            Ok(_) => {
                                let _ = responder.send(Ok(()));
                            }
                            Err(e) => {
                                panic!("try_send set_feature_report_request failed: {}", e);
                            }
                        };
                    }
                    fidl_fuchsia_input_report::InputDeviceRequest::GetInputReportsReader {
                        ..
                    }
                    | fidl_fuchsia_input_report::InputDeviceRequest::GetInputReportsReaderV2 {
                        ..
                    } => {
                        // Do not panic as `initialize_report_stream()` will call this protocol.
                    }
                    r => panic!("unsupported request {:?}", r),
                }
            }
        });

        let (device_event_sender, _) = futures::channel::mpsc::unbounded();

        // Create a test inspect node as required by TouchBinding::new()
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");

        // Create a `TouchBinding` to exercise its call to `SetFeatureReport`. But drop
        // the binding immediately, so that `set_feature_report_receiver.collect()`
        // does not hang.
        let (_binding, _task) = TouchBinding::new(
            input_device_proxy,
            0,
            device_event_sender,
            test_node,
            input_device::InputPipelineFeatureFlags::default(),
            metrics::MetricsLogger::default(),
        )
        .await
        .unwrap();
        assert_matches!(
            set_feature_report_receiver.collect::<Vec<_>>().await.as_slice(),
            [fidl_fuchsia_input_report::FeatureReport {
                touch: Some(fidl_fuchsia_input_report::TouchFeatureReport {
                    input_mode: Some(
                        fidl_fuchsia_input_report::TouchConfigurationInputMode::WindowsPrecisionTouchpadCollection
                    ),
                    ..
                }),
                ..
            }]
        );
    }

    #[test_case(true, None, TouchDeviceType::TouchScreen; "touch screen")]
    #[test_case(false, None, TouchDeviceType::TouchScreen; "no mouse descriptor, no touch_input_mode")]
    #[test_case(true, Some(fidl_fuchsia_input_report::TouchConfigurationInputMode::MouseCollection), TouchDeviceType::WindowsPrecisionTouchpad; "touchpad in mouse mode")]
    #[test_case(true, Some(fidl_fuchsia_input_report::TouchConfigurationInputMode::WindowsPrecisionTouchpadCollection), TouchDeviceType::WindowsPrecisionTouchpad; "touchpad in touchpad mode")]
    #[fuchsia::test(allow_stalls = false)]
    async fn identifies_correct_touch_device_type(
        has_mouse_descriptor: bool,
        touch_input_mode: Option<fidl_fuchsia_input_report::TouchConfigurationInputMode>,
        expect_touch_device_type: TouchDeviceType,
    ) {
        let (input_device_proxy, _task) =
            spawn_input_stream_handler(move |input_device_request| async move {
                match input_device_request {
                    fidl_fuchsia_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                        let _ =
                            responder.send(&get_touchpad_device_descriptor(has_mouse_descriptor));
                    }
                    fidl_fuchsia_input_report::InputDeviceRequest::GetFeatureReport {
                        responder,
                    } => {
                        let _ = responder.send(Ok(&fidl_fuchsia_input_report::FeatureReport {
                            touch: Some(fidl_fuchsia_input_report::TouchFeatureReport {
                                input_mode: touch_input_mode,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }));
                    }
                    fidl_fuchsia_input_report::InputDeviceRequest::SetFeatureReport {
                        responder,
                        ..
                    } => {
                        let _ = responder.send(Ok(()));
                    }
                    r => panic!("unsupported request {:?}", r),
                }
            });

        let (device_event_sender, _) = futures::channel::mpsc::unbounded();

        // Create a test inspect node as required by TouchBinding::new()
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");

        let (binding, _task) = TouchBinding::new(
            input_device_proxy,
            0,
            device_event_sender,
            test_node,
            input_device::InputPipelineFeatureFlags::default(),
            metrics::MetricsLogger::default(),
        )
        .await
        .unwrap();
        let actual_type = match binding.device_descriptor {
            TouchDeviceDescriptor::TouchScreen(_) => TouchDeviceType::TouchScreen,
            TouchDeviceDescriptor::Touchpad(_) => TouchDeviceType::WindowsPrecisionTouchpad,
        };
        pretty_assertions::assert_eq!(actual_type, expect_touch_device_type);
    }

    /// Returns an |fidl_fuchsia_input_report::DeviceDescriptor| for
    /// touchpad related tests.
    fn get_touchpad_device_descriptor(
        has_mouse_descriptor: bool,
    ) -> fidl_fuchsia_input_report::DeviceDescriptor {
        fidl_fuchsia_input_report::DeviceDescriptor {
            mouse: match has_mouse_descriptor {
                true => Some(fidl_fuchsia_input_report::MouseDescriptor::default()),
                false => None,
            },
            touch: Some(fidl_fuchsia_input_report::TouchDescriptor {
                input: Some(fidl_fuchsia_input_report::TouchInputDescriptor {
                    contacts: Some(vec![fidl_fuchsia_input_report::ContactInputDescriptor {
                        position_x: Some(fidl_fuchsia_input::Axis {
                            range: fidl_fuchsia_input::Range { min: 1, max: 2 },
                            unit: fidl_fuchsia_input::Unit {
                                type_: fidl_fuchsia_input::UnitType::None,
                                exponent: 0,
                            },
                        }),
                        position_y: Some(fidl_fuchsia_input::Axis {
                            range: fidl_fuchsia_input::Range { min: 2, max: 3 },
                            unit: fidl_fuchsia_input::Unit {
                                type_: fidl_fuchsia_input::UnitType::Other,
                                exponent: 100000,
                            },
                        }),
                        pressure: Some(fidl_fuchsia_input::Axis {
                            range: fidl_fuchsia_input::Range { min: 3, max: 4 },
                            unit: fidl_fuchsia_input::Unit {
                                type_: fidl_fuchsia_input::UnitType::Grams,
                                exponent: -991,
                            },
                        }),
                        contact_width: Some(fidl_fuchsia_input::Axis {
                            range: fidl_fuchsia_input::Range { min: 5, max: 6 },
                            unit: fidl_fuchsia_input::Unit {
                                type_: fidl_fuchsia_input::UnitType::EnglishAngularVelocity,
                                exponent: 123,
                            },
                        }),
                        contact_height: Some(fidl_fuchsia_input::Axis {
                            range: fidl_fuchsia_input::Range { min: 7, max: 8 },
                            unit: fidl_fuchsia_input::Unit {
                                type_: fidl_fuchsia_input::UnitType::Pascals,
                                exponent: 100,
                            },
                        }),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    // Tests that a pressed button with no contacts generates an event with the
    // button.
    #[test_case(true; "merge touch events enabled")]
    #[test_case(false; "merge touch events disabled")]
    #[fuchsia::test]
    async fn send_pressed_button_no_contact(enable_merge_touch_events: bool) {
        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();

        let reports = vec![create_touch_input_report(
            vec![],
            Some(vec![fidl_fuchsia_input_report::TouchButton::Palm]),
            event_time_i64,
        )];

        let expected_events = vec![create_touch_screen_event_with_buttons(
            SortedVecMap::new(),
            vec![fidl_fuchsia_input_report::TouchButton::Palm],
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events_with_feature_flags!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
            feature_flags: input_device::InputPipelineFeatureFlags {
                enable_merge_touch_events,
                ..Default::default()
            },
        );
    }

    // Tests that a pressed button with a contact generates an event with
    // contact and button.
    #[test_case(true; "merge touch events enabled")]
    #[test_case(false; "merge touch events disabled")]
    #[fuchsia::test]
    async fn send_pressed_button_with_contact(enable_merge_touch_events: bool) {
        const TOUCH_ID: u32 = 2;

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();

        let contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            pressure: None,
            contact_width: None,
            contact_height: None,
            ..Default::default()
        };
        let reports = vec![create_touch_input_report(
            vec![contact],
            Some(vec![fidl_fuchsia_input_report::TouchButton::Palm]),
            event_time_i64,
        )];

        let expected_events = vec![create_touch_screen_event_with_buttons(
            SortedVecMap::from_iter(vec![
                (
                    fidl_ui_input::PointerEventPhase::Add,
                    vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                ),
                (
                    fidl_ui_input::PointerEventPhase::Down,
                    vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                ),
            ]),
            vec![fidl_fuchsia_input_report::TouchButton::Palm],
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events_with_feature_flags!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
            feature_flags: input_device::InputPipelineFeatureFlags {
                enable_merge_touch_events,
                ..Default::default()
            },
        );
    }

    // Tests that multiple pressed buttons with contacts generates an event
    // with contact and buttons.
    #[test_case(true; "merge touch events enabled")]
    #[test_case(false; "merge touch events disabled")]
    #[fuchsia::test]
    async fn send_multiple_pressed_buttons_with_contact(enable_merge_touch_events: bool) {
        const TOUCH_ID: u32 = 2;

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();

        let contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            pressure: None,
            contact_width: None,
            contact_height: None,
            ..Default::default()
        };
        let reports = vec![create_touch_input_report(
            vec![contact],
            Some(vec![
                fidl_fuchsia_input_report::TouchButton::Palm,
                fidl_fuchsia_input_report::TouchButton::__SourceBreaking { unknown_ordinal: 2 },
            ]),
            event_time_i64,
        )];

        let expected_events = vec![create_touch_screen_event_with_buttons(
            SortedVecMap::from_iter(vec![
                (
                    fidl_ui_input::PointerEventPhase::Add,
                    vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                ),
                (
                    fidl_ui_input::PointerEventPhase::Down,
                    vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                ),
            ]),
            vec![
                fidl_fuchsia_input_report::TouchButton::Palm,
                fidl_fuchsia_input_report::TouchButton::__SourceBreaking { unknown_ordinal: 2 },
            ],
            event_time_u64,
            &descriptor,
        )];

        assert_input_report_sequence_generates_events_with_feature_flags!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
            feature_flags: input_device::InputPipelineFeatureFlags {
                enable_merge_touch_events,
                ..Default::default()
            },
        );
    }

    // Tests that no buttons and no contacts generates no events.
    #[test_case(true; "merge touch events enabled")]
    #[test_case(false; "merge touch events disabled")]
    #[fuchsia::test]
    async fn send_no_buttons_no_contacts(enable_merge_touch_events: bool) {
        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, _) = testing_utilities::event_times();

        let reports = vec![create_touch_input_report(vec![], Some(vec![]), event_time_i64)];

        let expected_events: Vec<input_device::InputEvent> = vec![];

        assert_input_report_sequence_generates_events_with_feature_flags!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
            feature_flags: input_device::InputPipelineFeatureFlags {
                enable_merge_touch_events,
                ..Default::default()
            },
        );
    }

    // Tests a buttons event after a contact event does not remove contacts.
    #[test_case(true; "merge touch events enabled")]
    #[test_case(false; "merge touch events disabled")]
    #[fuchsia::test]
    async fn send_button_does_not_remove_contacts(enable_merge_touch_events: bool) {
        const TOUCH_ID: u32 = 2;

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, event_time_u64) = testing_utilities::event_times();

        let contact = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            pressure: None,
            contact_width: None,
            contact_height: None,
            ..Default::default()
        };
        let reports = vec![
            create_touch_input_report(vec![contact], None, event_time_i64),
            create_touch_input_report(
                vec![],
                Some(vec![fidl_fuchsia_input_report::TouchButton::Palm]),
                event_time_i64,
            ),
            create_touch_input_report(vec![], Some(vec![]), event_time_i64),
        ];

        let expected_events = vec![
            create_touch_screen_event_with_buttons(
                SortedVecMap::from_iter(vec![
                    (
                        fidl_ui_input::PointerEventPhase::Add,
                        vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                    ),
                    (
                        fidl_ui_input::PointerEventPhase::Down,
                        vec![create_touch_contact(TOUCH_ID, Position { x: 0.0, y: 0.0 })],
                    ),
                ]),
                vec![],
                event_time_u64,
                &descriptor,
            ),
            create_touch_screen_event_with_buttons(
                SortedVecMap::new(),
                vec![fidl_fuchsia_input_report::TouchButton::Palm],
                event_time_u64,
                &descriptor,
            ),
            create_touch_screen_event_with_buttons(
                SortedVecMap::new(),
                vec![],
                event_time_u64,
                &descriptor,
            ),
        ];

        assert_input_report_sequence_generates_events_with_feature_flags!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: TouchBinding,
            feature_flags: input_device::InputPipelineFeatureFlags {
                enable_merge_touch_events,
                ..Default::default()
            },
        );
    }

    #[fuchsia::test]
    async fn process_reports_batches_events() {
        const TOUCH_ID: u32 = 2;

        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, _) = testing_utilities::event_times();

        let contact1 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            ..Default::default()
        };
        let contact2 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(10),
            position_y: Some(10),
            ..Default::default()
        };
        let reports = vec![
            create_touch_input_report(vec![contact1], None, event_time_i64),
            create_touch_input_report(vec![contact2], None, event_time_i64),
        ];

        let (mut event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();

        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("TestDevice_Touch");
        let mut inspect_status = InputDeviceStatus::new(test_node);
        inspect_status.health_node.set_ok();

        let reports_wire = crate::testing_utilities::reports_to_wire(reports);
        let _ = TouchBinding::process_reports(
            &reports_wire,
            None,
            &descriptor,
            &mut event_sender,
            &inspect_status,
            &metrics::MetricsLogger::default(),
            &input_device::InputPipelineFeatureFlags::default(),
        );

        // Expect EXACTLY one batch containing two events.
        let batch = event_receiver.try_next().expect("Expected a batch of events");
        let events = batch.expect("Expected events in the batch");
        assert_eq!(events.len(), 2);

        // Verify no more batches.
        assert!(event_receiver.try_next().is_err());
    }

    #[fuchsia::test]
    async fn process_reports_merges_touch_events_when_enabled() {
        const TOUCH_ID: u32 = 2;
        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, _) = testing_utilities::event_times();

        let contact_add = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            ..Default::default()
        };
        let contact_move1 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(10),
            position_y: Some(10),
            ..Default::default()
        };
        let contact_move2 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(20),
            position_y: Some(20),
            ..Default::default()
        };
        let contact_move3 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(30),
            position_y: Some(30),
            ..Default::default()
        };
        let reports = vec![
            create_touch_input_report(vec![contact_add], None, event_time_i64),
            create_touch_input_report(vec![contact_move1], None, event_time_i64),
            create_touch_input_report(vec![contact_move2], None, event_time_i64),
            create_touch_input_report(vec![contact_move3], None, event_time_i64),
            create_touch_input_report(vec![], None, event_time_i64),
        ];

        let (mut event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();
        let inspector = fuchsia_inspect::Inspector::default();
        let mut inspect_status =
            InputDeviceStatus::new(inspector.root().create_child("TestDevice_Touch"));
        inspect_status.health_node.set_ok();

        let reports_wire = crate::testing_utilities::reports_to_wire(reports);
        let _ = TouchBinding::process_reports(
            &reports_wire,
            None,
            &descriptor,
            &mut event_sender,
            &inspect_status,
            &metrics::MetricsLogger::default(),
            &input_device::InputPipelineFeatureFlags {
                enable_merge_touch_events: true,
                ..Default::default()
            },
        );

        let batch = event_receiver.try_next().unwrap().unwrap();

        // Expected events: Add, Move(30), Remove.
        assert_eq!(batch.len(), 3);

        // Verify Add event
        assert_matches!(
            &batch[0].device_event,
            input_device::InputDeviceEvent::TouchScreen(event)
                if event.injector_contacts.get(&pointerinjector::EventPhase::Add).is_some()
        );
        // Verify Move event (merged to the last one)
        assert_matches!(
            &batch[1].device_event,
            input_device::InputDeviceEvent::TouchScreen(event)
                if event.injector_contacts.get(&pointerinjector::EventPhase::Change).map(|c| c[0].position.x) == Some(30.0)
        );
        // Verify Remove event
        assert_matches!(
            &batch[2].device_event,
            input_device::InputDeviceEvent::TouchScreen(event)
                if event.injector_contacts.get(&pointerinjector::EventPhase::Remove).is_some()
        );
    }

    #[fuchsia::test]
    async fn process_reports_does_not_merge_touch_events_when_disabled() {
        const TOUCH_ID: u32 = 2;
        let descriptor =
            input_device::InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let (event_time_i64, _) = testing_utilities::event_times();

        let contact_add = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(0),
            position_y: Some(0),
            ..Default::default()
        };
        let contact_move1 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(10),
            position_y: Some(10),
            ..Default::default()
        };
        let contact_move2 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(20),
            position_y: Some(20),
            ..Default::default()
        };
        let contact_move3 = fidl_fuchsia_input_report::ContactInputReport {
            contact_id: Some(TOUCH_ID),
            position_x: Some(30),
            position_y: Some(30),
            ..Default::default()
        };
        let reports = vec![
            create_touch_input_report(vec![contact_add], None, event_time_i64),
            create_touch_input_report(vec![contact_move1], None, event_time_i64),
            create_touch_input_report(vec![contact_move2], None, event_time_i64),
            create_touch_input_report(vec![contact_move3], None, event_time_i64),
            create_touch_input_report(vec![], None, event_time_i64),
        ];

        let (mut event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();
        let inspector = fuchsia_inspect::Inspector::default();
        let mut inspect_status =
            InputDeviceStatus::new(inspector.root().create_child("TestDevice_Touch"));
        inspect_status.health_node.set_ok();

        let reports_wire = crate::testing_utilities::reports_to_wire(reports);
        let _ = TouchBinding::process_reports(
            &reports_wire,
            None,
            &descriptor,
            &mut event_sender,
            &inspect_status,
            &metrics::MetricsLogger::default(),
            &input_device::InputPipelineFeatureFlags {
                enable_merge_touch_events: false,
                ..Default::default()
            },
        );

        let batch = event_receiver.try_next().unwrap().unwrap();

        // Expected events: Add, Move(10), Move(20), Move(30), Remove.
        assert_eq!(batch.len(), 5);
    }
}
