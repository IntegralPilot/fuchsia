// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_syscalls::{SyscallArg, SyscallResult};
use std::fmt;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Watch descriptor returned by inotify_add_watch(2).
///
/// See inotify(7) for details.
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
pub struct WdNumber(i32);

impl WdNumber {
    pub fn from_raw(n: i32) -> WdNumber {
        WdNumber(n)
    }

    pub fn raw(&self) -> i32 {
        self.0
    }
}

impl std::convert::From<WdNumber> for SyscallResult {
    fn from(value: WdNumber) -> Self {
        value.raw().into()
    }
}

impl std::convert::From<SyscallArg> for WdNumber {
    fn from(value: SyscallArg) -> Self {
        WdNumber::from_raw(value.into())
    }
}

impl fmt::Display for WdNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wd({})", self.0)
    }
}
