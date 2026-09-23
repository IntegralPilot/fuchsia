// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This file contains utilities related to probing and mitigating architectural
//! bugs and vulnerabilities.
//!
//! In general, we cannot rely on the official means of enumerating whether a
//! vulnerability is present. For example, it might only be enumerable after
//! certain microcode updates are performed. Accordingly, if we cannot get an
//! definitive "is not vulnerable" from the official means, we fall back to
//! pessimistically assigning vulnerability on the basis of microarchitecture,
//! making implicit reference to the following documents:
//!
//! * Intel:
//!   https://software.intel.com/security-software-guidance/processors-affected-transient-execution-attack-mitigation-product-cpu-model
//!   Discontinued models (e.g., Core 2, Nehalem, and Westmere) are not present
//!   in the table; in those cases, we assume vulnerability by default, unless
//!   otherwise mentions.
//!
//! * AMD: https://www.amd.com/en/corporate/product-security
//!
//! Further pessimistically, we default to assigning vulnerability in the case
//! unknown architectures.
//!
//! For non-architectural MSRs whose fields give workarounds to a specific
//! erratum, if no further qualification is given (e.g., a field name/mnemonic),
//! we name the field "erratum_${ID}_workaround".

use super::cpuid::{
    EXTENDED_AMD_FEATURES_B, EXTENDED_FEATURES_D, FEATURE_FLAGS_C, Microarchitecture, VERSION_INFO,
    VendorString,
};
use super::{
    ArchCapabilitiesMsr, SpeculationControlMsr, Vendor, VirtualSpeculationControlMsr, has_ibrs,
    has_stibp, tsx_is_supported,
};
use bitrs::layout;
use regio::traits::{ReadReg, RwSafeReg, SafeWriteReg};
use regio::x86::{Cpuid, Msr};
use regio::{LayoutOver, RwSafe};

/// Whether the CPU is susceptible to swapgs speculation attacks:
/// https://software.intel.com/security-software-guidance/advisory-guidance/speculative-behavior-swapgs-and-segment-registers
///
/// CVE-2019-1125.
pub fn has_x86_swapgs_bug(cpuid: &impl Cpuid) -> bool {
    let vendor = VendorString::from_cpuid(cpuid).vendor();
    match vendor {
        // All Intel CPUs seem to be affected and there is no indication that they
        // intend to fix this.
        Vendor::Unknown | Vendor::Intel => true,
        Vendor::Amd => false,
    }
}

/// Whether the CPU is susceptible to any of the Microarchitectural Data
/// Sampling (MDS) bugs.
///
/// CVE-2018-12126, CVE-2018-12127, CVE-2018-12130, CVE-2019-11091.
pub fn has_x86_mds_bugs(cpuid: &impl Cpuid, msr: &impl ReadReg<ArchCapabilitiesMsr>) -> bool {
    // https://software.intel.com/security-software-guidance/resources/processors-affected-microarchitectural-data-sampling
    if ArchCapabilitiesMsr::is_supported(cpuid) && msr.read().mds_no() {
        return false;
    }
    let version_info = cpuid.read(VERSION_INFO);
    let vendor = VendorString::from_cpuid(cpuid).vendor();
    let march = version_info.microarchitecture(vendor);
    match march {
        Microarchitecture::Unknown
        | Microarchitecture::IntelCore2
        | Microarchitecture::IntelNehalem
        | Microarchitecture::IntelWestmere
        | Microarchitecture::IntelSandyBridge
        | Microarchitecture::IntelIvyBridge
        | Microarchitecture::IntelHaswell
        | Microarchitecture::IntelBroadwell
        | Microarchitecture::IntelSkylake
        | Microarchitecture::IntelSkylakeServer
        | Microarchitecture::IntelCannonLake
        | Microarchitecture::IntelIceLake
        | Microarchitecture::IntelSilvermont
        | Microarchitecture::IntelAirmont => true,

        Microarchitecture::IntelTigerLake
        | Microarchitecture::IntelAlderLake
        | Microarchitecture::IntelRaptorLake
        | Microarchitecture::IntelBonnell
        | Microarchitecture::IntelSaltwell
        | Microarchitecture::IntelGoldmont
        | Microarchitecture::IntelGoldmontPlus
        | Microarchitecture::IntelTremont
        | Microarchitecture::AmdFamilyBulldozer
        | Microarchitecture::AmdFamilyJaguar
        | Microarchitecture::AmdFamilyZen
        | Microarchitecture::AmdFamilyZen3 => false,
    }
}

