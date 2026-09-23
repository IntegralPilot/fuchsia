// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::task::Poll;

use sapphire_sync::mutex::raw::RawMutex;
use sapphire_sync::mutex::{Mutex, MutexGuard};

use crate::notification::Notification;

/// An asynchronous coordination fundamental protecting a shared payload `T` under a condition.
///
/// `Condition` wraps a payload `T` behind an internal `Mutex`, and coordinates async tasks
/// by blocking them until a user-provided predicate evaluates to [`Poll::Ready`].
/// When the state is updated, the publisher calls `notify_one`, `notify_many`, or `notify_all`
/// to wake up blocked tasks and re-evaluate their predicate.
///
/// # Examples
///
/// Basic predicate-based async blocking:
///
/// ```
/// use sapphire_async::condition::Condition;
/// use sapphire_async::testing::TestExecutor;
/// use sapphire_async::executor::BoundedExecutor;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
/// use core::task::Poll;
///
/// type TestCondition = Condition<SingleThreadMutex, i32>;
///
/// # let cond = TestCondition::new(0);
///
/// let mut last_seen_count = core::cell::Cell::new(-1);
/// # BoundedExecutor::new(TestExecutor::new(), |s| {
/// #     s.spawn(async {
/// // Block until count becomes >= 2
/// let final_val = cond.when(|count| {
///     last_seen_count.set(*count);
///     if *count >= 2 {
///          Poll::Ready(*count)
///      } else {
///          Poll::Pending
///      }
///  }).await;
///  assert_eq!(final_val, 2);
///  #   });
///
///  assert_eq!(last_seen_count.get(), -1);
///  s.run_until_stalled();
///  assert_eq!(last_seen_count.get(), 0);
///  // On another task...
///  # s.block_on(async {
///  // Increment and notify
///  {
///      let mut lock = cond.lock();
///      *lock += 1;
///  }
///  # });
///  // Awake the task but nothing will happen since count < 2
///  cond.notify_one();
///  s.run_until_stalled();
///  assert_eq!(last_seen_count.get(), 1);
///
///  # s.block_on(async {
///  // Increment and notify again
///  {
///      let mut lock = cond.lock();
///      *lock += 1;
///  }
///  # });
///  // Awake the other task, now it will return ready
///  cond.notify_one();
///  s.run_until_stalled();
///  assert_eq!(last_seen_count.get(), 2);
/// # });
/// ```
pub struct Condition<Mtx, T> {
    data: Mutex<Mtx, T>,
    signal: Notification<Mtx>,
}

impl<Mtx: RawMutex, T> Condition<Mtx, T> {
    /// Creates a new `Condition` protecting the given initial payload.
    pub fn new(data: T) -> Self {
        Self { data: Mutex::new(data), signal: Notification::new() }
    }

    /// Asynchronously blocks until the provided predicate returns `Poll::Ready(R)`.
    ///
    /// Locks the internal mutex and evaluates the predicate. If it returns `Poll::Pending`,
    /// the lock is atomically released and the task blocks until notified.
    pub async fn when<F, R>(&self, fun: F) -> R
    where
        F: FnMut(&mut T) -> Poll<R>,
    {
        let guard = self.data.lock();
        self.signal.when(guard, fun).await
    }

    /// Returns an RAII guard representing exclusive mutable access to the protected payload.
    pub fn lock(&self) -> MutexGuard<'_, Mtx, T> {
        self.data.lock()
    }

    /// Wakes up exactly one blocked task waiting on this condition.
    pub fn notify_one(&self) {
        self.signal.notify_one();
    }

    /// Wakes up to `count` blocked tasks waiting on this condition.
    pub fn notify_many(&self, count: usize) {
        self.signal.notify_many(count);
    }

    /// Wakes up all blocked tasks waiting on this condition.
    pub fn notify_all(&self) {
        self.signal.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::BoundedExecutor;
    use crate::testing::TestExecutor;
    use sapphire_sync::mutex::raw::SingleThreadMutex;

    type TestCondition = Condition<SingleThreadMutex, i32>;

    #[test]
    fn test_condition_basic() {
        let cond = TestCondition::new(0);
        BoundedExecutor::new(TestExecutor::new(), |s| {
            let handle = s.spawn(async {
                let val = cond
                    .when(|count| if *count >= 2 { Poll::Ready(*count) } else { Poll::Pending })
                    .await;
                assert_eq!(val, 2);
            });

            s.run_until_stalled();
            assert!(!handle.is_finished());

            // Increment to 1, notify. Should still block.
            {
                let mut lock = cond.lock();
                *lock = 1;
            }
            cond.notify_one();
            s.run_until_stalled();
            assert!(!handle.is_finished());

            // Increment to 2, notify. Should wake and complete.
            {
                let mut lock = cond.lock();
                *lock = 2;
            }
            cond.notify_one();
            s.run_until_stalled();
            assert!(handle.is_finished());
        });
    }

    #[test]
    fn test_condition_immediate() {
        let cond = TestCondition::new(2);
        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                let val = cond
                    .when(|count| if *count >= 2 { Poll::Ready(*count) } else { Poll::Pending })
                    .await;
                assert_eq!(val, 2);
            });
        });
    }
}
