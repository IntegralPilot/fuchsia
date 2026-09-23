// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/pdev/power/power.cc
//
// Platform Device (pdev) Power Driver Interface.
//
// Provides platform-level abstraction and dispatch for system power operations,
// including system reboot, shutdown, CPU core power state management, and
// dynamic Operating Performance Point (OPP) control for CPU power domains.

#[cfg(console_enabled)]
pub mod console;
#[cfg(not(console_enabled))]
use debug as _;

use core::sync::atomic::Ordering;
#[cfg(ktest)]
use unittest as _;
use zr::AtomicConstPtr;
use zx_status::Status;

/// Flags passed to `power_reboot` specifying the reason/target for reboot.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PowerRebootFlags {
    Normal = 0,
    Bootloader = 1,
    Recovery = 2,
    Panic = 3,
}

/// Represents the architectural power state of a CPU.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PowerCpuState {
    On = 0,
    Off = 1,
    OnPending = 2,
    Started = 3,
    Stopped = 4,
    StartPending = 5,
    StopPending = 6,
    Suspended = 7,
    SuspendPending = 8,
    ResumePending = 9,
}

/// Platform device power operations table.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct PdevPowerOps {
    /// Reboots the system according to the provided reboot flags.
    pub reboot: Option<extern "C" fn(flags: PowerRebootFlags) -> Result<(), Status>>,
    /// Powers off / shuts down the system.
    pub shutdown: Option<extern "C" fn() -> Result<(), Status>>,
    /// Powers off the current CPU.
    pub cpu_off: Option<extern "C" fn() -> Result<(), Status>>,
    /// Initiates power-on sequence for the specified hardware CPU.
    pub cpu_on:
        Option<extern "C" fn(hw_cpu_id: u64, entry: u64, context: u64) -> Result<(), Status>>,
    /// Retrieves the current power state of the specified CPU.
    pub get_cpu_state:
        Option<extern "C" fn(hw_cpu_id: u64, out_state: *mut PowerCpuState) -> Result<(), Status>>,
    /// Sets the Operating Performance Point (OPP) for the specified power domain.
    pub opp_set: Option<extern "C" fn(domain_id: u32, opp: u64) -> Result<(), Status>>,
    /// Retrieves the active Operating Performance Point (OPP) for the specified power domain.
    pub opp_get: Option<extern "C" fn(domain_id: u32, out_opp: *mut u64) -> Result<(), Status>>,
    /// Returns the number of supported OPP control domains.
    pub opp_get_domain_count: Option<extern "C" fn(out_count: *mut usize) -> Result<(), Status>>,
}

zr::static_assert!(core::mem::size_of::<PowerRebootFlags>() == 4);
zr::static_assert!(core::mem::align_of::<PowerRebootFlags>() == 4);
zr::static_assert!(core::mem::size_of::<PowerCpuState>() == 4);
zr::static_assert!(core::mem::align_of::<PowerCpuState>() == 4);
zr::static_assert!(core::mem::size_of::<PdevPowerOps>() == 64);
zr::static_assert!(core::mem::align_of::<PdevPowerOps>() == 8);

/// Power level options flag: entity power level is independent of other CPUs in the domain.
pub const K_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT: u32 = 1;

/// Control interface identifier for ARM WFI (Wait-For-Interrupt) transitions.
pub const CONTROL_INTERFACE_ARM_WFI: u32 = 0;

/// Control interface identifier for CPU power driver transitions.
pub const CONTROL_INTERFACE_CPU_DRIVER: u32 = 1;

/// FFI representation of a processor power level for energy model registration.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ProcessorPowerLevelFfi {
    /// Power level options (e.g. `kPowerLevelOptionsDomainIndependent`).
    pub options: u32,
    /// Processing rate associated with this power level.
    pub processing_rate: u64,
    /// Power consumption coefficient in nanowatts.
    pub power_coefficient_nw: u64,
    /// Power control interface (0 for `kArmWfi`, 1 for `kCpuDriver`).
    pub control_interface: u32,
    /// Control argument passed to the power control interface.
    pub control_argument: u64,
    /// Optional diagnostic name string pointer for debugging/inspect.
    pub diagnostic_name: *const core::ffi::c_char,
}

zr::static_assert!(core::mem::size_of::<ProcessorPowerLevelFfi>() == 48);
zr::static_assert!(core::mem::align_of::<ProcessorPowerLevelFfi>() == 8);

