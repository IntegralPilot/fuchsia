// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::task::{Context, Poll};

use libasync_dispatcher::{AsyncDispatcher, DetectDispatcher, GetAsyncDispatcher};
use libasync_sys::{async_cancel_task, async_dispatcher, async_post_task, async_task};

use futures::task::AtomicWaker;
use zx::Status;
use zx::sys::{ZX_ERR_CANCELED, ZX_OK};

use crate::callback_state::CallbackSharedState;

type SharedState = CallbackSharedState<async_task, AfterDeadlineState>;

/// Implements methods used for setting and waiting on timers on a dispatcher.
pub trait DispatcherTimerExt {
    /// Returns a future that will fire when after the given deadline time.
    ///
    /// This can be used instead of the fuchsia-async timer primitives in situations where
    /// there isn't a currently active fuchsia-async executor running on that dispatcher for some
    /// reason (ie. the rust code does not own the dispatcher) or for cases where the small overhead
    /// of fuchsia-async compatibility is too much.
    ///
    /// # Panics
    ///
    /// If the dispatcher pointed to by `self` is not available right now (like [`CurrentDispatcher`])
    /// on a thread with no current dispatcher set, this function will panic. You can use
    /// [`Self::try_after_deadline`] to handle the condition where there is no current dispatcher,
    /// or if you're trying to run it on the current dispatcher you may want to use [`AfterDeadline::new`]
    /// instead.
    fn after_deadline(&self, deadline: zx::MonotonicInstant) -> AfterDeadline;

    /// Returns a future that will fire when after the given deadline time.
    ///
    /// This can be used instead of the fuchsia-async timer primitives in situations where
    /// there isn't a currently active fuchsia-async executor running on that dispatcher for some
    /// reason (ie. the rust code does not own the dispatcher) or for cases where the small overhead
    /// of fuchsia-async compatibility is too much.
    fn try_after_deadline(&self, deadline: zx::MonotonicInstant) -> Option<AfterDeadline>;
}

impl<T> DispatcherTimerExt for T
where
    T: GetAsyncDispatcher,
{
    fn after_deadline(&self, deadline: zx::MonotonicInstant) -> AfterDeadline {
        self.try_after_deadline(deadline).expect("No current dispatcher")
    }

    fn try_after_deadline(&self, deadline: zx::MonotonicInstant) -> Option<AfterDeadline> {
        let dispatcher = self.try_get_async_dispatcher()?;
        Some(AfterDeadline::new_on(dispatcher, deadline))
    }
}

struct AfterDeadlineState {
    async_dispatcher: NonNull<async_dispatcher>,
    waker: AtomicWaker,
    /// The status will initially be [`Status::SHOULD_WAIT`]. Once fired it will be the status
    /// returned by the callback.
    status: AtomicI32,
}

// SAFETY: All fields in AfterDeadlineState are either atomic or immutable.
unsafe impl Send for AfterDeadlineState {}
unsafe impl Sync for AfterDeadlineState {}

impl AfterDeadlineState {
    extern "C" fn call(_dispatcher: *mut async_dispatcher, task: *mut async_task, status: i32) {
        debug_assert!(
            status == ZX_OK || status == ZX_ERR_CANCELED,
            "task callback called with status other than ok or canceled"
        );
        // SAFETY: This callback's copy of the `async_task` object was refcounted for when we
        // started the wait.
        let state = unsafe { SharedState::from_raw_ptr(task) };
        state.status.store(status, Ordering::Relaxed);
        state.waker.wake();
    }
}

/// A future that represents a deferral to a future time.
///
/// See [`OnDispatcher::after_deadline`] for more information.
pub struct AfterDeadline {
    dispatcher: DetectDispatcher,
    state: Option<Arc<SharedState>>,
    deadline: zx::MonotonicInstant,
}

impl AfterDeadline {
    /// Creates a new timer object that will fire when the deadline has passed.
    ///
    /// This will get the current dispatcher on first poll. If you want to run it against a
    /// specific dispatcher, use [`DispatcherTimerExt::after_deadline`].
    pub fn new(deadline: zx::MonotonicInstant) -> Self {
        let state = None;
        let dispatcher = DetectDispatcher::default();
        Self { dispatcher, state, deadline }
    }

    fn new_on(dispatcher: AsyncDispatcher, deadline: zx::MonotonicInstant) -> Self {
        let state = None;
        let dispatcher = DetectDispatcher::new(dispatcher);
        Self { dispatcher, state, deadline }
    }
}

impl Future for AfterDeadline {
    type Output = Result<(), Status>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // if we didn't have a dispatcher when the future was created, return BAD_STATE.
        let dispatcher = self.dispatcher.get_or_detect()?;

        // if we've already spawned a task then return based on the task's state.
        if let Some(state) = &self.state {
            let status = state.status.load(Ordering::Relaxed);
            if status != Status::SHOULD_WAIT.into_raw() {
                return Poll::Ready(Status::ok(status));
            } else {
                state.waker.register(cx.waker());
                return Poll::Pending;
            }
        }

        let deadline = self.deadline;
        let now = dispatcher.now();
        if deadline < zx::MonotonicInstant::from_nanos(now) {
            return Poll::Ready(Ok(()));
        }

