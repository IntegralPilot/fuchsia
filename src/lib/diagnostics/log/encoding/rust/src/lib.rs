// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

//! This crate provides an implementation of Fuchsia Diagnostic Streams, often referred to as
//! "logs."

#![warn(missing_docs)]

use bitfield::bitfield;
use std::borrow::{Borrow, Cow};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

mod constants;
pub mod encode;
pub mod parse;

#[cfg(target_os = "fuchsia")]
pub use zx;

#[cfg(not(target_os = "fuchsia"))]
/// Implementation of the `zx` crate for host platforms.
pub mod zx {
    use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

    /// Mock `sys` types for host platforms.
    pub mod sys {
        #[allow(non_camel_case_types)]
        /// Raw koid type.
        pub type zx_koid_t = u64;

        #[allow(non_camel_case_types)]
        /// Raw time type.
        pub type zx_time_t = i64;
    }

    /// A timestamp from the boot clock.
    #[derive(
        Clone,
        Copy,
        Debug,
        Default,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
        FromBytes,
        IntoBytes,
        Immutable,
        KnownLayout,
    )]
    #[repr(transparent)]
    pub struct BootInstant(i64);

    impl BootInstant {
        /// A `BootInstant` representing timestamp 0.
        pub const ZERO: Self = Self(0);

        /// Return a strongly-typed `BootInstant` from a raw number of nanoseconds.
        pub const fn from_nanos(nanos: i64) -> Self {
            Self(nanos)
        }

        /// Returns the number of nanoseconds contained by this `BootInstant`.
        pub const fn into_nanos(self) -> i64 {
            self.0
        }

        /// Get the current boot time.
        pub fn get() -> Self {
            let nanos = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_nanos() as i64,
                Err(e) => -(e.duration().as_nanos() as i64),
            };
            Self::from_nanos(nanos)
        }
    }

    /// The unique id assigned by kernel to the object referenced by a handle.
    #[derive(
        Clone,
        Copy,
        Debug,
        Default,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
        FromBytes,
        IntoBytes,
        Immutable,
        KnownLayout,
    )]
    #[repr(transparent)]
    pub struct Koid(u64);

    impl Koid {
        /// Creates a `Koid` from a raw u64.
        pub const fn from_raw(raw: u64) -> Self {
            Self(raw)
        }

        /// Returns the raw u64 koid value.
        pub const fn raw_koid(&self) -> u64 {
            self.0
        }
    }
}

pub use constants::*;

/// A raw severity.
pub type RawSeverity = u8;

/// A log record.
#[derive(Clone, Debug, PartialEq)]
pub struct Record<'a> {
    /// Time at which the log was emitted.
    pub timestamp: zx::BootInstant,
    /// The severity of the log.
    pub severity: RawSeverity,
    /// Arguments associated with the log.
    pub arguments: Vec<Argument<'a>>,
}

impl Record<'_> {
    /// Consumes the current value and returns one in the static lifetime.
    pub fn into_owned(self) -> Record<'static> {
        Record {
            timestamp: self.timestamp,
            severity: self.severity,
            arguments: self.arguments.into_iter().map(|arg| arg.into_owned()).collect(),
        }
    }
}

