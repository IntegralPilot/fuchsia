// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::ScanError;

use thiserror::Error;
use wlan_common::append::BufferTooSmall;
use wlan_common::error::{FrameParseError, FrameWriteError};

#[derive(Debug, Error)]
pub enum Error {
    #[error("provided buffer to small")]
    BufferTooSmall,
    #[error("error parsing frame: {}", _0)]
    ParsingFrame(FrameParseError),
    #[error("error writing frame: {}", _0)]
    WritingFrame(FrameWriteError),
    #[error("scan error: {}", _0)]
    ScanError(ScanError),
    #[error("{}", _0)]
    Internal(anyhow::Error),
    #[error("{}", _0)]
    Fidl(fidl::Error),
    #[error("{}; {}", _0, _1)]
    Status(String, zx::Status),
}

impl From<Error> for zx::Status {
    fn from(e: Error) -> Self {
        match e {
            Error::BufferTooSmall => zx::Status::BUFFER_TOO_SMALL,
            Error::Internal(_) => zx::Status::INTERNAL,
            Error::ParsingFrame(_) => zx::Status::IO_INVALID,
            Error::WritingFrame(_) => zx::Status::IO_REFUSED,
            Error::ScanError(e) => e.into(),
            Error::Fidl(e) => match e {
                fidl::Error::ClientRead(fidl::TransportError::Status(status))
                | fidl::Error::ClientWrite(fidl::TransportError::Status(status))
                | fidl::Error::ServerResponseWrite(fidl::TransportError::Status(status))
                | fidl::Error::ServerRequestRead(fidl::TransportError::Status(status)) => status,
                _ => zx::Status::IO,
            },
            Error::Status(_, status) => status,
        }
    }
}

pub trait ResultExt {
    /// Returns ZX_OK if Self is Ok, otherwise, prints an error and turns Self into a corresponding
    /// ZX_ERR_*.
    fn into_raw_zx_status(self) -> zx::sys::zx_status_t;
}

impl ResultExt for Result<(), Error> {
    fn into_raw_zx_status(self) -> zx::sys::zx_status_t {
        match self {
            Ok(()) => zx::sys::ZX_OK,
            Err(e) => {
                eprintln!("{}", e);
                Into::<zx::Status>::into(e).into_raw()
            }
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Internal(e)
    }
}

impl From<FrameParseError> for Error {
    fn from(e: FrameParseError) -> Self {
        Error::ParsingFrame(e)
    }
}

impl From<FrameWriteError> for Error {
    fn from(e: FrameWriteError) -> Self {
        Error::WritingFrame(e)
    }
}

impl From<ScanError> for Error {
    fn from(e: ScanError) -> Self {
        Error::ScanError(e)
    }
}

impl From<BufferTooSmall> for Error {
    fn from(_: BufferTooSmall) -> Self {
        Error::BufferTooSmall
    }
}

impl From<fidl::Error> for Error {
    fn from(e: fidl::Error) -> Self {
        Error::Fidl(e)
    }
}

impl From<zx::Status> for Error {
    fn from(e: zx::Status) -> Self {
        Error::Status(e.to_string(), e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::format_err;

    #[test]
    fn test_error_into_status() {
        let status = zx::Status::from(Error::Status("foo".to_string(), zx::Status::INVALID_ARGS));
        assert_eq!(status, zx::Status::INVALID_ARGS);

        let status = zx::Status::from(Error::Status("foo".to_string(), zx::Status::NOT_SUPPORTED));
        assert_eq!(status, zx::Status::NOT_SUPPORTED);

        let status = zx::Status::from(Error::Internal(format_err!("lorem")));
        assert_eq!(status, zx::Status::INTERNAL);

        let status = zx::Status::from(Error::WritingFrame(FrameWriteError::BufferTooSmall));
        assert_eq!(status, zx::Status::IO_REFUSED);

        let status = zx::Status::from(Error::BufferTooSmall);
        assert_eq!(status, zx::Status::BUFFER_TOO_SMALL);

        let status = zx::Status::from(Error::Fidl(fidl::Error::ClientWrite(
            zx::Status::NOT_SUPPORTED.into(),
        )));
        assert_eq!(status, zx::Status::NOT_SUPPORTED);
    }
}
