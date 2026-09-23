// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::{bitfield_repr, layout, multilayout};
use regio::arm64::{SysReg, spec};

use super::PhysicalAddressSize;

/// [arm/sysreg]/currentel: CurrentEL, Current Exception Level
pub const CURRENT_EL: SysReg<spec::CurrentEL, CurrentEl> = SysReg::new();

layout!({
    /// The layout of [`CURRENT_EL`].
    pub struct CurrentEl(u64);
    {
        let __ @ 63..4;
        let el @ 3..2;
        let __ @ 1..0;
    }
});

/// [arm/sysreg]/mpidr_el1: Multiprocessor Affinity Register (EL1)
pub const MPIDR_EL1: SysReg<spec::MPIDR_EL1, MultiprocessorAffinityRegister> = SysReg::new();

layout!({
    /// The layout of [`MPIDR_EL1`] and `VMPIDR_EL2`.
    pub struct MultiprocessorAffinityRegister(u64);
    {
        let __ @ 63..40;
        let aff3 @ 39..32; // Affinity level 3
        let __ @ 31 = 1;
        let u @ 30; // Uniprocessor system
        let __ @ 29..25;
        let mt @ 24; // Multithreading
        let aff2 @ 23..16; // Affinity level 2
        let aff1 @ 15..8; // Affinity level 1
        let aff0 @ 7..0; // Affinity level 0
    }
});

impl MultiprocessorAffinityRegister {
    const AFFINITY_MASK: u64 = MultiprocessorAffinityRegister::AFF3_MASK
        | MultiprocessorAffinityRegister::AFF2_MASK
        | MultiprocessorAffinityRegister::AFF1_MASK
        | MultiprocessorAffinityRegister::AFF0_MASK;

    /// Returns the full affinity value (bits [39:32], [23:16], [15:8], and [7:0]).
    pub fn affinity(&self) -> u64 {
        self.bits() & Self::AFFINITY_MASK
    }
}

/// [arm/sysreg]/sctlr_el1: System Control Register (EL1)
pub const SCTLR_EL1: SysReg<spec::SCTLR_EL1, SystemControlRegisterEl1> = SysReg::new();

/// [arm/sysreg]/sctlr_el2: System Control Register (EL2)
pub const SCTLR_EL2: SysReg<spec::SCTLR_EL2, SystemControlRegisterEl2> = SysReg::new();

/// [arm/sysreg]/sctlr_el3: System Control Register (EL3)
pub const SCTLR_EL3: SysReg<spec::SCTLR_EL3, SystemControlRegisterEl3> = SysReg::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum TagCheckFault {
    None = 0b00,            // Faults have no effect.
    Synchronous = 0b01,     // All faults cause a synchronous exception.
    Asynchronous = 0b10,    // All faults accumulate asynchronously.
    SynchronousRead = 0b11, // Synchronous for read, asynchronous for write.
}

multilayout!({
    /// The layout of [`SCTLR_EL1`].
    #[bitrs(el1)]
    pub struct SystemControlRegisterEl1(u64);

    /// The layout of [`SCTLR_EL2`].
    #[bitrs(el2)]
    pub struct SystemControlRegisterEl2(u64);

    /// The layout of [`SCTLR_EL3`].
    #[bitrs(el3)]
    pub struct SystemControlRegisterEl3(u64);

    // EL1, EL2, EL3
    {
        let spintmask @ 62;
        let nmi @ 61;
        let tsco @ 59;
        let tme @ 53;
        let tmt @ 51;
        let dsbss @ 44;
        let ata @ 43;
        let tcf @ 41..40: TagCheckFault;
        let itfsb @ 37;
        let bt @ 36;
        let __ @ 34;
        let enia @ 31;
        let enib @ 30;
        let enda @ 27;
        let ee @ 25;
        let eis @ 22;
        let iesb @ 21;
        let wxn @ 19;
        let __ @ 17;
        let dze @ 14;
        let endb @ 13;
        let i @ 12;
        let eos @ 11;
        let naa @ 6;
        let sa @ 3;
        let c @ 2;
        let a @ 1;
        let m @ 0;
    }

    // EL1, EL2
    #[any(el1, el2)]
    {
        let tidcp @ 63;
        let entp2 @ 60;
        let tsco0 @ 58;
        let epan @ 57;
        let enals @ 56;
        let enas0 @ 55;
        let enasr @ 54;
        let tme0 @ 52;
        let tmt0 @ 50;
        let mscen @ 33;
        let cmow @ 32;
    }

    // EL1
    #[el1]
    {
        let twedel @ 49..46;
        let tweden @ 45;
        let ata0 @ 42;
        let tcf0 @ 39..38: TagCheckFault;
        let bt0 @ 35;
        let lsmaoe @ 29;
        let ntlsmd @ 28;
        let uci @ 26;
        let e0e @ 24;
        let span @ 23;
        let tscxt @ 20;
        let ntwe @ 18;
        let ntwi @ 16;
        let uct @ 15;
        let enrctx @ 10;
        let uma @ 9;
        let sed @ 8;
        let itd @ 7;
        let cp15ben @ 5;
        let sa0 @ 4;
    }
});

