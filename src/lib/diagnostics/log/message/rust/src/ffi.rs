// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::MessageError;
use crate::{ExtendedMetadata, MessageFormatter};
use bumpalo::Bump;
use bumpalo::collections::{String as BumpaloString, Vec as BumpaloVec};
use diagnostics_data::{ExtendedMoniker, Severity};
use diagnostics_log_encoding::zx::BootInstant;
use diagnostics_log_encoding::{Argument, Record, Value};
use flyweights::FlyStr;
use static_assertions::const_assert;
use std::marker::PhantomData;
use std::ops::Deref;
use std::str;

pub use crate::constants::*;

/// Array for FFI purposes between C++ and Rust.
/// If len is 0, ptr is allowed to be nullptr,
/// otherwise, ptr must be valid.
#[repr(C)]
pub struct CppArray<'a, T> {
    /// Number of elements in the array
    pub len: usize,
    /// Pointer to the first element in the array,
    /// may be null in the case of a 0 length array,
    /// but is not guaranteed to always be null of
    /// len is 0.
    pub ptr: *const T,

    phantom: PhantomData<&'a T>,
}

impl<T> Deref for CppArray<'_, T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        if self.len == 0 || self.ptr.is_null() {
            &[]
        } else {
            // SAFETY: The `CPPArray` is constructed from valid slices or arrays,
            // ensuring `self.ptr` points to `self.len` elements of type `T`.
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }
}

impl<T> Default for CppArray<'_, T> {
    fn default() -> Self {
        CppArray { len: 0, ptr: std::ptr::null(), phantom: PhantomData }
    }
}

impl<'a> From<&'a str> for CppString<'a> {
    fn from(value: &'a str) -> Self {
        Self { inner: CppArray { len: value.len(), ptr: value.as_ptr(), phantom: PhantomData } }
    }
}

/// Represents a UTF-8 encoded string for FFI purposes.
/// This is equivalent to a CppArray<u8> as it is
/// #[repr(transparent)], but with the additional
/// constraint that the contents of the array
/// is a valid UTF-8 string.
#[derive(Default)]
#[repr(transparent)]
pub struct CppString<'a> {
    inner: CppArray<'a, u8>,
}

impl Deref for CppString<'_> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        if self.inner.len == 0 || self.inner.ptr.is_null() {
            ""
        } else {
            // SAFETY: CppString is always constructed from valid UTF-8.
            unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    self.inner.ptr,
                    self.inner.len,
                ))
            }
        }
    }
}

impl std::fmt::Display for CppString<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&**self, f)
    }
}

impl<'a> From<Option<&'a str>> for CppString<'a> {
    fn from(value: Option<&'a str>) -> Self {
        value.map(|v| v.into()).unwrap_or_default()
    }
}

impl<'a, T> From<&'a [T]> for CppArray<'a, T> {
    fn from(value: &'a [T]) -> Self {
        CppArray { len: value.len(), ptr: value.as_ptr(), phantom: PhantomData }
    }
}

impl<'a> From<BumpaloString<'a>> for CppString<'a> {
    fn from(value: BumpaloString<'a>) -> Self {
        value.into_bump_str().into()
    }
}

/// Represents a value in a key-value pair for FFI purposes between C++ and Rust.
#[repr(C, u8)]
pub enum CppValue<'a> {
    SignedInt(i64),
    UnsignedInt(u64),
    Floating(f64),
    Boolean(bool),
    Text(CppString<'a>),
}

/// Represents a key-value pair for FFI purposes between C++ and Rust.
#[repr(C)]
pub struct CppKeyValue<'a> {
    pub key: CppString<'a>,
    pub value: CppValue<'a>,
}

/// Log message representation for FFI with C++.
///
/// # Lifetime and Borrowing
/// Strings within `LogMessage` (`tags`, `message`, and string keys/values in `kvps`)
/// borrow directly from the incoming encoded message buffer (`bytes`) when possible,
/// or from the provided arena allocator (`Bump`).
/// Consequently, the incoming message buffer MUST remain valid and unmodified
/// for at least as long as the `LogMessage` is in use.
#[repr(C)]
pub struct LogMessage<'a> {
    /// Severity of a log message.
    pub severity: u8,
    /// Tags in a log message, guaranteed to be non-null.
    pub tags: CppArray<'a, CppString<'a>>,
    /// Process ID from a LogMessage, or 0 if unknown
    pub pid: u64,
    /// Thread ID from a LogMessage, or 0 if unknown
    pub tid: u64,
    /// Number of dropped log messages.
    pub dropped: u64,
    /// The UTF-encoded log message, guaranteed to be valid UTF-8.
    pub message: CppString<'a>,
    /// Source file where the log was emitted, if any.
    pub file: CppString<'a>,
    /// Source line where the log was emitted, if any.
    pub line: u64,
    /// Last segment of the moniker, used as a default log tag if needed.
    pub moniker_tag: CppString<'a>,
    /// Key-value pairs in a log message.
    pub kvps: CppArray<'a, CppKeyValue<'a>>,
    /// Timestamp on the boot timeline of the log message,
    /// in nanoseconds.
    pub timestamp: i64,
}

