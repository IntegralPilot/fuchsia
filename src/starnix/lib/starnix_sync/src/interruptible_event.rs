// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;
use std::sync::atomic::Ordering;

/// A blocking object that can either be notified normally or interrupted
///
/// To block using an `InterruptibleEvent`, first call `begin_wait`. At this point, the event is
/// in the "waiting" state, and future calls to `notify` or `interrupt` will terminate the wait.
///
/// After `begin_wait` returns, call `block_until` to block the current thread until one of the
/// following conditions occur:
///
///  1. The given deadline expires.
///  2. At least one of the `notify` or `interrupt` functions were called after `begin_wait`.
///
/// It's safe to call `notify` or `interrupt` at any time. However, calls to `begin_wait` and
/// `block_until` must alternate, starting with `begin_wait`.
///
/// `InterruptibleEvent` uses two-phase waiting so that clients can register for notification,
/// perform some related work, and then start blocking. This approach ensures that clients do not
/// miss notifications that arrive after they perform the related work but before they actually
/// start blocking.
///
/// # Priority Inheritance and Dynamic Futex Owner Assignment
///
/// `InterruptibleEvent` supports Zircon Priority Inheritance (PI). If the target owner thread is
/// known at wait time, it can be passed directly to `block_until` / `zx_futex_wait`.
///
/// When the owner thread is not known when waiting begins (for example, when a transaction is
/// queued to a process-wide worker pool), `assign_new_owner` can be called by the worker thread
/// that later dequeues the work item. `assign_new_owner` uses `zx_futex_requeue` to atomically
/// transfer waiting threads to a secondary `requeue_target` futex while designating the worker
/// thread as the futex PI owner.
///
/// TODO(https://fxbug.dev/542307988): The long-term solution is a dedicated Zircon syscall (such as
/// `zx_futex_assign_owner`) to assign or update the PI owner of an existing futex without
/// requiring a secondary requeue target futex.
#[derive(Debug)]
pub struct InterruptibleEvent {
    futex: zx::Futex,
    requeue_target: zx::Futex,
}

/// The initial state.
///
///  * Transitions to `WAITING` after `begin_wait`.
const READY: i32 = 0;

/// The event is waiting for a notification or an interruption.
///
///  * Transitions to `NOTIFIED` after `notify`.
///  * Transitions to `INTERRUPTED` after `interrupt`.
///  * Transitions to `REQUEUED` after `assign_new_owner`.
///  * Transitions to `READY` if the deadline for `block_until` expires.
const WAITING: i32 = 1;

/// The event has been notified and will wake up.
///
///  * Transitions to `READY` after `block_until` processes the notification.
const NOTIFIED: i32 = 2;

/// The event has been interrupted and will wake up.
///
///  * Transitions to `READY` after `block_until` processes the interruption.
const INTERRUPTED: i32 = 3;

/// The event has been requeued to a secondary futex target with a new PI owner.
const REQUEUED: i32 = 4;

/// A guard object to enforce that clients call `begin_wait` before `block_until`.
#[must_use = "call block_until to advance the event state machine"]
pub struct EventWaitGuard<'a> {
    event: &'a Arc<InterruptibleEvent>,
}

impl<'a> EventWaitGuard<'a> {
    /// The underlying event associated with this guard.
    pub fn event(&self) -> &'a Arc<InterruptibleEvent> {
        self.event
    }

    /// Returns the owner of the underlying futex, if any.
    pub fn get_owner(&self) -> Option<zx::Koid> {
        self.event.get_owner()
    }

    /// Block the thread until either `deadline` expires, the event is notified, or the event is
    /// interrupted.
    pub fn block_until(
        self,
        new_owner: Option<&zx::Thread>,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), WakeReason> {
        self.event.block_until(new_owner, deadline)
    }
}

/// A description of why a `block_until` returned without the event being notified.
#[derive(Debug, PartialEq, Eq)]
pub enum WakeReason {
    /// `block_until` returned because another thread interrupted the wait using `interrupt`.
    Interrupted,

    /// `block_until` returned because the given deadline expired.
    DeadlineExpired,
}

impl Default for InterruptibleEvent {
    fn default() -> Self {
        InterruptibleEvent { futex: zx::Futex::new(READY), requeue_target: zx::Futex::new(READY) }
    }
}

