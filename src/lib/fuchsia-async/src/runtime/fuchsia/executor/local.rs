// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::atomic_future::AtomicFutureHandle;
use super::common::{EHandle, Executor, ExecutorTime, TaskHandle};
use super::scope::ScopeHandle;
use super::time::{BootInstant, MonotonicInstant};
use zx::BootDuration;

use crate::runtime::instrument::TaskInstrument;
use futures::future::{self, Either};
use futures::task::AtomicWaker;
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::task::{Context, Poll};

/// A single-threaded port-based executor for Fuchsia.
///
/// Having a `LocalExecutor` in scope allows the creation and polling of zircon objects, such as
/// [`fuchsia_async::Channel`].
///
/// # Panics
///
/// `LocalExecutor` will panic on drop if any zircon objects attached to it are still alive. In
/// other words, zircon objects backed by a `LocalExecutor` must be dropped before it.
pub struct LocalExecutor {
    // LINT.IfChange
    /// The inner executor state.
    pub(crate) ehandle: EHandle,
    // LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)
}

impl fmt::Debug for LocalExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalExecutor").field("port", &self.ehandle.inner().port).finish()
    }
}

impl Default for LocalExecutor {
    /// Create a new single-threaded executor running with actual time.
    fn default() -> Self {
        Self::new_with_port(zx::Port::create(), None)
    }
}

impl LocalExecutor {
    /// Create a new single-threaded executor running with actual time, with a port
    /// and instrumentation.
    pub(crate) fn new_with_port(
        port: zx::Port,
        instrument: Option<Arc<dyn TaskInstrument>>,
    ) -> Self {
        let inner = Arc::new(Executor::new_with_port(
            ExecutorTime::RealTime,
            /* is_local */ true,
            /* num_threads */ 1,
            port,
            instrument,
        ));
        let root_scope = ScopeHandle::root(inner);
        Executor::set_local(root_scope.clone());
        Self { ehandle: EHandle { root_scope } }
    }

    /// Get a reference to the Fuchsia `zx::Port` being used to listen for events.
    pub fn port(&self) -> &zx::Port {
        self.ehandle.port()
    }

    /// Run a single future to completion on a single thread, also polling other active tasks.
    pub fn run_singlethreaded<F>(&mut self, main_future: F) -> F::Output
    where
        F: Future,
    {
        assert!(
            self.ehandle.inner().is_real_time(),
            "Error: called `run_singlethreaded` on an executor using fake time"
        );

        let Poll::Ready(result) = self.run(main_future, |executor, task| {
            executor.ehandle.inner().worker_lifecycle::</*UNTIL_STALLED=*/false>(Some(task));
            // SAFETY: This is the correct type for the main future.
            unsafe { executor.poll_join_result(task) }
        }) else {
            unreachable!()
        };
        result
    }

    fn run<Fut: Future>(
        &mut self, // &mut ensures exclusive access
        main_future: Fut,
        runner: impl FnOnce(&Self, &TaskHandle) -> Poll<Fut::Output>,
    ) -> Poll<Fut::Output> {
        /// # Safety
        ///
        /// See the comment below.
        unsafe fn remove_lifetime(obj: AtomicFutureHandle<'_>) -> TaskHandle {
            unsafe { std::mem::transmute(obj) }
        }

        let scope = &self.ehandle.root_scope;
        let task = scope.new_local_task(main_future);
        // SAFETY: Erasing the lifetime is safe because we make sure to drop the main task within
        // the required lifetime.
        let task_handle = unsafe { remove_lifetime(task) };

        scope.insert_task(task_handle.clone(), false);

        struct DropMainTask<'a>(&'a EHandle, TaskHandle);
        impl Drop for DropMainTask<'_> {
            fn drop(&mut self) {
                // SAFETY: drop_main_tasks requires that the executor isn't running
                // i.e. worker_lifecycle isn't running, which will be the case when this runs.
                unsafe { self.0.inner().drop_main_task(&self.0.root_scope, &self.1) };
            }
        }
        let _drop_main_task = DropMainTask(&self.ehandle, task_handle.clone());

        // Ensure that this object is available for zxdb to find in this stack frame (even if it is
        // inlined). With certain optimization settings, `self` might get optimized such that there
        // is insufficient metadata for the debugger to recover this symbol, which it needs in
        // order to walk the async task tree.
        std::hint::black_box(&self);

        runner(self, &task_handle)
    }

    /// Polls the join result for the task.
    ///
    /// # Safety
    ///
    /// `R` must be the correct type.
    unsafe fn poll_join_result<R>(&self, task: &TaskHandle) -> Poll<R> {
        // SAFETY: See function comment.
        unsafe {
            self.ehandle
                .global_scope()
                .poll_join_result(task, &mut Context::from_waker(std::task::Waker::noop()))
        }
    }

    #[doc(hidden)]
    /// Returns the root scope of the executor.
    pub fn root_scope(&self) -> &ScopeHandle {
        self.ehandle.global_scope()
    }
}

