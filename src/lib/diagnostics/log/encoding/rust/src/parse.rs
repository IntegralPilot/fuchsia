// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parse diagnostic records from streams, returning FIDL-generated structs that match expected
//! diagnostic service APIs.

use crate::{ArgType, Argument, Header, RawSeverity, Record, Value, constants, zx};
use std::borrow::Cow;
use thiserror::Error;
use zerocopy::FromBytes;

/// Extracts the basic information of a log message: timestamp and severity.
pub fn basic_info(buf: &[u8]) -> Result<(zx::BootInstant, RawSeverity), ParseError> {
    let (header, after_header) =
        Header::read_from_prefix(buf).map_err(|_| ParseError::InvalidHeader)?;
    if header.raw_type() != crate::TRACING_FORMAT_LOG_RECORD_TYPE {
        return Err(ParseError::InvalidRecordType);
    }
    let (timestamp, _) =
        i64::read_from_prefix(after_header).map_err(|_| ParseError::InvalidTimestamp)?;
    Ok((zx::BootInstant::from_nanos(timestamp), header.severity()))
}

/// Attempt to parse a diagnostic record from the head of this buffer, returning the record and any
/// unused portion of the buffer if successful.
pub fn parse_record(buf: &[u8]) -> Result<(Record<'_>, &[u8]), ParseError> {
    let (header, after_header) =
        Header::read_from_prefix(buf).map_err(|_| ParseError::InvalidHeader)?;

    if header.raw_type() != crate::TRACING_FORMAT_LOG_RECORD_TYPE {
        return Err(ParseError::ValueOutOfValidRange);
    }

    let (timestamp, remaining_buffer) =
        i64::read_from_prefix(after_header).map_err(|_| ParseError::InvalidTimestamp)?;

    let arguments_length = if header.size_words() >= 2 {
        // Remove two word lengths for header and timestamp.
        (header.size_words() - 2) as usize * 8
    } else {
        return Err(ParseError::ValueOutOfValidRange);
    };

    let Some((mut arguments_buffer, remaining)) =
        remaining_buffer.split_at_checked(arguments_length)
    else {
        return Err(ParseError::ValueOutOfValidRange);
    };

    let mut arguments = vec![];
    let mut state = ParseState::Initial;
    while !arguments_buffer.is_empty() {
        let (argument, rem) = parse_argument_internal(arguments_buffer, &mut state)?;
        arguments_buffer = rem;
        arguments.push(argument);
    }

    Ok((
        Record {
            timestamp: zx::BootInstant::from_nanos(timestamp),
            severity: header.severity(),
            arguments,
        },
        remaining,
    ))
}

/// Internal parser state.
/// Used to support handling of invalid utf-8 in msg fields.
enum ParseState {
    /// Initial parsing state
    Initial,
    /// We're in a message
    InMessage,
    /// We're in arguments (no special Unicode treatment)
    InArguments,
}

/// Parses an argument
pub fn parse_argument(buf: &[u8]) -> Result<(Argument<'_>, &[u8]), ParseError> {
    parse_argument_internal(buf, &mut ParseState::Initial)
}

fn parse_argument_internal<'a>(
    buf: &'a [u8],
    state: &mut ParseState,
) -> Result<(Argument<'a>, &'a [u8]), ParseError> {
    let (header, after_header) =
        Header::read_from_prefix(buf).map_err(|_| ParseError::InvalidArgumentHeader)?;
    let arg_ty = ArgType::try_from(header.raw_type())?;

    let (name, after_name) = string_ref(header.name_ref(), after_header, false)?;
    if matches!(state, ParseState::Initial) && name == Cow::Borrowed(constants::MESSAGE) {
        *state = ParseState::InMessage;
    }
    let (value, after_value) = match arg_ty {
        ArgType::Null => (Value::UnsignedInt(1), after_name),
        ArgType::I64 => {
            let (n, rem) =
                i64::read_from_prefix(after_name).map_err(|_| ParseError::InvalidArgument)?;
            (Value::SignedInt(n), rem)
        }
        ArgType::U64 => {
            let (n, rem) =
                u64::read_from_prefix(after_name).map_err(|_| ParseError::InvalidArgument)?;
            (Value::UnsignedInt(n), rem)
        }
        ArgType::F64 => {
            let (n, rem) =
                f64::read_from_prefix(after_name).map_err(|_| ParseError::InvalidArgument)?;
            (Value::Floating(n), rem)
        }
        ArgType::String => {
            let (s, rem) =
                string_ref(header.value_ref(), after_name, matches!(state, ParseState::InMessage))?;
            (Value::Text(s), rem)
        }
        ArgType::Bool => (Value::Boolean(header.bool_val()), after_name),
        ArgType::Pointer | ArgType::Koid | ArgType::I32 | ArgType::U32 => {
            return Err(ParseError::Unsupported);
        }
    };
    if matches!(state, ParseState::InMessage) {
        *state = ParseState::InArguments;
    }

    Ok((Argument::new(name, value), after_value))
}

