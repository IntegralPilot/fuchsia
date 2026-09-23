// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Read debug logs, convert them to LogMessages and serve them.

use crate::identity::ComponentIdentity;
use crate::logs::error::LogsError;
use crate::logs::stored_message::StoredMessage;
use fidl::prelude::*;
use fidl_fuchsia_boot::ReadOnlyLogMarker;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::stream::{Stream, unfold};
use moniker::ExtendedMoniker;
use std::future::Future;
use std::sync::{Arc, LazyLock};

const KERNEL_URL: &str = "fuchsia-boot://kernel";
pub static KERNEL_IDENTITY: LazyLock<Arc<ComponentIdentity>> = LazyLock::new(|| {
    Arc::new(ComponentIdentity::new(ExtendedMoniker::parse_str("./klog").unwrap(), KERNEL_URL))
});

pub trait DebugLog {
    /// Reads a single entry off the debug log into `buffer`.  Any existing
    /// contents in `buffer` are overwritten.
    fn read(&self) -> Result<zx::DebugLogRecord, zx::Status>;

    /// Returns a future that completes when there is another log to read.
    fn ready_signal(&self) -> impl Future<Output = Result<(), zx::Status>> + Send;
}

pub struct KernelDebugLog {
    debuglogger: zx::DebugLog,
}

impl DebugLog for KernelDebugLog {
    fn read(&self) -> Result<zx::DebugLogRecord, zx::Status> {
        self.debuglogger.read()
    }

    async fn ready_signal(&self) -> Result<(), zx::Status> {
        fasync::OnSignals::new(&self.debuglogger, zx::Signals::LOG_READABLE).await?;
        Ok(())
    }
}

impl KernelDebugLog {
    /// Connects to `fuchsia.boot.ReadOnlyLog` to retrieve a handle.
    pub async fn new() -> Result<Self, LogsError> {
        let boot_log = connect_to_protocol::<ReadOnlyLogMarker>().map_err(|source| {
            LogsError::ConnectingToService { protocol: ReadOnlyLogMarker::PROTOCOL_NAME, source }
        })?;
        let debuglogger =
            boot_log.get().await.map_err(|source| LogsError::RetrievingDebugLog { source })?;
        Ok(KernelDebugLog { debuglogger })
    }
}

pub struct DebugLogBridge<K: DebugLog> {
    debug_log: K,
    next_sequence_id: u64,
}

impl<K: DebugLog> DebugLogBridge<K> {
    pub fn create(debug_log: K) -> Self {
        DebugLogBridge { debug_log, next_sequence_id: 0 }
    }

    fn read_log(&mut self) -> Result<StoredMessage, zx::Status> {
        let record = self.debug_log.read()?;
        let dropped = record.sequence - self.next_sequence_id;
        self.next_sequence_id = record.sequence + 1;
        Ok(StoredMessage::from_debuglog(record, dropped))
    }

    pub fn existing_logs(&mut self) -> Result<Vec<StoredMessage>, zx::Status> {
        let mut result = vec![];
        loop {
            match self.read_log() {
                Err(zx::Status::SHOULD_WAIT) => break,
                Err(err) => return Err(err),
                Ok(log) => result.push(log),
            }
        }
        Ok(result)
    }