impl Drop for LocalExecutor {
    fn drop(&mut self) {
        self.ehandle.inner().mark_done();
        self.ehandle.inner().on_parent_drop(&self.ehandle.root_scope);
    }
}

/// A builder for `LocalExecutor`.
#[derive(Default)]
pub struct LocalExecutorBuilder {
    port: Option<zx::Port>,
    instrument: Option<Arc<dyn TaskInstrument>>,
    allow_interrupts: bool,
}

impl LocalExecutorBuilder {
    /// Creates a new builder used for constructing a `LocalExecutor`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the port for the executor.
    pub fn port(mut self, port: zx::Port) -> Self {
        self.port = Some(port);
        self
    }

    /// Sets whether the executor should support binding interrupts.
    pub fn allow_interrupts(mut self, allow_interrupts: bool) -> Self {
        self.allow_interrupts = allow_interrupts;
        self
    }

    /// Sets the instrumentation hook.
    pub fn instrument(mut self, instrument: Option<Arc<dyn TaskInstrument>>) -> Self {
        self.instrument = instrument;
        self
    }

    /// Builds the `LocalExecutor`, consuming this `LocalExecutorBuilder`.
    pub fn build(self) -> LocalExecutor {
        let port = self.port.unwrap_or_else(|| {
            if self.allow_interrupts {
                zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT)
            } else {
                zx::Port::create()
            }
        });
        LocalExecutor::new_with_port(port, self.instrument)
    }
}

/// A single-threaded executor for testing. Exposes additional APIs for manipulating executor state
/// and validating behavior of executed tasks.
///
/// TODO(https://fxbug.dev/375631801): This is lack of BootInstant support.
pub struct TestExecutor {
    /// LocalExecutor used under the hood, since most of the logic is shared.
    local: LocalExecutor,
}

impl Default for TestExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl TestExecutor {
    /// Create a new executor for testing.
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Create a new single-threaded executor running with fake time.
    pub fn new_with_fake_time() -> Self {
        Self::builder().fake_time(true).build()
    }

    /// Creates a new builder for a `TestExecutor`.
    pub fn builder() -> TestExecutorBuilder {
        TestExecutorBuilder::new()
    }

    /// Get a reference to the Fuchsia `zx::Port` being used to listen for events.
    pub fn port(&self) -> &zx::Port {
        self.local.port()
    }

    /// Return the current time according to the executor.
    pub fn now(&self) -> MonotonicInstant {
        self.local.ehandle.inner().now()
    }

    /// Return the current time on the boot timeline, according to the executor.
    pub fn boot_now(&self) -> BootInstant {
        self.local.ehandle.inner().boot_now()
    }

    /// Set the fake time to a given value.
    ///
    /// # Panics
    ///
    /// If the executor was not created with fake time.
    pub fn set_fake_time(&self, t: MonotonicInstant) {
        self.local.ehandle.inner().set_fake_time(t)
    }

