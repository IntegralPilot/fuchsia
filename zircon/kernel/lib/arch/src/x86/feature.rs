// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::layout;
use regio::x86::{Cpuid, Msr};
use regio::{Ro, RwSafe};

use super::Vendor;
use super::cpuid::EXTENDED_FEATURES_D;

/// [intel/vol4]: Table 2-2.  IA-32 Architectural MSRs (Contd.).
///
/// Enumerates general archicturectural features.
pub const IA32_ARCH_CAPABILITIES: Msr<0x0000_010a, ArchCapabilitiesMsr, Ro> = Msr::new();

layout!({
    /// Layout for [`IA32_ARCH_CAPABILITIES`].
    pub struct ArchCapabilitiesMsr(u64);
    {
        let __ @ 63..9;
        let taa_no @ 8;
        let tsx_ctrl @ 7;
        let if_pschange_mc_no @ 6;
        let mds_no @ 5;
        let ssb_no @ 4;
        let skip_l1dfl_vmentry @ 3;
        let rsba @ 2;
        let ibrs_all @ 1;
        let rdcl_no @ 0;
    }
});

impl ArchCapabilitiesMsr {
    pub fn is_supported(cpuid: impl Cpuid) -> bool {
        cpuid.read(EXTENDED_FEATURES_D).ia32_arch_capabilities()
    }
}

/// [intel/vol4]: Table 2-3.  MSRs in Processors Based on Intel® Core™ Microarchitecture.
///
/// Enables miscellaenous processor features.
pub const IA32_MISC_ENABLE: Msr<0x0000_01a0, MiscFeaturesMsr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`IA32_MISC_ENABLE`].
    pub struct MiscFeaturesMsr(u64);
    {
        let __ @ 63..40;
        let ip_prefetch_disable @ 39;
        let ida_disable @ 38;
        let dcu_prefetch_disable @ 37;
        let __ @ 36..35;
        let xd_bit_disable @ 34;
        let __ @ 33..24;
        let xtpr_message_disable @ 23;
        let limit_cpuid_maxval @ 22;
        let __ @ 21;
        let eist_select_lock @ 20;
        let adjacent_cache_line_prefetch_disable @ 19;
        let monitor_fsm @ 18;
        let __ @ 17;
        let eist @ 16;
        let __ @ 15..14;
        let tm2 @ 13;
        let pebs_unavailable @ 12;
        let bts_unavailable @ 11;
        let ferr_mux @ 10;
        let hardware_prefetch_disable @ 9;
        let __ @ 8;
        let perf_mon_available @ 7;
        let __ @ 6..4;
        let automatic_thermal_control_circuit @ 3;
        let __ @ 2..1;
        let fast_strings @ 0;
    }
});

impl MiscFeaturesMsr {
    pub fn is_supported(vendor: Vendor) -> bool {
        vendor == Vendor::Intel
    }
}

/// [amd/ppr/17h/01h,08h]:  2.1.14.2 MSRs - MSRC000_0xxx.
///
/// AMD hardware configuration.
pub const MSRC001_0015: Msr<0xc001_0015, AmdHardwareConfigurationMsr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`MSRC001_0015`].
    pub struct AmdHardwareConfigurationMsr(u64);
    {
        let __ @ 63..31;
        let ir_perf_en @ 30;
        let __ @ 29..28;
        let eff_freq_read_only_lock @ 27;
        let eff_frq_cnt_mwait @ 26;
        let cpb_dis @ 25;
        let tsc_freq_sel @ 24;
        let __ @ 23..22;
        let lock_tsc_to_current_p0 @ 21;
        let io_cfg_gp_fault @ 20;
        let __ @ 19;
        let mc_status_wr_en @ 18;
        let wrap32_dis @ 17;
        let __ @ 16..15;
        let rsm_sp_cyc_dis @ 14;
        let smi_sp_cyc_dis @ 13;
        let __ @ 12..11;
        let mon_mwait_user_en @ 10;
        let mon_mwait_dis @ 9;
        let ignne_em @ 8;
        let allow_ferr_on_ne @ 7;
        let __ @ 6..5;
        let invdwbinvd @ 4;
        let tlb_cache_dis @ 3;
        let __ @ 2..1;
        let smm_lock @ 0;
    }
});

/// [intel/vol3]: 2.2.1 Extended Feature Enable Register
/// [amd/vol2]: 3.1.7 Extended Feature Enable Register (EFER)
pub const IA32_EFER: Msr<0xc000_0080, X86ExtendedFeatureEnableRegisterMsr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`IA32_EFER`].
    pub struct X86ExtendedFeatureEnableRegisterMsr(u64);
    {
        // Bits [18:12] are reserved in Intel docs, but further specified by AMD.
        // AMD documents the reserved bits among [63:9] as MBZ while Intel simply
        // says "reserved".
        let __ @ 63..19 = 0;
        let mcommit @ 17; // (AMD only) Enable mcommit instruction.
        let __ @ 16 = 0; // Reserved, MBZ in AMD.
        let tce @ 15; // (AMD only) Translation Cache Extension.
        let ffxsr @ 14; // (AMD only) Fast fxsave/fxrstor.
        let lmsle @ 13; // (AMD only) Long Mode Segment Limit Enable
        let svme @ 12; // (AMD only) Secure Virtual Machine Enable
        let nxe @ 11; // Enable non-execute bit in page tables.
        let lma @ 10; // IA-32e (x86-64) mode active.
        let __ @ 9 = 0; // Reserved, MBZ in AMD.
        let lme @ 8; // IA-32e (x86-64) mode enable.
        // Bits [7:1] are reserved.  AMD documents them as R(ead)A(s)Z(ero) while
        // Intel simply says "reserved".
        let __ @ 7..1 = 0;
        let sce @ 0; // Enable syscall/sysret instructions.
    }
});
