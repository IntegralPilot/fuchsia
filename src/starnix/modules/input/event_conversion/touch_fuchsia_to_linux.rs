// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ui_pointer::{
    EventPhase as FidlEventPhase, TouchEvent as FidlTouchEvent, TouchPointerSample,
};
use sorted_vec_map::SortedVecMap;
use starnix_logging::log_warn;
use starnix_types::time::timeval_from_time;
use starnix_uapi::uapi;
use std::collections::VecDeque;

type SlotId = usize;
type TrackingId = u32;
type TimeNanos = i64;

/// TRACKING_ID changed to -1 means the contact is lifted.
const LIFTED_TRACKING_ID: i32 = -1;

#[derive(Debug, thiserror::Error)]
enum TouchEventConversionError {
    #[error("Event does not include enough information")]
    NotEnoughInformation,
    #[error("no more available slot id")]
    NoMoreAvailableSlotId,
    #[error("receive pointer add already added")]
    PointerAdded,
    #[error("receive pointer change/remove before added")]
    PointerNotFound,
    #[error("Input pipeline does not send out Cancel")]
    PointerCancel,
}

#[derive(Debug, Clone, PartialEq)]
struct TouchEvent {
    time_nanos: TimeNanos,
    pointer_id: TrackingId,
    phase: FidlEventPhase,
    x: i32,
    y: i32,
}

impl TryFrom<FidlTouchEvent> for TouchEvent {
    type Error = TouchEventConversionError;
    fn try_from(e: FidlTouchEvent) -> Result<TouchEvent, Self::Error> {
        match e {
            FidlTouchEvent {
                timestamp: Some(time_nanos),
                pointer_sample:
                    Some(TouchPointerSample {
                        position_in_viewport: Some([x, y]),
                        phase: Some(phase),
                        interaction: Some(id),
                        ..
                    }),
                ..
            } => Ok(TouchEvent {
                time_nanos,
                pointer_id: id.pointer_id,
                phase,
                x: x as i32,
                y: y as i32,
            }),
            _ => Err(TouchEventConversionError::NotEnoughInformation),
        }
    }
}

/// FuchsiaTouchEventToLinuxTouchEventConverter handles fuchsia.ui.pointer.TouchEvents
/// and converts them to Linux uapi::input_event in Multi Touch Protocol B.
#[derive(Debug, Default, PartialEq)]
pub struct FuchsiaTouchEventToLinuxTouchEventConverter {
    pointer_id_to_slot_id: SortedVecMap<TrackingId, SlotId>,
    pointer_id_to_event: SortedVecMap<TrackingId, TouchEvent>,
}

const MAX_TOUCH_CONTACT: usize = 10;

pub struct LinuxTouchEventBatch {
    // Linux Multi Touch Protocol B events
    pub events: VecDeque<uapi::input_event>,
    pub last_event_time_ns: i64,
    pub count_converted_fidl_events: u64,
    pub count_ignored_fidl_events: u64,
    pub count_unexpected_fidl_events: u64,
}

impl LinuxTouchEventBatch {
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            last_event_time_ns: 0,
            count_converted_fidl_events: 0,
            count_ignored_fidl_events: 0,
            count_unexpected_fidl_events: 0,
        }
    }
}

impl FuchsiaTouchEventToLinuxTouchEventConverter {
    pub fn create() -> Self {
        Self {
            pointer_id_to_slot_id: SortedVecMap::new(),
            pointer_id_to_event: SortedVecMap::new(),
        }
    }

    /// In Protocol B, the driver should only advertise as many slots as the hardware can report
    /// so this converter uses `available_slot_id` to find the first available slot id.
    fn available_slot_id(&self) -> Option<SlotId> {
        let mut used_slot_ids = bit_vec::BitVec::<u32>::from_elem(MAX_TOUCH_CONTACT, false);
        for slot_id in self.pointer_id_to_slot_id.values() {
            used_slot_ids.set(*slot_id, true);
        }

        used_slot_ids.iter().position(|used| !used)
    }

