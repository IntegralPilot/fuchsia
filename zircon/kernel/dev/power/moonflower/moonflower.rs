// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/power/moonflower/moonflower-power.cc
//
// Moonflower SoC CPU Power and Performance Driver.
//
// Implements platform power lifecycle operations (reboot, shutdown, CPU on/off)
// and hardware Operating Performance Point (OPP) frequency scaling for the Moonflower SoC.

use crate::pdev_power::{
    CONTROL_INTERFACE_ARM_WFI, CONTROL_INTERFACE_CPU_DRIVER,
    K_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT, PdevPowerOps, PowerCpuState, PowerDomainConfigFfi,
    PowerRebootFlags, ProcessorPowerLevelFfi, pdev_register_power,
    power_management_boot_boost_enabled, power_management_register_domains,
    power_management_rppm_enabled, power_management_set_rate_limits,
};
use core::sync::atomic::{AtomicPtr, Ordering};
use debug::dprintf;
use regio::{MmioBank, MmioPtr, Offset, RwSafe};
#[cfg(ktest)]
use unittest as _;
use zx_status::Status;

/// Download/debug mode that is entered after a warm reset.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
enum MoonflowerDownloadMode {
    NoDump = 0x00,
    #[allow(dead_code)]
    Edl = 0x01,
    #[allow(dead_code)]
    FullDump = 0x10,
    #[allow(dead_code)]
    MiniDump = 0x20,
}

const MOONFLOWER_DOWNLOAD_MODE_ADDR: usize = 0x003d_3000;
const VENDOR_SPECIFIC_WARM_RESET_TYPE: u32 = 0x8000_0000;

const POWER_DOMAIN_COUNT: usize = 1;
const DOMAIN_ID: u32 = 0;
const MAX_OPP_INDEX: u64 = 3;

// Register offset within the MMIO bank
const OPP_INDEX_OFFSET: Offset<u32, RwSafe> = Offset::new(0x920);
const OPP_BANK_SIZE: usize = 0x1000;

static OPP_REG_BASE: AtomicPtr<u32> = AtomicPtr::new(core::ptr::null_mut());

unsafe extern "C" {
    fn cpp_moonflower_get_opp_vaddr() -> usize;
    fn cpp_moonflower_tz_config_hw_for_ram_dump(
        disable_wd_dbg: u64,
        boot_partition_sel: u64,
    ) -> i64;
    fn cpp_moonflower_tz_io_write(paddr: usize, val: u32) -> i64;
    fn psci_system_reset2_raw(reset_type: u32, cookie: u32) -> Result<(), Status>;
    fn psci_system_off() -> Result<(), Status>;
    fn psci_cpu_off() -> Result<(), Status>;
    fn psci_cpu_on(hw_cpu_id: u64, entry: u64, context: u64) -> Result<(), Status>;
    fn psci_get_cpu_state(hw_cpu_id: u64, out_state: *mut PowerCpuState) -> Result<(), Status>;
}

/// Configures hardware for reboot/shutdown via Qualcomm TZ SMC call.
fn configure_hw_for_shutdown() {
    // SAFETY: Invokes Qualcomm TZ SMC for RAM dump hardware configuration.
    let r = unsafe { cpp_moonflower_tz_config_hw_for_ram_dump(1, 0) };
    if r != 0 {
        dprintf!(INFO, "POWER: Failed to configure moonflower for shutdown/reboot: {}\n", r);
    }
}

/// Sets the RAM dump download mode via Qualcomm TZ SMC call.
fn set_download_mode(mode: MoonflowerDownloadMode) {
    let val = mode as u32;
    // SAFETY: Invokes Qualcomm TZ SMC to write download mode.
    let r = unsafe { cpp_moonflower_tz_io_write(MOONFLOWER_DOWNLOAD_MODE_ADDR, val) };
    if r != 0 {
        dprintf!(INFO, "POWER: Failed to set moonflower download mode to {:#x}: {}\n", val, r);
    }
}

