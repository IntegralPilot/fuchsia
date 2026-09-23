// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::vfs::FsStr;
use starnix_syscalls::{SyscallArg, SyscallResult};
use starnix_uapi::errors::Errno;
use starnix_uapi::{AT_FDCWD, errno};

// NB: We believe deriving Default (i.e., have a default FdNumber of 0) will be error-prone.
#[derive(
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Debug,
    Copy,
    Clone,
    IntoBytes,
    KnownLayout,
    FromBytes,
    Immutable,
)]
#[repr(transparent)]
pub struct FdNumber(i32);

impl FdNumber {
    pub const AT_FDCWD: FdNumber = FdNumber(AT_FDCWD);

    pub fn from_raw(n: i32) -> FdNumber {
        FdNumber(n)
    }

    pub fn raw(&self) -> i32 {
        self.0
    }

    /// Parses a file descriptor number from a byte string.
    pub fn from_fs_str(s: &FsStr) -> Result<Self, Errno> {
        let name = std::str::from_utf8(s).map_err(|_| errno!(EINVAL))?;
        let num = name.parse::<i32>().map_err(|_| errno!(EINVAL))?;
        Ok(FdNumber(num))
    }
}

impl std::convert::From<FdNumber> for SyscallResult {
    fn from(value: FdNumber) -> Self {
        value.raw().into()
    }
}

impl std::convert::From<SyscallArg> for FdNumber {
    fn from(value: SyscallArg) -> Self {
        FdNumber::from_raw(value.into())
    }
}

impl std::str::FromStr for FdNumber {
    type Err = Errno;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(FdNumber::from_raw(s.parse::<i32>().map_err(|e| errno!(EINVAL, e))?))
    }
}

impl fmt::Display for FdNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fd({})", self.0)
    }
}
