// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Synchronization primitives for Netstack3.

#![warn(missing_docs, unreachable_patterns, unused)]

extern crate alloc;

#[cfg(loom)]
pub(crate) use loom::sync;
#[cfg(not(loom))]
pub(crate) use std::sync;

use net_types::ip::{GenericOverIp, Ip};

// Re-export atomics module for loom compatibility in appropriate targets.
pub use sync::atomic;

pub mod rc;

/// A [`sync::Mutex`] assuming lock poisoning will never occur.
#[derive(Debug, Default)]
pub struct Mutex<T>(sync::Mutex<T>);

/// Lock guard for access to a [`Mutex`].
pub type LockGuard<'a, T> = lock_guard::LockGuard<'a, Mutex<T>, sync::MutexGuard<'a, T>>;

impl<T> Mutex<T> {
    /// Creates a new mutex in an unlocked state ready for use.
    pub fn new(t: T) -> Mutex<T> {
        Mutex(sync::Mutex::new(t))
    }

    /// Acquires a mutex, blocking the current thread until it is able to do so.
    ///
    /// See [`sync::Mutex::lock`] for more details.
    ///
    /// # Panics
    ///
    /// This method may panic if the calling thread is already holding the
    /// lock.
    #[inline]
    #[cfg_attr(feature = "recursive-lock-panic", track_caller)]
    pub fn lock(&self) -> LockGuard<'_, T> {
        lock_guard::LockGuard::new(self, |Self(m)| m.lock().expect("unexpectedly poisoned"))
    }

    /// Consumes this mutex, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        let Self(mutex) = self;
        mutex.into_inner().expect("unexpectedly poisoned")
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// Since this call borrows the [`Mutex`] mutably, no actual locking needs
    /// to take place. See [`sync::Mutex::get_mut`] for more details.
    #[inline]
    // TODO(https://github.com/tokio-rs/loom/pull/322): remove the disable for
    // loom once loom's lock type supports the method.
    #[cfg(not(loom))]
    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut().expect("unexpectedly poisoned")
    }
}

impl<T: 'static> lock_order::lock::ExclusiveLock<T> for Mutex<T> {
    type Guard<'l> = LockGuard<'l, T>;

    fn lock(&self) -> Self::Guard<'_> {
        self.lock()
    }
}

impl<T, I: Ip> GenericOverIp<I> for Mutex<T>
where
    T: GenericOverIp<I>,
{
    type Type = Mutex<T::Type>;
}

/// A [`sync::RwLock`] assuming lock poisoning will never occur.
#[derive(Debug, Default)]
pub struct RwLock<T>(sync::RwLock<T>);

/// Lock guard for read access to a [`RwLock`].
pub type RwLockReadGuard<'a, T> =
    lock_guard::LockGuard<'a, RwLock<T>, sync::RwLockReadGuard<'a, T>>;

/// Lock guard for write access to a [`RwLock`].
pub type RwLockWriteGuard<'a, T> =
    lock_guard::LockGuard<'a, RwLock<T>, sync::RwLockWriteGuard<'a, T>>;

impl<T> RwLock<T> {
    /// Creates a new instance of an `RwLock<T>` which is unlocked.
    pub fn new(t: T) -> RwLock<T> {
        RwLock(sync::RwLock::new(t))
    }

    /// Locks this rwlock with shared read access, blocking the current thread
    /// until it can be acquired.
    ///
    /// See [`sync::RwLock::read`] for more details.
    ///
    /// # Panics
    ///
    /// This method may panic if the calling thread already holds the read or
    /// write lock.
    #[inline]
    #[cfg_attr(feature = "recursive-lock-panic", track_caller)]
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        lock_guard::LockGuard::new(self, |Self(rw)| rw.read().expect("unexpectedly poisoned"))
    }

    /// Locks this rwlock with exclusive write access, blocking the current
    /// thread until it can be acquired.
    ///
    /// See [`sync::RwLock::write`] for more details.
    ///
    /// # Panics
    ///
    /// This method may panic if the calling thread already holds the read or
    /// write lock.
    #[inline]
    #[cfg_attr(feature = "recursive-lock-panic", track_caller)]
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        lock_guard::LockGuard::new(self, |Self(rw)| rw.write().expect("unexpectedly poisoned"))
    }

    /// Consumes this rwlock, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        let Self(rwlock) = self;
        rwlock.into_inner().expect("unexpectedly poisoned")
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// Since this call borrows the [`RwLock`] mutably, no actual locking needs
    /// to take place. See [`sync::RwLock::get_mut`] for more details.
    #[inline]
    // TODO(https://github.com/tokio-rs/loom/pull/322): remove the disable for
    // loom once loom's lock type supports the method.
    #[cfg(not(loom))]
    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut().expect("unexpectedly poisoned")
    }
}

