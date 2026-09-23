// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module contains fuzzing targets for Archivist.

use arbitrary::{Arbitrary, Result, Unstructured};
use archivist_lib::identity::ComponentIdentity;
use archivist_lib::logs::stored_message::StoredMessage;
use diagnostics_data::LogsData;

use fuzz::fuzz;

#[derive(Clone, Debug)]
struct RandomLogRecord(zx::DebugLogRecord);

/// Fuzzer for kernel debuglog parser.
#[fuzz]
fn convert_debuglog_to_log_message_fuzzer(record: RandomLogRecord) -> Option<LogsData> {
    let msg = StoredMessage::from_debuglog(record.0, 0);
    msg.parse(&ComponentIdentity::unknown()).ok()
}

impl<'a> Arbitrary<'a> for RandomLogRecord {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let mut raw = zx::sys::zx_log_record_t::default();
        raw.header.sequence = u64::arbitrary(u)?;
        raw.header.datalen =
            std::cmp::min(u16::arbitrary(u)?, zx::sys::ZX_LOG_RECORD_DATA_MAX as u16);
        raw.header.severity = u8::arbitrary(u)?;
        raw.header.flags = u8::arbitrary(u)?;
        raw.header.timestamp = i64::arbitrary(u)? as zx::sys::zx_instant_boot_t;
        raw.header.pid = u64::arbitrary(u)?;
        raw.header.tid = u64::arbitrary(u)?;

        // Fill the first datalen bytes of data.
        let mut partial = &mut raw.data[0..raw.header.datalen as usize];
        u.fill_buffer(&mut partial)?;

        Ok(RandomLogRecord(zx::DebugLogRecord::from_raw(&raw).unwrap()))
    }

    fn size_hint(_: usize) -> (usize, Option<usize>) {
        (
            std::mem::size_of::<zx::sys::zx_log_record_t>(),
            Some(std::mem::size_of::<zx::sys::zx_log_record_t>()),
        )
    }
}
