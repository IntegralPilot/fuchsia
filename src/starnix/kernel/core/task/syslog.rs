// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security;
use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
use crate::task::{
    CurrentTask, EventHandler, Kernel, ThreadLockupDetector, WaitCallback, WaitCanceler, WaitQueue,
    Waiter,
};
use crate::vfs::OutputBuffer;
use diagnostics_data::{Data, Logs, LogsData, Severity};
use estimate_timeline::{DefaultFetcher, TimeFetcher, TimelineEstimator};
use fidl_fuchsia_diagnostics as fdiagnostics;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_inspect::Inspector;
use futures::FutureExt;
use serde::Deserialize;
use starnix_sync::{LockDepMutex, SyslogStateLock, SyslogSubscriptionLock};
use starnix_uapi::auth::CAP_SYSLOG;
use starnix_uapi::errors::{EAGAIN, Errno, errno, error};
use starnix_uapi::syslog::SyslogAction;
use starnix_uapi::vfs::FdEvents;
use std::cmp;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock, mpsc};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, TryFromBytes};

const BUFFER_SIZE: i32 = 1_049_000;

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const MICROS_PER_NANOSECOND: i64 = 1_000;

#[derive(Default)]
pub struct Syslog {
    syscall_subscription: OnceLock<LockDepMutex<LogSubscription, SyslogSubscriptionLock>>,
    state: Arc<LockDepMutex<TimelineEstimator<DefaultFetcher>, SyslogStateLock>>,
}

#[derive(Debug)]
pub enum SyslogAccess {
    DevKmsgRead,
    ProcKmsg(SyslogAction),
    Syscall(SyslogAction),
}

impl Syslog {
    pub fn init(&self, kernel: &Kernel) -> Result<(), anyhow::Error> {
        let state = self.state.clone();
        kernel.inspect_node.record_lazy_child("syslog", move || {
            let state = state.clone();
            async move {
                let inspector = Inspector::default();
                let state_guard = state.lock();
                inspector.root().record_uint("max_timeline_size", state_guard.max_timeline_size());
                inspector
                    .root()
                    .record_uint("timeline_overflows", state_guard.timeline_overflows());
                Ok(inspector)
            }
            .boxed()
        });

        let subscription = LogSubscription::snapshot_then_subscribe(kernel)?;
        self.syscall_subscription.set(subscription.into()).expect("syslog inititialized once");
        Ok(())
    }

    pub fn access(
        &self,
        current_task: &CurrentTask,
        access: SyslogAccess,
    ) -> Result<GrantedSyslog<'_>, Errno> {
        Self::validate_access(current_task, access)?;
        let syscall_subscription = self.subscription()?;
        Ok(GrantedSyslog { syscall_subscription })
    }

    /// Validates that syslog access is unrestricted, or that the `current_task` has the relevant
    /// capability, and applies the SELinux policy.
    pub fn validate_access(current_task: &CurrentTask, access: SyslogAccess) -> Result<(), Errno> {
        let (action, check_capabilities) = match access {
            SyslogAccess::ProcKmsg(SyslogAction::Open) => (SyslogAction::Open, true),
            SyslogAccess::DevKmsgRead => (SyslogAction::ReadAll, true),
            SyslogAccess::Syscall(a) => (a, true),
            // If we got here we already validated Open on /proc/kmsg.
            SyslogAccess::ProcKmsg(a) => (a, false),
        };

        // According to syslog(2) man, ReadAll (3) and SizeBuffer (10) are allowed unprivileged
        // access only if restrict_dmsg is 0.
        let action_is_privileged = !matches!(
            access,
            SyslogAccess::Syscall(SyslogAction::ReadAll | SyslogAction::SizeBuffer)
                | SyslogAccess::DevKmsgRead,
        );
        let restrict_dmesg = current_task.kernel().restrict_dmesg.load(Ordering::Relaxed);
        if check_capabilities && (action_is_privileged || restrict_dmesg) {
            security::check_task_capable(current_task, CAP_SYSLOG)?;
        }

        security::check_syslog_access(current_task, action)?;
        Ok(())
    }

    pub fn snapshot_then_subscribe(kernel: &Kernel) -> Result<LogSubscription, Errno> {
        LogSubscription::snapshot_then_subscribe(kernel)
    }

    pub fn subscribe(kernel: &Kernel) -> Result<LogSubscription, Errno> {
        LogSubscription::subscribe(kernel)
    }

    fn subscription(
        &self,
    ) -> Result<&LockDepMutex<LogSubscription, SyslogSubscriptionLock>, Errno> {
        self.syscall_subscription.get().ok_or_else(|| errno!(ENOENT))
    }
}

