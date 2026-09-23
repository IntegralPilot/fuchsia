// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::MessageError;
use byteorder::{ByteOrder, LittleEndian};
use diagnostics_data::{
    BuilderArgs, Data, ExtendedMoniker, Logs, LogsData, LogsDataBuilder, LogsField, LogsProperty,
    Severity, Timestamp,
};
use diagnostics_log_encoding::{
    ARCHIVIST_URL, Argument, Header, LOG_CONTROL_BIT, MONIKER, ROLLED_OUT, Record, URL, Value, zx,
};
use flyweights::FlyStr;
use libc::{c_char, c_int};
use moniker::Moniker;
use std::collections::HashMap;
use std::{mem, str};

#[cfg(fuchsia_api_level_at_least = "HEAD")]
use fidl_fuchsia_diagnostics as fdiagnostics;

mod constants;
pub mod error;
pub mod ffi;
pub use constants::*;

#[cfg(test)]
mod test;

#[derive(Clone)]
pub struct MonikerWithUrl {
    pub moniker: ExtendedMoniker,
    pub url: FlyStr,
}

/// Transforms the given legacy log message (already parsed) into a `LogsData` containing the
/// given identity information.
pub fn from_logger(source: MonikerWithUrl, msg: LoggerMessage) -> LogsData {
    let (raw_severity, severity) = Severity::parse_exact(msg.raw_severity);
    let mut builder = LogsDataBuilder::new(BuilderArgs {
        timestamp: Timestamp::from_nanos(msg.timestamp.into_nanos()),
        component_url: Some(source.url),
        moniker: source.moniker,
        severity,
    })
    .set_pid(msg.pid)
    .set_tid(msg.tid)
    .set_dropped(msg.dropped_logs)
    .set_message(msg.message);
    if let Some(raw_severity) = raw_severity {
        builder = builder.set_raw_severity(raw_severity);
    }
    for tag in &msg.tags {
        builder = builder.add_tag(tag.as_ref());
    }
    builder.build()
}

#[derive(Clone)]
pub struct ExtendedMetadata {
    pub moniker: ExtendedMoniker,
    pub url: FlyStr,
    pub rolled_out_logs: u64,
}

#[cfg(fuchsia_api_level_less_than = "HEAD")]
fn parse_archivist_args<'a>(
    builder: LogsDataBuilder,
    _input: &'a Record<'a>,
) -> Result<(LogsDataBuilder, usize), MessageError> {
    Ok((builder, 0))
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
fn parse_archivist_args<'a>(
    mut builder: LogsDataBuilder,
    input: &'a Record<'a>,
) -> Result<(LogsDataBuilder, usize), MessageError> {
    let mut archivist_argument_count = 0;
    for argument in input.arguments.iter().rev() {
        // If Archivist records are expected, they should always be at the end.
        // If we see a non-archivist record, we can stop looking.
        match argument {
            Argument::Other { name, value } => {
                if name == fdiagnostics::COMPONENT_URL_ARG_NAME {
                    if let Value::Text(url) = value {
                        builder = builder.set_url(Some(FlyStr::new(url.as_ref())));
                        archivist_argument_count += 1;
                        continue;
                    }
                } else if name == fdiagnostics::MONIKER_ARG_NAME {
                    if let Value::Text(moniker) = value {
                        builder = builder.set_moniker(ExtendedMoniker::parse_str(moniker)?);
                        archivist_argument_count += 1;
                        continue;
                    }
                } else if name == fdiagnostics::ROLLED_OUT_ARG_NAME
                    && let Value::UnsignedInt(count) = value
                {
                    builder = builder.set_rolled_out(*count);
                    archivist_argument_count += 1;
                    continue;
                }
            }
            _ => break,
        }
    }
    Ok((builder, archivist_argument_count))
}

