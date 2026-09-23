// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod scope;

pub mod task {
    use core::task::{Context, Poll};
    use std::future::Future;
    use std::mem::ManuallyDrop;
    use std::pin::Pin;
    use tokio::task::AbortHandle;
    pub use tokio::task::yield_now;

    use futures::FutureExt;

    // NOTE: This isn't quite API equivalent to Fuchsia's JoinHandle.
    pub use tokio::task::{JoinHandle, JoinHandle as CancelableJoinHandle};

    impl<T> From<JoinHandle<T>> for Task<T> {
        fn from(task: JoinHandle<T>) -> Self {
            Self { task, abort_on_drop: true }
        }
    }

    /// A handle to a task.
    ///
    /// A task can be polled for the output of the future it is executing. A
    /// dropped task will be cancelled after dropping. To immediately cancel a
    /// task, call the cancel() method. To run a task to completion without
    /// retaining the Task handle, call the detach() method.
    #[derive(Debug)]
    pub struct Task<T> {
        task: JoinHandle<T>,
        abort_on_drop: bool,
    }

    impl<T> Task<T> {
        /// Returns a `JoinHandle` which will have detach-on-drop semantics.
        pub fn detach_on_drop(self) -> JoinHandle<T> {
            let this = ManuallyDrop::new(self);
            // SAFETY: We are bypassing our drop implementation.
            unsafe { std::ptr::read(&this.task) }
        }
    }

    impl<T: 'static> Task<T> {
        /// Spawn a new `Send` task onto the current executor.
        ///
        /// # Panics
        ///
        /// `spawn` may panic if not called in the context of an executor (e.g.
        /// within a call to `run` or `run_singlethreaded`).
        pub fn spawn(fut: impl Future<Output = T> + Send + 'static) -> Self
        where
            T: Send,
        {
            let task = tokio::task::spawn(fut);
            Self { task, abort_on_drop: true }
        }

        /// Spawn a new non-`Send` task onto the single threaded executor.
        ///
        /// # Panics
        ///
        /// `local` may panic if not called in the context of a local executor
        /// (e.g. within a call to `run` or `run_singlethreaded`).
        pub fn local(fut: impl Future<Output = T> + 'static) -> Self {
            let task = tokio::task::spawn_local(fut);
            Self { task, abort_on_drop: true }
        }

        /// detach the Task handle. The contained future will be polled until completion.
        pub fn detach(mut self) {
            self.abort_on_drop = false;
        }

        /// Abort a task and returns a future that resolves once the task is
        /// aborted. The future can be ignored in which case the task will still
        /// be aborted.
        pub fn abort(mut self) -> impl Future<Output = Option<T>> {
            self.task.abort();
            async move {
                let res = (&mut self.task).await;

                match res {
                    Ok(value) => Some(value),
                    Err(err) => {
                        if err.is_panic() {
                            // Propagate panic
                            std::panic::resume_unwind(err.into_panic());
                        }
                        None
                    }
                }
            }
        }

        pub(crate) fn abort_handle(&self) -> AbortHandle {
            self.task.abort_handle()
        }
    }

    impl<T> Future for Task<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            use futures_lite::FutureExt;
            match self.task.poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Err(err)) => {
                    if err.is_panic() {
                        // Propagate panic
                        std::panic::resume_unwind(err.into_panic());
                    } else {
                        // All codepaths for canceling/aborting a task consume said task.
                        // It will not be polled afterwards, and if it is there's something very
                        // wrong going on.
                        unreachable!("Task was polled after being cancelled");
                    }
                }
                Poll::Ready(Ok(v)) => Poll::Ready(v),
            }
        }
    }

    impl<T> Drop for Task<T> {
        fn drop(&mut self) {
            if self.abort_on_drop {
                self.task.abort();
            }
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
    /// NOTE: Synchronous functions cannot be cancelled and may keep running after
    /// the returned future is dropped. As a result, resources held by the function
    /// should be assumed to be held until the returned future completes.
    ///
    /// For details on performance characteristics and edge cases, see [`blocking::unblock`].
    pub fn unblock<T: 'static + Send>(
        f: impl 'static + Send + FnOnce() -> T,
    ) -> impl 'static + Send + Future<Output = T> {
        tokio::task::spawn_blocking(f).map(|res| res.unwrap())
    }
}

pub mod executor {
    use crate::Timer;
    use crate::runtime::WakeupTime;
    use futures::future::BoxFuture;
    use std::future::Future;
    use std::ops::{Deref, DerefMut};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    pub use std::time::Duration as MonotonicDuration;
    /// A time relative to the executor's clock.
    pub use std::time::Instant as MonotonicInstant;
    use tokio::runtime::{EnterGuard, Runtime};
    use tokio::task::{LocalEnterGuard, LocalSet};