impl SystemControlRegisterEl1 {
    /// Returns the minimum delay in cycles if TWEDEn is enabled.
    pub fn twedel_cycles(&self) -> Option<u64> {
        if self.tweden() { Some(1u64 << self.twedel() << 8) } else { None }
    }
}

/// [arm/sysreg]/sctlr2_el1: System Control Register 2 (EL1)
pub const SCTLR2_EL1: SysReg<spec::SCTLR2_EL1, SystemControlRegister2El1> = SysReg::new();

/// [arm/sysreg]/sctlr2_el2: System Control Register 2 (EL2)
pub const SCTLR2_EL2: SysReg<spec::SCTLR2_EL2, SystemControlRegister2El2> = SysReg::new();

/// [arm/sysreg]/sctlr2_el3: System Control Register 2 (EL3)
pub const SCTLR2_EL3: SysReg<spec::SCTLR2_EL3, SystemControlRegister2El3> = SysReg::new();

multilayout!({
    /// The layout of [`SCTLR2_EL1`].
    #[bitrs(el1)]
    pub struct SystemControlRegister2El1(u64);

    /// The layout of [`SCTLR2_EL2`].
    #[bitrs(el2)]
    pub struct SystemControlRegister2El2(u64);

    /// The layout of [`SCTLR2_EL3`].
    #[bitrs(el3)]
    pub struct SystemControlRegister2El3(u64);

    // EL1, EL2, EL3
    {
        let __ @ 63..7;
        let enanerr @ 4;
        let enaderr @ 3;
        let __ @ 0;
    }

    // EL1, EL2
    #[any(el1, el2)]
    {
        let enidcp128 @ 6;
        let ease @ 5;
        let nmea @ 2;
    }

    // EL2, EL3
    #[any(el2, el3)]
    {
        let emec @ 1;
    }
});

/// [arm/sysreg]/scr_el3: Secure Configuration Register
pub const SCR_EL3: SysReg<spec::SCR_EL3, SecureConfigurationRegister> = SysReg::new();

layout!({
    /// The layout of [`SCR_EL3`].
    pub struct SecureConfigurationRegister(u64);
    {
        let __ @ 63..39;
        let hxen @ 38;
        let aden @ 37;
        let enas0 @ 36;
        let amvoffen @ 35;
        let __ @ 34;
        let twedel @ 33..30;
        let tweden @ 29;
        let ecven @ 28;
        let fgten @ 27;
        let ata @ 26;
        let enscxt @ 25;
        let __ @ 24..22;
        let fien @ 21;
        let nmea @ 20;
        let ease @ 19;
        let eel2 @ 18;
        let api @ 17;
        let apk @ 16;
        let terr @ 15;
        let tlor @ 14;
        let twe @ 13;
        let twi @ 12;
        let st @ 11;
        let rw @ 10;
        let sif @ 9;
        let hce @ 8;
        let smd @ 7;
        let __ @ 6;
        let __ @ 5..4 = 0b11;
        let ea @ 3;
        let fiq @ 2;
        let irq @ 1;
        let ns @ 0;
    }
});

/// The cache shareability attribute for memory regions, as defined by the
/// TCR_ELx.SHn fields.
///
/// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
/// [arm/v8]: D13.2.121 TCR_EL2, Translation Control Register (EL2)
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum ShareabilityAttribute {
    None = 0b00,
    // 0b01 is reserved.
    Outer = 0b10,
    Inner = 0b11,
}

/// The cacheability attribute of a normal memory regions, as defined by the
/// TCR_ELx.IRGNn and TCR_ELx.ORGNn fields.
///
/// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
/// [arm/v8]: D13.2.121 TCR_EL2, Translation Control Register (EL2)
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum CacheabilityAttribute {
    NonCacheable = 0b00,
    WriteBackReadWriteAllocate = 0b01,
    WriteThroughReadAllocate = 0b10,
    WriteBackReadAllocate = 0b11,
}

/// Granule size values for the TCR_EL1 and TCR_EL2 fields.
///
/// WARNING: The encodings for the TG0 field and TG1 field are different.
///
/// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum TcrTg0Value {
    Tg4KiB = 0b00,
    Tg16KiB = 0b10,
    Tg64KiB = 0b01,
}