/// FFI representation of a power domain configuration for energy model registration.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PowerDomainConfigFfi {
    /// Domain identifier.
    pub domain_id: u32,
    /// Bitmask of CPU core IDs belonging to this power domain.
    pub cpu_mask: u64,
    /// Pointer to the array of supported power levels.
    pub levels: *const ProcessorPowerLevelFfi,
    /// Number of power levels in the `levels` array.
    pub level_count: usize,
}

zr::static_assert!(core::mem::size_of::<PowerDomainConfigFfi>() == 32);
zr::static_assert!(core::mem::align_of::<PowerDomainConfigFfi>() == 8);

use boot_options::BootOptions;

unsafe extern "C" {
    fn cpp_power_management_register_domains(
        domains: *const PowerDomainConfigFfi,
        domain_count: usize,
    ) -> Result<(), Status>;
    fn cpp_power_management_set_rate_limits(
        cpu_mask: u64,
        min_rate: u64,
        max_rate: u64,
    ) -> Result<(), Status>;
    fn cpp_power_management_boot_boost_enabled() -> bool;
}

/// Registers the provided power domain configurations and energy models with the kernel scheduler.
pub fn power_management_register_domains(domains: &[PowerDomainConfigFfi]) -> Result<(), Status> {
    let ptr = if domains.is_empty() { core::ptr::null() } else { domains.as_ptr() };
    // SAFETY: `domains` is a valid slice of `PowerDomainConfigFfi`.
    unsafe { cpp_power_management_register_domains(ptr, domains.len()) }
}

/// Sets processing rate limits (min and max) for the CPU cores identified by `cpu_mask`.
///
/// # Arguments
///
/// * `cpu_mask` - Bitmask of logical CPU cores to update.
/// * `min_rate` - Minimum processing rate on the `[0, 1000]` user processing rate scale.
/// * `max_rate` - Maximum processing rate on the `[0, 1000]` user processing rate scale.
pub fn power_management_set_rate_limits(
    cpu_mask: u64,
    min_rate: u64,
    max_rate: u64,
) -> Result<(), Status> {
    // SAFETY: Foreign FFI call to C++ kernel scheduler helper.
    unsafe { cpp_power_management_set_rate_limits(cpu_mask, min_rate, max_rate) }
}

/// Returns whether early boot performance boosting is enabled via kernel boot options.
pub fn power_management_boot_boost_enabled() -> bool {
    // SAFETY: Foreign FFI call to C++ boot options query.
    unsafe { cpp_power_management_boot_boost_enabled() }
}

/// Returns whether runtime processor power management (RPPM) is enabled via kernel boot options.
pub fn power_management_rppm_enabled() -> bool {
    BootOptions::get().power_rppm
}

static DEFAULT_OPS: PdevPowerOps = PdevPowerOps {
    reboot: None,
    shutdown: None,
    cpu_off: None,
    cpu_on: None,
    get_cpu_state: None,
    opp_set: None,
    opp_get: None,
    opp_get_domain_count: None,
};

static POWER_OPS: AtomicConstPtr<PdevPowerOps> =
    AtomicConstPtr::new(core::ptr::addr_of!(DEFAULT_OPS));

fn get_ops() -> &'static PdevPowerOps {
    let ops_ptr = POWER_OPS.load(Ordering::Acquire);
    if ops_ptr.is_null() {
        &DEFAULT_OPS
    } else {
        // SAFETY: `POWER_OPS` always points to either `DEFAULT_OPS` or a registered `PdevPowerOps`
        // table with `'static` lifetime guaranteed by the caller of `pdev_register_power`.
        unsafe { &*ops_ptr }
    }
}

/// Registers the platform power operations table with `'static` lifetime.
pub fn pdev_register_power(ops: &'static PdevPowerOps) {
    POWER_OPS.store(ops as *const PdevPowerOps, Ordering::Release);
}

/// Registers the platform power operations table from C-ABI.
///
/// # Safety
///
/// If non-null, `ops` must point to a valid `PdevPowerOps` table that remains valid
/// for the duration of the kernel's execution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pdev_register_power(ops: *const PdevPowerOps) {
    let target_ptr = if ops.is_null() { core::ptr::addr_of!(DEFAULT_OPS) } else { ops };
    // SAFETY: Caller guarantees `ops` points to a valid `PdevPowerOps` table that remains valid
    // for the duration of the kernel's execution.
    POWER_OPS.store(target_ptr, Ordering::Release);
}

