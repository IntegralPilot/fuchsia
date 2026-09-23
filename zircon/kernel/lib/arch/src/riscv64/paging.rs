// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::{bitfield_repr, layout};
use regio::riscv64::{Csr, encoding};

/// [riscv/priv]: 4.1.11  Supervisor Address Translation and Protection Register (satp)
pub const SATP: Csr<encoding::satp, SupervisorAddressTranslationAndProtection> = Csr::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum TranslationMode {
    Bare = 0, // No translation or protection.
    // 1-7 are reserved for standard use.
    Sv39 = 8,
    Sv48 = 9,
    Sv57 = 10,
    Sv64 = 11,
    // 12-13 are reserved for standard use.
    // 14-15 are reserved for custom use.
}

layout!({
    /// The layout of [`SATP`].
    pub struct SupervisorAddressTranslationAndProtection(u64);
    {
        let mode @ 63..60: TranslationMode;
        let asid @ 59..44;
        let ppn @ 43..0;
    }
});

impl SupervisorAddressTranslationAndProtection {
    /// Returns the root page table physical address (PPN << 12).
    pub fn root_address(&self) -> u64 {
        self.ppn() << 12
    }

    /// Sets the root page table physical address (must be 4KiB-aligned).
    pub fn set_root_address(&mut self, addr: u64) -> &mut Self {
        assert!(addr & 0xfff == 0, "root address must be 4KiB-aligned");
        self.set_ppn(addr >> 12)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn satp() {
        let mut satp = SupervisorAddressTranslationAndProtection::new();
        satp.set_mode(TranslationMode::Sv39).set_asid(0x12).set_root_address(0x8000_0000);
        assert_eq!(satp.mode(), TranslationMode::Sv39);
        assert_eq!(satp.asid(), 0x12);
        assert_eq!(satp.root_address(), 0x8000_0000);
        assert_eq!(satp.ppn(), 0x8_0000);
    }
}
