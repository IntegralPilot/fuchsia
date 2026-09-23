// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filter::LogFilterCriteria;
use crate::log_socket_stream::{JsonDeserializeError, LogsDataStream};
use crate::{
    DetailedDateTime, InstanceGetter, LogCommand, LogError, LogProcessingResult, LogSubCommand,
    TimeFormat,
};
use anyhow::Result;
use async_trait::async_trait;
use diagnostics_data::{
    Data, LogTextColor, LogTextDisplayOptions, LogTextPresenter, LogTimeDisplayFormat, Logs,
    LogsData, LogsDataBuilder, LogsField, LogsProperty, Severity, Timezone,
};
use futures_util::future::Either;
use futures_util::stream::FuturesUnordered;
use futures_util::{StreamExt, select};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::io::Write;
use std::time::Duration;
use thiserror::Error;
use writer::ToolIO;

pub use diagnostics_data::Timestamp;

pub const TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S.%3f";

/// Type of data in a log entry
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LogData {
    /// A log entry from the target
    TargetLog(LogsData),
}

impl LogData {
    /// Gets the LogData as a target log.
    pub fn as_target_log(&self) -> Option<&LogsData> {
        match self {
            LogData::TargetLog(log) => Some(log),
        }
    }

    pub fn as_target_log_mut(&mut self) -> Option<&mut LogsData> {
        match self {
            LogData::TargetLog(log) => Some(log),
        }
    }
}

impl From<LogsData> for LogData {
    fn from(data: LogsData) -> Self {
        Self::TargetLog(data)
    }
}

/// A log entry from either the host, target, or
/// a symbolized log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The log
    pub data: LogData,
}

impl LogEntry {
    fn utc_timestamp(&self, boot_ts: Option<Timestamp>) -> Timestamp {
        Timestamp::from_nanos(match &self.data {
            LogData::TargetLog(data) => {
                data.metadata.timestamp.into_nanos()
                    + boot_ts.map(|value| value.into_nanos()).unwrap_or(0)
            }
        })
    }
}

// Required if we want to use ffx's built-in I/O, but
// this isn't really applicable to us because we have
// custom formatting rules.
impl Display for LogEntry {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unreachable!("UNSUPPORTED -- This type cannot be formatted with std format.");
    }
}

/// A trait for symbolizing log entries
#[async_trait(?Send)]
pub trait Symbolize {
    /// Symbolizes a LogEntry and optionally produces a result.
    /// The symbolizer may choose to discard the result.
    /// This method may be called multiple times concurrently.
    async fn symbolize(&self, entry: LogEntry) -> Option<LogEntry>;
}

async fn handle_value<S>(one: Data<Logs>, symbolizer: &S) -> Option<LogEntry>
where
    S: Symbolize + ?Sized,
{
    let entry = LogEntry { data: one.into() };
    symbolizer.symbolize(entry).await
}

fn generate_timestamp_message(boot_timestamp: Timestamp) -> LogEntry {
    LogEntry {
        data: LogData::TargetLog(
            LogsDataBuilder::new(diagnostics_data::BuilderArgs {
                moniker: "ffx".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("ffx".into()),
                severity: Severity::Info,
            })
            .set_message("Logging started")
            .add_key(LogsProperty::String(
                LogsField::Other("utc_time_now".into()),
                chrono::Utc::now().to_rfc3339(),
            ))
            .add_key(LogsProperty::Int(
                LogsField::Other("current_boot_timestamp".to_string()),
                boot_timestamp.into_nanos(),
            ))
            .build(),
        ),
    }
}

/// Reads logs from a socket and formats them using the given formatter and symbolizer.
pub async fn dump_logs_from_socket<F, S>(
    socket: flex_client::AsyncSocket,
    formatter: &mut F,
    symbolizer: &S,
    include_timestamp: bool,
) -> Result<LogProcessingResult, JsonDeserializeError>
where
    F: LogFormatter + BootTimeAccessor,
    S: Symbolize + ?Sized,
{
    let mut decoder = Box::pin(LogsDataStream::new(socket).fuse());
    let mut symbolize_pending = FuturesUnordered::new();
    if include_timestamp && !formatter.is_utc_time_format() {
        formatter.push_log(generate_timestamp_message(formatter.get_boot_timestamp())).await?;
    }
    while let Some(value) = select! {
        res = decoder.next() => Some(Either::Left(res)),
        res = symbolize_pending.next() => Some(Either::Right(res)),
        complete => None,
    } {
        match value {
            Either::Left(Some(result)) => match result {
                Ok(log) => symbolize_pending.push(handle_value(log, symbolizer)),
                Err(e) => return Err(e),
            },
            Either::Right(Some(Some(symbolized))) => match formatter.push_log(symbolized).await? {
                LogProcessingResult::Exit => {
                    formatter.flush().await?;
                    return Ok(LogProcessingResult::Exit);
                }
                LogProcessingResult::Continue => {}
            },
            _ => {}
        }
    }
    formatter.flush().await?;
    Ok(LogProcessingResult::Continue)
}