/// Whether the CPU is susceptible to the TSX Asynchronous Abort (TAA) bug.
///
/// CVE-2019-11135.
pub fn has_x86_taa_bug(cpuid: &impl Cpuid, msr: &impl ReadReg<ArchCapabilitiesMsr>) -> bool {
    // https://software.intel.com/security-software-guidance/advisory-guidance/intel-transactional-synchronization-extensions-intel-tsx-asynchronous-abort
    //
    // A processor is affected by TAA if both of the following are true:
    // * CPU supports TSX (indicated by the HLE or RTM features);
    // * CPU does not enumerate TAA_NO.
    let taa_no = ArchCapabilitiesMsr::is_supported(cpuid) && msr.read().taa_no();
    if !tsx_is_supported(cpuid) || taa_no {
        return false;
    }
    let version_info = cpuid.read(VERSION_INFO);
    let vendor = VendorString::from_cpuid(cpuid).vendor();
    match version_info.microarchitecture(vendor) {
        Microarchitecture::Unknown
        | Microarchitecture::IntelHaswell
        | Microarchitecture::IntelBroadwell
        | Microarchitecture::IntelSkylake
        | Microarchitecture::IntelSkylakeServer
        | Microarchitecture::IntelCannonLake
        | Microarchitecture::IntelIceLake => true,

        /* Does not implement TSX. */
        Microarchitecture::IntelCore2
        /* Does not implement TSX. */
        | Microarchitecture::IntelNehalem
        /* Does not implement TSX. */
        | Microarchitecture::IntelWestmere
        | Microarchitecture::IntelSandyBridge
        | Microarchitecture::IntelIvyBridge
        | Microarchitecture::IntelTigerLake
        | Microarchitecture::IntelAlderLake
        | Microarchitecture::IntelRaptorLake
        | Microarchitecture::IntelBonnell
        | Microarchitecture::IntelSaltwell
        | Microarchitecture::IntelSilvermont
        | Microarchitecture::IntelAirmont
        | Microarchitecture::IntelGoldmont
        | Microarchitecture::IntelGoldmontPlus
        | Microarchitecture::IntelTremont
        | Microarchitecture::AmdFamilyBulldozer
        | Microarchitecture::AmdFamilyJaguar
        | Microarchitecture::AmdFamilyZen
        | Microarchitecture::AmdFamilyZen3 => false,
    }
}

/// Whether the CPU is susceptible to any of the MDS or TAA bugs, which are
/// closely related and similarly mitigated.
pub fn has_x86_mds_taa_bugs(cpuid: &impl Cpuid, msr: &impl ReadReg<ArchCapabilitiesMsr>) -> bool {
    has_x86_mds_bugs(cpuid, msr) || has_x86_taa_bug(cpuid, msr)
}

/// Whether the MDS/TAA bugs can be mitigated, which all make use of the same
/// method (MD_CLEAR):
/// https://software.intel.com/security-software-guidance/deep-dives/deep-dive-intel-analysis-microarchitectural-data-sampling#mitigation4processors
pub fn can_mitigate_x86_mds_taa_bugs(cpuid: &impl Cpuid) -> bool {
    cpuid.read(EXTENDED_FEATURES_D).md_clear()
}

