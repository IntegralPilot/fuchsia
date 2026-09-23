// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia-native synchronization primitives.

mod condvar;
pub use condvar::*;

#[cfg(target_os = "fuchsia")]
mod mutex;
#[cfg(target_os = "fuchsia")]
mod rwlock;

#[cfg(target_os = "fuchsia")]
pub use mutex::RawSyncMutex as RawMutex;
#[cfg(not(target_os = "fuchsia"))]
pub use parking_lot::RawMutex;

#[cfg(not(target_os = "fuchsia"))]
pub use parking_lot::RawRwLock;
#[cfg(target_os = "fuchsia")]
pub use rwlock::RawSyncRwLock as RawRwLock;

#[cfg(not(detect_lock_cycles))]
type RawMutexImpl = RawMutex;
#[cfg(detect_lock_cycles)]
type RawMutexImpl = tracing_mutex::lockapi::TracingWrapper<RawMutex>;

#[cfg(not(detect_lock_cycles))]
type RawRwLockImpl = RawRwLock;
#[cfg(detect_lock_cycles)]
type RawRwLockImpl = tracing_mutex::lockapi::TracingWrapper<RawRwLock>;

pub type Mutex<T> = lock_api::Mutex<RawMutexImpl, T>;
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutexImpl, T>;
pub type MappedMutexGuard<'a, T> = lock_api::MappedMutexGuard<'a, RawMutexImpl, T>;

pub type RwLock<T> = lock_api::RwLock<RawRwLockImpl, T>;
pub type RwLockReadGuard<'a, T> = lock_api::RwLockReadGuard<'a, RawRwLockImpl, T>;
pub type RwLockWriteGuard<'a, T> = lock_api::RwLockWriteGuard<'a, RawRwLockImpl, T>;
pub type MappedRwLockReadGuard<'a, T> = lock_api::MappedRwLockReadGuard<'a, RawRwLockImpl, T>;
pub type MappedRwLockWriteGuard<'a, T> = lock_api::MappedRwLockWriteGuard<'a, RawRwLockImpl, T>;

/// Prevent potential deadlocks from panicking when lock cycle detection is enabled. This will
/// cause them to print instead of exiting the process.
pub fn suppress_lock_cycle_panics() {
    #[cfg(detect_lock_cycles)]
    tracing_mutex::suppress_panics();
}

/// A trait for locks whose dynamic dependency tracking graph can be reset.
///
/// This should only be called when we need to change a previous lock ordering.
pub trait ResetDependencies {
    /// Resets the lock dependency graph for this lock.
    ///
    /// # Safety
    ///
    /// It is the responsibility of the caller to ensure changing this lock ordering is safe.
    unsafe fn reset_dependencies(&self);
}

impl<T> ResetDependencies for RwLock<T> {
    #[inline(always)]
    unsafe fn reset_dependencies(&self) {
        #[cfg(detect_lock_cycles)]
        // SAFETY: The caller guarantees they are enforcing a sound locking order
        // and that resetting the graph will not mask a real deadlock.
        unsafe {
            tracing_mutex::util::reset_dependencies(lock_api::RwLock::raw(self));
        }
    }
}

impl<T> ResetDependencies for Mutex<T> {
    #[inline(always)]
    unsafe fn reset_dependencies(&self) {
        #[cfg(detect_lock_cycles)]
        // SAFETY: The caller guarantees they are enforcing a sound locking order
        // and that resetting the graph will not mask a real deadlock.
        unsafe {
            tracing_mutex::util::reset_dependencies(lock_api::Mutex::raw(self));
        }
    }
}