// These are allocated using the Bumpalo allocator.
const_assert!(!std::mem::needs_drop::<LogMessage<'_>>());

pub struct CPPLogMessageBuilder<'a> {
    severity: u8,
    tags: BumpaloVec<'a, CppString<'a>>,
    pid: Option<u64>,
    tid: Option<u64>,
    dropped: u64,
    file: Option<CppString<'a>>,
    line: Option<u64>,
    moniker: Option<BumpaloString<'a>>,
    message: Option<CppString<'a>>,
    timestamp: i64,
    kvps: BumpaloVec<'a, CppKeyValue<'a>>,
    allocator: &'a Bump,
}

impl<'a> CPPLogMessageBuilder<'a> {
    fn set_raw_severity(mut self, raw_severity: u8) -> Self {
        self.severity = raw_severity;
        self
    }

    fn cow_to_cpp_string(&self, cow: &std::borrow::Cow<'a, str>) -> CppString<'a> {
        match cow {
            std::borrow::Cow::Borrowed(s) => (*s).into(),
            std::borrow::Cow::Owned(s) => {
                BumpaloString::from_str_in(s, self.allocator).into_bump_str().into()
            }
        }
    }

    fn add_tag(mut self, tag: &std::borrow::Cow<'a, str>) -> Self {
        let s = self.cow_to_cpp_string(tag);
        self.tags.push(s);
        self
    }

    fn set_pid(mut self, pid: u64) -> Self {
        self.pid = Some(pid);
        self
    }

    fn set_tid(mut self, tid: u64) -> Self {
        self.tid = Some(tid);
        self
    }

    fn set_dropped(mut self, dropped: u64) -> Self {
        self.dropped = dropped;
        self
    }

    fn set_file(mut self, file: &std::borrow::Cow<'a, str>) -> Self {
        self.file = Some(self.cow_to_cpp_string(file));
        self
    }

    fn set_line(mut self, line: u64) -> Self {
        self.line = Some(line);
        self
    }

    fn set_message(mut self, msg: &std::borrow::Cow<'a, str>) -> Self {
        self.message = Some(self.cow_to_cpp_string(msg));
        self
    }

    fn add_kvp(mut self, kvp: &Argument<'a>) -> Self {
        let key: CppString<'a> = match kvp {
            Argument::Other { name, .. } => self.cow_to_cpp_string(name),
            other => {
                BumpaloString::from_str_in(other.name(), self.allocator).into_bump_str().into()
            }
        };
        let value = match kvp {
            Argument::Pid(pid) => CppValue::UnsignedInt(pid.raw_koid()),
            Argument::Tid(tid) => CppValue::UnsignedInt(tid.raw_koid()),
            Argument::Tag(tag) => CppValue::Text(self.cow_to_cpp_string(tag)),
            Argument::Dropped(dropped) => CppValue::UnsignedInt(*dropped),
            Argument::File(file) => CppValue::Text(self.cow_to_cpp_string(file)),
            Argument::Line(line) => CppValue::UnsignedInt(*line),
            Argument::Message(msg) => CppValue::Text(self.cow_to_cpp_string(msg)),
            Argument::Other { value, .. } => match value {
                Value::Text(t) => CppValue::Text(self.cow_to_cpp_string(t)),
                Value::SignedInt(v) => CppValue::SignedInt(*v),
                Value::UnsignedInt(v) => CppValue::UnsignedInt(*v),
                Value::Floating(v) => CppValue::Floating(*v),
                Value::Boolean(v) => CppValue::Boolean(*v),
            },
        };
        self.kvps.push(CppKeyValue { key, value });
        self
    }

    fn set_moniker(mut self, value: &str) -> Self {
        self.moniker = Some(BumpaloString::from_str_in(value, self.allocator));
        self
    }

