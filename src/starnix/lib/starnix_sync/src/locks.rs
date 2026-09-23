// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Use these crates so that we don't need to make the dependencies conditional.
use fuchsia_sync as _;
use lock_api as _;

use lock_api::RawMutex;

pub use fuchsia_sync::{
    MappedMutexGuard, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

/// A trait for lock guards that can be temporarily unlocked asynchronously.
/// This is useful for performing async operations while holding a lock, without
/// causing deadlocks or holding the lock for an extended period.
#[async_trait::async_trait(?Send)]
pub trait AsyncUnlockable {
    /// Temporarily unlocks the guard `s`, executes the async function `f`, and then
    /// re-locks the guard.
    /// The lock is guaranteed to be re-acquired before this function returns.
    async fn unlocked_async<F, U>(s: &mut Self, f: F) -> U
    where
        F: AsyncFnOnce() -> U;
}

#[async_trait::async_trait(?Send)]
impl<'a, T> crate::AsyncUnlockable for MutexGuard<'a, T> {
    async fn unlocked_async<F, U>(s: &mut Self, f: F) -> U
    where
        F: AsyncFnOnce() -> U,
    {
        // SAFETY: The guard always have a lock mutex.
        unsafe {
            Self::mutex(s).raw().unlock();
        }
        scopeguard::defer!(
            // SAFETY: The mutex has been unlocked previously.
            unsafe { Self::mutex(s).raw().lock() }
        );
        f().await
    }
}

/// A generic mutex for the ordered_lock operations.
pub trait MutexLike {
    type Guard<'a>
    where
        Self: 'a;
    type Context;

    fn context() -> Self::Context;

    /// Lock the mutex. `level` is the index of the locked mutex in the lock ordering.
    fn lock(&self, context: &mut Self::Context) -> Self::Guard<'_>;

    #[inline(always)]
    fn key(&self) -> *const ()
    where
        Self: Sized,
    {
        self as *const Self as *const ()
    }
}

impl<T> MutexLike for Mutex<T> {
    type Guard<'a>
        = MutexGuard<'a, T>
    where
        T: 'a;
    type Context = ();

    #[inline(always)]
    fn context() -> Self::Context {
        ()
    }

    #[inline(always)]
    fn lock(&self, _context: &mut Self::Context) -> Self::Guard<'_> {
        return self.lock();
    }
}

/// A generic rwlock for the ordered_lock operations.
pub trait RwLockLike {
    type ReadGuard<'a>
    where
        Self: 'a;
    type WriteGuard<'a>
    where
        Self: 'a;
    type Context;

    fn context() -> Self::Context;

    fn read(&self, context: &mut Self::Context) -> Self::ReadGuard<'_>;
    fn write(&self, context: &mut Self::Context) -> Self::WriteGuard<'_>;

    #[inline(always)]
    fn key(&self) -> *const ()
    where
        Self: Sized,
    {
        self as *const Self as *const ()
    }
}

impl<T> RwLockLike for RwLock<T> {
    type ReadGuard<'a>
        = RwLockReadGuard<'a, T>
    where
        T: 'a;
    type WriteGuard<'a>
        = RwLockWriteGuard<'a, T>
    where
        T: 'a;
    type Context = ();

    #[inline(always)]
    fn context() -> Self::Context {
        ()
    }

    #[inline(always)]
    fn read(&self, _context: &mut Self::Context) -> Self::ReadGuard<'_> {
        self.read()
    }

    #[inline(always)]
    fn write(&self, _context: &mut Self::Context) -> Self::WriteGuard<'_> {
        self.write()
    }
}

/// Lock `m1` and `m2` in a consistent order (using the memory address of m1 and m2 and returns the
/// associated guard. This ensure that `ordered_lock(m1, m2)` and `ordered_lock(m2, m1)` will not
/// deadlock.
pub fn ordered_lock<'a, M: MutexLike>(m1: &'a M, m2: &'a M) -> (M::Guard<'a>, M::Guard<'a>) {
    let mut context = M::context();
    if m1.key() < m2.key() {
        let g1 = m1.lock(&mut context);
        let g2 = m2.lock(&mut context);
        (g1, g2)
    } else {
        let g2 = m2.lock(&mut context);
        let g1 = m1.lock(&mut context);
        (g1, g2)
    }
}

/// Acquires multiple mutexes in a consistent order based on their memory addresses.
/// This helps prevent deadlocks.
pub fn ordered_lock_vec<'a, M: MutexLike>(mutexes: &[&'a M]) -> Vec<M::Guard<'a>> {
    let mut context = M::context();

    // Create a vector of tuples containing the mutex and its original index.
    let mut indexed_mutexes = mutexes.iter().enumerate().map(|(i, m)| (i, *m)).collect::<Vec<_>>();

    // Sort the indexed mutexes by their keys.
    indexed_mutexes.sort_by_key(|(_, m)| m.key());

    // Acquire the locks in the sorted order.
    let mut guards =
        indexed_mutexes.into_iter().map(|(i, m)| (i, m.lock(&mut context))).collect::<Vec<_>>();

    // Reorder the guards to match the original order of the mutexes.
    guards.sort_by_key(|(i, _)| *i);

    guards.into_iter().map(|(_, g)| g).collect::<Vec<_>>()
}

/// Lock `r1` and `r2` in a consistent order (using the memory address of r1 and r2) for reading.
pub fn ordered_read_lock<'a, R: RwLockLike>(
    r1: &'a R,
    r2: &'a R,
) -> (R::ReadGuard<'a>, R::ReadGuard<'a>) {
    let w1 = RwLockReadWrapper(r1);
    let w2 = RwLockReadWrapper(r2);
    ordered_lock(&w1, &w2)
}

