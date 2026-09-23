// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::common::{Executor, ExecutorTime};
use super::scope::ScopeHandle;
use fuchsia_sync::{Condvar, Mutex};

use crate::runtime::instrument::TaskInstrument;
use futures::FutureExt;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::{fmt, thread};

/// A multi-threaded port-based executor for Fuchsia. Requires that tasks scheduled on it
/// implement `Send` so they can be load balanced between worker threads.
///
/// Having a `SendExecutor` in scope allows the creation and polling of zircon objects, such as
/// [`fuchsia_async::Channel`].
///
/// # Panics
///
/// `SendExecutor` will panic on drop if any zircon objects attached to it are still alive. In other
/// words, zircon objects backed by a `SendExecutor` must be dropped before it.
pub struct SendExecutor {
    /// The inner executor state.
    inner: Arc<Executor>,
    // LINT.IfChange
    /// The root scope.
    root_scope: ScopeHandle,
    // LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)
    /// Worker thread handles
    threads: Vec<thread::JoinHandle<()>>,
    worker_init: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
}

impl fmt::Debug for SendExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendExecutor").field("port", &self.inner.port).finish()
    }
}

impl SendExecutor {
    fn new_inner(
        num_threads: u8,
        worker_init: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
        instrument: Option<Arc<dyn TaskInstrument>>,
        allow_interrupts: bool,
    ) -> Self {
        let port = if allow_interrupts {
            zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT)
        } else {
            zx::Port::create()
        };
        let inner = Arc::new(Executor::new_with_port(
            ExecutorTime::RealTime,
            /* is_local */ false,
            num_threads,
            port,
            instrument,
        ));
        let root_scope = ScopeHandle::root(inner.clone());
        Executor::set_local(root_scope.clone());
        Self { inner, root_scope, threads: Vec::default(), worker_init }
    }

    /// Get a reference to the Fuchsia `zx::Port` being used to listen for events.
    pub fn port(&self) -> &zx::Port {
        &self.inner.port
    }

    /// Run `future` to completion, using this thread and `num_threads` workers in a pool to
    /// poll active tasks.
    // The debugger looks for this function on the stack, so if its (fully-qualified) name changes,
    // the debugger needs to be updated.
    // LINT.IfChange
    pub fn run<F>(&mut self, future: F) -> F::Output
    // LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        assert!(self.inner.is_real_time(), "Error: called `run` on an executor using fake time");

        let pair = Arc::new((Mutex::new(None), Condvar::new()));
        let pair2 = pair.clone();

        // Spawn a future which will set the result upon completion.
        let task = self.root_scope.new_task(future.map(move |fut_result| {
            let (lock, cvar) = &*pair2;
            let mut result = lock.lock();
            *result = Some(fut_result);
            cvar.notify_one();
        }));
        task.detach();
        assert!(self.root_scope.insert_task(task, false));

        // Start worker threads, handing off timers from the current thread.
        self.inner.done.store(false, Ordering::SeqCst);
        self.create_worker_threads();

        // Wait until the signal the future has completed.
        let (lock, cvar) = &*pair;
        let mut result = lock.lock();
        if result.is_none() {
            let mut last_polled = 0;
            let mut last_tasks_ready = false;
            loop {
                // This timeout is chosen to be quite high since it impacts all processes that have
                // multi-threaded async executors, and it exists to workaround arguably misbehaving
                // users (see the comment below).
                cvar.wait_for(&mut result, Duration::from_millis(250));
                if result.is_some() {
                    break;
                }
                let polled = self.inner.polled.load(Ordering::Relaxed);
                let tasks_ready = !self.inner.ready_tasks.is_empty();
                if polled == last_polled && last_tasks_ready && tasks_ready {
                    // If this log message is printed, it most likely means that a task has blocked
                    // making a reentrant synchronous call that doesn't involve a port message being
                    // processed by this same executor. This can arise even if you would expect
                    // there to normally be other port messages involved. One example (that has
                    // actually happened): spawn a task to service a fuchsia.io connection, then try
                    // and synchronously connect to that service. If the task hasn't had a chance to
                    // run, then the async channel might not be registered with the executor, and so
                    // sending messages to the channel doesn't trigger a port message. Typically,
                    // the way to solve these issues is to run the service in a different executor
                    // (which could be the same or a different process).
                    eprintln!("Tasks might be stalled!");
                    self.inner.wake_one_thread();
                }
                last_polled = polled;
                last_tasks_ready = tasks_ready;
            }
        }

        // Spin down worker threads
        self.join_all();

        // Unwrap is fine because of the check to `is_none` above.
        result.take().unwrap()
    }

    #[doc(hidden)]
    /// Returns the root scope of the executor.
    pub fn root_scope(&self) -> &ScopeHandle {
        &self.root_scope
    }

    /// Add `self.num_threads` worker threads to the executor's thread pool.
    /// `timers`: timers from the "main" thread which would otherwise be lost.
    fn create_worker_threads(&mut self) {
        for _ in 0..self.inner.num_threads {
            let inner = self.inner.clone();
            let root_scope = self.root_scope.clone();
            let worker_init = self.worker_init.clone();
            let thread = thread::Builder::new()
                .name("executor_worker".to_string())
                .spawn(move || {
                    Executor::set_local(root_scope);
                    if let Some(init) = worker_init.as_ref() {
                        init();
                    }
                    inner.worker_lifecycle::</* UNTIL_STALLED: */ false>(None);
                })
                .expect("must be able to spawn threads");
            self.threads.push(thread);
        }
    }

    fn join_all(&mut self) {
        self.inner.mark_done();

        // Join the worker threads
        for thread in self.threads.drain(..) {
            thread.join().expect("Couldn't join worker thread.");
        }
    }
}