/// Reboots the Moonflower platform via PSCI warm reset call.
extern "C" fn moonflower_reboot(flags: PowerRebootFlags) -> Result<(), Status> {
    dprintf!(INFO, "Moonflower reboot: flags {:#x}\n", flags as u32);
    set_download_mode(MoonflowerDownloadMode::NoDump);
    configure_hw_for_shutdown();
    // SAFETY: PSCI warm reset call for Moonflower SoC.
    unsafe { psci_system_reset2_raw(VENDOR_SPECIFIC_WARM_RESET_TYPE, 0) }
}

/// Shuts down the Moonflower platform via PSCI system off call.
extern "C" fn moonflower_shutdown() -> Result<(), Status> {
    configure_hw_for_shutdown();
    // SAFETY: PSCI system off call to hardware firmware.
    unsafe { psci_system_off() }
}

/// Powers off the calling CPU core via PSCI CPU off call.
extern "C" fn moonflower_cpu_off() -> Result<(), Status> {
    // SAFETY: PSCI CPU off call.
    unsafe { psci_cpu_off() }
}

/// Powers on the CPU core with the specified hardware ID via PSCI CPU on call.
extern "C" fn moonflower_cpu_on(hw_cpu_id: u64, entry: u64, context: u64) -> Result<(), Status> {
    // SAFETY: PSCI CPU on call.
    unsafe { psci_cpu_on(hw_cpu_id, entry, context) }
}

/// Retrieves the current power state of the specified CPU core via PSCI.
extern "C" fn moonflower_get_cpu_state(
    hw_cpu_id: u64,
    out_state: *mut PowerCpuState,
) -> Result<(), Status> {
    if out_state.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: PSCI get cpu state with valid pointer.
    unsafe { psci_get_cpu_state(hw_cpu_id, out_state) }
}

/// Helper function to construct a `regio::MmioBank` for the OPP index register.
fn get_opp_bank() -> Option<MmioBank<u32, RwSafe>> {
    let base = OPP_REG_BASE.load(Ordering::Acquire);
    if base.is_null() {
        return None;
    }
    // SAFETY: `OPP_REG_BASE` is mapped into kernel address space during early boot
    // and remains mapped for the kernel's lifetime.
    let ptr = unsafe { MmioPtr::<u32, RwSafe>::new(base) };
    Some(MmioBank::new(ptr, OPP_BANK_SIZE))
}

/// Sets the active Operating Performance Point (OPP) for the specified domain.
extern "C" fn moonflower_opp_set(domain_id: u32, opp: u64) -> Result<(), Status> {
    let Some(bank) = get_opp_bank() else {
        return Err(Status::BAD_STATE);
    };
    if domain_id != DOMAIN_ID || opp > MAX_OPP_INDEX {
        return Err(Status::INVALID_ARGS);
    }

    // SAFETY: `OPP_INDEX_OFFSET` (0x920) is within `OPP_BANK_SIZE` (0x1000) and aligned to 4 bytes.
    let reg = unsafe { bank.at(OPP_INDEX_OFFSET) };
    reg.write((MAX_OPP_INDEX - opp) as u32);
    Ok(())
}

/// Retrieves the active Operating Performance Point (OPP) for the specified domain.
extern "C" fn moonflower_opp_get(domain_id: u32, out_opp: *mut u64) -> Result<(), Status> {
    if out_opp.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let Some(bank) = get_opp_bank() else {
        return Err(Status::BAD_STATE);
    };
    if domain_id != DOMAIN_ID {
        return Err(Status::INVALID_ARGS);
    }

    // SAFETY: `OPP_INDEX_OFFSET` (0x920) is within `OPP_BANK_SIZE` (0x1000) and aligned to 4 bytes.
    let reg = unsafe { bank.at(OPP_INDEX_OFFSET) };
    let raw_val = reg.read();
    // SAFETY: `out_opp` was checked non-null.
    unsafe {
        *out_opp = MAX_OPP_INDEX.saturating_sub(raw_val as u64);
    }
    Ok(())
}

