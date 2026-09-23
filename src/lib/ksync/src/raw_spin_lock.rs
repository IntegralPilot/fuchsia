// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::raw_kernel_mutex::LockEntryStorage;
use crate::raw_lock::RawLock;
use core::ffi::c_void;
use pin_init::{PinInit, pin_data};

#[cfg(target_arch = "x86_64")]
type InnerSavedState = u64;
#[cfg(not(target_arch = "x86_64"))]
type InnerSavedState = bool;

/// Opaque token representing the saved interrupt state.
#[repr(transparent)]
#[derive(Copy, Clone, Default)]
pub struct InterruptSavedState(InnerSavedState);

unsafe extern "C" {
    fn cpp_spinlock_init(lock: *mut c_void, class_id: *const c_void);
    fn cpp_spinlock_destroy(lock: *mut c_void);
    fn cpp_spinlock_acquire_irqsave(
        lock: *mut c_void,
        entry_storage: *mut c_void,
    ) -> InterruptSavedState;
    fn cpp_spinlock_release_irqrestore(
        lock: *mut c_void,
        entry_storage: *mut c_void,
        state: InterruptSavedState,
    );
    fn cpp_spinlock_acquire_no_irqsave(lock: *mut c_void, entry_storage: *mut c_void);
    fn cpp_spinlock_release_no_irqrestore(lock: *mut c_void, entry_storage: *mut c_void);

    fn cpp_monitored_spinlock_init(lock: *mut c_void, class_id: *const c_void);
    fn cpp_monitored_spinlock_destroy(lock: *mut c_void);
    fn cpp_monitored_spinlock_acquire_irqsave(
        lock: *mut c_void,
        entry_storage: *mut c_void,
        name: *const core::ffi::c_char,
    ) -> InterruptSavedState;
    fn cpp_monitored_spinlock_release_irqrestore(
        lock: *mut c_void,
        entry_storage: *mut c_void,
        state: InterruptSavedState,
    );
    fn cpp_monitored_spinlock_acquire_no_irqsave(
        lock: *mut c_void,
        entry_storage: *mut c_void,
        name: *const core::ffi::c_char,
    );
    fn cpp_monitored_spinlock_release_no_irqrestore(lock: *mut c_void, entry_storage: *mut c_void);
}

#[cfg(feature = "spin_lock_tracing")]
const RAW_SPINLOCK_SIZE: usize = 16;
#[cfg(not(feature = "spin_lock_tracing"))]
const RAW_SPINLOCK_SIZE: usize = 4;

#[repr(C, align(8))]
struct RawSpinlockStorage(zr::OpaqueBytes<RAW_SPINLOCK_SIZE>);

/// Opaque layout block matching the Zircon C++ SpinLock exactly.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct RawSpinlock {
    #[cfg(feature = "lock_dep")]
    class_id: *const c_void,
    storage: RawSpinlockStorage,
}

impl RawSpinlock {
    pub const INIT: Self = Self::const_init(core::ptr::null());

    /// Statically initializes a RawSpinlock in constant context.
    pub const fn const_init(_class_id: *const c_void) -> Self {
        Self {
            #[cfg(feature = "lock_dep")]
            class_id: _class_id,
            storage: RawSpinlockStorage(zr::OpaqueBytes::new([0u8; RAW_SPINLOCK_SIZE])),
        }
    }
}

impl Default for RawSpinlock {
    fn default() -> Self {
        Self::INIT
    }
}

// SAFETY: RawSpinlock is safe to share and access across threads.
unsafe impl Sync for RawSpinlock {}
unsafe impl Send for RawSpinlock {}

zr::unsafe_pinned_drop_ffi!(RawSpinlock, cpp_spinlock_destroy);

/// Opaque layout block matching the Zircon C++ MonitoredSpinLock exactly.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct RawMonitoredSpinlock {
    #[cfg(feature = "lock_dep")]
    class_id: *const c_void,
    storage: RawSpinlockStorage,
}

impl RawMonitoredSpinlock {
    pub const INIT: Self = Self::const_init(core::ptr::null());

