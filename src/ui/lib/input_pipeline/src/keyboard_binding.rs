// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device::{self, Handled, InputDeviceStatus, InputEvent};
use crate::{Transport, metrics, utils};
use anyhow::{Error, Result, format_err};
use async_trait::async_trait;
use fidl_fuchsia_ui_input3 as fidl_ui_input3;
use fidl_fuchsia_ui_input3::{KeyEventType, Modifiers};
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use metrics_registry::*;

/// A [`KeyboardEvent`] represents an input event from a keyboard device.
///
/// The keyboard event contains information about a key event.  A key event represents a change in
/// the key state. Clients can expect the following sequence of events for a given key:
///
/// 1. [`KeyEventType::Pressed`]: the key has transitioned to being pressed.
/// 2. [`KeyEventType::Released`]: the key has transitioned to being released.
///
/// No duplicate [`KeyEventType::Pressed`] events will be sent for keys, even if the
/// key is present in a subsequent [`InputReport`]. Clients can assume that
/// a key is pressed for all received input events until the key is present in
/// the [`KeyEventType::Released`] entry of [`keys`].
///
/// Use `new` to create.  Use `get_*` methods to read fields.  Use `into_with_*`
/// methods to add optional information.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardEvent {
    /// The key that changed state in this [KeyboardEvent].
    key: fidl_fuchsia_input::Key,

    /// A description of what happened to `key`.
    event_type: KeyEventType,

    /// The [`fidl_ui_input3::Modifiers`] associated with the pressed keys.
    modifiers: Option<fidl_ui_input3::Modifiers>,

    /// The [`fidl_ui_input3::LockState`] currently computed.
    lock_state: Option<fidl_ui_input3::LockState>,

    /// If set, contains the unique identifier of the keymap to be used when or
    /// if remapping the keypresses.
    keymap: Option<String>,

    /// If set, denotes the meaning of `key` in terms of the key effect.
    /// A `KeyboardEvent` starts off with `key_meaning` unset, and the key
    /// meaning is added in the input pipeline by the appropriate
    /// keymap-aware input handlers.
    key_meaning: Option<fidl_fuchsia_ui_input3::KeyMeaning>,

    /// If this keyboard event has been generated as a result of a repeated
    /// generation of the same key, then this will be a nonzero. A nonzero
    /// value N here means that this is Nth generated autorepeat for this
    /// keyboard event.  The counter is reset for each new autorepeat key
    /// span.
    repeat_sequence: u32,
}

impl KeyboardEvent {
    /// Creates a new KeyboardEvent, with required fields filled out.  Use the
    /// `into_with_*` methods to add optional information.
    pub fn new(key: fidl_fuchsia_input::Key, event_type: KeyEventType) -> Self {
        KeyboardEvent {
            key,
            event_type,
            modifiers: None,
            lock_state: None,
            keymap: None,
            key_meaning: None,
            repeat_sequence: 0,
        }
    }

    pub fn get_key(&self) -> fidl_fuchsia_input::Key {
        self.key
    }

    /// Converts [KeyboardEvent] into the same one, but with specified key.
    pub fn into_with_key(self, key: fidl_fuchsia_input::Key) -> Self {
        Self { key, ..self }
    }

    pub fn get_event_type(&self) -> KeyEventType {
        self.event_type
    }

    /// Converts [KeyboardEvent] into the same one, but with specified event type.
    pub fn into_with_event_type(self, event_type: KeyEventType) -> Self {
        Self { event_type, ..self }
    }

    /// Folds the key event type into an active event (Pressed, Released).
    pub fn into_with_folded_event(self) -> Self {
        Self { event_type: self.get_event_type_folded(), ..self }
    }

    /// Gets [KeyEventType], folding `SYNC` into `PRESSED` and `CANCEL` into `RELEASED`.
    pub fn get_event_type_folded(&self) -> KeyEventType {
        match self.event_type {
            KeyEventType::Pressed | KeyEventType::Sync => KeyEventType::Pressed,
            KeyEventType::Released | KeyEventType::Cancel => KeyEventType::Released,
        }
    }

