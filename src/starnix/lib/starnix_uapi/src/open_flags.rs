// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;

// The inner mod is required because bitflags cannot pass the attribute through to the single
// variant, and attributes cannot be applied to macro invocations.
mod inner_flags {
    // Part of the code for the O_RDONLY case that's produced by the macro triggers the lint, but as
    // a whole, the produced code is still correct.
    #![allow(clippy::bad_bit_mask)] // TODO(b/303500202) Remove once addressed in bitflags.
    use super::uapi;
    use atomic_bitflags::atomic_bitflags;

    atomic_bitflags! {
        #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
        pub struct OpenFlags: u32 {
        const ACCESS_MASK = 0x3;

        // The access modes are not really bits. Instead, they're an enum
        // embedded in the bitfield. Use ACCESS_MASK to extract the enum
        // or use the OpenFlags::can_read and OpenFlags::can_write functions.
        const RDONLY = uapi::O_RDONLY;
        const WRONLY = uapi::O_WRONLY;
        const RDWR = uapi::O_RDWR;

        const CREAT = uapi::O_CREAT;
        const EXCL = uapi::O_EXCL;
        const NOCTTY = uapi::O_NOCTTY;
        const TRUNC = uapi::O_TRUNC;
        const APPEND = uapi::O_APPEND;
        const NONBLOCK = uapi::O_NONBLOCK;
        const DSYNC = uapi::O_DSYNC;
        const ASYNC = uapi::FASYNC;
        const DIRECT = uapi::O_DIRECT;
        const LARGEFILE = uapi::O_LARGEFILE;
        const DIRECTORY = uapi::O_DIRECTORY;
        const NOFOLLOW = uapi::O_NOFOLLOW;
        const NOATIME = uapi::O_NOATIME;
        const CLOEXEC = uapi::O_CLOEXEC;
        const SYNC = uapi::O_SYNC;
        const PATH = uapi::O_PATH;
        const TMPFILE = uapi::O_TMPFILE;
        const NDELAY = uapi::O_NDELAY;
        }
    }

    impl OpenFlags {
        pub fn can_read(&self) -> bool {
            let access_mode = self.bits() & Self::ACCESS_MASK.bits();
            access_mode == uapi::O_RDONLY || access_mode == uapi::O_RDWR
        }

        pub fn can_write(&self) -> bool {
            let access_mode = self.bits() & Self::ACCESS_MASK.bits();
            access_mode == uapi::O_WRONLY || access_mode == uapi::O_RDWR
        }
    }
}

pub use inner_flags::{AtomicOpenFlags, OpenFlags};
