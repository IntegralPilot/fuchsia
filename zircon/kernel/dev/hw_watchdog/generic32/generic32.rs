// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/hw_watchdog/generic32/hw_watchdog.cc

use crate::kernel::deadline::{Deadline, SlackMode, TimerSlack};
use crate::kernel::timer::{Timer, ZX_CLOCK_BOOT};
use crate::platform_rs::timer::{
    DurationBoot, DurationUnknown, InstantBoot, InstantUnknown, current_boot_time,
};
use core::pin::Pin;
use core::ptr::with_exposed_provenance_mut;
use debug::dprintf;
use lazy_init::LazyInit;
use regio::{Mmio, MmioPtr, RwSafe};
#[cfg(ktest)]
use unittest as _;
use zbi::{
    DcfgGeneric32Watchdog, DcfgGeneric32WatchdogAction,
    KERNEL_DRIVER_GENERIC32_WATCHDOG_MIN_PERIOD, KernelDriverGeneric32WatchdogFlags,
};
use zx_status::Status;

const FORCE_WATCHDOG_DISABLED_NAME: &str = "kernel.force-watchdog-disabled";

unsafe extern "C" fn watchdog_timer_cb(_timer: *mut Timer, _now: i64, arg: *mut core::ffi::c_void) {
    // SAFETY: The caller must ensure that `arg` is a valid pointer that can be passed
    // to `rust_watchdog_on_pet_timer`.
    unsafe {
        rust_watchdog_on_pet_timer(arg);
    }
}

unsafe extern "C" {
    /// Translates a physical MMIO address in-place to a virtual address. Returns `true` if valid.
    fn cpp_watchdog_translate_paddr(paddr: *mut u64) -> bool;
    /// Returns `true` if the `force_watchdog_disabled` kernel command-line option is set.
    fn cpp_watchdog_is_force_disabled_cmdline() -> bool;
}

/// Inner state of the generic 32-bit watchdog driver, protected by a spinlock.
pub struct GenericWatchdog32Inner {
    pub cfg: DcfgGeneric32Watchdog,
    /// Result code of the early initialization phase.
    pub early_init_result: Result<(), Status>,
    /// Timestamp (`zx_instant_boot_t`) when the watchdog was last pet.
    pub last_pet_time: InstantBoot,
    /// Zircon kernel timer used for scheduling periodic pet callbacks.
    pub pet_timer: core::mem::MaybeUninit<Timer>,
    pub pet_timer_initialized: bool,
    /// Whether the hardware watchdog is currently enabled.
    pub is_enabled: bool,
    /// Whether watchdog petting is currently suppressed (e.g., during panic/crashlog generation).
    pub is_petting_suppressed: bool,
    /// The `GenericWatchdog32` owning this state, which the pet timer callback runs
    /// against. `None` for a free-standing instance (unit tests), which never arms.
    owner: Option<&'static GenericWatchdog32>,
}

impl GenericWatchdog32Inner {
    const fn new() -> Self {
        Self {
            cfg: DcfgGeneric32Watchdog {
                pet_action: DcfgGeneric32WatchdogAction { addr: 0, clr_mask: 0, set_mask: 0 },
                enable_action: DcfgGeneric32WatchdogAction { addr: 0, clr_mask: 0, set_mask: 0 },
                disable_action: DcfgGeneric32WatchdogAction { addr: 0, clr_mask: 0, set_mask: 0 },
                watchdog_period_nsec: 0,
                flags: KernelDriverGeneric32WatchdogFlags::empty(),
                reserved: 0,
            },
            early_init_result: Err(Status::INTERNAL),
            last_pet_time: InstantBoot(0),
            pet_timer: core::mem::MaybeUninit::uninit(),
            pet_timer_initialized: false,
            is_enabled: false,
            is_petting_suppressed: false,
            owner: None,
        }
    }

    /// Performs the read-modify-write MMIO action on the watchdog register using `regio`.
    ///
    /// # Safety
    ///
    /// `action.addr` must be a valid, mapped virtual address pointing to a 32-bit MMIO register.
    unsafe fn take_action(&self, action: &DcfgGeneric32WatchdogAction) {
        if action.addr == 0 {
            return;
        }
        // SAFETY: `action.addr` is verified to be a valid mapped virtual address when non-zero.
        let raw_ptr = with_exposed_provenance_mut::<u32>(action.addr as usize);
        let ptr = unsafe { MmioPtr::<u32, RwSafe>::new(raw_ptr) };
        let reg = Mmio::<u32, u32, RwSafe>::new(ptr);
        reg.modify(|val| {
            *val &= !action.clr_mask;
            *val |= action.set_mask;
        });
    }