/// Swaps the current power operations table with a new one for testing, returning the previous
/// table.
///
/// # Safety
///
/// If non-null, `ops` must point to a valid `PdevPowerOps` table for the duration of the test.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pdev_swap_power_for_test(
    ops: *const PdevPowerOps,
) -> *const PdevPowerOps {
    let target_ptr = if ops.is_null() { core::ptr::addr_of!(DEFAULT_OPS) } else { ops };
    // SAFETY: Caller guarantees `ops` points to a valid `PdevPowerOps` table for the duration of
    // the test.
    POWER_OPS.swap(target_ptr, Ordering::AcqRel)
}

/// Reboots the system using the registered power operations.
#[unsafe(no_mangle)]
pub extern "C" fn rust_power_reboot(flags: PowerRebootFlags) {
    let ops = get_ops();
    if let Some(reboot_fn) = ops.reboot {
        let _ = reboot_fn(flags);
    }
}

/// Shuts down the system using the registered power operations.
#[unsafe(no_mangle)]
pub extern "C" fn rust_power_shutdown() {
    let ops = get_ops();
    if let Some(shutdown_fn) = ops.shutdown {
        let _ = shutdown_fn();
    }
}

/// Powers off the calling CPU.
#[unsafe(no_mangle)]
pub extern "C" fn rust_power_cpu_off() -> Result<(), Status> {
    let ops = get_ops();
    if let Some(cpu_off_fn) = ops.cpu_off { cpu_off_fn() } else { Ok(()) }
}

/// Powers on the CPU with the specified hardware ID.
#[unsafe(no_mangle)]
pub extern "C" fn rust_power_cpu_on(
    hw_cpu_id: u64,
    entry: u64,
    context: u64,
) -> Result<(), Status> {
    let ops = get_ops();
    if let Some(cpu_on_fn) = ops.cpu_on { cpu_on_fn(hw_cpu_id, entry, context) } else { Ok(()) }
}

/// Retrieves the power state of the CPU with the specified hardware ID.
///
/// # Safety
///
/// `out_state` must point to valid, writable memory for a `PowerCpuState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_power_get_cpu_state(
    hw_cpu_id: u64,
    out_state: *mut PowerCpuState,
) -> Result<(), Status> {
    if out_state.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let ops = get_ops();
    if let Some(get_cpu_state_fn) = ops.get_cpu_state {
        get_cpu_state_fn(hw_cpu_id, out_state)
    } else {
        Err(Status::NOT_SUPPORTED)
    }
}

/// Sets the Operating Performance Point (OPP) for the specified domain.
#[unsafe(no_mangle)]
pub extern "C" fn rust_power_opp_set(domain_id: u32, opp: u64) -> Result<(), Status> {
    let ops = get_ops();
    if let Some(opp_set_fn) = ops.opp_set {
        opp_set_fn(domain_id, opp)
    } else {
        Err(Status::NOT_SUPPORTED)
    }
}

/// Retrieves the active Operating Performance Point (OPP) for the specified domain.
///
/// # Safety
///
/// `out_opp` must point to valid, writable memory for a `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_power_opp_get(
    domain_id: u32,
    out_opp: *mut u64,
) -> Result<(), Status> {
    if out_opp.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let ops = get_ops();
    if let Some(opp_get_fn) = ops.opp_get {
        opp_get_fn(domain_id, out_opp)
    } else {
        Err(Status::NOT_SUPPORTED)
    }
}

/// Retrieves the number of supported OPP control domains.
///
/// # Safety
///
/// `out_count` must point to valid, writable memory for a `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_power_opp_get_domain_count(
    out_count: *mut usize,
) -> Result<(), Status> {
    if out_count.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let ops = get_ops();
    if let Some(opp_get_domain_count_fn) = ops.opp_get_domain_count {
        opp_get_domain_count_fn(out_count)
    } else {
        Err(Status::NOT_SUPPORTED)
    }
}

/// In-kernel unit tests for the platform power subsystem.
#[cfg(ktest)]
#[unittest::suite(name = "pdev_power")]
mod tests {
    use crate::pdev_power::{
        PdevPowerOps, PowerCpuState, PowerRebootFlags, power_management_register_domains,
        power_management_set_rate_limits, rust_pdev_swap_power_for_test, rust_power_cpu_off,
        rust_power_cpu_on, rust_power_get_cpu_state, rust_power_opp_get,
        rust_power_opp_get_domain_count, rust_power_opp_set, rust_power_reboot,
        rust_power_shutdown,
    };
    use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use unittest::{assert_eq, assert_err, assert_ok};
    use zx_status::Status;

