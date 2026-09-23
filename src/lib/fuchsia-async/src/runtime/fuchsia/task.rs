// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::runtime::fuchsia::executor::TaskHandle;
use crate::runtime::fuchsia::scope::JoinError;
use crate::scope::ScopeHandle;
use crate::{EHandle, MonotonicDuration, MonotonicInstant, Timer};
use futures::prelude::*;
use std::future::poll_fn;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A handle to a future that is owned and polled by the executor.
///
/// Once a task is created, the executor will poll it until done, even if the task handle itself is
/// not polled.
///
/// NOTE: When a JoinHandle is dropped, its future will be detached.
///
/// Polling (or attempting to extract the value from) a task after the executor is dropped may
/// trigger a panic.
#[derive(Debug)]
// LINT.IfChange
pub struct JoinHandle<T> {
    scope: ScopeHandle,
    task: Option<TaskHandle>,
    phantom: PhantomData<T>,
}
// LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)

impl<T> Unpin for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    pub(crate) fn new(scope: ScopeHandle, task: TaskHandle) -> Self {
        Self { scope, task: Some(task), phantom: PhantomData }
    }

    /// Aborts a task and returns a future that resolves once the task is
    /// aborted. The future can be ignored in which case the task will still be
    /// aborted.
    pub fn abort(mut self) -> impl Future<Output = Option<T>> {
        // SAFETY: We spawned the task so the return type should be correct.
        let result = self.task.as_ref().and_then(|t| unsafe { self.scope.abort_task(t) });
        // TODO(https://fxbug.dev/452064816): The compiler throws a false
        // positive linter warning because it thinks that `self.task = None;` is
        // never read, even though it is read in the Drop implementation below.
        #[allow(unused_assignments)]
        async move {
            match result {
                Some(output) => Some(output),
                None => {
                    // If we are dropped from here, we'll end up calling `abort_and_detach`.
                    let result = std::future::poll_fn(|cx| {
                        let Some(task) = self.task.as_ref() else {
                            return Poll::Ready(None);
                        };
                        // SAFETY: We spawned the task so the return type should be correct.
                        unsafe { self.scope.poll_aborted(task, cx) }
                    })
                    .await;
                    self.task = None;
                    result
                }
            }
        }
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            self.scope.detach(task);
        }
    }
}

impl<T: 'static> Future for JoinHandle<T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We spawned the task so the return type should be correct.
        let result = unsafe { self.scope.poll_join_result(self.task.as_ref().unwrap(), cx) };
        if result.is_ready() {
            self.task = None;
        }
        result
    }
}

/// A `JoinHandle` which returns `Err` when canceled instead of pending forever.
#[derive(Debug)]
pub struct CancelableJoinHandle<T> {
    inner: JoinHandle<T>,
}

impl<T> CancelableJoinHandle<T> {
    /// Aborts a task and returns a future that resolves once the task is
    /// aborted. The future can be ignored in which case the task will still be
    /// aborted.
    pub fn abort(self) -> impl Future<Output = Option<T>> {
        self.inner.abort()
    }
}

impl<T> From<JoinHandle<T>> for CancelableJoinHandle<T> {
    fn from(inner: JoinHandle<T>) -> Self {
        Self { inner }
    }
}

impl<T: 'static> Future for CancelableJoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We spawned the task so the return type should be correct.
        let result =
            unsafe { self.inner.scope.try_poll_join_result(self.inner.task.as_ref().unwrap(), cx) };
        if result.is_ready() {
            self.inner.task = None;
        }
        result
    }
}

/// This is the same as a JoinHandle, except that the future will be aborted when the task is
/// dropped.
#[must_use]
#[repr(transparent)]
#[derive(Debug)]
// LINT.IfChange
pub struct Task<T>(JoinHandle<T>);
// LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)

impl<T> Task<T> {
    /// Returns a `JoinHandle` which will have detach-on-drop semantics.
    pub fn detach_on_drop(self) -> JoinHandle<T> {
        let this = ManuallyDrop::new(self);
        // SAFETY: We are bypassing our drop implementation.
        unsafe { std::ptr::read(&this.0) }
    }
}