    impl WakeupTime for MonotonicInstant {
        fn into_timer(self) -> Timer {
            Timer::from(self)
        }
    }

    /// A tokio runtime with an active [`EnterGuard`].
    ///
    /// This type allows [`SendExecutor`] and [`LocalExecutor`] to behave like
    /// the Fuchsia implementation: the runtime is in scope for the local thread
    /// from the executor's creation.
    struct GuardedRuntime {
        // Drop order matters. We have transmuted the EnterGuard into a static
        // lifetime, so we must drop it before the runtime.
        _guard: EnterGuard<'static>,
        runtime: Pin<Box<Runtime>>,
    }

    impl GuardedRuntime {
        fn new(runtime: Runtime) -> Self {
            let runtime = Box::pin(runtime);
            let guard = runtime.enter();
            // SAFETY: We're transmuting the lifecycle here. We guarantee that
            // EnterGuard is dropped before the runtime in GuardedRuntime. The
            // runtime is pinned, so any references kept by the guard remain
            // valid.
            let guard =
                unsafe { std::mem::transmute::<EnterGuard<'_>, EnterGuard<'static>>(guard) };
            Self { _guard: guard, runtime }
        }
    }

    /// A multi-threaded executor.
    ///
    /// Mostly API-compatible with the Fuchsia variant.  This differs from Fuchsia in one important
    /// regard: tasks can only be spawned whilst the executor is running, whereas Fuchsia will allow
    /// you to spawn tasks before the executor is running.  LocalExecutor does not have this
    /// limitation.
    ///
    /// The current implementation of Executor does not isolate work (as the underlying executor is
    /// not yet capable of this).
    pub struct SendExecutor {
        runtime: GuardedRuntime,
    }

    impl SendExecutor {
        /// Create a new executor running with actual time.
        pub(crate) fn new(num_threads: u8) -> Self {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(num_threads.into())
                .enable_time()
                .enable_io()
                .build()
                .expect("Could not start tokio runtime on current thread");
            Self { runtime: GuardedRuntime::new(rt) }
        }

        /// Run a single future to completion using multiple threads.
        pub fn run<F>(&mut self, main_future: F) -> F::Output
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            self.runtime.runtime.block_on(main_future)
        }
    }

    /// A builder for `SendExecutor`.
    #[derive(Default)]
    pub struct SendExecutorBuilder {
        num_threads: Option<u8>,
    }

    impl SendExecutorBuilder {
        /// Creates a new builder used for constructing a `SendExecutor`.
        pub fn new() -> Self {
            Self::default()
        }

        /// Sets the number of threads for the executor.
        pub fn num_threads(mut self, num_threads: u8) -> Self {
            self.num_threads = Some(num_threads);
            self
        }

        /// Sets whether the executor should support binding interrupts.
        pub fn allow_interrupts(self, #[expect(unused)] allow_interrupts: bool) -> Self {
            // Interrupt binding is not supported on this platform, so we ignore it.
            self
        }

        /// Builds the `SendExecutor`, consuming this `SendExecutorBuilder`.
        pub fn build(self) -> SendExecutor {
            SendExecutor::new(self.num_threads.unwrap_or(1))
        }
    }

    /// A single-threaded executor.
    ///
    /// API-compatible with the Fuchsia variant with the exception of testing APIs.
    ///
    /// The current implementation of Executor does not isolate work
    /// (as the underlying executor is not yet capable of this).
    pub struct LocalExecutor {
        // Drop order matters here. Drop the guard before the runtime, and the
        // local guard after the local set.
        local_set: LocalSet,
        _local_guard: LocalEnterGuard,
        runtime: GuardedRuntime,
    }

    impl Default for LocalExecutor {
        fn default() -> Self {
            Self::new()
        }
    }

    impl LocalExecutor {
        /// Create a new executor.
        pub fn new() -> Self {
            let local_set = LocalSet::new();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .expect("Could not start tokio runtime on current thread");
            let runtime = GuardedRuntime::new(runtime);
            let local_guard = local_set.enter();
            Self { local_set, _local_guard: local_guard, runtime }
        }

        /// Run a single future to completion on a single thread.
        pub fn run_singlethreaded<F>(&mut self, main_future: F) -> F::Output
        where
            F: Future,
        {
            self.local_set.block_on(&self.runtime.runtime, main_future)
        }
    }

    /// A builder for `LocalExecutor`.
    #[derive(Default)]
    pub struct LocalExecutorBuilder {}

    impl LocalExecutorBuilder {
        /// Creates a new builder used for constructing a `LocalExecutor`.
        pub fn new() -> Self {
            Self::default()
        }

        /// Sets whether the executor should support binding interrupts.
        pub fn allow_interrupts(self, #[expect(unused)] allow_interrupts: bool) -> Self {
            // Interrupt binding is not supported on this platform, so we ignore it.
            self
        }

        /// Builds the `LocalExecutor`, consuming this `LocalExecutorBuilder`.
        pub fn build(self) -> LocalExecutor {
            LocalExecutor::default()
        }
    }

    /// A single-threaded executor for testing.
    ///
    /// The current implementation of Executor does not isolate work
    /// (as the underlying executor is not yet capable of this).
    pub struct TestExecutor {
        executor: LocalExecutor,
    }

    impl Default for TestExecutor {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestExecutor {
        /// Create a new executor for testing.
        pub fn new() -> Self {
            Self { executor: LocalExecutor::default() }
        }
    }

    /// A builder for `TestExecutor`.
    #[derive(Default)]
    pub struct TestExecutorBuilder {}

    impl TestExecutorBuilder {
        /// Creates a new builder used for constructing a `TestExecutor`.
        pub fn new() -> Self {
            Self::default()
        }

        /// Sets whether the executor should support binding interrupts.
        pub fn allow_interrupts(self, #[expect(unused)] allow_interrupts: bool) -> Self {
            // Interrupt binding is not supported on this platform, so we ignore it.
            self
        }

        /// Builds the `TestExecutor`, consuming this `TestExecutorBuilder`.
        pub fn build(self) -> TestExecutor {
            TestExecutor::new()
        }
    }

    impl Deref for TestExecutor {
        type Target = LocalExecutor;

        fn deref(&self) -> &Self::Target {
            &self.executor
        }
    }

    impl DerefMut for TestExecutor {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.executor
        }
    }

    /// On portable runtimes, SpawnableFuture is the same as a boxed future and lacks any memory
    /// saving optimisations.
    pub struct SpawnableFuture<'a, O>(BoxFuture<'a, O>);

    impl<'a, O> SpawnableFuture<'a, O> {
        /// Returns a new SpawnableFuture.
        pub fn new(future: impl Future<Output = O> + Send + 'a) -> Self {
            Self(Box::pin(future))
        }
    }

    impl<O> Future for SpawnableFuture<'_, O> {
        type Output = O;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(&mut self.0).poll(cx)
        }
    }
}

