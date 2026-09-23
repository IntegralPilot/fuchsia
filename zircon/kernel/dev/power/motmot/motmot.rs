// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/power/motmot/power.cc
//
// Motmot SoC CPU Power and Performance Driver.
//
// Implements platform power lifecycle operations (reboot, shutdown, CPU on/off)
// for the Motmot platform via ARM PSCI firmware interfaces.

#[cfg(ktest)]
use unittest as _;

use crate::pdev_power::{PdevPowerOps, PowerRebootFlags, pdev_register_power};
use debug::dprintf;
use zx_status::Status;

// The base physical address of the PMU
const PMU_ALIVE_BASE: usize = 0x1746_0000;

// PMU docs, section 1.6.176
const SYSTEM_CONFIGURATION_REG: usize = PMU_ALIVE_BASE + 0x3a00;
const SWRESET_SYSTEM: u32 = 1 << 1;

// PMU docs, section 1.6.312
const PAD_CTRL_PWR_HOLD_REG: usize = PMU_ALIVE_BASE + 0x3e9c;
const PS_HOLD_CTRL_DATA: u32 = 1 << 8;

unsafe extern "C" {
    fn cpp_motmot_modify_register_via_smc(phys_addr: usize, mask: u32, val: u32) -> u64;
    fn cpp_motmot_cpu_off_wfi_loop() -> !;
    fn psci_cpu_on(hw_cpu_id: u64, entry: u64, context: u64) -> Result<(), Status>;
}

/// Modifies PMU registers via ARM SMC calls.
fn modify_register_via_smc(phys_addr: usize, mask: u32, val: u32) -> u64 {
    // SAFETY: Invokes SMC command to modify PMU registers.
    unsafe { cpp_motmot_modify_register_via_smc(phys_addr, mask, val) }
}

/// Reboots the Motmot platform via PMU SWRESET SMC call.
extern "C" fn motmot_reboot(flags: PowerRebootFlags) -> Result<(), Status> {
    match flags {
        PowerRebootFlags::Bootloader | PowerRebootFlags::Recovery => {
            dprintf!(INFO, "Motmot does not support rebooting into recovery or bootloader yet.\n");
        }
        PowerRebootFlags::Normal | PowerRebootFlags::Panic => {}
    }
    dprintf!(INFO, "Sending reboot command via SMC\n");
    let result = modify_register_via_smc(SYSTEM_CONFIGURATION_REG, SWRESET_SYSTEM, SWRESET_SYSTEM);
    modify_register_via_smc(SYSTEM_CONFIGURATION_REG, SWRESET_SYSTEM, SWRESET_SYSTEM);
    dprintf!(INFO, "Reboot command failed, result was {:#x}.\n", result);
    Err(Status::BAD_STATE)
}

/// Shuts down the Motmot platform by clearing PS_HOLD_CTRL_DATA via SMC call.
extern "C" fn motmot_shutdown() -> Result<(), Status> {
    dprintf!(INFO, "Sending shutdown command via SMC\n");
    let result = modify_register_via_smc(PAD_CTRL_PWR_HOLD_REG, PS_HOLD_CTRL_DATA, 0);
    dprintf!(INFO, "Shutdown command failed, result was {:#x}.\n", result);
    Err(Status::BAD_STATE)
}

/// Powers off the calling CPU core by looping on WFI with interrupts disabled.
extern "C" fn motmot_cpu_off() -> Result<(), Status> {
    // SAFETY: Disables interrupts and loops on WFI to halt the CPU core.
    unsafe {
        cpp_motmot_cpu_off_wfi_loop();
    }
}

/// Powers on the specified hardware CPU core via PSCI CPU on.
extern "C" fn motmot_cpu_on(hw_cpu_id: u64, entry: u64, context: u64) -> Result<(), Status> {
    // SAFETY: Invokes PSCI CPU on call.
    unsafe { psci_cpu_on(hw_cpu_id, entry, context) }
}

static MOTMOT_POWER_OPS: PdevPowerOps = PdevPowerOps {
    reboot: Some(motmot_reboot),
    shutdown: Some(motmot_shutdown),
    cpu_off: Some(motmot_cpu_off),
    cpu_on: Some(motmot_cpu_on),
    get_cpu_state: None,
    opp_set: None,
    opp_get: None,
    opp_get_domain_count: None,
};

/// Early initialization hook for Motmot power management.
#[unsafe(no_mangle)]
pub extern "C" fn motmot_power_init_early() {
    dprintf!(INFO, "POWER: registering motmot power hooks\n");
    pdev_register_power(&MOTMOT_POWER_OPS);
}

/// In-kernel unit tests for the Motmot power driver.
#[cfg(ktest)]
#[unittest::suite(name = "motmot_power")]
mod tests {
    /// Tests that all expected operations in the MOTMOT_POWER_OPS table are populated.
    #[test]
    fn test_motmot_power_ops_structure() {
        assert!(super::MOTMOT_POWER_OPS.reboot.is_some());
        assert!(super::MOTMOT_POWER_OPS.shutdown.is_some());
        assert!(super::MOTMOT_POWER_OPS.cpu_off.is_some());
        assert!(super::MOTMOT_POWER_OPS.cpu_on.is_some());
        assert!(super::MOTMOT_POWER_OPS.get_cpu_state.is_none());
        assert!(super::MOTMOT_POWER_OPS.opp_set.is_none());
        assert!(super::MOTMOT_POWER_OPS.opp_get.is_none());
        assert!(super::MOTMOT_POWER_OPS.opp_get_domain_count.is_none());
    }
}
