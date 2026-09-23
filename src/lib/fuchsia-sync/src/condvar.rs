// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::MutexGuard;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[cfg(not(target_os = "fuchsia"))]
use parking_lot_core::{
    DEFAULT_PARK_TOKEN, DEFAULT_UNPARK_TOKEN, ParkResult, park, unpark_all, unpark_one,
};
#[cfg(not(target_os = "fuchsia"))]
use std::sync::atomic::AtomicU32;
#[cfg(not(target_os = "fuchsia"))]
use std::time::Instant;

/// A [condition variable][wikipedia] that integrates with [`fuchsia_sync::Mutex`].
///
/// [wikipedia]: https://en.wikipedia.org/wiki/Monitor_(synchronization)#Condition_variables
pub struct Condvar {
    /// Incremented by 1 on each notification.
    inner: Futex,
}

impl Condvar {
    pub const fn new() -> Self {
        Self { inner: Futex::new(0) }
    }

    pub fn notify_one(&self) {
        // Relaxed because the futex operation synchronizes.
        self.inner.fetch_add(1, Ordering::Relaxed);
        self.inner.wake_one();
    }

    pub fn notify_all(&self) {
        // Relaxed because the futex operation synchronizes.
        self.inner.fetch_add(1, Ordering::Relaxed);
        self.inner.wake_all();
    }

    pub fn wait<T: ?Sized>(&self, guard: &mut MutexGuard<'_, T>) {
        assert!(!self.wait_inner(guard, None).timed_out, "an infinite wait should not timeout");
    }