    fn pet_locked(&mut self) -> InstantBoot {
        let now = current_boot_time();
        if !self.is_petting_suppressed {
            self.last_pet_time = now;
            // SAFETY: `self.cfg.pet_action` contains the validated MMIO address for petting.
            unsafe { self.take_action(&self.cfg.pet_action) };
        }
        now
    }

    /// Pets the watchdog and re-arms the pet timer for half a period from now.
    fn handle_pet_timer_locked(&mut self) {
        if !self.is_enabled {
            return;
        }
        let last_pet = self.pet_locked();
        if !self.pet_timer_initialized {
            return;
        }
        // The callback runs against the owner, so only an owned instance may arm.
        let Some(owner) = self.owner else {
            return;
        };
        let timeout = self.cfg.watchdog_period_nsec;
        let next_pet_time = last_pet.0.saturating_add(timeout / 2);
        let slack = timeout / 4;
        let deadline = Deadline::new(
            InstantUnknown(next_pet_time),
            TimerSlack::new(DurationUnknown(slack), SlackMode::Early),
        );
        // SAFETY: `owner` is a `'static` reference, so `pet_timer` is in a stable location
        // and we can safely create a Pin<&mut Timer>.
        unsafe {
            let mut timer = core::pin::Pin::new_unchecked(&mut *self.pet_timer.as_mut_ptr());
            timer.as_mut().set_deadline(
                &deadline,
                watchdog_timer_cb,
                core::ptr::from_ref(owner).cast_mut().cast(),
            );
        }
    }

    fn init_early(&mut self, config: &DcfgGeneric32Watchdog) {
        // SAFETY: We initialize pet_timer in place using its pin initializer.
        unsafe {
            let slot = self.pet_timer.as_mut_ptr();
            if pin_init::PinInit::__pinned_init(Timer::init(ZX_CLOCK_BOOT), slot).is_ok() {
                self.pet_timer_initialized = true;
            }
        }

        if config.pet_action.addr == 0 {
            self.early_init_result = Err(Status::INVALID_ARGS);
            return;
        }
        if config.watchdog_period_nsec < KERNEL_DRIVER_GENERIC32_WATCHDOG_MIN_PERIOD {
            self.early_init_result = Err(Status::INVALID_ARGS);
            return;
        }

        self.cfg = *config;
        // SAFETY: `self.cfg.pet_action.addr` is a pointer to the address field in our local config copy, valid for modification by the C++ translation shim.
        if !unsafe { cpp_watchdog_translate_paddr(&mut self.cfg.pet_action.addr) } {
            self.early_init_result = Err(Status::IO);
            return;
        }
        // SAFETY: `enable_action.addr` and `disable_action.addr` point to valid `u64` address fields in `self.cfg`.
        unsafe {
            cpp_watchdog_translate_paddr(&mut self.cfg.enable_action.addr);
            cpp_watchdog_translate_paddr(&mut self.cfg.disable_action.addr);
        }

        self.is_enabled = self.cfg.flags.contains(KernelDriverGeneric32WatchdogFlags::ENABLED);

        self.pet_locked();
        // SAFETY: Calling C++ helper to read kernel boot options requires no parameters and has no side effects.
        if unsafe { cpp_watchdog_is_force_disabled_cmdline() }
            && self.is_enabled
            && self.cfg.disable_action.addr != 0
        {
            // SAFETY: `disable_action` contains the validated MMIO address for disabling.
            unsafe { self.take_action(&self.cfg.disable_action) };
            self.is_enabled = false;
        }

        self.early_init_result = Ok(());
    }

