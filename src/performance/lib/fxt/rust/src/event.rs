// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{Arg, RawArg, RawArgValue};
use crate::fxt_builder::{FxtBuilder, SerializeError};
use crate::init::Ticks;
use crate::session::ResolveCtx;
use crate::string::StringRef;
use crate::thread::{ProcessKoid, ProcessRef, ThreadKoid, ThreadRef};
use crate::{EVENT_RECORD_TYPE, ParseResult, Provider, trace_header};
use flyweights::FlyStr;
use nom::Parser;
use nom::number::complete::le_u64;

pub(crate) const INSTANT_EVENT_TYPE: u8 = 0;
pub(crate) const COUNTER_EVENT_TYPE: u8 = 1;
pub(crate) const DURATION_BEGIN_EVENT_TYPE: u8 = 2;
pub(crate) const DURATION_END_EVENT_TYPE: u8 = 3;
pub(crate) const DURATION_COMPLETE_EVENT_TYPE: u8 = 4;
pub(crate) const ASYNC_BEGIN_EVENT_TYPE: u8 = 5;
pub(crate) const ASYNC_INSTANT_EVENT_TYPE: u8 = 6;
pub(crate) const ASYNC_END_EVENT_TYPE: u8 = 7;
pub(crate) const FLOW_BEGIN_EVENT_TYPE: u8 = 8;
pub(crate) const FLOW_STEP_EVENT_TYPE: u8 = 9;
pub(crate) const FLOW_END_EVENT_TYPE: u8 = 10;

