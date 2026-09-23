// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/pdev/hw_watchdog/hw_watchdog.cc

use crate::platform_rs::timer::{DurationBoot, InstantBoot};
use core::cell::UnsafeCell;
use core::sync::atomic::Ordering;
#[cfg(ktest)]
use unittest as _;
use zx_status::Status;

/// Hardware watchdog trait defining the platform-specific operations.
pub trait WatchdogOps: Sync + 'static {
    /// Callback to pet the hardware watchdog.
    fn pet(&self);
    /// Callback to set the enabled/disabled state of the watchdog.
    fn set_enabled(&self, enabled: bool) -> Result<(), Status>;
    /// Callback returning `true` if the watchdog is currently enabled.
    fn is_enabled(&self) -> bool;
    /// Callback returning the nominal timeout period in nanoseconds (`zx_duration_boot_t`).
    fn get_timeout_nsec(&self) -> DurationBoot;
    /// Callback returning the last successful pet time (`zx_instant_boot_t`).
    fn get_last_pet_time(&self) -> InstantBoot;
    /// Callback to enable (`true`) or disable (`false`) suppression of future pets.
    fn suppress_petting(&self, suppress: bool);
    /// Callback returning `true` if petting is currently suppressed.
    fn is_petting_suppressed(&self) -> bool;
}

struct DriverHolder(UnsafeCell<Option<&'static dyn WatchdogOps>>);
unsafe impl Sync for DriverHolder {}

static ACTIVE_DRIVER: DriverHolder = DriverHolder(UnsafeCell::new(None));

struct DefaultWatchdog;

impl WatchdogOps for DefaultWatchdog {
    fn pet(&self) {}
    fn set_enabled(&self, _: bool) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }
    fn is_enabled(&self) -> bool {
        false
    }
    fn get_timeout_nsec(&self) -> DurationBoot {
        DurationBoot(i64::MAX)
    }
    fn get_last_pet_time(&self) -> InstantBoot {
        InstantBoot(0)
    }
    fn suppress_petting(&self, _: bool) {}
    fn is_petting_suppressed(&self) -> bool {
        true
    }
}

static DEFAULT_DRIVER: DefaultWatchdog = DefaultWatchdog;

/// Resolves a registered driver, falling back to the no-op default when none is present.
fn resolve_driver(driver: Option<&'static dyn WatchdogOps>) -> &'static dyn WatchdogOps {
    driver.unwrap_or(&DEFAULT_DRIVER)
}

fn get_driver() -> &'static dyn WatchdogOps {
    // SAFETY: ACTIVE_DRIVER is written once during single-threaded early boot, and read immutable thereafter.
    resolve_driver(unsafe { *ACTIVE_DRIVER.0.get() })
}

use debug::dprintf;

/// Registers the hardware watchdog driver with the PDEV dispatch layer.
pub fn register_watchdog(driver: &'static dyn WatchdogOps) {
    // SAFETY: Called exactly once during single-threaded early boot before secondary CPUs run.
    unsafe {
        *ACTIVE_DRIVER.0.get() = Some(driver);
    }
    // Architectural memory barrier to guarantee the vtable and data pointers,
    // along with the driver's early initialization writes, are visible to all
    // future CPUs before they boot up and enable interrupts.
    core::sync::atomic::fence(Ordering::SeqCst);

    dprintf!(INFO, "watchdog registered: timeout={}ns\n", driver.get_timeout_nsec().0);
}

/// Returns `true` if this platform has a hardware watchdog, `false` otherwise.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_present() -> bool {
    // SAFETY: Checking whether ACTIVE_DRIVER is Some. Written once during single-threaded early boot.
    unsafe { (*ACTIVE_DRIVER.0.get()).is_some() }
}

/// Pets the hardware watchdog if present and petting is not suppressed.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_pet() {
    get_driver().pet();
}

/// Attempts to enable or disable the hardware watchdog. Note that depending on
/// hardware details, it may not be possible to change its enabled/disabled state.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_set_enabled(enabled: bool) -> Result<(), Status> {
    get_driver().set_enabled(enabled)
}

/// Returns `true` if the hardware watchdog is currently enabled.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_is_enabled() -> bool {
    get_driver().is_enabled()
}

/// Returns the nominal hardware watchdog timeout period in nanoseconds (`zx_duration_boot_t`).
/// Returns `ZX_TIME_INFINITE` (`i64::MAX`) if no hardware watchdog is present.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_get_timeout_nsec() -> i64 {
    get_driver().get_timeout_nsec().0
}