    /// Converts [KeyboardEvent] into the same one, but with specified modifiers.
    pub fn into_with_modifiers(self, modifiers: Option<fidl_ui_input3::Modifiers>) -> Self {
        Self { modifiers, ..self }
    }

    /// Returns the currently applicable modifiers.
    pub fn get_modifiers(&self) -> Option<fidl_ui_input3::Modifiers> {
        self.modifiers
    }

    /// Returns the currently applicable modifiers, with the sided modifiers removed.
    ///
    /// For example, if LEFT_SHIFT is pressed, returns SHIFT, rather than SHIFT | LEFT_SHIFT
    pub fn get_unsided_modifiers(&self) -> Modifiers {
        let mut modifiers = self.modifiers.unwrap_or(Modifiers::empty());
        modifiers.set(
            Modifiers::LEFT_ALT
                | Modifiers::LEFT_CTRL
                | Modifiers::LEFT_SHIFT
                | Modifiers::LEFT_META
                | Modifiers::RIGHT_ALT
                | Modifiers::RIGHT_CTRL
                | Modifiers::RIGHT_SHIFT
                | Modifiers::RIGHT_META,
            false,
        );
        modifiers
    }

    /// Converts [KeyboardEvent] into the same one, but with the specified lock state.
    pub fn into_with_lock_state(self, lock_state: Option<fidl_ui_input3::LockState>) -> Self {
        Self { lock_state, ..self }
    }

    /// Returns the currently applicable lock state.
    pub fn get_lock_state(&self) -> Option<fidl_ui_input3::LockState> {
        self.lock_state
    }

    /// Converts [KeyboardEvent] into the same one, but with the specified keymap
    /// applied.
    pub fn into_with_keymap(self, keymap: Option<String>) -> Self {
        Self { keymap, ..self }
    }

    /// Returns the currently applied keymap.
    pub fn get_keymap(&self) -> Option<String> {
        self.keymap.clone()
    }

    /// Converts [KeyboardEvent] into the same one, but with the key meaning applied.
    pub fn into_with_key_meaning(
        self,
        key_meaning: Option<fidl_fuchsia_ui_input3::KeyMeaning>,
    ) -> Self {
        Self { key_meaning, ..self }
    }

    /// Returns the currently valid key meaning.
    pub fn get_key_meaning(&self) -> Option<fidl_fuchsia_ui_input3::KeyMeaning> {
        self.key_meaning
    }

    /// Returns the repeat sequence number.  If a nonzero number N is returned,
    /// that means this [KeyboardEvent] is the N-th generated autorepeat event.
    /// A zero means this is an event that came from the keyboard driver.
    pub fn get_repeat_sequence(&self) -> u32 {
        self.repeat_sequence
    }

    /// Converts [KeyboardEvent] into the same one, but with the repeat sequence
    /// changed.
    pub fn into_with_repeat_sequence(self, repeat_sequence: u32) -> Self {
        Self { repeat_sequence, ..self }
    }

    /// Centralizes the conversion from [KeyboardEvent] to `KeyEvent`.
    #[cfg(test)]
    pub(crate) fn from_key_event_at_time(
        &self,
        event_time: zx::MonotonicInstant,
    ) -> fidl_ui_input3::KeyEvent {
        fidl_ui_input3::KeyEvent {
            timestamp: Some(event_time.into_nanos()),
            type_: Some(self.event_type),
            key: Some(self.key),
            modifiers: self.modifiers,
            lock_state: self.lock_state,
            repeat_sequence: Some(self.repeat_sequence),
            key_meaning: self.key_meaning,
            ..Default::default()
        }
    }
}

impl KeyboardEvent {
    /// Returns true if the two keyboard events are about the same key.
    pub fn same_key(this: &KeyboardEvent, that: &KeyboardEvent) -> bool {
        this.get_key() == that.get_key()
    }
}

/// A [`KeyboardDeviceDescriptor`] contains information about a specific keyboard device.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardDeviceDescriptor {
    /// All the [`fidl_fuchsia_input::Key`]s available on the keyboard device.
    pub keys: Vec<fidl_fuchsia_input::Key>,

    /// The vendor ID, product ID and version.
    pub device_information: fidl_fuchsia_input_report::DeviceInformation,

    /// The unique identifier of this device.
    pub device_id: u32,
}