    pub fn wait_while<'a, T: ?Sized, F>(&self, guard: &mut MutexGuard<'a, T>, mut condition: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            self.wait(guard);
        }
    }

    pub fn wait_for<T: ?Sized>(
        &self,
        guard: &mut MutexGuard<'_, T>,
        timeout: Duration,
    ) -> WaitTimeoutResult {
        self.wait_inner(guard, Some(timeout))
    }

    pub fn wait_while_for<'a, T: ?Sized, F>(
        &self,
        guard: &mut MutexGuard<'a, T>,
        mut condition: F,
        timeout: Duration,
    ) -> WaitTimeoutResult
    where
        F: FnMut(&mut T) -> bool,
    {
        let mut result = WaitTimeoutResult { timed_out: false };

        while !result.timed_out() && condition(&mut *guard) {
            result = self.wait_for(guard, timeout);
        }

        result
    }

    fn wait_inner<T: ?Sized>(
        &self,
        guard: &mut MutexGuard<'_, T>,
        timeout: Option<Duration>,
    ) -> WaitTimeoutResult {
        // Relaxed because the futex and mutex operations synchronize.
        let current = self.inner.load(Ordering::Relaxed);
        MutexGuard::unlocked(guard, || self.inner.wait(current, timeout))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaitTimeoutResult {
    timed_out: bool,
}

impl WaitTimeoutResult {
    pub fn timed_out(&self) -> bool {
        self.timed_out
    }
}

#[cfg(target_os = "fuchsia")]
struct Futex(zx::Futex);

#[cfg(target_os = "fuchsia")]
impl Futex {
    const fn new(value: u32) -> Self {
        Self(zx::Futex::new(value as i32))
    }

    /// Returns the current value of the futex.
    fn load(&self, order: Ordering) -> u32 {
        self.0.load(order) as u32
    }

    fn fetch_add(&self, value: u32, order: Ordering) -> u32 {
        self.0.fetch_add(value as i32, order) as u32
    }

    fn wake_one(&self) {
        self.0.wake_single_owner();
    }

    fn wake_all(&self) {
        self.0.wake_all();
    }

    fn wait(&self, current: u32, timeout: Option<Duration>) -> WaitTimeoutResult {
        let deadline = timeout
            .map(|t| zx::MonotonicInstant::after(t.into()))
            .unwrap_or(zx::MonotonicInstant::INFINITE);
        match self.0.wait(current as i32, None, deadline) {
            // The count only goes up. If `current` isn't the current value, a notification
            // was received in between reading `current` and waiting.
            Ok(()) | Err(zx::Status::BAD_STATE) => WaitTimeoutResult { timed_out: false },
            Err(zx::Status::TIMED_OUT) => WaitTimeoutResult { timed_out: true },
            Err(e) => panic!("unexpected wait error {e:?}"),
        }
    }
}

#[cfg(not(target_os = "fuchsia"))]
struct Futex(AtomicU32);

#[cfg(not(target_os = "fuchsia"))]
impl Futex {
    const fn new(value: u32) -> Self {
        Self(AtomicU32::new(value))
    }

    fn load(&self, order: Ordering) -> u32 {
        self.0.load(order)
    }

    fn fetch_add(&self, value: u32, order: Ordering) -> u32 {
        self.0.fetch_add(value, order)
    }

    fn wake_one(&self) {
        // SAFETY: the address of `inner` is controlled by us.
        unsafe {
            unpark_one(self.0.as_ptr() as usize, |_| DEFAULT_UNPARK_TOKEN);
        }
    }

    fn wake_all(&self) {
        // SAFETY: the address of `inner` is controlled by us.
        unsafe {
            unpark_all(self.0.as_ptr() as usize, DEFAULT_UNPARK_TOKEN);
        }
    }

    fn wait(&self, current: u32, timeout: Option<Duration>) -> WaitTimeoutResult {
        let key = self.0.as_ptr() as usize;
        let deadline = timeout.map(|t| Instant::now() + t);

        // SAFETY: the address of `inner` is controlled by us.
        let park_result = unsafe {
            park(
                key,
                || self.0.load(Ordering::Relaxed) == current,
                || {},
                |_, _| {},
                DEFAULT_PARK_TOKEN,
                deadline,
            )
        };

        match park_result {
            ParkResult::Unparked(token) => {
                assert_eq!(token, DEFAULT_UNPARK_TOKEN);
                WaitTimeoutResult { timed_out: false }
            }
            ParkResult::Invalid => {
                // The validation function failed, meaning the state changed before we could park.
                // This counts as a notification.
                WaitTimeoutResult { timed_out: false }
            }
            ParkResult::TimedOut => WaitTimeoutResult { timed_out: true },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mutex;

    #[test]
    fn notify_one_works() {
        let mutex = Mutex::new(());
        let condvar = Condvar::new();
        crossbeam::thread::scope(|s| {
            let mut locked = mutex.lock();
            s.spawn(|_| {
                // With the lock already held, this won't return until the below wait starts.
                let _locked = mutex.lock();
                condvar.notify_one();
            });

            // This will hang forever (and time out the test infra) if notification doesn't work.
            condvar.wait(&mut locked);
        })
        .unwrap();
    }

    #[test]
    fn notify_all_works() {
        let num_threads = 10;
        let count = Mutex::new(0);
        let condvar = Condvar::new();
        let (send, recv) = std::sync::mpsc::channel();

        crossbeam::thread::scope(|s| {
            for _ in 0..num_threads {
                s.spawn(|_| {
                    let mut count = count.lock();
                    *count += 1;
                    if *count == num_threads {
                        // Notify the main thread that the last thread has acquired the lock.
                        send.send(()).unwrap();
                    }
                    while *count != 0 {
                        condvar.wait(&mut count);
                    }
                });
            }

            // Wait for all threads to have started waiting on their condvar.
            recv.recv().unwrap();

            let mut count = count.lock();
            *count = 0;
            condvar.notify_all();
            drop(count);

            // The crossbeam scope will now wait for all of the spawned threads to observe count=0.
        })
        .unwrap();
    }

    #[test]
    fn wait_while_works() {
        let pending = Mutex::new(true);
        let condvar = Condvar::new();

        crossbeam::thread::scope(|s| {
            let mut locked_pending = pending.lock();

            s.spawn(|_| {
                // With the lock already held, this won't return until the below wait starts.
                let mut locked_pending = pending.lock();
                *locked_pending = false;
                condvar.notify_one();
            });

            // This will hang forever (and time out the test infra) if the notification doesn't work
            // or if the condition never evaluates to false.
            condvar.wait_while(&mut locked_pending, |pending| !*pending);
        })
        .unwrap();
    }

    #[test]
    fn wait_for_times_out() {
        let mutex = Mutex::new(());
        let condvar = Condvar::new();

        let mut locked = mutex.lock();

        // Account for possible spurious wakeups.
        loop {
            if condvar.wait_for(&mut locked, std::time::Duration::from_secs(1)).timed_out() {
                break;
            }
        }
    }

    #[test]
    fn wait_while_for_times_out() {
        let mutex = Mutex::new(());
        let condvar = Condvar::new();

        let mut locked = mutex.lock();

        let result =
            condvar.wait_while_for(&mut locked, |_value| true, std::time::Duration::from_secs(1));

        assert!(result.timed_out());
    }
}