impl Drop for SendExecutor {
    fn drop(&mut self) {
        self.join_all();
        self.inner.on_parent_drop(&self.root_scope);
    }
}

/// A builder for `SendExecutor`.
#[derive(Default)]
pub struct SendExecutorBuilder {
    num_threads: Option<u8>,
    worker_init: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
    instrument: Option<Arc<dyn TaskInstrument>>,
    allow_interrupts: bool,
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

    /// Sets the worker initialization function.
    pub fn worker_init(mut self, worker_init: impl Fn() + Send + Sync + 'static) -> Self {
        self.worker_init = Some(Arc::new(worker_init));
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

    /// Builds the `SendExecutor`, consuming this `SendExecutorBuilder`.
    pub fn build(self) -> SendExecutor {
        SendExecutor::new_inner(
            self.num_threads.unwrap_or(1),
            self.worker_init,
            self.instrument,
            self.allow_interrupts,
        )
    }
}

// TODO(https://fxbug.dev/42156503) test SendExecutor with unit tests

#[cfg(test)]
mod tests {
    use super::SendExecutorBuilder;
    use crate::{Task, Timer};

    use fuchsia_sync::{Condvar, Mutex};
    use futures::channel::oneshot;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn test_stalled_triggers_wake_up() {
        SendExecutorBuilder::new().num_threads(2).build().run(async {
            // The timer will only fire on one thread, so use one so we can get to a point where
            // only one thread is running.
            Timer::new(zx::MonotonicDuration::from_millis(10)).await;

            let (tx, rx) = oneshot::channel();
            let pair = Arc::new((Mutex::new(false), Condvar::new()));
            let pair2 = pair.clone();

            let _task = Task::spawn(async move {
                // Send a notification to the other task.
                tx.send(()).unwrap();
                // Now block the thread waiting for the result.
                let (lock, cvar) = &*pair;
                let mut done = lock.lock();
                while !*done {
                    cvar.wait(&mut done);
                }
            });

            rx.await.unwrap();
            let (lock, cvar) = &*pair2;
            *lock.lock() = true;
            cvar.notify_one();
        });
    }

    #[test]
    fn worker_init_called_once_per_worker() {
        static NUM_INIT_CALLS: AtomicU64 = AtomicU64::new(0);
        fn initialize_test_worker() {
            NUM_INIT_CALLS.fetch_add(1, Ordering::SeqCst);
        }

        let mut exec =
            SendExecutorBuilder::new().num_threads(2).worker_init(initialize_test_worker).build();
        exec.run(async {});
        assert_eq!(NUM_INIT_CALLS.load(Ordering::SeqCst), 2);
        exec.run(async {});
        assert_eq!(NUM_INIT_CALLS.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_allow_interrupts() {
        use crate::OnInterrupt;
        use futures::StreamExt;

        SendExecutorBuilder::new().num_threads(2).allow_interrupts(true).build().run(async {
            let irq_raw = zx::VirtualInterrupt::create_virtual().unwrap();
            let irq_clone = irq_raw.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let mut irq = std::pin::pin!(OnInterrupt::new(irq_raw));

            let timestamp = zx::BootInstant::from_nanos(42);

            let task = Task::spawn(async move {
                Timer::new(zx::MonotonicDuration::from_millis(10)).await;
                irq_clone.trigger(timestamp).unwrap();
            });

            let result = irq.next().await.unwrap().unwrap();
            assert_eq!(result, timestamp);
            task.await;
        });
    }
}