/// Retrieves the number of supported OPP control domains.
extern "C" fn moonflower_opp_get_domain_count(out_count: *mut usize) -> Result<(), Status> {
    if out_count.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: `out_count` was checked non-null.
    unsafe {
        *out_count = POWER_DOMAIN_COUNT;
    }
    Ok(())
}

static MOONFLOWER_POWER_OPS: PdevPowerOps = PdevPowerOps {
    reboot: Some(moonflower_reboot),
    shutdown: Some(moonflower_shutdown),
    cpu_off: Some(moonflower_cpu_off),
    cpu_on: Some(moonflower_cpu_on),
    get_cpu_state: Some(moonflower_get_cpu_state),
    opp_set: Some(moonflower_opp_set),
    opp_get: Some(moonflower_opp_get),
    opp_get_domain_count: Some(moonflower_opp_get_domain_count),
};

/// Early initialization hook for Moonflower power management and OPP register bank.
#[unsafe(no_mangle)]
pub extern "C" fn moonflower_power_init_early() {
    dprintf!(INFO, "POWER: registering moonflower power hooks\n");
    // SAFETY: Retrieves the mapped virtual address for the OPP peripheral block.
    let vaddr = unsafe { cpp_moonflower_get_opp_vaddr() };
    OPP_REG_BASE.store(core::ptr::with_exposed_provenance_mut::<u32>(vaddr), Ordering::Release);

    let mut current_opp = 0u64;
    let opp_res = moonflower_opp_get(DOMAIN_ID, &mut current_opp);
    if opp_res.is_ok() {
        dprintf!(INFO, "POWER: current opp {}\n", current_opp);
    } else {
        dprintf!(INFO, "POWER: current opp -1\n");
    }

    pdev_register_power(&MOONFLOWER_POWER_OPS);
}

/// Initializes Moonflower power domain and energy model for the kernel scheduler.
#[unsafe(no_mangle)]
pub extern "C" fn moonflower_power_init() {
    if !power_management_rppm_enabled() {
        dprintf!(INFO, "POWER: Moonflower energy model registration disabled by boot option\n");
        return;
    }

    dprintf!(INFO, "POWER: initializing moonflower power domain\n");

    let wfi_name = c"WFI".as_ptr();
    let lowsvs_name = c"LowSVS".as_ptr();
    let svs_name = c"SVS".as_ptr();
    let nominal_name = c"Nominal".as_ptr();
    let turbo_name = c"Turbo".as_ptr();

    let levels = [
        ProcessorPowerLevelFfi {
            options: K_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT,
            processing_rate: 0,
            power_coefficient_nw: 100_000,
            control_interface: CONTROL_INTERFACE_ARM_WFI,
            control_argument: 0,
            diagnostic_name: wfi_name,
        },
        ProcessorPowerLevelFfi {
            options: 0,
            processing_rate: 360,
            power_coefficient_nw: 20_000_000,
            control_interface: CONTROL_INTERFACE_CPU_DRIVER,
            control_argument: 3,
            diagnostic_name: lowsvs_name,
        },
        ProcessorPowerLevelFfi {
            options: 0,
            processing_rate: 506,
            power_coefficient_nw: 31_000_000,
            control_interface: CONTROL_INTERFACE_CPU_DRIVER,
            control_argument: 2,
            diagnostic_name: svs_name,
        },
        ProcessorPowerLevelFfi {
            options: 0,
            processing_rate: 798,
            power_coefficient_nw: 66_000_000,
            control_interface: CONTROL_INTERFACE_CPU_DRIVER,
            control_argument: 1,
            diagnostic_name: nominal_name,
        },
        ProcessorPowerLevelFfi {
            options: 0,
            processing_rate: 1000,
            power_coefficient_nw: 102_000_000,
            control_interface: CONTROL_INTERFACE_CPU_DRIVER,
            control_argument: 0,
            diagnostic_name: turbo_name,
        },
    ];

    let domain_config = [PowerDomainConfigFfi {
        domain_id: 0,
        cpu_mask: 0xf,
        levels: levels.as_ptr(),
        level_count: levels.len(),
    }];

    if let Err(status) = power_management_register_domains(&domain_config) {
        dprintf!(
            CRITICAL,
            "POWER: Failed to register moonflower power domain: {}\n",
            status.into_raw()
        );
        return;
    }

    dprintf!(INFO, "POWER: Registered moonflower power domain\n");

    // When boot boosting is enabled, set default boot performance limits matching boot OPP (Turbo,
    // 1000 rate) to ensure responsive boot performance and prevent the system from dropping to
    // lower OPPs until boot completes and userspace enables the full range of OPPs.
    if power_management_boot_boost_enabled()
        && let Err(status) = power_management_set_rate_limits(domain_config[0].cpu_mask, 1000, 1000)
    {
        dprintf!(
            CRITICAL,
            "POWER: Failed to set moonflower boot performance limits: {}\n",
            status.into_raw()
        );
    }
}