/// An argument of the log record identified by a name and with an associated value.
#[derive(Clone, Debug, PartialEq)]
pub enum Argument<'a> {
    /// Process ID
    Pid(zx::Koid),
    /// Thread ID
    Tid(zx::Koid),
    /// A log tag
    Tag(Cow<'a, str>),
    /// Number of dropped logs
    Dropped(u64),
    /// A filename
    File(Cow<'a, str>),
    /// A log message
    Message(Cow<'a, str>),
    /// A line number in a file
    Line(u64),
    /// A custom argument with a given name and value
    Other {
        /// The name of the argument.
        name: Cow<'a, str>,
        /// The value of the argument.
        value: Value<'a>,
    },
}

impl<'a> Argument<'a> {
    /// Creates a new argument given its name and a value.
    pub fn new(name: impl Into<Cow<'a, str>>, value: impl Into<Value<'a>>) -> Self {
        let name: Cow<'a, str> = name.into();
        match (name.as_ref(), value.into()) {
            (constants::PID, Value::UnsignedInt(pid)) => Self::pid(zx::Koid::from_raw(pid)),
            (constants::TID, Value::UnsignedInt(pid)) => Self::tid(zx::Koid::from_raw(pid)),
            (constants::TAG, Value::Text(tag)) => Self::tag(tag),
            (constants::NUM_DROPPED, Value::UnsignedInt(dropped)) => Self::dropped(dropped),
            (constants::FILE, Value::Text(file)) => Self::file(file),
            (constants::LINE, Value::UnsignedInt(line)) => Self::line(line),
            (constants::MESSAGE, Value::Text(msg)) => Self::message(msg),
            (_, value) => Self::other(name, value),
        }
    }

    #[inline]
    /// Creates a new argument for a process id.
    pub fn pid(koid: zx::Koid) -> Self {
        Argument::Pid(koid)
    }

    #[inline]
    /// Creates a new argument for a thread id.
    pub fn tid(koid: zx::Koid) -> Self {
        Argument::Tid(koid)
    }

    #[inline]
    /// Creates a new argument for a log message.
    pub fn message(message: impl Into<Cow<'a, str>>) -> Self {
        Argument::Message(message.into())
    }

    #[inline]
    /// Creates a new argument for a tag.
    pub fn tag(value: impl Into<Cow<'a, str>>) -> Self {
        Argument::Tag(value.into())
    }

    #[inline]
    /// Creates a new argument for the number of dropped logs.
    pub fn dropped(value: u64) -> Self {
        Argument::Dropped(value)
    }

    #[inline]
    /// Creates a new argument for a file.
    pub fn file(value: impl Into<Cow<'a, str>>) -> Self {
        Argument::File(value.into())
    }

    #[inline]
    /// Creates a new argument for a line number.
    pub fn line(value: u64) -> Self {
        Argument::Line(value)
    }

    #[inline]
    /// Creates a new key-value argument.
    pub fn other(name: impl Into<Cow<'a, str>>, value: impl Into<Value<'a>>) -> Self {
        Argument::Other { name: name.into(), value: value.into() }
    }

    /// Consumes the current value and returns one in the static lifetime.
    pub fn into_owned(self) -> Argument<'static> {
        match self {
            Self::Pid(pid) => Argument::Pid(pid),
            Self::Tid(tid) => Argument::Tid(tid),
            Self::Tag(tag) => Argument::Tag(Cow::Owned(tag.into_owned())),
            Self::Dropped(dropped) => Argument::Dropped(dropped),
            Self::File(file) => Argument::File(Cow::Owned(file.into_owned())),
            Self::Line(line) => Argument::Line(line),
            Self::Message(msg) => Argument::Message(Cow::Owned(msg.into_owned())),
            Self::Other { name, value } => {
                Argument::Other { name: Cow::Owned(name.into_owned()), value: value.into_owned() }
            }
        }
    }

    /// Returns the name of the argument.
    pub fn name(&self) -> &str {
        match self {
            Self::Pid(_) => constants::PID,
            Self::Tid(_) => constants::TID,
            Self::Tag(_) => constants::TAG,
            Self::Dropped(_) => constants::NUM_DROPPED,
            Self::File(_) => constants::FILE,
            Self::Line(_) => constants::LINE,
            Self::Message(_) => constants::MESSAGE,
            Self::Other { name, .. } => name.borrow(),
        }
    }

    /// Returns the value of the argument.
    pub fn value(&'a self) -> Value<'a> {
        match self {
            Self::Pid(pid) => Value::UnsignedInt(pid.raw_koid()),
            Self::Tid(tid) => Value::UnsignedInt(tid.raw_koid()),
            Self::Tag(tag) => Value::Text(Cow::Borrowed(tag.as_ref())),
            Self::Dropped(num_dropped) => Value::UnsignedInt(*num_dropped),
            Self::File(file) => Value::Text(Cow::Borrowed(file.as_ref())),
            Self::Message(msg) => Value::Text(Cow::Borrowed(msg.as_ref())),
            Self::Line(line) => Value::UnsignedInt(*line),
            Self::Other { value, .. } => value.clone_borrowed(),
        }
    }
}

/// The value of a logging argument.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> {
    /// A signed integer value for a logging argument.
    SignedInt(i64),
    /// An unsigned integer value for a logging argument.
    UnsignedInt(u64),
    /// A floating point value for a logging argument.
    Floating(f64),
    /// A boolean value for a logging argument.
    Boolean(bool),
    /// A string value for a logging argument.
    Text(Cow<'a, str>),
}

impl<'a> Value<'a> {
    fn into_owned(self) -> Value<'static> {
        match self {
            Self::Text(s) => Value::Text(Cow::Owned(s.into_owned())),
            Self::SignedInt(n) => Value::SignedInt(n),
            Self::UnsignedInt(n) => Value::UnsignedInt(n),
            Self::Floating(n) => Value::Floating(n),
            Self::Boolean(n) => Value::Boolean(n),
        }
    }

    fn clone_borrowed(&'a self) -> Value<'a> {
        match self {
            Self::Text(s) => Self::Text(Cow::Borrowed(s.as_ref())),
            Self::SignedInt(n) => Self::SignedInt(*n),
            Self::UnsignedInt(n) => Self::UnsignedInt(*n),
            Self::Floating(n) => Self::Floating(*n),
            Self::Boolean(n) => Self::Boolean(*n),
        }
    }
}

impl From<i32> for Value<'_> {
    fn from(number: i32) -> Value<'static> {
        Value::SignedInt(number as i64)
    }
}

