// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::{Debug, Display};

use crate::bindings::errno::ErrnoError;

/// An extension for common logging patterns for error types.
pub trait ErrorLogExt: Debug {
    fn log_level(&self) -> log::Level;

    #[track_caller]
    fn log(&self, msg: impl Display) {
        let level = self.log_level();
        log::log!(level, "{msg}: {self:?}");
    }
}

/// Extensions to `Result` types used in bindings.
///
/// It is implemented for well known result types.
pub trait ResultExt {
    /// The Ok type of this result.
    type Ok;
    /// The error type of this result.
    type Error: ErrorLogExt;

    fn as_result(self) -> Result<Self::Ok, Self::Error>;

    /// Consumes this result and logs an error if it's the `Err` variant.
    ///
    /// This allows many common logsites to be coalesced into a single function,
    /// which reduces the size of generated code.
    fn unwrap_or_log(self, msg: impl Display)
    where
        Self: ResultExt<Ok = ()>;

    fn log_if_err(&self, msg: impl Display);

    fn borrowed_ok(&self) -> Result<&'_ Self::Ok, Self::Error>
    where
        Self::Error: Clone;
}

impl<O, E: ErrorLogExt> ResultExt for Result<O, E> {
    type Ok = O;
    type Error = E;

    fn as_result(self) -> Result<O, E> {
        self
    }

    #[track_caller]
    fn unwrap_or_log(self, msg: impl Display)
    where
        Self: ResultExt<Ok = ()>,
    {
        match self.as_result() {
            Ok(()) => (),
            Err(e) => {
                e.log(msg);
            }
        }
    }

    #[track_caller]
    fn log_if_err(&self, msg: impl Display) {
        match self {
            Ok(_) => {}
            Err(e) => e.log(msg),
        }
    }

    fn borrowed_ok(&self) -> Result<&O, E>
    where
        Self::Error: Clone,
    {
        self.as_ref().map_err(Clone::clone)
    }
}

impl ErrorLogExt for fidl::Error {
    fn log_level(&self) -> log::Level {
        if self.is_closed() { log::Level::Debug } else { log::Level::Error }
    }
}

impl ErrorLogExt for fidl_fuchsia_posix::Errno {
    fn log_level(&self) -> log::Level {
        match self {
            // Errnos that indicate the socket API is being called incorrectly.
            fidl_fuchsia_posix::Errno::Einval
            | fidl_fuchsia_posix::Errno::Eafnosupport
            | fidl_fuchsia_posix::Errno::Enoprotoopt => log::Level::Warn,
            // Errnos that may occur under normal operation and are quite noisy.
            fidl_fuchsia_posix::Errno::Enetunreach
            | fidl_fuchsia_posix::Errno::Ehostunreach
            | fidl_fuchsia_posix::Errno::Eagain => log::Level::Trace,
            // All other errnos.
            _ => log::Level::Debug,
        }
    }
}

/// Extension trait implemented for [`Result`]s whose Err variants are [`ErrnoError`].
pub(crate) trait ErrnoResultExt {
    /// The Ok type of this result.
    type Ok;

    /// Logs the error with `msg` if it's an error and returns back the same result with the
    /// ErrnoError mapped to just an Errno.
    fn log_errno_error(self, msg: impl Display) -> Result<Self::Ok, fidl_fuchsia_posix::Errno>;
}

impl<O> ErrnoResultExt for Result<O, ErrnoError> {
    type Ok = O;

    #[track_caller]
    fn log_errno_error(self, msg: impl Display) -> Result<O, fidl_fuchsia_posix::Errno> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => {
                let (errno, source) = e.into_errno_and_source();
                errno.log(format_args!("{msg} ({source})"));
                Err(errno)
            }
        }
    }
}
