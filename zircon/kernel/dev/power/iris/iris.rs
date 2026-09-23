// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/power/iris/power.cc
//
// Iris SoC CPU Power and Performance Driver.
//
// Implements platform power lifecycle operations (reboot, shutdown, CPU on/off)
// and hardware Operating Performance Point (OPP) frequency scaling across Iris CPU domains.

use crate::pdev_power::{
    CONTROL_INTERFACE_ARM_WFI, CONTROL_INTERFACE_CPU_DRIVER,
    K_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT, PdevPowerOps, PowerCpuState, PowerDomainConfigFfi,
    PowerRebootFlags, ProcessorPowerLevelFfi, pdev_register_power,
    power_management_boot_boost_enabled, power_management_register_domains,
    power_management_rppm_enabled, power_management_set_rate_limits,
};
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use debug::dprintf;
use kalloc::Box;
use regio::{MmioBank, MmioPtr, Offset, RwSafe};
#[cfg(ktest)]
use unittest as _;
use zx_status::Status;

// Vendor-specific (bit 31) SYSTEM_RESET2 reset type to request a warm reset on Iris.
const VENDOR_SPECIFIC_WARM_RESET_TYPE: u32 = 0x8000_0000;

const POWER_DOMAIN_COUNT: usize = 4;

const DOMAIN0_REG_OFFSET: Offset<u32, RwSafe> = Offset::new(0x00);
const DOMAIN1_REG_OFFSET: Offset<u32, RwSafe> = Offset::new(0x08);
const DOMAIN2_REG_OFFSET: Offset<u32, RwSafe> = Offset::new(0x10);
const DOMAIN3_REG_OFFSET: Offset<u32, RwSafe> = Offset::new(0x18);
const OPP_BANK_SIZE: usize = 0x20;

static OPP_REG_BASE: AtomicPtr<u32> = AtomicPtr::new(core::ptr::null_mut());

const UNCACHED_OPP: u64 = u64::MAX;

static CURRENT_OPPS: [AtomicU64; POWER_DOMAIN_COUNT] = [
    AtomicU64::new(UNCACHED_OPP),
    AtomicU64::new(UNCACHED_OPP),
    AtomicU64::new(UNCACHED_OPP),
    AtomicU64::new(UNCACHED_OPP),
];

#[cfg(ktest)]
fn reset_cached_opps_for_test() {
    for opp in &CURRENT_OPPS {
        opp.store(UNCACHED_OPP, Ordering::SeqCst);
    }
}

ksync::declare_singleton_lock!(Domain0Lock, ::ksync::RawSpinlock);
ksync::declare_singleton_lock!(Domain1Lock, ::ksync::RawSpinlock);
ksync::declare_singleton_lock!(Domain2Lock, ::ksync::RawSpinlock);
ksync::declare_singleton_lock!(Domain3Lock, ::ksync::RawSpinlock);

/// Acquires the spinlock corresponding to `domain_index` and executes `f`.
fn with_domain_lock<R>(
    domain_index: usize,
    f: impl FnOnce() -> Result<R, Status>,
) -> Result<R, Status> {
    match domain_index {
        0 => {
            ksync::lock!(Domain0Lock::lock());
            f()
        }
        1 => {
            ksync::lock!(Domain1Lock::lock());
            f()
        }
        2 => {
            ksync::lock!(Domain2Lock::lock());
            f()
        }
        3 => {
            ksync::lock!(Domain3Lock::lock());
            f()
        }
        _ => Err(Status::INVALID_ARGS),
    }
}

#[derive(Copy, Clone, Debug)]
struct DomainInfo {
    opp_count: u32,
    mmio_offset: u32,
    boot_opp: u64,
    floor_opp: u64,
    reg_offset: Offset<u32, RwSafe>,
}