pub struct GrantedSyslog<'a> {
    syscall_subscription: &'a LockDepMutex<LogSubscription, SyslogSubscriptionLock>,
}

impl GrantedSyslog<'_> {
    pub fn read(&self, out: &mut dyn OutputBuffer) -> Result<i32, Errno> {
        let mut subscription = self.syscall_subscription.lock();
        if let Some(log) = subscription.try_next()? {
            let size_to_write = cmp::min(log.len(), out.available() as usize);
            out.write(&log[..size_to_write])?;
            return Ok(size_to_write as i32);
        }
        Ok(0)
    }

    pub fn wait(&self, waiter: &Waiter, events: FdEvents, handler: EventHandler) -> WaitCanceler {
        self.syscall_subscription.lock().wait(waiter, events, handler)
    }

    pub fn blocking_read(
        &self,
        current_task: &CurrentTask,
        out: &mut dyn OutputBuffer,
    ) -> Result<i32, Errno> {
        let mut write_log = |log: Vec<u8>| {
            let size_to_write = cmp::min(log.len(), out.available() as usize);
            out.write(&log[..size_to_write])?;
            Ok(size_to_write as i32)
        };
        match self.syscall_subscription.lock().try_next() {
            Err(errno) if errno == EAGAIN => {}
            Err(errno) => return Err(errno),
            Ok(Some(log)) => return write_log(log),
            Ok(None) => return Ok(0),
        }
        let waiter = Waiter::new();
        loop {
            let mut subscription = self.syscall_subscription.lock();
            subscription.wait(&waiter, FdEvents::POLLIN | FdEvents::POLLHUP, WaitCallback::none());
            match subscription.try_next() {
                Err(errno) if errno == EAGAIN => {}
                Err(errno) => return Err(errno),
                Ok(Some(log)) => return write_log(log),
                Ok(None) => return Ok(0),
            }
            // Drop the subscription lock to avoid holding a lock while waiting.
            std::mem::drop(subscription);
            waiter.wait(current_task)?;
        }
    }

    pub fn read_all(&self, kernel: &Kernel, out: &mut dyn OutputBuffer) -> Result<i32, Errno> {
        let mut subscription = LogSubscription::snapshot(kernel)?;
        let mut buffer = ResultBuffer::new(out.available());
        while let Some(log_result) = subscription.next() {
            buffer.push(log_result?);
        }
        let result: Vec<u8> = buffer.into();
        out.write(result.as_slice())?;
        Ok(result.len() as i32)
    }

    pub fn size_unread(&self) -> Result<i32, Errno> {
        let mut subscription = self.syscall_subscription.lock();
        Ok(subscription.available()?.try_into().unwrap_or(i32::MAX))
    }

    pub fn size_buffer(&self) -> Result<i32, Errno> {
        // For now always return a constant for this.
        Ok(BUFFER_SIZE)
    }
}

#[derive(Debug)]
pub struct LogSubscription {
    pending: Option<Vec<u8>>,
    receiver: mpsc::Receiver<Result<Vec<u8>, Errno>>,
    waiters: Arc<WaitQueue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    Many(Vec<T>),
    One(T),
}

impl LogSubscription {
    pub fn wait(&self, waiter: &Waiter, events: FdEvents, handler: EventHandler) -> WaitCanceler {
        self.waiters.wait_async_fd_events(waiter, events, handler)
    }

    pub fn available(&mut self) -> Result<usize, Errno> {
        if let Some(log) = &self.pending {
            return Ok(log.len());
        }
        match self.try_next() {
            Err(err) if err == EAGAIN => Ok(0),
            Err(err) => Err(err),
            Ok(Some(log)) => {
                let size = log.len();
                self.pending.replace(log);
                return Ok(size);
            }
            Ok(None) => Ok(0),
        }
    }

