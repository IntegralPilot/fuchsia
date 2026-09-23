// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::Errno;
use crate::{error, uapi};

/// See fcntl F_SETLEASE and F_GETLEASE.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileLeaseType {
    /// Corresponds to F_UNLCK.
    #[default]
    Unlocked,

    /// Corresponds to F_RDLCK.
    Read,

    /// Corresponds to F_WRLCK.
    Write,
}

impl FileLeaseType {
    pub fn from_bits(value: u32) -> Result<Self, Errno> {
        match value {
            uapi::F_UNLCK => Ok(Self::Unlocked),
            uapi::F_RDLCK => Ok(Self::Read),
            uapi::F_WRLCK => Ok(Self::Write),
            _ => error!(EINVAL),
        }
    }

    pub fn bits(&self) -> u32 {
        match self {
            Self::Unlocked => uapi::F_UNLCK,
            Self::Read => uapi::F_RDLCK,
            Self::Write => uapi::F_WRLCK,
        }
    }
}