/// Granule size values for the TCR_EL1.TG1 field.
///
/// WARNING: The encodings for the TG0 field and TG1 field are different.
///
/// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum TcrTg1Value {
    Tg16KiB = 0b01,
    Tg4KiB = 0b10,
    Tg64KiB = 0b11,
}

/// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
pub const TCR_EL1: SysReg<spec::TCR_EL1, TranslationControlRegisterEl1> = SysReg::new();

/// [arm/v8]: D13.2.121 TCR_EL2, Translation Control Register (EL2)
pub const TCR_EL2: SysReg<spec::TCR_EL2, TranslationControlRegisterEl2> = SysReg::new();

multilayout!({
    /// The layout of [`TCR_EL1`].
    #[bitrs(el1)]
    pub struct TranslationControlRegisterEl1(u64);

    /// The layout of [`TCR_EL2`].
    #[bitrs(el2)]
    pub struct TranslationControlRegisterEl2(u64);

    // EL1, EL2.
    {
        let __ @ 63..62;
        let tg0 @ 15..14: TcrTg0Value;
        let sh0 @ 13..12: ShareabilityAttribute;
        let orgn0 @ 11..10: CacheabilityAttribute;
        let irgn0 @ 9..8: CacheabilityAttribute;
        let t0sz @ 5..0;
    }

    #[el1]
    {
        let mtx1 @ 61;
        let mtx0 @ 60;
        let ds @ 59;
        let tcma1 @ 58;
        let tcma0 @ 57;
        let e0pd1 @ 56;
        let e0pd0 @ 55;
        let nfd1 @ 54;
        let nfd0 @ 53;
        let tbid1 @ 52;
        let tbid0 @ 51;
        let hwu162 @ 50;
        let hwu161 @ 49;
        let hwu160 @ 48;
        let hwu159 @ 47;
        let hwu062 @ 46;
        let hwu061 @ 45;
        let hwu060 @ 44;
        let hwu059 @ 43;
        let hpd1 @ 42;
        let hpd0 @ 41;
        let hd @ 40;
        let ha @ 39;
        let tbi1 @ 38;
        let tbi0 @ 37;
        let asid_size @ 36; // `as` is reserved
        let __ @ 35;
        let ips @ 34..32: PhysicalAddressSize;
        let tg1 @ 31..30: TcrTg1Value;
        let sh1 @ 29..28: ShareabilityAttribute;
        let orgn1 @ 27..26: CacheabilityAttribute;
        let irgn1 @ 25..24: CacheabilityAttribute;
        let epd1 @ 23;
        let a1 @ 22;
        let t1sz @ 21..16;
        let epd0 @ 7;
        let __ @ 6;
    }

    #[el2]
    {
        let mtx @ 33;
        let ds @ 32;
        let __ @ 31 = 1;
        let tcma @ 30;
        let tbid @ 29;
        let hwu62 @ 28;
        let hwu61 @ 27;
        let hwu60 @ 26;
        let hwu59 @ 25;
        let hpd @ 24;
        let __ @ 23 = 1;
        let hd @ 22;
        let ha @ 21;
        let tbi @ 20;
        let ps @ 18..16: PhysicalAddressSize;
        let __ @ 7..6;
    }
});

/// [arm/v8]: VTCR_EL2, Virtualization Translation Control Register
pub const VTCR_EL2: SysReg<spec::VTCR_EL2, VirtualizationTranslationControlRegister> =
    SysReg::new();

layout!({
    /// The layout of [`VTCR_EL2`].
    pub struct VirtualizationTranslationControlRegister(u64);
    {
        let __ @ 63..46;
        let hdbss @ 45;
        let haft @ 44;
        let __ @ 43..42;
        let tl0 @ 41;
        let gcsh @ 40;
        let __ @ 39;
        let d128 @ 38;
        let s2poe @ 37;
        let s2pie @ 36;
        let tl1 @ 35;
        let assured_only @ 34;
        let sl2 @ 33;
        let ds @ 32;
        let __ @ 31 = 1;
        let nsa @ 30;
        let nsw @ 29;
        let hwu62 @ 28;
        let hwu61 @ 27;
        let hwu60 @ 26;
        let hwu59 @ 25;
        let __ @ 24;
        let __ @ 23 = 1;
        let hd @ 22;
        let ha @ 21;
        let __ @ 20;
        let vs @ 19;
        let ps @ 18..16: PhysicalAddressSize;
        let tg0 @ 15..14: TcrTg0Value;
        let sh0 @ 13..12: ShareabilityAttribute;
        let orgn0 @ 11..10: CacheabilityAttribute;
        let irgn0 @ 9..8: CacheabilityAttribute;
        let sl0 @ 7..6;
        let t0sz @ 5..0;
    }
});