impl From<i64> for Value<'_> {
    fn from(number: i64) -> Value<'static> {
        Value::SignedInt(number)
    }
}

impl From<u64> for Value<'_> {
    fn from(number: u64) -> Value<'static> {
        Value::UnsignedInt(number)
    }
}

impl From<u32> for Value<'_> {
    fn from(number: u32) -> Value<'static> {
        Value::UnsignedInt(number as u64)
    }
}

impl From<zx::Koid> for Value<'_> {
    fn from(koid: zx::Koid) -> Value<'static> {
        Value::UnsignedInt(koid.raw_koid())
    }
}

impl From<f64> for Value<'_> {
    fn from(number: f64) -> Value<'static> {
        Value::Floating(number)
    }
}

impl<'a> From<&'a str> for Value<'a> {
    fn from(text: &'a str) -> Value<'a> {
        Value::Text(Cow::Borrowed(text))
    }
}

impl From<String> for Value<'static> {
    fn from(text: String) -> Value<'static> {
        Value::Text(Cow::Owned(text))
    }
}

impl<'a> From<Cow<'a, str>> for Value<'a> {
    fn from(text: Cow<'a, str>) -> Value<'a> {
        Value::Text(text)
    }
}

impl From<bool> for Value<'static> {
    fn from(boolean: bool) -> Value<'static> {
        Value::Boolean(boolean)
    }
}

/// The maximum value acceptable for `size_words` below.
pub const MAX_SIZE_WORDS: u16 = 4095;

/// The bit in the tag that indicates the record is a control message, rather than a FXT record
/// produced by a component.
pub const LOG_CONTROL_BIT: u32 = 1 << 31;

bitfield! {
    /// A header in the tracing format. Expected to precede every Record and Argument.
    ///
    /// The tracing format specifies [Record headers] and [Argument headers] as distinct types, but
    /// their layouts are the same in practice, so we represent both bitfields using the same
    /// struct.
    ///
    /// [Record headers]: https://fuchsia.dev/fuchsia-src/development/tracing/trace-format#record_header
    /// [Argument headers]: https://fuchsia.dev/fuchsia-src/development/tracing/trace-format#argument_header
    #[derive(IntoBytes, FromBytes, KnownLayout, Immutable)]
    pub struct Header(u64);
    impl Debug;

    /// Record type.
    pub u8, raw_type, set_type: 3, 0;

    /// Record size as a multiple of 8 bytes.
    pub u16, size_words, set_size_words: 15, 4;

    /// String ref for the associated name, if any.
    u16, name_ref, set_name_ref: 31, 16;

    /// Boolean value, if any.
    bool, bool_val, set_bool_val: 32;

    /// Reserved for record-type-specific data.
    u16, value_ref, set_value_ref: 47, 32;

    /// Tag for the record, if any.
    pub u32, tag, set_tag: 47, 16;

    /// Severity of the record, if any.
    pub u8, severity, set_severity: 63, 56;
}

