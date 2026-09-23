// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::ArchCapabilitiesMsr;
use super::cpuid::{EXTENDED_AMD_FEATURES_B, EXTENDED_FEATURES_D};
use bitrs::layout;
use regio::RwSafe;
use regio::traits::ReadReg;
use regio::x86::{Cpuid, Msr};

/// Speculation control.
///
/// [intel/vol4]: Table 2-2.  IA-32 Architectural MSRs (Contd.).
/// [amd/ibc]: PRESENCE.
/// [amd/ssbd]: PRESENCE.
pub const SPECULATION_CONTROL: Msr<0x0000_0048, SpeculationControlMsr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`SPECULATION_CONTROL`].
    pub struct SpeculationControlMsr(u64);
    {
        let __ @ 63..3;
        let ssbd @ 2;
        let stibp @ 1;
        let ibrs @ 0;
    }
});

impl SpeculationControlMsr {
    pub fn is_supported(cpuid: impl Cpuid) -> bool {
        // Intel documents that the MSR is supported only if one of the kinds of
        // speculation it can control is itself enumerated; AMD does similarly, but
        // that information must be cobbled together from [amd/ibc] and [amd/ssbd].

        // The Intel way:
        let intel_features = cpuid.read(EXTENDED_FEATURES_D);
        if intel_features.ibrs_ibpb() || intel_features.stibp() || intel_features.ssbd() {
            return true;
        }

        // The AMD way:
        if !cpuid.supports(EXTENDED_AMD_FEATURES_B) {
            return false;
        }
        let amd_features = cpuid.read(EXTENDED_AMD_FEATURES_B);
        amd_features.ibrs() || amd_features.stibp() || amd_features.ssbd()
    }
}

/// Virtual speculation control (e.g., for hypervisor usage).
///
/// [amd/ssbd]: PRESENCE.
pub const VIRTUAL_SPECULATION_CONTROL: Msr<0xc001_011f, VirtualSpeculationControlMsr, RwSafe> =
    Msr::new();

layout!({
    /// Layout for [`VIRTUAL_SPECULATION_CONTROL`].
    pub struct VirtualSpeculationControlMsr(u64);
    {
        let __ @ 63..3;
        let ssbd @ 2;
        let __ @ 1..0;
    }
});

impl VirtualSpeculationControlMsr {
    pub fn is_supported(cpuid: impl Cpuid) -> bool {
        // [amd/ssbd]: HYPERVISOR USAGE MODELS.
        cpuid.supports(EXTENDED_AMD_FEATURES_B) && cpuid.read(EXTENDED_AMD_FEATURES_B).virt_ssbd()
    }
}

/// Whether Indirect Branch Restricted Speculation (IBRS) is supported. The
/// "always on" mode refers to an optimization in which IBRS need only be
/// enabled once; IBRS in this mode are also referred to as "enhanced".
///
/// https://software.intel.com/security-software-guidance/deep-dives/deep-dive-indirect-branch-restricted-speculation.
pub fn has_ibrs(
    cpuid: &impl Cpuid,
    msr: &impl ReadReg<ArchCapabilitiesMsr>,
    always_on_mode: bool,
) -> bool {
    // The Intel way.
    let intel_always_on = ArchCapabilitiesMsr::is_supported(cpuid) && msr.read().ibrs_all();
    let intel_present = cpuid.read(EXTENDED_FEATURES_D).ibrs_ibpb();
    if intel_present && (!always_on_mode || intel_always_on) {
        return true;
    }

    // The AMD way.
    if cpuid.supports(EXTENDED_AMD_FEATURES_B) {
        let features = cpuid.read(EXTENDED_AMD_FEATURES_B);
        if features.ibrs() && (!always_on_mode || features.ibrs_always_on()) {
            return true;
        }
    }

    false
}

/// Whether Single Thread Indirect Branch Predictors (STIBP) are supported. The
/// "always on" mode refers to an optimization in which STIBP need only be
/// enabled once.
///
/// https://software.intel.com/security-software-guidance/deep-dives/deep-dive-single-thread-indirect-branch-predictors.
pub fn has_stibp(cpuid: &impl Cpuid, always_on_mode: bool) -> bool {
    // The Intel way.
    let intel_present = cpuid.read(EXTENDED_FEATURES_D).stibp();
    if intel_present && !always_on_mode {
        // Intel does not offer an "always on" mode.
        return true;
    }

    // The AMD way.
    if cpuid.supports(EXTENDED_AMD_FEATURES_B) {
        let features = cpuid.read(EXTENDED_AMD_FEATURES_B);
        if features.stibp() && (!always_on_mode || features.stibp_always_on()) {
            return true;
        }
    }
    false
}