const DOMAIN_INFOS: [DomainInfo; 4] = [
    // Domain 0 (Little)
    DomainInfo {
        opp_count: 22,
        mmio_offset: 2,
        boot_opp: 8,
        floor_opp: 15,
        reg_offset: DOMAIN0_REG_OFFSET,
    },
    // Domain 1 (Medium 1):
    DomainInfo {
        opp_count: 24,
        mmio_offset: 0,
        boot_opp: 11,
        floor_opp: 20,
        reg_offset: DOMAIN1_REG_OFFSET,
    },
    // Domain 2 (Medium 2):
    DomainInfo {
        opp_count: 24,
        mmio_offset: 0,
        boot_opp: 11,
        floor_opp: 20,
        reg_offset: DOMAIN2_REG_OFFSET,
    },
    // Domain 3 (Big):
    DomainInfo {
        opp_count: 23,
        mmio_offset: 1,
        boot_opp: 10,
        floor_opp: 21,
        reg_offset: DOMAIN3_REG_OFFSET,
    },
];

unsafe extern "C" {
    fn cpp_iris_get_opp_vaddr() -> usize;
    fn psci_system_reset_cold() -> Result<(), Status>;
    fn psci_system_reset2_raw(reset_type: u32, cookie: u32) -> Result<(), Status>;
    fn psci_system_off() -> Result<(), Status>;
    fn psci_cpu_off() -> Result<(), Status>;
    fn psci_cpu_on(hw_cpu_id: u64, entry: u64, context: u64) -> Result<(), Status>;
    fn psci_get_cpu_state(hw_cpu_id: u64, out_state: *mut PowerCpuState) -> Result<(), Status>;
}

/// Reboots the system via PSCI cold reset or vendor-specific warm reset for panic.
extern "C" fn iris_reboot(flags: PowerRebootFlags) -> Result<(), Status> {
    match flags {
        PowerRebootFlags::Normal | PowerRebootFlags::Bootloader | PowerRebootFlags::Recovery => {
            dprintf!(INFO, "Iris reboot: performing cold reset\n");
            // SAFETY: PSCI cold reset call to hardware firmware.
            unsafe { psci_system_reset_cold() }
        }
        PowerRebootFlags::Panic => {
            dprintf!(INFO, "Iris panic reboot: performing warm reset\n");
            // SAFETY: On Iris VENDOR_SPECIFIC_WARM_RESET_TYPE ignores the cookie parameter.
            unsafe { psci_system_reset2_raw(VENDOR_SPECIFIC_WARM_RESET_TYPE, 0) }
        }
    }
}

/// Shuts down the system via PSCI system off call.
extern "C" fn iris_shutdown() -> Result<(), Status> {
    // SAFETY: PSCI system off call to hardware firmware.
    unsafe { psci_system_off() }
}

/// Powers off the calling CPU core via PSCI cpu off call.
extern "C" fn iris_cpu_off() -> Result<(), Status> {
    // SAFETY: PSCI CPU off call.
    unsafe { psci_cpu_off() }
}

/// Powers on the CPU core with the specified hardware ID via PSCI.
extern "C" fn iris_cpu_on(hw_cpu_id: u64, entry: u64, context: u64) -> Result<(), Status> {
    // SAFETY: PSCI CPU on call.
    unsafe { psci_cpu_on(hw_cpu_id, entry, context) }
}

/// Retrieves the current power state of the CPU core with the specified hardware ID.
extern "C" fn iris_get_cpu_state(
    hw_cpu_id: u64,
    out_state: *mut PowerCpuState,
) -> Result<(), Status> {
    if out_state.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: `out_state` was checked non-null above.
    unsafe { psci_get_cpu_state(hw_cpu_id, out_state) }
}

/// Helper function to construct a `regio::MmioBank` for the OPP register block.
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

