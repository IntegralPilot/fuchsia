// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module stamps out [`SysRegSpec`] instances corresponding to known
//! arm64 system registers, named by their official mnemonic (e.g.,
//! `SCTLR_EL1`).

#![allow(non_camel_case_types)]

use super::SysRegSpec;
use crate::{Ro, RwSafe, RwUnsafe, WoSafe};

// This information is primarily found in the "AArch64 System Register
// Encoding" chapter in the ARM A-profile Architecture Reference Manual.
//
// The usage of RwSafe vs. RwUnsafe depends on the nature of the particular
// system register.
macro_rules! for_each_spec {
    ($macro:path) => {
        $macro!(ACCDATA_EL1, 0b11, 0b000, 0b1101, 0b0000, 0b101, RwSafe);
        $macro!(ACTLR_EL1, 0b11, 0b000, 0b0001, 0b0000, 0b001, RwSafe);
        $macro!(ACTLR_EL12, 0b11, 0b101, 0b0001, 0b0000, 0b001, RwSafe);
        $macro!(ACTLR_EL2, 0b11, 0b100, 0b0001, 0b0000, 0b001, RwSafe);
        $macro!(ACTLR_EL3, 0b11, 0b110, 0b0001, 0b0000, 0b001, RwSafe);
        $macro!(AFSR0_EL1, 0b11, 0b000, 0b0101, 0b0001, 0b000, RwSafe);
        $macro!(AFSR0_EL12, 0b11, 0b101, 0b0101, 0b0001, 0b000, RwSafe);
        $macro!(AFSR0_EL2, 0b11, 0b100, 0b0101, 0b0001, 0b000, RwSafe);
        $macro!(AFSR0_EL3, 0b11, 0b110, 0b0101, 0b0001, 0b000, RwSafe);
        $macro!(AFSR1_EL1, 0b11, 0b000, 0b0101, 0b0001, 0b001, RwSafe);
        $macro!(AFSR1_EL12, 0b11, 0b101, 0b0101, 0b0001, 0b001, RwSafe);
        $macro!(AFSR1_EL2, 0b11, 0b100, 0b0101, 0b0001, 0b001, RwSafe);
        $macro!(AFSR1_EL3, 0b11, 0b110, 0b0101, 0b0001, 0b001, RwSafe);
        $macro!(AIDR_EL1, 0b11, 0b001, 0b0000, 0b0000, 0b111, Ro);
        $macro!(ALLINT, 0b11, 0b000, 0b0100, 0b0011, 0b000, RwSafe);

        //
        // Can change cacheability.
        //
        $macro!(AMAIR_EL1, 0b11, 0b000, 0b1010, 0b0011, 0b000, RwUnsafe);
        $macro!(AMAIR_EL12, 0b11, 0b101, 0b1010, 0b0011, 0b000, RwUnsafe);
        $macro!(AMAIR_EL2, 0b11, 0b100, 0b1010, 0b0011, 0b000, RwUnsafe);
        $macro!(AMAIR_EL3, 0b11, 0b110, 0b1010, 0b0011, 0b000, RwUnsafe);
        $macro!(AMAIR2_EL1, 0b11, 0b000, 0b1010, 0b0011, 0b001, RwUnsafe);
        $macro!(AMAIR2_EL12, 0b11, 0b101, 0b1010, 0b0011, 0b001, RwUnsafe);
        $macro!(AMAIR2_EL2, 0b11, 0b100, 0b1010, 0b0011, 0b001, RwUnsafe);
        $macro!(AMAIR2_EL3, 0b11, 0b110, 0b1010, 0b0011, 0b001, RwUnsafe);

        $macro!(AMCFGR_EL0, 0b11, 0b011, 0b1101, 0b0010, 0b001, Ro);
        $macro!(AMCGCR_EL0, 0b11, 0b011, 0b1101, 0b0010, 0b010, Ro);
        $macro!(AMCG1IDR_EL0, 0b11, 0b011, 0b1101, 0b0010, 0b110, Ro);
        $macro!(AMCNTENCLR0_EL0, 0b11, 0b011, 0b1101, 0b0010, 0b100, RwSafe);
        $macro!(AMCNTENCLR1_EL0, 0b11, 0b011, 0b1101, 0b0011, 0b000, RwSafe);
        $macro!(AMCNTENSET0_EL0, 0b11, 0b011, 0b1101, 0b0010, 0b101, RwSafe);
        $macro!(AMCNTENSET1_EL0, 0b11, 0b011, 0b1101, 0b0011, 0b001, RwSafe);
        $macro!(AMCR_EL0, 0b11, 0b011, 0b1101, 0b0010, 0b000, RwSafe);
        $macro!(AMUSERENR_EL0, 0b11, 0b011, 0b1101, 0b0010, 0b011, RwSafe);
        $macro!(APDAKeyHi_EL1, 0b11, 0b000, 0b0010, 0b0010, 0b001, RwSafe);
        $macro!(APDAKeyLo_EL1, 0b11, 0b000, 0b0010, 0b0010, 0b000, RwSafe);
        $macro!(APDBKeyHi_EL1, 0b11, 0b000, 0b0010, 0b0010, 0b011, RwSafe);
        $macro!(APDBKeyLo_EL1, 0b11, 0b000, 0b0010, 0b0010, 0b010, RwSafe);
        $macro!(APGAKeyHi_EL1, 0b11, 0b000, 0b0010, 0b0011, 0b001, RwSafe);
        $macro!(APGAKeyLo_EL1, 0b11, 0b000, 0b0010, 0b0011, 0b000, RwSafe);
        $macro!(APIAKeyHi_EL1, 0b11, 0b000, 0b0010, 0b0001, 0b001, RwSafe);
        $macro!(APIAKeyLo_EL1, 0b11, 0b000, 0b0010, 0b0001, 0b000, RwSafe);
        $macro!(APIBKeyHi_EL1, 0b11, 0b000, 0b0010, 0b0001, 0b011, RwSafe);
        $macro!(APIBKeyLo_EL1, 0b11, 0b000, 0b0010, 0b0001, 0b010, RwSafe);
        $macro!(BRBCR_EL1, 0b10, 0b001, 0b1001, 0b0000, 0b000, RwSafe);
        $macro!(BRBCR_EL12, 0b10, 0b101, 0b1001, 0b0000, 0b000, RwSafe);
        $macro!(BRBCR_EL2, 0b10, 0b100, 0b1001, 0b0000, 0b000, RwSafe);
        $macro!(BRBFCR_EL1, 0b10, 0b001, 0b1001, 0b0000, 0b001, RwSafe);
        $macro!(BRBIDR0_EL1, 0b10, 0b001, 0b1001, 0b0010, 0b000, Ro);
        $macro!(BRBINFINJ_EL1, 0b10, 0b001, 0b1001, 0b0001, 0b000, RwSafe);
        $macro!(BRBSRCINJ_EL1, 0b10, 0b001, 0b1001, 0b0001, 0b001, RwSafe);
        $macro!(BRBTGTINJ_EL1, 0b10, 0b001, 0b1001, 0b0001, 0b010, RwSafe);
        $macro!(BRBTS_EL1, 0b10, 0b001, 0b1001, 0b0000, 0b010, RwSafe);
        $macro!(CCSIDR_EL1, 0b11, 0b001, 0b0000, 0b0000, 0b000, Ro);
        $macro!(CCSIDR2_EL1, 0b11, 0b001, 0b0000, 0b0000, 0b010, Ro);
        $macro!(CLIDR_EL1, 0b11, 0b001, 0b0000, 0b0000, 0b001, Ro);
        $macro!(CNTFRQ_EL0, 0b11, 0b011, 0b1110, 0b0000, 0b000, RwSafe);
        $macro!(CNTHCTL_EL2, 0b11, 0b100, 0b1110, 0b0001, 0b000, RwSafe);
        $macro!(CNTHP_CTL_EL2, 0b11, 0b100, 0b1110, 0b0010, 0b001, RwSafe);
        $macro!(CNTHP_CVAL_EL2, 0b11, 0b100, 0b1110, 0b0010, 0b010, RwSafe);
        $macro!(CNTHP_TVAL_EL2, 0b11, 0b100, 0b1110, 0b0010, 0b000, RwSafe);
        $macro!(CNTHPS_CTL_EL2, 0b11, 0b100, 0b1110, 0b0101, 0b001, RwSafe);
        $macro!(CNTHPS_CVAL_EL2, 0b11, 0b100, 0b1110, 0b0101, 0b010, RwSafe);
        $macro!(CNTHPS_TVAL_EL2, 0b11, 0b100, 0b1110, 0b0101, 0b000, RwSafe);
        $macro!(CNTHV_CTL_EL2, 0b11, 0b100, 0b1110, 0b0011, 0b001, RwSafe);
        $macro!(CNTHV_CVAL_EL2, 0b11, 0b100, 0b1110, 0b0011, 0b010, RwSafe);
        $macro!(CNTHV_TVAL_EL2, 0b11, 0b100, 0b1110, 0b0011, 0b000, RwSafe);
        $macro!(CNTHVS_CTL_EL2, 0b11, 0b100, 0b1110, 0b0100, 0b001, RwSafe);
        $macro!(CNTHVS_CVAL_EL2, 0b11, 0b100, 0b1110, 0b0100, 0b010, RwSafe);
        $macro!(CNTHVS_TVAL_EL2, 0b11, 0b100, 0b1110, 0b0100, 0b000, RwSafe);
        $macro!(CNTKCTL_EL1, 0b11, 0b000, 0b1110, 0b0001, 0b000, RwSafe);
        $macro!(CNTKCTL_EL12, 0b11, 0b101, 0b1110, 0b0001, 0b000, RwSafe);
        $macro!(CNTP_CTL_EL0, 0b11, 0b011, 0b1110, 0b0010, 0b001, RwSafe);
        $macro!(CNTP_CTL_EL02, 0b11, 0b101, 0b1110, 0b0010, 0b001, RwSafe);
        $macro!(CNTP_CVAL_EL0, 0b11, 0b011, 0b1110, 0b0010, 0b010, RwSafe);
        $macro!(CNTP_CVAL_EL02, 0b11, 0b101, 0b1110, 0b0010, 0b010, RwSafe);
        $macro!(CNTP_TVAL_EL0, 0b11, 0b011, 0b1110, 0b0010, 0b000, RwSafe);
        $macro!(CNTP_TVAL_EL02, 0b11, 0b101, 0b1110, 0b0010, 0b000, RwSafe);
        $macro!(CNTPCT_EL0, 0b11, 0b011, 0b1110, 0b0000, 0b001, Ro);
        $macro!(CNTPCTSS_EL0, 0b11, 0b011, 0b1110, 0b0000, 0b101, Ro);
        $macro!(CNTPOFF_EL2, 0b11, 0b100, 0b1110, 0b0000, 0b110, RwSafe);
        $macro!(CNTPS_CTL_EL1, 0b11, 0b111, 0b1110, 0b0010, 0b001, RwSafe);
        $macro!(CNTPS_CVAL_EL1, 0b11, 0b111, 0b1110, 0b0010, 0b010, RwSafe);
        $macro!(CNTPS_TVAL_EL1, 0b11, 0b111, 0b1110, 0b0010, 0b000, RwSafe);
        $macro!(CNTV_CTL_EL0, 0b11, 0b011, 0b1110, 0b0011, 0b001, RwSafe);
        $macro!(CNTV_CTL_EL02, 0b11, 0b101, 0b1110, 0b0011, 0b001, RwSafe);
        $macro!(CNTV_CVAL_EL0, 0b11, 0b011, 0b1110, 0b0011, 0b010, RwSafe);
        $macro!(CNTV_CVAL_EL02, 0b11, 0b101, 0b1110, 0b0011, 0b010, RwSafe);
        $macro!(CNTV_TVAL_EL0, 0b11, 0b011, 0b1110, 0b0011, 0b000, RwSafe);
        $macro!(CNTV_TVAL_EL02, 0b11, 0b101, 0b1110, 0b0011, 0b000, RwSafe);
        $macro!(CNTVCT_EL0, 0b11, 0b011, 0b1110, 0b0000, 0b010, Ro);
        $macro!(CNTVCTSS_EL0, 0b11, 0b011, 0b1110, 0b0000, 0b110, Ro);
        $macro!(CNTVOFF_EL2, 0b11, 0b100, 0b1110, 0b0000, 0b011, RwSafe);
        $macro!(CONTEXTIDR_EL1, 0b11, 0b000, 0b1101, 0b0000, 0b001, RwSafe);
        $macro!(CONTEXTIDR_EL12, 0b11, 0b101, 0b1101, 0b0000, 0b001, RwSafe);
        $macro!(CONTEXTIDR_EL2, 0b11, 0b100, 0b1101, 0b0000, 0b001, RwSafe);
        $macro!(CPACR_EL1, 0b11, 0b000, 0b0001, 0b0000, 0b010, RwSafe);
        $macro!(CPACR_EL12, 0b11, 0b101, 0b0001, 0b0000, 0b010, RwSafe);
        $macro!(CPTR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b010, RwSafe);
        $macro!(CPTR_EL3, 0b11, 0b110, 0b0001, 0b0001, 0b010, RwSafe);
        $macro!(CSSELR_EL1, 0b11, 0b010, 0b0000, 0b0000, 0b000, RwSafe);
        $macro!(CTR_EL0, 0b11, 0b011, 0b0000, 0b0000, 0b001, Ro);
        $macro!(CurrentEL, 0b11, 0b000, 0b0100, 0b0010, 0b010, Ro);
        $macro!(DACR32_EL2, 0b11, 0b100, 0b0011, 0b0000, 0b000, RwSafe);
        $macro!(DAIF, 0b11, 0b011, 0b0100, 0b0010, 0b001, RwSafe);
        $macro!(DBGAUTHSTATUS_EL1, 0b10, 0b000, 0b0111, 0b1110, 0b110, Ro);
        $macro!(DBGBCR0_EL1, 0b10, 0b000, 0b0000, 0b0000, 0b101, RwSafe);
        $macro!(DBGBCR1_EL1, 0b10, 0b000, 0b0000, 0b0001, 0b101, RwSafe);
        $macro!(DBGBCR2_EL1, 0b10, 0b000, 0b0000, 0b0010, 0b101, RwSafe);
        $macro!(DBGBCR3_EL1, 0b10, 0b000, 0b0000, 0b0011, 0b101, RwSafe);
        $macro!(DBGBCR4_EL1, 0b10, 0b000, 0b0000, 0b0100, 0b101, RwSafe);
        $macro!(DBGBCR5_EL1, 0b10, 0b000, 0b0000, 0b0101, 0b101, RwSafe);
        $macro!(DBGBCR6_EL1, 0b10, 0b000, 0b0000, 0b0110, 0b101, RwSafe);
        $macro!(DBGBCR7_EL1, 0b10, 0b000, 0b0000, 0b0111, 0b101, RwSafe);
        $macro!(DBGBCR8_EL1, 0b10, 0b000, 0b0000, 0b1000, 0b101, RwSafe);
        $macro!(DBGBCR9_EL1, 0b10, 0b000, 0b0000, 0b1001, 0b101, RwSafe);
        $macro!(DBGBCR10_EL1, 0b10, 0b000, 0b0000, 0b1010, 0b101, RwSafe);
        $macro!(DBGBCR11_EL1, 0b10, 0b000, 0b0000, 0b1011, 0b101, RwSafe);
        $macro!(DBGBCR12_EL1, 0b10, 0b000, 0b0000, 0b1100, 0b101, RwSafe);
        $macro!(DBGBCR13_EL1, 0b10, 0b000, 0b0000, 0b1101, 0b101, RwSafe);
        $macro!(DBGBCR14_EL1, 0b10, 0b000, 0b0000, 0b1110, 0b101, RwSafe);
        $macro!(DBGBCR15_EL1, 0b10, 0b000, 0b0000, 0b1111, 0b101, RwSafe);
        $macro!(DBGBVR0_EL1, 0b10, 0b000, 0b0000, 0b0000, 0b100, RwSafe);
        $macro!(DBGBVR1_EL1, 0b10, 0b000, 0b0000, 0b0001, 0b100, RwSafe);
        $macro!(DBGBVR2_EL1, 0b10, 0b000, 0b0000, 0b0010, 0b100, RwSafe);
        $macro!(DBGBVR3_EL1, 0b10, 0b000, 0b0000, 0b0011, 0b100, RwSafe);
        $macro!(DBGBVR4_EL1, 0b10, 0b000, 0b0000, 0b0100, 0b100, RwSafe);
        $macro!(DBGBVR5_EL1, 0b10, 0b000, 0b0000, 0b0101, 0b100, RwSafe);
        $macro!(DBGBVR6_EL1, 0b10, 0b000, 0b0000, 0b0110, 0b100, RwSafe);
        $macro!(DBGBVR7_EL1, 0b10, 0b000, 0b0000, 0b0111, 0b100, RwSafe);
        $macro!(DBGBVR8_EL1, 0b10, 0b000, 0b0000, 0b1000, 0b100, RwSafe);
        $macro!(DBGBVR9_EL1, 0b10, 0b000, 0b0000, 0b1001, 0b100, RwSafe);
        $macro!(DBGBVR10_EL1, 0b10, 0b000, 0b0000, 0b1010, 0b100, RwSafe);
        $macro!(DBGBVR11_EL1, 0b10, 0b000, 0b0000, 0b1011, 0b100, RwSafe);
        $macro!(DBGBVR12_EL1, 0b10, 0b000, 0b0000, 0b1100, 0b100, RwSafe);
        $macro!(DBGBVR13_EL1, 0b10, 0b000, 0b0000, 0b1101, 0b100, RwSafe);
        $macro!(DBGBVR14_EL1, 0b10, 0b000, 0b0000, 0b1110, 0b100, RwSafe);
        $macro!(DBGBVR15_EL1, 0b10, 0b000, 0b0000, 0b1111, 0b100, RwSafe);
        $macro!(DBGCLAIMCLR_EL1, 0b10, 0b000, 0b0111, 0b1001, 0b110, RwSafe);
        $macro!(DBGCLAIMSET_EL1, 0b10, 0b000, 0b0111, 0b1000, 0b110, RwSafe);
        $macro!(DBGDTR_EL0, 0b10, 0b011, 0b0000, 0b0100, 0b000, RwSafe);
        $macro!(DBGDTRRX_EL0, 0b10, 0b011, 0b0000, 0b0101, 0b000, Ro);
        $macro!(DBGDTRTX_EL0, 0b10, 0b011, 0b0000, 0b0101, 0b000, WoSafe);
        $macro!(DBGPRCR_EL1, 0b10, 0b000, 0b0001, 0b0100, 0b100, RwSafe);
        $macro!(DBGVCR32_EL2, 0b10, 0b100, 0b0000, 0b0111, 0b000, RwSafe);
        $macro!(DBGWCR0_EL1, 0b10, 0b000, 0b0000, 0b0000, 0b111, RwSafe);
        $macro!(DBGWCR1_EL1, 0b10, 0b000, 0b0000, 0b0001, 0b111, RwSafe);
        $macro!(DBGWCR2_EL1, 0b10, 0b000, 0b0000, 0b0010, 0b111, RwSafe);
        $macro!(DBGWCR3_EL1, 0b10, 0b000, 0b0000, 0b0011, 0b111, RwSafe);
        $macro!(DBGWCR4_EL1, 0b10, 0b000, 0b0000, 0b0100, 0b111, RwSafe);
        $macro!(DBGWCR5_EL1, 0b10, 0b000, 0b0000, 0b0101, 0b111, RwSafe);
        $macro!(DBGWCR6_EL1, 0b10, 0b000, 0b0000, 0b0110, 0b111, RwSafe);
        $macro!(DBGWCR7_EL1, 0b10, 0b000, 0b0000, 0b0111, 0b111, RwSafe);
        $macro!(DBGWCR8_EL1, 0b10, 0b000, 0b0000, 0b1000, 0b111, RwSafe);
        $macro!(DBGWCR9_EL1, 0b10, 0b000, 0b0000, 0b1001, 0b111, RwSafe);
        $macro!(DBGWCR10_EL1, 0b10, 0b000, 0b0000, 0b1010, 0b111, RwSafe);
        $macro!(DBGWCR11_EL1, 0b10, 0b000, 0b0000, 0b1011, 0b111, RwSafe);
        $macro!(DBGWCR12_EL1, 0b10, 0b000, 0b0000, 0b1100, 0b111, RwSafe);
        $macro!(DBGWCR13_EL1, 0b10, 0b000, 0b0000, 0b1101, 0b111, RwSafe);
        $macro!(DBGWCR14_EL1, 0b10, 0b000, 0b0000, 0b1110, 0b111, RwSafe);
        $macro!(DBGWCR15_EL1, 0b10, 0b000, 0b0000, 0b1111, 0b111, RwSafe);
        $macro!(DBGWVR0_EL1, 0b10, 0b000, 0b0000, 0b0000, 0b110, RwSafe);
        $macro!(DBGWVR1_EL1, 0b10, 0b000, 0b0000, 0b0001, 0b110, RwSafe);
        $macro!(DBGWVR2_EL1, 0b10, 0b000, 0b0000, 0b0010, 0b110, RwSafe);
        $macro!(DBGWVR3_EL1, 0b10, 0b000, 0b0000, 0b0011, 0b110, RwSafe);
        $macro!(DBGWVR4_EL1, 0b10, 0b000, 0b0000, 0b0100, 0b110, RwSafe);
        $macro!(DBGWVR5_EL1, 0b10, 0b000, 0b0000, 0b0101, 0b110, RwSafe);
        $macro!(DBGWVR6_EL1, 0b10, 0b000, 0b0000, 0b0110, 0b110, RwSafe);
        $macro!(DBGWVR7_EL1, 0b10, 0b000, 0b0000, 0b0111, 0b110, RwSafe);
        $macro!(DBGWVR8_EL1, 0b10, 0b000, 0b0000, 0b1000, 0b110, RwSafe);
        $macro!(DBGWVR9_EL1, 0b10, 0b000, 0b0000, 0b1001, 0b110, RwSafe);
        $macro!(DBGWVR10_EL1, 0b10, 0b000, 0b0000, 0b1010, 0b110, RwSafe);
        $macro!(DBGWVR11_EL1, 0b10, 0b000, 0b0000, 0b1011, 0b110, RwSafe);
        $macro!(DBGWVR12_EL1, 0b10, 0b000, 0b0000, 0b1100, 0b110, RwSafe);
        $macro!(DBGWVR13_EL1, 0b10, 0b000, 0b0000, 0b1101, 0b110, RwSafe);
        $macro!(DBGWVR14_EL1, 0b10, 0b000, 0b0000, 0b1110, 0b110, RwSafe);
        $macro!(DBGWVR15_EL1, 0b10, 0b000, 0b0000, 0b1111, 0b110, RwSafe);
        $macro!(DCZID_EL0, 0b11, 0b011, 0b0000, 0b0000, 0b111, Ro);
        $macro!(DISR_EL1, 0b11, 0b000, 0b1100, 0b0001, 0b001, RwSafe);
        $macro!(DIT, 0b11, 0b011, 0b0100, 0b0010, 0b101, RwSafe);

        //
        // Can change the return address.
        //
        $macro!(DLR_EL0, 0b11, 0b011, 0b0100, 0b0101, 0b001, RwUnsafe);
        $macro!(DSPSR_EL0, 0b11, 0b011, 0b0100, 0b0101, 0b000, RwUnsafe);
        $macro!(ELR_EL1, 0b11, 0b000, 0b0100, 0b0000, 0b001, RwUnsafe);
        $macro!(ELR_EL12, 0b11, 0b101, 0b0100, 0b0000, 0b001, RwUnsafe);
        $macro!(ELR_EL2, 0b11, 0b100, 0b0100, 0b0000, 0b001, RwUnsafe);
        $macro!(ELR_EL3, 0b11, 0b110, 0b0100, 0b0000, 0b001, RwUnsafe);

        $macro!(ERRIDR_EL1, 0b11, 0b000, 0b0101, 0b0011, 0b000, Ro);
        $macro!(ERRSELR_EL1, 0b11, 0b000, 0b0101, 0b0011, 0b001, RwSafe);
        $macro!(ERXADDR_EL1, 0b11, 0b000, 0b0101, 0b0100, 0b011, RwSafe);
        $macro!(ERXCTLR_EL1, 0b11, 0b000, 0b0101, 0b0100, 0b001, RwSafe);
        $macro!(ERXFR_EL1, 0b11, 0b000, 0b0101, 0b0100, 0b000, Ro);
        $macro!(ERXGSR_EL1, 0b11, 0b000, 0b0101, 0b0011, 0b010, Ro);
        $macro!(ERXMISC0_EL1, 0b11, 0b000, 0b0101, 0b0101, 0b000, RwSafe);
        $macro!(ERXMISC1_EL1, 0b11, 0b000, 0b0101, 0b0101, 0b001, RwSafe);
        $macro!(ERXMISC2_EL1, 0b11, 0b000, 0b0101, 0b0101, 0b010, RwSafe);
        $macro!(ERXMISC3_EL1, 0b11, 0b000, 0b0101, 0b0101, 0b011, RwSafe);
        $macro!(ERXPFGCDN_EL1, 0b11, 0b000, 0b0101, 0b0100, 0b110, RwSafe);
        $macro!(ERXPFGCTL_EL1, 0b11, 0b000, 0b0101, 0b0100, 0b101, RwSafe);
        $macro!(ERXPFGF_EL1, 0b11, 0b000, 0b0101, 0b0100, 0b100, Ro);
        $macro!(ERXSTATUS_EL1, 0b11, 0b000, 0b0101, 0b0100, 0b010, RwSafe);
        $macro!(ESR_EL1, 0b11, 0b000, 0b0101, 0b0010, 0b000, RwSafe);
        $macro!(ESR_EL12, 0b11, 0b101, 0b0101, 0b0010, 0b000, RwSafe);
        $macro!(ESR_EL2, 0b11, 0b100, 0b0101, 0b0010, 0b000, RwSafe);
        $macro!(ESR_EL3, 0b11, 0b110, 0b0101, 0b0010, 0b000, RwSafe);
        $macro!(FAR_EL1, 0b11, 0b000, 0b0110, 0b0000, 0b000, RwSafe);
        $macro!(FAR_EL12, 0b11, 0b101, 0b0110, 0b0000, 0b000, RwSafe);
        $macro!(FAR_EL2, 0b11, 0b100, 0b0110, 0b0000, 0b000, RwSafe);
        $macro!(FAR_EL3, 0b11, 0b110, 0b0110, 0b0000, 0b000, RwSafe);
        $macro!(FGWTE3_EL3, 0b11, 0b110, 0b0001, 0b0001, 0b101, RwSafe);
        $macro!(FPCR, 0b11, 0b011, 0b0100, 0b0100, 0b000, RwSafe);
        $macro!(FPEXC32_EL2, 0b11, 0b100, 0b0101, 0b0011, 0b000, RwSafe);
        $macro!(FPMR, 0b11, 0b011, 0b0100, 0b0100, 0b010, RwSafe);
        $macro!(FPSR, 0b11, 0b011, 0b0100, 0b0100, 0b001, RwSafe);
        $macro!(GCR_EL1, 0b11, 0b000, 0b0001, 0b0000, 0b110, RwSafe);
        $macro!(GCSCR_EL1, 0b11, 0b000, 0b0010, 0b0101, 0b000, RwSafe);
        $macro!(GCSCR_EL12, 0b11, 0b101, 0b0010, 0b0101, 0b000, RwSafe);
        $macro!(GCSCR_EL2, 0b11, 0b100, 0b0010, 0b0101, 0b000, RwSafe);
        $macro!(GCSCR_EL3, 0b11, 0b110, 0b0010, 0b0101, 0b000, RwSafe);
        $macro!(GCSCRE0_EL1, 0b11, 0b000, 0b0010, 0b0101, 0b010, RwSafe);

        //
        // Can change active shadow stack pointer.
        //
        $macro!(GCSPR_EL0, 0b11, 0b011, 0b0010, 0b0101, 0b001, RwUnsafe);
        $macro!(GCSPR_EL1, 0b11, 0b000, 0b0010, 0b0101, 0b001, RwUnsafe);
        $macro!(GCSPR_EL12, 0b11, 0b101, 0b0010, 0b0101, 0b001, RwUnsafe);
        $macro!(GCSPR_EL2, 0b11, 0b100, 0b0010, 0b0101, 0b001, RwUnsafe);
        $macro!(GCSPR_EL3, 0b11, 0b110, 0b0010, 0b0101, 0b001, RwUnsafe);

        $macro!(GMID_EL1, 0b11, 0b001, 0b0000, 0b0000, 0b100, Ro);
        $macro!(GPCCR_EL3, 0b11, 0b110, 0b0010, 0b0001, 0b110, RwSafe);
        $macro!(GPTBR_EL3, 0b11, 0b110, 0b0010, 0b0001, 0b100, RwSafe);
        $macro!(HACDBSBR_EL2, 0b11, 0b100, 0b0010, 0b0011, 0b100, RwSafe);
        $macro!(HACDBSCONS_EL2, 0b11, 0b100, 0b0010, 0b0011, 0b101, RwSafe);
        $macro!(HACR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b111, RwSafe);
        $macro!(HAFGRTR_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b110, RwSafe);

        //
        // Safe-writable, since the writer's execution context is not subject
        // to the related (stage 2) memory mappings.
        //
        $macro!(HCR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b000, RwSafe);
        $macro!(HCRX_EL2, 0b11, 0b100, 0b0001, 0b0010, 0b010, RwSafe);

        $macro!(HDBSSBR_EL2, 0b11, 0b100, 0b0010, 0b0011, 0b010, RwSafe);
        $macro!(HDBSSPRoD_EL2, 0b11, 0b100, 0b0010, 0b0011, 0b011, RwSafe);
        $macro!(HDFGRTR_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b100, RwSafe);
        $macro!(HDFGRTR2_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b000, RwSafe);
        $macro!(HDFGWTR_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b101, RwSafe);
        $macro!(HDFGWTR2_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b001, RwSafe);
        $macro!(HFGITR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b110, RwSafe);
        $macro!(HFGITR2_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b111, RwSafe);
        $macro!(HFGRTR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b100, RwSafe);
        $macro!(HFGRTR2_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b010, RwSafe);
        $macro!(HFGWTR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b101, RwSafe);
        $macro!(HFGWTR2_EL2, 0b11, 0b100, 0b0011, 0b0001, 0b011, RwSafe);
        $macro!(HPFAR_EL2, 0b11, 0b100, 0b0110, 0b0000, 0b100, RwSafe);
        $macro!(HSTR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b011, RwSafe);
        $macro!(ICC_ASGI1R_EL1, 0b11, 0b000, 0b1100, 0b1011, 0b110, WoSafe);
        $macro!(ICC_BPR0_EL1, 0b11, 0b000, 0b1100, 0b1000, 0b011, RwSafe);
        $macro!(ICC_BPR1_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b011, RwSafe);
        $macro!(ICC_CTLR_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b100, RwSafe);
        $macro!(ICC_CTLR_EL3, 0b11, 0b110, 0b1100, 0b1100, 0b100, RwSafe);
        $macro!(ICC_DIR_EL1, 0b11, 0b000, 0b1100, 0b1011, 0b001, WoSafe);
        $macro!(ICC_EOIR0_EL1, 0b11, 0b000, 0b1100, 0b1000, 0b001, WoSafe);
        $macro!(ICC_EOIR1_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b001, WoSafe);
        $macro!(ICC_HPPIR0_EL1, 0b11, 0b000, 0b1100, 0b1000, 0b010, Ro);
        $macro!(ICC_HPPIR1_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b010, Ro);
        $macro!(ICC_IAR0_EL1, 0b11, 0b000, 0b1100, 0b1000, 0b000, Ro);
        $macro!(ICC_IAR1_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b000, Ro);
        $macro!(ICC_IGRPEN0_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b110, RwSafe);
        $macro!(ICC_IGRPEN1_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b111, RwSafe);
        $macro!(ICC_IGRPEN1_EL3, 0b11, 0b110, 0b1100, 0b1100, 0b111, RwSafe);
        $macro!(ICC_NMIAR1_EL1, 0b11, 0b000, 0b1100, 0b1001, 0b101, Ro);
        $macro!(ICC_PMR_EL1, 0b11, 0b000, 0b0100, 0b0110, 0b000, RwSafe);
        $macro!(ICC_RPR_EL1, 0b11, 0b000, 0b1100, 0b1011, 0b011, Ro);
        $macro!(ICC_SGI0R_EL1, 0b11, 0b000, 0b1100, 0b1011, 0b111, WoSafe);
        $macro!(ICC_SGI1R_EL1, 0b11, 0b000, 0b1100, 0b1011, 0b101, WoSafe);
        $macro!(ICC_SRE_EL1, 0b11, 0b000, 0b1100, 0b1100, 0b101, RwSafe);
        $macro!(ICC_SRE_EL2, 0b11, 0b100, 0b1100, 0b1001, 0b101, RwSafe);
        $macro!(ICC_SRE_EL3, 0b11, 0b110, 0b1100, 0b1100, 0b101, RwSafe);
        $macro!(ICH_EISR_EL2, 0b11, 0b100, 0b1100, 0b1011, 0b011, Ro);
        $macro!(ICH_ELRSR_EL2, 0b11, 0b100, 0b1100, 0b1011, 0b101, Ro);
        $macro!(ICH_HCR_EL2, 0b11, 0b100, 0b1100, 0b1011, 0b000, RwSafe);
        $macro!(ICH_MISR_EL2, 0b11, 0b100, 0b1100, 0b1011, 0b010, Ro);
        $macro!(ICH_VMCR_EL2, 0b11, 0b100, 0b1100, 0b1011, 0b111, RwSafe);
        $macro!(ICH_VTR_EL2, 0b11, 0b100, 0b1100, 0b1011, 0b001, Ro);
        $macro!(ID_AA64AFR0_EL1, 0b11, 0b000, 0b0000, 0b0101, 0b100, Ro);
        $macro!(ID_AA64AFR1_EL1, 0b11, 0b000, 0b0000, 0b0101, 0b101, Ro);
        $macro!(ID_AA64DFR0_EL1, 0b11, 0b000, 0b0000, 0b0101, 0b000, Ro);
        $macro!(ID_AA64DFR1_EL1, 0b11, 0b000, 0b0000, 0b0101, 0b001, Ro);
        $macro!(ID_AA64DFR2_EL1, 0b11, 0b000, 0b0000, 0b0101, 0b010, Ro);
        $macro!(ID_AA64FPFR0_EL1, 0b11, 0b000, 0b0000, 0b0100, 0b111, Ro);
        $macro!(ID_AA64ISAR0_EL1, 0b11, 0b000, 0b0000, 0b0110, 0b000, Ro);
        $macro!(ID_AA64ISAR1_EL1, 0b11, 0b000, 0b0000, 0b0110, 0b001, Ro);
        $macro!(ID_AA64ISAR2_EL1, 0b11, 0b000, 0b0000, 0b0110, 0b010, Ro);
        $macro!(ID_AA64ISAR3_EL1, 0b11, 0b000, 0b0000, 0b0110, 0b011, Ro);
        $macro!(ID_AA64MMFR0_EL1, 0b11, 0b000, 0b0000, 0b0111, 0b000, Ro);
        $macro!(ID_AA64MMFR1_EL1, 0b11, 0b000, 0b0000, 0b0111, 0b001, Ro);
        $macro!(ID_AA64MMFR2_EL1, 0b11, 0b000, 0b0000, 0b0111, 0b010, Ro);
        $macro!(ID_AA64MMFR3_EL1, 0b11, 0b000, 0b0000, 0b0111, 0b011, Ro);
        $macro!(ID_AA64MMFR4_EL1, 0b11, 0b000, 0b0000, 0b0111, 0b100, Ro);
        $macro!(ID_AA64PFR0_EL1, 0b11, 0b000, 0b0000, 0b0100, 0b000, Ro);
        $macro!(ID_AA64PFR1_EL1, 0b11, 0b000, 0b0000, 0b0100, 0b001, Ro);
        $macro!(ID_AA64PFR2_EL1, 0b11, 0b000, 0b0000, 0b0100, 0b010, Ro);
        $macro!(ID_AA64SMFR0_EL1, 0b11, 0b000, 0b0000, 0b0100, 0b101, Ro);
        $macro!(ID_AA64ZFR0_EL1, 0b11, 0b000, 0b0000, 0b0100, 0b100, Ro);
        $macro!(ID_AFR0_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b011, Ro);
        $macro!(ID_DFR0_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b010, Ro);
        $macro!(ID_DFR1_EL1, 0b11, 0b000, 0b0000, 0b0011, 0b101, Ro);
        $macro!(ID_ISAR0_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b000, Ro);
        $macro!(ID_ISAR1_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b001, Ro);
        $macro!(ID_ISAR2_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b010, Ro);
        $macro!(ID_ISAR3_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b011, Ro);
        $macro!(ID_ISAR4_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b100, Ro);
        $macro!(ID_ISAR5_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b101, Ro);
        $macro!(ID_ISAR6_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b111, Ro);
        $macro!(ID_MMFR0_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b100, Ro);
        $macro!(ID_MMFR1_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b101, Ro);
        $macro!(ID_MMFR2_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b110, Ro);
        $macro!(ID_MMFR3_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b111, Ro);
        $macro!(ID_MMFR4_EL1, 0b11, 0b000, 0b0000, 0b0010, 0b110, Ro);
        $macro!(ID_MMFR5_EL1, 0b11, 0b000, 0b0000, 0b0011, 0b110, Ro);
        $macro!(ID_PFR0_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b000, Ro);
        $macro!(ID_PFR1_EL1, 0b11, 0b000, 0b0000, 0b0001, 0b001, Ro);
        $macro!(ID_PFR2_EL1, 0b11, 0b000, 0b0000, 0b0011, 0b100, Ro);
        $macro!(IFSR32_EL2, 0b11, 0b100, 0b0101, 0b0000, 0b001, RwSafe);
        $macro!(ISR_EL1, 0b11, 0b000, 0b1100, 0b0001, 0b000, Ro);
        $macro!(LORC_EL1, 0b11, 0b000, 0b1010, 0b0100, 0b011, RwSafe);
        $macro!(LOREA_EL1, 0b11, 0b000, 0b1010, 0b0100, 0b001, RwSafe);
        $macro!(LORID_EL1, 0b11, 0b000, 0b1010, 0b0100, 0b111, Ro);
        $macro!(LORN_EL1, 0b11, 0b000, 0b1010, 0b0100, 0b010, RwSafe);
        $macro!(LORSA_EL1, 0b11, 0b000, 0b1010, 0b0100, 0b000, RwSafe);

        //
        // Can change cacheability.
        //
        $macro!(MAIR_EL1, 0b11, 0b000, 0b1010, 0b0010, 0b000, RwUnsafe);
        $macro!(MAIR_EL12, 0b11, 0b101, 0b1010, 0b0010, 0b000, RwUnsafe);
        $macro!(MAIR_EL2, 0b11, 0b100, 0b1010, 0b0010, 0b000, RwUnsafe);
        $macro!(MAIR_EL3, 0b11, 0b110, 0b1010, 0b0010, 0b000, RwUnsafe);
        $macro!(MAIR2_EL1, 0b11, 0b000, 0b1010, 0b0010, 0b001, RwUnsafe);
        $macro!(MAIR2_EL12, 0b11, 0b101, 0b1010, 0b0010, 0b001, RwUnsafe);
        $macro!(MAIR2_EL2, 0b11, 0b100, 0b1010, 0b0001, 0b001, RwUnsafe);
        $macro!(MAIR2_EL3, 0b11, 0b110, 0b1010, 0b0001, 0b001, RwUnsafe);

        $macro!(MDCCINT_EL1, 0b10, 0b000, 0b0000, 0b0010, 0b000, RwSafe);
        $macro!(MDCCSR_EL0, 0b10, 0b011, 0b0000, 0b0001, 0b000, Ro);
        $macro!(MDCR_EL2, 0b11, 0b100, 0b0001, 0b0001, 0b001, RwSafe);
        $macro!(MDCR_EL3, 0b11, 0b110, 0b0001, 0b0011, 0b001, RwSafe);
        $macro!(MDRAR_EL1, 0b10, 0b000, 0b0001, 0b0000, 0b000, Ro);
        $macro!(MDSCR_EL1, 0b10, 0b000, 0b0000, 0b0010, 0b010, RwSafe);
        $macro!(MDSELR_EL1, 0b10, 0b000, 0b0000, 0b0100, 0b010, RwSafe);
        $macro!(MDSTEPOP_EL1, 0b10, 0b000, 0b0000, 0b0101, 0b010, RwSafe);
        $macro!(MECID_A0_EL2, 0b11, 0b100, 0b1010, 0b1000, 0b001, RwSafe);
        $macro!(MECID_A1_EL2, 0b11, 0b100, 0b1010, 0b1000, 0b011, RwSafe);
        $macro!(MECID_P0_EL2, 0b11, 0b100, 0b1010, 0b1000, 0b000, RwSafe);
        $macro!(MECID_P1_EL2, 0b11, 0b100, 0b1010, 0b1000, 0b010, RwSafe);
        $macro!(MECID_RL_A_EL3, 0b11, 0b110, 0b1010, 0b1010, 0b001, RwSafe);
        $macro!(MECIDR_EL2, 0b11, 0b100, 0b1010, 0b1000, 0b111, Ro);
        $macro!(MFAR_EL3, 0b11, 0b110, 0b0110, 0b0000, 0b101, RwSafe);
        $macro!(MIDR_EL1, 0b11, 0b000, 0b0000, 0b0000, 0b000, Ro);
        $macro!(MPAMHCR_EL2, 0b11, 0b100, 0b1010, 0b0100, 0b000, RwSafe);
        $macro!(MPAMIDR_EL1, 0b11, 0b000, 0b1010, 0b0100, 0b100, Ro);
        $macro!(MPAMSM_EL1, 0b11, 0b000, 0b1010, 0b0101, 0b011, RwSafe);
        $macro!(MPAMVPMV_EL2, 0b11, 0b100, 0b1010, 0b0100, 0b001, RwSafe);
        $macro!(MPAMVPM0_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b000, RwSafe);
        $macro!(MPAMVPM1_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b001, RwSafe);
        $macro!(MPAMVPM2_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b010, RwSafe);
        $macro!(MPAMVPM3_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b011, RwSafe);
        $macro!(MPAMVPM4_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b100, RwSafe);
        $macro!(MPAMVPM5_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b101, RwSafe);
        $macro!(MPAMVPM6_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b110, RwSafe);
        $macro!(MPAMVPM7_EL2, 0b11, 0b100, 0b1010, 0b0110, 0b111, RwSafe);
        $macro!(MPAM0_EL1, 0b11, 0b000, 0b1010, 0b0101, 0b001, RwSafe);
        $macro!(MPAM1_EL1, 0b11, 0b000, 0b1010, 0b0101, 0b000, RwSafe);
        $macro!(MPAM1_EL12, 0b11, 0b101, 0b1010, 0b0101, 0b000, RwSafe);
        $macro!(MPAM2_EL2, 0b11, 0b100, 0b1010, 0b0101, 0b000, RwSafe);
        $macro!(MPAM3_EL3, 0b11, 0b110, 0b1010, 0b0101, 0b000, RwSafe);
        $macro!(MPIDR_EL1, 0b11, 0b000, 0b0000, 0b0000, 0b101, Ro);
        $macro!(MVFR0_EL1, 0b11, 0b000, 0b0000, 0b0011, 0b000, Ro);
        $macro!(MVFR1_EL1, 0b11, 0b000, 0b0000, 0b0011, 0b001, Ro);
        $macro!(MVFR2_EL1, 0b11, 0b000, 0b0000, 0b0011, 0b010, Ro);
        $macro!(NZCV, 0b11, 0b011, 0b0100, 0b0010, 0b000, RwSafe);
        $macro!(OSDLR_EL1, 0b10, 0b000, 0b0001, 0b0011, 0b100, RwSafe);
        $macro!(OSDTRRX_EL1, 0b10, 0b000, 0b0000, 0b0000, 0b010, RwSafe);
        $macro!(OSDTRTX_EL1, 0b10, 0b000, 0b0000, 0b0011, 0b010, RwSafe);
        $macro!(OSECCR_EL1, 0b10, 0b000, 0b0000, 0b0110, 0b010, RwSafe);
        $macro!(OSLAR_EL1, 0b10, 0b000, 0b0001, 0b0000, 0b100, WoSafe);
        $macro!(OSLSR_EL1, 0b10, 0b000, 0b0001, 0b0001, 0b100, Ro);
        $macro!(PAN, 0b11, 0b000, 0b0100, 0b0010, 0b011, RwSafe);
        $macro!(PAR_EL1, 0b11, 0b000, 0b0111, 0b0100, 0b000, RwSafe);
        $macro!(PFAR_EL1, 0b11, 0b000, 0b0110, 0b0000, 0b101, RwSafe);
        $macro!(PFAR_EL12, 0b11, 0b101, 0b0110, 0b0000, 0b101, RwSafe);
        $macro!(PFAR_EL2, 0b11, 0b100, 0b0110, 0b0000, 0b101, RwSafe);
        $macro!(PIR_EL1, 0b11, 0b000, 0b1010, 0b0010, 0b011, RwSafe);
        $macro!(PIR_EL12, 0b11, 0b101, 0b1010, 0b0010, 0b011, RwSafe);
        $macro!(PIR_EL2, 0b11, 0b100, 0b1010, 0b0010, 0b011, RwSafe);
        $macro!(PIR_EL3, 0b11, 0b110, 0b1010, 0b0010, 0b011, RwSafe);
        $macro!(PIRE0_EL1, 0b11, 0b000, 0b1010, 0b0010, 0b010, RwSafe);
        $macro!(PIRE0_EL12, 0b11, 0b101, 0b1010, 0b0010, 0b010, RwSafe);
        $macro!(PIRE0_EL2, 0b11, 0b100, 0b1010, 0b0010, 0b010, RwSafe);
        $macro!(PM, 0b11, 0b000, 0b0100, 0b0011, 0b001, RwSafe);
        $macro!(PMBIDR_EL1, 0b11, 0b000, 0b1001, 0b1010, 0b111, Ro);
        $macro!(PMBLIMITR_EL1, 0b11, 0b000, 0b1001, 0b1010, 0b000, RwSafe);
        $macro!(PMBPTR_EL1, 0b11, 0b000, 0b1001, 0b1010, 0b001, RwSafe);
        $macro!(PMBSR_EL1, 0b11, 0b000, 0b1001, 0b1010, 0b011, RwSafe);
        $macro!(PMCCFILTR_EL0, 0b11, 0b011, 0b1110, 0b1111, 0b111, RwSafe);
        $macro!(PMCCNTR_EL0, 0b11, 0b011, 0b1001, 0b1101, 0b000, RwSafe);
        $macro!(PMCCNTSVR_EL1, 0b10, 0b000, 0b1110, 0b1011, 0b111, Ro);
        $macro!(PMCEID0_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b110, Ro);
        $macro!(PMCEID1_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b111, Ro);
        $macro!(PMCNTENCLR_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b010, RwSafe);
        $macro!(PMCNTENSET_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b001, RwSafe);
        $macro!(PMCR_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b000, RwSafe);
        $macro!(PMECR_EL1, 0b11, 0b000, 0b1001, 0b1110, 0b101, RwSafe);
        $macro!(PMIAR_EL1, 0b11, 0b000, 0b1001, 0b1110, 0b111, RwSafe);
        $macro!(PMICFILTR_EL0, 0b11, 0b011, 0b1001, 0b0110, 0b000, RwSafe);
        $macro!(PMICNTR_EL0, 0b11, 0b011, 0b1001, 0b0100, 0b000, RwSafe);
        $macro!(PMICNTSVR_EL1, 0b10, 0b000, 0b1110, 0b1100, 0b000, Ro);
        $macro!(PMINTENCLR_EL1, 0b11, 0b000, 0b1001, 0b1110, 0b010, RwSafe);
        $macro!(PMINTENSET_EL1, 0b11, 0b000, 0b1001, 0b1110, 0b001, RwSafe);
        $macro!(PMMIR_EL1, 0b11, 0b000, 0b1001, 0b1110, 0b110, Ro);
        $macro!(PMOVSCLR_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b011, RwSafe);
        $macro!(PMOVSSET_EL0, 0b11, 0b011, 0b1001, 0b1110, 0b011, RwSafe);
        $macro!(PMSCR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b000, RwSafe);
        $macro!(PMSCR_EL12, 0b11, 0b101, 0b1001, 0b1001, 0b000, RwSafe);
        $macro!(PMSCR_EL2, 0b11, 0b100, 0b1001, 0b1001, 0b000, RwSafe);
        $macro!(PMSDSFR_EL1, 0b11, 0b000, 0b1001, 0b1010, 0b100, RwSafe);
        $macro!(PMSELR_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b101, RwSafe);
        $macro!(PMSEVFR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b101, RwSafe);
        $macro!(PMSFCR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b100, RwSafe);
        $macro!(PMSICR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b010, RwSafe);
        $macro!(PMSIDR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b111, Ro);
        $macro!(PMSIRR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b011, RwSafe);
        $macro!(PMSLATFR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b110, RwSafe);
        $macro!(PMSNEVFR_EL1, 0b11, 0b000, 0b1001, 0b1001, 0b001, RwSafe);
        $macro!(PMSSCR_EL1, 0b11, 0b000, 0b1001, 0b1101, 0b011, RwSafe);
        $macro!(PMSWINC_EL0, 0b11, 0b011, 0b1001, 0b1100, 0b100, WoSafe);
        $macro!(PMUACR_EL1, 0b11, 0b000, 0b1001, 0b1110, 0b100, RwSafe);
        $macro!(PMUSERENR_EL0, 0b11, 0b011, 0b1001, 0b1110, 0b000, RwSafe);
        $macro!(PMXEVCNTR_EL0, 0b11, 0b011, 0b1001, 0b1101, 0b010, RwSafe);
        $macro!(PMXEVTYPER_EL0, 0b11, 0b011, 0b1001, 0b1101, 0b001, RwSafe);
        $macro!(PMZR_EL0, 0b11, 0b011, 0b1001, 0b1101, 0b100, WoSafe);
        $macro!(POR_EL0, 0b11, 0b011, 0b1010, 0b0010, 0b100, RwSafe);
        $macro!(POR_EL1, 0b11, 0b000, 0b1010, 0b0010, 0b100, RwSafe);
        $macro!(POR_EL12, 0b11, 0b101, 0b1010, 0b0010, 0b100, RwSafe);
        $macro!(POR_EL2, 0b11, 0b100, 0b1010, 0b0010, 0b100, RwSafe);
        $macro!(POR_EL3, 0b11, 0b110, 0b1010, 0b0010, 0b100, RwSafe);
        $macro!(RCWMASK_EL1, 0b11, 0b000, 0b1101, 0b0000, 0b110, RwSafe);
        $macro!(RCWSMASK_EL1, 0b11, 0b000, 0b1101, 0b0000, 0b011, RwSafe);
        $macro!(REVIDR_EL1, 0b11, 0b000, 0b0000, 0b0000, 0b110, Ro);
        $macro!(RGSR_EL1, 0b11, 0b000, 0b0001, 0b0000, 0b101, RwSafe);
        $macro!(RMR_EL1, 0b11, 0b000, 0b1100, 0b0000, 0b010, RwSafe);
        $macro!(RMR_EL2, 0b11, 0b100, 0b1100, 0b0000, 0b010, RwSafe);
        $macro!(RMR_EL3, 0b11, 0b110, 0b1100, 0b0000, 0b010, RwSafe);
        $macro!(RNDR, 0b11, 0b011, 0b0010, 0b0100, 0b000, Ro);
        $macro!(RNDRRS, 0b11, 0b011, 0b0010, 0b0100, 0b001, Ro);
        $macro!(RVBAR_EL1, 0b11, 0b000, 0b1100, 0b0000, 0b001, Ro);
        $macro!(RVBAR_EL2, 0b11, 0b100, 0b1100, 0b0000, 0b001, Ro);
        $macro!(RVBAR_EL3, 0b11, 0b110, 0b1100, 0b0000, 0b001, Ro);
        $macro!(SCR_EL3, 0b11, 0b110, 0b0001, 0b0001, 0b000, RwSafe);

        //
        // Can disable paging, and change cache coherency and memory ordering semantics.
        //
        $macro!(SCTLR_EL1, 0b11, 0b000, 0b0001, 0b0000, 0b000, RwUnsafe);
        $macro!(SCTLR_EL12, 0b11, 0b101, 0b0001, 0b0000, 0b000, RwUnsafe);
        $macro!(SCTLR_EL2, 0b11, 0b100, 0b0001, 0b0000, 0b000, RwUnsafe);
        $macro!(SCTLR_EL3, 0b11, 0b110, 0b0001, 0b0000, 0b000, RwUnsafe);
        $macro!(SCTLR2_EL1, 0b11, 0b000, 0b0001, 0b0000, 0b011, RwUnsafe);
        $macro!(SCTLR2_EL12, 0b11, 0b101, 0b0001, 0b0000, 0b011, RwUnsafe);
        $macro!(SCTLR2_EL2, 0b11, 0b100, 0b0001, 0b0000, 0b011, RwUnsafe);
        $macro!(SCTLR2_EL3, 0b11, 0b110, 0b0001, 0b0000, 0b011, RwUnsafe);

        $macro!(SCXTNUM_EL0, 0b11, 0b011, 0b1101, 0b0000, 0b111, RwSafe);
        $macro!(SCXTNUM_EL1, 0b11, 0b000, 0b1101, 0b0000, 0b111, RwSafe);
        $macro!(SCXTNUM_EL12, 0b11, 0b101, 0b1101, 0b0000, 0b111, RwSafe);
        $macro!(SCXTNUM_EL2, 0b11, 0b100, 0b1101, 0b0000, 0b111, RwSafe);
        $macro!(SCXTNUM_EL3, 0b11, 0b110, 0b1101, 0b0000, 0b111, RwSafe);
        $macro!(SDER32_EL2, 0b11, 0b100, 0b0001, 0b0011, 0b001, RwSafe);
        $macro!(SDER32_EL3, 0b11, 0b110, 0b0001, 0b0001, 0b001, RwSafe);
        $macro!(SMCR_EL1, 0b11, 0b000, 0b0001, 0b0010, 0b110, RwSafe);
        $macro!(SMCR_EL12, 0b11, 0b101, 0b0001, 0b0010, 0b110, RwSafe);
        $macro!(SMCR_EL2, 0b11, 0b100, 0b0001, 0b0010, 0b110, RwSafe);
        $macro!(SMCR_EL3, 0b11, 0b110, 0b0001, 0b0010, 0b110, RwSafe);
        $macro!(SMIDR_EL1, 0b11, 0b001, 0b0000, 0b0000, 0b110, Ro);
        $macro!(SMPRI_EL1, 0b11, 0b000, 0b0001, 0b0010, 0b100, RwSafe);
        $macro!(SMPRIMAP_EL2, 0b11, 0b100, 0b0001, 0b0010, 0b101, RwSafe);

        //
        // Can change active stack pointer, corrupting stack frames.
        //
        $macro!(SP_EL0, 0b11, 0b000, 0b0100, 0b0001, 0b000, RwUnsafe);
        $macro!(SP_EL1, 0b11, 0b100, 0b0100, 0b0001, 0b000, RwUnsafe);
        $macro!(SP_EL2, 0b11, 0b110, 0b0100, 0b0001, 0b000, RwUnsafe);

        $macro!(SPMACCESSR_EL1, 0b10, 0b000, 0b1001, 0b1101, 0b011, RwSafe);
        $macro!(SPMACCESSR_EL12, 0b10, 0b101, 0b1001, 0b1101, 0b011, RwSafe);
        $macro!(SPMACCESSR_EL2, 0b10, 0b100, 0b1001, 0b1101, 0b011, RwSafe);
        $macro!(SPMACCESSR_EL3, 0b10, 0b110, 0b1001, 0b1101, 0b011, RwSafe);
        $macro!(SPMCFGR_EL1, 0b10, 0b000, 0b1001, 0b1101, 0b111, Ro);
        $macro!(SPMCNTENCLR_EL0, 0b10, 0b011, 0b1001, 0b1100, 0b010, RwSafe);
        $macro!(SPMCNTENSET_EL0, 0b10, 0b011, 0b1001, 0b1100, 0b001, RwSafe);
        $macro!(SPMCR_EL0, 0b10, 0b011, 0b1001, 0b1100, 0b000, RwSafe);
        $macro!(SPMDEVAFF_EL1, 0b10, 0b000, 0b1001, 0b1101, 0b110, Ro);
        $macro!(SPMDEVARCH_EL1, 0b10, 0b000, 0b1001, 0b1101, 0b101, Ro);
        $macro!(SPMIIDR_EL1, 0b10, 0b000, 0b1001, 0b1101, 0b100, Ro);
        $macro!(SPMINTENCLR_EL1, 0b10, 0b000, 0b1001, 0b1110, 0b010, RwSafe);
        $macro!(SPMINTENSET_EL1, 0b10, 0b000, 0b1001, 0b1110, 0b001, RwSafe);
        $macro!(SPMOVSCLR_EL0, 0b10, 0b011, 0b1001, 0b1100, 0b011, RwSafe);
        $macro!(SPMOVSSET_EL0, 0b10, 0b011, 0b1001, 0b1110, 0b011, RwSafe);
        $macro!(SPMRoOTCR_EL3, 0b10, 0b110, 0b1001, 0b1110, 0b111, RwSafe);
        $macro!(SPMSCR_EL1, 0b10, 0b111, 0b1001, 0b1110, 0b111, RwSafe);
        $macro!(SPMSELR_EL0, 0b10, 0b011, 0b1001, 0b1100, 0b101, RwSafe);
        $macro!(SPMZR_EL0, 0b10, 0b011, 0b1001, 0b1100, 0b100, WoSafe);

        //
        // Can change saved execution state restored on return.
        //
        $macro!(SPSR_EL1, 0b11, 0b000, 0b0100, 0b0000, 0b000, RwUnsafe);
        $macro!(SPSR_EL12, 0b11, 0b101, 0b0100, 0b0000, 0b000, RwUnsafe);
        $macro!(SPSR_EL2, 0b11, 0b100, 0b0100, 0b0000, 0b000, RwUnsafe);
        $macro!(SPSR_EL3, 0b11, 0b110, 0b0100, 0b0000, 0b000, RwUnsafe);
        $macro!(SPSR_abt, 0b11, 0b100, 0b0100, 0b0011, 0b001, RwUnsafe);
        $macro!(SPSR_fiq, 0b11, 0b100, 0b0100, 0b0011, 0b011, RwUnsafe);
        $macro!(SPSR_irq, 0b11, 0b100, 0b0100, 0b0011, 0b000, RwUnsafe);
        $macro!(SPSR_und, 0b11, 0b100, 0b0100, 0b0011, 0b010, RwUnsafe);

        // Can change active stack pointer, corrupting stack frames.
        $macro!(SPSel, 0b11, 0b000, 0b0100, 0b0010, 0b000, RwUnsafe);

        $macro!(SSBS, 0b11, 0b011, 0b0100, 0b0010, 0b110, RwSafe);
        $macro!(SVCR, 0b11, 0b011, 0b0100, 0b0010, 0b010, RwSafe);
        $macro!(S2PIR_EL2, 0b11, 0b100, 0b1010, 0b0010, 0b101, RwSafe);
        $macro!(S2POR_EL1, 0b11, 0b000, 0b1010, 0b0010, 0b101, RwSafe);
        $macro!(TCO, 0b11, 0b011, 0b0100, 0b0010, 0b111, RwSafe);

        //
        // Can change address bounds and translation walk attributes.
        //
        $macro!(TCR_EL1, 0b11, 0b000, 0b0010, 0b0000, 0b010, RwUnsafe);
        $macro!(TCR_EL12, 0b11, 0b101, 0b0010, 0b0000, 0b010, RwUnsafe);
        $macro!(TCR_EL2, 0b11, 0b100, 0b0010, 0b0000, 0b010, RwUnsafe);
        $macro!(TCR_EL3, 0b11, 0b110, 0b0010, 0b0000, 0b010, RwUnsafe);
        $macro!(TCR2_EL1, 0b11, 0b000, 0b0010, 0b0000, 0b011, RwUnsafe);
        $macro!(TCR2_EL12, 0b11, 0b101, 0b0010, 0b0000, 0b011, RwUnsafe);
        $macro!(TCR2_EL2, 0b11, 0b100, 0b0010, 0b0000, 0b011, RwUnsafe);

        $macro!(TFSR_EL1, 0b11, 0b000, 0b0101, 0b0110, 0b000, RwSafe);
        $macro!(TFSR_EL12, 0b11, 0b101, 0b0101, 0b0110, 0b000, RwSafe);
        $macro!(TFSR_EL2, 0b11, 0b100, 0b0101, 0b0110, 0b000, RwSafe);
        $macro!(TFSR_EL3, 0b11, 0b110, 0b0101, 0b0110, 0b000, RwSafe);
        $macro!(TFSRE0_EL1, 0b11, 0b000, 0b0101, 0b0110, 0b001, RwSafe);
        $macro!(TPIDR_EL0, 0b11, 0b011, 0b1101, 0b0000, 0b010, RwSafe);
        $macro!(TPIDR_EL1, 0b11, 0b000, 0b1101, 0b0000, 0b100, RwSafe);
        $macro!(TPIDR_EL2, 0b11, 0b100, 0b1101, 0b0000, 0b010, RwSafe);
        $macro!(TPIDR_EL3, 0b11, 0b110, 0b1101, 0b0000, 0b010, RwSafe);
        $macro!(TPIDRRo_EL0, 0b11, 0b011, 0b1101, 0b0000, 0b011, RwSafe);
        $macro!(TPIDR2_EL0, 0b11, 0b011, 0b1101, 0b0000, 0b101, RwSafe);
        $macro!(TRBBASER_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b010, RwSafe);
        $macro!(TRBIDR_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b111, Ro);
        $macro!(TRBLIMITR_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b000, RwSafe);
        $macro!(TRBMAR_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b100, RwSafe);
        $macro!(TRBMPAM_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b101, RwSafe);
        $macro!(TRBPTR_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b001, RwSafe);
        $macro!(TRBSR_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b011, RwSafe);
        $macro!(TRBTRG_EL1, 0b11, 0b000, 0b1001, 0b1011, 0b110, RwSafe);
        $macro!(TRCAUTHSTATUS, 0b10, 0b001, 0b0111, 0b1110, 0b110, Ro);
        $macro!(TRCAUXCTLR, 0b10, 0b001, 0b0000, 0b0110, 0b000, RwSafe);
        $macro!(TRCBBCTLR, 0b10, 0b001, 0b0000, 0b1111, 0b000, RwSafe);
        $macro!(TRCCCCTLR, 0b10, 0b001, 0b0000, 0b1110, 0b000, RwSafe);
        $macro!(TRCCIDCCTLR0, 0b10, 0b001, 0b0011, 0b0000, 0b010, RwSafe);
        $macro!(TRCCIDCCTLR1, 0b10, 0b001, 0b0011, 0b0001, 0b010, RwSafe);
        $macro!(TRCCLAIMCLR, 0b10, 0b001, 0b0111, 0b1001, 0b110, RwSafe);
        $macro!(TRCCLAIMSET, 0b10, 0b001, 0b0111, 0b1000, 0b110, RwSafe);
        $macro!(TRCCONFIGR, 0b10, 0b001, 0b0000, 0b0100, 0b000, RwSafe);
        $macro!(TRCDEVARCH, 0b10, 0b001, 0b0111, 0b1111, 0b110, Ro);
        $macro!(TRCDEVID, 0b10, 0b001, 0b0111, 0b0010, 0b111, Ro);
        $macro!(TRCEVENTCTL0R, 0b10, 0b001, 0b0000, 0b1000, 0b000, RwSafe);
        $macro!(TRCEVENTCTL1R, 0b10, 0b001, 0b0000, 0b1001, 0b000, RwSafe);
        $macro!(TRCIDR0, 0b10, 0b001, 0b0000, 0b1000, 0b111, Ro);
        $macro!(TRCIDR1, 0b10, 0b001, 0b0000, 0b1001, 0b111, Ro);
        $macro!(TRCIDR10, 0b10, 0b001, 0b0000, 0b0010, 0b110, Ro);
        $macro!(TRCIDR11, 0b10, 0b001, 0b0000, 0b0011, 0b110, Ro);
        $macro!(TRCIDR12, 0b10, 0b001, 0b0000, 0b0100, 0b110, Ro);
        $macro!(TRCIDR13, 0b10, 0b001, 0b0000, 0b0101, 0b110, Ro);
        $macro!(TRCIDR2, 0b10, 0b001, 0b0000, 0b1010, 0b111, Ro);
        $macro!(TRCIDR3, 0b10, 0b001, 0b0000, 0b1011, 0b111, Ro);
        $macro!(TRCIDR4, 0b10, 0b001, 0b0000, 0b1100, 0b111, Ro);
        $macro!(TRCIDR5, 0b10, 0b001, 0b0000, 0b1101, 0b111, Ro);
        $macro!(TRCIDR6, 0b10, 0b001, 0b0000, 0b1110, 0b111, Ro);
        $macro!(TRCIDR7, 0b10, 0b001, 0b0000, 0b1111, 0b111, Ro);
        $macro!(TRCIDR8, 0b10, 0b001, 0b0000, 0b0000, 0b110, Ro);
        $macro!(TRCIDR9, 0b10, 0b001, 0b0000, 0b0001, 0b110, Ro);
        $macro!(TRCIMSPEC0, 0b10, 0b001, 0b0000, 0b0000, 0b111, RwSafe);
        $macro!(TRCITECR_EL1, 0b11, 0b000, 0b0001, 0b0010, 0b011, RwSafe);
        $macro!(TRCITECR_EL12, 0b11, 0b101, 0b0001, 0b0010, 0b011, RwSafe);
        $macro!(TRCITECR_EL2, 0b11, 0b100, 0b0001, 0b0010, 0b011, RwSafe);
        $macro!(TRCITEEDCR, 0b10, 0b001, 0b0000, 0b0010, 0b001, RwSafe);
        $macro!(TRCOSLSR, 0b10, 0b001, 0b0001, 0b0001, 0b100, Ro);
        $macro!(TRCPRGCTLR, 0b10, 0b001, 0b0000, 0b0001, 0b000, RwSafe);
        $macro!(TRCQCTLR, 0b10, 0b001, 0b0000, 0b0001, 0b001, RwSafe);
        $macro!(TRCRSR, 0b10, 0b001, 0b0000, 0b1010, 0b000, RwSafe);
        $macro!(TRCSEQRSTEVR, 0b10, 0b001, 0b0000, 0b0110, 0b100, RwSafe);
        $macro!(TRCSEQSTR, 0b10, 0b001, 0b0000, 0b0111, 0b100, RwSafe);
        $macro!(TRCSTALLCTLR, 0b10, 0b001, 0b0000, 0b1011, 0b000, RwSafe);
        $macro!(TRCSTATR, 0b10, 0b001, 0b0000, 0b0011, 0b000, Ro);
        $macro!(TRCSYNCPR, 0b10, 0b001, 0b0000, 0b1101, 0b000, RwSafe);
        $macro!(TRCTRACEIDR, 0b10, 0b001, 0b0000, 0b0000, 0b001, RwSafe);
        $macro!(TRCTSCTLR, 0b10, 0b001, 0b0000, 0b1100, 0b000, RwSafe);
        $macro!(TRCVICTLR, 0b10, 0b001, 0b0000, 0b0000, 0b010, RwSafe);
        $macro!(TRCVIIECTLR, 0b10, 0b001, 0b0000, 0b0001, 0b010, RwSafe);
        $macro!(TRCVIPCSSCTLR, 0b10, 0b001, 0b0000, 0b0011, 0b010, RwSafe);
        $macro!(TRCVISSCTLR, 0b10, 0b001, 0b0000, 0b0010, 0b010, RwSafe);
        $macro!(TRCVMIDCCTLR0, 0b10, 0b001, 0b0011, 0b0010, 0b010, RwSafe);
        $macro!(TRCVMIDCCTLR1, 0b10, 0b001, 0b0011, 0b0011, 0b010, RwSafe);
        $macro!(TRFCR_EL1, 0b11, 0b000, 0b0001, 0b0010, 0b001, RwSafe);
        $macro!(TRFCR_EL12, 0b11, 0b101, 0b0001, 0b0010, 0b001, RwSafe);
        $macro!(TRFCR_EL2, 0b11, 0b100, 0b0001, 0b0010, 0b001, RwSafe);

        //
        // Can change memory mappings.
        //
        $macro!(TTBR0_EL1, 0b11, 0b000, 0b0010, 0b0000, 0b000, RwUnsafe);
        $macro!(TTBR0_EL12, 0b11, 0b101, 0b0010, 0b0000, 0b000, RwUnsafe);
        $macro!(TTBR0_EL2, 0b11, 0b100, 0b0010, 0b0000, 0b000, RwUnsafe);
        $macro!(TTBR0_EL3, 0b11, 0b110, 0b0010, 0b0000, 0b000, RwUnsafe);
        $macro!(TTBR1_EL1, 0b11, 0b000, 0b0010, 0b0000, 0b001, RwUnsafe);
        $macro!(TTBR1_EL12, 0b11, 0b101, 0b0010, 0b0000, 0b001, RwUnsafe);
        $macro!(TTBR1_EL2, 0b11, 0b100, 0b0010, 0b0000, 0b001, RwUnsafe);

        $macro!(UAO, 0b11, 0b000, 0b0100, 0b0010, 0b100, RwSafe);
        $macro!(VBAR_EL1, 0b11, 0b000, 0b1100, 0b0000, 0b000, RwSafe);
        $macro!(VBAR_EL12, 0b11, 0b101, 0b1100, 0b0000, 0b000, RwSafe);
        $macro!(VBAR_EL2, 0b11, 0b100, 0b1100, 0b0000, 0b000, RwSafe);
        $macro!(VBAR_EL3, 0b11, 0b110, 0b1100, 0b0000, 0b000, RwSafe);
        $macro!(VDISR_EL2, 0b11, 0b100, 0b1100, 0b0001, 0b001, RwSafe);
        $macro!(VDISR_EL3, 0b11, 0b110, 0b1100, 0b0001, 0b001, RwSafe);
        $macro!(VMECID_A_EL2, 0b11, 0b100, 0b1010, 0b1001, 0b001, RwSafe);
        $macro!(VMECID_P_EL2, 0b11, 0b100, 0b1010, 0b1001, 0b000, RwSafe);
        $macro!(VMPIDR_EL2, 0b11, 0b100, 0b0000, 0b0000, 0b101, RwSafe);

        // Safe-writable, since the writer's execution context is not subject to
        // the VNCR page.
        $macro!(VNCR_EL2, 0b11, 0b100, 0b0010, 0b0010, 0b000, RwSafe);

        $macro!(VPIDR_EL2, 0b11, 0b100, 0b0000, 0b0000, 0b000, RwSafe);
        $macro!(VSESR_EL2, 0b11, 0b100, 0b0101, 0b0010, 0b011, RwSafe);
        $macro!(VSESR_EL3, 0b11, 0b110, 0b0101, 0b0010, 0b011, RwSafe);

        // Safe-writable, since the writer's execution context is not subject to
        // the related (stage 2) memory mappings.
        $macro!(VSTCR_EL2, 0b11, 0b100, 0b0010, 0b0110, 0b010, RwSafe);

        $macro!(VSTTBR_EL2, 0b11, 0b100, 0b0010, 0b0110, 0b000, RwSafe);
        $macro!(VTCR_EL2, 0b11, 0b100, 0b0010, 0b0001, 0b010, RwSafe);
        $macro!(VTTBR_EL2, 0b11, 0b100, 0b0010, 0b0001, 0b000, RwSafe);
        $macro!(ZCR_EL1, 0b11, 0b000, 0b0001, 0b0010, 0b000, RwSafe);
        $macro!(ZCR_EL12, 0b11, 0b101, 0b0001, 0b0010, 0b000, RwSafe);
        $macro!(ZCR_EL2, 0b11, 0b100, 0b0001, 0b0010, 0b000, RwSafe);
        $macro!(ZCR_EL3, 0b11, 0b110, 0b0001, 0b0010, 0b000, RwSafe);
    };
}

macro_rules! define_spec {
    ($mnemonic:ident, $op0:literal, $op1:literal, $crn:literal, $crm:literal, $op2:literal, $access:ty) => {
        pub type $mnemonic = SysRegSpec<$op0, $op1, $crn, $crm, $op2, $access>;
    };
}

for_each_spec!(define_spec);
