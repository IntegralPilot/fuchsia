// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ffi::c_void;
use core::pin::Pin;
use pin_init::{PinInit, pin_data, pinned_drop};

unsafe extern "C" {
    fn cpp_brwlock_pi_init(lock: *mut c_void, class_id: *const c_void);
    fn cpp_brwlock_pi_destroy(lock: *mut c_void);
    fn cpp_brwlock_pi_acquire_read(lock: *mut c_void, entry_storage: *mut c_void);
    fn cpp_brwlock_pi_release_read(lock: *mut c_void, entry_storage: *mut c_void);
    fn cpp_brwlock_pi_acquire_write(lock: *mut c_void, entry_storage: *mut c_void);
    fn cpp_brwlock_pi_release_write(lock: *mut c_void, entry_storage: *mut c_void);
}

#[cfg(not(target_arch = "riscv64"))]
#[repr(C, align(16))]
struct RawBrwLockPiStorage(zr::OpaqueBytes<128>);

#[cfg(target_arch = "riscv64")]
#[repr(C, align(8))]
struct RawBrwLockPiStorage(zr::OpaqueBytes<72>);

/// Opaque layout block matching the Zircon C++ BrwLockPi exactly.
#[cfg(not(target_arch = "riscv64"))]
#[pin_data(PinnedDrop)]
#[repr(C, align(16))]
pub struct RawBrwLockPi {
    #[cfg(feature = "lock_dep")]
    class_id: *const c_void,
    storage: RawBrwLockPiStorage,
}

#[cfg(target_arch = "riscv64")]
#[pin_data(PinnedDrop)]
#[repr(C, align(8))]
pub struct RawBrwLockPi {
    #[cfg(feature = "lock_dep")]
    class_id: *const c_void,
    storage: RawBrwLockPiStorage,
}

// SAFETY: RawBrwLockPi is safe to share and access across threads because it delegates
// to the thread-safe Zircon kernel BrwLockPi implementation.
unsafe impl Sync for RawBrwLockPi {}
unsafe impl Send for RawBrwLockPi {}

#[pinned_drop]
impl PinnedDrop for RawBrwLockPi {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: `me.as_mut_ptr()` returns a valid pointer to the pinned C++ object,
        // and `cpp_brwlock_pi_destroy` is safe to call on a valid initialized object
        // before its memory is reclaimed.
        unsafe {
            let me = self.get_unchecked_mut();
            cpp_brwlock_pi_destroy(me.as_mut_ptr());
        }
    }
}

impl RawBrwLockPi {
    pub const LOCK_FLAGS: lockdep::LockFlags = lockdep::LOCK_FLAGS_NONE;

    /// Initializes a new `RawBrwLockPi` in-place.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `class_id` is either null or points to a valid,
    /// static `LockClassId` that remains valid for the lifetime of the lock.
    #[inline]
    pub unsafe fn init(class_id: *const c_void) -> impl PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(cpp_brwlock_pi_init, class_id)
    }

    /// Returns a raw pointer to the underlying storage.
    #[inline]
    pub fn as_mut_ptr(&self) -> *mut c_void {
        self as *const Self as *mut Self as *mut c_void
    }

    /// Acquires the lock in shared (read) mode.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the lock is pinned and initialized. If `lock_dep` is enabled,
    /// `entry` must point to valid storage for a lockdep entry.
    #[inline]
    pub unsafe fn acquire_read(&self, entry: *mut c_void) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` (if applicable) are valid.
        unsafe {
            cpp_brwlock_pi_acquire_read(self.as_mut_ptr(), entry);
        }
    }

    /// Releases the lock from shared (read) mode.
    ///
    /// # Safety
    ///
    /// The caller must currently hold the read lock on this instance. If `lock_dep` is enabled,
    /// `entry` must point to the same valid storage used during acquisition.
    #[inline]
    pub unsafe fn release_read(&self, entry: *mut c_void) {
        // SAFETY: The FFI call is safe because the lock is initialized and the caller currently
        // holds the read lock.
        unsafe {
            cpp_brwlock_pi_release_read(self.as_mut_ptr(), entry);
        }
    }

    /// Acquires the lock in exclusive (write) mode.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the lock is pinned and initialized. If `lock_dep` is enabled,
    /// `entry` must point to valid storage for a lockdep entry.
    #[inline]
    pub unsafe fn acquire_write(&self, entry: *mut c_void) {
        // SAFETY: The FFI call is safe because the lock is initialized, and the caller guarantees
        // that `entry` (if applicable) are valid.
        unsafe {
            cpp_brwlock_pi_acquire_write(self.as_mut_ptr(), entry);
        }
    }

    /// Releases the lock from exclusive (write) mode.
    ///
    /// # Safety
    ///
    /// The caller must currently hold the write lock on this instance. If `lock_dep` is enabled,
    /// `entry` must point to the same valid storage used during acquisition.
    #[inline]
    pub unsafe fn release_write(&self, entry: *mut c_void) {
        // SAFETY: The FFI call is safe because the lock is initialized and the caller currently
        // holds the write lock.
        unsafe {
            cpp_brwlock_pi_release_write(self.as_mut_ptr(), entry);
        }
    }
}

#[cfg(all(not(target_arch = "riscv64"), not(feature = "lock_dep")))]
const _: () = {
    assert!(core::mem::size_of::<RawBrwLockPi>() == 128);
    assert!(core::mem::align_of::<RawBrwLockPi>() == 16);
};

#[cfg(all(not(target_arch = "riscv64"), feature = "lock_dep"))]
const _: () = {
    assert!(core::mem::size_of::<RawBrwLockPi>() == 144);
    assert!(core::mem::align_of::<RawBrwLockPi>() == 16);
};

#[cfg(all(target_arch = "riscv64", not(feature = "lock_dep")))]
const _: () = {
    assert!(core::mem::size_of::<RawBrwLockPi>() == 72);
    assert!(core::mem::align_of::<RawBrwLockPi>() == 8);
};

#[cfg(all(target_arch = "riscv64", feature = "lock_dep"))]
const _: () = {
    assert!(core::mem::size_of::<RawBrwLockPi>() == 80);
    assert!(core::mem::align_of::<RawBrwLockPi>() == 8);
};