/// Sets the active Operating Performance Point (OPP) for the specified power domain.
extern "C" fn iris_opp_set(domain_id: u32, opp: u64) -> Result<(), Status> {
    let Ok(domain_index) = usize::try_from(domain_id) else {
        return Err(Status::INVALID_ARGS);
    };
    let Some(info) = DOMAIN_INFOS.get(domain_index) else {
        return Err(Status::INVALID_ARGS);
    };
    if opp >= info.opp_count as u64 {
        return Err(Status::INVALID_ARGS);
    }

    // Clamp the requested OPP to the hardware floor OPP as driver-level defense in depth.
    let opp = opp.min(info.floor_opp);

    // Fast path: if the requested OPP is already cached as active for this domain, return
    // immediately without acquiring the domain spinlock or writing to MMIO.
    if CURRENT_OPPS[domain_index].load(Ordering::Acquire) == opp {
        return Ok(());
    }

    with_domain_lock(domain_index, || {
        if CURRENT_OPPS[domain_index].load(Ordering::Relaxed) == opp {
            return Ok(());
        }

        let Some(bank) = get_opp_bank() else {
            return Err(Status::BAD_STATE);
        };

        let mmio_opp = (opp as u32) + info.mmio_offset;
        // SAFETY: `info.reg_offset` is within `OPP_BANK_SIZE` (0x20) and aligned to 4 bytes.
        let reg = unsafe { bank.at(info.reg_offset) };
        reg.write(mmio_opp);
        CURRENT_OPPS[domain_index].store(opp, Ordering::Release);
        Ok(())
    })
}

/// Retrieves the active Operating Performance Point (OPP) for the specified power domain.
extern "C" fn iris_opp_get(domain_id: u32, out_opp: *mut u64) -> Result<(), Status> {
    if out_opp.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let Ok(domain_index) = usize::try_from(domain_id) else {
        return Err(Status::INVALID_ARGS);
    };
    let Some(info) = DOMAIN_INFOS.get(domain_index) else {
        return Err(Status::INVALID_ARGS);
    };

    let cached = CURRENT_OPPS[domain_index].load(Ordering::Acquire);
    if cached != UNCACHED_OPP {
        // SAFETY: `out_opp` was checked non-null above.
        unsafe {
            *out_opp = cached;
        }
        return Ok(());
    }

    with_domain_lock(domain_index, || {
        let Some(bank) = get_opp_bank() else {
            return Err(Status::BAD_STATE);
        };

        // SAFETY: `info.reg_offset` is within `OPP_BANK_SIZE` (0x20) and aligned to 4 bytes.
        let reg = unsafe { bank.at(info.reg_offset) };
        let raw_val = reg.read();
        let opp = raw_val.saturating_sub(info.mmio_offset) as u64;
        CURRENT_OPPS[domain_index].store(opp, Ordering::Release);
        // SAFETY: `out_opp` was checked non-null above.
        unsafe {
            *out_opp = opp;
        }
        Ok(())
    })
}

/// Retrieves the number of supported OPP control domains.
extern "C" fn iris_opp_get_domain_count(out_count: *mut usize) -> Result<(), Status> {
    if out_count.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: `out_count` was checked non-null above.
    unsafe {
        *out_count = POWER_DOMAIN_COUNT;
    }
    Ok(())
}

static IRIS_POWER_OPS: PdevPowerOps = PdevPowerOps {
    reboot: Some(iris_reboot),
    shutdown: Some(iris_shutdown),
    cpu_off: Some(iris_cpu_off),
    cpu_on: Some(iris_cpu_on),
    get_cpu_state: Some(iris_get_cpu_state),
    opp_set: Some(iris_opp_set),
    opp_get: Some(iris_opp_get),
    opp_get_domain_count: Some(iris_opp_get_domain_count),
};

/// Early initialization hook for Iris power management and OPP register bank.
#[unsafe(no_mangle)]
pub extern "C" fn iris_power_init_early() {
    dprintf!(INFO, "POWER: registering iris power hooks\n");
    // SAFETY: Retrieves the mapped virtual address for the OPP peripheral block.
    let vaddr = unsafe { cpp_iris_get_opp_vaddr() };
    OPP_REG_BASE.store(core::ptr::with_exposed_provenance_mut::<u32>(vaddr), Ordering::Release);

    for (domain_id, info) in DOMAIN_INFOS.iter().enumerate() {
        let _ = iris_opp_set(domain_id as u32, info.boot_opp);
    }

    pdev_register_power(&IRIS_POWER_OPS);
}

