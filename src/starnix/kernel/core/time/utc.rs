// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Starnix-specific UTC clock implementation.
//!
//! UTC clock behaves differently in Fuchsia to what Starnix programs expect. This module abstracts
//! the differences away. It provides a UTC clock that always runs. In contrast to Fuchsia UTC
//! clock, which gets started only when the system is reasonably confident that the clock reading
//! is accurate.
//!
//! The paths in this module are somewhat hot, so we document typical measured performance in order
//! to remain performance-aware in this code. Assume that all the performance notes are made using
//! the same baseline device. If you need to add or change performance notes, verify first how far
//! removed your device is from the baseline.
//!
//! Starnix UTC clock is started from [backstop][ff] on initialization, and jumps to actual UTC once
//! Fuchsia provides actual UTC value.
//!
//! Consult the [Fuchsia UTC clock specification][ff] for details about UTC clock behavior
//! specifically on Fuchsia.
//!
//! [ff]: https://fuchsia.dev/fuchsia-src/concepts/kernel/time/utc/behavior#differences_from_other_operating_systems

use fidl_fuchsia_time as fftime;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_runtime::{UtcClock as UtcClockHandle, UtcClockTransform, UtcInstant, UtcTimeline};
use mapped_clock::MappedClock;
use starnix_logging::{log_info, log_warn};
use std::sync::LazyLock;
use zx::{Rights, Unowned};

type MemoryMappedClock = MappedClock<zx::BootTimeline, fuchsia_runtime::UtcTimeline>;

/// The basic rights to use when creating or duplicating a UTC clock. Restrict these
/// on a case-by-case basis only.
///
/// Rights:
///
/// - `Rights::DUPLICATE`, `Rights::TRANSFER`: used to forward the UTC clock in runners.
/// - `Rights::READ`: used to read the clock indication.
/// - `Rights::WAIT`: used to wait on signals such as "clock is updated" or "clock is started".
/// - `Rights::MAP`, `Rights::INSPECT`: used to memory-map the UTC clock.
///
/// The `Rights::WRITE` is notably absent, since on Fuchsia this right is given to particular
/// components only and a writable clock can not be obtained via procargs.
pub static UTC_CLOCK_BASIC_RIGHTS: std::sync::LazyLock<zx::Rights> =
    std::sync::LazyLock::new(|| {
        Rights::DUPLICATE
            | Rights::READ
            | Rights::WAIT
            | Rights::TRANSFER
            | Rights::MAP
            | Rights::INSPECT
    });

// Stores a vendored handle from a test fixture. In normal operation the value here must be
// `None`. In some Starnix container tests, we inject a custom UTC clock that the tests
// manipulate. This is a very special circumstance, so we log warnings accordingly.
static VENDORED_UTC_HANDLE_FOR_TESTS: LazyLock<Option<UtcClockHandle>> = LazyLock::new(|| {
    connect_to_protocol_sync::<fftime::MaintenanceMarker>()
        .inspect_err(|err| {
            log_info!("could not connect to fuchsia.time.Maintenance, this is expected to work only in special test code: {err:?}");
        })
        .map(|proxy: fftime::MaintenanceSynchronousProxy| {
            // Even in test code, the handle we obtain here will typically not be writable. The
            // test fixture will ensure this is the case.
            proxy.get_writable_utc_clock(zx::MonotonicInstant::INFINITE)
            .inspect_err(|err| {log_warn!("while getting UTC clock: {err:?}");})
            .map(|handle: zx::Clock| {
                // Verify that the handle koid matches with the handle koid logged by the UTC vendor component.
                log_warn!("Starnix kernel is using a vendored UTC handle. This is acceptable ONLY in tests.");
                log_warn!("Vendored UTC clock handle koid: {:?}", handle.koid());
                // Make sure to remove unneeded rights, even if we know that the test fixture will
                // give us proper handle rights.
                 handle.replace_handle(*UTC_CLOCK_BASIC_RIGHTS)
                    .map(|handle| handle.cast())
                    .inspect_err(|err| {
                        panic!("Could not replace UTC handle for vendored UTC clock: {err:?}");
                    }).ok()
            }).unwrap_or(None)
        }).unwrap_or(None)
});

fn utc_clock() -> Unowned<'static, UtcClockHandle> {
    VENDORED_UTC_HANDLE_FOR_TESTS
        .as_ref()
        .map(|handle| Unowned::new(handle))
        .unwrap_or_else(zx_libc::utc::reference_get)
}

fn duplicate_utc_clock_handle(rights: zx::Rights) -> Result<UtcClockHandle, zx::Status> {
    utc_clock().duplicate_handle(rights)
}

