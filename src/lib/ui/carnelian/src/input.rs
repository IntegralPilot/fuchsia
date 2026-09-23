// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::app::{InternalSender, MessageInternal};
use crate::geometry::{IntPoint, IntSize};
use anyhow::{Error, format_err};
use euclid::default::Transform2D;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_input_report as fidl_input_report;
use fuchsia_async::{self as fasync, MonotonicInstant, TimeoutExt};
use fuchsia_component::client::Service;
use futures::{StreamExt, TryFutureExt};
use keymaps::usages::input3_key_to_hid_usage;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use zx::{self as zx, MonotonicDuration};

#[derive(Debug)]
pub(crate) enum UserInputMessage {
    ScenicKeyEvent(fidl_fuchsia_ui_input3::KeyEvent),
    FlatlandMouseEvents(Vec<fidl_fuchsia_ui_pointer::MouseEvent>),
    FlatlandTouchEvents(Vec<fidl_fuchsia_ui_pointer::TouchEvent>),
}

/// A button on a mouse
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Button(pub u8);

const PRIMARY_BUTTON: u8 = 1;

impl Button {
    /// Is this the primary button, usually the leftmost button on
    /// a mouse.
    pub fn is_primary(&self) -> bool {
        self.0 == PRIMARY_BUTTON
    }
}

/// A set of buttons.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ButtonSet {
    buttons: HashSet<Button>,
}

impl ButtonSet {
    /// Create a new set of buttons from input report flags.
    pub fn new(buttons: &HashSet<u8>) -> ButtonSet {
        ButtonSet { buttons: buttons.iter().map(|button| Button(*button)).collect() }
    }

    /// Create a new set of buttons from scenic flags.
    pub fn new_from_flags(flags: u32) -> ButtonSet {
        let buttons: HashSet<u8> = (0..2)
            .filter_map(|index| {
                let mask = 1 << index;
                if flags & mask != 0 { Some(index + 1) } else { None }
            })
            .collect();
        ButtonSet::new(&buttons)
    }

    /// Convenience function for checking if the primary button is down.
    pub fn primary_button_is_down(&self) -> bool {
        self.buttons.contains(&Button(PRIMARY_BUTTON))
    }
}

/// Keyboard modifier keys.
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub struct Modifiers {
    /// A shift key is down.
    pub shift: bool,
    /// An alt or option key is down.
    pub alt: bool,
    /// A control key is down.
    pub control: bool,
    /// A caps lock key is down.
    pub caps_lock: bool,
}

impl Modifiers {
    pub(crate) fn from_pressed_keys_3(pressed_keys: &HashSet<fidl_fuchsia_input::Key>) -> Self {
        Self {
            shift: pressed_keys.contains(&fidl_fuchsia_input::Key::LeftShift)
                || pressed_keys.contains(&fidl_fuchsia_input::Key::RightShift),
            alt: pressed_keys.contains(&fidl_fuchsia_input::Key::LeftAlt)
                || pressed_keys.contains(&fidl_fuchsia_input::Key::RightAlt),
            control: pressed_keys.contains(&fidl_fuchsia_input::Key::LeftCtrl)
                || pressed_keys.contains(&fidl_fuchsia_input::Key::RightCtrl),
            caps_lock: pressed_keys.contains(&fidl_fuchsia_input::Key::CapsLock),
        }
    }

    pub(crate) fn is_modifier(key: &fidl_fuchsia_input::Key) -> bool {
        match key {
            fidl_fuchsia_input::Key::LeftShift
            | fidl_fuchsia_input::Key::RightShift
            | fidl_fuchsia_input::Key::LeftAlt
            | fidl_fuchsia_input::Key::RightAlt
            | fidl_fuchsia_input::Key::LeftCtrl
            | fidl_fuchsia_input::Key::RightCtrl
            | fidl_fuchsia_input::Key::CapsLock => true,
            _ => false,
        }
    }
}