/// Reads FXT logs from a socket and formats them using the given formatter and symbolizer.
pub async fn dump_fxt_logs_from_socket<F, S>(
    socket: flex_client::AsyncSocket,
    formatter: &mut F,
    symbolizer: &S,
    include_timestamp: bool,
) -> Result<LogProcessingResult, LogError>
where
    F: LogFormatter + BootTimeAccessor,
    S: Symbolize + ?Sized,
{
    let streamer = crate::fxt_streamer::FxtStreamer::new(socket);
    let mut decoder = std::pin::pin!(streamer.stream());
    let mut symbolize_pending = FuturesUnordered::new();
    if include_timestamp && !formatter.is_utc_time_format() {
        formatter.push_log(generate_timestamp_message(formatter.get_boot_timestamp())).await?;
    }
    while let Some(value) = select! {
        res = decoder.next() => Some(Either::Left(res)),
        res = symbolize_pending.next() => Some(Either::Right(res)),
        complete => None,
    } {
        match value {
            Either::Left(Some(result)) => match result {
                Ok(log) => symbolize_pending.push(handle_value(log, symbolizer)),
                Err(e) => return Err(e),
            },
            Either::Right(Some(Some(symbolized))) => match formatter.push_log(symbolized).await? {
                LogProcessingResult::Exit => {
                    formatter.flush().await?;
                    return Ok(LogProcessingResult::Exit);
                }
                LogProcessingResult::Continue => {}
            },
            _ => {}
        }
    }
    formatter.flush().await?;
    Ok(LogProcessingResult::Continue)
}

pub trait BootTimeAccessor {
    /// Sets the boot timestamp in nanoseconds since the Unix epoch.
    fn set_boot_timestamp(&mut self, _boot_ts_nanos: Timestamp);

    /// Returns the boot timestamp in nanoseconds since the Unix epoch.
    fn get_boot_timestamp(&self) -> Timestamp;
}

/// Timestamp filter which is either either boot-based or UTC-based.
#[derive(Clone, Debug)]
pub struct DeviceOrLocalTimestamp {
    /// Timestamp in boot time
    pub timestamp: Timestamp,
    /// True if this filter should be applied to boot time,
    /// false if UTC time.
    pub is_boot: bool,
}

impl DeviceOrLocalTimestamp {
    /// Creates a DeviceOrLocalTimestamp from a real-time date/time or
    /// a boot date/time. Returns None if both rtc and boot are None.
    /// Returns None if the timestamp is "now".
    pub fn new(
        rtc: Option<&DetailedDateTime>,
        boot: Option<&Duration>,
    ) -> Option<DeviceOrLocalTimestamp> {
        rtc.as_ref()
            .filter(|value| !value.is_now)
            .map(|value| DeviceOrLocalTimestamp {
                timestamp: Timestamp::from_nanos(
                    value.naive_utc().and_utc().timestamp_nanos_opt().unwrap(),
                ),
                is_boot: false,
            })
            .or_else(|| {
                boot.map(|value| DeviceOrLocalTimestamp {
                    timestamp: Timestamp::from_nanos(value.as_nanos() as i64),
                    is_boot: true,
                })
            })
    }
}

/// Log formatter options
#[derive(Clone, Debug)]
pub struct LogFormatterOptions {
    /// Text display options
    pub display: Option<LogTextDisplayOptions>,
    /// Only display logs since the specified time.
    pub since: Option<DeviceOrLocalTimestamp>,
    /// Only display logs until the specified time.
    pub until: Option<DeviceOrLocalTimestamp>,
    /// Only display the last N log lines.
    pub tail: Option<usize>,
}

impl Default for LogFormatterOptions {
    fn default() -> Self {
        LogFormatterOptions {
            display: Some(Default::default()),
            since: None,
            until: None,
            tail: None,
        }
    }
}

