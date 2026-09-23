// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! RISC-V feature set.
//!
//! This mirrors `arch::RiscvFeature` / `arch::RiscvFeatures` in
//! `<lib/arch/riscv64/feature.h>`.  Only the enumerator *order* is shared
//! between the two: the C++ container's layout is private, and Rust obtains its
//! contents through accessors rather than by mirroring it.

/// An enumeration of RISC-V features.
///
/// This is not intended to be exhaustive, but rather to include the features
/// that the kernel currently depends on.
///
/// The discriminants must match `arch::RiscvFeature` in
/// `<lib/arch/riscv64/feature.h>`, which is where they are checked.
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RiscvFeature {
    /// Extension for supervisor based timer.
    Sstc = 0,
    /// SuperVisor extension for Page-Based Memory Types.
    Svpbmt = 1,
    /// The standard V extension for vector support.
    Vector = 2,
    /// Cache-Block Management Operations.
    Zicbom = 3,
    /// Cache-Block Zero Operations.
    Zicboz = 4,
    /// Counter CSRs are accessible.
    Zicntr = 5,
}

impl RiscvFeature {
    /// Every feature, in discriminant order.
    pub const ALL: [RiscvFeature; 6] = [
        RiscvFeature::Sstc,
        RiscvFeature::Svpbmt,
        RiscvFeature::Vector,
        RiscvFeature::Zicbom,
        RiscvFeature::Zicboz,
        RiscvFeature::Zicntr,
    ];
}

/// A simple container abstraction around the set of RISC-V features.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct RiscvFeatures {
    bits: u64,
    cbom_size: u16,
    cboz_size: u16,
}

impl RiscvFeatures {
    /// Constructs a feature set from a raw bitmask, in which bit `n` records
    /// support for the feature whose discriminant is `n`.
    pub const fn from_bits(bits: u64) -> Self {
        Self { bits, cbom_size: 0, cboz_size: 0 }
    }

    /// The raw bitmask, in which bit `n` records support for the feature whose
    /// discriminant is `n`.
    pub const fn bits(&self) -> u64 {
        self.bits
    }

    /// Whether a given feature is supported.
    pub const fn supports(&self, feature: RiscvFeature) -> bool {
        (self.bits & (1 << (feature as u32))) != 0
    }

    /// Sets support for a given feature.
    pub fn set(&mut self, feature: RiscvFeature, supported: bool) -> &mut Self {
        if supported {
            self.bits |= 1 << (feature as u32);
        } else {
            self.bits &= !(1 << (feature as u32));
        }
        self
    }

    /// The cache block size for Cache-Block Management Operations, in bytes.
    pub const fn cbom_size(&self) -> u16 {
        self.cbom_size
    }

    /// The cache block size for Cache-Block Zero Operations, in bytes.
    pub const fn cboz_size(&self) -> u16 {
        self.cboz_size
    }

    /// Sets the Cache-Block Management Operations block size.
    pub fn set_cbom_size(&mut self, size: u16) -> &mut Self {
        self.cbom_size = size;
        self
    }

    /// Sets the Cache-Block Zero Operations block size.
    pub fn set_cboz_size(&mut self, size: u16) -> &mut Self {
        self.cboz_size = size;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features() {
        let mut features = RiscvFeatures::default();
        assert!(!features.supports(RiscvFeature::Vector));

        features.set(RiscvFeature::Vector, true).set(RiscvFeature::Zicbom, true);
        assert!(features.supports(RiscvFeature::Vector));
        assert!(features.supports(RiscvFeature::Zicbom));
        assert!(!features.supports(RiscvFeature::Zicboz));

        features.set(RiscvFeature::Vector, false);
        assert!(!features.supports(RiscvFeature::Vector));
        assert!(features.supports(RiscvFeature::Zicbom));
    }

    #[test]
    fn bits_round_trip() {
        let mut features = RiscvFeatures::default();
        for feature in RiscvFeature::ALL {
            features.set(feature, true);
        }
        assert_eq!(features.bits(), 0b11_1111);
        assert_eq!(RiscvFeatures::from_bits(features.bits()), RiscvFeatures::from_bits(0b11_1111));
    }
}