    /// Statically initializes a RawMonitoredSpinlock in constant context.
    pub const fn const_init(_class_id: *const c_void) -> Self {
        Self {
            #[cfg(feature = "lock_dep")]
            class_id: _class_id,
            storage: RawSpinlockStorage(zr::OpaqueBytes::new([0u8; RAW_SPINLOCK_SIZE])),
        }
    }
}

impl Default for RawMonitoredSpinlock {
    fn default() -> Self {
        Self::INIT
    }
}

// SAFETY: RawMonitoredSpinlock is safe to share and access across threads.
unsafe impl Sync for RawMonitoredSpinlock {}
unsafe impl Send for RawMonitoredSpinlock {}

zr::unsafe_pinned_drop_ffi!(RawMonitoredSpinlock, cpp_monitored_spinlock_destroy);

pub struct IrqSavePolicy;

impl crate::LockPolicy<RawSpinlock> for IrqSavePolicy {
    type AcquireArgs = ();
    type GuardState = InterruptSavedState;

    #[inline]
    unsafe fn acquire(
        lock: &RawSpinlock,
        entry: *mut LockEntryStorage,
        _args: (),
    ) -> Self::GuardState {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe { cpp_spinlock_acquire_irqsave(lock.as_mut_ptr(), entry as *mut c_void) }
    }

    #[inline]
    unsafe fn reacquire(
        lock: &RawSpinlock,
        entry: *mut LockEntryStorage,
        state: &mut Self::GuardState,
    ) {
        *state = unsafe { Self::acquire(lock, entry, ()) };
    }

    #[inline]
    unsafe fn release(lock: &RawSpinlock, entry: *mut LockEntryStorage, state: Self::GuardState) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe {
            cpp_spinlock_release_irqrestore(lock.as_mut_ptr(), entry as *mut c_void, state);
        }
    }
}

pub struct NoIrqSavePolicy;

impl crate::LockPolicy<RawSpinlock> for NoIrqSavePolicy {
    type AcquireArgs = ();
    type GuardState = ();

    #[inline]
    unsafe fn acquire(
        lock: &RawSpinlock,
        entry: *mut LockEntryStorage,
        _args: (),
    ) -> Self::GuardState {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe { cpp_spinlock_acquire_no_irqsave(lock.as_mut_ptr(), entry as *mut c_void) }
    }

    #[inline]
    unsafe fn reacquire(
        lock: &RawSpinlock,
        entry: *mut LockEntryStorage,
        state: &mut Self::GuardState,
    ) {
        *state = unsafe { Self::acquire(lock, entry, ()) };
    }

    #[inline]
    unsafe fn release(lock: &RawSpinlock, entry: *mut LockEntryStorage, _state: Self::GuardState) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe {
            cpp_spinlock_release_no_irqrestore(lock.as_mut_ptr(), entry as *mut c_void);
        }
    }
}

/// Saved interrupt state and source tag for a held `RawMonitoredSpinlock`.
#[derive(Copy, Clone, Default)]
pub struct MonitoredSpinlockGuardState {
    pub interrupt_state: InterruptSavedState,
    pub tag: crate::SourceTag,
}

impl crate::LockPolicy<RawMonitoredSpinlock> for IrqSavePolicy {
    type AcquireArgs = crate::SourceTag;
    type GuardState = MonitoredSpinlockGuardState;

    #[inline]
    unsafe fn acquire(
        lock: &RawMonitoredSpinlock,
        entry: *mut LockEntryStorage,
        tag: Self::AcquireArgs,
    ) -> Self::GuardState {
        // SAFETY: The FFI call is safe because the lock is initialized, `entry` points to valid
        // storage for a lockdep entry, and `tag` points to a static null-terminated C string.
        let interrupt_state = unsafe {
            cpp_monitored_spinlock_acquire_irqsave(
                lock.as_mut_ptr(),
                entry as *mut c_void,
                tag.as_ptr(),
            )
        };
        MonitoredSpinlockGuardState { interrupt_state, tag }
    }

