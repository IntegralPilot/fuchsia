// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::{bitfield_repr, layout, multilayout};
use regio::arm64::{SysReg, spec};

/// [arm/v8]: D13.2.33  CTR_EL0, Cache Type Register.
pub const CTR_EL0: SysReg<spec::CTR_EL0, CacheTypeRegister> = SysReg::new();

/// L1 instruction cache policy.
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum L1ICachePolicy {
    Vpipt = 0b00,
    Aivivt = 0b01,
    Vipt = 0b10,
    Pipt = 0b11,
}

layout!({
    /// The layout of [`CTR_EL0`].
    pub struct CacheTypeRegister(u64);
    {
        let __ @ 63..38;
        let tmin_line @ 37..32;
        let __ @ 31 = 1;
        let __ @ 30;
        let dic @ 29;
        let idc @ 28;
        let cwg @ 27..24;
        let erg @ 23..20;

        /// log2 of the number of words in the smallest data cache line.
        let dmin_line @ 19..16;

        let l1_ip @ 15..14: L1ICachePolicy;
        let __ @ 13..4;

        /// log2 of the number of words in the smallest instruction cache line.
        let imin_line @ 3..0;
    }
});

impl CacheTypeRegister {
    /// Returns the smallest data cache line size in bytes.
    pub const fn dcache_line_size(&self) -> usize {
        (1 << self.dmin_line()) * size_of::<u32>()
    }

    /// Returns the smallest instruction cache line size in bytes.
    pub const fn icache_line_size(&self) -> usize {
        (1 << self.imin_line()) * size_of::<u32>()
    }
}

/// [arm/v8]: D13.2.36  DCZID_EL0, Data Cache Zero ID register
pub const DCZID_EL0: SysReg<spec::DCZID_EL0, DataCacheZeroIdRegister> = SysReg::new();

layout!({
    /// The layout of [`DCZID_EL0`].
    pub struct DataCacheZeroIdRegister(u64);
    {
        let __ @ 63..5;
        let dzp @ 4;
        let bz @ 3..0;
    }
});

impl DataCacheZeroIdRegister {
    /// Returns the block size for DC ZVA in bytes.
    pub fn zva_line_size(&self) -> usize {
        (1 << self.bz()) * size_of::<u32>()
    }
}

/// [arm/sysreg]/clidr_el1: CLIDR_EL1, Cache Level ID Register
pub const CLIDR_EL1: SysReg<spec::CLIDR_EL1, CacheLevelIdRegister> = SysReg::new();

/// The type of cache implemented at a given level, per the `ctype<n>` fields
/// of [`CacheLevelIdRegister`].
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum CacheType {
    /// No cache at this level; no caches exist at higher levels either.
    None = 0b000,
    Instruction = 0b001,
    Data = 0b010,
    /// Separate instruction and data caches.
    Separate = 0b011,
    Unified = 0b100,
}

impl CacheType {
    pub const fn has_instruction_cache(self) -> bool {
        matches!(self, Self::Instruction | Self::Separate)
    }

    pub const fn has_data_cache(self) -> bool {
        matches!(self, Self::Data | Self::Separate)
    }
}

/// The type of allocation tag cache implemented at a given level, per the
/// `ttype<n>` fields of [`CacheLevelIdRegister`] (FEAT_MTE2).
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum TagCacheType {
    None = 0b00,
    /// Separate allocation tag cache.
    Separate = 0b01,
    /// Unified allocation tag and data cache, with tags and data in unified
    /// lines.
    UnifiedLines = 0b10,
    /// Unified allocation tag and data cache, with tags and data in separate
    /// lines.
    SeparateLines = 0b11,
}