    /// Converts fidl touch events to a batch of Linux Multi Touch Protocol B events.
    ///
    /// One vector of fidl touch events may convert to multiple Linux Multi Touch Protocol B
    /// sequences because:
    /// - Same pointer happens multiple times in the vector of fidl touch events.
    /// - Linux Multi Touch Protocol B does not allow slot with same id appear multiple times
    ///   one sequence.
    pub fn handle(&mut self, events: Vec<FidlTouchEvent>) -> LinuxTouchEventBatch {
        let mut batch = LinuxTouchEventBatch::new();

        // TODO(https://fxbug.dev/348726475): Group events by timestamp here because events from
        // fuchsia.ui.pointer.touch.Watch may not sorted by timestamp.
        let mut sequences: SortedVecMap<TimeNanos, Vec<TouchEvent>> = SortedVecMap::new();
        for event in events.into_iter() {
            match TouchEvent::try_from(event) {
                Ok(e) => {
                    if let Some(vec) = sequences.get_mut(&e.time_nanos) {
                        vec.push(e);
                    } else {
                        sequences.insert(e.time_nanos, vec![e]);
                    }
                }
                Err(_) => {
                    batch.count_ignored_fidl_events += 1;
                }
            }
        }

        if sequences.is_empty() {
            return batch;
        }

        batch.last_event_time_ns = *sequences.iter().next_back().unwrap().0;

        for (time_nanos, seq) in sequences.iter() {
            let count_events = seq.len() as u64;
            match self.translate_sequence(*time_nanos, seq) {
                Ok(mut res) => {
                    batch.events.append(&mut res);
                    batch.count_converted_fidl_events += count_events;
                }
                Err(e) => {
                    batch.count_unexpected_fidl_events += count_events;
                    self.reset_state();
                    log_warn!("{}", e);
                }
            }
        }

        batch
    }