    fn snapshot(kernel: &Kernel) -> Result<LogIterator, Errno> {
        LogIterator::new(&kernel.syslog, fdiagnostics::StreamMode::Snapshot)
    }

    fn subscribe(kernel: &Kernel) -> Result<Self, Errno> {
        Self::new_listening(kernel, fdiagnostics::StreamMode::Subscribe)
    }

    fn snapshot_then_subscribe(kernel: &Kernel) -> Result<Self, Errno> {
        Self::new_listening(kernel, fdiagnostics::StreamMode::SnapshotThenSubscribe)
    }

    fn new_listening(kernel: &Kernel, mode: fdiagnostics::StreamMode) -> Result<Self, Errno> {
        let iterator = LogIterator::new(&kernel.syslog, mode)?;
        let (snd, receiver) = mpsc::sync_channel(1);
        let waiters = Arc::new(WaitQueue::default());
        let waiters_clone = waiters.clone();
        let closure = move |_: &CurrentTask| {
            scopeguard::defer! {
                waiters_clone.notify_fd_events(FdEvents::POLLHUP);
            };
            for log in iterator {
                let send_result = {
                    let _waiting_guard = ThreadLockupDetector::pause_tracking();
                    snd.send(log)
                };
                if send_result.is_err() {
                    break;
                };
                waiters_clone.notify_fd_events(FdEvents::POLLIN);
            }
        };
        let req = SpawnRequestBuilder::new()
            .with_debug_name("syslog-listener")
            .with_sync_closure(closure)
            .build();
        kernel.kthreads.spawner().spawn_from_request(req);

        Ok(Self { receiver, waiters, pending: Default::default() })
    }

    fn try_next(&mut self) -> Result<Option<Vec<u8>>, Errno> {
        if let Some(value) = self.pending.take() {
            return Ok(Some(value));
        }
        match self.receiver.try_recv() {
            // We got the next log.
            Ok(Ok(log)) => Ok(Some(log)),
            // An error happened attempting to get the next log.
            Ok(Err(err)) => Err(err),
            // The channel was closed and there's no more messages in the queue.
            Err(mpsc::TryRecvError::Disconnected) => Ok(None),
            // No messages available but the channel hasn't closed.
            Err(mpsc::TryRecvError::Empty) => error!(EAGAIN),
        }
    }
}

struct LogIterator {
    iterator: fdiagnostics::BatchIteratorSynchronousProxy,
    pending_formatted_contents: VecDeque<fdiagnostics::FormattedContent>,
    pending_datas: VecDeque<Data<Logs>>,
    state: Arc<LockDepMutex<TimelineEstimator<DefaultFetcher>, SyslogStateLock>>,
    tags: std::collections::HashMap<u64, diagnostics_message::MonikerWithUrl>,
}

impl LogIterator {
    fn new(syslog: &Syslog, mode: fdiagnostics::StreamMode) -> Result<Self, Errno> {
        let accessor = connect_to_protocol_sync::<fdiagnostics::ArchiveAccessorMarker>()
            .map_err(|_| errno!(ENOENT, format!("Failed to connecto to ArchiveAccessor")))?;
        let is_subscribe = matches!(mode, fdiagnostics::StreamMode::Subscribe);
        let stream_parameters = fdiagnostics::StreamParameters {
            stream_mode: Some(mode),
            data_type: Some(fdiagnostics::DataType::Logs),
            format: Some(fdiagnostics::Format::Fxt),
            client_selector_configuration: Some(
                fdiagnostics::ClientSelectorConfiguration::SelectAll(true),
            ),
            ..fdiagnostics::StreamParameters::default()
        };
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fdiagnostics::BatchIteratorMarker>();
        accessor.stream_diagnostics(&stream_parameters, server_end).map_err(|err| {
            errno!(EIO, format!("ArchiveAccessor/StreamDiagnostics failed: {err}"))
        })?;
        let iterator = fdiagnostics::BatchIteratorSynchronousProxy::new(client_end.into_channel());
        if is_subscribe {
            let () = iterator.wait_for_ready(zx::MonotonicInstant::INFINITE).map_err(|err| {
                errno!(EIO, format!("Failed to wait for BatchIterator being ready: {err}"))
            })?;
        }
        Ok(Self {
            iterator,
            pending_formatted_contents: VecDeque::new(),
            pending_datas: VecDeque::new(),
            state: syslog.state.clone(),
            tags: std::collections::HashMap::new(),
        })
    }