    /// Set the offset between the reading of the monotonic and the boot
    /// clocks.
    ///
    /// This is useful to test the situations in which the boot and monotonic
    /// offsets diverge.  In realistic scenarios, the offset can only grow,
    /// and testers should keep that in view when setting duration.
    ///
    /// # Panics
    ///
    /// If the executor was not created with fake time.
    pub fn set_fake_boot_to_mono_offset(&self, d: BootDuration) {
        self.local.ehandle.inner().set_fake_boot_to_mono_offset(d)
    }

    /// Get the global executor handle.
    pub fn global_handle(&self) -> &EHandle {
        &self.local.ehandle
    }

    /// Get the global scope of the executor.
    pub fn global_scope(&self) -> &ScopeHandle {
        self.local.root_scope()
    }

    /// Run a single future to completion on a single thread, also polling other active tasks.
    pub fn run_singlethreaded<F>(&mut self, main_future: F) -> F::Output
    where
        F: Future,
    {
        self.local.run_singlethreaded(main_future)
    }

    /// Poll the future. If it is not ready, dispatch available packets and possibly try
    /// again. Timers will only fire if this executor uses fake time. Never blocks.
    ///
    /// This function is for testing. DO NOT use this function in tests or applications that
    /// involve any interaction with other threads or processes, as those interactions
    /// may become stalled waiting for signals from "the outside world" which is beyond
    /// the knowledge of the executor.
    ///
    /// Unpin: this function requires all futures to be `Unpin`able, so any `!Unpin`
    /// futures must first be pinned using the `pin!` macro.
    pub fn run_until_stalled<F>(&mut self, main_future: &mut F) -> Poll<F::Output>
    where
        F: Future + Unpin,
    {
        let main_future = pin!(main_future);

        // Set up an instance of UntilStalledData that works with `poll_until_stalled`.
        struct Cleanup(Arc<Executor>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                *self.0.owner_data.lock() = None;
            }
        }
        let _cleanup = Cleanup(self.local.ehandle.inner().clone());
        *self.local.ehandle.inner().owner_data.lock() =
            Some(Box::new(UntilStalledData { watcher: None }));