    /// Translates a vec of fidl FidlTouchEvent to Linux Multi Touch Protocol B sequence. Caller
    /// ensures the given vec does not include duplicated pointer, and all event includes same
    /// timestamp.
    ///
    /// Return err for unexpected events which should be filtered in earlier component:
    /// input-pipeline and scenic. If 1 event is unexpected, translate_sequence() drops all events
    /// from the same scan from driver.
    fn translate_sequence(
        &mut self,
        time_nanos: TimeNanos,
        events: &Vec<TouchEvent>,
    ) -> Result<VecDeque<uapi::input_event>, TouchEventConversionError> {
        let mut existing_slot: VecDeque<uapi::input_event> = VecDeque::new();
        let mut new_slots: VecDeque<uapi::input_event> = VecDeque::new();

        let time = timeval_from_time(zx::MonotonicInstant::from_nanos(time_nanos));

        let no_contact_before_process_events = self.pointer_id_to_slot_id.is_empty();
        let mut need_btn_touch_down = false;
        let mut need_btn_touch_up = false;

        // TODO(https://fxbug.dev/314151713): use event.device_info to route event to different
        // device file.

        for (index, event) in events.iter().enumerate() {
            let pointer_id = event.pointer_id;
            let slot_id = self.pointer_id_to_slot_id.get(&pointer_id).copied();
            let previous_event = self.pointer_id_to_event.insert(pointer_id, event.clone());

            match event.phase {
                FidlEventPhase::Add => match (slot_id, previous_event) {
                    (None, None) => {
                        let new_slot_id = match self.available_slot_id() {
                            Some(index) => index,
                            None => {
                                return Err(TouchEventConversionError::NoMoreAvailableSlotId);
                            }
                        };

                        if no_contact_before_process_events {
                            need_btn_touch_down = true;
                        }

                        self.pointer_id_to_slot_id.insert(pointer_id, new_slot_id);

                        new_slots.push_back(uapi::input_event {
                            time,
                            type_: uapi::EV_ABS as u16,
                            code: uapi::ABS_MT_SLOT as u16,
                            value: new_slot_id as i32,
                        });

                        new_slots.push_back(uapi::input_event {
                            time,
                            type_: uapi::EV_ABS as u16,
                            code: uapi::ABS_MT_TRACKING_ID as u16,
                            value: pointer_id as i32,
                        });

                        new_slots.push_back(uapi::input_event {
                            time,
                            type_: uapi::EV_ABS as u16,
                            code: uapi::ABS_MT_POSITION_X as u16,
                            value: event.x,
                        });

                        new_slots.push_back(uapi::input_event {
                            time,
                            type_: uapi::EV_ABS as u16,
                            code: uapi::ABS_MT_POSITION_Y as u16,
                            value: event.y,
                        });
                    }
                    (_, _) => {
                        return Err(TouchEventConversionError::PointerAdded);
                    }
                },
                FidlEventPhase::Change => match (slot_id, previous_event) {
                    (Some(slot_id), Some(prev)) => {
                        if prev.x != event.x || prev.y != event.y {
                            existing_slot.push_back(uapi::input_event {
                                time,
                                type_: uapi::EV_ABS as u16,
                                code: uapi::ABS_MT_SLOT as u16,
                                value: slot_id as i32,
                            });
                        }

                        if prev.x != event.x {
                            existing_slot.push_back(uapi::input_event {
                                time,
                                type_: uapi::EV_ABS as u16,
                                code: uapi::ABS_MT_POSITION_X as u16,
                                value: event.x,
                            });
                        }

                        if prev.y != event.y {
                            existing_slot.push_back(uapi::input_event {
                                time,
                                type_: uapi::EV_ABS as u16,
                                code: uapi::ABS_MT_POSITION_Y as u16,
                                value: event.y,
                            });
                        }
                    }
                    (_, _) => {
                        return Err(TouchEventConversionError::PointerNotFound);
                    }
                },
                FidlEventPhase::Remove => match (slot_id, previous_event) {
                    (Some(slot_id), Some(_)) => {
                        self.pointer_id_to_slot_id.remove(&pointer_id);
                        self.pointer_id_to_event.remove(&pointer_id);

                        // Ensure BTN_TOUCH up event is only sent when the last pointer is lifted.
                        // Here check if the event is the last event from the vec prevents a false
                        // BTN_TOUCH up event if any error reset_state of this converter.
                        if index == events.len() - 1 && self.pointer_id_to_slot_id.is_empty() {
                            need_btn_touch_up = true;
                        }

                        existing_slot.push_back(uapi::input_event {
                            time,
                            type_: uapi::EV_ABS as u16,
                            code: uapi::ABS_MT_SLOT as u16,
                            value: slot_id as i32,
                        });

                        existing_slot.push_back(uapi::input_event {
                            time,
                            type_: uapi::EV_ABS as u16,
                            code: uapi::ABS_MT_TRACKING_ID as u16,
                            value: LIFTED_TRACKING_ID,
                        });
                    }
                    (_, _) => {
                        return Err(TouchEventConversionError::PointerNotFound);
                    }
                },
                FidlEventPhase::Cancel => {
                    return Err(TouchEventConversionError::PointerCancel);
                }
            }
        }

        let mut result: VecDeque<uapi::input_event> = VecDeque::new();

        result.append(&mut existing_slot);
        result.append(&mut new_slots);

        if need_btn_touch_down {
            result.push_back(uapi::input_event {
                time,
                type_: uapi::EV_KEY as u16,
                code: uapi::BTN_TOUCH as u16,
                value: 1,
            });
        } else if need_btn_touch_up {
            result.push_back(uapi::input_event {
                time,
                type_: uapi::EV_KEY as u16,
                code: uapi::BTN_TOUCH as u16,
                value: 0,
            });
        }

        if result.len() > 0 {
            result.push_back(uapi::input_event {
                time,
                type_: uapi::EV_SYN as u16,
                code: uapi::SYN_REPORT as u16,
                value: 0,
            });
        }

        Ok(result)
    }

    fn reset_state(&mut self) {
        self.pointer_id_to_slot_id = SortedVecMap::new();
        self.pointer_id_to_event = SortedVecMap::new();
    }
}