impl Task<()> {
    /// Detach this task so that it can run independently in the background.
    ///
    /// *Note*: This is usually not what you want. This API severs the control flow from the
    /// caller. This can result in flaky tests and makes it impossible to return values
    /// (including errors).
    ///
    /// If your goal is to run multiple tasks concurrently, use [`Scope`][crate::Scope].
    ///
    /// You can also use other futures combinators such as:
    ///
    /// * [`futures::future::join`]
    /// * [`futures::future::select`]
    /// * [`futures::select`]
    ///
    /// or their error-aware variants
    ///
    /// * [`futures::future::try_join`]
    /// * [`futures::future::try_select`]
    ///
    /// or their stream counterparts
    ///
    /// * [`futures::stream::StreamExt::for_each`]
    /// * [`futures::stream::StreamExt::for_each_concurrent`]
    /// * [`futures::stream::TryStreamExt::try_for_each`]
    /// * [`futures::stream::TryStreamExt::try_for_each_concurrent`]
    ///
    /// can meet your needs.
    pub fn detach(mut self) {
        self.0.scope.detach(self.0.task.as_ref().unwrap());
        self.0.task = None;
    }
}

impl<T: Send + 'static> Task<T> {
    /// Spawn a new task on the global scope of the current executor.
    ///
    /// The task may be executed on any thread(s) owned by the current executor.
    /// See [`Task::local`] for an equivalent that ensures locality.
    ///
    /// The passed future will live until either (a) the future completes,
    /// (b) the returned [`Task`] is dropped while the executor is running, or
    /// (c) the executor is destroyed; whichever comes first.
    ///
    /// Code that uses scopes is encouraged to spawn on a shorter lived scope or
    /// explicitly call [`Scope::global()`][crate::Scope::global] for spawning.
    ///
    /// # Panics
    ///
    /// May panic if not called in the context of an executor (e.g. within a
    /// call to [`run`][crate::SendExecutor::run]).
    pub fn spawn(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
        EHandle::local().global_scope().compute(future)
    }
}

impl<T: 'static> Task<T> {
    /// Spawn a new task on the global scope of the thread local executor.
    ///
    /// The passed future will live until either (a) the future completes,
    /// (b) the returned [`Task`] is dropped while the executor is running, or
    /// (c) the executor is destroyed; whichever comes first.
    ///
    /// NOTE: This is not supported with a [`SendExecutor`] and will cause a
    /// runtime panic. Use [`Task::spawn`] instead.
    ///
    /// Code that uses scopes is encouraged to spawn on a shorter lived scope or
    /// explicitly call [`Scope::global()`][crate::Scope::global] for spawning.
    ///
    /// # Panics
    ///
    /// May panic if not called in the context of an executor (e.g. within a
    /// call to [`run`][crate::SendExecutor::run]).
    pub fn local(future: impl Future<Output = T> + 'static) -> Task<T> {
        EHandle::local().global_scope().compute_local(future)
    }
}

impl<T: 'static> Task<T> {
    /// Aborts a task and returns a future that resolves once the task is
    /// aborted. The future can be ignored in which case the task will still be
    /// aborted.
    pub fn abort(self) -> impl Future<Output = Option<T>> {
        self.detach_on_drop().abort()
    }
}

impl<T: 'static> Future for Task<T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We spawned the task so the return type should be correct.
        let result = unsafe { self.0.scope.poll_join_result(self.0.task.as_ref().unwrap(), cx) };
        if result.is_ready() {
            self.0.task = None;
        }
        result
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.0.task {
            self.0.scope.abort_and_detach(task);
            self.0.task = None;
        }
    }
}

impl<T> From<JoinHandle<T>> for Task<T> {
    fn from(value: JoinHandle<T>) -> Self {
        Self(value)
    }
}

