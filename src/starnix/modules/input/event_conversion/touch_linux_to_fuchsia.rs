// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ui_test_input as futinput;
use sorted_vec_map::{SortedVecMap, SortedVecSet};
use starnix_logging::log_warn;
use starnix_uapi::errors::Errno;
use starnix_uapi::{error, uapi};

type SlotId = usize;
type TrackingId = u32;

/// TRACKING_ID changed to -1 means the contact is lifted.
const LIFTED_TRACKING_ID: i32 = -1;

/// For unify conversion for ABS_MT_POSITION_X, ABS_MT_POSITION_Y.
enum MtPosition {
    X(i64),
    Y(i64),
}

/// A state machine accepts uapi::input_event, produces futinput::TouchInputReport
/// when (Touch Event.. + Sync Event) received.
///
/// This parser currently only supports "Type B" protocol in:
/// https://www.kernel.org/doc/Documentation/input/multi-touch-protocol.txt
///
/// Each event report contains a sequence of packets (uapi::input_event).
/// EV_EYN means the report is completed.
///
/// There may be multiple contacts inside the sequence of packets, each contact
/// data started by a MT_SLOT event with slot_id.
///
/// In the initiated contact, slot (X) will include a ABS_MT_TRACKING_ID
/// event.
/// In following events in slot (X) will continue use the same TRACKING_ID.
/// Slot (X) with TRACKING_ID (-1) means the contact is lifted.
///
/// Warning output, clean state and return errno when received events:
/// - unknown events type / code.
/// - "Type A" events: SYN_MT_REPORT.
/// - invalid event.
/// - not follow "Type B" pattern.
#[derive(Debug, Default, PartialEq)]
pub struct LinuxTouchEventParser {
    /// Store slot id -> tracking id mapping for Type B protocol. Remove the
    /// mapping when contact lifted.
    slot_id_to_tracking_id: SortedVecMap<SlotId, TrackingId>,
    /// Store slot id -> Contact.
    slot_id_to_contact: SortedVecMap<SlotId, futinput::ContactInputReport>,

    /// Store received events while conversion still ongoing.
    cached_events: Vec<uapi::input_event>,

    // Following states only when start parsing one event sequence (SYN_REPORT
    // received).
    /// There will be multiple slots in one event sequence, this field records
    /// the current parsing slot's id.
    current_slot_id: Option<SlotId>,
    /// This record processed slots' id to check if duplicated slot id appear
    /// in one event sequence.
    processed_slots: SortedVecSet<SlotId>,

    /// Allowing single pointer sequence without leading MT_SLOT, will set the
    /// pointer to slot 0.
    single_pointer_sequence: bool,
}

impl LinuxTouchEventParser {
    /// Create the LinuxTouchEventParser.
    pub fn create() -> Self {
        Self {
            slot_id_to_tracking_id: SortedVecMap::new(),
            slot_id_to_contact: SortedVecMap::new(),
            cached_events: vec![],
            current_slot_id: None,
            processed_slots: SortedVecSet::new(),
            single_pointer_sequence: false,
        }
    }

    /// Clean states stored in the parser, call when parser got any error.
    fn reset_state(&mut self) {
        self.cached_events = vec![];
        self.slot_id_to_tracking_id = SortedVecMap::new();
        self.slot_id_to_contact = SortedVecMap::new();

        self.reset_sequence_state();
    }

    /// Clean state for parsing sequence, call for parsing sequence begin and end.
    fn reset_sequence_state(&mut self) {
        self.current_slot_id = None;
        self.processed_slots = SortedVecSet::new();
        self.single_pointer_sequence = false;
    }

    /// There are 2 possible state for a MT_SLOT event:
    /// - This is the first slot so no previous slot.
    /// - This is the end of previous slot.
    ///   * Return errno if duplicated slot found.
    ///   * Add the current contact to the list if the current contact is
    ///     valid, otherwise return errno.
    ///
    /// Add the slot id to processed_slots, current_slot_id and reset
    /// current_contact.
    fn mt_slot(&mut self, new_slot_id: SlotId) -> Result<(), Errno> {
        if self.single_pointer_sequence {
            log_warn!("sequence contains events in slot and out of slot");
            self.reset_state();
            return error!(EINVAL);
        }

        if self.processed_slots.contains(&new_slot_id) {
            log_warn!("duplicated slot_id in one sequence, slot_id = {}", new_slot_id);
            self.reset_state();
            return error!(EINVAL);
        }

        self.processed_slots.insert(new_slot_id);
        self.current_slot_id = Some(new_slot_id);

        if !self.slot_id_to_contact.contains_key(&new_slot_id) {
            self.slot_id_to_contact.insert(
                new_slot_id,
                futinput::ContactInputReport {
                    contact_id: Some(new_slot_id as u32),
                    ..Default::default()
                },
            );
        }

        Ok(())
    }

