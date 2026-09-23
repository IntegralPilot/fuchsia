// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use starnix_uapi::{PROT_EXEC, PROT_GROWSDOWN, PROT_READ, PROT_WRITE};

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ProtectionFlags: u32 {
      const READ = PROT_READ;
      const WRITE = PROT_WRITE;
      const EXEC = PROT_EXEC;
      const GROWSDOWN = PROT_GROWSDOWN;
    }
}

impl ProtectionFlags {
    pub const ACCESS_FLAGS: Self =
        Self::from_bits_truncate(Self::READ.bits() | Self::WRITE.bits() | Self::EXEC.bits());

    pub fn to_vmar_flags(self) -> zx::VmarFlags {
        let mut vmar_flags = zx::VmarFlags::empty();
        if self.contains(ProtectionFlags::READ) {
            vmar_flags |= zx::VmarFlags::PERM_READ;
        }
        if self.contains(ProtectionFlags::WRITE) {
            vmar_flags |= zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE;
        }
        if self.contains(ProtectionFlags::EXEC) {
            vmar_flags |= zx::VmarFlags::PERM_EXECUTE | zx::VmarFlags::PERM_READ_IF_XOM_UNSUPPORTED;
        }
        vmar_flags
    }

    pub fn from_vmar_flags(vmar_flags: zx::VmarFlags) -> ProtectionFlags {
        let mut prot_flags = ProtectionFlags::empty();
        if vmar_flags.contains(zx::VmarFlags::PERM_READ) {
            prot_flags |= ProtectionFlags::READ;
        }
        if vmar_flags.contains(zx::VmarFlags::PERM_WRITE) {
            prot_flags |= ProtectionFlags::WRITE;
        }
        if vmar_flags.contains(zx::VmarFlags::PERM_EXECUTE) {
            prot_flags |= ProtectionFlags::EXEC;
        }
        prot_flags
    }

    pub fn from_access_bits(prot: u32) -> Option<Self> {
        if let Some(flags) = ProtectionFlags::from_bits(prot) {
            if flags.contains(Self::ACCESS_FLAGS.complement()) { None } else { Some(flags) }
        } else {
            None
        }
    }

    pub fn access_flags(&self) -> Self {
        *self & Self::ACCESS_FLAGS
    }
}