    // TODO(b/315520045): Investigate if we should make this
    // not allocate anything.
    fn get_next(&mut self) -> Result<Option<Vec<u8>>, Errno> {
        'main_loop: loop {
            while let Some(data) = self.pending_datas.pop_front() {
                if let Some(log) = format_log(data, &self.state).map_err(|_| errno!(EIO))? {
                    return Ok(Some(log));
                }
            }
            while let Some(formatted_content) = self.pending_formatted_contents.pop_front() {
                let output: OneOrMany<Data<Logs>> = match formatted_content {
                    fdiagnostics::FormattedContent::Fxt(data) => {
                        let buf = data
                            .read_to_vec(
                                0,
                                data.get_content_size().map_err(|a| {
                                    errno!(EIO, format!("Error {a} getting VMO size"))
                                })?,
                            )
                            .map_err(|err| {
                                errno!(EIO, format!("failed to read logs vmo: {err}"))
                            })?;
                        let mut current_slice = buf.as_ref();
                        let mut ret: Option<OneOrMany<LogsData>> = None;
                        loop {
                            let (record, remaining) =
                                diagnostics_log_encoding::parse::parse_record(current_slice)
                                    .map_err(|a| errno!(EIO, format!("Error {a} parsing FXT")))?;

                            let record_len = current_slice.len() - remaining.len();
                            let record_bytes = &current_slice[..record_len];

                            let header = diagnostics_log_encoding::Header::read_from_bytes(
                                &current_slice[..8],
                            )
                            .map_err(|_| errno!(EIO, "Invalid FXT header"))?;
                            let tag = header.tag();
                            let is_manifest =
                                (tag & diagnostics_log_encoding::LOG_CONTROL_BIT) != 0;
                            let actual_tag = tag & !diagnostics_log_encoding::LOG_CONTROL_BIT;

                            if is_manifest {
                                let mut moniker = None;
                                let mut url = None;
                                for arg in &record.arguments {
                                    use diagnostics_log_encoding::Value;
                                    if arg.name() == "moniker" {
                                        if let Value::Text(t) = arg.value() {
                                            moniker = Some(diagnostics_data::ExtendedMoniker::parse_str(&t).unwrap_or_else(|_| diagnostics_data::ExtendedMoniker::ComponentInstance(moniker::Moniker::parse_str("unknown").unwrap())));
                                        }
                                    } else if arg.name() == "url" {
                                        if let Value::Text(t) = arg.value() {
                                            url = Some(flyweights::FlyStr::new(t));
                                        }
                                    }
                                }
                                if let (Some(moniker), Some(url)) = (moniker, url) {
                                    self.tags.insert(
                                        actual_tag as u64,
                                        diagnostics_message::MonikerWithUrl { moniker, url },
                                    );
                                }
                            } else {
                                let source = self
                                    .tags
                                    .get(&(actual_tag as u64))
                                    .cloned()
                                    .unwrap_or_else(|| diagnostics_message::MonikerWithUrl {
                                        moniker:
                                            diagnostics_data::ExtendedMoniker::ComponentInstance(
                                                moniker::Moniker::parse_str("unknown").unwrap(),
                                            ),
                                        url: flyweights::FlyStr::new("unknown"),
                                    });

                                let data =
                                    diagnostics_message::from_structured(source, record_bytes)
                                        .map_err(|a| {
                                            errno!(EIO, format!("Error {a} parsing FXT"))
                                        })?;

                                ret = Some(match ret.take() {
                                    Some(OneOrMany::One(one)) => OneOrMany::Many(vec![one, data]),
                                    Some(OneOrMany::Many(mut many)) => {
                                        many.push(data);
                                        OneOrMany::Many(many)
                                    }
                                    None => OneOrMany::One(data),
                                });
                            }

                            if remaining.is_empty() {
                                break;
                            }
                            current_slice = remaining;
                        }
                        ret.unwrap_or_else(|| OneOrMany::Many(vec![]))
                    }
                    format => {
                        unreachable!("we only request and expect one format. Got: {format:?}")
                    }
                };
                match output {
                    OneOrMany::One(data) => {
                        if let Some(log) = format_log(data, &self.state).map_err(|_| errno!(EIO))? {
                            return Ok(Some(log));
                        }
                    }
                    OneOrMany::Many(datas) => {
                        if datas.len() > 0 {
                            self.pending_datas.extend(datas);
                            continue 'main_loop;
                        }
                    }
                }
            }
            let next_batch = {
                let _waiting_guard = ThreadLockupDetector::pause_tracking();
                self.iterator
                    .get_next(zx::MonotonicInstant::INFINITE)
                    .map_err(|_| errno!(ENOENT))?
                    .map_err(|_| errno!(ENOENT))?
            };
            if next_batch.is_empty() {
                return Ok(None);
            }
            self.pending_formatted_contents = VecDeque::from(next_batch);
        }
    }
}

