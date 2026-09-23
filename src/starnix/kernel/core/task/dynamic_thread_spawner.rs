// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The API for spawning dynamic kernel threads.
//!
//! If you want to run a closure on a kernel thread, check out [SpawnRequestBuilder] on
//! how to start and configure tasks that run closures.

use crate::execution::create_kernel_thread;
use crate::task::{
    CurrentTask, DelayedReleaser, Task, ThreadLockupDetector, WrappedFuture, with_new_current_task,
};
use futures::TryFutureExt;
use futures::channel::oneshot;
use starnix_logging::{CATEGORY_STARNIX, log_debug, log_error};
use starnix_sync::{DynamicThreadSpawnerLock, LockDepMutex};
use starnix_task_command::TaskCommand;
use starnix_types::ownership::release_after;
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::future::Future;
use std::sync::mpsc::{SendError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;

type BoxedClosure = Box<dyn FnOnce(&CurrentTask) -> () + Send + 'static>;

const DEFAULT_THREAD_ROLE: &str = "fuchsia.starnix.fair.16";

/// A builder for configuring new tasks to spawn.
///
/// The builder allows us to set up several different flavors of possible
/// tasks to spawn, with a menu of options as follows:
///
/// - A task may or may not have a name.
/// - A task may or may not have an assigned scheduler role.
/// - A task may be a sync, or an async closure.
/// - A task may return a result, or may not return a result.
/// - The task's result can be collected synchronously, asynchronously, or not collected at all.
///
/// Note that these parameters are not perfectly orthogonal. For example, a task spawned from an
/// async closure can not return a value asynchronously. (This is not a limitation of the approach,
/// rather it's the API that we explicitly use today.) Also, some parameter combinations do not
/// make sense, for example a spawn request can not have both a sync and an async closure to run.
///
/// The builder API is designed in a way that only allows chaining the configuration options which
/// are valid at that point in the configuration process. Invalid option combinations are
/// compile-time(!) errors. It will only allow creating a [SpawnRequest] if enough parameters have
/// been passed such that there is enough information to create a request. It will not allow
/// passing conflicting parameters: for example, if you already passed one synchronous closure, it
/// is impossible to pass another closure and have the code compile without errors.
///
/// ## Usage
///
/// Call [SpawnRequestBuilder::new()] to start building. Refer to the unit tests in this module for
/// usage examples.
pub struct SpawnRequestBuilder<C: ClosureKind> {
    debug_name: &'static str,
    role: Option<&'static str>,
    closure_kind: C,
}

/// You can only create an empty request builder.
impl SpawnRequestBuilder<ClosureNone> {
    /// Creates a new spawn request builder.
    pub fn new() -> Self {
        Self { role: None, closure_kind: ClosureNone {}, debug_name: "kthreadd" }
    }
}

/// You can call these at any point in the builder's lifecycle.
impl<C: ClosureKind> SpawnRequestBuilder<C> {
    /// Set a role to apply to the thread that will run your closure.
    pub fn with_role(self, role: &'static str) -> Self {
        Self { role: Some(role), ..self }
    }

    /// Set a task name to apply to the thread that will run your closure.
    pub fn with_debug_name(self, debug_name: &'static str) -> Self {
        Self { debug_name, ..self }
    }
}

/// You can call these only if you have not provided a closure yet.
impl SpawnRequestBuilder<ClosureNone> {
    /// Provides the closure that the spawner will run.
    pub fn with_sync_closure<F, T>(
        self,
        f: F,
    ) -> SpawnRequestBuilder<impl FnOnce(&CurrentTask) -> T + Send + 'static>
    where
        T: Send + 'static,
        F: FnOnce(&CurrentTask) -> T + Send + 'static,
    {
        let SpawnRequestBuilder { role, closure_kind: _, debug_name } = self;
        SpawnRequestBuilder { role, closure_kind: f, debug_name }
    }

    /// Provides the closure that the spawner will run.
    pub fn with_async_closure<F, T>(
        self,
        f: F,
    ) -> SpawnRequestBuilder<impl FnOnce(&CurrentTask) -> T + Send + 'static>
    where
        T: Send + 'static,
        F: AsyncFnOnce(&CurrentTask) -> T + Send + 'static,
    {
        let sync_fn = async_to_sync(f, self.debug_name);
        self.with_sync_closure(sync_fn)
    }
}

/// A fully configured spawn request.
pub struct SpawnRequest {
    /// The closure to run.
    closure: BoxedClosure,
    /// A name to give to the task.
    debug_name: &'static str,
}

impl<T, F> SpawnRequestBuilder<F>
where
    T: Send + 'static,
    F: FnOnce(&CurrentTask) -> T + Send + 'static,
{
    /// Build a spawn request.
    pub fn build(self) -> SpawnRequest {
        let Self { role, closure_kind, debug_name } = self;
        let closure = closure_kind;
        let closure = maybe_apply_role(role, closure);
        let closure = Box::new(move |current_task: &CurrentTask| {
            fuchsia_trace::duration!(CATEGORY_STARNIX, debug_name);
            let _ = closure(current_task);
        });
        SpawnRequest { closure, debug_name }
    }

    /// Like [build], but allows receiving a result synchronously.
    /// Do not forget to submit the spawn request to a spawner.
    ///
    /// Example:
    ///
    /// ```
    /// let (result_fn, request) = /*...*/ .build_with_sync_result();
    /// // spawn `request`
    /// let result = result_fn();
    /// ```
    pub fn build_with_sync_result(self) -> (impl FnOnce() -> Result<T, Errno>, SpawnRequest) {
        let Self { role, closure_kind, debug_name } = self;
        let closure = closure_kind;
        let (sender, receiver) = sync_channel::<T>(0);
        let result_fn = move || {
            receiver.recv().map_err(|err| errno!(EINTR, format!("while receiving: {err:?}")))
        };
        let closure = maybe_apply_role(role, closure);
        let closure = Box::new(move |current_task: &CurrentTask| {
            fuchsia_trace::duration!(CATEGORY_STARNIX, debug_name);
            let _ = sender.send(closure(current_task));
        });
        (result_fn, SpawnRequest { closure, debug_name })
    }

    /// Like [build], but allows receiving a result as a future.
    /// Do not forget to submit the spawn request to a spawner.
    ///
    /// Example:
    ///
    /// ```
    /// let (result_fut, request) = /*...*/ .build_with_async_result();
    /// // spawn `request`
    /// let result = result_fut.await;
    /// ```
    pub fn build_with_async_result(self) -> (impl Future<Output = Result<T, Errno>>, SpawnRequest) {
        let Self { role, closure_kind, debug_name } = self;
        let closure = closure_kind;
        let (sender_async, result_fut) = oneshot::channel::<T>();
        let maybe_with_role = maybe_apply_role(role, closure);
        let repackaged = Box::new(move |current_task: &CurrentTask| {
            fuchsia_trace::duration!(CATEGORY_STARNIX, debug_name);
            let result = maybe_with_role(current_task);
            let _ = sender_async.send(result);
        });
        let result_fut =
            result_fut.map_err(|err| errno!(EINTR, format!("while receiving async: {err:?}")));
        (result_fut, SpawnRequest { closure: repackaged, debug_name })
    }
}

/// A thread pool that immediately execute any new work sent to it and keep a maximum number of
/// idle threads.
#[derive(Debug)]
pub struct DynamicThreadSpawner {
    state: Arc<LockDepMutex<DynamicThreadSpawnerState, DynamicThreadSpawnerLock>>,
    /// The weak system task to create the kernel thread associated with each thread.
    system_task: Weak<Task>,
    /// A persistent thread that is used to create new thread. This ensures that threads are
    /// created from the initial starnix process and are not tied to a specific task.
    persistent_thread: RunningThread,
}

/// Wrap a closure with a thread role assignment, if one is available.
fn maybe_apply_role<R, F>(
    role: Option<&'static str>,
    f: F,
) -> impl FnOnce(&CurrentTask) -> R + Send + 'static
where
    F: FnOnce(&CurrentTask) -> R + Send + 'static,
{
    move |current_task| {
        if let Some(role) = role {
            if let Err(e) = fuchsia_scheduler::set_role_for_this_thread(role) {
                log_debug!(e:%; "failed to set kthread role");
            }
            let result = f(current_task);
            if let Err(e) = fuchsia_scheduler::set_role_for_this_thread(DEFAULT_THREAD_ROLE) {
                log_debug!(e:%; "failed to reset kthread role to default priority");
            }
            result
        } else {
            f(current_task)
        }
    }
}

/// Convert async closure to sync closure that can be submitted to the spawner.
fn async_to_sync<T, F>(f: F, name: &'static str) -> impl FnOnce(&CurrentTask) -> T + Send + 'static
where
    T: Send + 'static,
    F: AsyncFnOnce(&CurrentTask) -> T + Send + 'static,
{
    move |current_task| {
        let mut exec = fuchsia_async::LocalExecutor::default();

        let wrapped_future = WrappedSpawnedFuture::new(
            current_task,
            ThreadLockupDetector::track_future(f(current_task)),
            name,
        );
        let _waiting_guard = ThreadLockupDetector::pause_tracking();
        exec.run_singlethreaded(wrapped_future)
    }
}

/// Denotes whether a closure has been provided. A request can not be
/// built at all without a closure.
///
/// See [SpawnRequestBuilder] for usage details.
pub trait ClosureKind {}

/// A builder type state where no closure has been provided yet.
///
/// See [SpawnRequestBuilder] for usage details.
pub struct ClosureNone {}
impl ClosureKind for ClosureNone {}

/// A builder type state where a closure has been provided.
/// See [SpawnRequestBuilder] for usage details.
impl<T: Send + 'static, FN: FnOnce(&CurrentTask) -> T + Send + 'static> ClosureKind for FN {}

#[derive(Debug)]
struct DynamicThreadSpawnerState {
    threads: Vec<RunningThread>,
    idle_threads: u8,
    max_idle_threads: u8,
}

impl DynamicThreadSpawner {
    pub fn new(
        max_idle_threads: u8,
        system_task: Weak<Task>,
        debug_name: impl Into<String>,
    ) -> Self {
        let persistent_thread =
            RunningThread::new_persistent(system_task.clone(), debug_name.into());
        Self {
            state: Arc::new(
                DynamicThreadSpawnerState { max_idle_threads, idle_threads: 0, threads: vec![] }
                    .into(),
            ),
            system_task,
            persistent_thread,
        }
    }

    /// Run a given closure on a thread based on the provided [SpawnRequest].
    ///
    /// Use [SpawnRequestBuilder::new()] to start configuring a [SpawnRequest].
    ///
    /// This method will use an idle thread in the pool if one is available, otherwise it will
    /// start a new thread. When this method returns, it is guaranteed that a thread is
    /// responsible to start running the closure.
    pub fn spawn_from_request(&self, spawn_request: SpawnRequest) {
        // Check whether a thread already exists to handle the request.
        let mut function: BoxedClosure = spawn_request.closure;
        let mut state = self.state.lock();
        if state.idle_threads > 0 {
            let mut i = 0;
            while i < state.threads.len() {
                // Increases `i` immediately, so that it can be decreased it the thread must be
                // dropped.
                let thread_index = i;
                i += 1;
                match state.threads[thread_index].try_dispatch(function) {
                    Ok(_) => {
                        // The dispatch succeeded.
                        state.idle_threads -= 1;
                        return;
                    }
                    Err(TrySendError::Full(f)) => {
                        // The thread is busy.
                        function = f;
                    }
                    Err(TrySendError::Disconnected(f)) => {
                        // The receiver is disconnected, it means the thread has terminated, drop it.
                        state.idle_threads -= 1;
                        state.threads.remove(thread_index);
                        i -= 1;
                        function = f;
                    }
                }
            }
        }

        // A new thread must be created. It needs to be done from the persistent thread.
        let (sender, receiver) = sync_channel::<RunningThread>(0);
        let dispatch_function: BoxedClosure = Box::new({
            let state = self.state.clone();
            let system_task = self.system_task.clone();
            move |_| {
                sender
                    .send(RunningThread::new(
                        state,
                        system_task,
                        spawn_request.debug_name.to_string(),
                        function,
                    ))
                    .expect("receiver must not be dropped");
            }
        });
        self.persistent_thread
            .dispatch(dispatch_function)
            .expect("persistent thread should not have ended.");
        state.threads.push(receiver.recv().expect("persistent thread should not have ended."));
    }
}

type WrappedSpawnedFuture<'a, F> = WrappedFuture<F, &'a CurrentTask>;

impl<'a, F: 'a> WrappedSpawnedFuture<'a, F> {
    fn new(current_task: &'a CurrentTask, fut: F, name: &'static str) -> Self {
        Self::new_with_cleaner(current_task, trigger_delayed_releaser, fut, name)
    }
}

fn trigger_delayed_releaser(current_task: &CurrentTask) {
    current_task.trigger_delayed_releaser();
}

#[derive(Debug)]
struct RunningThread {
    thread: Option<JoinHandle<()>>,
    sender: Option<SyncSender<BoxedClosure>>,
}

impl RunningThread {
    fn new(
        state: Arc<LockDepMutex<DynamicThreadSpawnerState, DynamicThreadSpawnerLock>>,
        system_task: Weak<Task>,
        debug_task_name: String,
        f: BoxedClosure,
    ) -> Self {
        let (sender, receiver) = sync_channel::<BoxedClosure>(0);
        let thread = Some(
            std::thread::Builder::new()
                .name("kthread-dynamic-worker".to_string())
                .spawn(move || {
                    let result =
                        with_new_current_task(&system_task, debug_task_name, |current_task| {
                            while let Ok(f) = receiver.recv() {
                                let _guard = ThreadLockupDetector::track();
                                f(&current_task);
                                // Apply any delayed releasers.
                                current_task.trigger_delayed_releaser();
                                let mut state = state.lock();
                                state.idle_threads += 1;
                                if state.idle_threads > state.max_idle_threads {
                                    // If the number of idle thread is greater than the max, the
                                    // thread terminates.  This disconnects the receiver, which will
                                    // ensure that the thread will be joined and remove from the list
                                    // of available threads the next time the pool tries to use it.
                                    return;
                                }
                            }
                        });
                    if let Err(e) = result {
                        log_error!("Unable to create a kernel thread: {e:?}");
                    }
                })
                .expect("able to create threads"),
        );
        let result = Self { thread, sender: Some(sender) };
        // The dispatch cannot fail because the thread can only finish after having executed at
        // least one task, and this is the first task ever dispatched to it.
        result
            .sender
            .as_ref()
            .expect("sender should never be None")
            .send(f)
            .expect("Dispatch cannot fail");
        result
    }

    fn new_persistent(system_task: Weak<Task>, task_name: String) -> Self {
        // The persistent thread doesn't need to do any rendez-vous when received task.
        let (sender, receiver) = sync_channel::<BoxedClosure>(20);
        let thread = Some(
            std::thread::Builder::new()
                .name("kthread-persistent-worker".to_string())
                .spawn(move || {
                    let current_task = {
                        let Some(system_task) = system_task.upgrade() else {
                            return;
                        };
                        match create_kernel_thread(
                            &system_task,
                            TaskCommand::new(task_name.as_bytes()),
                        ) {
                            Ok(task) => task,
                            Err(e) => {
                                log_error!("Unable to create a kernel thread: {e:?}");
                                return;
                            }
                        }
                    };
                    release_after!(current_task, {
                        while let Ok(f) = receiver.recv() {
                            let _guard = ThreadLockupDetector::track();
                            f(&current_task);

                            // Apply any delayed releasers.
                            current_task.trigger_delayed_releaser();
                        }
                    });

                    // Ensure that no releasables are registered after this point as we unwind the stack.
                    DelayedReleaser::finalize();
                })
                .expect("able to create threads"),
        );
        Self { thread, sender: Some(sender) }
    }

    fn try_dispatch(&self, f: BoxedClosure) -> Result<(), TrySendError<BoxedClosure>> {
        self.sender.as_ref().expect("sender should never be None").try_send(f)
    }

    fn dispatch(&self, f: BoxedClosure) -> Result<(), SendError<BoxedClosure>> {
        self.sender.as_ref().expect("sender should never be None").send(f)
    }
}

impl Drop for RunningThread {
    fn drop(&mut self) {
        self.sender = None;
        match self.thread.take() {
            Some(thread) => thread.join().expect("Thread should join."),
            _ => panic!("Thread should never be None"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::spawn_kernel_and_run;

    #[fuchsia::test]
    async fn run_simple_task() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(2, current_task.weak_task(), "kthreadd");
            // Type decorations are needed sometimes to avoid "closure type is
            // not general enough" error.
            let closure = move |_: &CurrentTask| {};
            let req = SpawnRequestBuilder::new().with_sync_closure(closure).build();
            spawner.spawn_from_request(req);
        })
        .await;
    }

    #[fuchsia::test]
    async fn run_10_tasks() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(2, current_task.weak_task(), "kthreadd");
            for _ in 0..10 {
                let closure = move |_: &CurrentTask| {};
                let opts = SpawnRequestBuilder::new().with_sync_closure(closure).build();
                spawner.spawn_from_request(opts);
            }
        })
        .await;
    }

    #[fuchsia::test]
    async fn blocking_task_do_not_prevent_further_processing() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(1, current_task.weak_task(), "kthreadd");

            let pair = Arc::new((fuchsia_sync::Mutex::new(false), fuchsia_sync::Condvar::new()));
            for _ in 0..10 {
                let pair2 = Arc::clone(&pair);
                let closure = move |_: &CurrentTask| {
                    let (lock, cvar) = &*pair2;
                    let mut cont = lock.lock();
                    while !*cont {
                        cvar.wait(&mut cont);
                    }
                };
                let req = SpawnRequestBuilder::new().with_sync_closure(closure).build();
                spawner.spawn_from_request(req);
            }

            let closure = move |_: &CurrentTask| {
                let (lock, cvar) = &*pair;
                let mut cont = lock.lock();
                *cont = true;
                cvar.notify_all();
            };

            let (result, req) =
                SpawnRequestBuilder::new().with_sync_closure(closure).build_with_sync_result();
            spawner.spawn_from_request(req);

            assert_eq!(result(), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn run_spawn_and_get_result() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(2, current_task.weak_task(), "kthreadd");

            let (result, req) =
                SpawnRequestBuilder::new().with_sync_closure(|_| 3).build_with_sync_result();
            spawner.spawn_from_request(req);
            assert_eq!(result(), Ok(3));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_spawn_async() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(2, current_task.weak_task(), "kthreadd");

            // The closure free variables must be decorated with their respective types,
            // the rust compiler gets confused otherwise and is unable to infer the correct
            // lifetimes. Interestingly, adding your own lifetimes here does *not* help.
            let closure = move |current_task: &CurrentTask| {
                let mut exec = fuchsia_async::LocalExecutor::default();
                let fut = async {};
                let wrapped_future = WrappedSpawnedFuture::new(current_task, fut, "test-async");
                exec.run_singlethreaded(wrapped_future);
            };
            let req = SpawnRequestBuilder::new().with_sync_closure(closure).build();
            spawner.spawn_from_request(req);
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_spawn_async_closure() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(2, current_task.weak_task(), "kthreadd");
            let fut = async |_: &CurrentTask| 42;
            let (result, req) =
                SpawnRequestBuilder::new().with_async_closure(fut).build_with_sync_result();
            spawner.spawn_from_request(req);
            assert_eq!(result(), Ok(42));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_spawn_sync_to_async_result() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(2, current_task.weak_task(), "kthreadd");
            let fut = async |_: &CurrentTask| 42;
            let (result, req) =
                SpawnRequestBuilder::new().with_async_closure(fut).build_with_sync_result();

            let fut2 = async move |_: &CurrentTask| result().unwrap();
            let (result2, req2) =
                SpawnRequestBuilder::new().with_async_closure(fut2).build_with_sync_result();
            spawner.spawn_from_request(req2);
            spawner.spawn_from_request(req);
            assert_eq!(result2(), Ok(42));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_spawn_async_to_async_result() {
        spawn_kernel_and_run(async |current_task| {
            let spawner = DynamicThreadSpawner::new(2, current_task.weak_task(), "kthreadd");
            let fut = async |_: &CurrentTask| 42;
            let (result_fut, req) =
                SpawnRequestBuilder::new().with_async_closure(fut).build_with_async_result();

            let fut2 = async move |_: &CurrentTask| result_fut.await.unwrap();
            let (result2, req2) =
                SpawnRequestBuilder::new().with_async_closure(fut2).build_with_sync_result();
            spawner.spawn_from_request(req2);
            spawner.spawn_from_request(req);
            assert_eq!(result2(), Ok(42));
        })
        .await;
    }
}