fn allocate_and_populate_from_domain_opps(
    domain_index: usize,
    domain: &zbi::CpuEnergyModelDomain,
    wfi_name: *const core::ffi::c_char,
    opp_name: *const core::ffi::c_char,
) -> Option<Box<[ProcessorPowerLevelFfi]>> {
    let floor_opp = DOMAIN_INFOS.get(domain_index).map(|info| info.floor_opp).unwrap_or(u64::MAX);

    let num_opps = (domain.opp_count as usize)
        .min(domain.opps.len())
        .min(zbi::KERNEL_DRIVER_CPU_ENERGY_MODEL_MAX_OPPS as usize);

    let mut sorted_opps =
        [zbi::CpuEnergyModelOpp { frequency_khz: 0, capacity: 0, power_uw: 0, voltage_mv: 0 };
            zbi::KERNEL_DRIVER_CPU_ENERGY_MODEL_MAX_OPPS as usize];
    let sorted_opps_slice = if num_opps > 0 {
        // Sort OPPs in ascending order of frequency (or capacity if frequencies are equal)
        // so that power levels are strictly increasing in processing rate, regardless of
        // whether the ZBI payload provided OPPs in ascending, descending, or unsorted order.
        sorted_opps[..num_opps].copy_from_slice(&domain.opps[..num_opps]);
        sorted_opps[..num_opps].sort_unstable_by_key(|opp| (opp.frequency_khz, opp.capacity));
        &sorted_opps[..num_opps]
    } else {
        &[]
    };

    // First pass: count how many active OPP levels satisfy the floor OPP threshold.
    // Iris hardware OPP index 0 corresponds to the fastest OPP, and index num_opps - 1
    // corresponds to the slowest OPP. Since sorted_opps is ascending (idx 0 = slowest),
    // the hardware control argument is mapped as (num_opps - 1 - idx).
    let mut valid_opp_count = 0usize;
    for idx in 0..sorted_opps_slice.len() {
        let control_arg = (num_opps - 1 - idx) as u64;
        if control_arg <= floor_opp {
            valid_opp_count += 1;
        }
    }

    // Power levels consist of 1 WFI idle level + filtered active OPP levels.
    // Allocate directly on the kernel heap rather than maintaining a transient stack array.
    let total_level_count = 1 + valid_opp_count;
    let mut uninit =
        Box::<[ProcessorPowerLevelFfi]>::try_new_uninit_slice(total_level_count).ok()?;

    // Populate WFI idle level at index 0.
    uninit[0].write(ProcessorPowerLevelFfi {
        options: K_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT,
        processing_rate: 0,
        power_coefficient_nw: 100_000,
        control_interface: CONTROL_INTERFACE_ARM_WFI,
        control_argument: 0,
        diagnostic_name: wfi_name,
    });

    if !sorted_opps_slice.is_empty() {
        let max_freq = sorted_opps_slice.last().map(|opp| opp.frequency_khz as u64).unwrap_or(0);
        let mut write_idx = 1usize;
        for (idx, opp) in sorted_opps_slice.iter().enumerate() {
            let control_arg = (num_opps - 1 - idx) as u64;

            // Filter out power levels below the hardware floor OPP (i.e. control_arg > floor_opp).
            // Operating below this floor causes UFS storage controller and interconnect stalls.
            if control_arg > floor_opp {
                continue;
            }

            let rate = if opp.capacity > 0 {
                opp.capacity as u64
            } else if max_freq > 0 {
                (opp.frequency_khz as u64 * domain.max_rate).div_ceil(max_freq)
            } else {
                1
            };
            let power_nw = if opp.power_uw > 0 {
                opp.power_uw as u64 * 1_000
            } else {
                (rate * 200_000) + 10_000_000
            };

            uninit[write_idx].write(ProcessorPowerLevelFfi {
                options: 0,
                processing_rate: rate,
                power_coefficient_nw: power_nw,
                control_interface: CONTROL_INTERFACE_CPU_DRIVER,
                control_argument: control_arg,
                diagnostic_name: opp_name,
            });
            write_idx += 1;
        }
        debug_assert_eq!(write_idx, total_level_count);
    }

    // SAFETY: All elements from 0 to total_level_count-1 in `uninit` were explicitly initialized above.
    Some(unsafe { uninit.assume_init() })
}