impl Iterator for LogIterator {
    type Item = Result<Vec<u8>, Errno>;

    fn next(&mut self) -> Option<Result<Vec<u8>, Errno>> {
        self.get_next().transpose()
    }
}

impl Iterator for LogSubscription {
    type Item = Result<Vec<u8>, Errno>;

    fn next(&mut self) -> Option<Self::Item> {
        self.try_next().transpose()
    }
}

struct ResultBuffer {
    max_size: usize,
    buffer: VecDeque<Vec<u8>>,
    current_size: usize,
}

impl ResultBuffer {
    fn new(max_size: usize) -> Self {
        Self { max_size, buffer: VecDeque::default(), current_size: 0 }
    }

    fn push(&mut self, data: Vec<u8>) {
        while !self.buffer.is_empty() && self.current_size + data.len() > self.max_size {
            let old = self.buffer.pop_front().unwrap();
            self.current_size -= old.len();
        }
        self.current_size += data.len();
        self.buffer.push_back(data);
    }
}

impl Into<Vec<u8>> for ResultBuffer {
    fn into(self) -> Vec<u8> {
        let mut result = Vec::with_capacity(self.current_size);
        for mut item in self.buffer {
            result.append(&mut item);
        }
        // If we still exceed the size (for example, a single message of size N in a buffer of
        // size M when N>M), we trim the output.
        result.truncate(self.max_size);
        result
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone, KnownLayout, TryFromBytes, Immutable, IntoBytes)]
#[repr(u8)]
pub enum KmsgLevel {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl KmsgLevel {
    fn from_raw(value: u8) -> Option<KmsgLevel> {
        zerocopy::try_transmute!(value).ok()
    }
}

/// Given a string starting with <[0-9]*>, returns the level interpreted from the lower 3 bits.
/// The next 8 is the facility, which we ignore atm.
/// If the string doesn't start with a valid level, we return None.
/// The slice returned is the rest of the message after the closing '>'.
///
/// Reference: https://www.kernel.org/doc/Documentation/ABI/testing/dev-kmsg
pub(crate) fn extract_level(msg: &[u8]) -> Option<(KmsgLevel, &[u8])> {
    let mut bytes_iter = msg.iter();
    let Some(c) = bytes_iter.next() else {
        return None;
    };
    if *c != b'<' {
        return None;
    }
    let Some(end) = bytes_iter.enumerate().find(|(_, c)| **c == b'>').map(|(i, _)| i + 1) else {
        return None;
    };
    std::str::from_utf8(&msg[1..end])
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|level| (level & 0x07) as u8)
        .and_then(KmsgLevel::from_raw)
        .map(|level| (level, &msg[end + 1..]))
}

