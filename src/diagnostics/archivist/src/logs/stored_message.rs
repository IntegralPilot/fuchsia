// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::identity::ComponentIdentity;
use crate::logs::stats::LogStreamStats;
use anyhow::Result;
use bstr::{BStr, ByteSlice};
use diagnostics_data::{LogsData, Severity};
use diagnostics_log_encoding::Argument;
use diagnostics_log_encoding::encode::{
    Encoder, EncoderOpts, EncodingError, MutableBuffer, RecordEvent, WriteEventParams,
};
use fidl_fuchsia_diagnostics_types as fdiagnostics;
use fidl_fuchsia_logger::MAX_DATAGRAM_LEN_BYTES;
use std::fmt::Debug;
use std::io::Cursor;
use std::sync::Arc;

type RawSeverity = u8;

#[derive(Debug, Eq, PartialEq)]
pub struct StoredMessage {
    bytes: Box<[u8]>,
    severity: Severity,
    timestamp: zx::BootInstant,
}

impl StoredMessage {
    pub fn new(buf: Box<[u8]>, stats: &Arc<LogStreamStats>) -> Option<Self> {
        match diagnostics_log_encoding::parse::basic_info(&buf) {
            Ok((timestamp, severity)) => {
                Some(StoredMessage { bytes: buf, severity: severity.into(), timestamp })
            }
            _ => {
                stats.increment_invalid(buf.len());
                None
            }
        }
    }

    pub fn from_debuglog(record: zx::DebugLogRecord, dropped: u64) -> Self {
        let mut data = record.data();
        if let Some(b'\n') = data.last() {
            data = &data[..data.len() - 1];
        }

        let severity = match record.severity {
            zx::DebugLogSeverity::Trace => fdiagnostics::Severity::Trace,
            zx::DebugLogSeverity::Debug => fdiagnostics::Severity::Debug,
            zx::DebugLogSeverity::Warn => fdiagnostics::Severity::Warn,
            zx::DebugLogSeverity::Error => fdiagnostics::Severity::Error,
            zx::DebugLogSeverity::Fatal => fdiagnostics::Severity::Fatal,
            zx::DebugLogSeverity::Unknown => fdiagnostics::Severity::Info,
            zx::DebugLogSeverity::Info => {
                // By default `zx_log_record_t` carries INFO severity. Since `zx_debuglog_write`
                // doesn't support setting a severity, historically logs have been tagged and
                // annotated with their severity in the message. If we get here attempt to use the
                // severity in the message, otherwise fallback to INFO.
                const MAX_STRING_SEARCH_SIZE: usize = 170;
                let last = data
                    .char_indices()
                    .nth(MAX_STRING_SEARCH_SIZE)
                    .map(|(i, _, _)| i)
                    .unwrap_or(data.len());
                let early_contents = &data[..last];
                if early_contents.contains_str("ERROR:") {
                    fdiagnostics::Severity::Error
                } else if early_contents.contains_str("WARNING:") {
                    fdiagnostics::Severity::Warn
                } else {
                    fdiagnostics::Severity::Info
                }
            }
        };

        let mut encoder =
            Encoder::new(Cursor::new([0u8; MAX_DATAGRAM_LEN_BYTES as _]), EncoderOpts::default());
        let _ = encoder.write_event(WriteEventParams {
            event: DebugLogRecordEvent {
                severity: severity.into_primitive(),
                data,
                timestamp: record.timestamp,
            },
            tags: &["klog"],
            metatags: std::iter::empty(),
            pid: record.pid,
            tid: record.tid,
            dropped,
        });
        let cursor = encoder.take();
        let position = cursor.position() as usize;
        let buf = cursor.get_ref();

        Self {
            bytes: Box::from(&buf[..position]),
            severity: severity.into(),
            timestamp: record.timestamp,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn timestamp(&self) -> zx::BootInstant {
        self.timestamp
    }

    pub fn parse(&self, source: &ComponentIdentity) -> Result<LogsData> {
        let mut data = diagnostics_message::from_structured(source.into(), &self.bytes)?;
        // TODO(https://fxbug.dev/368426475): fix chromium, then remove. The problematic logs are
        // being ingested as sturctured logs. Not a legacy logs too.
        match i8::from_le_bytes(data.metadata.raw_severity().to_le_bytes()) {
            -1 => data.set_severity(Severity::Debug),
            -2 => data.set_severity(Severity::Trace),
            0 => data.set_severity(Severity::Info),
            _ => {}
        }
        Ok(data)
    }
}

struct DebugLogRecordEvent<'a> {
    severity: RawSeverity,
    data: &'a BStr,
    timestamp: zx::BootInstant,
}

impl RecordEvent for DebugLogRecordEvent<'_> {
    fn raw_severity(&self) -> RawSeverity {
        self.severity
    }