/// Initializes Iris power domains and energy models for the kernel scheduler.
///
/// Both the `kernel.power.rppm` boot option and a valid, populated `domains` array
/// in the ZBI must be present to enable OPP control on Iris.
///
/// # Safety
///
/// If `domains` is non-null and `domain_count` > 0, caller must ensure `domains` points
/// to a valid array of `domain_count` initialized `zbi::CpuEnergyModelDomain` structs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn iris_power_init(
    domains: *const zbi::CpuEnergyModelDomain,
    domain_count: usize,
) {
    if !power_management_rppm_enabled() {
        dprintf!(INFO, "POWER: Iris energy model registration disabled by boot option\n");
        return;
    }

    if domains.is_null() || domain_count == 0 {
        dprintf!(INFO, "POWER: Iris energy model not supplied in ZBI, skipping registration\n");
        return;
    }

    // SAFETY: Pointer and count are guaranteed valid by caller if non-null and non-zero.
    let domain_slice = unsafe { core::slice::from_raw_parts(domains, domain_count) };
    if domain_count < 4 || domain_slice[0].opp_count == 0 {
        dprintf!(
            INFO,
            "POWER: Iris energy model in ZBI is empty or incomplete, skipping registration\n"
        );
        return;
    }

    dprintf!(INFO, "POWER: initializing iris power domains from ZBI energy model payload\n");

    let wfi_name = c"WFI".as_ptr();
    let opp_name = c"OPP".as_ptr();

    // Allocate power level descriptors on the kernel heap using `kalloc::Box` rather than on the
    // kernel stack. The combined 4 domains have ~97 total levels (~4.6 KB), which would consume a
    // significant portion of the limited kernel stack (8-16 KB).
    let Some(levels_d0) =
        allocate_and_populate_from_domain_opps(0, &domain_slice[0], wfi_name, opp_name)
    else {
        dprintf!(CRITICAL, "POWER: Failed to allocate memory for iris domain 0 power levels\n");
        return;
    };
    let Some(levels_d1) =
        allocate_and_populate_from_domain_opps(1, &domain_slice[1], wfi_name, opp_name)
    else {
        dprintf!(CRITICAL, "POWER: Failed to allocate memory for iris domain 1 power levels\n");
        return;
    };
    let Some(levels_d2) =
        allocate_and_populate_from_domain_opps(2, &domain_slice[2], wfi_name, opp_name)
    else {
        dprintf!(CRITICAL, "POWER: Failed to allocate memory for iris domain 2 power levels\n");
        return;
    };
    let Some(levels_d3) =
        allocate_and_populate_from_domain_opps(3, &domain_slice[3], wfi_name, opp_name)
    else {
        dprintf!(CRITICAL, "POWER: Failed to allocate memory for iris domain 3 power levels\n");
        return;
    };

    let domain_configs = [
        // Domain 0: Little (CPUs 0-1)
        PowerDomainConfigFfi {
            domain_id: 0,
            cpu_mask: 0x03,
            levels: levels_d0.as_ptr(),
            level_count: levels_d0.len(),
        },
        // Domain 1: Medium 1 (CPUs 2-4)
        PowerDomainConfigFfi {
            domain_id: 1,
            cpu_mask: 0x1c,
            levels: levels_d1.as_ptr(),
            level_count: levels_d1.len(),
        },
        // Domain 2: Medium 2 (CPUs 5-6)
        PowerDomainConfigFfi {
            domain_id: 2,
            cpu_mask: 0x60,
            levels: levels_d2.as_ptr(),
            level_count: levels_d2.len(),
        },
        // Domain 3: Big (CPU 7)
        PowerDomainConfigFfi {
            domain_id: 3,
            cpu_mask: 0x80,
            levels: levels_d3.as_ptr(),
            level_count: levels_d3.len(),
        },
    ];

    if let Err(status) = power_management_register_domains(&domain_configs) {
        dprintf!(CRITICAL, "POWER: Failed to register iris power domains: {}\n", status.into_raw());
        return;
    }

    dprintf!(INFO, "POWER: Registered iris power domains\n");

    // When boot boosting is enabled, lock processing rates to boot OPPs during startup to ensure
    // responsive boot performance, prevent the system from dropping to lower OPPs during I/O gaps,
    // and prevent excessive thermal dissipation before userspace thermal services initialize.
    //
    // Note: power_management_set_rate_limits operates in userspace rate units (scale of 1000)
    // where rate = round(capacity * 1000 / max_system_capacity=1024):
    // - Domain 0 (Little, CPUs 0-1): Boot OPP 8 (1.882 GHz, cap 200) -> rate 196
    // - Domain 1 (Medium 1, CPUs 2-4): Boot OPP 11 (1.785 GHz, cap 556) -> rate 543
    // - Domain 2 (Medium 2, CPUs 5-6): Boot OPP 11 (1.785 GHz, cap 555) -> rate 542
    // - Domain 3 (Big, CPU 7): Boot OPP 10 (2.208 GHz, cap 811) -> rate 792
    if power_management_boot_boost_enabled() {
        if let Err(status) = power_management_set_rate_limits(0x03, 196, 196) {
            dprintf!(
                CRITICAL,
                "POWER: Failed to set iris domain 0 boot performance limits: {}\n",
                status.into_raw()
            );
        }
        if let Err(status) = power_management_set_rate_limits(0x1c, 543, 543) {
            dprintf!(
                CRITICAL,
                "POWER: Failed to set iris domain 1 boot performance limits: {}\n",
                status.into_raw()
            );
        }
        if let Err(status) = power_management_set_rate_limits(0x60, 542, 542) {
            dprintf!(
                CRITICAL,
                "POWER: Failed to set iris domain 2 boot performance limits: {}\n",
                status.into_raw()
            );
        }
        if let Err(status) = power_management_set_rate_limits(0x80, 792, 792) {
            dprintf!(
                CRITICAL,
                "POWER: Failed to set iris domain 3 boot performance limits: {}\n",
                status.into_raw()
            );
        }
    }
}