/// In-kernel unit tests for the Moonflower power driver.
#[cfg(ktest)]
#[unittest::suite(name = "moonflower_power")]
mod tests {
    use super::{DOMAIN_ID, OPP_REG_BASE, Ordering};
    use unittest::{assert_eq, assert_err, assert_ok};
    use zx_status::Status;

    /// Tests that passing a null output pointer to get_cpu_state returns INVALID_ARGS.
    #[test]
    fn test_moonflower_get_cpu_state_null_arg() {
        assert_err!(
            super::moonflower_get_cpu_state(0, core::ptr::null_mut()),
            Status::INVALID_ARGS
        );
    }

    /// Tests opp_get_domain_count.
    #[test]
    fn test_moonflower_opp_get_domain_count() {
        assert_err!(
            super::moonflower_opp_get_domain_count(core::ptr::null_mut()),
            Status::INVALID_ARGS
        );

        let mut count = 0usize;
        assert_ok!(super::moonflower_opp_get_domain_count(&mut count));
        assert_eq!(count, 1);
    }

    /// Tests opp_get and opp_set with mock backing register bank.
    #[test]
    fn test_moonflower_opp_get_set() {
        let mut mock_reg_bank = [0u32; 1024];
        let old_base = OPP_REG_BASE.swap(mock_reg_bank.as_mut_ptr(), Ordering::SeqCst);

        // Test OPP 0 -> register value should be MAX_OPP_INDEX (3) - 0 = 3
        assert_ok!(super::moonflower_opp_set(DOMAIN_ID, 0));
        // Index for offset 0x920 is 0x920 / 4 = 584
        assert_eq!(mock_reg_bank[0x920 / 4], 3);

        let mut opp = 0u64;
        assert_ok!(super::moonflower_opp_get(DOMAIN_ID, &mut opp));
        assert_eq!(opp, 0);

        // Test OPP 2 -> register value should be 3 - 2 = 1
        assert_ok!(super::moonflower_opp_set(DOMAIN_ID, 2));
        assert_eq!(mock_reg_bank[0x920 / 4], 1);

        assert_ok!(super::moonflower_opp_get(DOMAIN_ID, &mut opp));
        assert_eq!(opp, 2);

        // Test Out of bounds domain
        assert_err!(super::moonflower_opp_set(1, 0), Status::INVALID_ARGS);
        assert_err!(super::moonflower_opp_get(1, &mut opp), Status::INVALID_ARGS);

        // Test Out of bounds OPP index
        assert_err!(super::moonflower_opp_set(DOMAIN_ID, 4), Status::INVALID_ARGS);

        // Restore original base pointer
        OPP_REG_BASE.store(old_base, Ordering::SeqCst);
    }
}
