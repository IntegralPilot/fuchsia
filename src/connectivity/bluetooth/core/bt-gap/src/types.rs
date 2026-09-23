// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_bluetooth_sys as sys;
use thiserror::Error;

/// Type representing Possible errors raised in the operation of BT-GAP
#[derive(Debug, Error)]
pub enum Error {
    /// Internal bt-gap Error
    #[error("Internal bt-gap Error: {0}")]
    InternalError(anyhow::Error),

    /// fuchsia.bluetooth.sys API errors. Used to encapsulate errors that are reported by bt-host
    /// and for the fuchsia.bluetooth.sys.Access API.
    #[error("fuchsia.bluetooth.sys Error: {0:?}")]
    SysError(sys::Error),

    /// Errors from the fuchsia-bluetooth crate.
    #[error(transparent)]
    BTCrate(#[from] fuchsia_bluetooth::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn no_host() -> Error {
        Error::SysError(sys::Error::Failed)
    }

    pub fn as_failure(self) -> anyhow::Error {
        match self {
            Error::InternalError(err) => err,
            Error::SysError(err) => format_err!("Host Error: {err:?}"),
            Error::BTCrate(err) => format_err!("Bluetooth crate Error: {err:?}"),
        }
    }
}

impl From<sys::Error> for Error {
    fn from(err: sys::Error) -> Error {
        Error::SysError(err)
    }
}

impl Into<sys::Error> for Error {
    fn into(self) -> sys::Error {
        match self {
            Error::SysError(err) => err,
            Error::InternalError(_) | Error::BTCrate(_) => sys::Error::Failed,
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Error {
        Error::InternalError(err)
    }
}

impl From<fidl::Error> for Error {
    fn from(err: fidl::Error) -> Error {
        Error::InternalError(format_err!(format!("Internal FIDL error: {}", err)))
    }
}

pub fn from_fidl_result<T>(r: fidl::Result<std::result::Result<T, sys::Error>>) -> Result<T> {
    match r {
        Ok(r) => r.map_err(Error::from),
        Err(e) => Err(Error::from(e)),
    }
}
