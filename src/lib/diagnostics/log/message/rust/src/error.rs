// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use crate::{MAX_TAG_LEN, MAX_TAGS, MIN_PACKET_SIZE};
use diagnostics_log_encoding::parse::ParseError;
use moniker::MonikerError;
use thiserror::Error;

use std::str::Utf8Error;

#[derive(Debug, Error)]
pub enum MessageError {
    #[error("unrecognized value type encountered")]
    UnrecognizedValue,
    #[error("wrong value type encountered, expected integer, found {found} {value}")]
    ExpectedInteger { value: String, found: &'static str },
    #[error("couldn't parse message: {parse_error:?}")]
    ParseError {
        #[from]
        parse_error: ParseError,
    },
    #[error("incorrect or corrupt metadata indicates to read past end of buffer")]
    OutOfBounds,
    #[error("provided tag at position {index} with length {len} (max {})", MAX_TAG_LEN)]
    TagTooLong { index: usize, len: usize },
    #[error("provided {} or more tags", MAX_TAGS + 1)]
    TooManyTags,
    #[error("message incorrectly terminated, found {terminator}, expected 0")]
    NotNullTerminated { terminator: u8 },
    #[error("buffer too small ({len}) to contain a valid log message (min {})", MIN_PACKET_SIZE)]
    ShortRead { len: usize },
    #[error("No URL found in extended record")]
    MissingUrl,
    #[error("No moniker found in extended record")]
    MissingMoniker,
    #[error("couldn't parse moniker: {parse_error:?}")]
    MonikerParseError {
        #[from]
        parse_error: MonikerError,
    },
    #[error("string is not valid utf8: {source:?}")]
    Utf8Error {
        #[from]
        source: Utf8Error,
    },
}

impl PartialEq for MessageError {
    fn eq(&self, other: &Self) -> bool {
        use MessageError::*;
        match (self, other) {
            (ShortRead { len }, ShortRead { len: len2 }) => len == len2,
            (NotNullTerminated { terminator }, NotNullTerminated { terminator: t2 }) => {
                terminator == t2
            }
            (TooManyTags, TooManyTags) => true,
            (TagTooLong { index, len }, TagTooLong { index: i2, len: l2 }) => {
                index == i2 && len == l2
            }
            (OutOfBounds, OutOfBounds) => true,
            _ => false,
        }
    }
}