    pub fn build(mut self) -> &'a mut LogMessage<'a> {
        let allocator = self.allocator;

        let file: CppString<'a> = self.file.unwrap_or_default();
        let line: u64 = self.line.unwrap_or(0);
        let message: CppString<'a> = self.message.unwrap_or_default();

        let moniker_tag: CppString<'a> = match &self.moniker {
            Some(moniker) => match moniker.split('/').next_back() {
                Some(name) => BumpaloString::from_str_in(name, allocator).into_bump_str().into(),
                None => CppString::default(),
            },
            None => CppString::default(),
        };

        let tags: &[_] = allocator.alloc_slice_fill_iter(self.tags.drain(..));
        let kvps: &[_] = allocator.alloc_slice_fill_iter(self.kvps.drain(..));

        allocator.alloc(LogMessage {
            severity: self.severity,
            dropped: self.dropped,
            tags: tags.into(),
            pid: self.pid.unwrap_or(0),
            tid: self.tid.unwrap_or(0),
            message,
            file,
            line,
            moniker_tag,
            kvps: kvps.into(),
            timestamp: self.timestamp,
        })
    }
}

struct CPPLogMessageBuilderBuilder<'a>(&'a Bump);

impl<'a> CPPLogMessageBuilderBuilder<'a> {
    fn configure(
        self,
        _component_url: Option<FlyStr>,
        moniker: Option<ExtendedMoniker>,
        severity: Severity,
        timestamp: BootInstant,
    ) -> Result<CPPLogMessageBuilder<'a>, MessageError> {
        Ok(CPPLogMessageBuilder {
            severity: severity as u8,
            tags: BumpaloVec::new_in(self.0),
            pid: None,
            tid: None,
            dropped: 0,
            file: None,
            timestamp: timestamp.into_nanos(),
            line: None,
            allocator: self.0,
            kvps: BumpaloVec::new_in(self.0),
            moniker: moniker.map(|value| bumpalo::format!(in self.0,"{}", value)),
            message: None,
        })
    }
}

pub fn build_logs_data<'a>(
    input: &Record<'a>,
    source: Option<ExtendedMetadata>,
    allocator: &'a Bump,
) -> Result<&'a mut LogMessage<'a>, MessageError> {
    let builder = CPPLogMessageBuilderBuilder(allocator);
    let (raw_severity, severity) = Severity::parse_exact(input.severity);
    let (maybe_moniker, maybe_url, _) = source
        .map(|value| (Some(value.moniker), Some(value.url), Some(value.rolled_out_logs)))
        .unwrap_or((None, None, None));
    let mut builder =
        builder.configure(maybe_url.map(FlyStr::new), None, severity, input.timestamp)?;
    if let Some(moniker) = maybe_moniker {
        builder = builder.set_moniker(moniker.as_ref());
    }
    if let Some(raw_severity) = raw_severity {
        builder = builder.set_raw_severity(raw_severity);
    }

    for argument in input.arguments.iter() {
        match argument {
            Argument::Tag(tag) => {
                builder = builder.add_tag(tag);
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
                builder = builder.set_file(file);
            }
            Argument::Line(line) => {
                builder = builder.set_line(*line);
            }
            Argument::Message(msg) => {
                builder = builder.set_message(msg);
            }
            Argument::Other { .. } => builder = builder.add_kvp(argument),
        }
    }

    Ok(builder.build())
}

/// Constructs a `CPPLogsMessage` from the provided bytes, assuming the bytes
/// are in the format specified as in the [log encoding], and come from
/// an Archivist LogStream with moniker, URL, and dropped logs output enabled.
/// [log encoding] https://fuchsia.dev/fuchsia-src/development/logs/encodings
///
/// # Lifetime and Borrowing
/// Strings within the returned `LogMessage` borrow directly from `bytes`.
/// Therefore, `bytes` must remain valid and unmodified for the lifetime `'a`
/// of the returned `LogMessage`.
pub fn ffi_from_extended_record<'a>(
    bytes: &'a [u8],
    allocator: &'a Bump,
) -> Result<(&'a mut LogMessage<'a>, &'a [u8]), MessageError> {
    let (input, remaining) = diagnostics_log_encoding::parse::parse_record(bytes)?;
    let (source, new_remaining) = if remaining.len() >= 16 {
        let moniker_len = u32::from_le_bytes(remaining[0..4].try_into().unwrap()) as usize;
        let component_url_len = u32::from_le_bytes(remaining[4..8].try_into().unwrap()) as usize;
        let rolled_out_logs = u64::from_le_bytes(remaining[8..16].try_into().unwrap());
        let mut offset: usize = 16;

        // NOTE: This addition is safe as all platforms Fuchsia supports are 64-bit,
        // so usize will never overflow.
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
                url: url.into(),
                rolled_out_logs,
            }),
            &remaining[offset..],
        )
    } else {
        (None, remaining)
    };
    let record = build_logs_data(&input, source, allocator)?;
    Ok((record, new_remaining))
}