/// Log formatter error
#[derive(Error, Debug)]
pub enum FormatterError {
    /// An unknown error occurred
    #[error(transparent)]
    Other(#[from] anyhow::Error),
    /// An IO error occurred
    #[error(transparent)]
    IO(#[from] std::io::Error),
}

impl FormatterError {
    pub fn is_broken_pipe(&self) -> bool {
        match self {
            FormatterError::IO(error) => error.kind() == std::io::ErrorKind::BrokenPipe,
            FormatterError::Other(_) => false,
        }
    }
}

/// Default formatter implementation
pub struct DefaultLogFormatter<W>
where
    W: Write + ToolIO<OutputItem = LogEntry>,
{
    writer: W,
    filters: LogFilterCriteria,
    options: LogFormatterOptions,
    boot_ts_nanos: Option<Timestamp>,
    tail_queue: std::collections::VecDeque<LogEntry>,
    tail_limit: Option<usize>,
}

/// Converts from UTC time to boot time.
fn utc_to_boot(boot_ts: Timestamp, utc: Timestamp) -> Timestamp {
    Timestamp::from_nanos(utc.into_nanos() - boot_ts.into_nanos())
}

#[async_trait(?Send)]
impl<W> LogFormatter for DefaultLogFormatter<W>
where
    W: Write + ToolIO<OutputItem = LogEntry>,
{
    async fn push_log(&mut self, log_entry: LogEntry) -> Result<LogProcessingResult, LogError> {
        self.push_log_internal(log_entry, true).await.or_else(|err| {
            if err.is_broken_pipe() { Ok(LogProcessingResult::Exit) } else { Err(err) }
        })
    }

    fn is_utc_time_format(&self) -> bool {
        self.options.display.iter().any(|options| match options.time_format {
            LogTimeDisplayFormat::Original => false,
            LogTimeDisplayFormat::WallTime { tz, offset: _ } => tz == Timezone::Utc,
        })
    }

    async fn flush(&mut self) -> Result<(), LogError> {
        self.flush_tail().await
    }
}

impl<W> BootTimeAccessor for DefaultLogFormatter<W>
where
    W: Write + ToolIO<OutputItem = LogEntry>,
{
    fn set_boot_timestamp(&mut self, boot_ts_nanos: Timestamp) {
        if let Some(LogTextDisplayOptions {
            time_format: LogTimeDisplayFormat::WallTime { offset, .. },
            ..
        }) = &mut self.options.display
        {
            *offset = boot_ts_nanos.into_nanos();
        }
        self.boot_ts_nanos = Some(boot_ts_nanos);
    }
    fn get_boot_timestamp(&self) -> Timestamp {
        debug_assert!(self.boot_ts_nanos.is_some());
        self.boot_ts_nanos.unwrap_or_else(|| Timestamp::from_nanos(0))
    }
}

/// Object which contains a Writer that can be borrowed
pub trait WriterContainer<W>
where
    W: Write + ToolIO<OutputItem = LogEntry>,
{
    fn writer(&mut self) -> &mut W;
}

impl<W> WriterContainer<W> for DefaultLogFormatter<W>
where
    W: Write + ToolIO<OutputItem = LogEntry>,
{
    fn writer(&mut self) -> &mut W {
        &mut self.writer
    }
}

impl<W> DefaultLogFormatter<W>
where
    W: Write + ToolIO<OutputItem = LogEntry>,
{
    /// Creates a new DefaultLogFormatter with the given writer and options.
    pub fn new(filters: LogFilterCriteria, writer: W, options: LogFormatterOptions) -> Self {
        let tail_limit = options.tail;
        Self {
            filters,
            writer,
            options,
            boot_ts_nanos: None,
            tail_queue: std::collections::VecDeque::new(),
            tail_limit,
        }
    }

    pub async fn expand_monikers(&mut self, getter: &impl InstanceGetter) -> Result<(), LogError> {
        let warnings = self.filters.expand_monikers(getter).await?;
        for warning in warnings {
            writeln!(
                self.writer.stderr(),
                "WARN: Provided moniker '{}' was not an exact match. Using fuzzy match '{}' instead. Please check your component topology variations.",
                warning.query,
                warning.resolved
            )?;
        }
        Ok(())
    }

    pub async fn push_unfiltered_log(
        &mut self,
        log_entry: LogEntry,
    ) -> Result<LogProcessingResult, LogError> {
        self.push_log_internal(log_entry, false).await
    }

    async fn flush_tail(&mut self) -> Result<(), LogError> {
        while let Some(log_entry) = self.tail_queue.pop_front() {
            self.write_log_entry(log_entry, true)?;
        }
        Ok(())
    }

    fn write_log_entry(
        &mut self,
        log_entry: LogEntry,
        enable_filters: bool,
    ) -> Result<(), LogError> {
        match self.options.display {
            Some(text_options) => {
                let mut options_for_this_line_only = self.options.clone();
                options_for_this_line_only.display = Some(text_options);
                // For host logs, don't apply the boot time offset
                // as this is with reference to the UTC timeline
                if !enable_filters
                    && let LogTimeDisplayFormat::WallTime { ref mut offset, .. } =
                        options_for_this_line_only.display.as_mut().unwrap().time_format
                {
                    // 1 nanosecond so that LogTimeDisplayFormat in diagnostics_data
                    // knows that we have a valid UTC offset. It normally falls back if
                    // the UTC offset is 0. It prints at millisecond precision so being
                    // off by +1 nanosecond isn't an issue.
                    *offset = 1;
                }
                self.format_text_log(options_for_this_line_only, log_entry)
                    .map_err(LogError::FormatterError)?;
            }
            None => {
                self.writer.item(&log_entry).map_err(|err| LogError::UnknownError(err.into()))?;
            }
        };
        Ok(())
    }

    async fn push_log_internal(
        &mut self,
        log_entry: LogEntry,
        enable_filters: bool,
    ) -> Result<LogProcessingResult, LogError> {
        if enable_filters {
            if self.filter_by_timestamp(&log_entry, self.options.since.as_ref(), |a, b| a <= b) {
                return Ok(LogProcessingResult::Continue);
            }

            if self.filter_by_timestamp(&log_entry, self.options.until.as_ref(), |a, b| a >= b) {
                return Ok(LogProcessingResult::Exit);
            }

            if !self.filters.matches(&log_entry) {
                return Ok(LogProcessingResult::Continue);
            }
        }

        if let Some(limit) = self.tail_limit {
            self.tail_queue.push_back(log_entry);
            if self.tail_queue.len() > limit {
                self.tail_queue.pop_front();
            }
        } else {
            self.write_log_entry(log_entry, enable_filters)?;
        }
        Ok(LogProcessingResult::Continue)
    }

    /// Creates a new DefaultLogFormatter from command-line arguments.
    pub fn new_from_args(cmd: &LogCommand, writer: W) -> Result<Self, LogError> {
        let is_json = writer.is_machine();

        Ok(DefaultLogFormatter::new(
            LogFilterCriteria::try_from(cmd.clone())?,
            writer,
            LogFormatterOptions {
                display: if is_json {
                    None
                } else {
                    Some(LogTextDisplayOptions {
                        show_tags: !cmd.hide_tags(),
                        color: if cmd.no_color() {
                            LogTextColor::None
                        } else {
                            LogTextColor::BySeverity
                        },
                        show_metadata: cmd.show_metadata(),
                        time_format: match cmd.clock() {
                            TimeFormat::Boot => LogTimeDisplayFormat::Original,
                            TimeFormat::Local => LogTimeDisplayFormat::WallTime {
                                tz: Timezone::Local,
                                // This will receive a correct value when logging actually starts,
                                // see `set_boot_timestamp()` method on the log formatter.
                                offset: 0,
                            },
                            TimeFormat::Utc => LogTimeDisplayFormat::WallTime {
                                tz: Timezone::Utc,
                                // This will receive a correct value when logging actually starts,
                                // see `set_boot_timestamp()` method on the log formatter.
                                offset: 0,
                            },
                        },
                        show_file: !cmd.hide_file(),
                        show_moniker: !cmd.hide_moniker(),
                        show_full_moniker: cmd.show_full_moniker(),
                        prefer_url_component_name: cmd.prefer_url_component_name(),
                    })
                },
                since: DeviceOrLocalTimestamp::new(cmd.since(), cmd.since_boot().as_ref()),
                until: DeviceOrLocalTimestamp::new(cmd.until(), cmd.until_boot().as_ref()),
                tail: match &cmd.sub_command {
                    Some(LogSubCommand::Dump(dump)) => dump.tail,
                    _ => None,
                },
            },
        ))
    }

    fn filter_by_timestamp(
        &self,
        log_entry: &LogEntry,
        timestamp: Option<&DeviceOrLocalTimestamp>,
        callback: impl Fn(&Timestamp, &Timestamp) -> bool,
    ) -> bool {
        let Some(timestamp) = timestamp else {
            return false;
        };
        if timestamp.is_boot {
            callback(
                &utc_to_boot(
                    self.get_boot_timestamp(),
                    log_entry.utc_timestamp(self.boot_ts_nanos),
                ),
                &timestamp.timestamp,
            )
        } else {
            callback(&log_entry.utc_timestamp(self.boot_ts_nanos), &timestamp.timestamp)
        }
    }

    // This function's arguments are copied to make lifetimes in push_log easier since borrowing
    // &self would complicate spam highlighting.
    fn format_text_log(
        &mut self,
        options: LogFormatterOptions,
        log_entry: LogEntry,
    ) -> Result<(), FormatterError> {
        let text_options = match options.display {
            Some(o) => o,
            None => {
                unreachable!("If we are here, we can only be formatting text");
            }
        };
        match log_entry {
            LogEntry { data: LogData::TargetLog(data), .. } => {
                // TODO(https://fxbug.dev/42072442): Add support for log spam redaction and other
                // features listed in the design doc.
                writeln!(self.writer, "{}", LogTextPresenter::new(&data, text_options))?;
            }
        }
        Ok(())
    }
}

#[allow(dead_code)] // TODO(https://fxbug.dev/421409178)
/// Symbolizer that does nothing.
pub struct NoOpSymbolizer;

#[async_trait(?Send)]
impl Symbolize for NoOpSymbolizer {
    async fn symbolize(&self, entry: LogEntry) -> Option<LogEntry> {
        Some(entry)
    }
}

/// Trait for formatting logs one at a time.
#[async_trait(?Send)]
pub trait LogFormatter {
    /// Formats a log entry and writes it to the output.
    async fn push_log(&mut self, log_entry: LogEntry) -> Result<LogProcessingResult, LogError>;

    /// Returns true if the formatter is configured to output in UTC time format.
    fn is_utc_time_format(&self) -> bool;

    /// Flushes any buffered logs.
    async fn flush(&mut self) -> Result<(), LogError> {
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::parse_time;
    use assert_matches::assert_matches;
    use diagnostics_data::{LogsDataBuilder, Severity};
    use std::cell::Cell;
    use writer::{Format, JsonWriter, TestBuffers};

    use super::*;

    const DEFAULT_TS_NANOS: u64 = 1615535969000000000;

    struct FakeFormatter {
        logs: Vec<LogEntry>,
        boot_timestamp: Timestamp,
        is_utc_time_format: bool,
    }

    impl FakeFormatter {
        fn new() -> Self {
            Self {
                logs: Vec::new(),
                boot_timestamp: Timestamp::from_nanos(0),
                is_utc_time_format: false,
            }
        }
    }

    impl BootTimeAccessor for FakeFormatter {
        fn set_boot_timestamp(&mut self, boot_ts_nanos: Timestamp) {
            self.boot_timestamp = boot_ts_nanos;
        }

        fn get_boot_timestamp(&self) -> Timestamp {
            self.boot_timestamp
        }
    }

    #[async_trait(?Send)]
    impl LogFormatter for FakeFormatter {
        async fn push_log(&mut self, log_entry: LogEntry) -> Result<LogProcessingResult, LogError> {
            self.logs.push(log_entry);
            Ok(LogProcessingResult::Continue)
        }

        fn is_utc_time_format(&self) -> bool {
            self.is_utc_time_format
        }
    }

    /// Symbolizer that prints "Fuchsia".
    pub struct FakeFuchsiaSymbolizer;

    fn set_log_msg(entry: &mut LogEntry, msg: impl Into<String>) {
        *entry.data.as_target_log_mut().unwrap().msg_mut().unwrap() = msg.into();
    }

    #[async_trait(?Send)]
    impl Symbolize for FakeFuchsiaSymbolizer {
        async fn symbolize(&self, mut entry: LogEntry) -> Option<LogEntry> {
            set_log_msg(&mut entry, "Fuchsia");
            Some(entry)
        }
    }

    struct FakeSymbolizerCallback {
        should_discard: Cell<bool>,
    }

    impl FakeSymbolizerCallback {
        fn new() -> Self {
            Self { should_discard: Cell::new(true) }
        }
    }

    async fn dump_logs_from_socket<F, S>(
        socket: fuchsia_async::Socket,
        formatter: &mut F,
        symbolizer: &S,
    ) -> Result<LogProcessingResult, JsonDeserializeError>
    where
        F: LogFormatter + BootTimeAccessor,
        S: Symbolize + ?Sized,
    {
        super::dump_logs_from_socket(socket, formatter, symbolizer, false).await
    }

    #[async_trait(?Send)]
    impl Symbolize for FakeSymbolizerCallback {
        async fn symbolize(&self, mut input: LogEntry) -> Option<LogEntry> {
            self.should_discard.set(!self.should_discard.get());
            if self.should_discard.get() {
                None
            } else {
                set_log_msg(&mut input, "symbolized log");
                Some(input)
            }
        }
    }

    #[fuchsia::test]
    async fn test_boot_timestamp_setter() {
        let buffers = TestBuffers::default();
        let stdout = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let options = LogFormatterOptions {
            display: Some(LogTextDisplayOptions {
                time_format: LogTimeDisplayFormat::WallTime { tz: Timezone::Utc, offset: 0 },
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut formatter =
            DefaultLogFormatter::new(LogFilterCriteria::default(), stdout, options.clone());
        formatter.set_boot_timestamp(Timestamp::from_nanos(1234));
        assert_eq!(formatter.get_boot_timestamp(), Timestamp::from_nanos(1234));

        // Boot timestamp is supported when using JSON output (for filtering)
        let buffers = TestBuffers::default();
        let output = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let options = LogFormatterOptions { display: None, ..Default::default() };
        let mut formatter = DefaultLogFormatter::new(LogFilterCriteria::default(), output, options);
        formatter.set_boot_timestamp(Timestamp::from_nanos(1234));
        assert_eq!(formatter.get_boot_timestamp(), Timestamp::from_nanos(1234));
    }

    #[fuchsia::test]
    async fn test_format_single_message() {
        let symbolizer = NoOpSymbolizer {};
        let mut formatter = FakeFormatter::new();
        let target_log = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message("Hello world!")
        .build();
        let (sender, receiver) = zx::Socket::create_stream();
        sender
            .write(serde_json::to_string(&target_log).unwrap().as_bytes())
            .expect("failed to write target log");
        drop(sender);
        dump_logs_from_socket(flex_client::socket_to_async(receiver), &mut formatter, &symbolizer)
            .await
            .unwrap();
        assert_eq!(formatter.logs, vec![LogEntry { data: LogData::TargetLog(target_log) }]);
    }

    #[fuchsia::test]
    async fn test_format_utc_timestamp() {
        let symbolizer = NoOpSymbolizer {};
        let mut formatter = FakeFormatter::new();
        formatter.set_boot_timestamp(Timestamp::from_nanos(DEFAULT_TS_NANOS as i64));
        let (_, receiver) = zx::Socket::create_stream();
        super::dump_logs_from_socket(
            flex_client::socket_to_async(receiver),
            &mut formatter,
            &symbolizer,
            true,
        )
        .await
        .unwrap();
        let target_log = formatter.logs[0].data.as_target_log().unwrap();
        let properties = target_log.payload_keys().unwrap();
        assert_eq!(target_log.msg().unwrap(), "Logging started");

        // Ensure the end has a valid timestamp
        chrono::DateTime::parse_from_rfc3339(
            properties.get_property("utc_time_now").unwrap().string().unwrap(),
        )
        .unwrap();
        assert_eq!(
            properties.get_property("current_boot_timestamp").unwrap().int().unwrap(),
            DEFAULT_TS_NANOS as i64
        );
    }

    #[fuchsia::test]
    async fn test_format_utc_timestamp_does_not_print_if_utc_time() {
        let symbolizer = NoOpSymbolizer {};
        let mut formatter = FakeFormatter::new();
        formatter.is_utc_time_format = true;
        formatter.set_boot_timestamp(Timestamp::from_nanos(DEFAULT_TS_NANOS as i64));
        let (_, receiver) = zx::Socket::create_stream();
        super::dump_logs_from_socket(
            flex_client::socket_to_async(receiver),
            &mut formatter,
            &symbolizer,
            true,
        )
        .await
        .unwrap();
        assert_eq!(formatter.logs.len(), 0);
    }

    #[fuchsia::test]
    async fn test_format_multiple_messages() {
        let symbolizer = NoOpSymbolizer {};
        let mut formatter = FakeFormatter::new();
        let (sender, receiver) = zx::Socket::create_stream();
        let target_log_0 = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message("Hello world!")
        .set_pid(1)
        .set_tid(2)
        .build();
        let target_log_1 = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(1),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message("Hello world 2!")
        .build();
        sender
            .write(serde_json::to_string(&vec![&target_log_0, &target_log_1]).unwrap().as_bytes())
            .expect("failed to write target log");
        drop(sender);
        dump_logs_from_socket(flex_client::socket_to_async(receiver), &mut formatter, &symbolizer)
            .await
            .unwrap();
        assert_eq!(
            formatter.logs,
            vec![
                LogEntry { data: LogData::TargetLog(target_log_0) },
                LogEntry { data: LogData::TargetLog(target_log_1) }
            ]
        );
    }

    #[fuchsia::test]
    async fn test_format_timestamp_filter() {
        // test since and until args for the LogFormatter
        let symbolizer = NoOpSymbolizer {};
        let buffers = TestBuffers::default();
        let stdout = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let mut formatter = DefaultLogFormatter::new(
            LogFilterCriteria::default(),
            stdout,
            LogFormatterOptions {
                since: Some(DeviceOrLocalTimestamp {
                    timestamp: Timestamp::from_nanos(1),
                    is_boot: true,
                }),
                until: Some(DeviceOrLocalTimestamp {
                    timestamp: Timestamp::from_nanos(3),
                    is_boot: true,
                }),
                ..Default::default()
            },
        );
        formatter.set_boot_timestamp(Timestamp::from_nanos(0));

        let (sender, receiver) = zx::Socket::create_stream();
        let target_log_0 = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message("Hello world!")
        .build();
        let target_log_1 = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(1),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message("Hello world 2!")
        .build();
        let target_log_2 = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(2),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_pid(1)
        .set_tid(2)
        .set_message("Hello world 3!")
        .build();
        let target_log_3 = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(3),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message("Hello world 4!")
        .set_pid(1)
        .set_tid(2)
        .build();
        sender
            .write(
                serde_json::to_string(&vec![
                    &target_log_0,
                    &target_log_1,
                    &target_log_2,
                    &target_log_3,
                ])
                .unwrap()
                .as_bytes(),
            )
            .expect("failed to write target log");
        drop(sender);
        assert_matches!(
            dump_logs_from_socket(
                flex_client::socket_to_async(receiver),
                &mut formatter,
                &symbolizer,
            )
            .await,
            Ok(LogProcessingResult::Exit)
        );
        assert_eq!(
            buffers.stdout.into_string(),
            "[00000.000000][1][2][ffx] INFO: Hello world 3!\n"
        );
    }

    fn make_log_with_timestamp(timestamp: i64) -> LogsData {
        LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(timestamp),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message(format!("Hello world {timestamp}!"))
        .set_pid(1)
        .set_tid(2)
        .build()
    }

    #[fuchsia::test]
    async fn test_format_timestamp_filter_utc() {
        // test since and until args for the LogFormatter
        let symbolizer = NoOpSymbolizer {};
        let buffers = TestBuffers::default();
        let stdout = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let mut formatter = DefaultLogFormatter::new(
            LogFilterCriteria::default(),
            stdout,
            LogFormatterOptions {
                since: Some(DeviceOrLocalTimestamp {
                    timestamp: Timestamp::from_nanos(1),
                    is_boot: false,
                }),
                until: Some(DeviceOrLocalTimestamp {
                    timestamp: Timestamp::from_nanos(3),
                    is_boot: false,
                }),
                display: Some(LogTextDisplayOptions {
                    time_format: LogTimeDisplayFormat::WallTime { tz: Timezone::Utc, offset: 1 },
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        formatter.set_boot_timestamp(Timestamp::from_nanos(1));

        let (sender, receiver) = zx::Socket::create_stream();
        let logs = (0..4).map(make_log_with_timestamp).collect::<Vec<_>>();
        sender
            .write(serde_json::to_string(&logs).unwrap().as_bytes())
            .expect("failed to write target log");
        drop(sender);
        assert_matches!(
            dump_logs_from_socket(
                flex_client::socket_to_async(receiver),
                &mut formatter,
                &symbolizer,
            )
            .await,
            Ok(LogProcessingResult::Exit)
        );
        assert_eq!(
            buffers.stdout.into_string(),
            "[1970-01-01 00:00:00.000][1][2][ffx] INFO: Hello world 1!\n"
        );
    }

    fn logs_data_builder() -> LogsDataBuilder {
        diagnostics_data::LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            timestamp: Timestamp::from_nanos(default_ts().as_nanos() as i64),
            component_url: Some("component_url".into()),
            moniker: "some/moniker".try_into().unwrap(),
            severity: diagnostics_data::Severity::Warn,
        })
        .set_pid(1)
        .set_tid(2)
    }

    fn default_ts() -> Duration {
        Duration::from_nanos(DEFAULT_TS_NANOS)
    }

    fn log_entry() -> LogEntry {
        LogEntry {
            data: LogData::TargetLog(
                logs_data_builder().add_tag("tag1").add_tag("tag2").set_message("message").build(),
            ),
        }
    }

    #[fuchsia::test]
    async fn test_default_formatter() {
        let buffers = TestBuffers::default();
        let stdout = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let options = LogFormatterOptions::default();
        let mut formatter =
            DefaultLogFormatter::new(LogFilterCriteria::default(), stdout, options.clone());
        formatter.push_log(log_entry()).await.unwrap();
        drop(formatter);
        assert_eq!(
            buffers.into_stdout_str(),
            "[1615535969.000000][1][2][some/moniker][tag1,tag2] WARN: message\n"
        );
    }

    #[fuchsia::test]
    async fn test_default_formatter_with_hidden_metadata() {
        let buffers = TestBuffers::default();
        let stdout = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let options = LogFormatterOptions {
            display: Some(LogTextDisplayOptions { show_metadata: false, ..Default::default() }),
            ..LogFormatterOptions::default()
        };
        let mut formatter =
            DefaultLogFormatter::new(LogFilterCriteria::default(), stdout, options.clone());
        formatter.push_log(log_entry()).await.unwrap();
        drop(formatter);
        assert_eq!(
            buffers.into_stdout_str(),
            "[1615535969.000000][some/moniker][tag1,tag2] WARN: message\n"
        );
    }

    #[fuchsia::test]
    async fn test_default_formatter_with_json() {
        let buffers = TestBuffers::default();
        let stdout = JsonWriter::<LogEntry>::new_test(Some(Format::Json), &buffers);
        let options = LogFormatterOptions { display: None, ..Default::default() };
        {
            let mut formatter =
                DefaultLogFormatter::new(LogFilterCriteria::default(), stdout, options.clone());
            formatter.push_log(log_entry()).await.unwrap();
        }
        assert_eq!(
            serde_json::from_str::<LogEntry>(&buffers.into_stdout_str()).unwrap(),
            log_entry()
        );
    }

    fn emit_log(sender: &mut zx::Socket, msg: &str, timestamp: i64) -> Data<Logs> {
        let target_log = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(timestamp),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_message(msg)
        .build();

        sender
            .write(serde_json::to_string(&target_log).unwrap().as_bytes())
            .expect("failed to write target log");
        target_log
    }

    #[fuchsia::test]
    async fn test_default_formatter_discards_when_told_by_symbolizer() {
        let mut formatter = FakeFormatter::new();
        let (mut sender, receiver) = zx::Socket::create_stream();
        let mut target_log_0 = emit_log(&mut sender, "Hello world!", 0);
        emit_log(&mut sender, "Dropped world!", 1);
        let mut target_log_2 = emit_log(&mut sender, "Hello world!", 2);
        emit_log(&mut sender, "Dropped world!", 3);
        let mut target_log_4 = emit_log(&mut sender, "Hello world!", 4);
        drop(sender);
        // Drop every other log.
        let symbolizer = FakeSymbolizerCallback::new();
        *target_log_0.msg_mut().unwrap() = "symbolized log".into();
        *target_log_2.msg_mut().unwrap() = "symbolized log".into();
        *target_log_4.msg_mut().unwrap() = "symbolized log".into();
        dump_logs_from_socket(flex_client::socket_to_async(receiver), &mut formatter, &symbolizer)
            .await
            .unwrap();
        assert_eq!(
            formatter.logs,
            vec![
                LogEntry { data: LogData::TargetLog(target_log_0) },
                LogEntry { data: LogData::TargetLog(target_log_2) },
                LogEntry { data: LogData::TargetLog(target_log_4) }
            ],
        );
    }

    #[fuchsia::test]
    async fn test_symbolized_output() {
        let symbolizer = FakeFuchsiaSymbolizer;
        let buffers = TestBuffers::default();
        let output = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let mut formatter = DefaultLogFormatter::new(
            LogFilterCriteria::default(),
            output,
            LogFormatterOptions { ..Default::default() },
        );
        formatter.set_boot_timestamp(Timestamp::from_nanos(0));
        let target_log = LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            moniker: "ffx".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("ffx".into()),
            severity: Severity::Info,
        })
        .set_pid(1)
        .set_tid(2)
        .set_message("Hello world!")
        .build();
        let (sender, receiver) = zx::Socket::create_stream();
        sender
            .write(serde_json::to_string(&target_log).unwrap().as_bytes())
            .expect("failed to write target log");
        drop(sender);
        dump_logs_from_socket(flex_client::socket_to_async(receiver), &mut formatter, &symbolizer)
            .await
            .unwrap();
        assert_eq!(buffers.stdout.into_string(), "[00000.000000][1][2][ffx] INFO: Fuchsia\n");
    }

    #[test]
    fn test_device_or_local_timestamp_returns_none_if_now_is_passed() {
        assert_matches!(DeviceOrLocalTimestamp::new(Some(&parse_time("now").unwrap()), None), None);
    }

    struct BrokenPipeWriter;
    impl std::io::Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
        }
    }

    impl ToolIO for BrokenPipeWriter {
        type OutputItem = LogEntry;
        fn is_machine(&self) -> bool {
            false
        }

        fn stderr(&mut self) -> &mut dyn std::io::Write {
            self
        }

        fn item(&mut self, _value: &Self::OutputItem) -> writer::Result<()> {
            Err(writer::Error::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "broken pipe",
            )))
        }
    }

    #[fuchsia::test]
    async fn test_default_formatter_exits_on_broken_pipe() {
        let stdout = BrokenPipeWriter;
        let options = LogFormatterOptions::default();
        let mut formatter =
            DefaultLogFormatter::new(LogFilterCriteria::default(), stdout, options.clone());
        let result = formatter.push_log(log_entry()).await;
        assert_matches!(result, Ok(LogProcessingResult::Exit));
    }

    #[test]
    fn test_formatter_error_is_broken_pipe() {
        assert!(
            FormatterError::IO(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
                .is_broken_pipe()
        );
        assert!(!FormatterError::IO(std::io::Error::other("other")).is_broken_pipe());
        assert!(!FormatterError::Other(anyhow::anyhow!("other")).is_broken_pipe());
    }

    #[cfg(not(feature = "fdomain"))]
    #[fuchsia::test]
    async fn test_json_and_fxt_output_identical() {
        use diagnostics_data::ExtendedMoniker;
        use diagnostics_log_encoding::encode::{Encoder, EncoderOpts, ResizableBuffer};
        use diagnostics_log_encoding::{Argument, Header, LOG_CONTROL_BIT, MONIKER, Record, URL};
        use diagnostics_message::MonikerWithUrl;
        use flyweights::FlyStr;
        use zerocopy::{FromBytes, IntoBytes};

        let symbolizer = NoOpSymbolizer {};
        let options = LogFormatterOptions::default();

        let fn_encode = |record: Record<'_>, tag: u32| -> Vec<u8> {
            let mut encoder = Encoder::new(
                std::io::Cursor::new(ResizableBuffer::from(Vec::new())),
                EncoderOpts::default(),
            );
            encoder.write_record(record).unwrap();
            let mut bytes = encoder.take().into_inner().into_inner();
            let mut header = Header::read_from_bytes(&bytes[0..8]).unwrap();
            header.set_tag(tag);
            bytes[0..8].copy_from_slice(header.as_bytes());
            bytes
        };

        let manifest_bytes = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(0),
                severity: 0x30, // INFO
                arguments: vec![
                    Argument::other(MONIKER, "core/foo"),
                    Argument::other(URL, "fuchsia-pkg://foo"),
                ],
            },
            1 | LOG_CONTROL_BIT,
        );

        let log_bytes1 = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123456),
                severity: 0x30, // INFO
                arguments: vec![
                    Argument::pid(zx::Koid::from_raw(1000)),
                    Argument::tid(zx::Koid::from_raw(2000)),
                    Argument::tag("my_tag"),
                    Argument::message("Hello identical world!"),
                ],
            },
            1,
        );

        let log_bytes2 = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123457),
                severity: 0x40, // WARN
                arguments: vec![
                    Argument::file("src/main.rs"),
                    Argument::line(42),
                    Argument::dropped(5),
                    Argument::message("Warning with source location and dropped count"),
                ],
            },
            1,
        );

        let log_bytes3_signed = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123458),
                severity: 0x50, // ERROR
                arguments: vec![
                    Argument::message("Error with custom signed int"),
                    Argument::new("signed_val", -12345i64),
                ],
            },
            1,
        );

        let log_bytes3_unsigned = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123458),
                severity: 0x50, // ERROR
                arguments: vec![
                    Argument::message("Error with custom unsigned int"),
                    Argument::new("unsigned_val", 67890u64),
                ],
            },
            1,
        );

        let log_bytes3_bool = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123458),
                severity: 0x50, // ERROR
                arguments: vec![
                    Argument::message("Error with custom boolean"),
                    Argument::new("bool_val", true),
                ],
            },
            1,
        );

        let log_bytes3_str = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123458),
                severity: 0x50, // ERROR
                arguments: vec![
                    Argument::message("Error with custom string"),
                    Argument::new("string_val", "custom string"),
                ],
            },
            1,
        );

        let log_bytes4_pi = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123459),
                severity: 0x60, // FATAL
                arguments: vec![
                    Argument::message("Fatal with float pi"),
                    Argument::new("float_pi", std::f64::consts::PI),
                ],
            },
            1,
        );

        let log_bytes4_zero = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123459),
                severity: 0x60, // FATAL
                arguments: vec![
                    Argument::message("Fatal with float zero"),
                    Argument::new("float_zero", 0.0f64),
                ],
            },
            1,
        );

        let log_bytes4_large = fn_encode(
            Record {
                timestamp: zx::BootInstant::from_nanos(123459),
                severity: 0x60, // FATAL
                arguments: vec![
                    Argument::message("Fatal with float large"),
                    Argument::new("float_large", 123456.789f64),
                ],
            },
            1,
        );

        let all_records = [
            &log_bytes1,
            &log_bytes2,
            &log_bytes3_signed,
            &log_bytes3_unsigned,
            &log_bytes3_bool,
            &log_bytes3_str,
            &log_bytes4_pi,
            &log_bytes4_zero,
            &log_bytes4_large,
        ];

        // 1. JSON setup and execution using converted FXT messages
        let buffers_json = TestBuffers::default();
        let stdout_json = JsonWriter::<LogEntry>::new_test(None, &buffers_json);
        let mut formatter_json =
            DefaultLogFormatter::new(LogFilterCriteria::default(), stdout_json, options.clone());
        formatter_json.set_boot_timestamp(Timestamp::from_nanos(0));

        let source = MonikerWithUrl {
            moniker: ExtendedMoniker::parse_str("core/foo").unwrap(),
            url: FlyStr::new("fuchsia-pkg://foo"),
        };

        let (sender_json, receiver_json) = zx::Socket::create_stream();
        let mut json_stream_bytes = Vec::new();
        for record_bytes in &all_records {
            let target_log =
                diagnostics_message::from_structured(source.clone(), record_bytes).unwrap();
            serde_json::to_writer(&mut json_stream_bytes, &target_log).unwrap();
            json_stream_bytes.push(b'\n');
        }
        sender_json.write(&json_stream_bytes).expect("failed to write target logs");
        drop(sender_json);

        super::dump_logs_from_socket(
            flex_client::socket_to_async(receiver_json),
            &mut formatter_json,
            &symbolizer,
            false,
        )
        .await
        .unwrap();

        // 2. FXT setup and execution
        let buffers_fxt = TestBuffers::default();
        let stdout_fxt = JsonWriter::<LogEntry>::new_test(None, &buffers_fxt);
        let mut formatter_fxt =
            DefaultLogFormatter::new(LogFilterCriteria::default(), stdout_fxt, options.clone());
        formatter_fxt.set_boot_timestamp(Timestamp::from_nanos(0));

        let (sender_fxt, receiver_fxt) = zx::Socket::create_stream();
        sender_fxt.write(&manifest_bytes).unwrap();
        for record_bytes in &all_records {
            sender_fxt.write(record_bytes).unwrap();
        }
        drop(sender_fxt);

        super::dump_fxt_logs_from_socket(
            flex_client::socket_to_async(receiver_fxt),
            &mut formatter_fxt,
            &symbolizer,
            false,
        )
        .await
        .unwrap();

        // 3. Verify identity
        assert_eq!(buffers_json.stdout.into_string(), buffers_fxt.stdout.into_string());
    }

    #[fuchsia::test]
    async fn test_default_formatter_tail_limit() {
        let buffers = TestBuffers::default();
        let stdout = JsonWriter::<LogEntry>::new_test(None, &buffers);
        let options = LogFormatterOptions { tail: Some(2), ..Default::default() };
        let mut formatter = DefaultLogFormatter::new(LogFilterCriteria::default(), stdout, options);

        let entry1 = LogEntry {
            data: LogData::TargetLog(
                logs_data_builder().add_tag("tag1").set_message("msg1").build(),
            ),
        };
        let entry2 = LogEntry {
            data: LogData::TargetLog(
                logs_data_builder().add_tag("tag1").set_message("msg2").build(),
            ),
        };
        let entry3 = LogEntry {
            data: LogData::TargetLog(
                logs_data_builder().add_tag("tag1").set_message("msg3").build(),
            ),
        };

        formatter.push_log(entry1).await.unwrap();
        formatter.push_log(entry2).await.unwrap();
        formatter.push_log(entry3).await.unwrap();

        // Before flushing, nothing should be outputted because tail buffering delays output
        let stdout_snapshot = buffers.stdout.clone().into_string();
        assert_eq!(stdout_snapshot, "");

        formatter.flush().await.unwrap();

        // After flushing, only the last 2 entries should be outputted
        let output = buffers.into_stdout_str();
        assert!(!output.contains("msg1"));
        assert!(output.contains("msg2"));
        assert!(output.contains("msg3"));
    }
}