        // otherwise we want to wait for a callback
        let async_dispatcher = dispatcher.as_ptr();

        let task = async_task {
            handler: Some(AfterDeadlineState::call),
            deadline: deadline.into_nanos(),
            ..Default::default()
        };
        let state = AfterDeadlineState {
            async_dispatcher,
            waker: AtomicWaker::new(),
            status: AtomicI32::new(Status::SHOULD_WAIT.into_raw()),
        };
        let state = SharedState::new(task, state);
        state.waker.register(cx.waker());

        let state_ptr = SharedState::make_raw_ptr(state.clone());

        // SAFETY: We know the `async_dispatcher` is valid because we're running inside
        // `on_dispatcher` and we are giving ownership of the shared state object to the
        // callback.
        let res = Status::ok(unsafe { async_post_task(async_dispatcher.as_ptr(), state_ptr) });
        match res {
            Ok(_) => {
                self.state = Some(state);
                Poll::Pending
            }
            Err(err) => {
                // SAFETY: Posting the task failed, so we now have an outstanding reference to
                // the state object that will never have a callback called on it.
                unsafe { SharedState::release_raw_ptr(state_ptr) };
                Poll::Ready(Err(err))
            }
        }
    }
}

impl Drop for AfterDeadline {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            // if we never spawned a task we can just return.
            return;
        };
        let Some(dispatcher) = self.dispatcher.get() else {
            // if we never got a dispatcher or failed to get a dispatcher then we never
            // registered a wait and we can just return.
            return;
        };
        if state.status.load(Ordering::Relaxed) != Status::SHOULD_WAIT.into_raw() {
            // the callback has been called so we don't even need to try to cancel it.
            return;
        }
        let async_dispatcher = dispatcher.as_ptr();
        if async_dispatcher != state.async_dispatcher {
            panic!(
                "Dropping a pending `AfterDeadline` future from a different dispatcher than the one it was awaited on."
            );
        }
        let state_ptr = SharedState::as_raw_ptr(&state);
        // SAFETY: We know that the current async dispatcher is valid because we are running
        // inside `on_dispatcher`, and we know the `state_ptr` is valid because the `Arc`
        // holding it is still held.
        let status = unsafe { async_cancel_task(async_dispatcher.as_ptr(), state_ptr) };
        if Status::ok(status).is_ok() {
            // SAFETY: If the cancellation was successful, we know the callback won't be called
            // so we need to deallocate the copy of the arc that was given to it.
            unsafe { SharedState::release_raw_ptr(state_ptr) };
        }
    }
}

// TODO(528052543): Migrate these to a specifically test-oriented dispatcher when there is one
// so they don't require the driver runtime to be involved.
#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread::sleep;
    use std::time::Duration;

    use super::*;

    use futures::{FutureExt, poll};
    use std::task::Waker;

    use fdf_env::test::spawn_in_driver;
    use libasync_dispatcher::CurrentDispatcher;

    fn now() -> zx::MonotonicInstant {
        zx::MonotonicInstant::from_nanos(CurrentDispatcher.get_async_dispatcher().now())
    }

    #[test]
    fn after_the_past() {
        spawn_in_driver("testing task", async {
            let fut = CurrentDispatcher.after_deadline(zx::MonotonicInstant::INFINITE_PAST);
            assert_eq!(poll!(fut), Poll::Ready(Ok(())));
        });
    }

    #[test]
    fn after_now() {
        spawn_in_driver("testing task", async {
            let fut = CurrentDispatcher.after_deadline(now());
            assert_eq!(poll!(fut), Poll::Ready(Ok(())));
        });
    }

    #[test]
    fn after_future() {
        spawn_in_driver("testing task", async {
            let deadline = now() + zx::MonotonicDuration::from_seconds(3);
            let mut fut = CurrentDispatcher.after_deadline(deadline);
            assert_eq!(poll!(&mut fut), Poll::Pending);
            assert!(fut.await.is_ok());
            assert!(now() >= deadline);
        });
    }

    #[test]
    fn drop_after_poll() {
        spawn_in_driver("testing task", async {
            let deadline = now() + zx::MonotonicDuration::from_minutes(3);
            let mut fut = CurrentDispatcher.after_deadline(deadline);
            assert_eq!(poll!(&mut fut), Poll::Pending);
        });
    }

    #[test]
    fn dispatcher_shutdown_cancel() {
        let (fut_tx, fut_rx) = mpsc::channel();
        spawn_in_driver("testing task", async move {
            let deadline = now() + zx::MonotonicDuration::from_minutes(3);
            let mut fut = CurrentDispatcher.after_deadline(deadline);
            assert_eq!(poll!(&mut fut), Poll::Pending);
            fut_tx.send(fut).unwrap();
        });
        let mut fut = fut_rx.recv().unwrap();
        loop {
            let Poll::Ready(res) = fut.poll_unpin(&mut Context::from_waker(Waker::noop())) else {
                sleep(Duration::from_millis(10));
                continue;
            };
            assert_eq!(res, Err(Status::CANCELED));
            break;
        }
    }
}
