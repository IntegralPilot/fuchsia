// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::{bitfield_repr, layout};
use regio::arm64::{SysReg, spec};

/// [arm/sysreg]/ID_AA64ISAR0_EL1: AArch64 Instruction Set Attribute Register 0
pub const ID_AA64ISAR0_EL1: SysReg<spec::ID_AA64ISAR0_EL1, InstructionSetAttributeRegister0> =
    SysReg::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Rndr {
    None = 0b0000,
    Rng = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Tlb {
    None = 0b0000,
    Tlbios = 0b0001,
    Tlbirange = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Ts {
    None = 0b0000,
    FlagM = 0b0001,
    FlagM2 = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Fhm {
    None = 0b0000,
    Fhm = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0DotProd {
    None = 0b0000,
    DotProd = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Sm4 {
    None = 0b0000,
    Sm4 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Sm3 {
    None = 0b0000,
    Sm3 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Sha3 {
    None = 0b0000,
    Sha3 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Rdm {
    None = 0b0000,
    Rdm = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Atomic {
    None = 0b0000,
    Lse = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Crc32 {
    None = 0b0000,
    Crc32 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Sha2 {
    None = 0b0000,
    Sha256 = 0b0001,
    Sha512 = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Sha1 {
    None = 0b0000,
    Sha1 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR0Aes {
    None = 0b0000,
    Aes = 0b0001,
    Pmull = 0b0010,
}

layout!({
    /// The layout of [`ID_AA64ISAR0_EL1`].
    pub struct InstructionSetAttributeRegister0(u64);
    {
        let rndr @ 63..60: IdAa64IsaR0Rndr;
        let tlb @ 59..56: IdAa64IsaR0Tlb;
        let ts @ 55..52: IdAa64IsaR0Ts;
        let fhm @ 51..48: IdAa64IsaR0Fhm;
        let dp @ 47..44: IdAa64IsaR0DotProd;
        let sm4 @ 43..40: IdAa64IsaR0Sm4;
        let sm3 @ 39..36: IdAa64IsaR0Sm3;
        let sha3 @ 35..32: IdAa64IsaR0Sha3;
        let rdm @ 31..28: IdAa64IsaR0Rdm;
        let __ @ 27..24;
        let atomic @ 23..20: IdAa64IsaR0Atomic;
        let crc32 @ 19..16: IdAa64IsaR0Crc32;
        let sha2 @ 15..12: IdAa64IsaR0Sha2;
        let sha1 @ 11..8: IdAa64IsaR0Sha1;
        let aes @ 7..4: IdAa64IsaR0Aes;
        let __ @ 3..0;
    }
});

/// [arm/sysreg]/ID_AA64ISAR1_EL1: AArch64 Instruction Set Attribute Register 1
pub const ID_AA64ISAR1_EL1: SysReg<spec::ID_AA64ISAR1_EL1, InstructionSetAttributeRegister1> =
    SysReg::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Ls64 {
    None = 0b0000,
    Ls64 = 0b0001,
    Ls64V = 0b0010,
    Ls64Accdata = 0b0011,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Xs {
    None = 0b0000,
    Xs = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1I8mm {
    None = 0b0000,
    I8mm = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Dgh {
    None = 0b0000,
    Dgh = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Bf16 {
    None = 0b0000,
    Bf16 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Specres {
    None = 0b0000,
    Specres = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Sb {
    None = 0b0000,
    Sb = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Frintts {
    None = 0b0000,
    Frintts = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Gpi {
    None = 0b0000,
    Gpi = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Gpa {
    None = 0b0000,
    Gpa = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Lrcpc {
    None = 0b0000,
    Lrcpc = 0b0001,
    Lrcpc2 = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Fcma {
    None = 0b0000,
    Fcma = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Jscvt {
    None = 0b0000,
    Jscvt = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Pauth {
    None = 0b0000,
    Pauth = 0b0001,
    PauthEnhanced = 0b0010,
    Pauth2 = 0b0011,
    Fpac = 0b0100,
    FpacCombined = 0b0101,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR1Dpb {
    None = 0b0000,
    Dpb = 0b0001,
    Dpb2 = 0b0010,
}

layout!({
    /// The layout of [`ID_AA64ISAR1_EL1`].
    pub struct InstructionSetAttributeRegister1(u64);
    {
        let ls64 @ 63..60: IdAa64IsaR1Ls64;
        let xs @ 59..56: IdAa64IsaR1Xs;
        let i8mm @ 55..52: IdAa64IsaR1I8mm;
        let dgh @ 51..48: IdAa64IsaR1Dgh;
        let bf16 @ 47..44: IdAa64IsaR1Bf16;
        let specres @ 43..40: IdAa64IsaR1Specres;
        let sb @ 39..36: IdAa64IsaR1Sb;
        let frintts @ 35..32: IdAa64IsaR1Frintts;
        let gpi @ 31..28: IdAa64IsaR1Gpi;
        let gpa @ 27..24: IdAa64IsaR1Gpa;
        let lrcpc @ 23..20: IdAa64IsaR1Lrcpc;
        let fcma @ 19..16: IdAa64IsaR1Fcma;
        let jscvt @ 15..12: IdAa64IsaR1Jscvt;
        let api @ 11..8: IdAa64IsaR1Pauth;
        let apa @ 7..4: IdAa64IsaR1Pauth;
        let dpb @ 3..0: IdAa64IsaR1Dpb;
    }
});

/// [arm/sysreg]/ID_AA64ISAR2_EL1: AArch64 Instruction Set Attribute Register 2
pub const ID_AA64ISAR2_EL1: SysReg<spec::ID_AA64ISAR2_EL1, InstructionSetAttributeRegister2> =
    SysReg::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Ats1a {
    None = 0b0000,
    Ats1a = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Lut {
    None = 0b0000,
    Lut = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Cssc {
    None = 0b0000,
    Cssc = 0b0001, // FEAT_CSSC
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Rprfm {
    None = 0b0000,
    Rprfm = 0b0001, // FEAT_RPRFM
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Prfmslc {
    None = 0b0000,
    Prfmslc = 0b0001, // FEAT_PRFMSLC
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Sysinstr128 {
    None = 0b0000,
    Sysinstr128 = 0b0001, // FEAT_SYSINSTR128
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Sysreg128 {
    None = 0b0000,
    Sysreg128 = 0b0001, // FEAT_SYSREG128
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Clrbhb {
    None = 0b0000,
    Clrbhb = 0b0001, // FEAT_CLRBHB
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2PacFrac {
    None = 0b0000,
    PacFrac = 0b0001, // FEAT_CONSTPACFIELD
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Bc {
    None = 0b0000,
    Bc = 0b0001, // FEAT_HBC
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Mops {
    None = 0b0000,
    Mops = 0b0001, // FEAT_MOPS
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Apa3 {
    None = 0b0000,
    NoEnhanced = 0b0001,   // FEAT_PAuth
    EnhancedPac = 0b0010,  // FEAT_EPAC
    EnhancedPac2 = 0b0011, // FEAT_Pauth2
    Fpac = 0b0100,         // FEAT_FPAC
    FpacCombined = 0b0101, // FEAT_FPACCCOMBINE
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Gpa3 {
    None = 0b0000,
    Gpa3 = 0b0001, // FEAT_PACQARMA3
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Rpres {
    None = 0b0000,
    Rpres = 0b0001, // FEAT_RPRES
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64IsaR2Wfxt {
    None = 0b0000,
    Wfxt = 0b0001, // FEAT_WFxT
}

layout!({
    /// The layout of [`ID_AA64ISAR2_EL1`].
    pub struct InstructionSetAttributeRegister2(u64);
    {
        let ats1a @ 63..60: IdAa64IsaR2Ats1a;
        let lut @ 59..56: IdAa64IsaR2Lut;
        let cssc @ 55..52: IdAa64IsaR2Cssc;
        let rprfm @ 51..48: IdAa64IsaR2Rprfm;
        let __ @ 47..44;
        let prfmslc @ 43..40: IdAa64IsaR2Prfmslc;
        let sysinstr_128 @ 39..36: IdAa64IsaR2Sysinstr128;
        let sysreg_128 @ 35..32: IdAa64IsaR2Sysreg128;
        let clrbhb @ 31..28: IdAa64IsaR2Clrbhb;
        let pac_frac @ 27..24: IdAa64IsaR2PacFrac;
        let bc @ 23..20: IdAa64IsaR2Bc;
        let mops @ 19..16: IdAa64IsaR2Mops;
        let apa3 @ 15..12: IdAa64IsaR2Apa3;
        let gpa3 @ 11..8: IdAa64IsaR2Gpa3;
        let rpres @ 7..4: IdAa64IsaR2Rpres;
        let wfxt @ 3..0: IdAa64IsaR2Wfxt;
    }
});

/// [arm/sysreg]/ID_AA64PFR0_EL1: AArch64 Processor Feature Register 0
pub const ID_AA64PFR0_EL1: SysReg<spec::ID_AA64PFR0_EL1, ProcessorFeatureRegister0> = SysReg::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Csv3 {
    None = 0b0000,
    Csv3 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Csv2 {
    None = 0b0000,
    Csv2 = 0b0001,
    Csv2_2 = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Dit {
    None = 0b0000,
    Dit = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Amu {
    None = 0b0000,
    Amuv1 = 0b0001,
    Amuv1p1 = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Mpam {
    None = 0b0000,
    Mpam = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Sel2 {
    None = 0b0000,
    Sel2 = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Sve {
    None = 0b0000,
    Sve = 0b0001,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Ras {
    None = 0b0000,
    Ras = 0b0001,
    Rasv1p1 = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Gic {
    None = 0b0000,
    Gic4 = 0b0001,
    Gic4_1 = 0b0010,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0Fp {
    Fp = 0b0000,
    Fp16 = 0b0001,
    None = 0b1111,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr0El {
    None = 0b0000, // EL[23] not implemented.
    El64 = 0b0001, // ELn supported in AAarch64 state
    El32 = 0b0010, // ELn supported in AAarch64 or AAarch32 state
}

layout!({
    /// The layout of [`ID_AA64PFR0_EL1`].
    pub struct ProcessorFeatureRegister0(u64);
    {
        let csv3 @ 63..60: IdAa64Pfr0Csv3;
        let csv2 @ 59..56: IdAa64Pfr0Csv2;
        let __ @ 55..52;
        let dit @ 51..48: IdAa64Pfr0Dit;
        let amu @ 47..44: IdAa64Pfr0Amu;
        let mpam @ 43..40: IdAa64Pfr0Mpam;
        let sel2 @ 39..36: IdAa64Pfr0Sel2;
        let sve @ 35..32: IdAa64Pfr0Sve;
        let ras @ 31..28: IdAa64Pfr0Ras;
        let gic @ 27..24: IdAa64Pfr0Gic;
        let advsimd @ 23..20: IdAa64Pfr0Fp;
        let fp @ 19..16: IdAa64Pfr0Fp;
        let el3 @ 15..12: IdAa64Pfr0El;
        let el2 @ 11..8: IdAa64Pfr0El;
        let el1 @ 7..4: IdAa64Pfr0El;
        let el0 @ 3..0: IdAa64Pfr0El;
    }
});

/// [arm/sysreg]/ID_AA64PFR1_EL1: AArch64 Processor Feature Register 1
pub const ID_AA64PFR1_EL1: SysReg<spec::ID_AA64PFR1_EL1, ProcessorFeatureRegister1> = SysReg::new();

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Pfar {
    None = 0b0000,
    Pfar = 0b0001, // FEAT_PFAR
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Df2 {
    None = 0b0000,
    Df2 = 0b0001, // FEAT_DoubleFault2
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Mtex {
    None = 0b0000,
    Mtex = 0b0001, // FEAT_MTE_NO_ADDRESS_TAGS, FEAT_MTE_CANONICAL_TAGS
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1The {
    None = 0b0000,
    The = 0b0001, // FEAT_THE
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Gcs {
    None = 0b0000,
    Gcs = 0b0001, // FEAT_GCS
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1MteFrac {
    Async = 0b0000, // FEAT_MTE_ASYNC
    None = 0b1111,
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Nmi {
    None = 0b0000,
    Nmi = 0b0001, // FEAT_NMI
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Csv2Frac {
    None = 0b0000,
    Csv2_1p1 = 0b0001, // FEAT_CSV2_1p1
    Csv2_1p2 = 0b0010, // FEAT_CSV2_1p2
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1RndrTrap {
    None = 0b0000,
    Trap = 0b0001, // FEAT_RNG_TRAP
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Sme {
    None = 0b0000,
    Sme = 0b0001,  // FEAT_SME
    Sme2 = 0b0010, // FEAT_SME2
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Mte {
    None = 0b0000,
    Mte = 0b0001,  // FEAT_MTE
    Mte2 = 0b0010, // FEAT_MTE2
    Mte3 = 0b0011, // FEAT_MTE3
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Ssbs {
    None = 0b0000,
    Ssbs = 0b0001,  // FEAT_SSBS
    Ssbs2 = 0b0010, // FEAT_SSBS2
}

#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum IdAa64Pfr1El1Bt {
    None = 0b0000,
    Bt = 0b0001, // FEAT_BTI
}

layout!({
    /// The layout of [`ID_AA64PFR1_EL1`].
    pub struct ProcessorFeatureRegister1(u64);
    {
        let pfar @ 63..60: IdAa64Pfr1El1Pfar;
        let df2 @ 59..56: IdAa64Pfr1El1Df2;
        let mtex @ 55..52: IdAa64Pfr1El1Mtex;
        let the @ 51..48: IdAa64Pfr1El1The;
        let gcs @ 47..44: IdAa64Pfr1El1Gcs;
        let mte_frac @ 43..40: IdAa64Pfr1El1MteFrac;
        let nmi @ 39..36: IdAa64Pfr1El1Nmi;
        let csv2_frac @ 35..32: IdAa64Pfr1El1Csv2Frac;
        let rndr_trap @ 31..28: IdAa64Pfr1El1RndrTrap;
        let sme @ 27..24: IdAa64Pfr1El1Sme;
        let __ @ 23..20;
        let mpam_frac @ 19..16;
        let ras_frac @ 15..12;
        let mte @ 11..8: IdAa64Pfr1El1Mte;
        let ssbs @ 7..4: IdAa64Pfr1El1Ssbs;
        let bt @ 3..0: IdAa64Pfr1El1Bt;
    }
});

/// [arm/v9]: D24.2.82 ID_AA64MMFR0_EL1, AArch64 Memory Model Feature Register 0
pub const ID_AA64MMFR0_EL1: SysReg<spec::ID_AA64MMFR0_EL1, MemoryModelFeatureRegister0> =
    SysReg::new();

/// ASID size.
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum AsidSize {
    Asid8Bits = 0b0000,
    Asid16Bits = 0b0010,
}

/// Physical address size.
///
/// The same encoding is used by several registers, including system registers TCR_EL1.IPS,
/// TCR_EL2.PS, and the feature register ID_AA64MMFR0_EL1.PARange.
///
/// [arm/v9]: D24.2.82 ID_AA64MMFR0_EL1, AArch64 Memory Model Feature Register 0
/// [arm/v9]: D24.2.182 Translation Control Register (EL1)
/// [arm/v9]: D24.2.183 Translation Control Register (EL2)
#[bitfield_repr(u8)]
#[derive(Clone, Copy)]
pub enum PhysicalAddressSize {
    Pa32Bits = 0b0000,
    Pa36Bits = 0b0001,
    Pa40Bits = 0b0010,
    Pa42Bits = 0b0011,
    Pa44Bits = 0b0100,
    Pa48Bits = 0b0101,
    Pa52Bits = 0b0110,
    Pa56Bits = 0b0111, // FEAT_D128
}

layout!({
    /// The layout of [`ID_AA64MMFR0_EL1`].
    pub struct MemoryModelFeatureRegister0(u64);
    {
        let ecv @ 63..60; // Enhanced Counter Virtualization (FEAT_ECV)
        let fgt @ 59..56; // Fine-Grained Trap controls (FEAT_FGT)
        let __ @ 55..48;
        let exs @ 47..44; // Disable context synchronizing exceptions (FEAT_ExS)
        let tgran4_2 @ 43..40; // 4 kiB granules at stage 2
        let tgran64_2 @ 39..36; // 64 kiB granules at stage 2
        let tgran16_2 @ 35..32; // 16 kiB granules at stage 2
        let tgran4 @ 31..28; // 4 kiB granules at stage 1
        let tgran64 @ 27..24; // 64 kiB granules at stage 1
        let tgran16 @ 23..20; // 16 kiB granules at stage 1
        let big_end_el0 @ 19..16; // Mixed-endian support for EL0 only
        let sns_mem @ 15..12; // Secure and Non-secure Memory distinguished
        let big_end @ 11..8; // Mixed-endian support
        let asid_bits @ 7..4: AsidSize; // Number of ASID bits
        let pa_range @ 3..0: PhysicalAddressSize; // Supported Physical Address range
    }
});

/// [arm/v9]: D24.2.83 ID_AA64MMFR1_EL1, AArch64 Memory Model Feature Register 1
pub const ID_AA64MMFR1_EL1: SysReg<spec::ID_AA64MMFR1_EL1, MemoryModelFeatureRegister1> =
    SysReg::new();

layout!({
    /// The layout of [`ID_AA64MMFR1_EL1`].
    pub struct MemoryModelFeatureRegister1(u64);
    {
        let __ @ 63..60;
        let cmow @ 59..56; // Cache maintenance instruction permission (FEAT_CMOW)
        let tidcp1 @ 55..52; // TIDCP are implemented (FEAT_TIDCP1)
        let ntlbpa @ 51..48; // Intermediate caching of translation table walks (FEAT_nTLBPA)
        let afp @ 47..44; // FPCR.{AH, FIZ, NEP} support (FEAT_AFP)
        let hcx @ 43..40; // HCRX_EL2 and its associated EL3 trap (FEAT_HCX)
        let ets @ 39..36; // Enhanced Translation Synchronization (FEAT_ETS)
        let twed @ 35..32; // Configurable delayed trapping of WFE (FEAT_TWED)
        let xnx @ 31..28; // Execute-never control at stage 2 (FEAT_XNX)
        let spec_sei @ 27..24; // SError interrupt exceptions from speculative reads
        let pan @ 23..20; // Privileged Access Never (FEAT_PAN, FEAT_PAN2, FEAT_PAN3)
        let lo @ 19..16; // Limited ordering regions (FEAT_LOR)
        let hpds @ 15..12; // Hierarchical Permission Disables (FEAT_HPDS, FEAT_HPDS2)
        let vh @ 11..8; // Virtualization Host Extensions (FEAT_VHE)
        let vmid_bits @ 7..4: AsidSize; // Number of VMID bits (FEAT_VMID16)
        let hafdbs @ 3..0; // Hardware updates to access flag and dirty state (FEAT_HAFDBS)
    }
});

/// [arm/v9]: D24.2.84 ID_AA64MMFR2_EL1, AArch64 Memory Model Feature Register 2
pub const ID_AA64MMFR2_EL1: SysReg<spec::ID_AA64MMFR2_EL1, MemoryModelFeatureRegister2> =
    SysReg::new();

layout!({
    /// The layout of [`ID_AA64MMFR2_EL1`].
    pub struct MemoryModelFeatureRegister2(u64);
    {
        let e0pd @ 63..60; // Preventing EL0 access to halves to address space (FEAT_E0PD)
        let evt @ 59..56; // Enhanced Virtualization Traps (FEAT_EVT)
        let bbm @ 55..52; // Break-before-make level support (FEAT_BBM)
        let ttl @ 51..48; // Support for TTL field (FEAT_TTL)
        let __ @ 47..44;
        let fwb @ 43..40; // Support for HCR_EL2.FB (FEAT_S2FWB)
        let ids @ 39..36; // Exception generated by ID space (FEAT_IDST)
        let at @ 35..32; // Unaligned single-copy atomic functions (FEAT_LSE2)
        let st @ 31..28; // Small translation tables (FEAT_TTST)
        let nv @ 27..24; // Nested Virtualization (FEAT_NV/FEAT_NV2)
        let ccidx @ 23..20; // Revised CCSIDR_EL1 register format (FEAT_CCIDX)
        let varange @ 19..16; // Support for larger virtual addresses (FEAT_LVA)
        let iesb @ 15..12; // Support for IESB bit in SCTLR_ELx (FEAT_IESB)
        let lsm @ 11..8; // Supoprt for LSMAOE and nTLSMD bit in SCTLR_EL1 (FEAT_LSMAOC)
        let uao @ 7..4; // User Access Override (FEAT_UAO)
        let cnp @ 3..0; // Common not Private translations (FEAT_TTCNP)
    }
});

/// [arm/v9]: D24.2.85 ID_AA64MMFR3_EL1, AArch64 Memory Model Feature Register 3
pub const ID_AA64MMFR3_EL1: SysReg<spec::ID_AA64MMFR3_EL1, MemoryModelFeatureRegister3> =
    SysReg::new();

layout!({
    /// The layout of [`ID_AA64MMFR3_EL1`].
    pub struct MemoryModelFeatureRegister3(u64);
    {
        let spec_fpacc @ 63..60; // Speculative behavior in pac auth failure
        let aderr @ 59..56; // Asynchronous device error exception
        let sderr @ 55..52; // Synchronous device error exception
        let __ @ 51..48;
        let anerr @ 47..44; // Asynchronous normal error exception
        let snerr @ 43..40; // Synchronous normal error exception
        let d128_2 @ 39..36; // 128-bit S2 translation table descriptor
        let d128 @ 35..32; // 128-bit translation table descriptor (FEAT_D128)
        let mec @ 31..28; // Memory Encryption Contexts (FEAT_MEC)
        let aie @ 27..24; // Attribute indexing (FEAT_AIE)
        let s2poe @ 23..20; // Stage 2 permission overlay (FEAT_S2POE)
        let s1poe @ 19..16; // Stage 1 permission overlay (FEAT_S1POE)
        let s2pie @ 15..12; // Stage 2 permission indirection (FEAT_S2PIE)
        let s1pie @ 11..8; // Stage 1 permission indirection (FEAT_S1PIE)
        let sctlrx @ 7..4; // SCTLR extension (FEAT_SCTLR2)
        let tcrx @ 3..0; // TCR extension (FEAT_TCR2)
    }
});

/// [arm/v9]: D24.2.86 ID_AA64MMFR4_EL1, AArch64 Memory Model Feature Register 4
pub const ID_AA64MMFR4_EL1: SysReg<spec::ID_AA64MMFR4_EL1, MemoryModelFeatureRegister4> =
    SysReg::new();

layout!({
    /// The layout of [`ID_AA64MMFR4_EL1`].
    pub struct MemoryModelFeatureRegister4(u64);
    {
        let __ @ 63..40;
        let e3dse @ 39..36; // Delegated SError exceptions from EL3 (FEAT_E3DSE)
        let __ @ 35..28;
        let e2h0 @ 27..24; // Support for programming HCR_EL2.E2H (FEAT_E2H0)
        let nv_frac @ 23..20; // Support for a subset of FEAT_NV and FEAT_NV2
        let fgwte3 @ 19..16; // Fine Grained Write Trap EL3 (FEAT_FGWTE3)
        let hacdbs @ 15..12; // Hardware accelerated cleaning of dirty state (FEAT_HACDBS)
        let asid2 @ 11..8; // Support for concurrent use of two ASIDs (FEAT_ASID2)
        let eiesb @ 7..4; // Early Implicit Error Synchronization event (FEAT_IESB)
        let __ @ 3..0;
    }
});