/// Whether the CPU is susceptible to the Speculative Store Bypass (SSB) bug:
/// https://software.intel.com/security-software-guidance/advisory-guidance/speculative-store-bypass
///
/// CVE-2018-3639.
pub fn has_x86_ssb_bug(cpuid: &impl Cpuid, msr: &impl ReadReg<ArchCapabilitiesMsr>) -> bool {
    // Check if the processor explicitly advertises that it is not affected, in
    // both the Intel and AMD ways.
    if ArchCapabilitiesMsr::is_supported(cpuid) && msr.read().ssb_no() {
        return false;
    }
    if cpuid.supports(EXTENDED_AMD_FEATURES_B) && cpuid.read(EXTENDED_AMD_FEATURES_B).ssb_no() {
        return false;
    }
    let version_info = cpuid.read(VERSION_INFO);
    let vendor = VendorString::from_cpuid(cpuid).vendor();
    match version_info.microarchitecture(vendor) {
        Microarchitecture::Unknown
        | Microarchitecture::IntelCore2
        | Microarchitecture::IntelNehalem
        | Microarchitecture::IntelWestmere
        | Microarchitecture::IntelSandyBridge
        | Microarchitecture::IntelIvyBridge
        | Microarchitecture::IntelHaswell
        | Microarchitecture::IntelBroadwell
        | Microarchitecture::IntelSkylake
        | Microarchitecture::IntelSkylakeServer
        | Microarchitecture::IntelCannonLake
        | Microarchitecture::IntelIceLake
        | Microarchitecture::IntelTigerLake
        | Microarchitecture::IntelAlderLake
        | Microarchitecture::IntelRaptorLake
        | Microarchitecture::IntelGoldmont
        | Microarchitecture::IntelGoldmontPlus
        | Microarchitecture::IntelTremont
        | Microarchitecture::AmdFamilyBulldozer
        | Microarchitecture::AmdFamilyJaguar
        | Microarchitecture::AmdFamilyZen
        | Microarchitecture::AmdFamilyZen3 => true,
        Microarchitecture::IntelBonnell
        | Microarchitecture::IntelSaltwell
        | Microarchitecture::IntelSilvermont
        | Microarchitecture::IntelAirmont => false,
    }
}

/// Whether the CPU is susceptible to the Rogue Data Cache Load (Meltdown) bug:
/// https://software.intel.com/security-software-guidance/advisory-guidance/rogue-data-cache-load.
///
/// CVE-2017-5754.
pub fn has_x86_meltdown_bug(cpuid: &impl Cpuid, msr: &impl ReadReg<ArchCapabilitiesMsr>) -> bool {
    // Check if the processor explicitly advertises that it is not affected.
    if ArchCapabilitiesMsr::is_supported(cpuid) && msr.read().rdcl_no() {
        return false;
    }
    let version_info = cpuid.read(VERSION_INFO);
    let vendor = VendorString::from_cpuid(cpuid).vendor();
    let march = version_info.microarchitecture(vendor);
    match march {
        Microarchitecture::Unknown
        | Microarchitecture::IntelCore2
        | Microarchitecture::IntelNehalem
        | Microarchitecture::IntelWestmere
        | Microarchitecture::IntelSandyBridge
        | Microarchitecture::IntelIvyBridge
        | Microarchitecture::IntelHaswell
        | Microarchitecture::IntelBroadwell
        | Microarchitecture::IntelSkylake
        | Microarchitecture::IntelCannonLake => true,
        Microarchitecture::IntelIceLake
        | Microarchitecture::IntelTigerLake
        | Microarchitecture::IntelAlderLake
        | Microarchitecture::IntelRaptorLake
        | Microarchitecture::IntelBonnell
        | Microarchitecture::IntelSaltwell
        | Microarchitecture::IntelSilvermont
        | Microarchitecture::IntelAirmont
        | Microarchitecture::IntelGoldmont
        | Microarchitecture::IntelTremont
        | Microarchitecture::AmdFamilyBulldozer
        | Microarchitecture::AmdFamilyJaguar
        | Microarchitecture::AmdFamilyZen
        | Microarchitecture::AmdFamilyZen3 => false,
        // Special cases from the above table.
        Microarchitecture::IntelSkylakeServer => {
            let info = cpuid.read(VERSION_INFO);
            if info.stepping() >= 0x6 {
                // Cascade Lake server+
                return false;
            }
            true // Skylake server
        }
        Microarchitecture::IntelGoldmontPlus => {
            let info = cpuid.read(VERSION_INFO);
            if info.stepping() == 0x1 {
                // First stepping was suceptable to Meltdown.
                return true;
            }
            false
        }
    }
}

/// An architecturally prescribed mitigation for Spectre v2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpectreV2Mitigation {
    /// Enhanced/always-on IBRS (i.e., IBRS that can be enabled once without
    /// automatic disabling) is preferred alone.
    Ibrs,
    /// IBPB and/or retpoline are recommended.
    IbpbRetpoline,
    /// IBPB and/or retpoline are recommended - and STIPB, though not preferred as
    /// a performant mitigation, is also present and may be used.
    IbpbRetpolineStibp,
}

