// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::raw_lock::RawLock;
use core::ffi::c_void;
use pin_init::{PinInit, pin_data, pin_init_from_closure};

#[cfg(feature = "lock_name_tracing")]
const RAW_MUTEX_SIZE: usize = 32;
#[cfg(not(feature = "lock_name_tracing"))]
const RAW_MUTEX_SIZE: usize = 24;

unsafe extern "C" {
    fn cpp_mutex_destroy(mutex: *mut c_void);
    fn cpp_mutex_acquire(lock: *mut c_void, entry_storage: *mut c_void);
    fn cpp_mutex_release(lock: *mut c_void, entry_storage: *mut c_void);

    fn cpp_critical_mutex_destroy(mutex: *mut c_void);
    fn cpp_critical_mutex_acquire(lock: *mut c_void, entry_storage: *mut c_void) -> bool;
    fn cpp_critical_mutex_release(
        lock: *mut c_void,
        entry_storage: *mut c_void,
        should_clear: bool,
    );
}

const MUTEX_MAGIC: u32 = 0x6D757478; // 'mutx'
const INVALID_CPU: u32 = u32::MAX;

const fn make_mutex_storage() -> [u8; RAW_MUTEX_SIZE] {
    let mut bytes = [0u8; RAW_MUTEX_SIZE];
    #[cfg(feature = "lock_name_tracing")]
    let offset = 8;
    #[cfg(not(feature = "lock_name_tracing"))]
    let offset = 0;

    let magic_bytes = MUTEX_MAGIC.to_ne_bytes();
    let cpu_bytes = INVALID_CPU.to_ne_bytes();

    bytes[offset] = magic_bytes[0];
    bytes[offset + 1] = magic_bytes[1];
    bytes[offset + 2] = magic_bytes[2];
    bytes[offset + 3] = magic_bytes[3];

    bytes[offset + 4] = cpu_bytes[0];
    bytes[offset + 5] = cpu_bytes[1];
    bytes[offset + 6] = cpu_bytes[2];
    bytes[offset + 7] = cpu_bytes[3];

    bytes
}

#[repr(C, align(8))]
struct RawMutexStorage(zr::OpaqueBytes<RAW_MUTEX_SIZE>);

#[derive(Default)]
#[repr(C, align(8))]
pub struct LockEntryStorage(zr::OpaqueBytes<40>);

/// Opaque layout block matching the Zircon C++ Mutex exactly.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct RawMutex {
    #[cfg(feature = "lock_dep")]
    class_id: *const c_void,
    storage: RawMutexStorage,
}

impl RawMutex {
    pub const INIT: Self = Self::const_init(core::ptr::null());

    /// Statically initializes a RawMutex in constant context.
    pub const fn const_init(_class_id: *const c_void) -> Self {
        Self {
            #[cfg(feature = "lock_dep")]
            class_id: _class_id,
            storage: RawMutexStorage(zr::OpaqueBytes::new(make_mutex_storage())),
        }
    }

    /// Returns a slice over the raw mutex storage bytes.
    #[cfg(any(test, ktest))]
    #[inline]
    pub fn raw_storage_slice(&self) -> &[u8] {
        // SAFETY: The storage is valid and allocated with RAW_MUTEX_SIZE bytes.
        unsafe { core::slice::from_raw_parts(self.storage.0.get() as *const u8, RAW_MUTEX_SIZE) }
    }
}

impl Default for RawMutex {
    fn default() -> Self {
        Self::const_init(core::ptr::null())
    }
}

// SAFETY: RawMutex is safe to share and access across threads.
unsafe impl Sync for RawMutex {}
unsafe impl Send for RawMutex {}

zr::unsafe_pinned_drop_ffi!(RawMutex, cpp_mutex_destroy);

pub struct RawMutexPolicy;

impl crate::LockPolicy<RawMutex> for RawMutexPolicy {
    type AcquireArgs = ();
    type GuardState = ();

    #[inline]
    unsafe fn acquire(
        lock: &RawMutex,
        entry: *mut LockEntryStorage,
        _args: (),
    ) -> Self::GuardState {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe {
            cpp_mutex_acquire(lock.as_mut_ptr(), entry as *mut c_void);
        }
    }

    #[inline]
    unsafe fn reacquire(
        lock: &RawMutex,
        entry: *mut LockEntryStorage,
        state: &mut Self::GuardState,
    ) {
        *state = unsafe { Self::acquire(lock, entry, ()) };
    }

    #[inline]
    unsafe fn release(lock: &RawMutex, entry: *mut LockEntryStorage, _state: Self::GuardState) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe {
            cpp_mutex_release(lock.as_mut_ptr(), entry as *mut c_void);
        }
    }
}