/// [arm/v9]: D23.2.172 TCR2_EL1, Translation Control Register (EL1)
pub const TCR2_EL1: SysReg<spec::TCR2_EL1, ExtendedTranslationControlRegister> = SysReg::new();

layout!({
    /// The layout of [`TCR2_EL1`].
    pub struct ExtendedTranslationControlRegister(u64);
    {
        let __ @ 63..22;
        let fngna1 @ 21; // Force non-global for unassured using TTBR1. (FEAT_THE)
        let fngna0 @ 20; // Force non-global for unassured using TTBR0. (FEAT_THE)
        let __ @ 19;
        let fng1 @ 18; // Force non-global translations for TTBR1. (FEAT_ASID2)
        let fng0 @ 17; // Force non-global translations for TTBR0. (FEAT_ASID2)
        let a2 @ 16; // Enable use of two ASIDs. (FEAT_ASID2)
        let disch1 @ 15; // Disable contiguous bit for start table for TTBR1. (FEAT_D128)
        let disch0 @ 14; // Disable contiguous bit for start table for TTBR0. (FEAT_D128)
        let __ @ 13..12;
        let haft @ 11; // Hardware managed access flag for table descriptors. (FEAT_HAFT)
        let pttwi @ 10; // Permit translation table walk incoherence. (FEAT_THE)
        let __ @ 9..6;
        let d128 @ 5; // Enable 128 bit translation tables. (FEAT_D128)
        let aie @ 4; // Enable attribute indexing extension. (FEAT_AIE)
        let poe @ 3; // Enable permission overlays for privileged accesses. (FEAT_S1POE)
        let e0poe @ 2; // Enable permission overlays for unprivileged accesses. (FEAT_S1POE)
        let pie @ 1; // Enable indirect permission scheme. (FEAT_S1PIE)
        let pnch @ 0; // Enable protected attribute enable. (FEAT_THE)
    }
});

/// [arm/v8]: D13.2.132 TTBR0_EL1, Translation Table Base Register 0 (EL1)
pub const TTBR0_EL1: SysReg<spec::TTBR0_EL1, TranslationTableBaseRegister> = SysReg::new();

/// [arm/v8]: D13.2.133 TTBR0_EL2, Translation Table Base Register 0 (EL2)
pub const TTBR0_EL2: SysReg<spec::TTBR0_EL2, TranslationTableBaseRegister> = SysReg::new();

/// [arm/v8]: D13.2.134 TTBR0_EL3, Translation Table Base Register 0 (EL3)
pub const TTBR0_EL3: SysReg<spec::TTBR0_EL3, TranslationTableBaseRegister> = SysReg::new();

/// [arm/v8]: D13.2.135 TTBR1_EL1, Translation Table Base Register 1 (EL1)
pub const TTBR1_EL1: SysReg<spec::TTBR1_EL1, TranslationTableBaseRegister> = SysReg::new();

/// [arm/v8]: D13.2.136 TTBR1_EL2, Translation Table Base Register 1 (EL2)
pub const TTBR1_EL2: SysReg<spec::TTBR1_EL2, TranslationTableBaseRegister> = SysReg::new();

/// [arm/v8]: VTTBR_EL2, Virtualization Translation Table Base Register (EL2)
pub const VTTBR_EL2: SysReg<spec::VTTBR_EL2, TranslationTableBaseRegister> = SysReg::new();

layout!({
    /// The layout of [`TTBR0_EL1`], [`TTBR0_EL2`], [`TTBR0_EL3`],
    /// [`TTBR1_EL1`], [`TTBR1_EL2`], [`TTBR1_EL3`], and [`VTTBR_EL2`].
    pub struct TranslationTableBaseRegister(u64);
    {
        let asid @ 63..48;
        #[unshifted]
        let addr @ 47..1; // Bits [47:1] of the root table physical address.
        let cnp @ 0; // Common not private.
    }
});

impl TranslationTableBaseRegister {
    /// VMID field (same bit position as ASID when used for VTTBR_EL2).
    ///
    /// The layout is the same as TTBR0_ELx, but the ASID field is called VMID.
    pub fn vmid(&self) -> u16 {
        self.asid()
    }

    /// Sets VMID field.
    pub fn set_vmid(&mut self, vmid: u16) -> &mut Self {
        self.set_asid(vmid)
    }
}

/// [arm/sysreg]/currentel: DAIF, Interrupt Mask Bits
pub const DAIF: SysReg<spec::DAIF, Daif> = SysReg::new();

layout!({
    /// The layout of [`DAIF`].
    pub struct Daif(u64);
    {
        let __ @ 63..10;
        let d @ 9;
        let a @ 8;
        let i @ 7;
        let f @ 6;
        let __ @ 5..0;
    }
});

