// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;

bitflags! {
    /// Flags describing properties of a registered filesystem type.
    /// Corresponds to Linux `FS_*` flags.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct FileSystemTypeFlags: u32 {
        /// Filesystem requires a block device (corresponds to Linux `FS_REQUIRES_DEV`).
        const REQUIRES_DEV = 1;
    }
}