    /// Type B requires ABS events leading by a MT_SLOT.
    /// Returns SlotId if the requirement meet,
    /// else fallback to single_pointer_sequence.
    fn get_current_slot_id_or_err(&mut self, curr_event: &str) -> Result<SlotId, Errno> {
        match self.current_slot_id {
            Some(slot_id) => Ok(slot_id),
            None => {
                log_warn!(
                    "{:?} is not following ABS_MT_SLOT, fallback to single_pointer_sequence",
                    curr_event
                );
                let res = self.mt_slot(0);
                match res {
                    Ok(_) => {
                        self.single_pointer_sequence = true;
                        Ok(0)
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// MT_TRACKING_ID associate tracking id with slot id, this event is must
    /// have for a slot first appear in event sequences. Add slot id ->
    /// tracking id mapping, this only used to verify the tracking id is not
    /// changed.
    ///
    /// Tracking id = -1 means the contact is lifted, the slot id -> tracking
    /// id mapping should also be removed after this.
    ///
    /// Tracking id should not change otherwise, return errno.
    ///
    /// Returns errno if no leading MT_SLOT.
    fn mt_tracking_id(&mut self, tracking_id: i32) -> Result<(), Errno> {
        let slot_id = self.get_current_slot_id_or_err("ABS_MT_TRACKING_ID")?;

        if tracking_id < LIFTED_TRACKING_ID {
            // TRACKING_ID < -1, invalid value.
            log_warn!("invalid TRACKING_ID {}", tracking_id);
            self.reset_state();
            return error!(EINVAL);
        }

        if tracking_id == LIFTED_TRACKING_ID {
            self.slot_id_to_tracking_id.remove(&slot_id);
            self.slot_id_to_contact.remove(&slot_id);

            return Ok(());
        }

        // A valid TRACKING_ID. Check if it is changed.
        let tid = tracking_id as TrackingId;
        match self.slot_id_to_tracking_id.get(&slot_id) {
            Some(id) => {
                if tid != *id {
                    log_warn!(
                        "TRACKING_ID changed form {} to {} for unknown reason for slot {}",
                        *id,
                        tid,
                        slot_id
                    );
                    self.reset_state();
                    return error!(EINVAL);
                }
            }
            None => {
                self.slot_id_to_tracking_id.insert(slot_id, tid);
            }
        }

        Ok(())
    }

    /// Set contact position. Returns errno if:
    /// - no leading MT_SLOT.
    /// - contact is lifted.
    fn mt_position_x_y(&mut self, mt_position: MtPosition) -> Result<(), Errno> {
        let ty = match mt_position {
            MtPosition::X(_) => "ABS_MT_POSITION_X",
            MtPosition::Y(_) => "ABS_MT_POSITION_Y",
        };
        let slot_id = self.get_current_slot_id_or_err(ty)?;

        match self.slot_id_to_contact.get_mut(&slot_id) {
            Some(contact) => {
                match mt_position {
                    MtPosition::X(x) => {
                        contact.position_x = Some(x);
                    }
                    MtPosition::Y(y) => {
                        contact.position_y = Some(y);
                    }
                }
                Ok(())
            }
            None => {
                log_warn!("current_contact is None when set position");
                self.reset_state();
                return error!(EINVAL);
            }
        }
    }

    fn produce_input_report(&mut self) -> Result<Option<futinput::TouchInputReport>, Errno> {
        self.reset_sequence_state();

        let cached_events = std::mem::take(&mut self.cached_events);

        for e in cached_events {
            match e.code as u32 {
                uapi::ABS_MT_SLOT => {
                    let slot_id = e.value as SlotId;
                    self.mt_slot(slot_id)?;
                }
                uapi::ABS_MT_TRACKING_ID => {
                    self.mt_tracking_id(e.value)?;
                }
                uapi::ABS_MT_POSITION_X => {
                    self.mt_position_x_y(MtPosition::X(e.value as i64))?;
                }
                uapi::ABS_MT_POSITION_Y => {
                    self.mt_position_x_y(MtPosition::Y(e.value as i64))?;
                }
                _ => {
                    // handle() ensure only 4 event_code above will be stored in cached_events.
                    unreachable!();
                }
            }
        }

        let mut contacts: Vec<futinput::ContactInputReport> = vec![];
        for contact in self.slot_id_to_contact.values() {
            if validate_contact_input_report(contact) {
                contacts.push(contact.clone());
            } else {
                log_warn!(
                    "current contact does not have required information, current_contact = {:?}",
                    contact
                );
                self.reset_state();
                return error!(EINVAL);
            }
        }

        // This `unwrap` is safe because `validate_contact_input_report()` ensured `contact_id` exists.
        contacts.sort_by(|a, b| a.contact_id.unwrap().cmp(&b.contact_id.unwrap()));

        let res =
            Ok(Some(futinput::TouchInputReport { contacts: Some(contacts), ..Default::default() }));

        self.reset_sequence_state();

        res
    }

    /// Handle received input_event, only produce event when SYN_REPORT is received.
    pub fn handle(
        &mut self,
        e: uapi::input_event,
    ) -> Result<Option<futinput::TouchInputReport>, Errno> {
        let event_code = e.code as u32;
        match e.type_ as u32 {
            uapi::EV_SYN => match event_code {
                uapi::SYN_REPORT => self.produce_input_report(),
                uapi::SYN_MT_REPORT => {
                    log_warn!("Touchscreen got 'Type A' event SYN_MT_REPORT");
                    self.reset_state();
                    error!(EINVAL)
                }
                _ => {
                    log_warn!("Touchscreen got unexpected EV_SYN, event = {:?}", e);
                    self.reset_state();
                    error!(EINVAL)
                }
            },
            uapi::EV_ABS => match event_code {
                uapi::ABS_MT_SLOT
                | uapi::ABS_MT_TRACKING_ID
                | uapi::ABS_MT_POSITION_X
                | uapi::ABS_MT_POSITION_Y => {
                    self.cached_events.push(e);
                    Ok(None)
                }
                uapi::ABS_MT_TOUCH_MAJOR
                | uapi::ABS_MT_TOUCH_MINOR
                | uapi::ABS_MT_WIDTH_MAJOR
                | uapi::ABS_MT_WIDTH_MINOR
                | uapi::ABS_MT_ORIENTATION
                | uapi::ABS_MT_TOOL_TYPE
                | uapi::ABS_MT_BLOB_ID
                | uapi::ABS_MT_PRESSURE
                | uapi::ABS_MT_DISTANCE
                | uapi::ABS_MT_TOOL_X
                | uapi::ABS_MT_TOOL_Y => {
                    // We don't use these event. Just respsond Ok.
                    Ok(None)
                }
                _ => {
                    log_warn!("Touchscreen got unexpected EV_ABS, event = {:?}", e);
                    self.reset_state();
                    error!(EINVAL)
                }
            },
            uapi::EV_KEY => {
                match event_code {
                    // For "Type B" protocol, BTN_TOUCH can be ignored.
                    uapi::BTN_TOUCH => Ok(None),
                    _ => {
                        log_warn!("Touchscreen got unexpected EV_KEY, event = {:?}", e);
                        self.reset_state();
                        error!(EINVAL)
                    }
                }
            }
            _ => {
                log_warn!("Touchscreen got unexpected event type, got = {:?}", e);
                self.reset_state();
                error!(EINVAL)
            }
        }
    }
}

/// ContactInputReport should contains X, Y, Contact ID
fn validate_contact_input_report(c: &futinput::ContactInputReport) -> bool {
    match c {
        &futinput::ContactInputReport {
            contact_id: Some(_),
            position_x: Some(_),
            position_y: Some(_),
            ..
        } => true,
        _ => false,
    }
}

#[cfg(test)]
mod touchscreen_linux_fuchsia_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use test_case::test_case;
    use uapi::timeval;

    fn input_event(ty: u32, code: u32, value: i32) -> uapi::input_event {
        uapi::input_event { time: timeval::default(), type_: ty as u16, code: code as u16, value }
    }

    #[test]
    fn handle_btn_touch_ok_does_not_produce_input_report() {
        let e = input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 1);
        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(e), Ok(None));
        assert_eq!(parser, LinuxTouchEventParser::default());
    }

    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1); "ABS_MT_SLOT")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1); "ABS_MT_TRACKING_ID")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 1); "ABS_MT_POSITION_X")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 1); "ABS_MT_POSITION_Y")]
    fn handle_input_event_ok_does_not_produce_input_report(e: uapi::input_event) {
        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(e), Ok(None));
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![e],
                slot_id_to_tracking_id: SortedVecMap::new(),
                ..LinuxTouchEventParser::default()
            }
        );
    }

    #[test_case(input_event(uapi::EV_KEY, uapi::KEY_A, 1); "unsupported keycode")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_PRESSURE, 1); "unsupported ABS event")]
    #[test_case(input_event(uapi::EV_SYN, uapi::SYN_MT_REPORT, 1); "Type A")]
    #[test_case(input_event(uapi::EV_SYN, uapi::SYN_CONFIG, 1); "unsupported SYN event")]
    fn handle_input_event_error(e: uapi::input_event) {
        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(e), error!(EINVAL));
        assert_eq!(parser, LinuxTouchEventParser::default());
    }

    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_TOUCH_MAJOR, 1); "ignore ABS_MT_TOUCH_MAJOR event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_TOUCH_MINOR, 1); "ignore ABS_MT_TOUCH_MINOR event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_WIDTH_MAJOR, 1); "ignore ABS_MT_WIDTH_MAJOR event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_WIDTH_MINOR, 1); "ignore ABS_MT_WIDTH_MINOR event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_ORIENTATION, 1); "ignore ABS_MT_ORIENTATION event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_TOOL_TYPE, 1); "ignore ABS_MT_TOOL_TYPE event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_BLOB_ID, 1); "ignore ABS_MT_BLOB_ID event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_PRESSURE, 1); "ignore ABS_MT_PRESSURE event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_DISTANCE, 1); "ignore ABS_MT_DISTANCE event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_TOOL_X, 1); "ignore ABS_MT_TOOL_X event")]
    #[test_case(input_event(uapi::EV_ABS, uapi::ABS_MT_TOOL_Y , 1); "ignore ABS_MT_TOOL_Y event")]
    fn handle_input_event_ignore(e: uapi::input_event) {
        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(e), Ok(None));
        assert_eq!(parser, LinuxTouchEventParser::default());
    }

    #[test]
    fn no_slot_leading_event_fallback_to_single_pointer_mode() {
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1)), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 2)), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 3)), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(0),
                    position_x: Some(2),
                    position_y: Some(3),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1)]),
                slot_id_to_contact: SortedVecMap::from([(
                    0,
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(2),
                        position_y: Some(3),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );
    }

    #[test]
    fn single_pointer_mode_slot_does_not_have_enough_information() {
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1)), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 2)), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 3)), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(0),
                    position_x: Some(2),
                    position_y: Some(3),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1)]),
                slot_id_to_contact: SortedVecMap::from([(
                    0,
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(2),
                        position_y: Some(3),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );

        // the second event does not have x.
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 4)), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(0),
                    position_x: Some(2),
                    position_y: Some(4),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1)]),
                slot_id_to_contact: SortedVecMap::from([(
                    0,
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(2),
                        position_y: Some(4),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );

        // only contains pressure event.
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_PRESSURE, 10)), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(0),
                    position_x: Some(2),
                    position_y: Some(4),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1)]),
                slot_id_to_contact: SortedVecMap::from([(
                    0,
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(2),
                        position_y: Some(4),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );
    }

    #[test]
    fn slot_has_only_pressure_event() {
        let slot_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0);
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1)), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 2)), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 3)), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(0),
                    position_x: Some(2),
                    position_y: Some(3),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1)]),
                slot_id_to_contact: SortedVecMap::from([(
                    0,
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(2),
                        position_y: Some(3),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );

        // next event only contains pressure
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(input_event(uapi::EV_ABS, uapi::ABS_MT_PRESSURE, 10)), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(0),
                    position_x: Some(2),
                    position_y: Some(3),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1)]),
                slot_id_to_contact: SortedVecMap::from([(
                    0,
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(2),
                        position_y: Some(3),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );
    }

    #[test]
    fn slot_does_not_have_enough_information() {
        let slot_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0);
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();

        // The last slot does not have enough information.
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(syn), error!(EINVAL));
        assert_eq!(parser, LinuxTouchEventParser::default());

        // The first slot does not have enough information.
        let slot_1 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1);
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(slot_1), Ok(None));
        assert_eq!(parser.handle(syn), error!(EINVAL));
        assert_eq!(parser, LinuxTouchEventParser::default());
    }

    #[test]
    fn same_slot_id_in_one_event() {
        let slot_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0);
        let traking_id = input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 0);
        let x = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 0);
        let y = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 0);
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(traking_id), Ok(None));
        assert_eq!(parser.handle(x), Ok(None));
        assert_eq!(parser.handle(y), Ok(None));
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(syn), error!(EINVAL));
        assert_eq!(parser, LinuxTouchEventParser::default());
    }

    #[test]
    fn tracking_id_changed_in_slot() {
        let slot_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0);
        let traking_id_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 0);
        let traking_id_1 = input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1);
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(traking_id_0), Ok(None));
        assert_eq!(parser.handle(traking_id_1), Ok(None));
        assert_eq!(parser.handle(syn), error!(EINVAL));
        assert_eq!(parser, LinuxTouchEventParser::default());
    }

    #[test]
    fn tracking_id_different_with_parser_recorded() {
        let slot_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0);
        let traking_id_1 = input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1);
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();
        parser.slot_id_to_tracking_id.insert(0, 0);
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(traking_id_1), Ok(None));
        assert_eq!(parser.handle(syn), error!(EINVAL));
        assert_eq!(parser, LinuxTouchEventParser::default());
    }

    #[test]
    fn produce_input_report() {
        // 1 contact.
        let slot_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0);
        let traking_id_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1);
        let x_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 2);
        let y_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 3);
        let syn = input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0);

        let mut parser = LinuxTouchEventParser::create();
        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(traking_id_0), Ok(None));
        assert_eq!(parser.handle(x_0), Ok(None));
        assert_eq!(parser.handle(y_0), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(0),
                    position_x: Some(2),
                    position_y: Some(3),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1)]),
                slot_id_to_contact: SortedVecMap::from([(
                    0,
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(2),
                        position_y: Some(3),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );

        // 2 contact.
        let x_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 4);
        let y_0 = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 5);

        let slot_1 = input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1);
        let traking_id_1 = input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 2);
        let x_1 = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10);
        let y_1 = input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 11);

        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(x_0), Ok(None));
        assert_eq!(parser.handle(y_0), Ok(None));
        assert_eq!(parser.handle(slot_1), Ok(None));
        assert_eq!(parser.handle(traking_id_1), Ok(None));
        assert_eq!(parser.handle(x_1), Ok(None));
        assert_eq!(parser.handle(y_1), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![
                    futinput::ContactInputReport {
                        contact_id: Some(0),
                        position_x: Some(4),
                        position_y: Some(5),
                        ..Default::default()
                    },
                    futinput::ContactInputReport {
                        contact_id: Some(1),
                        position_x: Some(10),
                        position_y: Some(11),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }))
        );
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(0, 1), (1, 2)]),
                slot_id_to_contact: SortedVecMap::from([
                    (
                        0,
                        futinput::ContactInputReport {
                            contact_id: Some(0),
                            position_x: Some(4),
                            position_y: Some(5),
                            ..Default::default()
                        }
                    ),
                    (
                        1,
                        futinput::ContactInputReport {
                            contact_id: Some(1),
                            position_x: Some(10),
                            position_y: Some(11),
                            ..Default::default()
                        }
                    )
                ]),
                ..LinuxTouchEventParser::default()
            }
        );

        // lift the first contact.
        let tracking_id_lifted = input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1);

        assert_eq!(parser.handle(slot_0), Ok(None));
        assert_eq!(parser.handle(tracking_id_lifted), Ok(None));
        assert_eq!(parser.handle(slot_1), Ok(None));
        assert_eq!(parser.handle(x_1), Ok(None));
        assert_eq!(parser.handle(y_1), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport {
                contacts: Some(vec![futinput::ContactInputReport {
                    contact_id: Some(1),
                    position_x: Some(10),
                    position_y: Some(11),
                    ..Default::default()
                }]),
                ..Default::default()
            }))
        );
        // should remove the mapping.
        assert_eq!(
            parser,
            LinuxTouchEventParser {
                cached_events: vec![],
                slot_id_to_tracking_id: SortedVecMap::from([(1, 2)]),
                slot_id_to_contact: SortedVecMap::from([(
                    1,
                    futinput::ContactInputReport {
                        contact_id: Some(1),
                        position_x: Some(10),
                        position_y: Some(11),
                        ..Default::default()
                    }
                )]),
                ..LinuxTouchEventParser::default()
            }
        );

        // lift all contact.
        assert_eq!(parser.handle(slot_1), Ok(None));
        assert_eq!(parser.handle(tracking_id_lifted), Ok(None));
        assert_eq!(
            parser.handle(syn),
            Ok(Some(futinput::TouchInputReport { contacts: Some(vec![]), ..Default::default() }))
        );
        // should remove the mapping.
        assert_eq!(parser, LinuxTouchEventParser::default());
    }
}