/// Offload a blocking function call onto a different thread.
///
/// This function can be called from an asynchronous function without blocking
/// it, returning a future that can be `.await`ed normally. The provided
/// function should contain at least one blocking operation, such as:
///
/// - A synchronous syscall that does not yet have an async counterpart.
/// - A compute operation which risks blocking the executor for an unacceptable
///   amount of time.
///
/// If neither of these conditions are satisfied, just call the function normally,
/// as synchronous functions themselves are allowed within an async context,
/// as long as they are not blocking.
///
/// If you have an async function that may block, refactor the function such that
/// the blocking operations are offloaded onto the function passed to [`unblock`].
///
/// NOTE:
///
/// - The input function should not interact with the executor. Attempting to do so
///   can cause runtime errors. This includes spawning, creating new executors,
///   passing futures between the input function and the calling context, and
///   in some cases constructing async-aware types (such as IO-, IPC- and timer objects).
/// - Synchronous functions cannot be cancelled and may keep running after
///   the returned future is dropped. As a result, resources held by the function
///   should be assumed to be held until the returned future completes.
/// - This function assumes panic=abort semantics, so if the input function panics,
///   the process aborts. Behavior for panic=unwind is not defined.
// TODO(https://fxbug.dev/42158447): Consider using a backing thread pool to alleviate the cost of
// spawning new threads if this proves to be a bottleneck.
pub fn unblock<T: 'static + Send>(
    f: impl 'static + Send + FnOnce() -> T,
) -> impl 'static + Send + Future<Output = T> {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.map(|r| r.unwrap())
}