    static TEST_REBOOT_CALLED: AtomicU32 = AtomicU32::new(0);
    static TEST_SHUTDOWN_CALLED: AtomicU32 = AtomicU32::new(0);
    static TEST_CPU_OFF_CALLED: AtomicU32 = AtomicU32::new(0);
    static TEST_CPU_ON_HW_ID: AtomicU64 = AtomicU64::new(0);
    static TEST_OPP_SET_DOMAIN: AtomicU32 = AtomicU32::new(0);
    static TEST_OPP_SET_VALUE: AtomicU64 = AtomicU64::new(0);

    extern "C" fn test_reboot(flags: PowerRebootFlags) -> Result<(), Status> {
        TEST_REBOOT_CALLED.store(flags as u32 + 1, Ordering::Relaxed);
        Ok(())
    }

    extern "C" fn test_shutdown() -> Result<(), Status> {
        TEST_SHUTDOWN_CALLED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    extern "C" fn test_cpu_off() -> Result<(), Status> {
        TEST_CPU_OFF_CALLED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    extern "C" fn test_cpu_on(hw_cpu_id: u64, _entry: u64, _context: u64) -> Result<(), Status> {
        TEST_CPU_ON_HW_ID.store(hw_cpu_id, Ordering::Relaxed);
        Ok(())
    }

    extern "C" fn test_get_cpu_state(
        hw_cpu_id: u64,
        out_state: *mut PowerCpuState,
    ) -> Result<(), Status> {
        if !out_state.is_null() {
            // SAFETY: Caller provides a valid pointer for test.
            unsafe {
                *out_state = if hw_cpu_id == 42 { PowerCpuState::On } else { PowerCpuState::Off };
            }
        }
        Ok(())
    }

    extern "C" fn test_opp_set(domain_id: u32, opp: u64) -> Result<(), Status> {
        TEST_OPP_SET_DOMAIN.store(domain_id, Ordering::Relaxed);
        TEST_OPP_SET_VALUE.store(opp, Ordering::Relaxed);
        Ok(())
    }

    extern "C" fn test_opp_get(domain_id: u32, out_opp: *mut u64) -> Result<(), Status> {
        if domain_id == 1 && !out_opp.is_null() {
            // SAFETY: Caller provides a valid pointer for test.
            unsafe {
                *out_opp = 7;
            }
            Ok(())
        } else {
            Err(Status::INVALID_ARGS)
        }
    }

    extern "C" fn test_opp_get_domain_count(out_count: *mut usize) -> Result<(), Status> {
        if !out_count.is_null() {
            // SAFETY: Caller provides a valid pointer for test.
            unsafe {
                *out_count = 2;
            }
            Ok(())
        } else {
            Err(Status::INVALID_ARGS)
        }
    }

    static TEST_OPS: PdevPowerOps = PdevPowerOps {
        reboot: Some(test_reboot),
        shutdown: Some(test_shutdown),
        cpu_off: Some(test_cpu_off),
        cpu_on: Some(test_cpu_on),
        get_cpu_state: Some(test_get_cpu_state),
        opp_set: Some(test_opp_set),
        opp_get: Some(test_opp_get),
        opp_get_domain_count: Some(test_opp_get_domain_count),
    };

    struct TestOpsGuard {
        previous: *const PdevPowerOps,
    }

    impl TestOpsGuard {
        fn new(ops: *const PdevPowerOps) -> Self {
            // SAFETY: Swapping ops for the duration of the test.
            let previous = unsafe { rust_pdev_swap_power_for_test(ops) };
            Self { previous }
        }
    }

    impl Drop for TestOpsGuard {
        fn drop(&mut self) {
            // SAFETY: Restoring the previous ops table when test ends.
            unsafe {
                rust_pdev_swap_power_for_test(self.previous);
            }
        }
    }

    /// Tests that default operations fallback cleanly without registered power ops.
    #[test]
    fn test_default_ops_fallback() {
        let _guard = TestOpsGuard::new(core::ptr::null());

        assert_ok!(rust_power_cpu_off());
        assert_ok!(rust_power_cpu_on(1, 0, 0));

        let mut cpu_state = PowerCpuState::Off;
        // SAFETY: Pointer to local stack variable is valid and aligned.
        let state_res = unsafe { rust_power_get_cpu_state(1, &mut cpu_state) };
        assert_err!(state_res, Status::NOT_SUPPORTED);

        assert_err!(rust_power_opp_set(0, 1), Status::NOT_SUPPORTED);

        let mut opp = 0u64;
        // SAFETY: Pointer to local stack variable is valid and aligned.
        let opp_res = unsafe { rust_power_opp_get(0, &mut opp) };
        assert_err!(opp_res, Status::NOT_SUPPORTED);

        let mut domain_count = 0usize;
        // SAFETY: Pointer to local stack variable is valid and aligned.
        let count_res = unsafe { rust_power_opp_get_domain_count(&mut domain_count) };
        assert_err!(count_res, Status::NOT_SUPPORTED);
    }

    /// Tests that registered power ops are dispatched correctly.
    #[test]
    fn test_registered_ops_dispatch() {
        let _guard = TestOpsGuard::new(&TEST_OPS);

        TEST_REBOOT_CALLED.store(0, Ordering::Relaxed);
        rust_power_reboot(PowerRebootFlags::Recovery);
        assert_eq!(
            TEST_REBOOT_CALLED.load(Ordering::Relaxed),
            PowerRebootFlags::Recovery as u32 + 1
        );

        TEST_SHUTDOWN_CALLED.store(0, Ordering::Relaxed);
        rust_power_shutdown();
        assert_eq!(TEST_SHUTDOWN_CALLED.load(Ordering::Relaxed), 1);

        TEST_CPU_OFF_CALLED.store(0, Ordering::Relaxed);
        assert_ok!(rust_power_cpu_off());
        assert_eq!(TEST_CPU_OFF_CALLED.load(Ordering::Relaxed), 1);

        TEST_CPU_ON_HW_ID.store(0, Ordering::Relaxed);
        assert_ok!(rust_power_cpu_on(1234, 0, 0));
        assert_eq!(TEST_CPU_ON_HW_ID.load(Ordering::Relaxed), 1234);

        let mut state = PowerCpuState::Off;
        // SAFETY: Valid pointer to stack variable.
        assert_ok!(unsafe { rust_power_get_cpu_state(42, &mut state) });
        assert_eq!(state, PowerCpuState::On);

        TEST_OPP_SET_DOMAIN.store(0, Ordering::Relaxed);
        TEST_OPP_SET_VALUE.store(0, Ordering::Relaxed);
        assert_ok!(rust_power_opp_set(3, 5));
        assert_eq!(TEST_OPP_SET_DOMAIN.load(Ordering::Relaxed), 3);
        assert_eq!(TEST_OPP_SET_VALUE.load(Ordering::Relaxed), 5);

        let mut opp = 0u64;
        // SAFETY: Valid pointer to stack variable.
        assert_ok!(unsafe { rust_power_opp_get(1, &mut opp) });
        assert_eq!(opp, 7);

        let mut domain_count = 0usize;
        // SAFETY: Valid pointer to stack variable.
        assert_ok!(unsafe { rust_power_opp_get_domain_count(&mut domain_count) });
        assert_eq!(domain_count, 2);
    }

    /// Tests null pointer handling for power operations.
    #[test]
    fn test_null_pointer_arguments() {
        let _guard = TestOpsGuard::new(&TEST_OPS);

        // SAFETY: Testing null pointer validation behavior.
        assert_err!(
            unsafe { rust_power_get_cpu_state(1, core::ptr::null_mut()) },
            Status::INVALID_ARGS
        );
        // SAFETY: Testing null pointer validation behavior.
        assert_err!(unsafe { rust_power_opp_get(1, core::ptr::null_mut()) }, Status::INVALID_ARGS);
        // SAFETY: Testing null pointer validation behavior.
        assert_err!(
            unsafe { rust_power_opp_get_domain_count(core::ptr::null_mut()) },
            Status::INVALID_ARGS
        );
    }

    /// Tests registering an empty domain slice with power_management_register_domains.
    #[test]
    fn test_power_management_register_domains_empty() {
        let status = power_management_register_domains(&[]);
        assert_ok!(status);
    }

    /// Tests setting rate limits with an empty CPU mask.
    #[test]
    fn test_power_management_set_rate_limits_empty_mask() {
        let status = power_management_set_rate_limits(0, 0, 1000);
        assert_ok!(status);
    }

    /// Tests querying the boot boost enabled configuration.
    #[test]
    fn test_power_management_boot_boost_enabled() {
        let _ = super::power_management_boot_boost_enabled();
    }

    /// Tests querying the RPPM enabled configuration.
    #[test]
    fn test_power_management_rppm_enabled() {
        let _ = super::power_management_rppm_enabled();
    }
}