    fn init(&mut self) {
        if let Err(res) = self.early_init_result {
            dprintf!(
                INFO,
                "WDT: Generic watchdog driver attempted to load, but failed during early init \
                 (res {res}).\n",
            );
            return;
        }
        let force_disabled = unsafe { cpp_watchdog_is_force_disabled_cmdline() };
        let can_disable = self.cfg.disable_action.addr != 0;
        let msec = self.cfg.watchdog_period_nsec / 1_000_000;
        let usec = (self.cfg.watchdog_period_nsec % 1_000_000) / 1000;
        dprintf!(
            INFO,
            "WDT: Generic watchdog driver loaded.  Period ({}.{:03} mSec) Enabled ({})\n",
            msec,
            usec,
            if self.is_enabled { "yes" } else { "no" }
        );
        if force_disabled {
            if can_disable {
                dprintf!(
                    INFO,
                    "WDT: {} was set, watchdog was force-disabled\n",
                    FORCE_WATCHDOG_DISABLED_NAME
                );
            } else {
                dprintf!(
                    INFO,
                    "WDT: {} was set, but the watchdog cannot be disabled.  It is currently {}.\n",
                    FORCE_WATCHDOG_DISABLED_NAME,
                    if self.is_enabled { "enabled" } else { "disabled" }
                );
            }
        }
        self.handle_pet_timer_locked();
    }

    fn set_enabled(&mut self, enb: bool) -> Result<(), Status> {
        if enb == self.is_enabled {
            return Ok(());
        }
        let target_addr =
            if enb { self.cfg.enable_action.addr } else { self.cfg.disable_action.addr };
        if target_addr == 0 {
            return Err(Status::NOT_SUPPORTED);
        }
        self.is_enabled = enb;
        if self.is_enabled {
            // SAFETY: `enable_action` contains the validated MMIO address for enabling.
            unsafe { self.take_action(&self.cfg.enable_action) };
            self.handle_pet_timer_locked();
        } else {
            // SAFETY: `disable_action` contains the validated MMIO address for disabling.
            unsafe { self.take_action(&self.cfg.disable_action) };
            if self.pet_timer_initialized {
                // SAFETY: G_WATCHDOG is static, so pet_timer is pinned.
                let mut timer =
                    unsafe { core::pin::Pin::new_unchecked(&mut *self.pet_timer.as_mut_ptr()) };
                timer.as_mut().cancel();
            }
        }
        Ok(())
    }
}

/// Global 32-bit generic hardware watchdog driver structure.
#[ksync::guarded]
pub struct GenericWatchdog32 {
    #[guarded_by(lock)]
    inner: GenericWatchdog32Inner,

    #[mutex]
    lock: ksync::KMutex<ksync::RawSpinlock>,
}

unsafe impl Sync for GenericWatchdog32 {}

static G_WATCHDOG: LazyInit<GenericWatchdog32> = LazyInit::uninit();

/// Callback invoked from Zircon timer tick when the pet timer expires.
///
/// # Safety
///
/// `arg` must be a pointer to `GenericWatchdog32`.
#[unsafe(no_mangle)]
unsafe extern "C" fn rust_watchdog_on_pet_timer(arg: *mut core::ffi::c_void) {
    // SAFETY: `arg` is the `owner` recorded at early init, so it points to a live
    // `GenericWatchdog32` in static storage.
    let watchdog = unsafe { &*(arg as *const GenericWatchdog32) };
    ksync::lock!(let mut guard = watchdog.lock_lock());
    guard.as_mut().fields_mut().inner.handle_pet_timer_locked();
}

impl crate::pdev_watchdog::WatchdogOps for GenericWatchdog32 {
    fn pet(&self) {
        ksync::lock!(let mut guard = self.lock_lock());
        let inner = guard.as_mut().fields_mut().inner;
        if inner.is_enabled {
            inner.pet_locked();
        }
    }

    fn set_enabled(&self, enb: bool) -> Result<(), Status> {
        ksync::lock!(let mut guard = self.lock_lock());
        guard.as_mut().fields_mut().inner.set_enabled(enb)
    }

    fn is_enabled(&self) -> bool {
        ksync::lock!(let guard = self.lock_lock());
        guard.fields().inner.is_enabled
    }

    fn get_timeout_nsec(&self) -> DurationBoot {
        ksync::lock!(let guard = self.lock_lock());
        DurationBoot(guard.fields().inner.cfg.watchdog_period_nsec)
    }

    fn get_last_pet_time(&self) -> InstantBoot {
        ksync::lock!(let guard = self.lock_lock());
        guard.fields().inner.last_pet_time
    }