// Check whether the UTC clock is started based on actual clock read. If you need something
// faster, cache the `read` value. Takes about `350ns` to complete.
fn check_mapped_clock_started(
    clock: &MemoryMappedClock,
    backstop: UtcInstant,
) -> (bool, UtcInstant) {
    let read = clock.read().expect("clock is readable");
    (read != backstop, read)
}

// Returns the details of `clock`.
// Takes around `500ns`.
fn get_utc_clock_details(
    clock: &MemoryMappedClock,
) -> zx::ClockDetails<zx::BootTimeline, UtcTimeline> {
    // 500ns.
    clock.get_details().expect("clock details are readable")
}

// The implementation of a UTC clock that is offered to programs in a Starnix container.
//
// Many Linux APIs need a running UTC clock to function. Since there can be a delay until the UTC
// clock in Zircon starts up (https://fxbug.dev/42081426), Starnix provides a synthetic utc clock
// initially, Once the UTC clock is started, the synthetic utc clock is replaced by a real utc
// clock.
#[derive(Debug)]
pub struct UtcClock {
    // The real underlying Fuchsia UTC clock. This clock may never start,
    // see module-level documentation for details.
    real_utc_clock: UtcClockHandle,
    // The memory mapped clock derived from `real_utc_clock`.
    // Operations on this clock are up to 3x faster than on the companion
    // zx::Clock` object.
    mapped_clock: MemoryMappedClock,
    // The UTC clock transform from boot timeline to UTC timeline, used while
    // `real_utc_clock` is not started.  This clock starts from UTC backstop
    // on boot, and progresses with a nominal 1sec/1sec rate.
    synthetic_transform: UtcClockTransform,
    // The UTC backstop value. This is the earliest UTC value that may ever be
    // shown by any UTC clock in Fuchsia.
    backstop: UtcInstant,
}

impl UtcClock {
    /// Creates a new `UtcClock` instance.
    ///
    /// The `real_utc_clock` is a handle to an underlying Fuchsia UTC clock. It will
    /// be used once started.
    pub fn new(real_utc_clock: UtcClockHandle) -> Self {
        let backstop = real_utc_clock.get_details().unwrap().backstop;
        let synthetic_transform = zx::ClockTransformation {
            // The boot timeline always starts at zero on boot.
            reference_offset: zx::BootInstant::ZERO,
            // By definition, absent other information, a zero reference offset
            // represents a backstop UTC time instant.
            synthetic_offset: backstop,
            // Default rate of 1 synthetic second per 1 reference second disregards
            // any device variations.
            rate: zx::sys::zx_clock_rate_t { synthetic_ticks: 1, reference_ticks: 1 },
        };

        let vmar_parent = fuchsia_runtime::vmar_root_self();
        let real_utc_clock_clone = real_utc_clock
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("UTC clock duplication should work");
        let mapped_clock: MemoryMappedClock =
            MappedClock::try_new(real_utc_clock_clone, &vmar_parent, zx::VmarFlags::PERM_READ)
                .expect("failed to map clock into VMAR");
        let (is_real_utc_clock_started, _) = check_mapped_clock_started(&mapped_clock, backstop);
        let utc_clock = Self { real_utc_clock, mapped_clock, synthetic_transform, backstop };
        if !is_real_utc_clock_started {
            log_warn!(
                "Waiting for real UTC clock to start, using synthetic clock in the meantime."
            );
        }
        utc_clock
    }

    fn duplicate_real_utc_clock_handle(
        &self,
        rights: zx::Rights,
    ) -> Result<UtcClockHandle, zx::Status> {
        self.real_utc_clock.duplicate_handle(rights)
    }

    /// A slower way to verify whether the real UTC clock has started.
    ///
    /// This call takes about `350ns` to complete, refer to the benchmarks
    /// at `//src/lib/mapped-clock/benchmarks`.
    fn check_real_utc_clock_started(&self) -> (bool, UtcInstant) {
        // 350ns.
        check_mapped_clock_started(&self.mapped_clock, self.backstop)
    }

    /// Returns the current Starnix UTC time.
    ///
    /// In Starnix, UTC time is always running. It is started from backstop
    /// at Starnix boot, and adjusted to actual UTC once Fuchsia UTC clock
    /// is started.
    pub fn now(&self) -> UtcInstant {
        // 350 ns.
        let (is_started, utc_now) = self.check_real_utc_clock_started();
        if is_started {
            utc_now
        } else {
            let boot_time = zx::BootInstant::get();
            // Utc time is calculated using the same (constant) transform as the one stored in vdso
            // code. This ensures that the result of `now()` is the same as in
            // `calculate_utc_time_nsec` in `vdso_calculate_utc.cc`.
            self.synthetic_transform.apply(boot_time)
        }
    }