        self.local.run(main_future, |executor, task| {
            loop {
                executor.ehandle.inner().worker_lifecycle::</*UNTIL_STALLED=*/true>(Some(task));

                // SAFETY: This is the correct type for the main future.
                let result = unsafe { executor.poll_join_result(task) };

                if result.is_ready() {
                    break result;
                }

                // If a waker was set by `poll_until_stalled`, disarm, wake, and loop.
                if let Some(watcher) = with_data(|data| data.watcher.take()) {
                    watcher.waker.wake();
                    // Relaxed ordering is fine here because this atomic is only ever access from
                    // the main thread.
                    watcher.done.store(true, Ordering::Relaxed);
                } else {
                    break Poll::Pending;
                }
            }
        })
    }

    /// Wake all tasks waiting for expired timers, and return `true` if any task was woken.
    ///
    /// This is intended for use in test code in conjunction with fake time.
    ///
    /// The wake will have effect on both the monotonic and the boot timers.
    pub fn wake_expired_timers(&mut self) -> bool {
        self.local.ehandle.inner().monotonic_timers().wake_timers()
            || self.local.ehandle.inner().boot_timers().wake_timers()
    }

    /// Wake up the next task waiting for a timer, if any, and return the time for which the
    /// timer was scheduled.
    ///
    /// This is intended for use in test code in conjunction with `run_until_stalled`.
    /// For example, here is how one could test that the Timer future fires after the given
    /// timeout:
    ///
    ///     let deadline = zx::MonotonicDuration::from_seconds(5).after_now();
    ///     let mut future = Timer::<Never>::new(deadline);
    ///     assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future));
    ///     assert_eq!(Some(deadline), exec.wake_next_timer());
    ///     assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut future));
    pub fn wake_next_timer(&mut self) -> Option<MonotonicInstant> {
        self.local.ehandle.inner().monotonic_timers().wake_next_timer()
    }

    /// Similar to [wake_next_timer], but operates on the timers on the boot
    /// timeline.
    pub fn wake_next_boot_timer(&mut self) -> Option<BootInstant> {
        self.local.ehandle.inner().boot_timers().wake_next_timer()
    }

    /// Returns the deadline for the next timer due to expire.
    pub fn next_timer() -> Option<MonotonicInstant> {
        EHandle::local().inner().monotonic_timers().next_timer()
    }

    /// Returns the deadline for the next boot timeline timer due to expire.
    pub fn next_boot_timer() -> Option<BootInstant> {
        EHandle::local().inner().boot_timers().next_timer()
    }

    /// Advances fake time to the specified time.  This will only work if the executor is being run
    /// via `TestExecutor::run_until_stalled` and can only be called by one task at a time.  This
    /// will make sure that repeating timers fire as expected.
    ///
    /// # Panics
    ///
    /// Panics if the executor was not created with fake time, and for the same reasons
    /// `poll_until_stalled` can below.
    pub async fn advance_to(time: MonotonicInstant) {
        let ehandle = EHandle::local();
        loop {
            let _: Poll<_> = Self::poll_until_stalled(future::pending::<()>()).await;
            if let Some(next_timer) = Self::next_timer()
                && next_timer <= time
            {
                ehandle.inner().set_fake_time(next_timer);
                continue;
            }
            ehandle.inner().set_fake_time(time);
            break;
        }
    }

    /// Runs the future until it is ready or the executor is stalled. Returns the state of the
    /// future.
    ///
    /// This will only work if the executor is being run via `TestExecutor::run_until_stalled` and
    /// can only be called by one task at a time.
    ///
    /// This can be used in tests to assert that a future should be pending:
    /// ```
    /// assert!(
    ///     TestExecutor::poll_until_stalled(my_fut).await.is_pending(),
    ///     "my_fut should not be ready!"
    /// );
    /// ```
    ///
    /// If you just want to know when the executor is stalled, you can do:
    /// ```
    /// let _: Poll<()> = TestExecutor::poll_until_stalled(future::pending::<()>()).await;
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if another task is currently trying to use `poll_until_stalled`, or the executor is
    /// not using `TestExecutor::run_until_stalled`.
    pub async fn poll_until_stalled<T>(fut: impl Future<Output = T> + Unpin) -> Poll<T> {
        let watcher =
            Arc::new(StalledWatcher { waker: AtomicWaker::new(), done: AtomicBool::new(false) });

        assert!(
            with_data(|data| data.watcher.replace(watcher.clone())).is_none(),
            "Error: Another task has called `poll_until_stalled`."
        );

        struct Watcher(Arc<StalledWatcher>);

        // Make sure we clean up if we're dropped.
        impl Drop for Watcher {
            fn drop(&mut self) {
                if !self.0.done.swap(true, Ordering::Relaxed) {
                    with_data(|data| data.watcher = None);
                }
            }
        }

        let watcher = Watcher(watcher);

        let poll_fn = poll_fn(|cx: &mut Context<'_>| {
            if watcher.0.done.load(Ordering::Relaxed) {
                Poll::Ready(())
            } else {
                watcher.0.waker.register(cx.waker());
                Poll::Pending
            }
        });
        match future::select(poll_fn, fut).await {
            Either::Left(_) => Poll::Pending,
            Either::Right((value, _)) => Poll::Ready(value),
        }
    }
}

/// A builder for `TestExecutor`.
#[derive(Default)]
pub struct TestExecutorBuilder {
    port: Option<zx::Port>,
    fake_time: bool,
    instrument: Option<Arc<dyn TaskInstrument>>,
    allow_interrupts: bool,
}

impl TestExecutorBuilder {
    /// Creates a new builder used for constructing a `TestExecutor`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the port for the executor.
    pub fn port(mut self, port: zx::Port) -> Self {
        self.port = Some(port);
        self
    }