/// [arm/sysreg]/vbar_el1: Vector Base Address Register (EL1)
pub const VBAR_EL1: SysReg<spec::VBAR_EL1, VectorBaseAddressRegister> = SysReg::new();

/// [arm/sysreg]/vbar_el2: Vector Base Address Register (EL2)
pub const VBAR_EL2: SysReg<spec::VBAR_EL2, VectorBaseAddressRegister> = SysReg::new();

/// [arm/sysreg]/vbar_el3: Vector Base Address Register (EL3)
pub const VBAR_EL3: SysReg<spec::VBAR_EL3, VectorBaseAddressRegister> = SysReg::new();

layout!({
    /// The layout of [`VBAR_EL1`], [`VBAR_EL2`], and [`VBAR_EL3`].
    pub struct VectorBaseAddressRegister(u64);
    {
        #[unshifted]
        let addr @ 63..11;
        let __ @ 10..0;
    }
});

/// [arm/sysreg]/elr_el1: Exception Link Register (EL1)
pub const ELR_EL1: SysReg<spec::ELR_EL1, ExceptionLinkRegister> = SysReg::new();

/// [arm/sysreg]/elr_el2: Exception Link Register (EL2)
pub const ELR_EL2: SysReg<spec::ELR_EL2, ExceptionLinkRegister> = SysReg::new();

/// [arm/sysreg]/elr_el3: Exception Link Register (EL3)
pub const ELR_EL3: SysReg<spec::ELR_EL3, ExceptionLinkRegister> = SysReg::new();

layout!({
    /// The layout of [`ELR_EL1`], [`ELR_EL2`], and [`ELR_EL3`].
    pub struct ExceptionLinkRegister(u64);
    {
        let pc @ 63..0;
    }
});

/// [arm/sysreg]/sp_el0: Stack Pointer (EL0)
pub const SP_EL0: SysReg<spec::SP_EL0, StackPointerRegister> = SysReg::new();

/// [arm/sysreg]/sp_el1: Stack Pointer (EL1)
pub const SP_EL1: SysReg<spec::SP_EL1, StackPointerRegister> = SysReg::new();

/// [arm/sysreg]/sp_el2: Stack Pointer (EL2)
pub const SP_EL2: SysReg<spec::SP_EL2, StackPointerRegister> = SysReg::new();

layout!({
    /// The layout of [`SP_EL1`], [`SP_EL2`], and [`SP_EL3`].
    pub struct StackPointerRegister(u64);
    {
        let sp @ 63..0;
    }
});

/// [arm/sysreg]/spsr_el1: Saved Program Status Register (El1)
pub const SPSR_EL1: SysReg<spec::SPSR_EL1, SavedProgramStatusRegister> = SysReg::new();

/// [arm/sysreg]/spsr_el2: Saved Program Status Register (El2)
pub const SPSR_EL2: SysReg<spec::SPSR_EL2, SavedProgramStatusRegister> = SysReg::new();

/// [arm/sysreg]/spsr_el3: Saved Program Status Register (El3)
pub const SPSR_EL3: SysReg<spec::SPSR_EL3, SavedProgramStatusRegister> = SysReg::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum SpsrExceptionLevel {
    El0t = 0b0000, // EL0 using SP_EL0
    El1t = 0b0100, // EL1 using SP_EL0
    El1h = 0b0101, // EL1 using SP_EL1
    El2t = 0b1000, // EL2 using SP_EL0
    El2h = 0b1001, // EL2 using SP_EL2
    El3t = 0b1100, // EL3 using SP_EL0
    El3h = 0b1101, // EL3 using SP_EL3
}

layout!({
    /// The layout of [`SPSR_EL1`], [`SPSR_EL2`], and [`SPSR_EL3`].
    ///
    /// These are the assignments when an exception is taken from AArch64 state.
    pub struct SavedProgramStatusRegister(u64);
    {
        let __ @ 63..32;
        let n @ 31;
        let z @ 30;
        let c @ 29;
        let v @ 28;
        let __ @ 27..26;
        let tco @ 25;
        let dit @ 24;
        let uao @ 23;
        let pan @ 22;
        let ss @ 21;
        let il @ 20;
        let __ @ 19..13;
        let ssbs @ 12;
        let btype @ 11..10;
        let d @ 9;
        let a @ 8;
        let i @ 7;
        let f @ 6;
        let __ @ 5;
        let a32 @ 4; // Always zero in this format.
        let m @ 3..0: SpsrExceptionLevel;
    }
});

impl SavedProgramStatusRegister {
    /// EL this exception was taken from.
    pub fn el(self) -> CurrentEl {
        CurrentEl::from((self.m() as u64) & CurrentEl::EL_MASK)
    }

