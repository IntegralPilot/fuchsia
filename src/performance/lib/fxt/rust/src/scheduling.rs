// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{Arg, RawArg};
use crate::error::ParseWarning;
use crate::init::Ticks;
use crate::session::ResolveCtx;
use crate::thread::{ProcessKoid, ProcessRef, ThreadKoid, ThreadRef};
use crate::{ParseError, ParseResult, Provider, SCHEDULING_RECORD_TYPE, trace_header};
use nom::Parser;
use nom::combinator::all_consuming;
use nom::number::complete::le_u64;
use std::num::NonZero;

const LEGACY_CONTEXT_SWITCH_SCHEDULING_TYPE: u8 = 0;
const CONTEXT_SWITCH_SCHEDULING_TYPE: u8 = 1;
const THREAD_WAKEUP_SCHEDULING_TYPE: u8 = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum SchedulingRecord {
    ContextSwitch(ContextSwitchEvent),
    ThreadWakeup(ThreadWakeupEvent),
    LegacyContextSwitch(LegacyContextSwitchEvent),
}

impl SchedulingRecord {
    /// The incoming process, if any.
    pub fn process(&self) -> Option<ProcessKoid> {
        match self {
            Self::LegacyContextSwitch(LegacyContextSwitchEvent { incoming_process, .. }) => {
                Some(*incoming_process)
            }
            Self::ContextSwitch(..) | Self::ThreadWakeup(..) => None,
        }
    }

