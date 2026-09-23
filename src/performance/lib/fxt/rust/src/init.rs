// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{trace_header, ParseResult, INIT_RECORD_TYPE};
use nom::combinator::all_consuming;
use nom::number::complete::le_u64;
use nom::Parser;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ticks(pub(crate) u64);

impl Ticks {
    pub(crate) fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        nom::combinator::map(nom::number::complete::le_u64, Ticks).parse(buf)
    }

    pub(crate) fn scale(self, ticks_per_second: u64) -> i64 {
        const NANOS_PER_SECOND: u128 = Duration::from_secs(1).as_nanos() as _;
        ((self.0 as u128 * NANOS_PER_SECOND) / ticks_per_second as u128)
            .try_into()
            .expect("overflowing a signed monotonic timestamp would take ~292 years of uptime")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct InitRecord {
    pub ticks_per_second: u64,
}

trace_header! {
    InitHeader (INIT_RECORD_TYPE) {}
}

impl InitRecord {
    pub(super) fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = InitHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (empty, ticks_per_second) = all_consuming(le_u64).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { ticks_per_second }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fxt_builder::FxtBuilder;
    use crate::RawTraceRecord;

    #[test]
    fn basic_ticks_to_monotonic() {
        assert_eq!(
            Ticks(1024).scale(2_000_000_000),
            512,
            "2 billion ticks/sec is twice the rate of the monotonic clock, half as many nanos",
        );

        assert_eq!(
            Ticks(1024).scale(500_000_000),
            2048,
            "500mm ticks/sec is half the rate of the monotonic clock, twice as many nanos",
        );
    }

    #[test]
    fn init_record() {
        assert_parses_to_record!(
            FxtBuilder::new(InitHeader::empty()).atom(2u64.to_le_bytes()).build(),
            RawTraceRecord::Init(InitRecord { ticks_per_second: 2 }),
        );
    }
}
