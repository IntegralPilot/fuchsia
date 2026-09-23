// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::layout;
use regio::RwSafe;
use regio::traits::{ReadReg, RwSafeReg};
use regio::x86::{Cpuid, Msr};

use super::ArchCapabilitiesMsr;
use super::cpuid::EXTENDED_FEATURES_B;

/// [intel/vol4]: Table 2-2.  IA-32 Architectural MSRs (Contd.).
///
/// TSX (Transactional Synchronization Extension) controls.
pub const IA32_TSX_CTRL: Msr<0x0000_0122, TsxControlMsr, RwSafe> = Msr::new();

layout!({
    /// Layout for [`IA32_TSX_CTRL`].
    pub struct TsxControlMsr(u64);
    {
        let __ @ 63..2;
        let rtm_disable @ 1;
        let tsx_cpuid_clear @ 0;
    }
});

impl TsxControlMsr {
    fn is_supported(cpuid: impl Cpuid, msr: impl ReadReg<ArchCapabilitiesMsr>) -> bool {
        ArchCapabilitiesMsr::is_supported(cpuid) && msr.read().tsx_ctrl()
    }
}

pub fn tsx_is_supported(cpuid: impl Cpuid) -> bool {
    // [intel/vol3]: 18.3.6.5     Performance Monitoring and Intel® TSX.
    let features = cpuid.read(EXTENDED_FEATURES_B);
    features.hle() || features.rtm()
}

/// Attempts to disable TSX and returns whether it was successful.
pub fn disable_tsx(
    cpuid: impl Cpuid,
    arch_capabilities_msr: impl ReadReg<ArchCapabilitiesMsr>,
    tsx_control_msr: impl RwSafeReg<TsxControlMsr>,
) -> bool {
    if !TsxControlMsr::is_supported(&cpuid, &arch_capabilities_msr) {
        return false;
    }

    tsx_control_msr.modify(|val| {
        val.set_rtm_disable(true).set_tsx_cpuid_clear(true);
    });

    true
}
