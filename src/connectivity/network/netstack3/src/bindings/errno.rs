// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides utilities for passing around errnos in a more debuggable way.

use fidl_fuchsia_posix::Errno;

use crate::bindings::error::Error;
use crate::bindings::util::ErrorLogExt;

#[derive(Debug, thiserror::Error)]
#[error("{errno:?} ({source})")]
pub(crate) struct ErrnoError {
    errno: Errno,
    #[source]
    source: ErrorWithLocation,
}

impl ErrnoError {
    pub(crate) fn map_errno(self, f: impl FnOnce(Errno) -> Errno) -> Self {
        let Self { errno, source: cause } = self;
        let errno = f(errno);
        Self { errno, source: cause }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("cause: {error}, location: {location}")]
pub(crate) struct ErrorWithLocation {
    location: &'static std::panic::Location<'static>,
    #[source]
    error: Error,
}

impl ErrnoError {
    #[track_caller]
    pub(crate) fn new(errno: Errno, source: impl Into<Error>) -> Self {
        Self {
            errno,
            source: ErrorWithLocation {
                location: std::panic::Location::caller(),
                error: source.into(),
            },
        }
    }

    pub(crate) fn into_errno_and_source(self) -> (Errno, ErrorWithLocation) {
        let Self { errno, source } = self;
        (errno, source)
    }
}

impl ErrorLogExt for ErrnoError {
    fn log_level(&self) -> log::Level {
        self.errno.log_level()
    }
}