/// Returns the preferred Spectre v2 mitigation strategy.
pub fn get_preferred_spectre_v2_mitigation(
    cpuid: &impl Cpuid,
    msr: &impl ReadReg<ArchCapabilitiesMsr>,
) -> SpectreV2Mitigation {
    let vendor = VendorString::from_cpuid(cpuid).vendor();
    match vendor {
        Vendor::Unknown => {}
        Vendor::Intel => {
            // https://software.intel.com/security-software-guidance/advisory-guidance/branch-target-injection
            //
            // If enhanced IBRS are supported, it should be used for mitigation
            // instead of retpoline; else retpoline
            if has_ibrs(cpuid, msr, /*always_on_mode=*/ true) {
                return SpectreV2Mitigation::Ibrs;
            }
        }
        Vendor::Amd => {
            // [amd/ibc]: EXTENDED USAGE MODELS.
            // AMD further offers a feature bit to indicate whether IBRS is a
            // preferred mitigation strategy.
            if has_ibrs(cpuid, msr, /*always_on_mode=*/ true)
                && cpuid.supports(EXTENDED_AMD_FEATURES_B)
                && cpuid.read(EXTENDED_AMD_FEATURES_B).prefers_ibrs()
            {
                return SpectreV2Mitigation::Ibrs;
            }
            // [amd/ibc]: USAGE.
            // Though not recommended, STIPB is still a viable mitigation strategy.
            if has_stibp(cpuid, /*always_on_mode=*/ true) {
                return SpectreV2Mitigation::IbpbRetpolineStibp;
            }
        }
    }
    // Retpolines comprise an architecturally agnostic, pure software solution,
    // which makes it a sensible default strategy.
    SpectreV2Mitigation::IbpbRetpoline
}

/// [amd/ssbd] references bits 10, 33, 54.
/// [amd/rg/17h/00h-0Fh] references bits 4, 57.
pub const AMD_LOAD_STORE_CONFIGURATION: Msr<0xc001_1020, AmdLoadStoreConfigurationMsr, RwSafe> =
    Msr::new();

layout!({
    /// Layout for [`AMD_LOAD_STORE_CONFIGURATION`].
    pub struct AmdLoadStoreConfigurationMsr(u64);
    {
        let __ @ 63..58;
        let erratum_1095_workaround @ 57;
        let __ @ 56..55;
        let ssbd_15h @ 54;
        let __ @ 53..34;
        let ssbd_16h @ 33;
        let __ @ 32..11;
        let ssbd_17h @ 10;
        let __ @ 9..5;
        let erratum_1033_workaround @ 4;
        let __ @ 3..0;
    }
});

/// [amd/rg/17h/00h-0Fh] references bit 4.
pub const AMD_C0011028: Msr<0xc001_1028, AmdC0011028Msr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`AMD_C0011028`].
    pub struct AmdC0011028Msr(u64);
    {
        let __ @ 63..5;
        let erratum_1049_workaround @ 4;
        let __ @ 3..0;
    }
});

/// [amd/rg/17h/00h-0Fh] references bit 13.
pub const AMD_C0011029: Msr<0xc001_1029, AmdC0011029Msr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`AMD_C0011029`].
    pub struct AmdC0011029Msr(u64);
    {
        let __ @ 63..14;
        let erratum_1021_workaround @ 13;
        let __ @ 12..0;
    }
});

/// [amd/rg/17h/00h-0Fh] references bit 34.
pub const AMD_C001102D: Msr<0xc001_102d, AmdC001102dMsr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`AMD_C001102d`].
    pub struct AmdC001102dMsr(u64);
    {
        let __ @ 63..35;
        let erratum_1091_workaround @ 34;
        let __ @ 33..0;
    }
});