#[cfg(test)]
impl Default for KeyboardDeviceDescriptor {
    fn default() -> Self {
        KeyboardDeviceDescriptor {
            keys: vec![],
            device_information: fidl_fuchsia_input_report::DeviceInformation {
                vendor_id: Some(0),
                product_id: Some(0),
                version: Some(0),
                polling_rate: Some(0),
                ..Default::default()
            },
            device_id: 0,
        }
    }
}

/// A [`KeyboardBinding`] represents a connection to a keyboard input device.
///
/// The [`KeyboardBinding`] parses and exposes keyboard device descriptor properties (e.g., the
/// available keyboard keys) for the device it is associated with. It also parses [`InputReport`]s
/// from the device, and sends them to the device binding owner over `event_sender`.
pub struct KeyboardBinding {
    /// The channel to stream InputEvents to.
    event_sender: UnboundedSender<Vec<InputEvent>>,

    /// Holds information about this device.
    device_descriptor: KeyboardDeviceDescriptor,
}

#[async_trait]
impl input_device::InputDeviceBinding for KeyboardBinding {
    fn input_event_sender(&self) -> UnboundedSender<Vec<InputEvent>> {
        self.event_sender.clone()
    }

    fn get_device_descriptor(&self) -> input_device::InputDeviceDescriptor {
        input_device::InputDeviceDescriptor::Keyboard(self.device_descriptor.clone())
    }
}

impl KeyboardBinding {
    /// Creates a new [`InputDeviceBinding`] from the `device_proxy`.
    ///
    /// The binding will start listening for input reports immediately and send new InputEvents
    /// to the device binding owner over `input_event_sender`.
    ///
    /// # Parameters
    /// - `device_proxy`: The proxy to bind the new [`InputDeviceBinding`] to.
    /// - `device_id`: The unique identifier of this device.
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
        let (device_descriptor, mut inspect_status) =
            Self::bind_device(&device_proxy, device_id, device_node, metrics_logger.clone())
                .await?;
        inspect_status.health_node.set_ok();
        let task = input_device::initialize_report_stream(
            device_proxy,
            input_device::InputDeviceDescriptor::Keyboard(device_descriptor.clone()),
            input_event_sender.clone(),
            inspect_status,
            metrics_logger.clone(),
            feature_flags,
            Self::process_reports,
        );