impl crate::RawLock for RawMutex {
    type LockEntry = LockEntryStorage;
    type DefaultPolicy = RawMutexPolicy;

    #[inline]
    unsafe fn init(class_id: *const c_void) -> impl PinInit<Self, core::convert::Infallible> {
        // SAFETY: The closure initializes the provided slot in place before returning Ok(()).
        unsafe {
            pin_init_from_closure(move |slot: *mut Self| {
                slot.write(Self::const_init(class_id));
                Ok(())
            })
        }
    }

    #[inline]
    fn as_mut_ptr(&self) -> *mut c_void {
        self as *const Self as *mut Self as *mut c_void
    }
}

/// Opaque layout block matching the Zircon C++ CriticalMutex exactly.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct RawCriticalMutex {
    #[cfg(feature = "lock_dep")]
    class_id: *const c_void,
    storage: RawMutexStorage,
}

impl RawCriticalMutex {
    pub const INIT: Self = Self::const_init(core::ptr::null());

    /// Statically initializes a RawCriticalMutex in constant context.
    pub const fn const_init(_class_id: *const c_void) -> Self {
        Self {
            #[cfg(feature = "lock_dep")]
            class_id: _class_id,
            storage: RawMutexStorage(zr::OpaqueBytes::new(make_mutex_storage())),
        }
    }

    /// Returns a slice over the raw critical mutex storage bytes.
    #[cfg(any(test, ktest))]
    #[inline]
    pub fn raw_storage_slice(&self) -> &[u8] {
        // SAFETY: The storage is valid and allocated with RAW_MUTEX_SIZE bytes.
        unsafe { core::slice::from_raw_parts(self.storage.0.get() as *const u8, RAW_MUTEX_SIZE) }
    }
}

impl Default for RawCriticalMutex {
    fn default() -> Self {
        Self::const_init(core::ptr::null())
    }
}

// SAFETY: RawCriticalMutex is safe to share and access across threads.
unsafe impl Sync for RawCriticalMutex {}
unsafe impl Send for RawCriticalMutex {}

zr::unsafe_pinned_drop_ffi!(RawCriticalMutex, cpp_critical_mutex_destroy);

pub struct RawCriticalMutexPolicy;

impl crate::LockPolicy<RawCriticalMutex> for RawCriticalMutexPolicy {
    type AcquireArgs = ();
    type GuardState = bool;

    #[inline]
    unsafe fn acquire(
        lock: &RawCriticalMutex,
        entry: *mut LockEntryStorage,
        _args: (),
    ) -> Self::GuardState {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe { cpp_critical_mutex_acquire(lock.as_mut_ptr(), entry as *mut c_void) }
    }

    #[inline]
    unsafe fn reacquire(
        lock: &RawCriticalMutex,
        entry: *mut LockEntryStorage,
        state: &mut Self::GuardState,
    ) {
        *state = unsafe { Self::acquire(lock, entry, ()) };
    }

    #[inline]
    unsafe fn release(
        lock: &RawCriticalMutex,
        entry: *mut LockEntryStorage,
        should_clear: Self::GuardState,
    ) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` points to valid storage for a lockdep entry.
        unsafe {
            cpp_critical_mutex_release(lock.as_mut_ptr(), entry as *mut c_void, should_clear);
        }
    }
}

impl crate::RawLock for RawCriticalMutex {
    type LockEntry = LockEntryStorage;
    type DefaultPolicy = RawCriticalMutexPolicy;

    #[inline]
    unsafe fn init(class_id: *const c_void) -> impl PinInit<Self, core::convert::Infallible> {
        // SAFETY: The closure initializes the provided slot in place before returning Ok(()).
        unsafe {
            pin_init_from_closure(move |slot: *mut Self| {
                slot.write(Self::const_init(class_id));
                Ok(())
            })
        }
    }

    #[inline]
    fn as_mut_ptr(&self) -> *mut c_void {
        self as *const Self as *mut Self as *mut c_void
    }
}

const _: () = {
    #[cfg(feature = "lock_dep")]
    const EXPECTED_SIZE: usize = if cfg!(feature = "lock_name_tracing") { 40 } else { 32 };
    #[cfg(not(feature = "lock_dep"))]
    const EXPECTED_SIZE: usize = if cfg!(feature = "lock_name_tracing") { 32 } else { 24 };

    assert!(core::mem::size_of::<RawMutex>() == EXPECTED_SIZE);
    assert!(core::mem::align_of::<RawMutex>() == 8);

    assert!(core::mem::size_of::<RawCriticalMutex>() == EXPECTED_SIZE);
    assert!(core::mem::align_of::<RawCriticalMutex>() == 8);
};