#[cfg(test)]
mod touchscreen_fuchsia_linux_tests {
    use super::*;
    use fidl_fuchsia_ui_pointer::TouchInteractionId;
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    fn make_touch_event_with_coords_phase_id_time(
        x: f32,
        y: f32,
        phase: FidlEventPhase,
        pointer_id: u32,
        time_nanos: i64,
    ) -> FidlTouchEvent {
        FidlTouchEvent {
            timestamp: Some(time_nanos),
            pointer_sample: Some(TouchPointerSample {
                position_in_viewport: Some([x, y]),
                phase: Some(phase),
                interaction: Some(TouchInteractionId {
                    pointer_id,
                    device_id: 0,
                    interaction_id: 0,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_touch_event_with_coords_phase_id(
        x: f32,
        y: f32,
        phase: FidlEventPhase,
        pointer_id: u32,
    ) -> FidlTouchEvent {
        make_touch_event_with_coords_phase_id_time(x, y, phase, pointer_id, 0)
    }

    fn make_uapi_input_event_with_time(
        ty: u32,
        code: u32,
        value: i32,
        time_nanos: i64,
    ) -> uapi::input_event {
        uapi::input_event {
            time: timeval_from_time(zx::MonotonicInstant::from_nanos(time_nanos)),
            type_: ty as u16,
            code: code as u16,
            value,
        }
    }

    fn make_uapi_input_event(ty: u32, code: u32, value: i32) -> uapi::input_event {
        make_uapi_input_event_with_time(ty, code, value, 0)
    }

    fn make_internal_touch_event(
        time_nanos: i64,
        x: i32,
        y: i32,
        phase: FidlEventPhase,
        pointer_id: u32,
    ) -> TouchEvent {
        TouchEvent { time_nanos, pointer_id, phase, x, y }
    }

    #[test_case(FidlTouchEvent::default(); "not enough fields")]
    fn ignored_events(e: FidlTouchEvent) {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();
        let _ = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Add,
            1,
        )]);
        let batch = converter.handle(vec![e]);
        assert_eq!(batch.events, vec![]);
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 0);
        assert_eq!(batch.count_ignored_fidl_events, 1);
        assert_eq!(batch.count_unexpected_fidl_events, 0);
    }

    #[test_case(make_touch_event_with_coords_phase_id(
        1.0,
        2.0,
        FidlEventPhase::Add,
        1,
    ); "touch add pointer already added")]
    #[test_case(make_touch_event_with_coords_phase_id(
        1.0,
        2.0,
        FidlEventPhase::Change,
        2,
    ); "touch change pointer not added")]
    #[test_case(make_touch_event_with_coords_phase_id(
        0.0,
        0.0,
        FidlEventPhase::Remove,
        2,
    ); "touch remove pointer not added")]
    #[test_case(make_touch_event_with_coords_phase_id(
        0.0,
        0.0,
        FidlEventPhase::Cancel,
        1,
    ); "touch cancel")]
    fn unexpected_events(e: FidlTouchEvent) {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();
        let _ = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Add,
            1,
        )]);
        let batch = converter.handle(vec![e]);
        assert_eq!(batch.events, vec![]);
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 0);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 1);
    }

    #[test]
    fn touch_add() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();
        let batch = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Add,
            1,
        )]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 20),
                make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 1),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 1);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        let mut want_converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();
        want_converter.pointer_id_to_slot_id.insert(1, 0);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 10, 20, FidlEventPhase::Add, 1));

        assert_eq!(converter, want_converter);
    }

    #[test]
    fn touch_change() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();
        let _ = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Add,
            1,
        )]);

        let batch = converter.handle(vec![make_touch_event_with_coords_phase_id(
            11.0,
            21.0,
            FidlEventPhase::Change,
            1,
        )]);
        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 11),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 21),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 1);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        let mut want_converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        want_converter.pointer_id_to_slot_id.insert(1, 0);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 11, 21, FidlEventPhase::Change, 1));

        assert_eq!(converter, want_converter);
    }

    #[test]
    fn touch_change_no_change() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();
        let _ = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Add,
            1,
        )]);

        let batch = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Change,
            1,
        )]);
        assert_eq!(batch.events, vec![]);
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 1);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        let mut want_converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        want_converter.pointer_id_to_slot_id.insert(1, 0);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 10, 20, FidlEventPhase::Change, 1));

        assert_eq!(converter, want_converter);
    }

    #[test]
    fn touch_remove() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();
        let _ = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Add,
            1,
        )]);
        let batch = converter.handle(vec![make_touch_event_with_coords_phase_id(
            0.0,
            0.0,
            FidlEventPhase::Remove,
            1,
        )]);
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 1);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1),
                make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 0),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );

        assert_eq!(
            converter,
            FuchsiaTouchEventToLinuxTouchEventConverter {
                pointer_id_to_slot_id: SortedVecMap::new(),
                pointer_id_to_event: SortedVecMap::new()
            }
        );
    }

    #[test]
    fn multi_touch_sequence() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        // The first pointer down.
        let _ = converter.handle(vec![make_touch_event_with_coords_phase_id(
            10.0,
            20.0,
            FidlEventPhase::Add,
            1,
        )]);

        // The second pointer down, and the first pointer move.
        let batch = converter.handle(vec![
            make_touch_event_with_coords_phase_id(11.0, 21.0, FidlEventPhase::Change, 1),
            make_touch_event_with_coords_phase_id(100.0, 200.0, FidlEventPhase::Add, 2),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 11),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 21),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 2),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 100),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 200),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        let mut want_converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        want_converter.pointer_id_to_slot_id.insert(1, 0);
        want_converter.pointer_id_to_slot_id.insert(2, 1);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 11, 21, FidlEventPhase::Change, 1));
        want_converter
            .pointer_id_to_event
            .insert(2, make_internal_touch_event(0, 100, 200, FidlEventPhase::Add, 2));

        assert_eq!(converter, want_converter);

        // Both pointer move.
        let batch = converter.handle(vec![
            make_touch_event_with_coords_phase_id(12.0, 22.0, FidlEventPhase::Change, 1),
            make_touch_event_with_coords_phase_id(101.0, 201.0, FidlEventPhase::Change, 2),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 12),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 22),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 101),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 201),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 12, 22, FidlEventPhase::Change, 1));
        want_converter
            .pointer_id_to_event
            .insert(2, make_internal_touch_event(0, 101, 201, FidlEventPhase::Change, 2));
        assert_eq!(converter, want_converter);

        // The second pointer up, and the first pointer move.
        let batch = converter.handle(vec![
            make_touch_event_with_coords_phase_id(13.0, 23.0, FidlEventPhase::Change, 1),
            make_touch_event_with_coords_phase_id(0.0, 0.0, FidlEventPhase::Remove, 2),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 13),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 23),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 13, 23, FidlEventPhase::Change, 1));
        want_converter.pointer_id_to_slot_id.remove(&2);
        want_converter.pointer_id_to_event.remove(&2);

        assert_eq!(converter, want_converter);

        // The third pointer down, and the first pointer move.
        let batch = converter.handle(vec![
            make_touch_event_with_coords_phase_id(14.0, 24.0, FidlEventPhase::Change, 1),
            make_touch_event_with_coords_phase_id(50.0, 60.0, FidlEventPhase::Add, 3),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 14),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 24),
                // should reuse slot id 1.
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 3),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 50),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 60),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        want_converter.pointer_id_to_slot_id.insert(3, 1);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 14, 24, FidlEventPhase::Change, 1));
        want_converter
            .pointer_id_to_event
            .insert(3, make_internal_touch_event(0, 50, 60, FidlEventPhase::Add, 3));

        assert_eq!(converter, want_converter);

        // The third pointer up, and the first pointer move.
        let batch = converter.handle(vec![
            make_touch_event_with_coords_phase_id(15.0, 25.0, FidlEventPhase::Change, 1),
            make_touch_event_with_coords_phase_id(0.0, 0.0, FidlEventPhase::Remove, 3),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 15),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 25),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 15, 25, FidlEventPhase::Change, 1));
        want_converter.pointer_id_to_slot_id.remove(&3);
        want_converter.pointer_id_to_event.remove(&3);

        assert_eq!(converter, want_converter);

        // The first pointer up.
        let batch = converter.handle(vec![make_touch_event_with_coords_phase_id(
            0.0,
            0.0,
            FidlEventPhase::Remove,
            1,
        )]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1),
                make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 0),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 1);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        want_converter.reset_state();
        assert_eq!(converter, want_converter);
    }

    #[test]
    fn multi_touch_sequence_receive_only_one_pointer_change_when_two_pointer_contacting() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        // 2 pointer down.
        let batch = converter.handle(vec![
            make_touch_event_with_coords_phase_id(10.0, 20.0, FidlEventPhase::Add, 1),
            make_touch_event_with_coords_phase_id(100.0, 200.0, FidlEventPhase::Add, 2),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 20),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 2),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 100),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 200),
                make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 1),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        let mut want_converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        want_converter.pointer_id_to_slot_id.insert(1, 0);
        want_converter.pointer_id_to_slot_id.insert(2, 1);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 10, 20, FidlEventPhase::Add, 1));
        want_converter
            .pointer_id_to_event
            .insert(2, make_internal_touch_event(0, 100, 200, FidlEventPhase::Add, 2));
        assert_eq!(converter, want_converter);

        // 1st pointer move, no event for 2nd pointer.
        let batch = converter.handle(vec![make_touch_event_with_coords_phase_id(
            12.0,
            22.0,
            FidlEventPhase::Change,
            1,
        )]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 12),
                make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 22),
                make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 0);
        assert_eq!(batch.count_converted_fidl_events, 1);
        assert_eq!(batch.count_ignored_fidl_events, 0);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(0, 12, 22, FidlEventPhase::Change, 1));
        assert_eq!(converter, want_converter);
    }

    #[test]
    fn handle_return_multi_protocl_b_seq() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        let batch = converter.handle(vec![
            // ignore
            FidlTouchEvent::default(),
            make_touch_event_with_coords_phase_id_time(10.0, 20.0, FidlEventPhase::Add, 1, 1),
            make_touch_event_with_coords_phase_id_time(11.0, 21.0, FidlEventPhase::Change, 1, 1000),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 20, 1),
                make_uapi_input_event_with_time(uapi::EV_KEY, uapi::BTN_TOUCH, 1, 1),
                make_uapi_input_event_with_time(uapi::EV_SYN, uapi::SYN_REPORT, 0, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0, 1000),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 11, 1000),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 21, 1000),
                make_uapi_input_event_with_time(uapi::EV_SYN, uapi::SYN_REPORT, 0, 1000),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 1000);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 1);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        let mut want_converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        want_converter.pointer_id_to_slot_id.insert(1, 0);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(1000, 11, 21, FidlEventPhase::Change, 1));

        assert_eq!(converter, want_converter);
    }

    #[test]
    fn handle_unsorted_events() {
        let mut converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        let batch = converter.handle(vec![
            // ignore
            FidlTouchEvent::default(),
            make_touch_event_with_coords_phase_id_time(11.0, 21.0, FidlEventPhase::Change, 1, 1000),
            make_touch_event_with_coords_phase_id_time(10.0, 20.0, FidlEventPhase::Add, 1, 1),
        ]);

        assert_eq!(
            batch.events,
            vec![
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 20, 1),
                make_uapi_input_event_with_time(uapi::EV_KEY, uapi::BTN_TOUCH, 1, 1),
                make_uapi_input_event_with_time(uapi::EV_SYN, uapi::SYN_REPORT, 0, 1),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0, 1000),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 11, 1000),
                make_uapi_input_event_with_time(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 21, 1000),
                make_uapi_input_event_with_time(uapi::EV_SYN, uapi::SYN_REPORT, 0, 1000),
            ]
        );
        assert_eq!(batch.last_event_time_ns, 1000);
        assert_eq!(batch.count_converted_fidl_events, 2);
        assert_eq!(batch.count_ignored_fidl_events, 1);
        assert_eq!(batch.count_unexpected_fidl_events, 0);

        let mut want_converter = FuchsiaTouchEventToLinuxTouchEventConverter::create();

        want_converter.pointer_id_to_slot_id.insert(1, 0);
        want_converter
            .pointer_id_to_event
            .insert(1, make_internal_touch_event(1000, 11, 21, FidlEventPhase::Change, 1));

        assert_eq!(converter, want_converter);
    }
}