    /// Estimates the boot time corresponding to `utc`.
    ///
    /// # Returns
    /// - zx::BootInstant: estimated boot time;
    /// - bool: true if the system UTC clock has been started.
    ///
    /// Takes about 900ns worst case.
    pub fn estimate_boot_time(&self, utc: UtcInstant) -> (zx::BootInstant, bool) {
        // 350 ns.
        // Could be reduced on average by caching `started`.
        let (started, _) = self.check_real_utc_clock_started();
        let estimated_boot = if started {
            // 500ns.
            let details = get_utc_clock_details(&self.mapped_clock);
            details.reference_to_synthetic.apply_inverse(utc)
        } else {
            self.synthetic_transform.apply_inverse(utc)
        };
        (estimated_boot, started)
    }
}

static UTC_CLOCK: LazyLock<UtcClock> =
    LazyLock::new(|| UtcClock::new(duplicate_utc_clock_handle(zx::Rights::SAME_RIGHTS).unwrap()));

/// Creates a copy of the UTC clock handle currently in use in Starnix.
///
/// Ensure that you are not reading UTC clock for Starnix use from this clock,
/// use the [utc_now] function instead.
pub fn duplicate_real_utc_clock_handle() -> Result<UtcClockHandle, zx::Status> {
    // Maybe reduce rights here?
    (*UTC_CLOCK).duplicate_real_utc_clock_handle(zx::Rights::SAME_RIGHTS)
}

/// Returns the current UTC time based on the Starnix UTC clock.
///
/// The Starnix UTC clock is always started. This is in contrast to Fuchsia's
/// UTC clock which may spend an undefined amount of wall-clock time stuck at
/// [backstop] time reading.
///
/// To ensure an uniform reading of the Starnix UTC clock, always use this
/// function call if you need to know Starnix's view of the current wall time.
///
/// [backstop]: https://fuchsia.dev/fuchsia-src/concepts/kernel/time/utc/behavior#differences_from_other_operating_systems
pub fn utc_now() -> UtcInstant {
    #[cfg(test)]
    {
        if let Some(test_time) = UTC_CLOCK_OVERRIDE_FOR_TESTING
            .with(|cell| cell.borrow().as_ref().map(|test_clock| test_clock.read().unwrap()))
        {
            return test_time;
        }
    }
    (*UTC_CLOCK).now()
}

/// Estimates the boot time corresponding to `utc`, based on the currently
/// operating Starnix UTC clock.
///
/// # Returns
/// - zx::BootInstant: estimated boot time;
/// - bool: true if the system UTC clock has been started.
pub fn estimate_boot_deadline_from_utc(utc: UtcInstant) -> (zx::BootInstant, bool) {
    #[cfg(test)]
    {
        if let Some(test_time) = UTC_CLOCK_OVERRIDE_FOR_TESTING.with(|cell| {
            cell.borrow().as_ref().map(|test_clock| {
                test_clock.get_details().unwrap().reference_to_synthetic.apply_inverse(utc)
            })
        }) {
            return (test_time, true);
        }
    }
    (*UTC_CLOCK).estimate_boot_time(utc)
}

#[cfg(test)]
thread_local! {
    static UTC_CLOCK_OVERRIDE_FOR_TESTING: std::cell::RefCell<Option<UtcClockHandle>> =
        std::cell::RefCell::new(None);
}

/// A guard that temporarily overrides the UTC clock for testing.
///
/// When this guard is created, it replaces the global UTC clock with a test clock. When the guard
/// is dropped, the original clock is restored.
#[cfg(test)]
pub struct UtcClockOverrideGuard(());

#[cfg(test)]
impl UtcClockOverrideGuard {
    /// Creates a new `UtcClockOverrideGuard`.
    ///
    /// This function replaces the global UTC clock with `test_clock`. The original clock is
    /// restored when the returned guard is dropped.
    pub fn new(test_clock: UtcClockHandle) -> Self {
        UTC_CLOCK_OVERRIDE_FOR_TESTING.with(|cell| {
            assert_eq!(*cell.borrow(), None); // We don't expect a previously set clock override when using this type.
            *cell.borrow_mut() = Some(test_clock);
        });
        Self(())
    }
}

#[cfg(test)]
impl Drop for UtcClockOverrideGuard {
    fn drop(&mut self) {
        UTC_CLOCK_OVERRIDE_FOR_TESTING.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}
