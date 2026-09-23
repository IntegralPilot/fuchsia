// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::task::Poll;

use crate::condition::Condition;
use sapphire_sync::mutex::raw::RawMutex;

/// An asynchronous mutual exclusion foundation protecting shared data of type `T`.
///
/// Unlike synchronous mutexes, `sapphire_async::mutex::Mutex` does not block the calling OS thread
/// when a lock is contended. Instead, tasks that attempt to acquire the lock via [`Mutex::lock`]
/// asynchronously yield to the executor until the lock becomes available.
///
/// # Examples
///
/// ```
/// use sapphire_async::mutex::Mutex;
/// use sapphire_async::testing::TestExecutor;
/// use sapphire_async::executor::BoundedExecutor;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// type TestMutex = Mutex<SingleThreadMutex, i32>;
///
/// let mtx = TestMutex::new(0);
/// BoundedExecutor::new(TestExecutor::new(), |s| {
///     s.spawn(async {
///         let mut guard = mtx.lock().await;
///         *guard += 42;
///     });
///     s.run_until_stalled();
///     assert_eq!(*mtx.try_lock().unwrap(), 42);
/// });
/// ```
pub struct Mutex<Mtx, T> {
    state: Condition<Mtx, bool>,
    payload: UnsafeCell<T>,
}

unsafe impl<Mtx: RawMutex + Send, T: Send> Send for Mutex<Mtx, T> {}
unsafe impl<Mtx: RawMutex + Sync, T: Send> Sync for Mutex<Mtx, T> {}

impl<Mtx: RawMutex, T> Mutex<Mtx, T> {
    /// Creates a new unlocked asynchronous `Mutex` protecting the provided data.
    pub fn new(data: T) -> Self {
        Self { state: Condition::new(false), payload: UnsafeCell::new(data) }
    }

    /// Asynchronously acquires the lock, returning an RAII [`MutexGuard`] when available.
    pub async fn lock(&self) -> MutexGuard<'_, Mtx, T> {
        self.state
            .when(|locked| {
                if !*locked {
                    *locked = true;
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        MutexGuard { mutex: self }
    }

    /// Attempts to acquire the lock immediately without waiting.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, Mtx, T>> {
        let mut guard = self.state.lock();
        if !*guard {
            *guard = true;
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }
}

impl<Mtx: RawMutex, T: fmt::Debug> fmt::Debug for Mutex<Mtx, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Mutex");
        if let Some(guard) = self.try_lock() {
            d.field("data", &&*guard);
        } else {
            d.field("data", &format_args!("<locked>"));
        }
        d.finish()
    }
}

/// An RAII guard representing exclusive mutable access to the data protected by an async [`Mutex`].
pub struct MutexGuard<'a, Mtx: RawMutex, T> {
    mutex: &'a Mutex<Mtx, T>,
}

impl<'a, Mtx: RawMutex, T> MutexGuard<'a, Mtx, T> {
    /// Returns a reference to the underlying async mutex.
    pub fn mutex(&self) -> &'a Mutex<Mtx, T> {
        self.mutex
    }
}

impl<Mtx: RawMutex, T> Deref for MutexGuard<'_, Mtx, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.payload.get() }
    }
}

impl<Mtx: RawMutex, T> DerefMut for MutexGuard<'_, Mtx, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.payload.get() }
    }
}

impl<Mtx: RawMutex, T> Drop for MutexGuard<'_, Mtx, T> {
    fn drop(&mut self) {
        *self.mutex.state.lock() = false;
        self.mutex.state.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::BoundedExecutor;
    use crate::testing::TestExecutor;
    use sapphire_sync::mutex::raw::SingleThreadMutex;

    type TestMutex = Mutex<SingleThreadMutex, i32>;

    #[test]
    fn test_async_mutex_basic() {
        let mtx = TestMutex::new(0);
        BoundedExecutor::new(TestExecutor::new(), |s| {
            let handle = s.spawn(async {
                let mut guard = mtx.lock().await;
                *guard += 10;
            });
            s.run_until_stalled();
            assert!(handle.is_finished());
            assert_eq!(*mtx.try_lock().unwrap(), 10);
        });
    }

    #[test]
    fn test_async_mutex_contention() {
        let mtx = TestMutex::new(0);
        BoundedExecutor::new(TestExecutor::new(), |s| {
            let h1 = s.spawn(async {
                let mut guard = mtx.lock().await;
                *guard += 1;
            });

            let h2 = s.spawn(async {
                let mut guard = mtx.lock().await;
                *guard += 2;
            });

            s.run_until_stalled();
            assert!(h1.is_finished());
            assert!(h2.is_finished());
            assert_eq!(*mtx.try_lock().unwrap(), 3);
        });
    }
}