    /// SPSel state at the exception (true if it used SP_ELx).
    pub fn spsel(self) -> bool {
        (self.m() as u8) & 1 != 0
    }
}

/// [arm/sysreg]/nzcv: Condition Flags
pub const NZCV: SysReg<spec::NZCV, Nzcv> = SysReg::new();

layout!({
    /// The layout of [`NZCV`].
    ///
    /// This is a subset of SPSR_ELx that is accessible R/W to everyone.
    pub struct Nzcv(u64);
    {
        let __ @ 63..32;
        let n @ 31;
        let z @ 30;
        let c @ 29;
        let v @ 28;
        let __ @ 27..0;
    }
});

/// [arm/sysreg]/esr_el1: Exception Syndrome Register (El1)
pub const ESR_EL1: SysReg<spec::ESR_EL1, ExceptionSyndromeRegister> = SysReg::new();

/// [arm/sysreg]/esr_el2: Exception Syndrome Register (El2)
pub const ESR_EL2: SysReg<spec::ESR_EL2, ExceptionSyndromeRegister> = SysReg::new();

/// [arm/sysreg]/esr_el3: Exception Syndrome Register (El3)
pub const ESR_EL3: SysReg<spec::ESR_EL3, ExceptionSyndromeRegister> = SysReg::new();

/// Some values are only possible in ESR_EL2 and/or ESR_EL3.
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum ExceptionClass {
    Unknown = 0b000000,
    Wf = 0b000001,         // WF
    Mcr = 0b000011,        // MCR or MRC
    Mcrr = 0b000100,       // MCRR or MRRC
    McrCoproc = 0b000101,  // MCR or MRC (coproc=0b1110)
    Ldc = 0b000110,        // LDC or STC
    Fp = 0b000111,         // SVE or SIMD
    Ld64b = 0b001010,      // LD64B, ST64B, ST64BV, or ST64BVO
    McrrCoproc = 0b001100, // MRRC (coproc==0b1110)
    Bti = 0b001101,        // Branch target exception
    IllegalExecution = 0b001110,
    Svc32 = 0b010001,
    Hvc32 = 0b010010, // EL2, EL3
    Smc32 = 0b010011, // EL2, EL3
    TrappedSysreg128 = 0b010100,
    Svc64 = 0b010101,
    Hvc64 = 0b010110, // EL2, EL3
    Smc64 = 0b010111, // EL2, EL3
    Msr = 0b011000,   // MSR, MRS, or System Instruction
    Sve = 0b011001,
    Eret = 0b011010, // EL2, EL3
    Tstart = 0b011011,
    Pac = 0b011100,
    Sme = 0b011101,
    ImplementationDefined = 0b011111, // EL3
    InstructionAbortLowerEl = 0b100000,
    InstructionAbortSameEl = 0b100001,
    PcAlignment = 0b100010,
    DataAbortLowerEl = 0b100100,
    DataAbortSameEl = 0b100101,
    SpAlignment = 0b100110,
    Mops = 0b100111,
    Fpe32 = 0b101000,
    Fpe64 = 0b101100,
    Gcs = 0b101101,
    Serror = 0b101111,
    BreakpointLowerEl = 0b110000,
    BreakpointSameEl = 0b110001,
    StepLowerEl = 0b110010,
    StepSameEl = 0b110011,
    WatchpointLowerEl = 0b110100,
    WatchpointSameEl = 0b110101,
    Bkpt = 0b111000,        // AArch32 BKPT #<n>
    VectorCatch = 0b111010, // EL2, EL3
    Brk = 0b111100,         // AArch64 BRK #<n>
    Profiling = 0b111101,
}

layout!({
    /// The layout of [`ESR_EL1`], [`ESR_EL2`], and [`ESR_EL3`].
    ///
    /// These are the assignments when an exception is taken from AArch64 state.
    pub struct ExceptionSyndromeRegister(u64);
    {
        let __ @ 63..56;
        let iss2 @ 55..32;
        let ec @ 31..26: ExceptionClass;
        let il @ 25;
        let iss @ 24..0;
    }
});

/// [arm/sysreg]/cptr_el2: Architectural Feature Trap Register (EL2) when `HCR_EL2.E2H == 0`
pub const CPTR_EL2_NO_VHE: SysReg<spec::CPTR_EL2, ArchitecturalFeatureTrapRegisterNoVhe> =
    SysReg::new();

/// [arm/sysreg]/cptr_el2: Architectural Feature Trap Register (EL2) when `HCR_EL2.E2H == 1` (FEAT_VHE)
pub const CPTR_EL2_VHE: SysReg<spec::CPTR_EL2, ArchitecturalFeatureTrapRegisterVhe> = SysReg::new();