/// In-kernel unit tests for the Iris power driver.
#[cfg(ktest)]
#[unittest::suite(name = "iris_power")]
mod tests {
    use super::{OPP_REG_BASE, Ordering};
    use unittest::{assert_eq, assert_err, assert_ok};
    use zx_status::Status;

    /// Tests that passing a null output pointer to get_cpu_state returns INVALID_ARGS.
    #[test]
    fn test_iris_get_cpu_state_null_arg() {
        assert_err!(super::iris_get_cpu_state(0, core::ptr::null_mut()), Status::INVALID_ARGS);
    }

    /// Tests that with_domain_lock acquires and releases domain spinlocks properly.
    #[test]
    fn test_iris_domain_spinlock() {
        for domain in 0..4 {
            assert_ok!(super::with_domain_lock(domain, || Ok(())));
        }
        assert_err!(super::with_domain_lock(4, || Ok(())), Status::INVALID_ARGS);
    }

    /// Tests opp_get_domain_count.
    #[test]
    fn test_iris_opp_get_domain_count() {
        assert_err!(super::iris_opp_get_domain_count(core::ptr::null_mut()), Status::INVALID_ARGS);

        let mut count = 0usize;
        assert_ok!(super::iris_opp_get_domain_count(&mut count));
        assert_eq!(count, 4);
    }

    /// Tests opp_get and opp_set with mock backing memory and verifies cached OPP behavior.
    #[test]
    fn test_iris_opp_get_set() {
        super::reset_cached_opps_for_test();
        let mut mock_reg_bank = [0u32; 8];
        let old_base = OPP_REG_BASE.swap(mock_reg_bank.as_mut_ptr(), Ordering::SeqCst);

        // Test Domain 0 (mmio_offset = 2)
        assert_ok!(super::iris_opp_set(0, 5));
        assert_eq!(mock_reg_bank[0], 7); // 5 + 2

        let mut opp = 0u64;
        assert_ok!(super::iris_opp_get(0, &mut opp));
        assert_eq!(opp, 5);

        // Setting the same OPP should hit the cache and skip MMIO write.
        mock_reg_bank[0] = 0xbeef;
        assert_ok!(super::iris_opp_set(0, 5));
        assert_eq!(mock_reg_bank[0], 0xbeef); // Untouched due to cache hit

        // Setting a different OPP writes to MMIO and updates cache.
        assert_ok!(super::iris_opp_set(0, 6));
        assert_eq!(mock_reg_bank[0], 8); // 6 + 2

        // Test Domain 3 (mmio_offset = 1, reg offset 0x18 -> index 6)
        assert_ok!(super::iris_opp_set(3, 10));
        assert_eq!(mock_reg_bank[6], 11); // 10 + 1

        assert_ok!(super::iris_opp_get(3, &mut opp));
        assert_eq!(opp, 10);

        // Test Out of bounds domain
        assert_err!(super::iris_opp_set(4, 0), Status::INVALID_ARGS);
        assert_err!(super::iris_opp_get(4, &mut opp), Status::INVALID_ARGS);

        // Test Out of bounds opp
        assert_err!(super::iris_opp_set(0, 22), Status::INVALID_ARGS);

        // Restore original base pointer and reset cache
        OPP_REG_BASE.store(old_base, Ordering::SeqCst);
        super::reset_cached_opps_for_test();
    }