/// Mouse-related items
pub mod mouse {
    use super::*;
    use crate::geometry::IntVector;

    /// Phase of a mouse event.
    #[derive(Debug, PartialEq, Clone)]
    pub enum Phase {
        /// A particular button went down.
        Down(Button),
        /// A particular button came up.
        Up(Button),
        /// The mouse moved, with or without a change in button state.
        Moved,
        /// The mouse wheel changed position.
        Wheel(IntVector),
    }

    /// A mouse event.
    #[derive(Debug, PartialEq, Clone)]
    pub struct Event {
        /// Pressed buttons.
        pub buttons: ButtonSet,
        /// Event phase.
        pub phase: Phase,
        /// Location of the mouse cursor during this event.
        pub location: IntPoint,
    }

    pub(crate) fn create_event(
        event_time: u64,
        device_id: &DeviceId,
        button_set: &ButtonSet,
        cursor_position: IntPoint,
        transform: &Transform2D<f32>,
        phase: mouse::Phase,
    ) -> super::Event {
        let cursor_position = transform.transform_point(cursor_position.to_f32()).to_i32();
        let mouse_event =
            mouse::Event { buttons: button_set.clone(), phase, location: cursor_position };
        super::Event {
            event_time,
            device_id: device_id.clone(),
            event_type: EventType::Mouse(mouse_event),
        }
    }
}

/// Keyboard-related items.
pub mod keyboard {
    use super::*;

    /// Phase of a keyboard event.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Phase {
        /// A key is pressed.
        Pressed,
        /// A key is released.
        Released,
        /// A key is no longer pressed without being released.
        Cancelled,
        /// A key has been held down long enough to start repeating.
        Repeat,
    }

    /// A keyboard event.
    #[derive(Debug, PartialEq, Clone)]
    pub struct Event {
        /// Event phase.
        pub phase: Phase,
        /// Unicode code point of the key causing the event, if any.
        pub code_point: Option<u32>,
        /// USB HID usage of the key causing the event.
        pub hid_usage: u32,
        /// Modifier keys being pressed or held in addition to the key
        /// causing the event.
        pub modifiers: Modifiers,
    }
}

/// Touch-related items.
pub mod touch {
    use super::*;

    #[derive(Debug, Eq)]
    pub(crate) struct RawContact {
        pub contact_id: u32,
        pub position: IntPoint,
        // TODO(https://fxbug.dev/42165549)
        #[allow(unused)]
        pub pressure: Option<i64>,
        pub contact_size: Option<IntSize>,
    }

    impl PartialEq for RawContact {
        fn eq(&self, rhs: &Self) -> bool {
            self.contact_id == rhs.contact_id
        }
    }

    impl Hash for RawContact {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.contact_id.hash(state);
        }
    }

    /// ID of a touch contact.
    #[derive(Clone, Copy, Debug, Eq, Ord, PartialOrd, PartialEq, Hash)]
    pub struct ContactId(pub u32);

    /// Phase of a touch event.
    #[derive(Debug, PartialEq, Clone)]
    pub enum Phase {
        /// A contact began.
        Down(IntPoint, IntSize),
        /// A contact moved.
        Moved(IntPoint, IntSize),
        /// A contact ended.
        Up,
        /// A contact was removed.
        Remove,
        /// A contact was cancelled.
        Cancel,
    }

    /// A single contact found in a touch event.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Contact {
        /// ID of this contact
        pub contact_id: ContactId,
        /// Phase of this contact
        pub phase: Phase,
    }

    /// A touch event.
    #[derive(Debug, PartialEq, Clone)]
    pub struct Event {
        /// All the current contact in this event
        pub contacts: Vec<Contact>,
        /// Buttons in this touch event, possible if the touch comes
        /// from a stylus with buttons.
        pub buttons: HashSet<fidl_input_report::TouchButton>,
    }
}