    /// Sets whether the executor should use fake time.
    pub fn fake_time(mut self, fake_time: bool) -> Self {
        self.fake_time = fake_time;
        self
    }

    /// Sets whether the executor should support binding interrupts.
    pub fn allow_interrupts(mut self, allow_interrupts: bool) -> Self {
        self.allow_interrupts = allow_interrupts;
        self
    }

    /// Sets the task instrumentation.
    pub fn instrument(mut self, instrument: Arc<dyn TaskInstrument>) -> Self {
        self.instrument = Some(instrument);
        self
    }

    /// Builds the `TestExecutor`, consuming this `TestExecutorBuilder`.
    pub fn build(self) -> TestExecutor {
        let time = if self.fake_time {
            ExecutorTime::FakeTime {
                mono_reading_ns: AtomicI64::new(zx::MonotonicInstant::INFINITE_PAST.into_nanos()),
                mono_to_boot_offset_ns: AtomicI64::new(0),
            }
        } else {
            ExecutorTime::RealTime
        };
        let port = self.port.unwrap_or_else(|| {
            if self.allow_interrupts {
                zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT)
            } else {
                zx::Port::create()
            }
        });
        let inner = Arc::new(Executor::new_with_port(
            time,
            /* is_local */ true,
            /* num_threads */ 1,
            port,
            self.instrument,
        ));
        let root_scope = ScopeHandle::root(inner);
        Executor::set_local(root_scope.clone());
        let local = LocalExecutor { ehandle: EHandle { root_scope } };
        TestExecutor { local }
    }
}

struct StalledWatcher {
    waker: AtomicWaker,
    done: AtomicBool,
}

struct UntilStalledData {
    watcher: Option<Arc<StalledWatcher>>,
}