    /// Tests that passing a null or empty config to iris_power_init returns gracefully without panicking.
    #[test]
    fn test_iris_power_init_null_or_empty_config() {
        // SAFETY: Testing null pointer handling.
        unsafe {
            super::iris_power_init(core::ptr::null(), 0);
        }

        const EMPTY_DOMAINS: [zbi::CpuEnergyModelDomain; 4] = [zbi::CpuEnergyModelDomain {
            cpu_mask: 0,
            max_rate: 0,
            domain_id: 0,
            opp_count: 0,
            opps: [zbi::CpuEnergyModelOpp {
                frequency_khz: 0,
                capacity: 0,
                power_uw: 0,
                voltage_mv: 0,
            }; zbi::KERNEL_DRIVER_CPU_ENERGY_MODEL_MAX_OPPS as usize],
        }; 4];
        // SAFETY: Pointer and length are valid for call duration.
        unsafe {
            super::iris_power_init(EMPTY_DOMAINS.as_ptr(), EMPTY_DOMAINS.len());
        }
    }

    /// Tests that allocate_and_populate_from_domain_opps sorts OPPs ascending.
    #[test]
    fn test_allocate_and_populate_opp_sorting() {
        let wfi_name = c"WFI".as_ptr();
        let opp_name = c"OPP".as_ptr();

        // Ascending OPPs (500MHz, 1000MHz, 2000MHz).
        let mut asc_domain = zbi::CpuEnergyModelDomain {
            cpu_mask: 0x03,
            max_rate: 150,
            domain_id: 0,
            opp_count: 3,
            opps: [zbi::CpuEnergyModelOpp {
                frequency_khz: 0,
                capacity: 0,
                power_uw: 0,
                voltage_mv: 0,
            }; zbi::KERNEL_DRIVER_CPU_ENERGY_MODEL_MAX_OPPS as usize],
        };
        asc_domain.opps[0] = zbi::CpuEnergyModelOpp {
            frequency_khz: 500_000,
            capacity: 50,
            power_uw: 100,
            voltage_mv: 700,
        };
        asc_domain.opps[1] = zbi::CpuEnergyModelOpp {
            frequency_khz: 1_000_000,
            capacity: 100,
            power_uw: 200,
            voltage_mv: 800,
        };
        asc_domain.opps[2] = zbi::CpuEnergyModelOpp {
            frequency_khz: 2_000_000,
            capacity: 150,
            power_uw: 300,
            voltage_mv: 900,
        };

        // Descending OPPs (2000MHz, 1000MHz, 500MHz).
        let mut desc_domain = zbi::CpuEnergyModelDomain {
            cpu_mask: 0x03,
            max_rate: 150,
            domain_id: 0,
            opp_count: 3,
            opps: [zbi::CpuEnergyModelOpp {
                frequency_khz: 0,
                capacity: 0,
                power_uw: 0,
                voltage_mv: 0,
            }; zbi::KERNEL_DRIVER_CPU_ENERGY_MODEL_MAX_OPPS as usize],
        };
        desc_domain.opps[0] = zbi::CpuEnergyModelOpp {
            frequency_khz: 2_000_000,
            capacity: 150,
            power_uw: 300,
            voltage_mv: 900,
        };
        desc_domain.opps[1] = zbi::CpuEnergyModelOpp {
            frequency_khz: 1_000_000,
            capacity: 100,
            power_uw: 200,
            voltage_mv: 800,
        };
        desc_domain.opps[2] = zbi::CpuEnergyModelOpp {
            frequency_khz: 500_000,
            capacity: 50,
            power_uw: 100,
            voltage_mv: 700,
        };

        let asc_levels =
            super::allocate_and_populate_from_domain_opps(0, &asc_domain, wfi_name, opp_name)
                .unwrap();
        let desc_levels =
            super::allocate_and_populate_from_domain_opps(0, &desc_domain, wfi_name, opp_name)
                .unwrap();

        assert_eq!(asc_levels.len(), 4);
        assert_eq!(desc_levels.len(), 4);

        // Levels should be identical regardless of input order.
        for i in 0..4 {
            assert_eq!(asc_levels[i].processing_rate, desc_levels[i].processing_rate);
            assert_eq!(asc_levels[i].power_coefficient_nw, desc_levels[i].power_coefficient_nw);
            assert_eq!(asc_levels[i].control_interface, desc_levels[i].control_interface);
            assert_eq!(asc_levels[i].control_argument, desc_levels[i].control_argument);
        }

        // Verify ordering: Level 0 = WFI (rate 0), Level 1 = 50 (ctrl arg 2), Level 2 = 100 (ctrl
        // arg 1), Level 3 = 150 (ctrl arg 0).
        assert_eq!(asc_levels[0].processing_rate, 0);
        assert_eq!(asc_levels[0].control_interface, CONTROL_INTERFACE_ARM_WFI);

        assert_eq!(asc_levels[1].processing_rate, 50);
        assert_eq!(asc_levels[1].control_argument, 2);

        assert_eq!(asc_levels[2].processing_rate, 100);
        assert_eq!(asc_levels[2].control_argument, 1);

        assert_eq!(asc_levels[3].processing_rate, 150);
        assert_eq!(asc_levels[3].control_argument, 0);
    }