impl<T: 'static> lock_order::lock::ReadWriteLock<T> for RwLock<T> {
    type ReadGuard<'l> = RwLockReadGuard<'l, T>;

    type WriteGuard<'l> = RwLockWriteGuard<'l, T>;

    fn read_lock(&self) -> Self::ReadGuard<'_> {
        self.read()
    }

    fn write_lock(&self) -> Self::WriteGuard<'_> {
        self.write()
    }
}

impl<T, I: Ip> GenericOverIp<I> for RwLock<T>
where
    T: GenericOverIp<I>,
{
    type Type = RwLock<T::Type>;
}

mod lock_guard {
    #[cfg(not(feature = "recursive-lock-panic"))]
    use core::marker::PhantomData;
    use core::ops::{Deref, DerefMut};

    #[cfg(feature = "recursive-lock-panic")]
    use crate::lock_tracker::LockTracker;

    /// An RAII implementation used to release a lock when dropped.
    ///
    /// Wraps inner guard to provide lock instrumentation (when the appropriate
    /// feature is enabled).
    pub struct LockGuard<'a, L, G> {
        guard: G,

        // Placed after `guard` so that the tracker's destructor is run (and the
        // unlock is tracked) after the lock is actually unlocked.
        #[cfg(feature = "recursive-lock-panic")]
        _lock_tracker: LockTracker<'a, L>,
        #[cfg(not(feature = "recursive-lock-panic"))]
        _marker: PhantomData<&'a L>,
    }

    impl<'a, L, G> LockGuard<'a, L, G> {
        /// Returns a new lock guard.
        #[cfg_attr(feature = "recursive-lock-panic", track_caller)]
        pub fn new<F: FnOnce(&'a L) -> G>(lock: &'a L, lock_fn: F) -> Self {
            #[cfg(feature = "recursive-lock-panic")]
            let lock_tracker = LockTracker::new(lock);

            Self {
                guard: lock_fn(lock),

                #[cfg(feature = "recursive-lock-panic")]
                _lock_tracker: lock_tracker,
                #[cfg(not(feature = "recursive-lock-panic"))]
                _marker: PhantomData,
            }
        }
    }

    impl<L, G: Deref> Deref for LockGuard<'_, L, G> {
        type Target = G::Target;

        fn deref(&self) -> &G::Target {
            self.guard.deref()
        }
    }

    impl<L, G: DerefMut> DerefMut for LockGuard<'_, L, G> {
        fn deref_mut(&mut self) -> &mut G::Target {
            self.guard.deref_mut()
        }
    }
}

#[cfg(feature = "recursive-lock-panic")]
mod lock_tracker {
    use core::cell::RefCell;
    use core::panic::Location;
    use std::collections::HashMap;

    std::thread_local! {
        static HELD_LOCKS: RefCell<HashMap<*const usize, &'static Location<'static>>> =
            RefCell::new(HashMap::new());
    }