impl Header {
    /// Sets the length of the item the header refers to. Panics if not 8-byte aligned.
    pub fn set_len(&mut self, new_len: usize) {
        assert_eq!(new_len % 8, 0, "encoded message must be 8-byte aligned");
        self.set_size_words((new_len / 8) as u16 + u16::from(!new_len.is_multiple_of(8)))
    }
}

/// Tag derived from metadata.
///
/// Unlike tags, metatags are not represented as strings and instead must be resolved from event
/// metadata. This means that they may resolve to different text for different events.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Metatag {
    /// The location of a span or event.
    ///
    /// The target is typically a module path, but this can be configured by a particular span or
    /// event when it is constructed.
    Target,
}

/// These literal values are specified by the tracing format:
///
/// https://fuchsia.dev/fuchsia-src/development/tracing/trace-format#argument_header
#[repr(u8)]
enum ArgType {
    Null = 0,
    I32 = 1,
    U32 = 2,
    I64 = 3,
    U64 = 4,
    F64 = 5,
    String = 6,
    Pointer = 7,
    Koid = 8,
    Bool = 9,
}

impl TryFrom<u8> for ArgType {
    type Error = parse::ParseError;
    fn try_from(b: u8) -> Result<Self, Self::Error> {
        Ok(match b {
            0 => ArgType::Null,
            1 => ArgType::I32,
            2 => ArgType::U32,
            3 => ArgType::I64,
            4 => ArgType::U64,
            5 => ArgType::F64,
            6 => ArgType::String,
            7 => ArgType::Pointer,
            8 => ArgType::Koid,
            9 => ArgType::Bool,
            _ => return Err(parse::ParseError::ValueOutOfValidRange),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{Encoder, EncoderOpts, EncodingError, MutableBuffer};
    use fidl_fuchsia_diagnostics_types::Severity;
    use std::fmt::Debug;
    use std::io::Cursor;

    fn parse_argument(bytes: &[u8]) -> (&[u8], Argument<'static>) {
        let (decoded_from_full, remaining) = crate::parse::parse_argument(bytes).unwrap();
        (remaining, decoded_from_full.into_owned())
    }

    fn parse_record(bytes: &[u8]) -> (&[u8], Record<'static>) {
        let (decoded_from_full, remaining) = crate::parse::parse_record(bytes).unwrap();
        (remaining, decoded_from_full.into_owned())
    }

    const BUF_LEN: usize = 1024;

    pub(crate) fn assert_roundtrips<T, F>(
        val: T,
        encoder_method: impl Fn(&mut Encoder<Cursor<Vec<u8>>>, &T) -> Result<(), EncodingError>,
        parser: F,
        canonical: Option<&[u8]>,
    ) where
        T: Debug + PartialEq,
        F: Fn(&[u8]) -> (&[u8], T),
    {
        let mut encoder = Encoder::new(Cursor::new(vec![0; BUF_LEN]), EncoderOpts::default());
        encoder_method(&mut encoder, &val).unwrap();

        // next we'll parse the record out of a buf with padding after the record
        let (_, decoded_from_full) = parser(encoder.buf.get_ref());
        assert_eq!(val, decoded_from_full, "decoded version with trailing padding must match");

        if let Some(canonical) = canonical {
            let recorded = encoder.buf.get_ref().split_at(canonical.len()).0;
            assert_eq!(canonical, recorded, "encoded repr must match the canonical value provided");

            let (zero_buf, decoded) = parser(recorded);
            assert_eq!(val, decoded, "decoded version must match what we tried to encode");
            assert_eq!(zero_buf.len(), 0, "must parse record exactly out of provided buffer");
        }
    }

    /// Bit pattern for the log record type, severity info, and a record of two words: one header,
    /// one timestamp.
    const MINIMAL_LOG_HEADER: u64 = 0x3000000000000029;

    #[fuchsia::test]
    fn minimal_header() {
        let mut poked = Header(0);
        poked.set_type(TRACING_FORMAT_LOG_RECORD_TYPE);
        poked.set_size_words(2);
        poked.set_severity(Severity::Info.into_primitive());

        assert_eq!(
            poked.0, MINIMAL_LOG_HEADER,
            "minimal log header should only describe type, size, and severity"
        );
    }

    #[fuchsia::test]
    fn no_args_roundtrip() {
        let mut expected_record = MINIMAL_LOG_HEADER.to_le_bytes().to_vec();
        let timestamp = zx::BootInstant::from_nanos(5_000_000i64);
        expected_record.extend(timestamp.into_nanos().to_le_bytes());

        assert_roundtrips(
            Record { timestamp, severity: Severity::Info.into_primitive(), arguments: vec![] },
            |encoder, val| encoder.write_record(val),
            parse_record,
            Some(&expected_record),
        );
    }

    #[fuchsia::test]
    fn signed_arg_roundtrip() {
        assert_roundtrips(
            Argument::other("signed", -1999),
            |encoder, val| encoder.write_argument(val),
            parse_argument,
            None,
        );
    }

    #[fuchsia::test]
    fn unsigned_arg_roundtrip() {
        assert_roundtrips(
            Argument::other("unsigned", 42),
            |encoder, val| encoder.write_argument(val),
            parse_argument,
            None,
        );
    }

    #[fuchsia::test]
    fn text_arg_roundtrip() {
        assert_roundtrips(
            Argument::other("stringarg", "owo"),
            |encoder, val| encoder.write_argument(val),
            parse_argument,
            None,
        );
    }

    #[fuchsia::test]
    fn float_arg_roundtrip() {
        assert_roundtrips(
            Argument::other("float", 3.25),
            |encoder, val| encoder.write_argument(val),
            parse_argument,
            None,
        );
    }

    #[fuchsia::test]
    fn bool_arg_roundtrip() {
        assert_roundtrips(
            Argument::other("bool", false),
            |encoder, val| encoder.write_argument(val),
            parse_argument,
            None,
        );
    }

    #[fuchsia::test]
    fn arg_of_each_type_roundtrips() {
        assert_roundtrips(
            Record {
                timestamp: zx::BootInstant::get(),
                severity: Severity::Warn.into_primitive(),
                arguments: vec![
                    Argument::other("signed", -10),
                    Argument::other("unsigned", 7),
                    Argument::other("float", 3.25),
                    Argument::other("bool", true),
                    Argument::other("msg", "test message one"),
                ],
            },
            |encoder, val| encoder.write_record(val),
            parse_record,
            None,
        );
    }

    #[fuchsia::test]
    fn multiple_string_args() {
        assert_roundtrips(
            Record {
                timestamp: zx::BootInstant::get(),
                severity: Severity::Trace.into_primitive(),
                arguments: vec![
                    Argument::other("msg", "test message one"),
                    Argument::other("msg", "test message two"),
                    Argument::other("msg", "test message three"),
                ],
            },
            |encoder, val| encoder.write_record(val),
            parse_record,
            None,
        );
    }

    #[fuchsia::test]
    fn invalid_records() {
        // invalid word size
        let mut encoder = Encoder::new(Cursor::new(vec![0; BUF_LEN]), EncoderOpts::default());
        let mut header = Header(0);
        header.set_type(TRACING_FORMAT_LOG_RECORD_TYPE);
        header.set_size_words(0); // invalid, should be at least 2 as header and time are included
        encoder.buf.put_u64_le(header.0).unwrap();
        encoder.buf.put_i64_le(zx::BootInstant::get().into_nanos()).unwrap();
        encoder.write_argument(Argument::other("msg", "test message one")).unwrap();
        assert!(crate::parse::parse_record(encoder.buf.get_ref()).is_err());
    }
}
