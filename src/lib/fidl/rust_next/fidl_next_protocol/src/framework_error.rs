// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::error::Error;
use core::fmt;

/// An internal framework error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FrameworkError {
    /// The protocol method was not recognized by the receiver.
    UnknownMethod = -2,
}

impl fmt::Display for FrameworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMethod => write!(f, "unknown method"),
        }
    }
}

impl Error for FrameworkError {}