    #[inline]
    unsafe fn reacquire(
        lock: &RawMonitoredSpinlock,
        entry: *mut LockEntryStorage,
        state: &mut Self::GuardState,
    ) {
        // SAFETY: Same safety requirements as `acquire`.
        state.interrupt_state = unsafe {
            cpp_monitored_spinlock_acquire_irqsave(
                lock.as_mut_ptr(),
                entry as *mut c_void,
                state.tag.as_ptr(),
            )
        };
    }

    #[inline]
    unsafe fn release(
        lock: &RawMonitoredSpinlock,
        entry: *mut LockEntryStorage,
        state: Self::GuardState,
    ) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe {
            cpp_monitored_spinlock_release_irqrestore(
                lock.as_mut_ptr(),
                entry as *mut c_void,
                state.interrupt_state,
            );
        }
    }
}

impl crate::LockPolicy<RawMonitoredSpinlock> for NoIrqSavePolicy {
    type AcquireArgs = crate::SourceTag;
    type GuardState = crate::SourceTag;

    #[inline]
    unsafe fn acquire(
        lock: &RawMonitoredSpinlock,
        entry: *mut LockEntryStorage,
        tag: Self::AcquireArgs,
    ) -> Self::GuardState {
        // SAFETY: The FFI call is safe because the lock is initialized, `entry` points to valid
        // storage for a lockdep entry, and `tag` points to a static null-terminated C string.
        unsafe {
            cpp_monitored_spinlock_acquire_no_irqsave(
                lock.as_mut_ptr(),
                entry as *mut c_void,
                tag.as_ptr(),
            );
        }
        tag
    }

    #[inline]
    unsafe fn reacquire(
        lock: &RawMonitoredSpinlock,
        entry: *mut LockEntryStorage,
        state: &mut Self::GuardState,
    ) {
        // SAFETY: Same safety requirements as `acquire`.
        unsafe {
            cpp_monitored_spinlock_acquire_no_irqsave(
                lock.as_mut_ptr(),
                entry as *mut c_void,
                state.as_ptr(),
            );
        }
    }

    #[inline]
    unsafe fn release(
        lock: &RawMonitoredSpinlock,
        entry: *mut LockEntryStorage,
        _state: Self::GuardState,
    ) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe {
            cpp_monitored_spinlock_release_no_irqrestore(lock.as_mut_ptr(), entry as *mut c_void);
        }
    }
}

impl crate::RawLock for RawSpinlock {
    const LOCK_FLAGS: lockdep::LockFlags = lockdep::LOCK_FLAGS_IRQ_SAFE;

    type LockEntry = LockEntryStorage;
    type DefaultPolicy = IrqSavePolicy;

    #[inline]
    unsafe fn init(class_id: *const c_void) -> impl PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(cpp_spinlock_init, class_id)
    }

    #[inline]
    fn as_mut_ptr(&self) -> *mut c_void {
        self as *const Self as *mut Self as *mut c_void
    }
}

impl crate::RawLock for RawMonitoredSpinlock {
    const LOCK_FLAGS: lockdep::LockFlags = lockdep::LOCK_FLAGS_IRQ_SAFE;

    type LockEntry = LockEntryStorage;
    type DefaultPolicy = IrqSavePolicy;

    #[inline]
    unsafe fn init(class_id: *const c_void) -> impl PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(cpp_monitored_spinlock_init, class_id)
    }

    #[inline]
    fn as_mut_ptr(&self) -> *mut c_void {
        self as *const Self as *mut Self as *mut c_void
    }
}

const _: () = {
    #[cfg(feature = "lock_dep")]
    const BASE_SIZE: usize = 8;
    #[cfg(not(feature = "lock_dep"))]
    const BASE_SIZE: usize = 0;

    const EXPECTED_SPINLOCK_SIZE: usize = BASE_SIZE + if RAW_SPINLOCK_SIZE == 4 { 8 } else { 16 };

    assert!(core::mem::size_of::<RawSpinlock>() == EXPECTED_SPINLOCK_SIZE);
    assert!(core::mem::align_of::<RawSpinlock>() == 8);
    assert!(core::mem::size_of::<RawMonitoredSpinlock>() == EXPECTED_SPINLOCK_SIZE);
    assert!(core::mem::align_of::<RawMonitoredSpinlock>() == 8);
};