    /// The incoming thread, if any.
    pub fn thread(&self) -> ThreadKoid {
        match self {
            Self::LegacyContextSwitch(LegacyContextSwitchEvent { incoming_thread, .. }) => {
                *incoming_thread
            }
            Self::ContextSwitch(ContextSwitchEvent { incoming_thread_id, .. }) => {
                *incoming_thread_id
            }
            Self::ThreadWakeup(ThreadWakeupEvent { waking_thread_id, .. }) => *waking_thread_id,
        }
    }

    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawSchedulingRecord<'_>) -> Option<Self> {
        match raw {
            RawSchedulingRecord::ContextSwitch(c) => {
                Some(Self::ContextSwitch(ContextSwitchEvent::resolve(ctx, c)))
            }
            RawSchedulingRecord::ThreadWakeup(t) => {
                Some(Self::ThreadWakeup(ThreadWakeupEvent::resolve(ctx, t)))
            }
            RawSchedulingRecord::LegacyContextSwitch(c) => {
                Some(Self::LegacyContextSwitch(LegacyContextSwitchEvent::resolve(ctx, c)))
            }
            RawSchedulingRecord::Unknown { raw_type, .. } => {
                ctx.add_warning(ParseWarning::UnknownSchedulingRecordType(raw_type));
                None
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum RawSchedulingRecord<'a> {
    ContextSwitch(RawContextSwitchEvent<'a>),
    ThreadWakeup(RawThreadWakeupEvent<'a>),
    LegacyContextSwitch(RawLegacyContextSwitchEvent),
    Unknown { raw_type: u8, bytes: &'a [u8] },
}

trace_header! {
    SchedulingHeader (SCHEDULING_RECORD_TYPE) {
        u8, record_type: 60, 63;
    }
}

impl<'a> RawSchedulingRecord<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        use nom::combinator::map;
        let base_header = SchedulingHeader::parse(buf)?.1;
        match base_header.record_type() {
            LEGACY_CONTEXT_SWITCH_SCHEDULING_TYPE => {
                map(RawLegacyContextSwitchEvent::parse, Self::LegacyContextSwitch).parse(buf)
            }
            CONTEXT_SWITCH_SCHEDULING_TYPE => {
                map(RawContextSwitchEvent::parse, Self::ContextSwitch).parse(buf)
            }
            THREAD_WAKEUP_SCHEDULING_TYPE => {
                map(RawThreadWakeupEvent::parse, Self::ThreadWakeup).parse(buf)
            }
            unknown => {
                let size_bytes = base_header.size_words() as usize * 8;
                if size_bytes <= buf.len() {
                    let (unknown_record, rem) = buf.split_at(size_bytes);
                    Ok((rem, Self::Unknown { raw_type: unknown, bytes: unknown_record }))
                } else {
                    Err(nom::Err::Incomplete(nom::Needed::Size(
                        NonZero::new(size_bytes - buf.len()).unwrap(),
                    )))
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextSwitchEvent {
    pub provider: Option<Provider>,
    pub cpu_id: u16,
    pub timestamp: i64,
    pub outgoing_thread_state: ThreadState,
    pub outgoing_thread_id: ThreadKoid,
    pub incoming_thread_id: ThreadKoid,
    pub args: Vec<Arg>,
}

impl ContextSwitchEvent {
    fn resolve(ctx: &mut ResolveCtx, raw: RawContextSwitchEvent<'_>) -> Self {
        Self {
            provider: ctx.current_provider(),
            cpu_id: raw.cpu_id,
            timestamp: ctx.resolve_ticks(raw.ticks),
            outgoing_thread_state: raw.outgoing_thread_state,
            outgoing_thread_id: ThreadKoid(raw.outgoing_thread_id),
            incoming_thread_id: ThreadKoid(raw.incoming_thread_id),
            args: Arg::resolve_n(ctx, raw.args),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawContextSwitchEvent<'a> {
    cpu_id: u16,
    ticks: Ticks,
    outgoing_thread_state: ThreadState,
    outgoing_thread_id: u64,
    incoming_thread_id: u64,
    args: Vec<RawArg<'a>>,
}

impl<'a> RawContextSwitchEvent<'a> {
    fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = ContextSwitchHeader::parse(buf)?;
        if header.record_type() != CONTEXT_SWITCH_SCHEDULING_TYPE {
            return Err(nom::Err::Error(ParseError::WrongType {
                observed: header.record_type(),
                expected: CONTEXT_SWITCH_SCHEDULING_TYPE,
                context: "ContextSwitchEvent",
            }));
        }
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, outgoing_thread_id) = le_u64(payload)?;
        let (payload, incoming_thread_id) = le_u64(payload)?;
        let (empty, args) =
            all_consuming(|p| RawArg::parse_n(header.num_args(), p)).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((
            rem,
            Self {
                cpu_id: header.cpu_id(),
                outgoing_thread_state: ThreadState::parse(header.outgoing_thread_state()),
                ticks,
                outgoing_thread_id,
                incoming_thread_id,
                args,
            },
        ))
    }
}

trace_header! {
    ContextSwitchHeader (SCHEDULING_RECORD_TYPE) {
        u8, num_args: 16, 19;
        u16, cpu_id: 20, 35;
        u8, outgoing_thread_state: 36, 39;
        u8, record_type: 60, 63;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadWakeupEvent {
    pub provider: Option<Provider>,
    pub timestamp: i64,
    pub cpu_id: u16,
    pub waking_thread_id: ThreadKoid,
    pub args: Vec<Arg>,
}

impl ThreadWakeupEvent {
    fn resolve(ctx: &mut ResolveCtx, raw: RawThreadWakeupEvent<'_>) -> Self {
        Self {
            provider: ctx.current_provider(),
            timestamp: ctx.resolve_ticks(raw.ticks),
            cpu_id: raw.cpu_id,
            waking_thread_id: ThreadKoid(raw.waking_thread_id),
            args: Arg::resolve_n(ctx, raw.args),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawThreadWakeupEvent<'a> {
    ticks: Ticks,
    cpu_id: u16,
    waking_thread_id: u64,
    args: Vec<RawArg<'a>>,
}

impl<'a> RawThreadWakeupEvent<'a> {
    fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = ThreadWakeupHeader::parse(buf)?;
        if header.record_type() != THREAD_WAKEUP_SCHEDULING_TYPE {
            return Err(nom::Err::Error(ParseError::WrongType {
                observed: header.record_type(),
                expected: THREAD_WAKEUP_SCHEDULING_TYPE,
                context: "ThreadWakeupEvent",
            }));
        }
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, waking_thread_id) = le_u64(payload)?;
        let (empty, args) =
            all_consuming(|p| RawArg::parse_n(header.num_args(), p)).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { ticks, cpu_id: header.cpu_id(), waking_thread_id, args }))
    }
}

trace_header! {
    ThreadWakeupHeader (SCHEDULING_RECORD_TYPE) {
        u8, num_args: 16, 19;
        u16, cpu_id: 20, 35;
        u8, record_type: 60, 63;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegacyContextSwitchEvent {
    pub provider: Option<Provider>,
    pub timestamp: i64,
    pub cpu_id: u16,
    pub outgoing_thread_state: ThreadState,
    pub outgoing_process: ProcessKoid,
    pub outgoing_thread: ThreadKoid,
    pub outgoing_thread_priority: u8,
    pub incoming_process: ProcessKoid,
    pub incoming_thread: ThreadKoid,
    pub incoming_thread_priority: u8,
}

impl LegacyContextSwitchEvent {
    fn resolve(ctx: &mut ResolveCtx, raw: RawLegacyContextSwitchEvent) -> Self {
        Self {
            provider: ctx.current_provider(),
            timestamp: ctx.resolve_ticks(raw.ticks),
            cpu_id: raw.cpu_id,
            outgoing_thread_state: raw.outgoing_thread_state,
            outgoing_process: ctx.resolve_process(raw.outgoing_process),
            outgoing_thread: ctx.resolve_thread(raw.outgoing_thread),
            outgoing_thread_priority: raw.outgoing_thread_priority,
            incoming_process: ctx.resolve_process(raw.incoming_process),
            incoming_thread: ctx.resolve_thread(raw.incoming_thread),
            incoming_thread_priority: raw.incoming_thread_priority,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawLegacyContextSwitchEvent {
    ticks: Ticks,
    cpu_id: u16,
    outgoing_thread_state: ThreadState,
    outgoing_process: ProcessRef,
    outgoing_thread: ThreadRef,
    outgoing_thread_priority: u8,
    incoming_process: ProcessRef,
    incoming_thread: ThreadRef,
    incoming_thread_priority: u8,
}

impl RawLegacyContextSwitchEvent {
    fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = LegacyContextSwitchHeader::parse(buf)?;
        if header.record_type() != LEGACY_CONTEXT_SWITCH_SCHEDULING_TYPE {
            return Err(nom::Err::Error(ParseError::WrongType {
                observed: header.record_type(),
                expected: LEGACY_CONTEXT_SWITCH_SCHEDULING_TYPE,
                context: "LegacyContextSwitchEvent",
            }));
        }
        let outgoing_thread_state = ThreadState::parse(header.outgoing_thread_state());
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, outgoing_process) = ProcessRef::parse(header.outgoing_thread(), payload)?;
        let (payload, outgoing_thread) = ThreadRef::parse(header.outgoing_thread(), payload)?;
        let (payload, incoming_process) = ProcessRef::parse(header.incoming_thread(), payload)?;
        let (empty, incoming_thread) =
            all_consuming(|p| ThreadRef::parse(header.incoming_thread(), p)).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");

        Ok((
            rem,
            Self {
                ticks,
                cpu_id: header.cpu_id(),
                outgoing_thread_priority: header.outgoing_thread_priority(),
                incoming_thread_priority: header.incoming_thread_priority(),
                outgoing_thread_state,
                outgoing_process,
                outgoing_thread,
                incoming_process,
                incoming_thread,
            },
        ))
    }
}

trace_header! {
    LegacyContextSwitchHeader (SCHEDULING_RECORD_TYPE) {
        u16, cpu_id: 16, 23;
        u8, outgoing_thread_state: 24, 27;
        u8, outgoing_thread: 28, 35;
        u8, incoming_thread: 36, 43;
        u8, outgoing_thread_priority: 44, 51;
        u8, incoming_thread_priority: 52, 59;
        u8, record_type: 60, 63;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThreadState {
    New,
    Running,
    Suspended,
    Blocked,
    Dying,
    Dead,
    Unknown(u8),
}

impl ThreadState {
    fn parse(raw: u8) -> Self {
        match raw {
            0 => Self::New,
            1 => Self::Running,
            2 => Self::Suspended,
            3 => Self::Blocked,
            4 => Self::Dying,
            5 => Self::Dead,
            unknown => Self::Unknown(unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTraceRecord;
    use crate::args::{I32Header, RawArgValue};
    use crate::fxt_builder::FxtBuilder;
    use crate::string::StringRef;

    #[test]
    fn context_switch_event() {
        let mut header = ContextSwitchHeader::empty();
        header.set_record_type(CONTEXT_SWITCH_SCHEDULING_TYPE);
        header.set_num_args(2);
        header.set_cpu_id(6);
        header.set_outgoing_thread_state(4);

        let first_arg_name = "incoming_weight";
        let mut first_arg_header = I32Header::empty();
        first_arg_header
            .set_name_ref(fxt_layout::StringRefHeader::inline(first_arg_name.len() as u16).bits());
        first_arg_header.set_value(12);

        let second_arg_name = "outgoing_weight";
        let mut second_arg_header = I32Header::empty();
        second_arg_header
            .set_name_ref(fxt_layout::StringRefHeader::inline(second_arg_name.len() as u16).bits());
        second_arg_header.set_value(14);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(1024u64.to_le_bytes())
                .atom(5u64.to_le_bytes())
                .atom(8u64.to_le_bytes())
                .atom(FxtBuilder::new(first_arg_header).atom(first_arg_name).build())
                .atom(FxtBuilder::new(second_arg_header).atom(second_arg_name).build())
                .build(),
            RawTraceRecord::Scheduling(RawSchedulingRecord::ContextSwitch(RawContextSwitchEvent {
                cpu_id: 6,
                ticks: Ticks(1024),
                outgoing_thread_state: ThreadState::Dying,
                outgoing_thread_id: 5,
                incoming_thread_id: 8,
                args: vec![
                    RawArg {
                        name: StringRef::Inline(first_arg_name),
                        value: RawArgValue::Signed32(12),
                    },
                    RawArg {
                        name: StringRef::Inline(second_arg_name),
                        value: RawArgValue::Signed32(14),
                    },
                ],
            })),
        );
    }

    #[test]
    fn thread_wakeup_event() {
        let mut header = ThreadWakeupHeader::empty();
        header.set_record_type(THREAD_WAKEUP_SCHEDULING_TYPE);
        header.set_cpu_id(6);
        header.set_num_args(1);

        let arg_name = "weight";
        let mut arg_header = I32Header::empty();
        arg_header.set_name_ref(fxt_layout::StringRefHeader::inline(arg_name.len() as u16).bits());
        arg_header.set_value(12);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(1024u64.to_le_bytes())
                .atom(5u64.to_le_bytes())
                .atom(FxtBuilder::new(arg_header).atom(arg_name).build())
                .build(),
            RawTraceRecord::Scheduling(RawSchedulingRecord::ThreadWakeup(RawThreadWakeupEvent {
                cpu_id: 6,
                ticks: Ticks(1024),
                waking_thread_id: 5,
                args: vec![RawArg {
                    name: StringRef::Inline(arg_name),
                    value: RawArgValue::Signed32(12),
                }]
            })),
        );
    }

    #[test]
    fn legacy_context_switch_event() {
        let mut header = LegacyContextSwitchHeader::empty();
        header.set_record_type(LEGACY_CONTEXT_SWITCH_SCHEDULING_TYPE);
        header.set_cpu_id(6);
        header.set_outgoing_thread_state(2);
        header.set_outgoing_thread_priority(10);
        header.set_incoming_thread_priority(11);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(1024u64.to_le_bytes()) // ticks
                .atom(25u64.to_le_bytes()) // outgoing process
                .atom(26u64.to_le_bytes()) // outgoing thread
                .atom(100u64.to_le_bytes()) // incoming process
                .atom(101u64.to_le_bytes()) // incoming thread
                .build(),
            RawTraceRecord::Scheduling(RawSchedulingRecord::LegacyContextSwitch(
                RawLegacyContextSwitchEvent {
                    ticks: Ticks(1024),
                    cpu_id: 6,
                    outgoing_thread_state: ThreadState::Suspended,
                    outgoing_process: ProcessRef::Inline(ProcessKoid(25)),
                    outgoing_thread: ThreadRef::Inline(ThreadKoid(26)),
                    outgoing_thread_priority: 10,
                    incoming_process: ProcessRef::Inline(ProcessKoid(100)),
                    incoming_thread: ThreadRef::Inline(ThreadKoid(101)),
                    incoming_thread_priority: 11,
                }
            ))
        );
    }
}
