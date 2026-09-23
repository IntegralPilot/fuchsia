// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth as bt;
use thiserror::Error;

/// Errors that occur in the fuchsia-bluetooth crate.
#[derive(Debug, Error)]
pub enum Error {
    #[error("Error using the `bredr.Profile` resource: {}", .0)]
    Profile(String),

    #[error("Error using the `sys` resource: {}", .0)]
    Sys(String),

    #[error("Error using the `le` resource: {}", .0)]
    LE(String),

    #[error("Conversion to/from a type failed: {}", .0)]
    FailedConversion(String),

    #[error("Mandatory field {} is missing", .0)]
    MissingRequired(String),

    #[error("FIDL Error: {0}")]
    Fidl(#[from] fidl::Error),

    /// An error from another source
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    pub fn profile(msg: impl Into<String>) -> Self {
        Self::Profile(msg.into())
    }

    pub fn sys(msg: impl Into<String>) -> Self {
        Self::Sys(msg.into())
    }

    pub fn le(msg: impl Into<String>) -> Self {
        Self::LE(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(anyhow::format_err!("{}", msg.into()))
    }

    pub fn external(e: impl Into<anyhow::Error>) -> Self {
        Self::Other(e.into())
    }

    pub fn missing(msg: impl Into<String>) -> Self {
        Self::MissingRequired(msg.into())
    }

    pub fn conversion(msg: impl Into<String>) -> Self {
        Self::FailedConversion(msg.into())
    }
}

impl From<bt::Error> for Error {
    fn from(err: bt::Error) -> Error {
        let message = err.description.unwrap_or_else(|| "unknown Bluetooth FIDL error".to_string());
        Error::other(message)
    }
}

impl From<bt::ErrorCode> for Error {
    fn from(err: bt::ErrorCode) -> Error {
        let message = format!("Bluetooth Error Code {err:?}");
        Error::other(message)
    }
}