fn string_ref(
    ref_mask: u16,
    buf: &[u8],
    support_invalid_utf8: bool,
) -> Result<(Cow<'_, str>, &[u8]), ParseError> {
    if ref_mask == 0 {
        return Ok((Cow::Borrowed(""), buf));
    }
    if (ref_mask & (1 << 15)) == 0 {
        return Err(ParseError::Unsupported);
    }
    // zero out the top bit
    let name_len = (ref_mask & !(1 << 15)) as usize;
    let Some((name, after_name)) = buf.split_at_checked(name_len) else {
        return Err(ParseError::ValueOutOfValidRange);
    };
    let parsed = if support_invalid_utf8 {
        String::from_utf8_lossy(name)
    } else {
        let name = std::str::from_utf8(name)?;
        Cow::Borrowed(name)
    };
    // In Fuchsia Trace Format (FXT), inline string contents are padded with zeroes up to the next
    // 8-byte word boundary:
    // 1. `name_len % 8` calculates how many bytes exist in the final partial 8-byte word (0..=7).
    // 2. `8 - (name_len % 8)` determines the distance to the next 8-byte boundary (1..=8).
    // 3. The outer `% 8` converts 8 back to 0 when `name_len` is already a multiple of 8.
    let padding_len = (8 - (name_len % 8)) % 8;
    let Some((_padding, after_padding)) = after_name.split_at_checked(padding_len) else {
        return Err(ParseError::ValueOutOfValidRange);
    };
    Ok((parsed, after_padding))
}

/// Errors which occur when interacting with streams of diagnostic records.
#[derive(Debug, Clone, Error)]
pub enum ParseError {
    /// We attempted to parse bytes as a type for which the bytes are not a valid pattern.
    #[error("value out of range")]
    ValueOutOfValidRange,

    /// We attempted to parse or encode values which are not yet supported by this implementation of
    /// the Fuchsia Tracing format.
    #[error("unsupported value type")]
    Unsupported,

    /// We failed to parse a record header.
    #[error("found invalid header")]
    InvalidHeader,

    /// We failed to parse a record header.
    #[error("found invalid header in an argument")]
    InvalidArgumentHeader,

    /// We failed to parse a record timestamp.
    #[error("found invalid timestamp after header")]
    InvalidTimestamp,

    /// We failed to parse a record argument.
    #[error("found invalid argument")]
    InvalidArgument,

    /// The record type wasn't the one we expected: LOGS
    #[error("found invalid record type")]
    InvalidRecordType,

    /// We failed to parse a complete item.
    #[error("parsing terminated early, remaining bytes: {0:?}")]
    Incomplete(usize),
}

impl From<std::str::Utf8Error> for ParseError {
    fn from(_: std::str::Utf8Error) -> Self {
        ParseError::ValueOutOfValidRange
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{Encoder, EncoderOpts};
    use fidl_fuchsia_diagnostics_types::Severity;
    use std::io::Cursor;

    #[fuchsia::test]
    fn basic_structured_info() {
        let expected_timestamp = zx::BootInstant::from_nanos(72);
        let record = Record {
            timestamp: expected_timestamp,
            severity: Severity::Error.into_primitive(),
            arguments: vec![],
        };
        let mut buffer = Cursor::new(vec![0u8; 1000]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();
        let encoded = &buffer.get_ref().as_slice()[..buffer.position() as usize];

        let (timestamp, severity) = basic_info(encoded).unwrap();
        assert_eq!(timestamp, expected_timestamp);
        assert_eq!(severity, Severity::Error.into_primitive());
    }

    #[fuchsia::test]
    fn parse_record_with_zeros() {
        let expected_timestamp = zx::BootInstant::from_nanos(72);
        let record = Record {
            timestamp: expected_timestamp,
            severity: Severity::Error.into_primitive(),
            arguments: vec![],
        };
        let mut buffer = Cursor::new(vec![0u8; 1000]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record.clone()).unwrap();

        // Ensure that some additional padding is ok to parse the record.
        let encoded = &buffer.get_ref().as_slice()[..buffer.position() as usize + 3];

        let (result_record, rem) = parse_record(encoded).unwrap();
        assert_eq!(rem.len(), 3);
        assert_eq!(record, result_record);
    }

    #[fuchsia::test]
    fn test_string_ref_padding() {
        let mut buf = vec![b'f', b'o', b'o', 0, 0, 0, 0, 0];
        buf.extend_from_slice(b"extra");
        let ref_mask = (1 << 15) | 3;
        let (parsed, remaining) = string_ref(ref_mask, &buf, false).unwrap();
        assert_eq!(parsed, "foo");
        assert_eq!(remaining, b"extra");
    }
}