pub fn parse_logs_data<'a>(
    input: &'a Record<'a>,
    source: Option<ExtendedMetadata>,
    rolled_out: u64,
) -> Result<LogsData, MessageError> {
    let (raw_severity, severity) = Severity::parse_exact(input.severity);
    let has_attribution = source.is_some();

    let (maybe_moniker, maybe_url) =
        source.map(|value| (Some(value.moniker), Some(value.url))).unwrap_or((None, None));

    let mut builder = LogsDataBuilder::new(BuilderArgs {
        component_url: maybe_url,
        moniker: maybe_moniker.unwrap_or(ExtendedMoniker::ComponentInstance(
            Moniker::parse_str("placeholder").unwrap(),
        )),
        severity,
        timestamp: Timestamp::from_nanos(input.timestamp.into_nanos()),
    });

    if rolled_out > 0 {
        builder = builder.set_rolled_out(rolled_out);
    }

    if let Some(raw_severity) = raw_severity {
        builder = builder.set_raw_severity(raw_severity);
    }
    let archivist_argument_count = if has_attribution {
        0
    } else {
        let (new_builder, count) = parse_archivist_args(builder, input)?;
        builder = new_builder;
        count
    };

    for argument in input.arguments.iter().take(input.arguments.len() - archivist_argument_count) {
        match argument {
            Argument::Tag(tag) => {
                builder = builder.add_tag(tag.as_ref());
            }
            Argument::Pid(pid) => {
                builder = builder.set_pid(pid.raw_koid());
            }
            Argument::Tid(tid) => {
                builder = builder.set_tid(tid.raw_koid());
            }
            Argument::Dropped(dropped) => {
                builder = builder.set_dropped(*dropped);
            }
            Argument::File(file) => {
                builder = builder.set_file(file.as_ref());
            }
            Argument::Line(line) => {
                builder = builder.set_line(*line);
            }
            Argument::Message(msg) => {
                builder = builder.set_message(msg.as_ref());
            }
            Argument::Other { value, name } => {
                let name = LogsField::Other(name.to_string());
                builder = builder.add_key(match value {
                    Value::SignedInt(v) => LogsProperty::Int(name, *v),
                    Value::UnsignedInt(v) => LogsProperty::Uint(name, *v),
                    Value::Floating(v) => LogsProperty::Double(name, *v),
                    Value::Text(v) => LogsProperty::String(name, v.to_string()),
                    Value::Boolean(v) => LogsProperty::Bool(name, *v),
                })
            }
        }
    }

    Ok(builder.build())
}

/// A stateful parser that reconstructs fully attributed `LogsData` records from a stream of
/// FXT log packets.
///
/// # Background & Architecture
///
/// In the Fuchsia Trace Format (FXT) structured logging protocol, log attribution metadata is
/// separated from the actual log payload to optimize transmission overhead. Instead of repeating
/// the full moniker and component URL on every log record, the system transmits two distinct
/// types of records:
///
/// 1. **Manifest/Control Records**: Sent with the `LOG_CONTROL_BIT` set. These records map a
///    numeric base tag ID to component identity metadata (`ExtendedMoniker` and URL).
/// 2. **Legacy Log Records**: Contain the message content, severity, timestamp, and
///    arguments, along with a tag ID indicating which component produced the log.
///
/// # Stateful Parsing
///
/// `MessageParser` maintains an internal `tag_map` to track the active association between
/// numeric tag IDs and their component identity (`ExtendedMetadata`).
///
/// - When parsing a manifest record (`is_control == true`), `MessageParser` updates its state
///   mapping for the derived base tag. If the record also reports rolled out (dropped) logs, a
///   `LogsData` payload representing those dropped logs is returned. Otherwise, it registers
///   the attribution mapping and returns `Ok((None, remaining))`.
/// - When parsing a legacy log record (`is_control == false`), `MessageParser` resolves the
///   record's tag to retrieve the component's identity from the internal mapping, constructing
///   a fully attributed `LogsData` containing the correct component moniker and URL.
#[derive(Default)]
pub struct MessageParser {
    tag_map: HashMap<u32, ExtendedMetadata>,
}

