// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_uapi::file_lease::FileLeaseType;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::seal_flags::SealFlags;
use starnix_uapi::signals::Signal;
use starnix_uapi::user_address::UserAddress;

#[derive(Eq, PartialEq, Debug)]
pub struct SyscallResult(u64);
pub const SUCCESS: SyscallResult = SyscallResult(0);

impl SyscallResult {
    pub fn value(&self) -> u64 {
        self.0
    }
}

impl From<UserAddress> for SyscallResult {
    fn from(value: UserAddress) -> Self {
        SyscallResult(value.ptr() as u64)
    }
}

impl From<FileMode> for SyscallResult {
    fn from(value: FileMode) -> Self {
        SyscallResult(value.bits() as u64)
    }
}

impl From<SealFlags> for SyscallResult {
    fn from(value: SealFlags) -> Self {
        SyscallResult(value.bits() as u64)
    }
}

impl From<OpenFlags> for SyscallResult {
    fn from(value: OpenFlags) -> Self {
        SyscallResult(value.bits() as u64)
    }
}

impl From<Signal> for SyscallResult {
    fn from(value: Signal) -> Self {
        SyscallResult(value.number() as u64)
    }
}

impl From<FileLeaseType> for SyscallResult {
    fn from(value: FileLeaseType) -> Self {
        SyscallResult(value.bits() as u64)
    }
}

impl From<bool> for SyscallResult {
    fn from(value: bool) -> Self {
        #[allow(clippy::bool_to_int_with_if)]
        SyscallResult(if value { 1 } else { 0 })
    }
}

impl From<u8> for SyscallResult {
    fn from(value: u8) -> Self {
        SyscallResult(value as u64)
    }
}

impl From<i32> for SyscallResult {
    fn from(value: i32) -> Self {
        SyscallResult(value as u64)
    }
}

impl From<u32> for SyscallResult {
    fn from(value: u32) -> Self {
        SyscallResult(value as u64)
    }
}

impl From<i64> for SyscallResult {
    fn from(value: i64) -> Self {
        SyscallResult(value as u64)
    }
}

impl From<u64> for SyscallResult {
    fn from(value: u64) -> Self {
        SyscallResult(value)
    }
}

impl From<usize> for SyscallResult {
    fn from(value: usize) -> Self {
        SyscallResult(value as u64)
    }
}

impl From<()> for SyscallResult {
    fn from(_value: ()) -> Self {
        SyscallResult(0)
    }
}