layout!({
    /// The layout of [`CLIDR_EL1`].
    pub struct CacheLevelIdRegister(u64);
    {
        let __ @ 63..47;
        let ttype7 @ 46..45: TagCacheType; // Tag cache type at L7
        let ttype6 @ 44..43: TagCacheType;
        let ttype5 @ 42..41: TagCacheType;
        let ttype4 @ 40..39: TagCacheType;
        let ttype3 @ 38..37: TagCacheType;
        let ttype2 @ 36..35: TagCacheType;
        let ttype1 @ 34..33: TagCacheType;
        let icb @ 32..30; // Inner cache boundary
        let lou_u @ 29..27; // Level of Unification Uniprocessor
        let loc @ 26..24; // Level of Coherence
        let lou_is @ 23..21; // Level of Unification Inner Shareable
        let ctype7 @ 20..18: CacheType; // Cache type at L7
        let ctype6 @ 17..15: CacheType;
        let ctype5 @ 14..12: CacheType;
        let ctype4 @ 11..9: CacheType;
        let ctype3 @ 8..6: CacheType;
        let ctype2 @ 5..3: CacheType;
        let ctype1 @ 2..0: CacheType;
    }
});

impl CacheLevelIdRegister {
    /// The number of cache levels described by the register.
    pub const MAX_LEVELS: usize = 7;

    /// Returns the type of cache at `level`, which is zero-based (L1 is level
    /// 0) to match the `level` field of [`CacheSizeSelectionRegister`].
    ///
    /// Panics if `level` is not below [`Self::MAX_LEVELS`] or the field holds
    /// a reserved value.
    pub fn cache_type(&self, level: usize) -> CacheType {
        match level {
            0 => self.ctype1(),
            1 => self.ctype2(),
            2 => self.ctype3(),
            3 => self.ctype4(),
            4 => self.ctype5(),
            5 => self.ctype6(),
            6 => self.ctype7(),
            _ => panic!("cache level {level} out of range"),
        }
    }

    /// Returns the type of allocation tag cache at `level`, which is
    /// zero-based (L1 is level 0).
    ///
    /// Panics if `level` is not below [`Self::MAX_LEVELS`].
    pub fn tag_cache_type(&self, level: usize) -> TagCacheType {
        match level {
            0 => self.ttype1(),
            1 => self.ttype2(),
            2 => self.ttype3(),
            3 => self.ttype4(),
            4 => self.ttype5(),
            5 => self.ttype6(),
            6 => self.ttype7(),
            _ => panic!("cache level {level} out of range"),
        }
    }
}

/// [arm/sysreg]/ccsidr_el1: CCSIDR_EL1, Current Cache Size ID Register
///
/// Layout without FEAT_CCIDX; use [`CCSIDR_EL1_CCIDX`] when
/// `ID_AA64MMFR2_EL1.ccidx` reports the revised format.
pub const CCSIDR_EL1: SysReg<spec::CCSIDR_EL1, CacheSizeIdRegister> = SysReg::new();

/// [arm/sysreg]/ccsidr_el1: CCSIDR_EL1, Current Cache Size ID Register
///
/// Layout with FEAT_CCIDX.
pub const CCSIDR_EL1_CCIDX: SysReg<spec::CCSIDR_EL1, CacheSizeIdRegisterCcidx> = SysReg::new();

multilayout!({
    /// The layout of [`CCSIDR_EL1`].
    #[bitrs(legacy)]
    pub struct CacheSizeIdRegister(u64);

    /// The layout of [`CCSIDR_EL1_CCIDX`].
    #[bitrs(ccidx)]
    pub struct CacheSizeIdRegisterCcidx(u64);

    #[legacy]
    {
        let __ @ 63..28;
        let num_sets @ 27..13; // Number of sets, minus 1
        let associativity @ 12..3; // Associativity, minus 1
    }

    #[ccidx]
    {
        let __ @ 63..56;
        let num_sets @ 55..32; // Number of sets, minus 1
        let __ @ 31..24;
        let associativity @ 23..3; // Associativity, minus 1
    }

    {
        /// log2 of the number of words in a cache line, minus 2.
        let line_size @ 2..0;
    }
});