/// Yields execution back to the runtime.
pub async fn yield_now() {
    let mut done = false;
    poll_fn(|cx| {
        if done {
            Poll::Ready(())
        } else {
            done = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// LowPriorityTask is to be used for tasks that are low priority.  Low priority tasks should
/// periodically call `wait_until_idle` which will wait until no other normal priority tasks have
/// been running for the specified tasks.  A normal priority task is any task that isn't a low
/// priority one (due to creating an instance of `LowPriorityTask`).
pub struct LowPriorityTask(TaskHandle);

impl Default for LowPriorityTask {
    /// See `LowPriorityTask::new()`.
    fn default() -> Self {
        Self::new()
    }
}

impl LowPriorityTask {
    /// Marks the current task as a low priority task which means that it won't count as activity
    /// that `wait_until_idle` will respect.
    ///
    /// # Panics
    ///
    /// This will panic if there is no task currently running or if the task is already marked
    /// as a low priority task.
    pub fn new() -> Self {
        let handle = TaskHandle::with_current(|handle| handle.unwrap().clone());
        assert!(!handle.set_low_priority(true));
        Self(handle)
    }

    /// Waits until the executor has been idle for `period` i.e. no normal priority tasks have been
    /// polled for `period`.  `deadline` is the limit for how long this will wait.
    pub async fn wait_until_idle_for(&self, period: MonotonicDuration, deadline: MonotonicInstant) {
        let executor = self.0.scope().executor();
        loop {
            let deadline = std::cmp::min(executor.last_active() + period, deadline);
            if executor.now() >= deadline {
                break;
            }
            Timer::new(deadline).await;
        }
    }
}

impl Drop for LowPriorityTask {
    fn drop(&mut self) {
        assert!(self.0.set_low_priority(false));
    }
}

#[cfg(test)]
mod tests {
    use super::super::executor::{
        LocalExecutor, SendExecutorBuilder, TestExecutor, TestExecutorBuilder,
    };
    use super::*;
    use fuchsia_sync::Mutex;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// This struct holds a thread-safe mutable boolean and
    /// sets its value to true when dropped.
    #[derive(Clone)]
    struct SetsBoolTrueOnDrop {
        value: Arc<Mutex<bool>>,
    }

    impl SetsBoolTrueOnDrop {
        fn new() -> (Self, Arc<Mutex<bool>>) {
            let value = Arc::new(Mutex::new(false));
            let sets_bool_true_on_drop = Self { value: value.clone() };
            (sets_bool_true_on_drop, value)
        }
    }

    impl Drop for SetsBoolTrueOnDrop {
        fn drop(&mut self) {
            let mut lock = self.value.lock();
            *lock = true;
        }
    }

    #[test]
    #[should_panic]
    fn spawn_from_unblock_fails() {
        // no executor in the off-thread, so spawning fails
        SendExecutorBuilder::new().num_threads(2).build().run(async move {
            unblock(|| {
                #[allow(clippy::let_underscore_future)]
                let _ = Task::spawn(async {});
            })
            .await;
        });
    }

    #[test]
    fn future_destroyed_before_await_returns() {
        LocalExecutor::default().run_singlethreaded(async {
            let (sets_bool_true_on_drop, value) = SetsBoolTrueOnDrop::new();

            // Move the switch into a different thread.
            // Once we return from this await, that switch should have been dropped.
            unblock(move || {
                let lock = sets_bool_true_on_drop.value.lock();
                assert!(!*lock);
            })
            .await;

            // Switch moved into the future should have been dropped at this point.
            // The value of the boolean should now be true.
            let lock = value.lock();
            assert!(*lock);
        });
    }

    #[test]
    fn test_low_priority_task() {
        let mut executor = TestExecutorBuilder::new().fake_time(true).build();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));
        assert!(
            executor
                .run_until_stalled(&mut pin!(async {
                    // We want this main future to be a low priority task so that it doesn't
                    // interfere with what we're trying to test.
                    let _low = LowPriorityTask::new();

                    let stop = Arc::new(AtomicBool::new(false));
                    let stop_clone = stop.clone();
                    let normal_task = Task::spawn(async move {
                        loop {
                            Timer::new(MonotonicInstant::after(MonotonicDuration::from_millis(1)))
                                .await;
                            if stop_clone.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    });

                    let mut low_priority_task = Task::spawn(async move {
                        let low = LowPriorityTask::new();
                        // Wait for 10ms of idle.  The normal task is active every 1ms, so this
                        // should not finish until we stop the normal task.
                        low.wait_until_idle_for(
                            MonotonicDuration::from_millis(10),
                            MonotonicInstant::after(MonotonicDuration::from_seconds(100)),
                        )
                        .await;
                    });

                    // Run for a bit.
                    for _ in 0..50 {
                        TestExecutor::advance_to(MonotonicInstant::after(
                            MonotonicDuration::from_millis(1),
                        ))
                        .await;
                        assert!(futures::poll!(&mut low_priority_task).is_pending());
                    }

                    stop.store(true, Ordering::Relaxed);

                    // Run until normal_task finishes.
                    TestExecutor::advance_to(MonotonicInstant::after(
                        MonotonicDuration::from_millis(1),
                    ))
                    .await;
                    normal_task.await;

                    // Now that the normal task has stopped, the low priority task should finish
                    // after 10ms.
                    TestExecutor::advance_to(MonotonicInstant::after(
                        MonotonicDuration::from_millis(10),
                    ))
                    .await;
                    low_priority_task.await;
                }))
                .is_ready()
        );
    }

    #[test]
    fn test_low_priority_task_deadline() {
        let mut executor = TestExecutorBuilder::new().fake_time(true).build();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));
        assert!(
            executor
                .run_until_stalled(&mut pin!(async {
                    // We want this main future to be a low priority task so that it doesn't
                    // interfere with what we're trying to test.
                    let _low = LowPriorityTask::new();

                    let stop = Arc::new(AtomicBool::new(false));
                    let stop_clone = stop.clone();
                    let _normal_task = Task::spawn(async move {
                        loop {
                            Timer::new(MonotonicInstant::after(MonotonicDuration::from_millis(1)))
                                .await;
                            if stop_clone.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    });

                    let mut low_priority_task = Task::spawn(async move {
                        let low = LowPriorityTask::new();
                        // Wait for 10ms of idle, with a deadline of 50ms.  The normal task is
                        // active every 1ms, so this should not reach the idle period, but it should
                        // finish when the deadline is reached.
                        low.wait_until_idle_for(
                            MonotonicDuration::from_millis(10),
                            MonotonicInstant::after(MonotonicDuration::from_millis(50)),
                        )
                        .await;
                    });

                    // Run for 49ms.
                    for _ in 0..49 {
                        TestExecutor::advance_to(MonotonicInstant::after(
                            MonotonicDuration::from_millis(1),
                        ))
                        .await;
                        assert!(futures::poll!(&mut low_priority_task).is_pending());
                    }

                    // Advance to 50ms.  The low priority task should finish now because of the
                    // deadline.
                    TestExecutor::advance_to(MonotonicInstant::after(
                        MonotonicDuration::from_millis(1),
                    ))
                    .await;
                    low_priority_task.await;

                    stop.store(true, Ordering::Relaxed);
                }))
                .is_ready()
        );
    }
}