/// Returns the time (`zx_instant_boot_t`) of the last successful pet of the hardware watchdog.
/// Returns `0` if no hardware watchdog is present or has never been pet.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_get_last_pet_time() -> i64 {
    get_driver().get_last_pet_time().0
}

/// Sets whether future pets to the hardware watchdog should be suppressed (`true`)
/// or allowed (`false`). Used during kernel crash/panic paths so the watchdog will
/// trigger a reboot if the system hangs during crash dump.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_suppress_petting(suppressed: bool) {
    get_driver().suppress_petting(suppressed);
}

/// Returns `true` if hardware watchdog petting is currently suppressed.
///
/// # Safety
///
/// The caller must ensure that `get_driver()` returns a valid reference to a
/// `WatchdogOps` trait object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hw_watchdog_is_petting_suppressed() -> bool {
    get_driver().is_petting_suppressed()
}

// The registered global is read concurrently from timer context, so these drive
// `resolve_driver` directly rather than publishing a driver.
/// PDEV hardware watchdog layer kernel tests.
#[cfg(ktest)]
#[unittest::suite(name = "hw_watchdog")]
mod tests {
    use super::{DurationBoot, InstantBoot, Status, WatchdogOps, resolve_driver};
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use unittest::{assert_eq, assert_err, assert_false, assert_ok, assert_true};

    /// Test default ops dispatch table fallback behavior and default state.
    #[test]
    fn test_pdev_watchdog_default_driver_ops() {
        // With no driver registered, dispatch falls back to the no-op default.
        let driver = resolve_driver(None);

        assert_err!(driver.set_enabled(true), Status::NOT_SUPPORTED);
        assert_false!(driver.is_enabled());
        assert_eq!(driver.get_timeout_nsec().0, i64::MAX);
        assert_eq!(driver.get_last_pet_time().0, 0);

        // Verify default no-op callbacks don't crash when invoked.
        driver.pet();
        driver.suppress_petting(true);
        driver.suppress_petting(false);

        // Suppression is unconditional for the default driver.
        assert_true!(driver.is_petting_suppressed());
    }

    /// Test that a driver's hooks are reached through the dispatch layer.
    #[test]
    fn test_pdev_watchdog_custom_driver_dispatch() {
        static PET_COUNT: AtomicU32 = AtomicU32::new(0);
        static SUPPRESS_STATE: AtomicBool = AtomicBool::new(false);

        struct DummyWatchdog;
        impl WatchdogOps for DummyWatchdog {
            fn pet(&self) {
                PET_COUNT.fetch_add(1, Ordering::SeqCst);
            }
            fn set_enabled(&self, _: bool) -> Result<(), Status> {
                Ok(())
            }
            fn is_enabled(&self) -> bool {
                true
            }
            fn get_timeout_nsec(&self) -> DurationBoot {
                DurationBoot(12345)
            }
            fn get_last_pet_time(&self) -> InstantBoot {
                InstantBoot(67890)
            }
            fn suppress_petting(&self, suppressed: bool) {
                SUPPRESS_STATE.store(suppressed, Ordering::SeqCst);
            }
            fn is_petting_suppressed(&self) -> bool {
                SUPPRESS_STATE.load(Ordering::SeqCst)
            }
        }

        static DUMMY_DRIVER: DummyWatchdog = DummyWatchdog;

        PET_COUNT.store(0, Ordering::SeqCst);
        SUPPRESS_STATE.store(false, Ordering::SeqCst);

        // A registered driver is dispatched to in preference to the default.
        let driver = resolve_driver(Some(&DUMMY_DRIVER));

        assert_ok!(driver.set_enabled(true));
        assert_true!(driver.is_enabled());
        assert_eq!(driver.get_timeout_nsec().0, 12345);
        assert_eq!(driver.get_last_pet_time().0, 67890);
        assert_false!(driver.is_petting_suppressed());

        // Verify pet hook execution.
        driver.pet();
        assert_eq!(PET_COUNT.load(Ordering::SeqCst), 1);
        driver.pet();
        assert_eq!(PET_COUNT.load(Ordering::SeqCst), 2);

        // Verify suppress_petting hook execution and state propagation.
        driver.suppress_petting(true);
        assert_true!(SUPPRESS_STATE.load(Ordering::SeqCst));
        assert_true!(driver.is_petting_suppressed());

        driver.suppress_petting(false);
        assert_false!(SUPPRESS_STATE.load(Ordering::SeqCst));
        assert_false!(driver.is_petting_suppressed());
    }
}