pub trait MessageFormatter<'a> {
    type Result;

    fn format(
        &mut self,
        record: &Record<'a>,
        metadata: Option<ExtendedMetadata>,
    ) -> Result<Self::Result, MessageError>;
}

#[derive(Default)]
pub struct RustMessageFormatter;

impl<'a> MessageFormatter<'a> for RustMessageFormatter {
    type Result = Data<Logs>;

    fn format(
        &mut self,
        record: &Record<'a>,
        metadata: Option<ExtendedMetadata>,
    ) -> Result<Self::Result, MessageError> {
        let rolled_out = metadata.as_ref().map(|value| value.rolled_out_logs).unwrap_or(0);
        parse_logs_data(record, metadata, rolled_out)
    }
}

impl MessageParser {
    /// Parses the next log record from the given `bytes`.
    ///
    /// This function can handle both standard log records and "Archivist" manifest records.
    /// Archivist records update an internal map used to attribute subsequent log records.
    ///
    /// # Arguments
    ///
    /// * `bytes`: A byte slice containing one or more log records.
    ///
    /// # Returns
    ///
    /// A `Result` containing:
    /// * `Ok((Option<LogsData>, &[u8]))`: A tuple where the first element is `Some(LogsData)`
    ///   if a log message was parsed, or `None` if it was an Archivist manifest record. The
    ///   second element is the remaining slice of bytes after parsing the record.
    /// * `Err(MessageError)`: An error if parsing failed.
    pub fn parse_next<'a, F: MessageFormatter<'a>>(
        &mut self,
        bytes: &'a [u8],
        mut formatter: F,
    ) -> Result<(Option<F::Result>, &'a [u8]), MessageError> {
        if bytes.len() < 8 {
            return Err(MessageError::ShortRead { len: bytes.len() });
        }
        let header_bytes: [u8; 8] = bytes[0..8].try_into().unwrap();
        let header_val = u64::from_le_bytes(header_bytes);
        let header = Header(header_val);
        let tag = header.tag();
        let base_tag = tag & !LOG_CONTROL_BIT;
        let is_control = (tag & LOG_CONTROL_BIT) != 0;

        let (input, remaining) = diagnostics_log_encoding::parse::parse_record(bytes)?;

        if is_control {
            let mut moniker = None;
            let mut url = None;
            let mut rolled_out = None;
            for arg in &input.arguments {
                if arg.name() == MONIKER
                    && let Value::Text(v) = arg.value()
                {
                    moniker = Some(v);
                } else if arg.name() == URL
                    && let Value::Text(v) = arg.value()
                {
                    url = Some(v);
                } else if arg.name() == ROLLED_OUT
                    && let Value::UnsignedInt(v) = arg.value()
                {
                    rolled_out = Some(v);
                }
            }
            if let Some(count) = rolled_out {
                let metadata =
                    self.tag_map.get(&base_tag).cloned().unwrap_or_else(|| ExtendedMetadata {
                        moniker: diagnostics_data::ExtendedMoniker::ComponentInstance(
                            moniker::Moniker::parse_str("/UNKNOWN").unwrap(),
                        ),
                        url: flyweights::FlyStr::new(ARCHIVIST_URL),
                        rolled_out_logs: count,
                    });
                let data = formatter.format(&input, Some(metadata))?;
                return Ok((Some(data), remaining));
            }
            if let (Some(m), Some(u)) = (moniker, url)
                && let Ok(extended_moniker) = ExtendedMoniker::parse_str(&m)
            {
                self.tag_map.insert(
                    base_tag,
                    ExtendedMetadata {
                        moniker: extended_moniker,
                        url: FlyStr::new(u),
                        rolled_out_logs: 0,
                    },
                );
            }
            Ok((None, remaining))
        } else {
            let metadata = self.tag_map.get(&base_tag).cloned();
            let data = formatter.format(&input, metadata)?;
            Ok((Some(data), remaining))
        }
    }
}