fn format_log<T: TimeFetcher>(
    data: Data<Logs>,
    state: &Arc<LockDepMutex<TimelineEstimator<T>, SyslogStateLock>>,
) -> Result<Option<Vec<u8>>, io::Error> {
    let mut formatted_tags = match data.tags() {
        None => vec![],
        Some(tags) => {
            let mut formatted = vec![];
            for (i, tag) in tags.iter().enumerate() {
                // TODO(b/299533466): remove this.
                if tag.contains("fxlogcat") {
                    return Ok(None);
                }
                if i != 0 {
                    write!(&mut formatted, ",")?;
                }
                write!(&mut formatted, "{tag}")?;
            }
            write!(&mut formatted, ": ")?;
            formatted
        }
    };

    let mut result = Vec::<u8>::new();
    let (level, msg_after_level) = match data.msg().and_then(|msg| extract_level(msg.as_bytes())) {
        Some((level, remaining_msg)) => (level as u8, Some(remaining_msg)),
        None => match data.severity() {
            Severity::Trace | Severity::Debug => (KmsgLevel::Debug as u8, None),
            Severity::Info => (KmsgLevel::Info as u8, None),
            Severity::Warn => (KmsgLevel::Warning as u8, None),
            Severity::Error => (KmsgLevel::Error as u8, None),
            Severity::Fatal => (KmsgLevel::Critical as u8, None),
        },
    };

    // TODO(https://fxbug.dev/433724019): this isn't correct strictly speaking, but will be in most
    // cases. We unapply the *current* offset and in the case where suspension happened between
    // when the log message was generated and when Starnix is forwarding the log message, this will
    // be different from the *actual* offset prior to suspension.
    let time = state.lock().boot_time_to_monotonic_time(data.metadata.timestamp);
    let time_nanos = time.into_nanos();
    let time_secs = time_nanos / NANOS_PER_SECOND;
    // Microsecond-level precision fractional time.
    let time_fract = (time_nanos % NANOS_PER_SECOND) / MICROS_PER_NANOSECOND;
    let component_name = data.component_name();
    write!(&mut result, "<{level}>[{time_secs:05}.{time_fract:06}] {component_name}",)?;

    match data.metadata.pid {
        Some(pid) => write!(&mut result, "[{pid}]: ")?,
        None => write!(&mut result, ": ")?,
    }

    result.append(&mut formatted_tags);

    if let Some(msg) = msg_after_level {
        write!(&mut result, "{}", String::from_utf8_lossy(msg))?;
    } else if let Some(msg) = data.msg() {
        write!(&mut result, "{msg}")?;
    }

    for kvp in data.payload_keys_strings() {
        write!(&mut result, " {kvp}")?;
    }
    write!(&mut result, "\n")?;
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_buffer() {
        let mut buffer = ResultBuffer::new(100);
        buffer.push(vec![0; 200]);
        let result: Vec<u8> = buffer.into();
        assert_eq!(result.len(), 100);

        let mut buffer = ResultBuffer::new(100);
        buffer.push(Vec::from_iter(0..20));
        buffer.push(Vec::from_iter(20..50));
        let result: Vec<u8> = buffer.into();
        assert_eq!(result.len(), 50);
        for i in 0..50u8 {
            assert_eq!(result[i as usize], i);
        }

        let mut buffer = ResultBuffer::new(100);
        buffer.push(Vec::from_iter(0..20));
        buffer.push(Vec::from_iter(20..150));
        let result: Vec<u8> = buffer.into();
        assert_eq!(result.len(), 100);
        for i in 0..100u8 {
            assert_eq!(result[i as usize], i + 20u8);
        }

        let mut buffer = ResultBuffer::new(100);
        buffer.push(Vec::from_iter(0..20));
        buffer.push(Vec::from_iter(20..150));
        buffer.push(Vec::from_iter(150..210));
        let result: Vec<u8> = buffer.into();
        assert_eq!(result.len(), 60);
        for i in 0..60u8 {
            assert_eq!(result[i as usize], i + 150u8);
        }
    }

    #[test]
    fn test_extract_level() {
        for level in 0..8 {
            let msg = format!("<{level}> some message");
            let result = extract_level(msg.as_bytes()).map(|(x, i)| (x as u8, i));
            assert_eq!(Some((level, " some message".as_bytes())), result);
        }
    }

    #[test]
    fn parse_message_accepts_levels_greater_than_7() {
        assert_eq!(
            Some((KmsgLevel::Warning, " message".as_bytes())),
            extract_level("<100> message".as_bytes())
        );
    }

    #[test]
    fn parse_message_defaults_when_non_numbers() {
        assert_eq!(None, extract_level("<a> some message".as_bytes()));
    }

    #[test]
    fn parse_message_defaults_when_invalid_level_syntax() {
        assert_eq!(None, extract_level("<1 some message".as_bytes()));
    }

    #[test]
    fn parse_message_defaults_when_no_level() {
        assert_eq!(None, extract_level("some message".as_bytes()));
    }
}