pub mod timer {
    use crate::WakeupTime;
    use futures::prelude::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    #[derive(Debug)]
    #[must_use = "futures do nothing unless polled"]
    /// An asynchronous timer.
    pub struct Timer {
        inner: Option<Pin<Box<tokio::time::Sleep>>>,
        deadline: std::time::Instant,
    }

    impl Timer {
        /// Create a new timer scheduled to fire at `time`.
        pub fn new(time: impl WakeupTime) -> Self {
            time.into_timer()
        }
    }

    impl From<std::time::Duration> for Timer {
        fn from(duration: std::time::Duration) -> Self {
            Self::from(crate::MonotonicInstant::now() + duration)
        }
    }

    impl From<crate::MonotonicInstant> for Timer {
        fn from(instant: crate::MonotonicInstant) -> Self {
            Timer { inner: None, deadline: instant }
        }
    }

    impl Future for Timer {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.inner.is_none() {
                self.inner = Some(Box::pin(tokio::time::sleep_until(self.deadline.into())))
            }
            self.inner.as_mut().unwrap().poll_unpin(cx).map(drop)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::executor::{LocalExecutorBuilder, SendExecutorBuilder};
    use super::task::Task;

    struct SpawnOnDrop(bool);

    impl Drop for SpawnOnDrop {
        fn drop(&mut self) {
            let fut = async move {
                panic!("should not run on drop");
            };
            if self.0 {
                Task::local(fut).detach();
            } else {
                Task::spawn(fut).detach();
            }
        }
    }

    #[test]
    fn local_exec_spawn_local_on_drop() {
        let exec = LocalExecutorBuilder::new().build();
        let bomb = SpawnOnDrop(true);
        Task::local(async move { drop(bomb) }).detach();
        drop(exec);
    }

    #[test]
    fn local_exec_spawn_on_drop() {
        let exec = LocalExecutorBuilder::new().build();
        let bomb = SpawnOnDrop(false);
        Task::spawn(async move { drop(bomb) }).detach();
        drop(exec);
    }

    #[test]
    fn send_exec_spawn_on_drop() {
        let exec = SendExecutorBuilder::new().num_threads(2).build();
        let bomb = SpawnOnDrop(false);
        Task::spawn(async move { drop(bomb) }).detach();
        drop(exec);
    }
}