    pub fn listen(self) -> impl Stream<Item = Result<StoredMessage, zx::Status>> {
        unfold((true, self), move |(mut is_readable, mut klogger)| async move {
            loop {
                if !is_readable && let Err(e) = klogger.debug_log.ready_signal().await {
                    break Some((Err(e), (is_readable, klogger)));
                }
                is_readable = true;
                match klogger.read_log() {
                    Err(zx::Status::SHOULD_WAIT) => {
                        is_readable = false;
                        continue;
                    }
                    x => break Some((x, (is_readable, klogger))),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::testing::*;
    use diagnostics_data::{BuilderArgs, LogsDataBuilder, Severity};
    use futures::stream::{StreamExt, TryStreamExt};

    #[fuchsia::test]
    fn logger_existing_logs_test() {
        let debug_log = TestDebugLog::default();
        let klog = TestDebugEntry::new("test log".as_bytes());
        debug_log.enqueue_read_entry(&klog);
        debug_log.enqueue_read_fail(zx::Status::SHOULD_WAIT);
        let mut log_bridge = DebugLogBridge::create(debug_log);

        assert_eq!(
            log_bridge
                .existing_logs()
                .unwrap()
                .into_iter()
                .map(|m| m.parse(&KERNEL_IDENTITY).unwrap())
                .collect::<Vec<_>>(),
            vec![
                LogsDataBuilder::new(BuilderArgs {
                    timestamp: klog.record.timestamp,
                    component_url: Some(KERNEL_IDENTITY.url.clone()),
                    moniker: KERNEL_IDENTITY.moniker.clone(),
                    severity: Severity::Info,
                })
                .set_pid(klog.record.pid.raw_koid())
                .set_tid(klog.record.tid.raw_koid())
                .add_tag("klog")
                .set_message("test log".to_string())
                .build()
            ]
        );

        // Unprocessable logs should be skipped.
        let debug_log = TestDebugLog::default();
        // This is a malformed record because the message contains invalid UTF8.
        let malformed_klog = TestDebugEntry::new(b"\x80");
        debug_log.enqueue_read_entry(&malformed_klog);

        debug_log.enqueue_read_fail(zx::Status::SHOULD_WAIT);
        let mut log_bridge = DebugLogBridge::create(debug_log);
        assert!(!log_bridge.existing_logs().unwrap().is_empty());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn logger_keep_listening_after_exhausting_initial_contents_test() {
        let debug_log = TestDebugLog::default();
        debug_log.enqueue_read_entry(&TestDebugEntry::new("test log".as_bytes()));
        debug_log.enqueue_read_fail(zx::Status::SHOULD_WAIT);
        debug_log.enqueue_read_entry(&TestDebugEntry::new("second test log".as_bytes()));
        let log_bridge = DebugLogBridge::create(debug_log);
        let mut log_stream =
            Box::pin(log_bridge.listen()).map(|r| r.unwrap().parse(&KERNEL_IDENTITY));
        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(log_message.msg().unwrap(), "test log");
        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(log_message.msg().unwrap(), "second test log");

        // Unprocessable logs should NOT be skipped.
        let debug_log = TestDebugLog::default();
        // This is a malformed record because the message contains invalid UTF8.
        let malformed_klog = TestDebugEntry::new(b"\x80");
        debug_log.enqueue_read_entry(&malformed_klog);

        debug_log.enqueue_read_entry(&TestDebugEntry::new("test log".as_bytes()));
        let log_bridge = DebugLogBridge::create(debug_log);
        let mut log_stream = Box::pin(log_bridge.listen());
        let log_message =
            log_stream.try_next().await.unwrap().unwrap().parse(&KERNEL_IDENTITY).unwrap();
        assert_eq!(log_message.msg().unwrap(), "�");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn severity_parsed_from_log() {
        let debug_log = TestDebugLog::default();
        debug_log.enqueue_read_entry(&TestDebugEntry::new("ERROR: first log".as_bytes()));
        // We look for the string 'ERROR:' to label this as a Severity::Error.
        debug_log.enqueue_read_entry(&TestDebugEntry::new("first log error".as_bytes()));
        debug_log.enqueue_read_entry(&TestDebugEntry::new("WARNING: second log".as_bytes()));
        debug_log.enqueue_read_entry(&TestDebugEntry::new("INFO: third log".as_bytes()));
        debug_log.enqueue_read_entry(&TestDebugEntry::new("fourth log".as_bytes()));
        debug_log.enqueue_read_entry(&TestDebugEntry::new_with_severity(
            "ERROR: severity takes precedence over msg when not info".as_bytes(),
            zx::DebugLogSeverity::Warn,
        ));
        // Create a string prefixed with multi-byte UTF-8 characters. This entry will be labeled as
        // Info rather than Error because the string "ERROR:" only appears after the
        // MAX_STRING_SEARCH_SIZE. It's crucial that we use multi-byte UTF-8 characters because we
        // want to verify that the search is character oriented rather than byte oriented and that
        // it can handle the MAX_STRING_SEARCH_SIZE boundary falling in the middle of a multi-byte
        // character.
        let long_padding = (0..100).map(|_| "\u{10FF}").collect::<String>();
        let long_log = format!("{long_padding}ERROR: fifth log");
        debug_log.enqueue_read_entry(&TestDebugEntry::new(long_log.as_bytes()));

        let log_bridge = DebugLogBridge::create(debug_log);
        let mut log_stream =
            Box::pin(log_bridge.listen()).map(|r| r.unwrap().parse(&KERNEL_IDENTITY));

        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(log_message.msg().unwrap(), "ERROR: first log");
        assert_eq!(log_message.metadata.severity, Severity::Error);

        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(log_message.msg().unwrap(), "first log error");
        assert_eq!(log_message.metadata.severity, Severity::Info);

        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(log_message.msg().unwrap(), "WARNING: second log");
        assert_eq!(log_message.metadata.severity, Severity::Warn);

        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(log_message.msg().unwrap(), "INFO: third log");
        assert_eq!(log_message.metadata.severity, Severity::Info);

        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(log_message.msg().unwrap(), "fourth log");
        assert_eq!(log_message.metadata.severity, Severity::Info);

        let log_message = log_stream.try_next().await.unwrap().unwrap();
        assert_eq!(
            log_message.msg().unwrap(),
            "ERROR: severity takes precedence over msg when not info"
        );
        assert_eq!(log_message.metadata.severity, Severity::Warn);

        // TODO(https://fxbug.dev/42154302): Once 74601 is resolved, uncomment the lines below. Prior to 74601
        // being resolved, the follow case may fail because the line is very long, may be truncated,
        // and if it is truncated, may no longer be valid UTF8 because the truncation may occur in
        // the middle of a multi-byte character.
        //
        // let log_message = log_stream.try_next().await.unwrap().unwrap();
        // assert_eq!(log_message.msg().unwrap(), &long_log);
        // assert_eq!(log_message.metadata.severity, Severity::Info);
    }
}
