// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;
use bitflags::bitflags;

bitflags! {
    /// The flags that are used to seal a memory-backed file descriptor (memfd)
    /// and prevent it from being modified.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct SealFlags: u32 {
      const FUTURE_WRITE = uapi::F_SEAL_FUTURE_WRITE;
      const WRITE = uapi::F_SEAL_WRITE;
      const GROW = uapi::F_SEAL_GROW;
      const SHRINK = uapi::F_SEAL_SHRINK;
      const SEAL = uapi::F_SEAL_SEAL;
      const NO_EXEC = uapi::F_SEAL_EXEC;
    }
}
