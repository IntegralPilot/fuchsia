// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::init::Ticks;
use crate::session::ResolveCtx;
use crate::string::parse_padded_string;
use crate::thread::{ProcessKoid, ProcessRef, ThreadKoid, ThreadRef};
use crate::{trace_header, ParseResult, Provider, LOG_RECORD_TYPE};
use flyweights::FlyStr;
use nom::combinator::all_consuming;
use nom::Parser;

#[derive(Clone, Debug, PartialEq)]
pub struct LogRecord {
    pub provider: Option<Provider>,
    pub timestamp: i64,
    pub process: ProcessKoid,
    pub thread: ThreadKoid,
    pub message: FlyStr,
}

impl LogRecord {
    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawLogRecord<'_>) -> Self {
        Self {
            provider: ctx.current_provider(),
            timestamp: ctx.resolve_ticks(raw.ticks),
            process: ctx.resolve_process(raw.process),
            thread: ctx.resolve_thread(raw.thread),
            message: raw.message.into(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawLogRecord<'a> {
    ticks: Ticks,
    process: ProcessRef,
    thread: ThreadRef,
    message: &'a str,
}

impl<'a> RawLogRecord<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = LogHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, process) = ProcessRef::parse(header.thread_ref(), payload)?;
        let (payload, thread) = ThreadRef::parse(header.thread_ref(), payload)?;
        let (empty, message) =
            all_consuming(|p| parse_padded_string(header.message_len() as usize, p))
                .parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { ticks, process, thread, message }))
    }
}

trace_header! {
    LogHeader (LOG_RECORD_TYPE) {
        u16, message_len: 16, 30;
        u8, thread_ref: 32, 39;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fxt_builder::FxtBuilder;
    use crate::RawTraceRecord;
    use std::num::NonZeroU8;

    #[test]
    fn log_record_index_thread() {
        let message = "hello, world!";
        let mut header = LogHeader::empty();
        header.set_message_len(message.len() as u16);
        header.set_thread_ref(16);

        assert_parses_to_record!(
            FxtBuilder::new(header).atom(1024u64.to_le_bytes()).atom(message).build(),
            RawTraceRecord::Log(RawLogRecord {
                ticks: Ticks(1024),
                process: ProcessRef::Index(NonZeroU8::new(16).unwrap()),
                thread: ThreadRef::Index(NonZeroU8::new(16).unwrap()),
                message,
            }),
        );
    }

    #[test]
    fn log_record_inline_thread() {
        let message = "hello, world!";
        let mut header = LogHeader::empty();
        header.set_message_len(message.len() as u16);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(1024u64.to_le_bytes())
                .atom(24u64.to_le_bytes())
                .atom(26u64.to_le_bytes())
                .atom(message)
                .build(),
            RawTraceRecord::Log(RawLogRecord {
                ticks: Ticks(1024),
                process: ProcessRef::Inline(ProcessKoid(24)),
                thread: ThreadRef::Inline(ThreadKoid(26)),
                message,
            }),
        );
    }
}