    fn suppress_petting(&self, suppress: bool) {
        ksync::lock!(let mut guard = self.lock_lock());
        guard.as_mut().fields_mut().inner.is_petting_suppressed = suppress;
    }

    fn is_petting_suppressed(&self) -> bool {
        ksync::lock!(let guard = self.lock_lock());
        guard.fields().inner.is_petting_suppressed
    }
}

/// Early initialization routine for the generic 32-bit watchdog driver.
///
/// # Safety
///
/// `config` must be a valid pointer to a `DcfgGeneric32Watchdog` structure.
/// Initialization must be serialized with respect to any other access.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generic_32bit_watchdog_early_init(config: *const DcfgGeneric32Watchdog) {
    if config.is_null() {
        return;
    }
    // SAFETY: Initialization is serialized with respect to any other access.
    let _ = unsafe {
        Pin::static_ref(&G_WATCHDOG).init_pin(pin_init::pin_init!(GenericWatchdog32 {
            inner: ksync::KCell::new(GenericWatchdog32Inner::new()),
            lock <- ksync::KSpinlock::init(),
        }))
    };
    let is_ok = {
        ksync::lock!(let mut guard = G_WATCHDOG.lock_lock());
        // SAFETY: `config` is checked to be non-null and guaranteed by caller to point to a valid `DcfgGeneric32Watchdog`.
        let inner = guard.as_mut().fields_mut().inner;
        inner.owner = Some(&G_WATCHDOG);
        inner.init_early(unsafe { &*config });
        inner.early_init_result.is_ok()
    };
    if is_ok {
        crate::pdev_watchdog::register_watchdog(G_WATCHDOG.get());
    }
}

/// Post-VM initialization routine for the generic 32-bit watchdog driver.
/// Currently a no-op matching C++.
///
/// # Safety
///
/// `config` must be a valid pointer to a `DcfgGeneric32Watchdog` structure if accessed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generic_32bit_watchdog_init_post_vm(
    _config: *const DcfgGeneric32Watchdog,
) {
}

/// Late multi-threaded initialization routine for the generic 32-bit watchdog driver.
#[unsafe(no_mangle)]
pub extern "C" fn generic_32bit_watchdog_late_init() {
    ksync::lock!(let mut guard = G_WATCHDOG.lock_lock());
    guard.as_mut().fields_mut().inner.init();
}

/// Generic 32-bit watchdog kernel unit tests.
#[cfg(ktest)]
#[unittest::suite(name = "generic32_watchdog")]
mod tests {
    use unittest::{assert_eq, assert_err, assert_false, assert_ok, assert_true};

    /// Tests `take_action` MMIO read-modify-write on a mock register buffer and no-op on zero address.
    #[test]
    fn test_generic32_take_action_mock_mmio() {
        let mut mock_mmio: u32 = 0x1234_5678;
        let action = DcfgGeneric32WatchdogAction {
            addr: core::ptr::addr_of_mut!(mock_mmio) as u64,
            clr_mask: 0x0000_FF00,
            set_mask: 0x0000_00AB,
        };
        let zero_action =
            DcfgGeneric32WatchdogAction { addr: 0, clr_mask: 0xFFFF_FFFF, set_mask: 0xFFFF_FFFF };
        let inner = GenericWatchdog32Inner::new();
        // SAFETY: `action` points to valid stack u32, and `zero_action` has addr 0 which safely returns early.
        unsafe {
            inner.take_action(&action);
            inner.take_action(&zero_action);
        }
        assert_eq!(mock_mmio, 0x1234_00FB);

        // Verify region Mmio register construction and read interface directly.
        let raw_ptr = core::ptr::addr_of_mut!(mock_mmio);
        let ptr = unsafe { MmioPtr::<u32, RwSafe>::new(raw_ptr) };
        let reg = Mmio::<u32, u32, RwSafe>::new(ptr);
        assert_eq!(reg.read(), 0x1234_00FB);
    }

