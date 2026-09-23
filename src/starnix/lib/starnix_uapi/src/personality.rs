// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct PersonalityFlags: u32 {
        const UNAME26 = uapi::UNAME26;
        const ADDR_NO_RANDOMIZE = uapi::ADDR_NO_RANDOMIZE;
        const FDPIC_FUNCPTRS = uapi::FDPIC_FUNCPTRS;
        const MMAP_PAGE_ZERO = uapi::MMAP_PAGE_ZERO;
        const ADDR_COMPAT_LAYOUT = uapi::ADDR_COMPAT_LAYOUT;
        const READ_IMPLIES_EXEC = uapi::READ_IMPLIES_EXEC;
        const ADDR_LIMIT_32BIT = uapi::ADDR_LIMIT_32BIT;
        const SHORT_INODE = uapi::SHORT_INODE;
        const WHOLE_SECONDS = uapi::WHOLE_SECONDS;
        const STICKY_TIMEOUTS = uapi::STICKY_TIMEOUTS;
        const ADDR_LIMIT_3GB = uapi::ADDR_LIMIT_3GB;
    }
}

impl PersonalityFlags {
    /// Returns the execution domain (persona) part of the personality flags.
    pub fn execution_domain(&self) -> u32 {
        self.bits() & (uapi::PER_MASK as u32)
    }

    /// Updates the personality flags from a syscall argument.
    /// If `value` is `0xffffffff`, it does not update the flags.
    /// Returns the previous value of the flags as a u32.
    pub fn update_from_syscall(&mut self, value: u32) -> u32 {
        let prev = self.bits();
        if value != 0xffffffff {
            // Use `from_bits_retain()` since we want to keep unknown flags.
            *self = PersonalityFlags::from_bits_retain(value);
        }
        prev
    }
}