    fn file(&self) -> Option<&str> {
        None
    }

    fn line(&self) -> Option<u32> {
        None
    }

    fn target(&self) -> &str {
        ""
    }

    fn timestamp(&self) -> zx::BootInstant {
        self.timestamp
    }

    fn write_arguments<B: MutableBuffer>(
        self,
        writer: &mut Encoder<B>,
    ) -> Result<(), EncodingError> {
        writer.write_argument(Argument::message(self.data.to_str_lossy().as_ref()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::debuglog::KERNEL_IDENTITY;
    use crate::logs::testing::TestDebugEntry;
    use diagnostics_data::{BuilderArgs, LogsDataBuilder};
    use fidl_fuchsia_logger::LogMessage;

    #[fuchsia::test]
    fn convert_debuglog_to_log_message_test() {
        let klog = TestDebugEntry::new("test log".as_bytes());
        let data = StoredMessage::from_debuglog(klog.record, 10).parse(&KERNEL_IDENTITY).unwrap();
        assert_eq!(
            data,
            LogsDataBuilder::new(BuilderArgs {
                timestamp: klog.record.timestamp,
                component_url: Some(KERNEL_IDENTITY.url.clone()),
                moniker: KERNEL_IDENTITY.moniker.clone(),
                severity: Severity::Info,
            })
            .set_dropped(10)
            .set_pid(klog.record.pid.raw_koid())
            .set_tid(klog.record.tid.raw_koid())
            .add_tag("klog")
            .set_message("test log".to_string())
            .build()
        );
        // make sure the `klog` tag still shows up for legacy listeners
        let log_message: LogMessage = data.into();
        assert_eq!(
            log_message,
            LogMessage {
                pid: klog.record.pid.raw_koid(),
                tid: klog.record.tid.raw_koid(),
                time: klog.record.timestamp,
                severity: fdiagnostics::Severity::Info.into_primitive() as i32,
                dropped_logs: 10,
                tags: vec!["klog".to_string()],
                msg: "test log".to_string(),
            }
        );

        // maximum allowed klog size
        let klog = TestDebugEntry::new(&vec![b'a'; zx::sys::ZX_LOG_RECORD_DATA_MAX]);
        let data = StoredMessage::from_debuglog(klog.record, 0).parse(&KERNEL_IDENTITY).unwrap();
        assert_eq!(
            data,
            LogsDataBuilder::new(BuilderArgs {
                timestamp: klog.record.timestamp,
                component_url: Some(KERNEL_IDENTITY.url.clone()),
                moniker: KERNEL_IDENTITY.moniker.clone(),
                severity: Severity::Info,
            })
            .set_pid(klog.record.pid.raw_koid())
            .set_tid(klog.record.tid.raw_koid())
            .add_tag("klog")
            .set_message(String::from_utf8(vec![b'a'; zx::sys::ZX_LOG_RECORD_DATA_MAX]).unwrap())
            .build()
        );

        // empty message
        let klog = TestDebugEntry::new(&[]);
        let data = StoredMessage::from_debuglog(klog.record, 0).parse(&KERNEL_IDENTITY).unwrap();
        assert_eq!(
            data,
            LogsDataBuilder::new(BuilderArgs {
                timestamp: klog.record.timestamp,
                component_url: Some(KERNEL_IDENTITY.url.clone()),
                moniker: KERNEL_IDENTITY.moniker.clone(),
                severity: Severity::Info,
            })
            .set_pid(klog.record.pid.raw_koid())
            .set_tid(klog.record.tid.raw_koid())
            .add_tag("klog")
            .set_message("".to_string())
            .build()
        );

        // invalid utf-8
        let klog = TestDebugEntry::new(b"\x00\x9f\x92");
        let data = StoredMessage::from_debuglog(klog.record, 0).parse(&KERNEL_IDENTITY).unwrap();
        assert_eq!(data.msg().unwrap(), "\x00��");
    }
}