/// Constructs a `LogsData` from the provided bytes, assuming the bytes
/// are a single FXT log record with a potentially extended metadata section.
/// [log encoding] https://fuchsia.dev/fuchsia-src/reference/platform-spec/diagnostics/logs-encoding
pub fn from_extended_record(bytes: &[u8]) -> Result<(LogsData, &[u8]), MessageError> {
    let (input, remaining) = diagnostics_log_encoding::parse::parse_record(bytes)?;
    let (source, new_remaining, rolled_out_logs) = if remaining.len() >= 16 {
        let moniker_len = u32::from_le_bytes(remaining[0..4].try_into().unwrap()) as usize;
        let component_url_len = u32::from_le_bytes(remaining[4..8].try_into().unwrap()) as usize;
        let rolled_out_logs = u64::from_le_bytes(remaining[8..16].try_into().unwrap());
        let mut offset = 16;

        // NOTE: In the extended metadata format, string fields (moniker and URL) are
        // 8-byte word aligned with trailing zero padding.
        // `(len + 7) & !7` rounds up `len` to the nearest multiple of 8.
        // This arithmetic is overflow-safe on 64-bit platforms because moniker_len and
        // component_url_len are originally 32-bit integers (u32).
        let moniker_padded_len = (moniker_len + 7) & !7;
        let component_url_padded_len = (component_url_len + 7) & !7;
        let moniker_padded_end = offset + moniker_padded_len;
        let url_padded_end = moniker_padded_end + component_url_padded_len;
        if url_padded_end > remaining.len() {
            return Err(MessageError::OutOfBounds);
        }
        let moniker = str::from_utf8(&remaining[offset..offset + moniker_len])?;
        offset += moniker_padded_len;
        let url = str::from_utf8(&remaining[offset..offset + component_url_len])?;
        offset += component_url_padded_len;
        (
            Some(ExtendedMetadata {
                moniker: ExtendedMoniker::parse_str(moniker)?,
                url: FlyStr::new(url),
                rolled_out_logs: 0,
            }),
            &remaining[offset..],
            rolled_out_logs,
        )
    } else {
        (None, remaining, 0)
    };
    let record = parse_logs_data(&input, source, rolled_out_logs)?;
    Ok((record, new_remaining))
}