    /// Tests `init_early` configuration parsing and validation.
    #[test]
    fn test_generic32_config_parsing() {
        let mut inner = GenericWatchdog32Inner::new();
        let mut bad_config = DcfgGeneric32Watchdog {
            pet_action: DcfgGeneric32WatchdogAction { addr: 0, clr_mask: 0, set_mask: 0 },
            enable_action: DcfgGeneric32WatchdogAction { addr: 0, clr_mask: 0, set_mask: 0 },
            disable_action: DcfgGeneric32WatchdogAction { addr: 0, clr_mask: 0, set_mask: 0 },
            watchdog_period_nsec: KERNEL_DRIVER_GENERIC32_WATCHDOG_MIN_PERIOD,
            flags: KernelDriverGeneric32WatchdogFlags::empty(),
            reserved: 0,
        };
        inner.init_early(&bad_config);
        assert_err!(inner.early_init_result, Status::INVALID_ARGS);

        let mut mock_mmio: u32 = 0;
        bad_config.pet_action.addr = core::ptr::addr_of_mut!(mock_mmio) as u64;
        bad_config.watchdog_period_nsec = KERNEL_DRIVER_GENERIC32_WATCHDOG_MIN_PERIOD - 1;
        inner.init_early(&bad_config);
        assert_err!(inner.early_init_result, Status::INVALID_ARGS);

        bad_config.watchdog_period_nsec = KERNEL_DRIVER_GENERIC32_WATCHDOG_MIN_PERIOD;
        inner.init_early(&bad_config);
        assert_ok!(inner.early_init_result);
    }

    /// Tests watchdog state transitions when enabling, disabling, and suppressing petting.
    #[test]
    fn test_generic32_state_transitions() {
        let mut mock_pet: u32 = 0;
        let mut mock_enb: u32 = 0;
        let mut mock_dis: u32 = 0;

        let config = DcfgGeneric32Watchdog {
            pet_action: DcfgGeneric32WatchdogAction {
                addr: core::ptr::addr_of_mut!(mock_pet) as u64,
                clr_mask: 0,
                set_mask: 1,
            },
            enable_action: DcfgGeneric32WatchdogAction {
                addr: core::ptr::addr_of_mut!(mock_enb) as u64,
                clr_mask: 0,
                set_mask: 2,
            },
            disable_action: DcfgGeneric32WatchdogAction {
                addr: core::ptr::addr_of_mut!(mock_dis) as u64,
                clr_mask: 0,
                set_mask: 4,
            },
            watchdog_period_nsec: KERNEL_DRIVER_GENERIC32_WATCHDOG_MIN_PERIOD,
            flags: KernelDriverGeneric32WatchdogFlags::empty(),
            reserved: 0,
        };

        let mut inner = GenericWatchdog32Inner::new();
        inner.init_early(&config);
        assert_ok!(inner.early_init_result);
        assert_false!(inner.is_enabled);

        assert_true!(inner.set_enabled(true).is_ok());
        assert_true!(inner.is_enabled);
        assert_eq!(mock_enb, 2);

        mock_enb = 0;
        assert_true!(inner.set_enabled(true).is_ok());
        assert_eq!(mock_enb, 0);

        assert_false!(inner.is_petting_suppressed);
        inner.is_petting_suppressed = true;
        assert_true!(inner.is_petting_suppressed);

        assert_true!(inner.set_enabled(false).is_ok());
        assert_false!(inner.is_enabled);
        assert_eq!(mock_dis, 4);
    }

    /// Tests global boot handoff null safety and `WatchdogOps` trait implementation.
    #[test]
    fn test_generic32_thunks_and_handoff_apis() {
        // SAFETY: Testing global C-ABI initialization functions with null pointers, which should safely early-return as no-ops.
        unsafe {
            generic_32bit_watchdog_early_init(core::ptr::null());
            generic_32bit_watchdog_init_post_vm(core::ptr::null());
        }

        // Verify trait methods on a local `GenericWatchdog32` instance.
        let initializer = pin_init::pin_init!(GenericWatchdog32 {
            inner: ksync::KCell::new(GenericWatchdog32Inner::new()),
            lock <- ksync::KSpinlock::init(),
        });
        pin_init::stack_pin_init!(let watchdog = initializer);

        use crate::pdev_watchdog::WatchdogOps as _;
        assert_false!(watchdog.is_enabled());
        watchdog.suppress_petting(true);
        assert_true!(watchdog.is_petting_suppressed());
        watchdog.suppress_petting(false);
        assert_false!(watchdog.is_petting_suppressed());
        watchdog.pet();
    }
}
