// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::task::Poll;

use crate::condition::Condition;
use sapphire_sync::mutex::raw::RawMutex;

/// An asynchronous counting semaphore.
///
/// Semaphores track a number of available permits. Tasks can asynchronously acquire
/// permits using [`Semaphore::down`], blocking if no permits are available, and release
/// permits using [`Semaphore::up`], unblocking waiting tasks.
///
/// The semaphore's unblock order is the same as [`Notification`](crate::notification::Notification)
///
/// # Examples
///
/// Coordinating permit acquisition across concurrent tasks:
///
/// ```
/// use sapphire_async::semaphore::Semaphore;
/// use sapphire_async::testing::TestExecutor;
/// use sapphire_async::executor::BoundedExecutor;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
/// use std::cell::Cell;
///
/// type TestSemaphore = Semaphore<SingleThreadMutex>;
///
/// # ;
/// let sem = TestSemaphore::new(0); // Start with no permits
/// let completed = Cell::new(false);
///
/// # BoundedExecutor::new(TestExecutor::new(), |s| {
/// #     s.spawn(async {
/// sem.down().await; // Blocks until a permit is released
/// completed.set(true);
/// #     });
/// #
/// #     s.run_until_stalled();
///
/// // In another task ...
/// assert!(!completed.get()); // Task remains blocked
///
/// sem.up(); // Release a permit, unblocking the task!
/// #     s.run_until_stalled();
/// assert!(completed.get()); // Task successfully completed!
/// # });
/// ```
///
/// A `Semaphore` using `SingleThreadMutex` cannot be shared across threads (compile_fail):
///
/// ```compile_fail
/// use sapphire_async::semaphore::Semaphore;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;

/// type NonSyncSemaphore = Semaphore<SingleThreadMutex>;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<NonSyncSemaphore>(); // Correctly fails to compile because SingleThreadMutex is !Sync!
/// ```
pub struct Semaphore<Mtx> {
    count: Condition<Mtx, usize>,
}

impl<Mtx: RawMutex> Semaphore<Mtx> {
    /// Creates a new Counting Semaphore with the specified `initial_count` of available permits.
    pub fn new(initial_count: usize) -> Self {
        Self { count: Condition::new(initial_count) }
    }

    /// Asynchronously acquires a permit from the semaphore.
    ///
    /// Blocks the calling task if no permits are available (count is 0).
    pub async fn down(&self) {
        self.count
            .when(|count| {
                if *count > 0 {
                    *count -= 1;
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
    }

    /// Releases a permit back to the semaphore.
    ///
    /// Increments the permit count and wakes up the next blocked task waiting. Ordering is dependent
    /// on the underlying [`Notification`] fundamental.
    pub fn up(&self) {
        *self.count.lock() += 1;
        self.count.notify_one();
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::executor::BoundedExecutor;
    use crate::testing::TestExecutor;
    use sapphire_sync::mutex::raw::SingleThreadMutex;
    use std::cell::RefCell;

    type TestSemaphore = Semaphore<SingleThreadMutex>;

    #[test]
    fn test_semaphore_acquire_initial_permits() {
        let sem = TestSemaphore::new(2);

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                sem.down().await;
                sem.down().await;
            });
        });
    }

    #[test]
    fn test_semaphore_block_until_up() {
        let sem = TestSemaphore::new(0);

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let handle = s.spawn(async {
                sem.down().await;
            });

            s.run_until_stalled();
            assert!(!handle.is_finished(), "Task should be blocked on semaphore");

            sem.up();
            s.run_until_stalled();
            assert!(handle.is_finished(), "Task should be completed after semaphore up");
        });
    }

    #[test]
    fn test_semaphore_order_matches_notification() {
        let sem = TestSemaphore::new(0);
        let order = RefCell::new(Vec::new());

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.spawn(async {
                sem.down().await;
                order.borrow_mut().push(1);
            });

            s.run_until_stalled();
            s.spawn(async {
                sem.down().await;
                order.borrow_mut().push(2);
            });

            s.run_until_stalled();
            assert!(order.borrow().is_empty());

            sem.up();
            s.run_until_stalled();
            assert_eq!(*order.borrow(), vec![1]);

            sem.up();
            s.run_until_stalled();
            assert_eq!(*order.borrow(), vec![1, 2]);
        });
    }

    #[test]
    fn test_semaphore_cancellation_lost_wakeup() {
        use futures::future::FutureExt;
        let sem = TestSemaphore::new(0);
        let cancel_notif = crate::notification::Notification::<SingleThreadMutex>::new();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let h1 = s.spawn(async {
                let fut = sem.down().fuse();
                let cancel = cancel_notif.wait().fuse();
                futures::pin_mut!(fut);
                futures::pin_mut!(cancel);
                futures::select_biased! {
                    _ = cancel => {},
                    _ = fut => {}
                }
            });

            let h2 = s.spawn(async {
                sem.down().await;
            });

            s.run_until_stalled();
            assert!(!h1.is_finished());
            assert!(!h2.is_finished());

            // Wake both.
            // cancel_notif wakes h1's cancel branch.
            // sem.up() wakes h1's sem branch (since it is first in queue).
            cancel_notif.notify_one();
            sem.up();

            s.run_until_stalled();

            // h1 should be finished (via cancel).
            assert!(h1.is_finished());

            // If there is a lost wakeup, h2 is still blocked, even though sem has 1 permit.
            assert!(h2.is_finished(), "h2 should be finished if semaphore is cancel-safe");
        });
    }

    mod proptests {
        use super::*;
        use crate::executor::BoundedExecutor;
        use crate::testing::TestExecutor;
        use proptest::prelude::*;
        use sapphire_sync::mutex::raw::SingleThreadMutex;
        use std::cell::Cell;

        #[derive(Debug, Clone)]
        enum SemOp {
            Acquire,
            Release,
        }

        proptest! {
            #[test]
            fn test_semaphore_completed_acquires_equals_permits_provided(
                initial_count in 0..10usize,
                ops in prop::collection::vec(
                    prop_oneof![
                        Just(SemOp::Acquire),
                        Just(SemOp::Release),
                    ],
                    0..50
                )
            ) {
                let sem = Semaphore::<SingleThreadMutex>::new(initial_count);

                let completed = Cell::new(0);

                BoundedExecutor::new(TestExecutor::new(), |s| {
                    let mut total_acquires = 0;
                    let mut total_releases = 0;

                    for op in ops {
                        match op {
                            SemOp::Acquire => {
                                total_acquires += 1;
                                s.spawn(async {
                                    sem.down().await;
                                    completed.set(completed.get() + 1);
                                });
                            }
                            SemOp::Release => {
                                total_releases += 1;
                                sem.up();
                            }
                        }
                        s.run_until_stalled();

                        let expected_completed = std::cmp::min(total_acquires, initial_count + total_releases);
                        assert_eq!(completed.get(), expected_completed);
                    }
                });
            }
        }
    }
}