macro_rules! impl_cache_size_id {
    ($($layout:ty),*) => {
        $(
            impl $layout {
                /// Returns the number of sets.
                pub const fn sets(&self) -> u32 {
                    self.num_sets() as u32 + 1
                }

                /// Returns the associativity (number of ways).
                pub const fn ways(&self) -> u32 {
                    self.associativity() as u32 + 1
                }

                /// Returns the cache line size in bytes.
                pub const fn line_size_bytes(&self) -> usize {
                    1 << (self.line_size() as usize + 4)
                }
            }
        )*
    };
}

impl_cache_size_id!(CacheSizeIdRegister, CacheSizeIdRegisterCcidx);

/// [arm/sysreg]/csselr_el1: CSSELR_EL1, Cache Size Selection Register
pub const CSSELR_EL1: SysReg<spec::CSSELR_EL1, CacheSizeSelectionRegister> = SysReg::new();

layout!({
    /// The layout of [`CSSELR_EL1`].
    pub struct CacheSizeSelectionRegister(u64);
    {
        let __ @ 63..5;
        let tnd @ 4; // Allocation tag not data (FEAT_MTE2)
        let level @ 3..1; // Cache level, zero-based (L1 is 0)
        let ind @ 0; // Instruction not data
    }
});

impl CacheSizeSelectionRegister {
    /// Returns a value selecting the instruction or data cache at `level`,
    /// which is zero-based (L1 is level 0).
    pub fn select(level: u8, instruction: bool) -> Self {
        *Self::new().set_level(level).set_ind(instruction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_type_el0() {
        let ctr = *CacheTypeRegister::new().set_dmin_line(4).set_imin_line(4);
        assert_eq!(ctr.dcache_line_size(), 64);
        assert_eq!(ctr.icache_line_size(), 64);
    }

    #[test]
    fn data_cache_zero_id_el0() {
        let dczid = *DataCacheZeroIdRegister::new().set_bz(4);
        assert_eq!(dczid.zva_line_size(), 64);
    }

    #[test]
    fn cache_level_id_el1() {
        let clidr = *CacheLevelIdRegister::new()
            .set_ctype1(CacheType::Separate)
            .set_ctype2(CacheType::Unified)
            .set_ttype2(TagCacheType::UnifiedLines)
            .set_loc(2)
            .set_lou_is(1);
        assert_eq!(clidr.cache_type(0), CacheType::Separate);
        assert_eq!(clidr.cache_type(1), CacheType::Unified);
        assert_eq!(clidr.cache_type(2), CacheType::None);
        assert_eq!(clidr.tag_cache_type(0), TagCacheType::None);
        assert_eq!(clidr.tag_cache_type(1), TagCacheType::UnifiedLines);
        assert_eq!(clidr.loc(), 2);
        assert_eq!(clidr.lou_is(), 1);
        assert!(CacheType::Separate.has_instruction_cache());
        assert!(CacheType::Separate.has_data_cache());
        assert!(!CacheType::Instruction.has_data_cache());
        assert!(!CacheType::Unified.has_data_cache());
    }

    #[test]
    fn cache_size_id_el1() {
        let ccsidr =
            *CacheSizeIdRegister::new().set_num_sets(63).set_associativity(3).set_line_size(2);
        assert_eq!(ccsidr.sets(), 64);
        assert_eq!(ccsidr.ways(), 4);
        assert_eq!(ccsidr.line_size_bytes(), 64);

        let ccsidr = *CacheSizeIdRegisterCcidx::new()
            .set_num_sets(2047)
            .set_associativity(15)
            .set_line_size(3);
        assert_eq!(ccsidr.sets(), 2048);
        assert_eq!(ccsidr.ways(), 16);
        assert_eq!(ccsidr.line_size_bytes(), 128);
    }

    #[test]
    fn cache_size_selection_el1() {
        let csselr = CacheSizeSelectionRegister::select(2, true);
        assert_eq!(csselr.bits(), 0b101);
        assert_eq!(csselr.level(), 2);
        assert!(csselr.ind());
    }
}