/// Attempt to mitigate the SSB bug. Return true if the bug was successfully
/// mitigated.
pub fn mitigate_x86_ssb_bug(
    cpuid: &impl Cpuid,
    speculation: &impl RwSafeReg<SpeculationControlMsr>,
    virtual_speculation: &impl RwSafeReg<VirtualSpeculationControlMsr>,
    load_store_configuration: &impl RwSafeReg<AmdLoadStoreConfigurationMsr>,
) -> bool {
    if cpuid.read(EXTENDED_FEATURES_D).ssbd() {
        debug_assert!(SpeculationControlMsr::is_supported(cpuid));
        speculation.modify(|value| *value.set_ssbd(true));
        return true;
    }
    if cpuid.supports(EXTENDED_AMD_FEATURES_B) {
        let amd_features = cpuid.read(EXTENDED_AMD_FEATURES_B);
        if amd_features.ssbd() {
            debug_assert!(SpeculationControlMsr::is_supported(cpuid));
            speculation.modify(|value| *value.set_ssbd(true));
            return true;
        }

        if amd_features.virt_ssbd() {
            debug_assert!(VirtualSpeculationControlMsr::is_supported(cpuid));
            virtual_speculation.modify(|value| *value.set_ssbd(true));
            return true;
        }
    }

    // [amd/ssbd]: NON-ARCHITECTURAL MSRS.
    //
    // There are non-architectural mechanisms to disable SSB for AMD families
    // 0x15-0x17.
    let version_info = cpuid.read(VERSION_INFO);
    let vendor = VendorString::from_cpuid(cpuid).vendor();
    match version_info.microarchitecture(vendor) {
        Microarchitecture::AmdFamilyBulldozer => {
            load_store_configuration.modify(|value| *value.set_ssbd_15h(true));
            true
        }
        Microarchitecture::AmdFamilyJaguar => {
            load_store_configuration.modify(|value| *value.set_ssbd_16h(true));
            true
        }
        Microarchitecture::AmdFamilyZen => {
            load_store_configuration.modify(|value| *value.set_ssbd_17h(true));
            true
        }
        _ => false,
    }
}

pub fn can_mitigate_x86_ssb_bug(cpuid: &impl Cpuid) -> bool {
    // With a null I/O provider, we can make the requisite checks without
    // actually committing the writes.

    struct NullMsr;

    impl<Layout: LayoutOver<u64>> ReadReg<Layout> for NullMsr {
        fn read(&self) -> Layout {
            Layout::from(0u64)
        }
    }

    impl<Layout> SafeWriteReg<Layout> for NullMsr {
        fn write(&self, _value: Layout) {}
    }

    mitigate_x86_ssb_bug(cpuid, &NullMsr, &NullMsr, &NullMsr)
}

/// Applies workarounds to processor-specific errata.
pub fn apply_x86_errata_workarounds(
    cpuid: &impl Cpuid,
    c0011028: &impl RwSafeReg<AmdC0011028Msr>,
    c0011029: &impl RwSafeReg<AmdC0011029Msr>,
    c001102d: &impl RwSafeReg<AmdC001102dMsr>,
    load_store: &impl RwSafeReg<AmdLoadStoreConfigurationMsr>,
) {
    if cpuid.read(FEATURE_FLAGS_C).hypervisor() {
        return;
    }

    let vendor = VendorString::from_cpuid(cpuid).vendor();
    match vendor {
        Vendor::Unknown => {}
        Vendor::Intel => {}
        Vendor::Amd => {
            let info = cpuid.read(VERSION_INFO);
            #[expect(clippy::single_match)]
            match info.family() {
                0x17 => {
                    #[expect(clippy::single_match)]
                    match info.model() {
                        // [amd/rg/17h/00h-0Fh].
                        0x00..=0x0f => {
                            // ZP-B1 refers to (model, stepping) == (1, 1); some of the errata are
                            // detailed as only applying to that CPU.
                            let zp_b1 = info.model() == 1 && info.stepping() == 1;
                            // 1021: Load Operation May Receive Stale Data From Older Store
                            //       Operation.
                            c0011029.modify(|value| *value.set_erratum_1021_workaround(true));

                            let mut lscfg = load_store.read();
                            // 1033: A Lock Operation May Cause the System to Hang.
                            if zp_b1 {
                                lscfg.set_erratum_1033_workaround(true);
                            }
                            // 1095: Potential Violation of Read Ordering In Lock Operation in SMT
                            //       Mode.
                            if true {
                                // TODO(https://fxbug.dev/42113091): Do not apply if SMT is
                                // disabled.
                                lscfg.set_erratum_1095_workaround(true);
                            }
                            load_store.write(lscfg);

                            // 1049: FCMOV Instruction May Not Execute Correctly.
                            c0011028.modify(|value| *value.set_erratum_1049_workaround(true));

                            // 1091: 4K Address Boundary Crossing Load Operation May Receive Stale
                            //       Data.
                            c001102d.modify(|value| *value.set_erratum_1091_workaround(true));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}
