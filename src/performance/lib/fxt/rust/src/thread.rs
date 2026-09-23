// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{trace_header, ParseError, ParseResult, THREAD_RECORD_TYPE};
use nom::combinator::all_consuming;
use nom::number::complete::le_u64;
use nom::Parser;
use std::num::NonZeroU8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct ProcessKoid(pub u64);

impl From<u64> for ProcessKoid {
    fn from(n: u64) -> Self {
        Self(n)
    }
}

impl PartialEq<u64> for ProcessKoid {
    fn eq(&self, other: &u64) -> bool {
        self.0.eq(other)
    }
}

impl std::fmt::Display for ProcessKoid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProcessRef {
    Index(NonZeroU8),
    Inline(ProcessKoid),
}

impl ProcessRef {
    pub(crate) fn parse<'a>(thread_ref: u8, buf: &'a [u8]) -> ParseResult<'a, Self> {
        Ok(if let Some(index) = NonZeroU8::new(thread_ref) {
            (buf, Self::Index(index))
        } else {
            let (buf, koid) = le_u64(buf)?;
            (buf, Self::Inline(ProcessKoid(koid)))
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct ThreadKoid(pub u64);

impl From<u64> for ThreadKoid {
    fn from(n: u64) -> Self {
        Self(n)
    }
}

impl PartialEq<u64> for ThreadKoid {
    fn eq(&self, other: &u64) -> bool {
        self.0.eq(other)
    }
}

impl std::fmt::Display for ThreadKoid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ThreadRef {
    Index(NonZeroU8),
    Inline(ThreadKoid),
}

impl ThreadRef {
    pub(crate) fn parse<'a>(thread_ref: u8, buf: &'a [u8]) -> ParseResult<'a, Self> {
        Ok(if let Some(index) = NonZeroU8::new(thread_ref) {
            (buf, Self::Index(index))
        } else {
            let (buf, koid) = le_u64(buf)?;
            (buf, Self::Inline(ThreadKoid(koid)))
        })
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct ThreadRecord {
    pub index: NonZeroU8,
    pub process_koid: ProcessKoid,
    pub thread_koid: ThreadKoid,
}

impl ThreadRecord {
    pub(super) fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = ThreadHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, process_koid) = nom::combinator::map(le_u64, ProcessKoid).parse(payload)?;
        let (empty, thread_koid) =
            all_consuming(nom::combinator::map(le_u64, ThreadKoid)).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        let index =
            NonZeroU8::new(header.thread_index()).ok_or(nom::Err::Error(ParseError::InvalidRef))?;
        Ok((rem, Self { index, process_koid, thread_koid }))
    }
}

trace_header! {
    ThreadHeader (THREAD_RECORD_TYPE) {
        u8, thread_index: 16, 23;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTraceRecord;

    #[test]
    fn process_ref_index() {
        let (trailing, parsed) = ProcessRef::parse(11, &[1, 1, 1, 1]).unwrap();
        assert_eq!(parsed, ProcessRef::Index(NonZeroU8::new(11).unwrap()));
        assert_eq!(trailing, [1, 1, 1, 1],);
    }

    #[test]
    fn process_ref_inline() {
        let mut buf = 52u64.to_le_bytes().to_vec(); // value
        buf.extend([1, 1, 1, 1]); // trailing

        let (trailing, parsed) = ProcessRef::parse(0, &buf).unwrap();
        assert_eq!(parsed, ProcessRef::Inline(ProcessKoid(52)));
        assert_eq!(trailing, [1, 1, 1, 1]);
    }

    #[test]
    fn thread_ref_index() {
        let (trailing, parsed) = ThreadRef::parse(14, &[1, 1, 1, 1]).unwrap();
        assert_eq!(parsed, ThreadRef::Index(NonZeroU8::new(14).unwrap()));
        assert_eq!(trailing, [1, 1, 1, 1]);
    }

    #[test]
    fn thread_ref_inline() {
        let mut buf = 54u64.to_le_bytes().to_vec(); // value
        buf.extend([1, 1, 1, 1]); // trailing

        let (trailing, parsed) = ThreadRef::parse(0, &buf).unwrap();
        assert_eq!(parsed, ThreadRef::Inline(ThreadKoid(54)));
        assert_eq!(trailing, [1, 1, 1, 1]);
    }

    #[test]
    fn thread_record() {
        let mut header = ThreadHeader::empty();
        header.set_thread_index(10);
        header.set_size_words(3); // header, process koid, thread koid

        let mut buf = header.0.to_le_bytes().to_vec(); // header
        buf.extend(52u64.to_le_bytes()); // process
        buf.extend(54u64.to_le_bytes()); // thread
        buf.extend([1, 1, 1, 1]); // trailing

        let (trailing, parsed) = RawTraceRecord::parse(&buf).unwrap();
        assert_eq!(
            parsed.parsed,
            RawTraceRecord::Thread(ThreadRecord {
                index: NonZeroU8::new(10).unwrap(),
                process_koid: ProcessKoid(52),
                thread_koid: ThreadKoid(54)
            })
        );
        assert_eq!(trailing, [1, 1, 1, 1]);
    }
}