pub struct CPPMessageFormatter<'a>(pub &'a Bump);
impl<'a> MessageFormatter<'a> for &CPPMessageFormatter<'a> {
    type Result = &'a mut LogMessage<'a>;

    fn format(
        &mut self,
        record: &Record<'a>,
        metadata: Option<ExtendedMetadata>,
    ) -> Result<Self::Result, MessageError> {
        build_logs_data(record, metadata, self.0)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::MessageParser;
    use bumpalo::Bump;
    use diagnostics_log_encoding::encode::{Encoder, EncoderOpts};
    use diagnostics_log_encoding::zx::BootInstant;
    use diagnostics_log_encoding::{Argument, Header, LOG_CONTROL_BIT, Record};
    use std::io::Cursor;

    fn overwrite_header_tag(bytes: &mut [u8], tag: u32) {
        if bytes.len() >= 8 {
            let mut header = Header(u64::from_le_bytes(bytes[0..8].try_into().unwrap()));
            header.set_tag(tag);
            bytes[0..8].copy_from_slice(&header.0.to_le_bytes());
        }
    }

    #[fuchsia::test]
    fn test_short_read() {
        let mut parser = MessageParser::default();
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);
        let bytes = vec![0u8; 7];
        let res = parser.parse_next(&bytes, &formatter);
        assert!(matches!(res, Err(MessageError::ShortRead { len: 7 })));
    }

    #[fuchsia::test]
    fn test_normal_parsing() {
        let mut parser = MessageParser::default();
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);

        let record = Record {
            timestamp: BootInstant::from_nanos(72),
            severity: 0x30,
            arguments: vec![Argument::message("hello world")],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();

        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        let res = parser.parse_next(&bytes, &formatter).unwrap();
        assert!(res.0.is_some());
        let log_message = &res.0.unwrap();
        assert_eq!(&*log_message.message, "hello world");
        assert_eq!(log_message.timestamp, 72);
        assert_eq!(log_message.severity, 0x30);
    }

    #[fuchsia::test]
    fn test_escaping_in_kvp() {
        let mut parser = MessageParser::default();
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);

        let record = Record {
            timestamp: BootInstant::from_nanos(72),
            severity: 0x30,
            arguments: vec![
                Argument::message("hello world"),
                Argument::new("key", r#"val"with\escapes"#),
            ],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();

        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        let res = parser.parse_next(&bytes, &formatter).unwrap();
        assert!(res.0.is_some());
        let log_message = &res.0.unwrap();
        assert_eq!(&*log_message.message, "hello world");
        assert_eq!(log_message.kvps.len, 1);
        assert_eq!(&*log_message.kvps[0].key, "key");
        assert!(matches!(
            &log_message.kvps[0].value,
            CppValue::Text(val) if &**val == r#"val"with\escapes"#
        ));
    }

    #[fuchsia::test]
    fn test_out_of_bounds_extended_record() {
        let allocator = Bump::new();

        let record = Record {
            timestamp: BootInstant::from_nanos(72),
            severity: 0x30,
            arguments: vec![Argument::message("hello world")],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();
        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        // Append a corrupt moniker_len or component_url_len in the remaining slice.
        let extended_metadata_suffix = [
            0xE8, 0x03, 0x00, 0x00, // moniker_len = 1000
            0xE8, 0x03, 0x00, 0x00, // component_url_len = 1000
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // rolled_out_logs = 0
        ];
        bytes.extend_from_slice(&extended_metadata_suffix);

        let res = ffi_from_extended_record(&bytes, &allocator);
        assert!(matches!(res, Err(MessageError::OutOfBounds)));
    }

    #[fuchsia::test]
    fn test_control_message_tags() {
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);
        let mut parser = MessageParser::default();

        let tag_id = 0;

        let control_record = Record {
            timestamp: BootInstant::from_nanos(72),
            severity: 0x30,
            arguments: vec![
                Argument::new("moniker", "test/moniker"),
                Argument::new("url", "fuchsia-pkg://test"),
            ],
        };

        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(control_record).unwrap();

        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        overwrite_header_tag(&mut bytes, LOG_CONTROL_BIT);

        let (log, _) = parser.parse_next(&bytes, &formatter).unwrap();
        assert!(log.is_none());

        let tag_data = parser.tag_map.get(&tag_id).unwrap();
        assert_eq!(tag_data.moniker, ExtendedMoniker::parse_str("test/moniker").unwrap());
        assert_eq!(tag_data.url, "fuchsia-pkg://test");

        let rolled_out_record = Record {
            timestamp: BootInstant::from_nanos(73),
            severity: 0x30,
            arguments: vec![Argument::new("rolled_out", 5u64)],
        };

        let mut buffer2 = Cursor::new(vec![0u8; 1024]);
        let mut encoder2 = Encoder::new(&mut buffer2, EncoderOpts::default());
        encoder2.write_record(rolled_out_record).unwrap();

        let len2 = buffer2.position() as usize;
        let mut bytes2 = buffer2.into_inner();
        bytes2.truncate(len2);

        overwrite_header_tag(&mut bytes2, LOG_CONTROL_BIT);

        let (log2, _) = parser.parse_next(&bytes2, &formatter).unwrap();
        assert!(log2.is_some());

        let log2_msg = log2.unwrap();
        assert_eq!(&*log2_msg.message, "");
        assert_eq!(log2_msg.kvps.len, 1);
        assert_eq!(&*log2_msg.kvps[0].key, "rolled_out");
        assert!(matches!(log2_msg.kvps[0].value, CppValue::UnsignedInt(5)));
        let tag_data2 = parser.tag_map.get(&tag_id).unwrap();
        assert_eq!(tag_data2.moniker, ExtendedMoniker::parse_str("test/moniker").unwrap());

        let normal_record = Record {
            timestamp: BootInstant::from_nanos(74),
            severity: 0x30,
            arguments: vec![Argument::message("some log with tag")],
        };

        let mut buffer3 = Cursor::new(vec![0u8; 1024]);
        let mut encoder3 = Encoder::new(&mut buffer3, EncoderOpts::default());
        encoder3.write_record(normal_record).unwrap();

        let len3 = buffer3.position() as usize;
        let mut bytes3 = buffer3.into_inner();
        bytes3.truncate(len3);

        overwrite_header_tag(&mut bytes3, tag_id);

        let (log3, _) = parser.parse_next(&bytes3, &formatter).unwrap();

        let log_msg3 = &log3.unwrap();
        assert_eq!(&*log_msg3.message, "some log with tag");
        assert_eq!(log_msg3.tags.len(), 0);
        assert_eq!(&*log_msg3.moniker_tag, "moniker");
    }

    #[fuchsia::test]
    fn test_message_with_kvps() {
        let mut parser = MessageParser::default();
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);

        let record = Record {
            timestamp: BootInstant::from_nanos(100),
            severity: 0x10,
            arguments: vec![
                Argument::message("A message"),
                Argument::new("key1", "value1"),
                Argument::new("key2", 123u64),
            ],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();

        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        let res = parser.parse_next(&bytes, &formatter).unwrap();
        assert!(res.0.is_some());
        let log_message = res.0.unwrap();
        assert_eq!(&*log_message.message, "A message");
        assert_eq!(log_message.kvps.len, 2);
        assert_eq!(&*log_message.kvps[0].key, "key1");
        assert!(matches!(
            &log_message.kvps[0].value,
            CppValue::Text(val) if &**val == "value1"
        ));
        assert_eq!(&*log_message.kvps[1].key, "key2");
        assert!(matches!(&log_message.kvps[1].value, CppValue::UnsignedInt(123)));
    }

    #[fuchsia::test]
    fn test_file_line_message_with_kvps() {
        let mut parser = MessageParser::default();
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);

        let record = Record {
            timestamp: BootInstant::from_nanos(100),
            severity: 0x10,
            arguments: vec![
                Argument::file("src/file.rs"),
                Argument::line(42),
                Argument::message("Another message"),
                Argument::new("temp", 30.5),
                Argument::new("valid", true),
            ],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();

        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        let res = parser.parse_next(&bytes, &formatter).unwrap();
        assert!(res.0.is_some());
        let log_message = res.0.unwrap();
        assert_eq!(&*log_message.file, "src/file.rs");
        assert_eq!(log_message.line, 42);
        assert_eq!(&*log_message.message, "Another message");
        assert_eq!(log_message.kvps.len, 2);
        assert_eq!(&*log_message.kvps[0].key, "temp");
        assert!(matches!(
            &log_message.kvps[0].value,
            CppValue::Floating(val) if *val == 30.5
        ));
        assert_eq!(&*log_message.kvps[1].key, "valid");
        assert!(matches!(&log_message.kvps[1].value, CppValue::Boolean(true)));
    }

    #[fuchsia::test]
    fn test_only_kvps() {
        let mut parser = MessageParser::default();
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);

        let record = Record {
            timestamp: BootInstant::from_nanos(100),
            severity: 0x10,
            arguments: vec![Argument::new("status", "ok"), Argument::new("code", 200i64)],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();

        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        let res = parser.parse_next(&bytes, &formatter).unwrap();
        assert!(res.0.is_some());
        let log_message = res.0.unwrap();
        assert_eq!(&*log_message.message, "");
        assert_eq!(log_message.kvps.len, 2);
        assert_eq!(&*log_message.kvps[0].key, "status");
        assert!(matches!(
            &log_message.kvps[0].value,
            CppValue::Text(val) if &**val == "ok"
        ));
        assert_eq!(&*log_message.kvps[1].key, "code");
        assert!(matches!(&log_message.kvps[1].value, CppValue::SignedInt(200)));
    }

    #[fuchsia::test]
    fn test_zero_copy_string_borrowing() {
        let mut parser = MessageParser::default();
        let allocator = Bump::new();
        let formatter = CPPMessageFormatter(&allocator);

        let record = Record {
            timestamp: BootInstant::from_nanos(100),
            severity: 0x30,
            arguments: vec![
                Argument::File("src/lib.rs".into()),
                Argument::Line(10),
                Argument::Message("hello zero-copy".into()),
            ],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();

        let len = buffer.position() as usize;
        let mut bytes = buffer.into_inner();
        bytes.truncate(len);

        let (log_message, _) = parser.parse_next(&bytes, &formatter).unwrap();
        let log_message = log_message.unwrap();

        let bytes_start = bytes.as_ptr() as usize;
        let bytes_end = bytes_start + bytes.len();

        let msg_ptr = log_message.message.inner.ptr as usize;
        assert!(
            bytes_start <= msg_ptr && msg_ptr < bytes_end,
            "message pointer must borrow directly from incoming record buffer"
        );

        let file_ptr = log_message.file.inner.ptr as usize;
        assert!(
            bytes_start <= file_ptr && file_ptr < bytes_end,
            "file pointer must borrow directly from incoming record buffer"
        );
    }

    #[fuchsia::test]
    fn test_moniker_tag() {
        let allocator = Bump::new();
        let builder = CPPLogMessageBuilder {
            severity: 0x30,
            tags: BumpaloVec::new_in(&allocator),
            pid: None,
            tid: None,
            dropped: 0,
            file: None,
            line: None,
            moniker: Some(BumpaloString::from_str_in("core/foo", &allocator)),
            message: None,
            timestamp: 0,
            kvps: BumpaloVec::new_in(&allocator),
            allocator: &allocator,
        };
        let msg = builder.build();
        assert_eq!(&*msg.moniker_tag, "foo");
    }

    #[fuchsia::test]
    fn test_cpp_array_and_string_deref_handles_null_and_zero_len() {
        use std::ops::Deref;

        // Test CppArray deref handles null pointer or zero length
        let empty_array: CppArray<'_, u32> =
            CppArray { len: 0, ptr: std::ptr::null(), phantom: PhantomData };
        assert_eq!(empty_array.deref(), &[] as &[u32]);

        let zero_len_array: CppArray<'_, u32> =
            CppArray { len: 0, ptr: 0x1234 as *const u32, phantom: PhantomData };
        assert_eq!(zero_len_array.deref(), &[] as &[u32]);

        let null_ptr_array: CppArray<'_, u32> =
            CppArray { len: 5, ptr: std::ptr::null(), phantom: PhantomData };
        assert_eq!(null_ptr_array.deref(), &[] as &[u32]);

        // Test CppString deref handles null pointer or zero length
        let empty_string = CppString::default();
        assert_eq!(empty_string.deref(), "");

        let zero_len_string = CppString {
            inner: CppArray { len: 0, ptr: 0x1234 as *const u8, phantom: PhantomData },
        };
        assert_eq!(zero_len_string.deref(), "");

        let null_ptr_string =
            CppString { inner: CppArray { len: 5, ptr: std::ptr::null(), phantom: PhantomData } };
        assert_eq!(null_ptr_string.deref(), "");
    }
}