/// Calls `f` with `&mut UntilStalledData` that is stored in `owner_data`.
///
/// # Panics
///
/// Panics if `owner_data` isn't an instance of `UntilStalledData`.
fn with_data<R>(f: impl Fn(&mut UntilStalledData) -> R) -> R {
    const MESSAGE: &str = "poll_until_stalled only works if the executor is being run \
                           with TestExecutor::run_until_stalled";
    f(EHandle::local()
        .inner()
        .owner_data
        .lock()
        .as_mut()
        .expect(MESSAGE)
        .downcast_mut::<UntilStalledData>()
        .expect(MESSAGE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::on_signals::OnSignals;
    use crate::{Interval, Timer, WakeupTime};
    use assert_matches::assert_matches;
    use futures::StreamExt;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::task::Waker;

    fn spawn(future: impl Future<Output = ()> + Send + 'static) {
        crate::EHandle::local().spawn_detached(future);
    }

    // Runs a future that suspends and returns after being resumed.
    #[test]
    fn stepwise_two_steps() {
        let fut_step = Rc::new(Cell::new(0));
        let fut_waker: Rc<RefCell<Option<Waker>>> = Rc::new(RefCell::new(None));
        let fut_waker_clone = fut_waker.clone();
        let fut_step_clone = fut_step.clone();
        let fut_fn = move |cx: &mut Context<'_>| {
            fut_waker_clone.borrow_mut().replace(cx.waker().clone());
            match fut_step_clone.get() {
                0 => {
                    fut_step_clone.set(1);
                    Poll::Pending
                }
                1 => {
                    fut_step_clone.set(2);
                    Poll::Ready(())
                }
                _ => panic!("future called after done"),
            }
        };
        let fut = Box::new(future::poll_fn(fut_fn));
        let mut executor = TestExecutorBuilder::new().fake_time(true).build();
        // Spawn the future rather than waking it the main task because run_until_stalled will wake
        // the main future on every call, and we want to wake it ourselves using the waker.
        executor.local.ehandle.spawn_local_detached(fut);
        assert_eq!(fut_step.get(), 0);
        assert_eq!(executor.run_until_stalled(&mut future::pending::<()>()), Poll::Pending);
        assert_eq!(fut_step.get(), 1);

        fut_waker.borrow_mut().take().unwrap().wake();
        assert_eq!(executor.run_until_stalled(&mut future::pending::<()>()), Poll::Pending);
        assert_eq!(fut_step.get(), 2);
    }

    #[test]
    // Runs a future that waits on a timer.
    fn stepwise_timer() {
        let mut executor = TestExecutorBuilder::new().fake_time(true).build();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));
        let mut fut =
            pin!(Timer::new(MonotonicInstant::after(zx::MonotonicDuration::from_nanos(1000))));

        let _ = executor.run_until_stalled(&mut fut);
        assert_eq!(MonotonicInstant::now(), MonotonicInstant::from_nanos(0));

        executor.set_fake_time(MonotonicInstant::from_nanos(1000));
        assert_eq!(MonotonicInstant::now(), MonotonicInstant::from_nanos(1000));
        assert!(executor.run_until_stalled(&mut fut).is_ready());
    }

    // Runs a future that waits on an event.
    #[test]
    fn stepwise_event() {
        let mut executor = TestExecutorBuilder::new().fake_time(true).build();
        let event = zx::Event::create();
        let mut fut = pin!(OnSignals::new(&event, zx::Signals::USER_0));

        let _ = executor.run_until_stalled(&mut fut);

        event.signal(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
        assert_matches!(executor.run_until_stalled(&mut fut), Poll::Ready(Ok(zx::Signals::USER_0)));
    }

    // Using `run_until_stalled` does not modify the order of events
    // compared to normal execution.
    #[test]
    fn run_until_stalled_preserves_order() {
        let mut executor = TestExecutorBuilder::new().fake_time(true).build();
        let spawned_fut_completed = Arc::new(AtomicBool::new(false));
        let spawned_fut_completed_writer = spawned_fut_completed.clone();
        let spawned_fut = Box::pin(async move {
            Timer::new(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5))).await;
            spawned_fut_completed_writer.store(true, Ordering::SeqCst);
        });
        let mut main_fut = pin!(async {
            Timer::new(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(10))).await;
        });
        spawn(spawned_fut);
        assert_eq!(executor.run_until_stalled(&mut main_fut), Poll::Pending);
        executor.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(15)));
        // The timer in `spawned_fut` should fire first, then the
        // timer in `main_fut`.
        assert_eq!(executor.run_until_stalled(&mut main_fut), Poll::Ready(()));
        assert!(spawned_fut_completed.load(Ordering::SeqCst));
    }

    #[test]
    fn task_destruction() {
        struct DropSpawner {
            dropped: Arc<AtomicBool>,
        }
        impl Drop for DropSpawner {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
                let dropped_clone = self.dropped.clone();
                spawn(async {
                    // Hold on to a reference here to verify that it, too, is destroyed later
                    let _dropped_clone = dropped_clone;
                    panic!("task spawned in drop shouldn't be polled");
                });
            }
        }
        let mut dropped = Arc::new(AtomicBool::new(false));
        let drop_spawner = DropSpawner { dropped: dropped.clone() };
        let mut executor = TestExecutorBuilder::new().build();
        let mut main_fut = pin!(async move {
            spawn(async move {
                // Take ownership of the drop spawner
                let _drop_spawner = drop_spawner;
                future::pending::<()>().await;
            });
        });
        assert!(executor.run_until_stalled(&mut main_fut).is_ready());
        assert!(
            !dropped.load(Ordering::SeqCst),
            "executor dropped pending task before destruction"
        );

        // Should drop the pending task and it's owned drop spawner,
        // as well as gracefully drop the future spawned from the drop spawner.
        drop(executor);
        let dropped = Arc::get_mut(&mut dropped)
            .expect("someone else is unexpectedly still holding on to a reference");
        assert!(
            dropped.load(Ordering::SeqCst),
            "executor did not drop pending task during destruction"
        );
    }

    #[test]
    fn time_now_real_time() {
        let _executor = LocalExecutorBuilder::new().build();
        let t1 = zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(0));
        let t2 = MonotonicInstant::now().into_zx();
        let t3 = zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(0));
        assert!(t1 <= t2);
        assert!(t2 <= t3);
    }

    #[test]
    fn time_now_fake_time() {
        let executor = TestExecutorBuilder::new().fake_time(true).build();
        let t1 = MonotonicInstant::from_zx(zx::MonotonicInstant::from_nanos(0));
        executor.set_fake_time(t1);
        assert_eq!(MonotonicInstant::now(), t1);

        let t2 = MonotonicInstant::from_zx(zx::MonotonicInstant::from_nanos(1000));
        executor.set_fake_time(t2);
        assert_eq!(MonotonicInstant::now(), t2);
    }

    #[test]
    fn time_now_fake_time_boot() {
        let executor = TestExecutorBuilder::new().fake_time(true).build();
        let t1 = MonotonicInstant::from_zx(zx::MonotonicInstant::from_nanos(0));
        executor.set_fake_time(t1);
        assert_eq!(MonotonicInstant::now(), t1);
        assert_eq!(BootInstant::now().into_nanos(), t1.into_nanos());

        let t2 = MonotonicInstant::from_zx(zx::MonotonicInstant::from_nanos(1000));
        executor.set_fake_time(t2);
        assert_eq!(MonotonicInstant::now(), t2);
        assert_eq!(BootInstant::now().into_nanos(), t2.into_nanos());

        const TEST_BOOT_OFFSET: i64 = 42;

        executor.set_fake_boot_to_mono_offset(zx::BootDuration::from_nanos(TEST_BOOT_OFFSET));
        assert_eq!(BootInstant::now().into_nanos(), t2.into_nanos() + TEST_BOOT_OFFSET);
    }

    #[test]
    fn time_boot_now() {
        let executor = TestExecutorBuilder::new().fake_time(true).build();
        let t1 = MonotonicInstant::from_zx(zx::MonotonicInstant::from_nanos(0));
        executor.set_fake_time(t1);
        assert_eq!(MonotonicInstant::now(), t1);
        assert_eq!(BootInstant::now().into_nanos(), t1.into_nanos());

        let t2 = MonotonicInstant::from_zx(zx::MonotonicInstant::from_nanos(1000));
        executor.set_fake_time(t2);
        assert_eq!(MonotonicInstant::now(), t2);
        assert_eq!(BootInstant::now().into_nanos(), t2.into_nanos());

        const TEST_BOOT_OFFSET: i64 = 42;

        executor.set_fake_boot_to_mono_offset(zx::BootDuration::from_nanos(TEST_BOOT_OFFSET));
        assert_eq!(BootInstant::now().into_nanos(), t2.into_nanos() + TEST_BOOT_OFFSET);
    }

    #[test]
    fn time_after_overflow() {
        let executor = TestExecutorBuilder::new().fake_time(true).build();

        executor.set_fake_time(MonotonicInstant::INFINITE - zx::MonotonicDuration::from_nanos(100));
        assert_eq!(
            MonotonicInstant::after(zx::MonotonicDuration::from_seconds(200)),
            MonotonicInstant::INFINITE
        );

        executor.set_fake_time(
            MonotonicInstant::INFINITE_PAST + zx::MonotonicDuration::from_nanos(100),
        );
        assert_eq!(
            MonotonicInstant::after(zx::MonotonicDuration::from_seconds(-200)),
            MonotonicInstant::INFINITE_PAST
        );
    }

    // This future wakes itself up a number of times during the same cycle
    async fn multi_wake(n: usize) {
        let mut done = false;
        futures::future::poll_fn(|cx| {
            if done {
                return Poll::Ready(());
            }
            for _ in 1..n {
                cx.waker().wake_by_ref()
            }
            done = true;
            Poll::Pending
        })
        .await;
    }

    #[test]
    fn test_boot_time_tracks_mono_time() {
        const FAKE_TIME: i64 = 42;
        let executor = TestExecutorBuilder::new().fake_time(true).build();
        executor.set_fake_time(MonotonicInstant::from_nanos(FAKE_TIME));
        assert_eq!(
            BootInstant::from_nanos(FAKE_TIME),
            executor.boot_now(),
            "boot time should have advanced"
        );

        // Now advance boot without mono.
        executor.set_fake_boot_to_mono_offset(BootDuration::from_nanos(FAKE_TIME));
        assert_eq!(
            BootInstant::from_nanos(2 * FAKE_TIME),
            executor.boot_now(),
            "boot time should have advanced again"
        );
    }

    // Ensure that a large amount of wakeups does not exhaust kernel resources,
    // such as the zx port queue limit.
    #[test]
    fn many_wakeups() {
        let mut executor = LocalExecutorBuilder::new().build();
        executor.run_singlethreaded(multi_wake(4096 * 2));
    }

    fn advance_to_with(timer_duration: impl WakeupTime) {
        let mut executor = TestExecutorBuilder::new().fake_time(true).build();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));

        let mut fut = pin!(async {
            let timer_fired = Arc::new(AtomicBool::new(false));
            futures::join!(
                async {
                    // Oneshot timer.
                    Timer::new(timer_duration).await;
                    timer_fired.store(true, Ordering::SeqCst);
                },
                async {
                    // Interval timer, fires periodically.
                    let mut fired = 0;
                    let mut interval = pin!(Interval::new(zx::MonotonicDuration::from_seconds(1)));
                    while interval.next().await.is_some() {
                        fired += 1;
                        if fired == 3 {
                            break;
                        }
                    }
                    assert_eq!(fired, 3, "interval timer should have fired multiple times.");
                },
                async {
                    assert!(
                        !timer_fired.load(Ordering::SeqCst),
                        "the oneshot timer shouldn't be fired"
                    );
                    TestExecutor::advance_to(MonotonicInstant::after(
                        zx::MonotonicDuration::from_millis(500),
                    ))
                    .await;
                    // Timer still shouldn't be fired.
                    assert!(
                        !timer_fired.load(Ordering::SeqCst),
                        "the oneshot timer shouldn't be fired"
                    );
                    TestExecutor::advance_to(MonotonicInstant::after(
                        zx::MonotonicDuration::from_millis(500),
                    ))
                    .await;

                    assert!(
                        timer_fired.load(Ordering::SeqCst),
                        "the oneshot timer should have fired"
                    );

                    // The interval timer should have fired once.  Make it fire twice more.
                    TestExecutor::advance_to(MonotonicInstant::after(
                        zx::MonotonicDuration::from_seconds(2),
                    ))
                    .await;
                }
            )
        });
        assert!(executor.run_until_stalled(&mut fut).is_ready());
    }

    #[test]
    fn test_advance_to() {
        advance_to_with(zx::MonotonicDuration::from_seconds(1));
    }

    #[test]
    fn test_advance_to_boot() {
        advance_to_with(zx::BootDuration::from_seconds(1));
    }

    #[test]
    fn test_allow_interrupts() {
        use crate::OnInterrupt;
        use futures::StreamExt;

        let mut executor = LocalExecutorBuilder::new().allow_interrupts(true).build();
        executor.run_singlethreaded(async {
            let irq_raw = zx::VirtualInterrupt::create_virtual().unwrap();
            // Duplicate the handle
            let irq_clone = irq_raw.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let mut irq = std::pin::pin!(OnInterrupt::new(irq_raw));

            let timestamp = zx::BootInstant::from_nanos(42);

            let task = crate::Task::spawn(async move {
                crate::Timer::new(zx::MonotonicDuration::from_millis(10)).await;
                irq_clone.trigger(timestamp).unwrap();
            });

            let result = irq.next().await.unwrap().unwrap();
            assert_eq!(result, timestamp);
            task.await;
        });
    }
}
