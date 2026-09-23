// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct UnmountFlags: u32 {
        const FORCE = uapi::MNT_FORCE;
        const DETACH = uapi::MNT_DETACH;
        const EXPIRE = uapi::MNT_EXPIRE;
        const NOFOLLOW = uapi::UMOUNT_NOFOLLOW;
    }
}