/// [arm/sysreg]/cptr_el3: Architectural Feature Trap Register (EL3) when `HCR_EL2.E2H == 0`
pub const CPTR_EL3_NO_VHE: SysReg<spec::CPTR_EL3, ArchitecturalFeatureTrapRegisterNoVhe> =
    SysReg::new();

/// [arm/sysreg]/cptr_el3: Architectural Feature Trap Register (EL3) when `HCR_EL2.E2H == 1` (FEAT_VHE)
pub const CPTR_EL3_VHE: SysReg<spec::CPTR_EL3, ArchitecturalFeatureTrapRegisterVhe> = SysReg::new();

multilayout!({
    /// The layout of [`CPTR_EL2`] and [`CPTR_EL3`] when `HCR_EL2.E2H == 0`.
    #[bitrs(no_e2h)]
    pub struct ArchitecturalFeatureTrapRegisterNoVhe(u64);

    /// The layout of [`CPTR_EL2`] and [`CPTR_EL3`] when `HCR_EL2.E2H == 1` (FEAT_VHE).
    #[bitrs(e2h)]
    pub struct ArchitecturalFeatureTrapRegisterVhe(u64);

    {
        let __ @ 63..32;
        let tcpac @ 31;
        let tam @ 30;
        let __ @ 29;
        let __ @ 27..26;
        let __ @ 23..22;
        let __ @ 19..18;
    }

    #[e2h]
    {
        let tta @ 28;
        let smen @ 25..24;
        let fpen @ 21..20;
        let zen @ 17..16;
        let __ @ 15..0;
    }

    #[no_e2h]
    {
        let __ @ 28;
        let __ @ 25..24;
        let __ @ 21;
        let tta @ 20;
        let __ @ 17..14;
        let __ @ 13 = 1;
        let tsm @ 12;
        let __ @ 11;
        let tfp @ 10;
        let __ @ 9 = 1;
        let tz @ 8;
        let __ @ 7..0;
    }
});

/// [arm/sysreg]/hcr_el2: Hypervisor Configuration register (EL2)
pub const HCR_EL2: SysReg<spec::HCR_EL2, HypervisorConfigurationRegister> = SysReg::new();

layout!({
    /// The layout of [`HCR_EL2`].
    pub struct HypervisorConfigurationRegister(u64);
    {
        let twedel @ 63..60;
        let tweden @ 59;
        let tid5 @ 58;
        let dct @ 57;
        let ata @ 56;
        let ttlbos @ 55;
        let ttlbis @ 54;
        let enscxt @ 53;
        let tocu @ 52;
        let amvoffen @ 51;
        let ticab @ 50;
        let tid4 @ 49;
        let gpf @ 48;
        let fien @ 47;
        let fwb @ 46;
        let nv2 @ 45;
        let at @ 44;
        let nv1 @ 43;
        let nv @ 42;
        let api @ 41;
        let apk @ 40;
        let tme @ 39;
        let miocnce @ 38;
        let tea @ 37;
        let terr @ 36;
        let tlor @ 35;
        let e2h @ 34;
        let id @ 33;
        let cd @ 32;
        let rw @ 31;
        let trvm @ 30;
        let hcd @ 29;
        let tdz @ 28;
        let tge @ 27;
        let tvm @ 26;
        let ttlb @ 25;
        let tpu @ 24;
        let tcpc @ 23;
        let tsw @ 22;
        let tacr @ 21;
        let tidcp @ 20;
        let tsc @ 19;
        let tid3 @ 18;
        let tid2 @ 17;
        let tid1 @ 16;
        let tid0 @ 15;
        let twe @ 14;
        let twi @ 13;
        let dc @ 12;
        let bsu @ 11..10;
        let fb @ 9;
        let vse @ 8;
        let vi @ 7;
        let vf @ 6;
        let amo @ 5;
        let imo @ 4;
        let fmo @ 3;
        let ptw @ 2;
        let swio @ 1;
        let vm @ 0;
    }
});

/// [arm/sysreg]/hcrx_el2: Extended Hypervisor Configuration register (EL2)
pub const HCRX_EL2: SysReg<spec::HCRX_EL2, ExtendedHypervisorConfigurationRegister> = SysReg::new();