pub fn symbolize<'a>(
    ordinal: u64,
    method: &'a str,
    raw_record: &RawEventRecord<'a>,
) -> RawEventRecord<'a> {
    let mut new_args = vec![];
    for arg in &raw_record.args {
        if let &RawArgValue::Unsigned64(arg_value) = &arg.value {
            if arg_value == ordinal {
                let symbolized_arg = RawArg {
                    name: StringRef::Inline("method"),
                    value: RawArgValue::String(StringRef::Inline(method)),
                };
                new_args.push(symbolized_arg);
                continue;
            }
        }
        new_args.push(arg.clone());
    }

    RawEventRecord {
        event_type: raw_record.event_type,
        ticks: raw_record.ticks.clone(),
        process: raw_record.process.clone(),
        thread: raw_record.thread.clone(),
        category: raw_record.category.clone(),
        name: raw_record.name.clone(),
        args: new_args,
        payload: raw_record.payload.clone(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventRecord {
    pub provider: Option<Provider>,
    pub timestamp: i64,
    pub process: ProcessKoid,
    pub thread: ThreadKoid,
    pub category: FlyStr,
    pub name: FlyStr,
    pub args: Vec<Arg>,
    pub payload: EventPayload<i64>,
}

impl EventRecord {
    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawEventRecord<'_>) -> Self {
        Self {
            provider: ctx.current_provider(),
            timestamp: ctx.resolve_ticks(raw.ticks),
            process: ctx.resolve_process(raw.process),
            thread: ctx.resolve_thread(raw.thread),
            category: ctx.resolve_str(raw.category),
            name: ctx.resolve_str(raw.name),
            args: Arg::resolve_n(ctx, raw.args),
            payload: raw.payload.resolve(ctx),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct RawEventRecord<'a> {
    pub(crate) event_type: u8,
    pub(crate) ticks: Ticks,
    pub(crate) process: ProcessRef,
    pub(crate) thread: ThreadRef,
    pub(crate) category: StringRef<'a>,
    pub name: StringRef<'a>,
    pub args: Vec<RawArg<'a>>,
    pub(crate) payload: EventPayload<Ticks>,
}

impl<'a> RawEventRecord<'a> {
    pub fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = EventHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let event_type = header.event_type();
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, process) = ProcessRef::parse(header.thread_ref(), payload)?;
        let (payload, thread) = ThreadRef::parse(header.thread_ref(), payload)?;
        let (payload, category) = StringRef::parse(header.category_ref(), payload)?;
        let (payload, name) = StringRef::parse(header.name_ref(), payload)?;
        let (payload, args) = RawArg::parse_n(header.num_args(), payload)?;

        // Some trace events attach an undocumented "scope" word on instant events for chrome trace
        // viewer compatibility that we don't need to return, so don't use all_consuming here.
        let (_empty, payload) = EventPayload::parse(event_type, payload)?;
        Ok((rem, Self { event_type, ticks, process, thread, category, name, args, payload }))
    }

    pub fn make_header(&self) -> EventHeader {
        let mut header = EventHeader::empty();
        header.set_event_type(self.event_type);
        header.set_num_args(self.args.len() as u8);

        if let ProcessRef::Index(id) = self.process {
            header.set_thread_ref(id.into());
        }
        let category_ref: u16 = match self.category {
            StringRef::Index(id) => fxt_layout::StringRefHeader::indexed(id.into()).bits(),
            StringRef::Inline(category_stream) => {
                fxt_layout::StringRefHeader::inline(category_stream.len() as u16).bits()
            }
            StringRef::Empty => 0u16,
        };
        header.set_category_ref(category_ref);

        let name_ref: u16 = match self.name {
            StringRef::Index(id) => fxt_layout::StringRefHeader::indexed(id.into()).bits(),
            StringRef::Inline(name_stream) => {
                fxt_layout::StringRefHeader::inline(name_stream.len() as u16).bits()
            }
            StringRef::Empty => 0u16,
        };
        header.set_name_ref(name_ref);
        header
    }

    pub fn serialize(&self) -> Result<Vec<u8>, SerializeError> {
        let mut event_record = FxtBuilder::new(self.make_header());

        event_record = event_record.atom(self.ticks.0.to_le_bytes());

        if let ProcessRef::Inline(process_koid) = self.process {
            event_record = event_record.atom(process_koid.0.to_le_bytes());
        }

        if let ThreadRef::Inline(thread_koid) = self.thread {
            event_record = event_record.atom(thread_koid.0.to_le_bytes());
        }

        if let StringRef::Inline(category_stream) = self.category {
            event_record = event_record.atom(category_stream);
        }

        if let StringRef::Inline(name_stream) = self.name {
            event_record = event_record.atom(name_stream);
        }

        for arg in &self.args {
            event_record = event_record.atom(arg.serialize()?);
        }

        match &self.payload {
            EventPayload::Instant | EventPayload::DurationBegin | EventPayload::DurationEnd => {}

            EventPayload::Counter { id }
            | EventPayload::AsyncBegin { id }
            | EventPayload::AsyncInstant { id }
            | EventPayload::AsyncEnd { id }
            | EventPayload::FlowBegin { id }
            | EventPayload::FlowStep { id }
            | EventPayload::FlowEnd { id } => {
                event_record = event_record.atom(id.to_le_bytes());
            }

            EventPayload::DurationComplete { end_timestamp } => {
                event_record = event_record.atom(end_timestamp.0.to_le_bytes());
            }

            EventPayload::Unknown { raw_type: _, bytes } => {
                event_record = event_record.atom(bytes);
            }
        }
        Ok(event_record.build())
    }

    pub fn set_flow_step_payload(&mut self, id: u64) {
        self.event_type = FLOW_STEP_EVENT_TYPE;
        self.payload = EventPayload::FlowStep { id };
    }
}

trace_header! {
    EventHeader (EVENT_RECORD_TYPE) {
        u8, event_type: 16, 19;
        u8, num_args: 20, 23;
        u8, thread_ref: 24, 31;
        u16, category_ref: 32, 47;
        u16, name_ref: 48, 63;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventPayload<Time> {
    Instant,
    Counter { id: u64 },
    DurationBegin,
    DurationEnd,
    DurationComplete { end_timestamp: Time },
    AsyncBegin { id: u64 },
    AsyncInstant { id: u64 },
    AsyncEnd { id: u64 },
    FlowBegin { id: u64 },
    FlowStep { id: u64 },
    FlowEnd { id: u64 },
    Unknown { raw_type: u8, bytes: Vec<u8> },
}

impl EventPayload<Ticks> {
    pub(crate) fn resolve(self, ctx: &ResolveCtx) -> EventPayload<i64> {
        match self {
            EventPayload::Instant => EventPayload::Instant,
            EventPayload::Counter { id } => EventPayload::Counter { id },
            EventPayload::DurationBegin => EventPayload::DurationBegin,
            EventPayload::DurationEnd => EventPayload::DurationEnd,
            EventPayload::DurationComplete { end_timestamp } => {
                EventPayload::DurationComplete { end_timestamp: ctx.resolve_ticks(end_timestamp) }
            }
            EventPayload::AsyncBegin { id } => EventPayload::AsyncBegin { id },
            EventPayload::AsyncInstant { id } => EventPayload::AsyncInstant { id },
            EventPayload::AsyncEnd { id } => EventPayload::AsyncEnd { id },
            EventPayload::FlowBegin { id } => EventPayload::FlowBegin { id },
            EventPayload::FlowStep { id } => EventPayload::FlowStep { id },
            EventPayload::FlowEnd { id } => EventPayload::FlowEnd { id },
            EventPayload::Unknown { raw_type, bytes } => EventPayload::Unknown { raw_type, bytes },
        }
    }
}

impl EventPayload<Ticks> {
    fn parse(event_type: u8, buf: &[u8]) -> ParseResult<'_, Self> {
        use nom::combinator::map;
        match event_type {
            INSTANT_EVENT_TYPE => Ok((buf, EventPayload::Instant)),
            COUNTER_EVENT_TYPE => map(le_u64, |id| EventPayload::Counter { id }).parse(buf),
            DURATION_BEGIN_EVENT_TYPE => Ok((buf, EventPayload::DurationBegin)),
            DURATION_END_EVENT_TYPE => Ok((buf, EventPayload::DurationEnd)),
            DURATION_COMPLETE_EVENT_TYPE => {
                map(Ticks::parse, |end_timestamp| EventPayload::DurationComplete { end_timestamp })
                    .parse(buf)
            }
            ASYNC_BEGIN_EVENT_TYPE => map(le_u64, |id| EventPayload::AsyncBegin { id }).parse(buf),
            ASYNC_INSTANT_EVENT_TYPE => {
                map(le_u64, |id| EventPayload::AsyncInstant { id }).parse(buf)
            }
            ASYNC_END_EVENT_TYPE => map(le_u64, |id| EventPayload::AsyncEnd { id }).parse(buf),
            FLOW_BEGIN_EVENT_TYPE => map(le_u64, |id| EventPayload::FlowBegin { id }).parse(buf),
            FLOW_STEP_EVENT_TYPE => map(le_u64, |id| EventPayload::FlowStep { id }).parse(buf),
            FLOW_END_EVENT_TYPE => map(le_u64, |id| EventPayload::FlowEnd { id }).parse(buf),
            unknown => {
                Ok((&[][..], EventPayload::Unknown { raw_type: unknown, bytes: buf.to_vec() }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTraceRecord;
    use std::num::{NonZeroU8, NonZeroU16};

    #[test]
    fn event_no_args() {
        let mut header = EventHeader::empty();
        header.set_thread_ref(11);
        header.set_category_ref(27);
        header.set_name_ref(93);
        header.set_num_args(0);
        header.set_event_type(INSTANT_EVENT_TYPE);

        let event_record_bytes = FxtBuilder::new(header).atom(2048u64.to_le_bytes()).build();
        let raw_event_record = RawEventRecord {
            event_type: INSTANT_EVENT_TYPE,
            ticks: Ticks(2048),
            process: ProcessRef::Index(NonZeroU8::new(11).unwrap()),
            thread: ThreadRef::Index(NonZeroU8::new(11).unwrap()),
            category: StringRef::Index(NonZeroU16::new(27).unwrap()),
            name: StringRef::Index(NonZeroU16::new(93).unwrap()),
            args: vec![],
            payload: EventPayload::Instant,
        };

        assert_eq!(raw_event_record.serialize().unwrap(), event_record_bytes);
        assert_parses_to_record!(event_record_bytes, RawTraceRecord::Event(raw_event_record));
    }

    #[test]
    fn event_with_args() {
        let mut header = EventHeader::empty();
        header.set_event_type(DURATION_COMPLETE_EVENT_TYPE);
        header.set_category_ref(
            fxt_layout::StringRefHeader::inline("event_category".len() as u16).bits(),
        );
        header.set_name_ref(fxt_layout::StringRefHeader::inline("event_name".len() as u16).bits());
        header.set_num_args(2);

        let first_arg_name = "arg1";
        let first_arg_value = "val1";
        let mut first_arg_header = crate::args::StringHeader::empty();
        first_arg_header
            .set_name_ref(fxt_layout::StringRefHeader::inline(first_arg_name.len() as u16).bits());
        first_arg_header.set_value_ref(
            fxt_layout::StringRefHeader::inline(first_arg_value.len() as u16).bits(),
        );

        let second_arg_name = "arg2";
        let mut second_arg_header = crate::args::BaseArgHeader::empty();
        second_arg_header.set_raw_type(crate::args::PTR_ARG_TYPE);
        second_arg_header
            .set_name_ref(fxt_layout::StringRefHeader::inline(second_arg_name.len() as u16).bits());

        let event_record_bytes = FxtBuilder::new(header)
            // begin ticks
            .atom(2048u64.to_le_bytes())
            // process
            .atom(345u64.to_le_bytes())
            // thread
            .atom(678u64.to_le_bytes())
            // category
            .atom("event_category")
            // name
            .atom("event_name")
            // first arg
            .atom(
                FxtBuilder::new(first_arg_header)
                    .atom(first_arg_name)
                    .atom(first_arg_value)
                    .build(),
            )
            // second arg
            .atom(
                FxtBuilder::new(second_arg_header)
                    .atom(second_arg_name)
                    .atom(123456u64.to_le_bytes())
                    .build(),
            )
            // end ticks
            .atom(4096u64.to_le_bytes())
            .build();

        let raw_event_record = RawEventRecord {
            event_type: DURATION_COMPLETE_EVENT_TYPE,
            ticks: Ticks(2048),
            process: ProcessRef::Inline(ProcessKoid(345)),
            thread: ThreadRef::Inline(ThreadKoid(678)),
            category: StringRef::Inline("event_category"),
            name: StringRef::Inline("event_name"),
            args: vec![
                RawArg {
                    name: StringRef::Inline(first_arg_name),
                    value: crate::args::RawArgValue::String(StringRef::Inline(first_arg_value)),
                },
                RawArg {
                    name: StringRef::Inline(second_arg_name),
                    value: crate::args::RawArgValue::Pointer(123456),
                },
            ],
            payload: EventPayload::DurationComplete { end_timestamp: Ticks(4096) },
        };

        assert_eq!(raw_event_record.serialize().unwrap(), event_record_bytes);
        assert_parses_to_record!(event_record_bytes, RawTraceRecord::Event(raw_event_record));
    }

    #[test]
    fn symbolize_event() {
        let ordinal: u64 = 123456;
        let method_name = "fidl_method";
        let raw_event_record = RawEventRecord {
            event_type: INSTANT_EVENT_TYPE,
            ticks: Ticks(2048),
            process: ProcessRef::Inline(ProcessKoid(345)),
            thread: ThreadRef::Inline(ThreadKoid(678)),
            category: StringRef::Inline("event_category"),
            name: StringRef::Inline("event_name"),
            args: vec![
                RawArg {
                    name: StringRef::Inline("arg1"),
                    value: crate::args::RawArgValue::Pointer(123456),
                },
                RawArg {
                    name: StringRef::Index(NonZeroU16::new(8).unwrap()),
                    value: crate::args::RawArgValue::Unsigned64(ordinal),
                },
            ],
            payload: EventPayload::DurationComplete { end_timestamp: Ticks(4096) },
        };

        assert_eq!(
            symbolize(ordinal, method_name, &raw_event_record),
            RawEventRecord {
                event_type: INSTANT_EVENT_TYPE,
                ticks: Ticks(2048),
                process: ProcessRef::Inline(ProcessKoid(345)),
                thread: ThreadRef::Inline(ThreadKoid(678)),
                category: StringRef::Inline("event_category"),
                name: StringRef::Inline("event_name"),
                args: vec![
                    RawArg {
                        name: StringRef::Inline("arg1"),
                        value: crate::args::RawArgValue::Pointer(123456),
                    },
                    RawArg {
                        name: StringRef::Inline("method"),
                        value: crate::args::RawArgValue::String(StringRef::Inline(method_name)),
                    }
                ],
                payload: EventPayload::DurationComplete { end_timestamp: Ticks(4096) },
            }
        );
    }
}
