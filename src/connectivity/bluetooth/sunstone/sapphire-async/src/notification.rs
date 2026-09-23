// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod intrusive_list;

use core::cell::UnsafeCell;
use core::future::Future;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

use sapphire_sync::mutex::raw::RawMutex;
use sapphire_sync::mutex::{Mutex, MutexGuard};

use crate::notification::intrusive_list::{WaiterList, WaiterListLink};

/// The node stored within the waiting futures.
pub struct Waiter {
    link: WaiterListLink,
    waker: UnsafeCell<Option<Waker>>,
}

impl Waiter {
    pub fn new() -> Self {
        Self { link: WaiterListLink::null(), waker: UnsafeCell::new(None) }
    }
}

/// A runtime-agnostic async notification and thread-waking synchronization fundamental.
///
/// Uses `intrusive_collections` doubly-linked list strategy to store pending wakers
/// directly inside the waiting futures themselves, avoiding any heap allocations.
///
/// Notifications happen in FIFO order
pub struct Notification<Mtx> {
    state: Mutex<Mtx, NotificationState>,
}

struct NotificationState {
    wakers: WaiterList,
}

impl<Mtx: RawMutex> Notification<Mtx> {
    /// Creates a new, unnotified `Notification` fundamental.
    pub fn new() -> Self {
        Self { state: Mutex::new(NotificationState { wakers: WaiterList::new() }) }
    }

    /// Asynchronously blocks the current task until notified.
    pub fn wait(&self) -> WaitFuture<'_, Mtx> {
        WaitFuture {
            notification: self,
            waiter: UnsafeCell::new(Waiter::new()),
            was_polled: false,
            on_first_poll: None,
            _pinned: PhantomPinned,
        }
    }

    /// Asynchronously blocks the current task and executes the provided closure.
    ///
    /// The provided closure will be executed after linking the waiter to the notification
    /// but before going to sleep.
    pub async fn wait_and<F>(&self, fun: F)
    where
        F: FnOnce(),
    {
        WaitFuture {
            notification: self,
            waiter: UnsafeCell::new(Waiter::new()),
            was_polled: false,
            on_first_poll: Some(fun),
            _pinned: PhantomPinned,
        }
        .await;
    }

    /// Asynchronously blocks until the provided predicate closure `fun` evaluates to `Poll::Ready(R)`.
    ///
    /// Performs predicate checking in a loop: if `fun` returns `Poll::Pending`, it atomically
    /// releases the lock and blocks. Wakes up on notification to re-evaluate.
    pub async fn when<'a, F, ChannelMtx: RawMutex, T, R>(
        &self,
        mut lock: MutexGuard<'a, ChannelMtx, T>,
        mut fun: F,
    ) -> R
    where
        F: FnMut(&mut T) -> Poll<R>,
    {
        match fun(&mut *lock) {
            Poll::Ready(out) => return out,
            Poll::Pending => {}
        }

        let mutex = lock.mutex();
        loop {
            self.wait_and(move || drop(lock)).await;
            let mut guard = mutex.lock();
            match fun(&mut *guard) {
                Poll::Ready(out) => return out,
                Poll::Pending => {
                    lock = guard;
                }
            }
        }
    }

    /// Wakes up exactly one blocked task waiting on this notification.
    pub fn notify_one(&self) {
        self.notify_many(1);
    }

    /// Wakes up `min(count, self.waiters())` blocked tasks waiting on this notification.
    ///
    /// Does nothing if `count == 0`
    pub fn notify_many(&self, mut count: usize) {
        if count == 0 {
            return;
        }
        let mut state = self.state.lock();
        while count > 0
            && let Some(waiter) = state.wakers.pop_front()
        {
            // SAFETY: The pointer is pinned to the Future, so either it's present or Drop would clean it up from the list.
            let waker_opt = unsafe { &*waiter.as_ref().waker.get() };
            if let Some(waker) = waker_opt {
                waker.wake_by_ref();
                count -= 1;
            }
        }
    }

    /// Wakes up all blocked tasks waiting on this notification.
    pub fn notify_all(&self) {
        let mut state = self.state.lock();
        while let Some(waiter) = state.wakers.pop_front() {
            // SAFETY: The pointer is pinned to the Future, so either it's present or Drop would clean it up from the list.
            let waker_opt = unsafe { &*waiter.as_ref().waker.get() };
            if let Some(waker) = waker_opt {
                waker.wake_by_ref();
            }
        }
    }
}

/// Future returned by [`Notification::wait`].
pub struct WaitFuture<'n, Mtx: RawMutex, F = fn()> {
    notification: &'n Notification<Mtx>,
    waiter: UnsafeCell<Waiter>,
    was_polled: bool,
    on_first_poll: Option<F>,
    _pinned: PhantomPinned,
}

