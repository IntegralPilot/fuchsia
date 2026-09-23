// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use nom::error::{ErrorKind, ParseError};
use serde_json::error::Error as SerdeError;
use std::result;
use std::str::Utf8Error;
use thiserror::Error;

/// A result for codec operations.
pub type Result<T> = result::Result<T, Error>;

/// Error type used by the codec.
#[derive(Debug, Error)]
pub enum Error {
    #[error("Error loading from FIDL JSON: {0}")]
    LibraryError(String),
    #[error("Could not parse FIDL JSON: {0}")]
    LibraryParseError(SerdeError),
    #[error("Encoding Error: {0}")]
    EncodeError(String),
    #[error("Decoding Error: {0}")]
    DecodeError(String),
    #[error("Value Error: {0}")]
    ValueError(String),
    #[error("UTF-8 Decoding error: {0}")]
    Utf8Error(Utf8Error),
    #[error("RecursionLimitExceeded")]
    RecursionLimitExceeded,
}

impl From<SerdeError> for Error {
    fn from(error: SerdeError) -> Self {
        Error::LibraryParseError(error)
    }
}

impl From<Error> for nom::Err<Error> {
    fn from(error: Error) -> Self {
        nom::Err::Failure(error)
    }
}

impl From<nom::Err<Error>> for Error {
    fn from(error: nom::Err<Error>) -> Self {
        match error {
            nom::Err::Incomplete(_) => Error::DecodeError("<Parsing Incomplete>".to_owned()),
            nom::Err::Error(x) | nom::Err::Failure(x) => x.into(),
        }
    }
}

impl ParseError<&[u8]> for Error {
    fn from_error_kind(_: &[u8], kind: ErrorKind) -> Self {
        Error::DecodeError(format!("Nom encountered an error (kind: {})", kind.description()))
    }

    fn append(_: &[u8], kind: ErrorKind, other: Self) -> Self {
        Error::DecodeError(format!(
            "Nom encountered an error (kind: {}) caused by: {}",
            kind.description(),
            other
        ))
    }
}