    /// Tests that allocate_and_populate_from_domain_opps filters out power levels below floor_opp.
    #[test]
    fn test_allocate_and_populate_opp_filtering_below_floor() {
        let wfi_name = c"WFI".as_ptr();
        let opp_name = c"OPP".as_ptr();

        // Create a domain with 24 OPPs (frequencies 100MHz to 2400MHz, ctrl args 0..23).
        let mut domain = zbi::CpuEnergyModelDomain {
            cpu_mask: 0x60,
            max_rate: 2400,
            domain_id: 2,
            opp_count: 24,
            opps: [zbi::CpuEnergyModelOpp {
                frequency_khz: 0,
                capacity: 0,
                power_uw: 0,
                voltage_mv: 0,
            }; zbi::KERNEL_DRIVER_CPU_ENERGY_MODEL_MAX_OPPS as usize],
        };
        for (i, opp) in domain.opps.iter_mut().enumerate().take(24) {
            *opp = zbi::CpuEnergyModelOpp {
                frequency_khz: ((i + 1) * 100_000) as u32,
                capacity: ((i + 1) * 100) as u32,
                power_uw: ((i + 1) * 50) as u32,
                voltage_mv: 800,
            };
        }

        // Domain 2 has floor_opp = 20. Control args > 20 (i.e. 21..23) must be filtered out.
        let levels =
            super::allocate_and_populate_from_domain_opps(2, &domain, wfi_name, opp_name).unwrap();

        // 1 WFI level + 21 OPP levels (ctrl args 0..20) = 22 total levels.
        assert_eq!(levels.len(), 22);
        assert_eq!(levels[0].processing_rate, 0);
        assert_eq!(levels[0].control_interface, CONTROL_INTERFACE_ARM_WFI);

        // Slowest active level (Level 1) should correspond to floor_opp (ctrl arg 20).
        assert_eq!(levels[1].control_argument, 20);
        assert_eq!(levels[1].processing_rate, 400);

        // Fastest level (Level 21) should correspond to max OPP (ctrl arg 0).
        assert_eq!(levels[21].control_argument, 0);
        assert_eq!(levels[21].processing_rate, 2400);
    }
}