impl<'n, Mtx: RawMutex, F: FnOnce()> Future for WaitFuture<'n, Mtx, F> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We won't move out of `this`
        let this = unsafe { self.get_unchecked_mut() };
        let mut state = this.notification.state.lock();
        let waiter_ptr = this.waiter.get();
        if !this.was_polled {
            // SAFETY: The pointer is pinned to the Future, so either it's present or Drop would
            // clean it up from the list.
            unsafe { *(*waiter_ptr).waker.get() = Some(cx.waker().clone()) };
            // SAFETY: `waiter` is a pinned projection to `self`, so the pointer will live long
            // enough or Drop will clean it up
            unsafe { state.wakers.push_back(NonNull::new_unchecked(waiter_ptr)) };
            this.was_polled = true;

            if let Some(f) = this.on_first_poll.take() {
                f();
            }

            Poll::Pending
        // SAFETY: waiter can only be linked to `state.wakers`
        } else if unsafe { state.wakers.is_unlinked(&*waiter_ptr) } {
            this.was_polled = false;
            Poll::Ready(())
        } else {
            // SAFETY: The pointer is pinned to the Future, so either it's present or Drop would clean it up from the list.
            unsafe { *(*waiter_ptr).waker.get() = Some(cx.waker().clone()) };
            Poll::Pending
        }
    }
}

impl<'n, Mtx: RawMutex, F> Drop for WaitFuture<'n, Mtx, F> {
    fn drop(&mut self) {
        let mut notify_needed = false;
        {
            let mut state = self.notification.state.lock();
            let waiter_ptr = self.waiter.get();
            // SAFETY: waiter can only be linked to `state.wakers`
            if unsafe { state.wakers.is_linked(&*waiter_ptr) } {
                // SAFETY: waiter can only be linked to `state.wakers`
                unsafe { state.wakers.remove(NonNull::new_unchecked(waiter_ptr)) };
            } else if self.was_polled && unsafe { state.wakers.is_unlinked(&*waiter_ptr) } {
                notify_needed = true;
            }
        }
        if notify_needed {
            self.notification.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::BoundedExecutor;
    use crate::testing::TestExecutor;
    use sapphire_sync::mutex::raw::SingleThreadMutex;

    type TestNotification = Notification<SingleThreadMutex>;

    #[test]
    fn test_notification_basic() {
        let notif = TestNotification::new();
        BoundedExecutor::new(TestExecutor::new(), |s| {
            let handle = s.spawn(async {
                notif.wait().await;
            });

            s.run_until_stalled();
            assert!(!handle.is_finished());

            notif.notify_one();
            s.run_until_stalled();
            assert!(handle.is_finished());
        });
    }

    #[test]
    fn test_notification_notify_all() {
        let notif = TestNotification::new();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let mut handles = Vec::new();
            for _ in 0..3 {
                handles.push(s.spawn(async {
                    notif.wait().await;
                }));
            }

            s.run_until_stalled();
            for h in &handles {
                assert!(!h.is_finished());
            }

            notif.notify_all();
            s.run_until_stalled();
            for h in &handles {
                assert!(h.is_finished());
            }
        });
    }

    #[test]
    fn test_notification_notify_many() {
        let notif = TestNotification::new();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let mut handles = Vec::new();
            for _ in 0..3 {
                handles.push(s.spawn(async {
                    notif.wait().await;
                }));
            }

            s.run_until_stalled();
            for h in &handles {
                assert!(!h.is_finished());
            }

            notif.notify_many(2);
            s.run_until_stalled();
            assert!(handles[0].is_finished());
            assert!(handles[1].is_finished());
            assert!(!handles[2].is_finished());

            notif.notify_many(1);
            s.run_until_stalled();
            assert!(handles[2].is_finished());
        });
    }

    #[test]
    fn test_notification_cancellation() {
        let notif = TestNotification::new();
        let cancel_notif = TestNotification::new();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            // Task 1: Wait on `notif` OR `cancel_notif`
            let h1 = s.spawn(async {
                let fut = notif.wait();
                let cancel = cancel_notif.wait();
                futures::pin_mut!(fut);
                futures::pin_mut!(cancel);
                futures::future::select(fut, cancel).await;
            });

            // Task 2: Just wait on `notif`
            let h2 = s.spawn(async {
                notif.wait().await;
            });

            s.run_until_stalled();
            assert!(!h1.is_finished());
            assert!(!h2.is_finished());

            // Cancel Task 1's wait on `notif` by triggering `cancel_notif`
            cancel_notif.notify_one();
            s.run_until_stalled();

            // Task 1 should be finished (completed via cancel_notif)
            assert!(h1.is_finished());
            // Task 2 should still be pending
            assert!(!h2.is_finished());

            // Notify `notif` once. It should notify Task 2, because Task 1 was cancelled/finished.
            notif.notify_one();
            s.run_until_stalled();

            // Task 2 should now be finished
            assert!(h2.is_finished());
        });
    }
}
