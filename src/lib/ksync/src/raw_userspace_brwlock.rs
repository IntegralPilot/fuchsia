// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use lock_api::RawRwLock as _;
use pin_init::{PinInit, pin_init_from_closure};

/// A userspace implementation of `RawBrwLockPi` backed by `fuchsia_sync::RawRwLock`.
pub struct RawBrwLockPi {
    inner: fuchsia_sync::RawRwLock,
}

// SAFETY: RawBrwLockPi is safe to share across threads.
unsafe impl Sync for RawBrwLockPi {}
unsafe impl Send for RawBrwLockPi {}

impl RawBrwLockPi {
    pub const LOCK_FLAGS: lockdep::LockFlags = lockdep::LOCK_FLAGS_NONE;

    /// Initializes a new `RawBrwLockPi` in-place.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `class_id` is either null or points to a valid,
    /// static `LockClassId` that remains valid for the lifetime of the lock.
    #[inline]
    pub unsafe fn init(
        _class_id: *const core::ffi::c_void,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        // SAFETY: The closure correctly initializes the raw userspace lock state in-place
        // and satisfies all safety requirements of `pin_init_from_closure`.
        unsafe {
            pin_init_from_closure(|slot| {
                core::ptr::write(slot, Self { inner: fuchsia_sync::RawRwLock::INIT });
                Ok(())
            })
        }
    }

    /// Acquires the lock in shared (read) mode.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the lock is pinned and initialized.
    #[inline]
    pub unsafe fn acquire_read(&self, _entry: *mut core::ffi::c_void) {
        self.inner.lock_shared();
    }

    /// Releases the lock from shared (read) mode.
    ///
    /// # Safety
    ///
    /// The caller must currently hold the read lock on this instance.
    #[inline]
    pub unsafe fn release_read(&self, _entry: *mut core::ffi::c_void) {
        // SAFETY: The caller guarantees they hold the read lock on this instance.
        unsafe { self.inner.unlock_shared() };
    }

    /// Acquires the lock in exclusive (write) mode.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the lock is pinned and initialized.
    #[inline]
    pub unsafe fn acquire_write(&self, _entry: *mut core::ffi::c_void) {
        self.inner.lock_exclusive();
    }

    /// Releases the lock from exclusive (write) mode.
    ///
    /// # Safety
    ///
    /// The caller must currently hold the write lock on this instance.
    #[inline]
    pub unsafe fn release_write(&self, _entry: *mut core::ffi::c_void) {
        // SAFETY: The caller guarantees they hold the write lock on this instance.
        unsafe { self.inner.unlock_exclusive() };
    }
}