impl InterruptibleEvent {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Returns the owner of the underlying futex or requeue target futex, if any.
    pub fn get_owner(&self) -> Option<zx::Koid> {
        self.futex.get_owner().or_else(|| self.requeue_target.get_owner())
    }

    /// Called to initiate a wait.
    ///
    /// Calls to `notify` or `interrupt` after this function returns will cause the event to wake
    /// up. Calls to those functions prior to calling `begin_wait` will be ignored.
    ///
    /// Once called, this function cannot be called again until `block_until` returns. Otherwise,
    /// this function will panic.
    pub fn begin_wait<'a>(self: &'a Arc<Self>) -> EventWaitGuard<'a> {
        self.requeue_target.store(READY, Ordering::Relaxed);
        self.futex
            .compare_exchange(READY, WAITING, Ordering::AcqRel, Ordering::Relaxed)
            .expect("Tried to begin waiting on an event when not ready.");
        EventWaitGuard { event: self }
    }

    /// Assigns `new_owner` as the Priority Inheritance (PI) owner for waiting thread(s).
    ///
    /// If threads are currently waiting on `futex`, this dynamically requeues them to
    /// `requeue_target`, designating `new_owner` as the futex PI owner.
    ///
    /// This establishes Zircon Priority Inheritance (PI) from the waiting thread(s) to
    /// `new_owner` when the owner was not known at `begin_wait` time.
    ///
    /// Note: This can only be called once per `begin_wait` cycle (while the event is in the
    /// `WAITING` state). If the event is not waiting or has already been requeued/notified,
    /// this returns `Err(zx::Status::BAD_STATE)`.
    ///
    /// TODO(https://fxbug.dev/542307988): Replace this requeue pattern with a dedicated Zircon
    /// syscall to assign a new owner to a futex with waiting threads once available.
    pub fn assign_new_owner(&self, new_owner: &zx::Thread) -> Result<(), zx::Status> {
        if self
            .futex
            .compare_exchange(WAITING, REQUEUED, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            let res =
                self.futex.requeue(0, REQUEUED, &self.requeue_target, u32::MAX, Some(new_owner));
            // Setting `requeue_target` to `WAITING` after `zx_futex_requeue` ensures that any
            // concurrent `wake()` loop does not issue `requeue_target.wake_all()` until after the
            // kernel wait queue transfer has completed.
            self.requeue_target.store(WAITING, Ordering::Release);
            res
        } else {
            Err(zx::Status::BAD_STATE)
        }
    }

    fn reset(&self) {
        // We use a store here rather than a compare_exchange because other threads are
        // only allowed to write to this value in the `WAITING` state and we are returning
        // from a completed wait.
        self.requeue_target.store(READY, Ordering::Relaxed);
        self.futex.store(READY, Ordering::Relaxed);
    }

    fn block_until(
        &self,
        new_owner: Option<&zx::Thread>,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), WakeReason> {
        // We need to loop around the call to zx_futex_wait because we can receive spurious
        // wakeups.
        loop {
            let futex_val = self.futex.load(Ordering::Acquire);
            let is_requeued = futex_val == REQUEUED;
            let target_futex = if is_requeued { &self.requeue_target } else { &self.futex };

            match target_futex.wait(WAITING, new_owner, deadline) {
                // The deadline expired while we were sleeping.
                Err(zx::Status::TIMED_OUT) => {
                    self.reset();
                    return Err(WakeReason::DeadlineExpired);
                }
                // The value changed before we went to sleep. Note: If `assign_new_owner()` set
                // `futex = REQUEUED` before `requeue_target` was initialized to `WAITING`,
                // `target_futex.wait()` fails with `BAD_STATE`. In this case, `effective_state`
                // evaluates to `WAITING`, causing the loop to retry on `requeue_target`.
                Err(zx::Status::BAD_STATE) => (),
                Err(e) => panic!("Unexpected error from zx_futex_wait: {e}"),
                Ok(()) => (),
            }

            let state = target_futex.load(Ordering::Acquire);
            let fallback_state = if is_requeued { futex_val } else { READY };

            let effective_state = if state == NOTIFIED || state == INTERRUPTED {
                state
            } else if fallback_state == NOTIFIED || fallback_state == INTERRUPTED {
                fallback_state
            } else {
                WAITING
            };

            let res = match effective_state {
                // If we're still in the `WAITING` state, then the wake ended spuriously and we
                // need to go back to sleep.
                WAITING => continue,
                NOTIFIED => Ok(()),
                INTERRUPTED => Err(WakeReason::Interrupted),
                _ => panic!("Unexpected event state: {effective_state}"),
            };
            self.reset();
            return res;
        }
    }

    /// Wake up the event normally.
    ///
    /// If this function is called before `begin_wait`, this notification is ignored. Calling this
    /// function repeatedly has no effect. If both `notify` and `interrupt` are called, the state
    /// observed by `block_until` is a race.
    pub fn notify(&self) {
        self.wake(NOTIFIED);
    }

    /// Wake up the event because of an interruption.
    ///
    /// If this function is called before `begin_wait`, this notification is ignored. Calling this
    /// function repeatedly has no effect. If both `notify` and `interrupt` are called, the state
    /// observed by `block_until` is a race.
    pub fn interrupt(&self) {
        self.wake(INTERRUPTED);
    }

    fn wake(&self, state: i32) {
        // Fast path for standard (non-requeued) events:
        // See <https://marabos.nl/atomics/hardware.html#failing-compare-exchange> for why we issue
        // this relaxed load before the `compare_exchange` below. Checking `observed == WAITING`
        // in Shared cache state prevents unnecessary exclusive cache line invalidations (RFOs)
        // when the event is already NOTIFIED, INTERRUPTED, or REQUEUED.
        //
        // We specify `Ordering::Acquire` on failure so that if `compare_exchange` fails because
        // another thread concurrently assigned a new owner (`futex` transitioned to `REQUEUED`),
        // we observe the updated state with Acquire semantics before moving to the slow path.
        let observed = self.futex.load(Ordering::Relaxed);
        if observed == WAITING
            && self
                .futex
                .compare_exchange(WAITING, state, Ordering::Release, Ordering::Acquire)
                .is_ok()
        {
            self.futex.wake_all();
            return;
        }

        // Slow path: `futex` was either already NOTIFIED/INTERRUPTED/READY, or `assign_new_owner()`
        // claimed `futex` by setting it to `REQUEUED`.
        //
        // This loop only applies when `assign_new_owner()` is called (e.g. during Binder worker
        // dispatch). When `assign_new_owner()` transitions `futex` to `REQUEUED`, there is a brief
        // window before it initializes `requeue_target` to `WAITING` and executes
        // `zx_futex_requeue`.
        //
        // We spin/yield until `requeue_target` transitions to `WAITING` (or until another thread
        // notifies it), ensuring that we wake the waiter on `requeue_target` without missing
        // notifications or deadlocking.
        loop {
            let futex_val = self.futex.load(Ordering::Acquire);
            if futex_val == NOTIFIED || futex_val == INTERRUPTED || futex_val == READY {
                return;
            }

            // futex_val is REQUEUED. Attempt to wake requeue_target once it is WAITING.
            let requeue_val = self.requeue_target.load(Ordering::Acquire);
            if requeue_val == WAITING {
                if self
                    .requeue_target
                    .compare_exchange(WAITING, state, Ordering::Release, Ordering::Acquire)
                    .is_ok()
                {
                    self.requeue_target.wake_all();
                    return;
                }
            } else if requeue_val == NOTIFIED || requeue_val == INTERRUPTED {
                return;
            }

            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_wait_block_and_notify() {
        let event = InterruptibleEvent::new();

        let guard = event.begin_wait();

        let other_event = Arc::clone(&event);
        let thread = std::thread::spawn(move || {
            other_event.notify();
        });

        guard.block_until(None, zx::MonotonicInstant::INFINITE).expect("failed to be notified");
        thread.join().expect("failed to join thread");
    }

    #[test]
    fn test_wait_block_and_interrupt() {
        let event = InterruptibleEvent::new();

        let guard = event.begin_wait();

        let other_event = Arc::clone(&event);
        let thread = std::thread::spawn(move || {
            other_event.interrupt();
        });

        let result = guard.block_until(None, zx::MonotonicInstant::INFINITE);
        assert_eq!(result, Err(WakeReason::Interrupted));
        thread.join().expect("failed to join thread");
    }

    #[test]
    fn test_wait_block_and_timeout() {
        let event = InterruptibleEvent::new();

        let guard = event.begin_wait();
        let result = guard
            .block_until(None, zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(20)));
        assert_eq!(result, Err(WakeReason::DeadlineExpired));
    }

    #[test]
    fn futex_ownership_is_transferred() {
        let event = Arc::new(InterruptibleEvent::new());

        let (root_thread_handle, root_thread_koid) = fuchsia_runtime::with_thread_self(|thread| {
            (thread.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(), thread.koid().unwrap())
        });

        let event_for_blocked_thread = event.clone();

        let blocked_thread = std::thread::spawn(move || {
            let event = event_for_blocked_thread;
            let guard = event.begin_wait();
            guard.block_until(Some(&root_thread_handle), zx::MonotonicInstant::INFINITE).unwrap();
        });

        // Wait for the correct owner to appear.
        // TODO(b/502692311): Replace this polling loop if it starts timing out.
        while event.get_owner() != Some(root_thread_koid) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        event.notify();
        blocked_thread.join().unwrap();
    }

    #[test]
    fn stale_pi_owner_is_noop() {
        let mut new_owner = None;
        std::thread::scope(|s| {
            s.spawn(|| {
                new_owner = Some(
                    fuchsia_runtime::with_thread_self(|thread| {
                        thread.duplicate_handle(zx::Rights::SAME_RIGHTS)
                    })
                    .unwrap(),
                );
            });
        });
        let new_owner = new_owner.unwrap();

        let event = InterruptibleEvent::new();
        let guard = event.begin_wait();
        let result = guard.block_until(
            Some(&new_owner),
            zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(20)),
        );
        assert_eq!(result, Err(WakeReason::DeadlineExpired));
    }

    #[test]
    fn futex_ownership_is_transferred_via_requeue() {
        let event = Arc::new(InterruptibleEvent::new());

        let (root_thread_handle, root_thread_koid) = fuchsia_runtime::with_thread_self(|thread| {
            (thread.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(), thread.koid().unwrap())
        });

        let event_for_blocked_thread = event.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let blocked_thread = std::thread::spawn(move || {
            let (thread_handle, _) = fuchsia_runtime::with_thread_self(|thread| {
                (thread.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(), thread.koid().unwrap())
            });
            tx.send(thread_handle).unwrap();
            let event = event_for_blocked_thread;
            let guard = event.begin_wait();
            guard.block_until(None, zx::MonotonicInstant::INFINITE).unwrap();
        });

        // Wait until the thread starts sleeping on the primary futex in the kernel.
        let blocked_thread_handle = rx.recv().unwrap();
        while blocked_thread_handle.info().unwrap().state
            != zx::ThreadState::Blocked(zx::ThreadBlockType::Futex)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Dynamically assign PI ownership to root_thread_handle.
        event.assign_new_owner(&root_thread_handle).unwrap();

        // Wait for the correct requeue owner to appear.
        while event.get_owner() != Some(root_thread_koid) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        event.notify();
        blocked_thread.join().unwrap();
    }

    #[test]
    fn concurrent_wait_wake_assign_new_owner_stress_test() {
        let (root_thread_handle, _) = fuchsia_runtime::with_thread_self(|thread| {
            (thread.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(), thread.koid().unwrap())
        });

        for _ in 0..200 {
            let event = Arc::new(InterruptibleEvent::new());
            let barrier = Arc::new(std::sync::Barrier::new(2));

            let event_waiter = event.clone();
            let event_assign = event.clone();
            let event_waker = event.clone();
            let barrier_assign = barrier.clone();
            let barrier_waker = barrier.clone();
            let thread_handle =
                root_thread_handle.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();

            let waiter = std::thread::spawn(move || {
                let guard = event_waiter.begin_wait();
                let _ = guard.block_until(None, zx::MonotonicInstant::INFINITE);
            });

            while event.futex.load(Ordering::Relaxed) != WAITING {
                std::thread::yield_now();
            }

            let assigner = std::thread::spawn(move || {
                barrier_assign.wait();
                let _ = event_assign.assign_new_owner(&thread_handle);
            });

            let waker = std::thread::spawn(move || {
                barrier_waker.wait();
                event_waker.notify();
            });

            assigner.join().unwrap();
            waker.join().unwrap();
            waiter.join().unwrap();
        }
    }
}