/// Pointer event
///
/// Carnelian provides a least-common-denominator pointer event that can be created from
/// either touch events or mouse events.
pub mod pointer {
    use super::*;

    /// Pointer phase.
    #[derive(Debug, PartialEq, Clone)]
    pub enum Phase {
        /// A pointer has gone down.
        Down(IntPoint),
        /// A pointer has moved.
        Moved(IntPoint),
        /// A pointer has come up.
        Up,
        /// A pointer has been removed without coming up.
        Remove,
        /// A pointer has been cancelled.
        Cancel,
    }

    /// Pointer ID.
    #[derive(Clone, Debug, Eq, Ord, PartialOrd, PartialEq, Hash)]
    pub enum PointerId {
        /// ID from a mouse event.
        Mouse(DeviceId),
        /// ID from a contact in a touch event.
        Contact(touch::ContactId),
    }

    /// Pointer event.
    #[derive(Debug, PartialEq, Clone)]
    pub struct Event {
        /// Pointer event phase.
        pub phase: Phase,
        /// Pointer event pointer ID.
        pub pointer_id: PointerId,
    }

    impl Event {
        /// Create a pointer event from a mouse event.
        pub fn new_from_mouse_event(
            device_id: &DeviceId,
            mouse_event: &mouse::Event,
        ) -> Option<Self> {
            match &mouse_event.phase {
                mouse::Phase::Down(button) => {
                    if button.is_primary() {
                        Some(pointer::Phase::Down(mouse_event.location))
                    } else {
                        None
                    }
                }
                mouse::Phase::Moved => {
                    if mouse_event.buttons.primary_button_is_down() {
                        Some(pointer::Phase::Moved(mouse_event.location))
                    } else {
                        None
                    }
                }
                mouse::Phase::Up(button) => {
                    if button.is_primary() {
                        Some(pointer::Phase::Up)
                    } else {
                        None
                    }
                }
                mouse::Phase::Wheel(_) => None,
            }
            .and_then(|phase| Some(Self { phase, pointer_id: PointerId::Mouse(device_id.clone()) }))
        }

        /// Create a pointer event from a single contact in a touch event.
        pub fn new_from_contact(contact: &touch::Contact) -> Self {
            let phase = match contact.phase {
                touch::Phase::Down(location, ..) => pointer::Phase::Down(location),
                touch::Phase::Moved(location, ..) => pointer::Phase::Moved(location),
                touch::Phase::Up => pointer::Phase::Up,
                touch::Phase::Remove => pointer::Phase::Remove,
                touch::Phase::Cancel => pointer::Phase::Cancel,
            };
            Self { phase, pointer_id: PointerId::Contact(contact.contact_id) }
        }
    }
}

/// Events related to "consumer control" buttons, like volume controls.
///
/// These events are separated because they are different devices at the driver
/// level, but it's not clear this is the right abstraction for Carnelian.
pub mod consumer_control {

    /// Phase of a consumer control event.
    #[derive(Debug, PartialEq, Clone, Copy)]
    pub enum Phase {
        /// Button went down.
        Down,
        /// Button came up.
        Up,
    }

    /// A consumer control event.
    #[derive(Debug, PartialEq, Clone)]
    pub struct Event {
        /// Phase of event.
        pub phase: Phase,
        /// USB HID for key being pressed or released.
        pub button: fidl_fuchsia_input::ConsumerControlButton,
    }
}

/// Unique identifier for an input device.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct DeviceId(pub String);

/// Enum of all supported user-input events.
#[derive(Debug, PartialEq, Clone)]
pub enum EventType {
    /// Mouse event.
    Mouse(mouse::Event),
    /// Keyboard event.
    Keyboard(keyboard::Event),
    /// Touch event.
    Touch(touch::Event),
    /// Consumer control event.
    ConsumerControl(consumer_control::Event),
}