layout!({
    /// The layout of [`HCRX_EL2`].
    pub struct ExtendedHypervisorConfigurationRegister(u64);
    {
        let __ @ 63..27; // Safe "pass-through" value for no-op EL2.
        let srmasken @ 26; // if FEAT_SRMASK
        let __ @ 25;
        let pacmen @ 24; // if FEAT_PAuth_LR
        let enfpm @ 23; // if FEAT_FPMR
        let gcsen @ 22; // if FEAT_GCS
        let enidcp128 @ 21; // if FEAT_SYSREG128
        let ensderr @ 20; // if FEAT_ADERR
        let tmea @ 19; // if FEAT_DoubleFault2
        let ensnerr @ 18; // if FEAT_ANERR
        let d128en @ 17; // if FEAT_D128
        let pttwi @ 16; // if FEAT_THE
        let sctlr2en @ 15; // if FEAT_SCTLR2
        let tcr2en @ 14; // if FEAT_TCR2
        let __ @ 13..12;
        let mscen @ 11; // if FEAT_MOPS
        let mce2 @ 10; // if FEAT_MOPS
        let cmow @ 9; // if CEAT_CMOW
        let vfnmi @ 8; // if FEAT_NMI
        let vinmi @ 7; // if FEAT_NMI
        let tallint @ 6; // if FEAT_NMI
        let smpme @ 5; // if FEAT_SME
        let fgtnxs @ 4; // if FEAT_XS
        let fnxs @ 3; // if FEAT_XS
        let enasr @ 2; // if FEAT_LS64_V
        let enals @ 1; // if FEAT_LS64
        let enas0 @ 0; // if FEAT_LS64_ACCDATA
    }
});

/// [arm/sysreg]/cnthctl_el2: Counter-timer Hypervisor Control register (EL2) when `HCR_EL2.E2H == 0`
pub const CNTHCTL_EL2_NO_VHE: SysReg<
    spec::CNTHCTL_EL2,
    CounterTimerHypervisorControlRegisterNoVhe,
> = SysReg::new();

/// [arm/sysreg]/cnthctl_el2: Counter-timer Hypervisor Control register (EL2) when `HCR_EL2.E2H == 1` (FEAT_VHE)
pub const CNTHCTL_EL2_VHE: SysReg<spec::CNTHCTL_EL2, CounterTimerHypervisorControlRegisterVhe> =
    SysReg::new();

multilayout!({
    /// The layout of [`CNTHCTL_EL2`] when `HCR_EL2.E2H == 0`.
    #[bitrs(no_e2h)]
    pub struct CounterTimerHypervisorControlRegisterNoVhe(u64);

    /// The layout of [`CNTHCTL_EL2`] when `HCR_EL2.E2H == 1` (FEAT_VHE).
    #[bitrs(e2h)]
    pub struct CounterTimerHypervisorControlRegisterVhe(u64);

    {
        let __ @ 63..20;
        let cntpmask @ 19;
        let cntvmask @ 18;
        let evntis @ 17;
        let el1nvvct @ 16;
        let el1nvpct @ 15;
        let el1tvct @ 14;
        let el1tvt @ 13;
        let ecv @ 12;
        let evnti @ 7..4;
        let evntdir @ 3;
        let evnten @ 2;
    }

    #[e2h]
    {
        let el1pten @ 11;
        let el1pcten @ 10;
        let el0pten @ 9;
        let el0vten @ 8;
        let el0vcten @ 1;
        let el0pcten @ 0;
    }

    #[no_e2h]
    {
        let __ @ 11..8;
        let el1pcen @ 1;
        let el1pcten @ 0;
    }
});

/// [arm/sysreg]/icc_sre_el1: Interrupt Controller System Register Enable Register (EL1)
pub const ICC_SRE_EL1: SysReg<spec::ICC_SRE_EL1, InterruptControllerSystemRegisterEnableRegister> =
    SysReg::new();

/// [arm/sysreg]/icc_sre_el2: Interrupt Controller System Register Enable Register (EL2)
pub const ICC_SRE_EL2: SysReg<spec::ICC_SRE_EL2, InterruptControllerSystemRegisterEnableRegister> =
    SysReg::new();

/// [arm/sysreg]/icc_sre_el3: Interrupt Controller System Register Enable Register (EL3)
pub const ICC_SRE_EL3: SysReg<spec::ICC_SRE_EL3, InterruptControllerSystemRegisterEnableRegister> =
    SysReg::new();

layout!({
    /// The layout of [`ICC_SRE_EL1`], [`ICC_SRE_EL2`], and [`ICC_SRE_EL3`].
    pub struct InterruptControllerSystemRegisterEnableRegister(u64);
    {
        let __ @ 63..4;
        let enable @ 3;
        let dib @ 2;
        let dfb @ 1;
        let sre @ 0;
    }
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiprocessor_affinity() {
        let mpidr =
            *MultiprocessorAffinityRegister::new().set_aff0(1).set_aff1(2).set_aff2(3).set_aff3(4);
        assert_eq!(mpidr.affinity(), 0x04_0003_0201);
    }
}