    /// An RAII object to keep track of a lock that is (or soon to be) held.
    ///
    /// The `Drop` implementation of this struct removes the lock from the
    /// thread-local table of held locks.
    pub(crate) struct LockTracker<'a, L>(&'a L);

    impl<'a, L> LockTracker<'a, L> {
        /// Tracks that the lock is to be held.
        ///
        /// This method adds the lock to the thread-local table of held locks.
        ///
        /// # Panics
        ///
        /// Panics if the lock is already held by the calling thread.
        #[track_caller]
        pub(crate) fn new(lock: &'a L) -> Self {
            {
                let ptr = lock as *const _ as *const _;
                match HELD_LOCKS.with(|l| l.borrow_mut().insert(ptr, Location::caller())) {
                    None => {}
                    Some(prev_lock_caller) => {
                        panic!("lock already held; ptr = {:p}\n{}", ptr, prev_lock_caller)
                    }
                }
            }

            Self(lock)
        }
    }

    impl<L> Drop for LockTracker<'_, L> {
        fn drop(&mut self) {
            let Self(lock) = self;
            let ptr = *lock as *const _ as *const _;
            assert_ne!(
                HELD_LOCKS.with(|l| l.borrow_mut().remove(&ptr)),
                None,
                "must have previously been locked; ptr = {:p}",
                ptr
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::thread;

    #[test]
    fn mutex_lock_and_write() {
        let m = Mutex::<u32>::new(0);
        {
            let mut guard = m.lock();
            assert_eq!(*guard, 0);
            *guard = 5;
        }

        {
            let guard = m.lock();
            assert_eq!(*guard, 5);
        }
    }

    #[test]
    fn mutex_lock_from_different_threads() {
        const NUM_THREADS: u32 = 4;

        let m = Mutex::<u32>::new(u32::MAX);
        let m = &m;

        thread::scope(|s| {
            for i in 0..NUM_THREADS {
                let _: thread::ScopedJoinHandle<'_, _> = s.spawn(move || {
                    let prev = {
                        let mut guard = m.lock();
                        let prev = *guard;
                        *guard = i;
                        prev
                    };

                    assert!(prev == u32::MAX || prev < NUM_THREADS);
                });
            }
        });

        let guard = m.lock();
        assert!(*guard < NUM_THREADS);
    }

    #[test]
    #[should_panic(expected = "lock already held")]
    #[cfg(feature = "recursive-lock-panic")]
    fn mutex_double_lock_panic() {
        let m = Mutex::<u32>::new(0);
        let _ok_guard = m.lock();
        let _panic_guard = m.lock();
    }

    #[test]
    fn rwlock_read_lock() {
        let rw = RwLock::<u32>::new(0);

        {
            let guard = rw.read();
            assert_eq!(*guard, 0);
        }

        {
            let guard = rw.read();
            assert_eq!(*guard, 0);
        }
    }

    #[test]
    fn rwlock_write_lock() {
        let rw = RwLock::<u32>::new(0);
        {
            let mut guard = rw.write();
            assert_eq!(*guard, 0);
            *guard = 5;
        }

        {
            let guard = rw.write();
            assert_eq!(*guard, 5);
        }
    }

    #[test]
    fn rwlock_read_and_write_from_different_threads() {
        const NUM_THREADS: u32 = 4;

        let rw = RwLock::<u32>::new(u32::MAX);
        let rw = &rw;

        thread::scope(|s| {
            for i in 0..NUM_THREADS {
                let _: thread::ScopedJoinHandle<'_, _> = s.spawn(move || {
                    let prev = if i % 2 == 0 {
                        // Only threads with even numbered `i` performs a write.
                        let mut guard = rw.write();
                        let prev = *guard;
                        *guard = i;
                        prev
                    } else {
                        let guard = rw.read();
                        *guard
                    };

                    assert!(prev == u32::MAX || (prev < NUM_THREADS && prev % 2 == 0));
                });
            }
        });

        let val = *rw.read();
        assert!(val < NUM_THREADS && val % 2 == 0);
    }

    #[test]
    #[cfg_attr(feature = "recursive-lock-panic", should_panic(expected = "lock already held"))]
    fn mutex_double_read() {
        let rw = RwLock::<u32>::new(0);
        let ok_guard = rw.read();
        assert_eq!(*ok_guard, 0);
        let maybe_panic_guard = rw.read();
        assert_eq!(*maybe_panic_guard, 0);
    }

    #[test]
    #[should_panic(expected = "lock already held")]
    #[cfg(feature = "recursive-lock-panic")]
    fn mutex_double_write_panic() {
        let rw = RwLock::<u32>::new(0);
        let _ok_guard = rw.write();
        let _panic_guard = rw.write();
    }

    #[test]
    #[should_panic(expected = "lock already held")]
    #[cfg(feature = "recursive-lock-panic")]
    fn mutex_double_read_then_write_panic() {
        let rw = RwLock::<u32>::new(0);
        let _ok_guard = rw.read();
        let _panic_guard = rw.write();
    }

    #[test]
    #[should_panic(expected = "lock already held")]
    #[cfg(feature = "recursive-lock-panic")]
    fn mutex_double_write_then_read_panic() {
        let rw = RwLock::<u32>::new(0);
        let _ok_guard = rw.read();
        let _panic_guard = rw.write();
    }
}