/// Over user-input struct.
#[derive(Debug, PartialEq, Clone)]
pub struct Event {
    /// Time of event.
    pub event_time: u64,
    /// Id of device producting this event.
    pub device_id: DeviceId,
    /// The event.
    pub event_type: EventType,
}

async fn listen_to_instance(
    instance: &fidl_input_report::ServiceProxy,
    internal_sender: &InternalSender,
) -> Result<(), Error> {
    let device = instance.connect_to_input_device()?;
    let descriptor = device
        .get_descriptor()
        .map_err(|err| format_err!("FIDL error on get_descriptor: {:?}", err))
        .on_timeout(MonotonicInstant::after(MonotonicDuration::from_millis(200)), || {
            Err(format_err!("FIDL timeout on get_descriptor"))
        })
        .await?;

    let device_id = instance.instance_name().to_string();
    internal_sender
        .unbounded_send(MessageInternal::RegisterDevice(
            DeviceId(device_id.clone()),
            Box::new(descriptor),
        ))
        .expect("unbounded_send");
    let input_report_sender = internal_sender.clone();
    let (input_reports_reader_proxy, input_reports_reader_request) =
        create_proxy::<fidl_input_report::InputReportsReaderV2Marker>();

    const MAX_UNACKNOWLEDGED_REPORTS_LIMIT: u16 = 120;
    device
        .get_input_reports_reader_v2(input_reports_reader_request, MAX_UNACKNOWLEDGED_REPORTS_LIMIT)
        .await
        .expect("failed to get InputReportsReaderV2; v1 is no longer supported");

    let mut event_stream = input_reports_reader_proxy.take_event_stream();
    fasync::Task::local(async move {
        // Keep `device` alive for the duration of this task.
        #[expect(unused)]
        let device = device;

        while let Some(event) = event_stream.next().await {
            match event {
                Ok(fidl_input_report::InputReportsReaderV2Event::OnInputReports {
                    reports,
                    last_report_stamp,
                }) => {
                    if let Err(err) =
                        input_reports_reader_proxy.acknowledge_reports(last_report_stamp)
                    {
                        eprintln!("Error acknowledging reports for {}: {}", device_id, err);
                    }
                    for report in reports {
                        input_report_sender
                            .unbounded_send(MessageInternal::InputReport(
                                DeviceId(device_id.clone()),
                                report,
                            ))
                            .expect("unbounded_send");
                    }
                }
                Ok(fidl_input_report::InputReportsReaderV2Event::_UnknownEvent {
                    ordinal, ..
                }) => {
                    eprintln!("Unknown event (ordinal {}) for device {}", ordinal, device_id);
                }
                Err(err) => {
                    eprintln!(
                        "Error from input reports reader event stream for {}: {}",
                        device_id, err
                    );
                    break;
                }
            }
        }
    })
    .detach();
    Ok(())
}

pub(crate) async fn listen_for_user_input(internal_sender: InternalSender) -> Result<(), Error> {
    let watcher_sender = internal_sender.clone();

    let service = Service::open(fidl_input_report::ServiceMarker)
        .map_err(|err| format_err!("failed to open fuchsia.input.report.Service: {:?}", err))?;
    let mut watcher = service
        .watch()
        .await
        .map_err(|err| format_err!("failed to watch fuchsia.input.report.Service: {:?}", err))?;

    fasync::Task::local(async move {
        while let Some(instance) = watcher.next().await {
            match instance {
                Ok(instance) => {
                    match listen_to_instance(&instance, &watcher_sender).await {
                        Err(err) => {
                            eprintln!("Error: {}: {}", instance.instance_name(), err)
                        }
                        _ => (),
                    };
                }
                Err(err) => {
                    eprintln!("Error watching input report service: {}", err);
                    break;
                }
            }
        }
    })
    .detach();

    Ok(())
}

pub(crate) mod flatland;
pub(crate) mod key3;
pub(crate) mod report;

#[cfg(test)]
mod tests;
