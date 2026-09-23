// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mutex::raw::RawMutex;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
pub mod raw;

/// A mutual exclusion foundation protecting shared data of type `T` using a `RawMutex`.
///
/// # Examples
///
/// Basic usage using `SpinMutex`:
///
/// ```
/// use sapphire_sync::mutex::Mutex;
/// use sapphire_sync::mutex::raw::SpinMutex;
///
/// let mtx = Mutex::<SpinMutex, i32>::new(0);
/// std::thread::scope(|s| {
///     for _ in 0..10 {
///         s.spawn(|| {
///             for _ in 0..100 {
///                 let mut guard = mtx.lock();
///                 *guard += 1;
///             }
///         });
///     }
/// });
/// assert_eq!(mtx.into_inner(), 1000);
/// ```
///
/// A `Mutex` using `SingleThreadMutex` can be safely sent across threads:
///
/// ```
/// use sapphire_sync::mutex::Mutex;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
/// use std::thread;
///
/// let mtx = Mutex::<SingleThreadMutex, i32>::new(42);
/// thread::spawn(move || {
///     let _ = mtx;
/// });
/// ```
///
/// but it **cannot** be shared across threads:
///
/// ```compile_fail
/// use sapphire_sync::mutex::Mutex;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// let mtx = Mutex::<SingleThreadMutex, i32>::new(42);
/// thread::spawn(|| {
///     let _guard = mtx.lock();
/// });
/// ```
pub struct Mutex<Kind, T> {
    raw_mutex: Kind,
    payload: UnsafeCell<T>,
}
// SAFETY: `Mutex` guarantees mutual exclusion when uniquely owned, which is sound if T: Send and Kind: Send.
unsafe impl<Kind: RawMutex + Send, T: Send> Send for Mutex<Kind, T> {}
// SAFETY: Similar to std::sync::Mutex but we require the RawMutex to also be Sync
unsafe impl<Kind: RawMutex + Sync, T: Send> Sync for Mutex<Kind, T> {}

impl<Kind: RawMutex, T> Mutex<Kind, T> {
    /// Creates a new mutex protecting the given data.
    pub fn new(data: T) -> Self {
        Self { raw_mutex: Kind::default(), payload: UnsafeCell::new(data) }
    }
    /// Consumes the mutex, returning the underlying protected data.
    pub fn into_inner(self) -> T {
        self.payload.into_inner()
    }
    /// Returns a mutable reference to the protected data without locking.
    pub fn get_mut(&mut self) -> &mut T {
        self.payload.get_mut()
    }
    /// Acquires the mutex, blocking until the lock becomes available.
    pub fn lock(&self) -> MutexGuard<'_, Kind, T> {
        MutexGuard::new(self)
    }
}
/// An RAII guard representing exclusive access to the data protected by a `Mutex`.
pub struct MutexGuard<'a, Kind: RawMutex, T> {
    mutex: &'a Mutex<Kind, T>,
}
impl<'a, Kind: RawMutex, T> MutexGuard<'a, Kind, T> {
    /// Acquires the lock and constructs a new guard.
    pub fn new(mutex: &'a Mutex<Kind, T>) -> Self {
        mutex.raw_mutex.lock();
        Self { mutex }
    }

    /// Returns a reference to the underlying mutex.
    pub fn mutex(&self) -> &'a Mutex<Kind, T> {
        self.mutex
    }
}
impl<Kind, T> Deref for MutexGuard<'_, Kind, T>
where
    Kind: RawMutex,
{
    type Target = T;
    fn deref(&self) -> &Self::Target {
        // SAFETY: The guard holds the lock, guaranteeing exclusive access to the payload.
        unsafe { &*self.mutex.payload.get() }
    }
}
impl<Kind, T> DerefMut for MutexGuard<'_, Kind, T>
where
    Kind: RawMutex,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The guard holds the lock, guaranteeing exclusive mutable access to the payload.
        unsafe { &mut *self.mutex.payload.get() }
    }
}
impl<Kind, T> Drop for MutexGuard<'_, Kind, T>
where
    Kind: RawMutex,
{
    fn drop(&mut self) {
        // SAFETY: The guard was successfully constructed and holds the lock, so unlocking is valid.
        unsafe {
            self.mutex.raw_mutex.unlock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutex::raw::{SingleThreadMutex, SpinMutex};
    use std::thread;

    #[test]
    fn test_single_thread_mutex_basic() {
        let mtx = Mutex::<SingleThreadMutex, i32>::new(42);
        {
            let mut guard = mtx.lock();
            assert_eq!(*guard, 42);
            *guard += 10;
        }
        assert_eq!(mtx.into_inner(), 52);
    }

    #[test]
    #[should_panic(expected = "Attempting to lock single-thread mutex twice is a deadlock")]
    fn test_single_thread_mutex_reentrancy_panic() {
        let mtx = Mutex::<SingleThreadMutex, i32>::new(42);
        let _guard1 = mtx.lock();
        let _guard2 = mtx.lock(); // Must panic
    }

    #[test]
    fn test_spin_mutex_basic() {
        let mtx = Mutex::<SpinMutex, i32>::new(100);
        {
            let mut guard = mtx.lock();
            assert_eq!(*guard, 100);
            *guard -= 50;
        }
        assert_eq!(mtx.into_inner(), 50);
    }

    #[test]
    fn test_spin_mutex_multithreaded() {
        let mtx = Mutex::<SpinMutex, i32>::new(0);

        thread::scope(|s| {
            for _ in 0..10 {
                s.spawn(|| {
                    for _ in 0..100 {
                        let mut guard = mtx.lock();
                        *guard += 1;
                    }
                });
            }
        });

        assert_eq!(mtx.into_inner(), 1000);
    }
}
