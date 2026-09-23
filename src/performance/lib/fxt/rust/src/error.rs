// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::thread::{ProcessRef, ThreadRef};
use flyweights::FlyStr;
use nom::error::ErrorKind;
use std::num::NonZeroU16;

pub(crate) type ParseResult<'a, T> = nom::IResult<&'a [u8], T, ParseError>;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// We encountered a generic `nom` error while parsing.
    #[error("nom parsing error: {1:?}")]
    Nom(nom::error::ErrorKind, #[source] Option<Box<Self>>),

    /// We encountered an error performing I/O to read the trace session.
    #[error("failure reading the trace session")]
    Io(#[source] std::io::Error),

    /// We encountered invalid UTF-8 while parsing.
    #[error("couldn't parse string as utf-8")]
    InvalidUtf8(
        #[from]
        #[source]
        std::str::Utf8Error,
    ),

    /// We encountered a non-magic-number record at the beginning of the session.
    #[error("trace session didn't start with the magic number record")]
    MissingMagicNumber,

    /// We encountered an unexpected type ordinal while parsing.
    #[error("expected type {expected} for {context}, observed type {observed}")]
    WrongType { expected: u8, observed: u8, context: &'static str },

    /// We encountered an incorrect magic number while parsing.
    #[error("got the wrong magic number: {observed}")]
    InvalidMagicNumber { observed: u32 },

    /// We encountered an invalid reference, like a zero thread id.
    #[error("got an invalid ref")]
    InvalidRef,

    /// We encountered an invalid length for a record.
    #[error("invalid length prefix encountered")]
    InvalidSize,
}

impl nom::error::ParseError<&[u8]> for ParseError {
    fn from_error_kind(_input: &[u8], kind: nom::error::ErrorKind) -> Self {
        ParseError::Nom(kind, None)
    }

    fn append(_input: &[u8], kind: nom::error::ErrorKind, prev: Self) -> Self {
        ParseError::Nom(kind, Some(Box::new(prev)))
    }
}

impl nom::error::FromExternalError<&[u8], ParseError> for ParseError {
    fn from_external_error(_input: &[u8], _kind: ErrorKind, e: ParseError) -> Self {
        e
    }
}

/// Scenarios encountered during parsing that didn't prevent parsing from succeeding but which
/// might affect analysis of the session.
#[derive(Clone, Debug, thiserror::Error, PartialEq)]
pub enum ParseWarning {
    #[error("encountered unknown thread reference `{_0:?}`")]
    UnknownThreadRef(ThreadRef),

    #[error("encountered unknown process reference `{_0:?}`")]
    UnknownProcessRef(ProcessRef),

    #[error("encountered unknown trace record type {_0}")]
    UnknownTraceRecordType(u8),

    #[error("skipped arg '{name}' because of unknown type")]
    SkippingArgWithUnknownType { name: FlyStr },

    #[error("encountered unknown provider id {_0}")]
    UnknownProviderId(u32),

    #[error("encountered unknown string id {_0}")]
    UnknownStringId(NonZeroU16),

    #[error("encountered an empty string record")]
    RecordForZeroStringId,

    #[error("encountered unknown large blob type {_0}")]
    UnknownLargeBlobType(u8),

    #[error("encountered unknown metadata record type {_0}")]
    UnknownMetadataRecordType(u8),

    #[error("encountered unknown scheduling record type {_0}")]
    UnknownSchedulingRecordType(u8),

    #[error("encountered unknown profiler record type {_0}")]
    UnknownProfilerRecordType(u8),

    #[error("encountered a record without a provider set")]
    MissingProviderId,
}