        Ok((KeyboardBinding { event_sender: input_event_sender, device_descriptor }, task))
    }

    /// Converts a vector of keyboard keys to the appropriate [`fidl_ui_input3::Modifiers`] bitflags.
    ///
    /// For example, if `keys` contains `Key::CapsLock`, the bitflags will contain the corresponding
    /// flags for `CapsLock`.
    ///
    /// # Parameters
    /// - `keys`: The keys to check for modifiers.
    ///
    /// # Returns
    /// Returns `None` if there are no modifier keys present.
    pub fn to_modifiers(keys: &[&fidl_fuchsia_input::Key]) -> Option<fidl_ui_input3::Modifiers> {
        let mut modifiers = fidl_ui_input3::Modifiers::empty();
        for key in keys {
            let modifier = match key {
                fidl_fuchsia_input::Key::CapsLock => Some(fidl_ui_input3::Modifiers::CAPS_LOCK),
                fidl_fuchsia_input::Key::NumLock => Some(fidl_ui_input3::Modifiers::NUM_LOCK),
                fidl_fuchsia_input::Key::ScrollLock => Some(fidl_ui_input3::Modifiers::SCROLL_LOCK),
                _ => None,
            };
            if let Some(modifier) = modifier {
                modifiers.insert(modifier);
            };
        }
        if modifiers.is_empty() {
            return None;
        }
        Some(modifiers)
    }

    /// Binds the provided input device to a new instance of `Self`.
    ///
    /// # Parameters
    /// - `device`: The device to use to initialize the binding.
    /// - `device_id`: The device ID being bound.
    /// - `device_node`: The inspect node for this device binding
    ///
    /// # Errors
    /// If the device descriptor could not be retrieved, or the descriptor could not be parsed
    /// correctly.
    async fn bind_device(
        device: &fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
        device_id: u32,
        device_node: fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<(KeyboardDeviceDescriptor, InputDeviceStatus), Error> {
        let mut input_device_status = InputDeviceStatus::new(device_node);
        let descriptor = match device.get_descriptor().await {
            Ok(descriptor) => descriptor.descriptor,
            Err(_) => {
                input_device_status.health_node.set_unhealthy("Could not get device descriptor.");
                return Err(format_err!("Could not get descriptor for device_id: {}", device_id));
            }
        };

        let device_info = descriptor.device_information.ok_or_else(|| {
            input_device_status.health_node.set_unhealthy("Empty device_information in descriptor");
            // Logging in addition to returning an error, as in some test
            // setups the error may never be displayed to the user.
            metrics_logger.log_error(
                InputPipelineErrorMetricDimensionEvent::KeyboardEmptyDeviceInfo,
                std::format!("DRIVER BUG: empty device_information for device_id: {}", device_id),
            );
            format_err!("empty device info for device_id: {}", device_id)
        })?;
        match descriptor.keyboard {
            Some(fidl_next_fuchsia_input_report::KeyboardDescriptor {
                input: Some(fidl_next_fuchsia_input_report::KeyboardInputDescriptor { keys3, .. }),
                output: _,
                ..
            }) => Ok((
                KeyboardDeviceDescriptor {
                    keys: keys3
                        .unwrap_or_default()
                        .into_iter()
                        .map(|k| utils::key_to_old(&k))
                        .collect(),
                    device_information: fidl_fuchsia_input_report::DeviceInformation {
                        vendor_id: device_info.vendor_id,
                        product_id: device_info.product_id,
                        version: device_info.version,
                        polling_rate: device_info.polling_rate,
                        ..Default::default()
                    },
                    device_id,
                },
                input_device_status,
            )),
            device_descriptor => {
                input_device_status
                    .health_node
                    .set_unhealthy("Keyboard Device Descriptor failed to parse.");
                Err(format_err!(
                    "Keyboard Device Descriptor failed to parse: \n {:?}",
                    device_descriptor
                ))
            }
        }
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
        fuchsia_trace::duration!("input", "keyboard-binding-process-report", "num_reports" => reports.len());
        let (inspect_sender, inspect_receiver) = futures::channel::mpsc::unbounded();

        for report in reports {
            previous_state = Self::process_report(
                report,
                previous_state,
                device_descriptor,
                input_event_sender,
                inspect_status,
                metrics_logger,
                inspect_sender.clone(),
            );
        }
        (previous_state, Some(inspect_receiver))
    }

    fn process_report(
        report: &fidl_next_fuchsia_input_report::wire::InputReport<'_>,
        previous_state: Option<input_device::PreviousDeviceState>,
        device_descriptor: &input_device::InputDeviceDescriptor,
        input_event_sender: &mut UnboundedSender<Vec<InputEvent>>,
        inspect_status: &InputDeviceStatus,
        metrics_logger: &metrics::MetricsLogger,
        inspect_sender: UnboundedSender<InputEvent>,
    ) -> Option<input_device::PreviousDeviceState> {
        if let Some(trace_id) = report.trace_id() {
            fuchsia_trace::flow_end!("input", "input_report", trace_id.0.into());
        }

        let tracing_id = fuchsia_trace::Id::new();
        fuchsia_trace::flow_begin!("input", "key_event_thread", tracing_id);

        inspect_status.count_received_report_wire(report);
        // Input devices can have multiple types so ensure `report` is a KeyboardInputReport.
        match report.keyboard() {
            None => {
                inspect_status.count_filtered_report();
                return previous_state;
            }
            _ => (),
        };

        let new_keys = match KeyboardBinding::parse_pressed_keys_wire(report) {
            Some(keys) => keys,
            None => {
                // It's OK for the report to contain an empty vector of keys, but it's not OK for
                // the report to not have the appropriate fields set.
                //
                // In this case the report is treated as malformed, and the previous state is not
                // updated.
                metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::KeyboardFailedToParse,
                    std::format!("Failed to parse keyboard keys: {:?}", report),
                );
                inspect_status.count_filtered_report();
                return previous_state;
            }
        };

        let previous_keys: Vec<fidl_fuchsia_input::Key> = match previous_state {
            Some(input_device::PreviousDeviceState::Keyboard { pressed_keys }) => pressed_keys,
            _ => vec![],
        };

        KeyboardBinding::send_key_events(
            &new_keys,
            &previous_keys,
            device_descriptor.clone(),
            zx::MonotonicInstant::get(),
            input_event_sender.clone(),
            inspect_sender,
            metrics_logger,
            tracing_id,
        );

        Some(input_device::PreviousDeviceState::Keyboard { pressed_keys: new_keys })
    }

    fn parse_pressed_keys_wire(
        input_report: &fidl_next_fuchsia_input_report::wire::InputReport<'_>,
    ) -> Option<Vec<fidl_fuchsia_input::Key>> {
        input_report
            .keyboard()
            .and_then(|unwrapped_keyboard| unwrapped_keyboard.pressed_keys3())
            .map(|unwrapped_keys| {
                unwrapped_keys
                    .iter()
                    .map(|&k| {
                        let natural_key = fidl_next::FromWire::from_wire(k);
                        utils::key_to_old(&natural_key)
                    })
                    .collect()
            })
    }

    /// Sends key events to clients based on the new and previously pressed keys.
    ///
    /// # Parameters
    /// - `new_keys`: The input3 keys which are currently pressed, as reported by the bound device.
    /// - `previous_keys`: The input3 keys which were pressed in the previous input report.
    /// - `device_descriptor`: The descriptor for the input device generating the input reports.
    /// - `event_time`: The time in nanoseconds when the event was first recorded.
    /// - `input_event_sender`: The sender for the device binding's input event stream.
    fn send_key_events(
        new_keys: &Vec<fidl_fuchsia_input::Key>,
        previous_keys: &Vec<fidl_fuchsia_input::Key>,
        device_descriptor: input_device::InputDeviceDescriptor,
        event_time: zx::MonotonicInstant,
        input_event_sender: UnboundedSender<Vec<InputEvent>>,
        inspect_sender: UnboundedSender<input_device::InputEvent>,
        metrics_logger: &metrics::MetricsLogger,
        tracing_id: fuchsia_trace::Id,
    ) {
        // Dispatches all key events individually. This is helper function to process
        // the event sequence.
        fn dispatch_events(
            key_events: Vec<(fidl_fuchsia_input::Key, fidl_fuchsia_ui_input3::KeyEventType)>,
            device_descriptor: input_device::InputDeviceDescriptor,
            event_time: zx::MonotonicInstant,
            input_event_sender: UnboundedSender<Vec<input_device::InputEvent>>,
            inspect_sender: UnboundedSender<input_device::InputEvent>,
            metrics_logger: metrics::MetricsLogger,
            tracing_id: fuchsia_trace::Id,
        ) {
            fuchsia_trace::duration!("input", "key_event_thread");
            fuchsia_trace::flow_end!("input", "key_event_thread", tracing_id);

            let mut event_time = event_time;
            for (key, event_type) in key_events.into_iter() {
                let trace_id = fuchsia_trace::Id::new();
                fuchsia_trace::duration!("input", "keyboard_event_in_binding");
                fuchsia_trace::flow_begin!("input", "event_in_input_pipeline", trace_id);

                let event = input_device::InputEvent {
                    device_event: input_device::InputDeviceEvent::Keyboard(KeyboardEvent::new(
                        key, event_type,
                    )),
                    device_descriptor: device_descriptor.clone(),
                    event_time,
                    handled: Handled::No,
                    trace_id: Some(trace_id),
                };
                match input_event_sender.unbounded_send(vec![event.clone()]) {
                    Err(error) => {
                        metrics_logger.log_error(
                            InputPipelineErrorMetricDimensionEvent::KeyboardFailedToSendKeyboardEvent,
                            std::format!(
                                "Failed to send KeyboardEvent for key: {:?}, event_type: {:?}: {:?}",
                                key,
                                event_type,
                                error));
                    }
                    _ => {
                        let _ = inspect_sender.unbounded_send(event).expect("Failed to count generated KeyboardEvent in Input Pipeline Inspect tree.");
                    }
                }
                // If key events happen to have been reported at the same time,
                // we pull them apart artificially. A 1ns increment will likely
                // be enough of a difference that it is recognizable but that it
                // does not introduce confusion.
                event_time = event_time + zx::MonotonicDuration::from_nanos(1);
            }
        }

        // Filter out the keys which were present in the previous keyboard report to avoid sending
        // multiple `KeyEventType::Pressed` events for a key.
        let pressed_keys = new_keys
            .iter()
            .cloned()
            .filter(|key| !previous_keys.contains(key))
            .map(|k| (k, fidl_fuchsia_ui_input3::KeyEventType::Pressed));

        // Any key which is not present in the new keys, but was present in the previous report
        // is considered to be released.
        let released_keys = previous_keys
            .iter()
            .cloned()
            .filter(|key| !new_keys.contains(key))
            .map(|k| (k, fidl_fuchsia_ui_input3::KeyEventType::Released));

        // It is important that key releases are dispatched before key presses,
        // so that modifier tracking would work correctly.  We collect the result
        // into a vector.
        let all_keys = released_keys.chain(pressed_keys).collect::<Vec<_>>();

        dispatch_events(
            all_keys,
            device_descriptor,
            event_time,
            input_event_sender,
            inspect_sender,
            metrics_logger.clone(),
            tracing_id,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing_utilities;
    use futures::StreamExt;

    /// Tests that a key that is present in the new report, but was not present in the previous report
    /// is propagated as pressed.
    #[fuchsia::test]
    async fn pressed_key() {
        let descriptor = input_device::InputDeviceDescriptor::Keyboard(KeyboardDeviceDescriptor {
            keys: vec![fidl_fuchsia_input::Key::A],
            ..Default::default()
        });
        let (event_time_i64, _) = testing_utilities::event_times();

        let reports = vec![testing_utilities::create_keyboard_input_report(
            vec![fidl_fuchsia_input::Key::A],
            event_time_i64,
        )];
        let expected_events = vec![testing_utilities::create_keyboard_event(
            fidl_fuchsia_input::Key::A,
            fidl_fuchsia_ui_input3::KeyEventType::Pressed,
            None,
            &descriptor,
            /* keymap= */ None,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: KeyboardBinding,
        );
    }

    /// Tests that a key that is not present in the new report, but was present in the previous report
    /// is propagated as released.
    #[fuchsia::test]
    async fn released_key() {
        let descriptor = input_device::InputDeviceDescriptor::Keyboard(KeyboardDeviceDescriptor {
            keys: vec![fidl_fuchsia_input::Key::A],
            ..Default::default()
        });
        let (event_time_i64, _) = testing_utilities::event_times();

        let reports = vec![
            testing_utilities::create_keyboard_input_report(
                vec![fidl_fuchsia_input::Key::A],
                event_time_i64,
            ),
            testing_utilities::create_keyboard_input_report(vec![], event_time_i64),
        ];

        let expected_events = vec![
            testing_utilities::create_keyboard_event(
                fidl_fuchsia_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                &descriptor,
                /* keymap= */ None,
            ),
            testing_utilities::create_keyboard_event(
                fidl_fuchsia_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Released,
                None,
                &descriptor,
                /* keymap= */ None,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor.clone(),
            device_type: KeyboardBinding,
        );
    }

    /// Tests that a key that is present in multiple consecutive input reports is not propagated
    /// as a pressed event more than once.
    #[fuchsia::test]
    async fn multiple_pressed_event_filtering() {
        let descriptor = input_device::InputDeviceDescriptor::Keyboard(KeyboardDeviceDescriptor {
            keys: vec![fidl_fuchsia_input::Key::A],
            ..Default::default()
        });
        let (event_time_i64, _) = testing_utilities::event_times();

        let reports = vec![
            testing_utilities::create_keyboard_input_report(
                vec![fidl_fuchsia_input::Key::A],
                event_time_i64,
            ),
            testing_utilities::create_keyboard_input_report(
                vec![fidl_fuchsia_input::Key::A],
                event_time_i64,
            ),
        ];

        let expected_events = vec![testing_utilities::create_keyboard_event(
            fidl_fuchsia_input::Key::A,
            fidl_fuchsia_ui_input3::KeyEventType::Pressed,
            None,
            &descriptor,
            /* keymap= */ None,
        )];

        assert_input_report_sequence_generates_events!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: KeyboardBinding,
        );
    }

    /// Tests that both pressed and released keys are sent at once.
    #[fuchsia::test]
    async fn pressed_and_released_keys() {
        let descriptor = input_device::InputDeviceDescriptor::Keyboard(KeyboardDeviceDescriptor {
            keys: vec![fidl_fuchsia_input::Key::A, fidl_fuchsia_input::Key::B],
            ..Default::default()
        });
        let (event_time_i64, _) = testing_utilities::event_times();

        let reports = vec![
            testing_utilities::create_keyboard_input_report(
                vec![fidl_fuchsia_input::Key::A],
                event_time_i64,
            ),
            testing_utilities::create_keyboard_input_report(
                vec![fidl_fuchsia_input::Key::B],
                event_time_i64,
            ),
        ];

        let expected_events = vec![
            testing_utilities::create_keyboard_event(
                fidl_fuchsia_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                &descriptor,
                /* keymap= */ None,
            ),
            testing_utilities::create_keyboard_event(
                fidl_fuchsia_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Released,
                None,
                &descriptor,
                /* keymap= */ None,
            ),
            testing_utilities::create_keyboard_event(
                fidl_fuchsia_input::Key::B,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                &descriptor,
                /* keymap= */ None,
            ),
        ];

        assert_input_report_sequence_generates_events!(
            input_reports: reports,
            expected_events: expected_events,
            device_descriptor: descriptor,
            device_type: KeyboardBinding,
        );
    }

    #[fuchsia::test]
    fn get_unsided_modifiers() {
        use fidl_ui_input3::Modifiers;
        let event = KeyboardEvent::new(fidl_fuchsia_input::Key::A, KeyEventType::Pressed)
            .into_with_modifiers(Some(Modifiers::all()));
        assert_eq!(
            event.get_unsided_modifiers(),
            Modifiers::CAPS_LOCK
                | Modifiers::NUM_LOCK
                | Modifiers::SCROLL_LOCK
                | Modifiers::FUNCTION
                | Modifiers::SYMBOL
                | Modifiers::SHIFT
                | Modifiers::ALT
                | Modifiers::ALT_GRAPH
                | Modifiers::META
                | Modifiers::CTRL
        )
    }

    #[fuchsia::test]
    fn conversion_fills_out_all_fields() {
        use fidl_fuchsia_input::Key;
        use fidl_ui_input3::{KeyMeaning, LockState, Modifiers, NonPrintableKey};
        let event = KeyboardEvent::new(Key::A, KeyEventType::Pressed)
            .into_with_modifiers(Some(Modifiers::all()))
            .into_with_lock_state(Some(LockState::all()))
            .into_with_repeat_sequence(42)
            .into_with_key_meaning(Some(KeyMeaning::NonPrintableKey(NonPrintableKey::Tab)));

        let actual = event.from_key_event_at_time(zx::MonotonicInstant::from_nanos(42));
        assert_eq!(
            actual,
            fidl_fuchsia_ui_input3::KeyEvent {
                timestamp: Some(42),
                type_: Some(KeyEventType::Pressed),
                key: Some(Key::A),
                modifiers: Some(Modifiers::all()),
                key_meaning: Some(KeyMeaning::NonPrintableKey(NonPrintableKey::Tab)),
                repeat_sequence: Some(42),
                lock_state: Some(LockState::all()),
                ..Default::default()
            }
        );
    }
}