/// Constructs a `LogsData` from the provided bytes, assuming the bytes
/// are in the format specified as in the [log encoding].
///
/// [log encoding] https://fuchsia.dev/fuchsia-src/development/logs/encodings
pub fn from_structured(source: MonikerWithUrl, bytes: &[u8]) -> Result<LogsData, MessageError> {
    let (input, _remaining) = diagnostics_log_encoding::parse::parse_record(bytes)?;
    let record = parse_logs_data(
        &input,
        Some(ExtendedMetadata { moniker: source.moniker, url: source.url, rolled_out_logs: 0 }),
        0,
    )?;
    Ok(record)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggerMessage {
    pub timestamp: zx::BootInstant,
    pub raw_severity: u8,
    pub pid: u64,
    pub tid: u64,
    pub size_bytes: usize,
    pub dropped_logs: u64,
    pub message: Box<str>,
    pub tags: Vec<Box<str>>,
}

/// Parse the provided buffer as if it implements the [logger/syslog wire format].
///
/// Note that this is distinct from the parsing we perform for the debuglog log, which also
/// takes a `&[u8]` and is why we don't implement this as `TryFrom`.
///
/// [logger/syslog wire format]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/zircon/system/ulib/syslog/include/lib/syslog/wire_format.h
impl TryFrom<&[u8]> for LoggerMessage {
    type Error = MessageError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() < MIN_PACKET_SIZE {
            return Err(MessageError::ShortRead { len: bytes.len() });
        }

        let terminator = bytes[bytes.len() - 1];
        if terminator != 0 {
            return Err(MessageError::NotNullTerminated { terminator });
        }

        let pid = LittleEndian::read_u64(&bytes[..8]);
        let tid = LittleEndian::read_u64(&bytes[8..16]);
        let timestamp = zx::BootInstant::from_nanos(LittleEndian::read_i64(&bytes[16..24]));

        let raw_severity = LittleEndian::read_i32(&bytes[24..28]);
        let raw_severity = if raw_severity > (u8::MAX as i32) {
            u8::MAX
        } else if raw_severity < 0 {
            0
        } else {
            u8::try_from(raw_severity).unwrap()
        };
        let dropped_logs = LittleEndian::read_u32(&bytes[28..METADATA_SIZE]) as u64;

        // start reading tags after the header
        let mut cursor = METADATA_SIZE;
        let mut tag_len = bytes[cursor] as usize;
        let mut tags = Vec::new();
        while tag_len != 0 {
            if tags.len() == MAX_TAGS {
                return Err(MessageError::TooManyTags);
            }

            if tag_len > MAX_TAG_LEN - 1 {
                return Err(MessageError::TagTooLong { index: tags.len(), len: tag_len });
            }

            if (cursor + tag_len + 1) > bytes.len() {
                return Err(MessageError::OutOfBounds);
            }

            let tag_start = cursor + 1;
            let tag_end = tag_start + tag_len;
            let tag = String::from_utf8_lossy(&bytes[tag_start..tag_end]);
            tags.push(tag.into());

            cursor = tag_end;
            tag_len = bytes[cursor] as usize;
        }

        let msg_start = cursor + 1;
        let mut msg_end = cursor + 1;
        while msg_end < bytes.len() {
            if bytes[msg_end] > 0 {
                msg_end += 1;
                continue;
            }
            let message = String::from_utf8_lossy(&bytes[msg_start..msg_end]).into_owned();
            let message_len = message.len();
            let result = LoggerMessage {
                timestamp,
                raw_severity,
                message: message.into_boxed_str(),
                pid,
                tid,
                dropped_logs,
                tags,
                size_bytes: cursor + message_len + 1,
            };
            return Ok(result);
        }

        Err(MessageError::OutOfBounds)
    }
}

#[allow(non_camel_case_types)]
pub type fx_log_severity_t = c_int;

#[repr(C)]
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
pub struct fx_log_metadata_t {
    pub pid: zx::sys::zx_koid_t,
    pub tid: zx::sys::zx_koid_t,
    pub time: zx::sys::zx_time_t,
    pub severity: fx_log_severity_t,
    pub dropped_logs: u32,
}

#[repr(C)]
#[derive(Clone)]
pub struct fx_log_packet_t {
    pub metadata: fx_log_metadata_t,
    // Contains concatenated tags and message and a null terminating character at
    // the end.
    // char(tag_len) + "tag1" + char(tag_len) + "tag2\0msg\0"
    pub data: [c_char; MAX_DATAGRAM_LEN - METADATA_SIZE],
}

impl Default for fx_log_packet_t {
    fn default() -> fx_log_packet_t {
        fx_log_packet_t {
            data: [0; MAX_DATAGRAM_LEN - METADATA_SIZE],
            metadata: Default::default(),
        }
    }
}

impl fx_log_packet_t {
    /// This struct has no padding bytes, but we can't use zerocopy because it needs const
    /// generics to support arrays this large.
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                (self as *const Self) as *const u8,
                mem::size_of::<fx_log_packet_t>(),
            )
        }
    }

    /// Fills data with a single value for defined region.
    pub fn fill_data(&mut self, region: std::ops::Range<usize>, with: c_char) {
        self.data[region].iter_mut().for_each(|c| *c = with);
    }

    /// Copies bytes to data at specifies offset.
    pub fn add_data<T: std::convert::TryInto<c_char> + Copy>(&mut self, offset: usize, bytes: &[T])
    where
        <T as std::convert::TryInto<c_char>>::Error: std::fmt::Debug,
    {
        self.data[offset..(offset + bytes.len())]
            .iter_mut()
            .enumerate()
            .for_each(|(i, x)| *x = bytes[i].try_into().unwrap());
    }
}