/// Lock `r1` and `r2` in a consistent order (using the memory address of r1 and r2) for writing.
pub fn ordered_write_lock<'a, R: RwLockLike>(
    r1: &'a R,
    r2: &'a R,
) -> (R::WriteGuard<'a>, R::WriteGuard<'a>) {
    let w1 = RwLockWriteWrapper(r1);
    let w2 = RwLockWriteWrapper(r2);
    ordered_lock(&w1, &w2)
}

/// Acquires multiple rwlocks in a consistent order based on their memory addresses for reading.
pub fn ordered_read_lock_vec<'a, R: RwLockLike>(rwlocks: &[&'a R]) -> Vec<R::ReadGuard<'a>> {
    let wrappers = rwlocks.iter().map(|r| RwLockReadWrapper(*r)).collect::<Vec<_>>();
    let wrapper_refs = wrappers.iter().collect::<Vec<_>>();
    ordered_lock_vec(&wrapper_refs)
}

/// Acquires multiple rwlocks in a consistent order based on their memory addresses for writing.
pub fn ordered_write_lock_vec<'a, R: RwLockLike>(rwlocks: &[&'a R]) -> Vec<R::WriteGuard<'a>> {
    let wrappers = rwlocks.iter().map(|r| RwLockWriteWrapper(*r)).collect::<Vec<_>>();
    let wrapper_refs = wrappers.iter().collect::<Vec<_>>();
    ordered_lock_vec(&wrapper_refs)
}

struct RwLockReadWrapper<'a, R>(&'a R);
struct RwLockWriteWrapper<'a, R>(&'a R);

impl<'a, R: RwLockLike> MutexLike for RwLockReadWrapper<'a, R> {
    type Guard<'b>
        = R::ReadGuard<'a>
    where
        Self: 'b;
    type Context = R::Context;

    #[inline(always)]
    fn context() -> Self::Context {
        R::context()
    }

    #[inline(always)]
    fn lock(&self, context: &mut Self::Context) -> Self::Guard<'_> {
        self.0.read(context)
    }

    #[inline(always)]
    fn key(&self) -> *const () {
        self.0.key()
    }
}

impl<'a, R: RwLockLike> MutexLike for RwLockWriteWrapper<'a, R> {
    type Guard<'b>
        = R::WriteGuard<'a>
    where
        Self: 'b;
    type Context = R::Context;

    #[inline(always)]
    fn context() -> Self::Context {
        R::context()
    }

    #[inline(always)]
    fn lock(&self, context: &mut Self::Context) -> Self::Guard<'_> {
        self.0.write(context)
    }

    #[inline(always)]
    fn key(&self) -> *const () {
        self.0.key()
    }
}
#[cfg(test)]
mod test {
    use super::*;

    #[::fuchsia::test]
    fn test_lock_ordering() {
        let l1 = Mutex::new(1);
        let l2 = Mutex::new(2);

        {
            let (g1, g2) = ordered_lock(&l1, &l2);
            assert_eq!(*g1, 1);
            assert_eq!(*g2, 2);
        }
        {
            let (g2, g1) = ordered_lock(&l2, &l1);
            assert_eq!(*g1, 1);
            assert_eq!(*g2, 2);
        }
    }

    #[::fuchsia::test]
    fn test_vec_lock_ordering() {
        let l1 = Mutex::new(1);
        let l0 = Mutex::new(0);
        let l2 = Mutex::new(2);

        {
            let guards = ordered_lock_vec(&[&l0, &l1, &l2]);
            assert_eq!(*guards[0], 0);
            assert_eq!(*guards[1], 1);
            assert_eq!(*guards[2], 2);
        }
        {
            let guards = ordered_lock_vec(&[&l2, &l1, &l0]);
            assert_eq!(*guards[0], 2);
            assert_eq!(*guards[1], 1);
            assert_eq!(*guards[2], 0);
        }
    }

    #[::fuchsia::test]
    fn test_ordered_rwlock_wrappers() {
        let l1: RwLock<u8> = RwLock::new(1);
        let l2: RwLock<u8> = RwLock::new(2);

        {
            let (g1, g2) = ordered_read_lock(&l1, &l2);
            assert_eq!(*g1, 1);
            assert_eq!(*g2, 2);
        }
        {
            let (g2, g1) = ordered_read_lock(&l2, &l1);
            assert_eq!(*g1, 1);
            assert_eq!(*g2, 2);
        }
        {
            let (g1, g2) = ordered_write_lock(&l1, &l2);
            assert_eq!(*g1, 1);
            assert_eq!(*g2, 2);
        }
        {
            let (g2, g1) = ordered_write_lock(&l2, &l1);
            assert_eq!(*g1, 1);
            assert_eq!(*g2, 2);
        }
    }

    #[::fuchsia::test]
    fn test_ordered_rwlock_vec() {
        let l1: RwLock<u8> = RwLock::new(1);
        let l0: RwLock<u8> = RwLock::new(0);
        let l2: RwLock<u8> = RwLock::new(2);

        {
            let guards = ordered_read_lock_vec(&[&l0, &l1, &l2]);
            assert_eq!(*guards[0], 0);
            assert_eq!(*guards[1], 1);
            assert_eq!(*guards[2], 2);
        }
        {
            let guards = ordered_write_lock_vec(&[&l2, &l1, &l0]);
            assert_eq!(*guards[0], 2);
            assert_eq!(*guards[1], 1);
            assert_eq!(*guards[2], 0);
        }
    }
}
