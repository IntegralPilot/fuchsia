// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Values of this type represent "execution scopes" used by the library to give fine grained
//! control of the lifetimes of the tasks associated with particular connections.  When a new
//! connection is attached to a pseudo directory tree, an execution scope is provided.  This scope
//! is then used to start any tasks related to this connection.  All connections opened as a result
//! of operations on this first connection will also use the same scope, as well as any tasks
//! related to those connections.
//!
//! This way, it is possible to control the lifetime of a group of connections.  All connections
//! and their tasks can be shutdown by calling `shutdown` method on the scope that is hosting them.
//! Scope will also shutdown all the tasks when it goes out of scope.
//!
//! Implementation wise, execution scope is just a proxy, that forwards all the tasks to an actual
//! executor, provided as an instance of a [`futures::task::Spawn`] trait.

use crate::token_registry::TokenRegistry;

use fuchsia_async::{JoinHandle, Scope, ScopeHandle, SpawnableFuture};
use fuchsia_sync::{MappedMutexGuard, Mutex, MutexGuard};
use futures::Future;
use futures::task::{self, Poll};
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::Context;

#[cfg(target_os = "fuchsia")]
use fuchsia_async::EHandle;

pub use fuchsia_async::scope::ScopeActiveGuard as ActiveGuard;

pub type SpawnError = task::SpawnError;

/// An execution scope that is hosting tasks for a group of connections.  See the module level
/// documentation for details.
///
/// Actual execution will be delegated to an "upstream" executor - something that implements
/// [`futures::task::Spawn`].  In a sense, this is somewhat of an analog of a multithreaded capable
/// [`futures::stream::FuturesUnordered`], but this some additional functionality specific to the
/// vfs library.
///
/// Use [`ExecutionScope::new()`] or [`ExecutionScope::build()`] to construct new
/// `ExecutionScope`es.
#[derive(Clone)]
pub struct ExecutionScope {
    executor: Arc<Executor>,

    /// The FDomain client, which lets us communicate with the Fuchsia device
    /// where the actual handles are.
    #[cfg(feature = "fdomain")]
    client: Arc<flex_client::Client>,
}

struct Executor {
    token_registry: TokenRegistry,
    scope: Mutex<Option<Scope>>,
}

impl ExecutionScope {
    /// Constructs an execution scope.  Use [`ExecutionScope::build()`] if you want to specify
    /// parameters.
    pub fn new(#[cfg(feature = "fdomain")] client: Arc<flex_client::Client>) -> Self {
        Self::build().new(
            #[cfg(feature = "fdomain")]
            client,
        )
    }

    /// Return the domain for handle creation for operations in this execution scope.
    #[cfg(feature = "fdomain")]
    pub fn domain(&self) -> Arc<flex_client::Client> {
        Arc::clone(&self.client)
    }

    /// Return the domain for handle creation for operations in this execution scope.
    #[cfg(not(feature = "fdomain"))]
    pub fn domain(&self) -> fidl::endpoints::ZirconClient {
        fidl::endpoints::ZirconClient
    }

    /// Constructs a new execution scope builder, wrapping the specified executor and optionally
    /// accepting additional parameters.  Run [`ExecutionScopeParams::new()`] to get an actual
    /// [`ExecutionScope`] object.
    pub fn build() -> ExecutionScopeParams {
        ExecutionScopeParams::default()
    }

    pub fn as_weak(&self) -> WeakExecutionScope {
        WeakExecutionScope {
            executor: Arc::downgrade(&self.executor),
            #[cfg(feature = "fdomain")]
            client: Arc::downgrade(&self.client),
        }
    }

    /// Sends a `task` to be executed in this execution scope.  This is very similar to
    /// [`futures::task::Spawn::spawn_obj()`] with a minor difference that `self` reference is not
    /// exclusive.
    ///
    /// If the task needs to prevent itself from being shutdown, then it should use the
    /// `try_active_guard` function below.
    ///
    /// For the "vfs" library it is more convenient that this method allows non-exclusive
    /// access.  And as the implementation is employing internal mutability there are no downsides.
    /// This way `ExecutionScope` can actually also implement [`futures::task::Spawn`] - it just was
    /// not necessary for now.
    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
        self.executor.scope().spawn(task)
    }

    /// Sends a `task` to be executed in this execution scope on the current thread.
    /// This is useful for spawning `!Send` tasks when running on a single-threaded executor.
    ///
    /// # Panics
    ///
    /// Panics if this execution scope is run on a multi-threaded executor (`SendExecutor`).
    pub fn spawn_local(&self, task: impl Future<Output = ()> + 'static) -> JoinHandle<()> {
        self.executor.scope().spawn_local(task)
    }

    /// Returns a task that can be spawned later.  The task can also be polled before spawning.
    pub fn new_task(self, task: impl Future<Output = ()> + Send + 'static) -> Task {
        Task(self.executor, SpawnableFuture::new(task))
    }

    pub fn token_registry(&self) -> &TokenRegistry {
        &self.executor.token_registry
    }

    pub fn shutdown(&self) {
        self.executor.shutdown();
    }

    /// Forcibly shut down the executor without respecting the active guards.
    pub fn force_shutdown(&self) {
        let _ = self.executor.scope().clone().abort();
    }

    /// Restores the executor so that it is no longer in the shut-down state.  Any tasks
    /// that are still running will continue to run after calling this.
    pub fn resurrect(&self) {
        // After setting the scope to None, a new scope will be created the next time `spawn` is
        // called.
        *self.executor.scope.lock() = None;
    }

    /// Wait for all tasks to complete and for there to be no guards.
    pub async fn wait(&self) {
        let scope = self.executor.scope().clone();
        scope.on_no_tasks_and_guards().await;
    }

    /// Prevents the executor from shutting down whilst the guard is held. Returns None if the
    /// executor is shutting down.
    pub fn try_active_guard(&self) -> Option<ActiveGuard> {
        self.executor.scope().active_guard()
    }
}

impl PartialEq for ExecutionScope {
    fn eq(&self, other: &Self) -> bool {
        Arc::as_ptr(&self.executor) == Arc::as_ptr(&other.executor)
    }
}

impl Eq for ExecutionScope {}

impl std::fmt::Debug for ExecutionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("ExecutionScope {:?}", Arc::as_ptr(&self.executor)))
    }
}

#[derive(Default)]
pub struct ExecutionScopeParams {
    #[cfg(target_os = "fuchsia")]
    async_executor: Option<EHandle>,
}

impl ExecutionScopeParams {
    #[cfg(target_os = "fuchsia")]
    pub fn executor(mut self, value: EHandle) -> Self {
        assert!(self.async_executor.is_none(), "`executor` is already set");
        self.async_executor = Some(value);
        self
    }

    pub fn new(
        self,
        #[cfg(feature = "fdomain")] client: Arc<flex_client::Client>,
    ) -> ExecutionScope {
        ExecutionScope {
            executor: Arc::new(Executor {
                token_registry: TokenRegistry::new(),
                #[cfg(target_os = "fuchsia")]
                scope: self.async_executor.map_or_else(
                    || Mutex::new(None),
                    |e| Mutex::new(Some(e.global_scope().new_child())),
                ),
                #[cfg(not(target_os = "fuchsia"))]
                scope: Mutex::new(None),
            }),
            #[cfg(feature = "fdomain")]
            client,
        }
    }
}

/// Holds a weak reference to the internal `ExecutionScope`, and can spawn futures on it as long as
/// the reference is still valid.
#[derive(Clone)]
pub struct WeakExecutionScope {
    executor: Weak<Executor>,
    #[cfg(feature = "fdomain")]
    client: Weak<flex_client::Client>,
}

impl WeakExecutionScope {
    /// Adds a task to the referenced [`ExecutionScope`]. The task is dropped if there are no more
    /// strong references to the original task group.
    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let executor = self.executor.upgrade();
        if let Some(executor) = executor {
            _ = executor.scope().spawn(task)
        }
    }

    /// Return the domain for handle creation for operations in this execution scope.
    #[cfg(feature = "fdomain")]
    pub fn domain(&self) -> Option<Arc<flex_client::Client>> {
        self.client.upgrade()
    }

    #[cfg(not(feature = "fdomain"))]
    pub fn domain(&self) -> Option<fidl::endpoints::ZirconClient> {
        Some(fidl::endpoints::ZirconClient)
    }
}

impl Executor {
    fn scope(&self) -> MappedMutexGuard<'_, Scope> {
        // We lazily initialize the executor rather than at construction time as there are currently
        // a few tests that create the ExecutionScope before the async executor has been initialized
        // (which means we cannot call EHandle::local()).
        MutexGuard::map(self.scope.lock(), |s| {
            s.get_or_insert_with(|| {
                #[cfg(target_os = "fuchsia")]
                return Scope::global().new_child();
                #[cfg(not(target_os = "fuchsia"))]
                return Scope::new();
            })
        })
    }

    fn shutdown(&self) {
        if let Some(scope) = &*self.scope.lock() {
            scope.wake_all_with_active_guard();
            let _ = ScopeHandle::clone(&*scope).cancel();
        }
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        self.shutdown();
        // We must detach the scope, because otherwise all the tasks will be aborted and the active
        // guards will be ignored.
        if let Some(scope) = self.scope.get_mut().take() {
            scope.detach();
        }
    }
}

/// Yields to the executor, providing an opportunity for other futures to run.
pub async fn yield_to_executor() {
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

pub struct Task(Arc<Executor>, SpawnableFuture<'static, ()>);

impl Task {
    /// Spawns the task on the scope.
    pub fn spawn(self) {
        self.0.scope().spawn(self.1);
    }
}

impl Future for Task {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut &mut self.1).poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionScope, yield_to_executor};

    use fuchsia_async::{TestExecutor, Timer};
    use futures::Future;
    use futures::channel::oneshot;
    use std::pin::pin;
    use std::sync::Arc;
    #[cfg(target_os = "fuchsia")]
    use std::sync::atomic::{AtomicBool, Ordering};
    #[cfg(target_os = "fuchsia")]
    use std::task::Poll;
    use std::time::Duration;

    #[cfg(target_os = "fuchsia")]
    fn run_test<GetTest, GetTestRes>(get_test: GetTest)
    where
        GetTest: FnOnce(ExecutionScope) -> GetTestRes,
        GetTestRes: Future<Output = ()>,
    {
        let mut exec = TestExecutor::new();

        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();

        let test = get_test(scope);

        assert_eq!(
            exec.run_until_stalled(&mut pin!(test)),
            Poll::Ready(()),
            "Test did not complete"
        );
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn run_test<GetTest, GetTestRes>(get_test: GetTest)
    where
        GetTest: FnOnce(ExecutionScope) -> GetTestRes,
        GetTestRes: Future<Output = ()>,
    {
        use fuchsia_async::TimeoutExt;
        let mut exec = TestExecutor::new();

        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();

        // This isn't a perfect equivalent to the target version, but Tokio
        // doesn't have run_until_stalled and it sounds like it's
        // architecturally impossible.
        let test =
            get_test(scope).on_stalled(Duration::from_secs(30), || panic!("Test did not complete"));

        exec.run_singlethreaded(&mut pin!(test));
    }

    #[test]
    fn simple() {
        run_test(|scope| {
            async move {
                let (sender, receiver) = oneshot::channel();
                let (counters, task) = mocks::ImmediateTask::new(sender);

                scope.spawn(task);

                // Make sure our task had a chance to execute.
                receiver.await.unwrap();

                assert_eq!(counters.drop_call(), 1);
                assert_eq!(counters.poll_call(), 1);
            }
        });
    }

    #[test]
    fn simple_drop() {
        run_test(|scope| {
            async move {
                let (poll_sender, poll_receiver) = oneshot::channel();
                let (processing_done_sender, processing_done_receiver) = oneshot::channel();
                let (drop_sender, drop_receiver) = oneshot::channel();
                let (counters, task) =
                    mocks::ControlledTask::new(poll_sender, processing_done_receiver, drop_sender);

                scope.spawn(task);

                poll_receiver.await.unwrap();

                processing_done_sender.send(()).unwrap();

                scope.shutdown();

                drop_receiver.await.unwrap();

                // poll might be called one or two times depending on the order in which the
                // executor decides to poll the two tasks (this one and the one we spawned).
                let poll_count = counters.poll_call();
                assert!(poll_count >= 1, "poll was not called");

                assert_eq!(counters.drop_call(), 1);
            }
        });
    }

    #[test]
    fn test_wait_waits_for_tasks_to_finish() {
        let mut executor = TestExecutor::new();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        executor.run_singlethreaded(async {
            let (poll_sender, poll_receiver) = oneshot::channel();
            let (processing_done_sender, processing_done_receiver) = oneshot::channel();
            let (drop_sender, _drop_receiver) = oneshot::channel();
            let (_, task) =
                mocks::ControlledTask::new(poll_sender, processing_done_receiver, drop_sender);

            scope.spawn(task);

            poll_receiver.await.unwrap();

            // We test that wait is working correctly by concurrently waiting and telling the
            // task to complete, and making sure that the order is correct.
            let done = fuchsia_sync::Mutex::new(false);
            futures::join!(
                async {
                    scope.wait().await;
                    assert_eq!(*done.lock(), true);
                },
                async {
                    // This is a Turing halting problem so the sleep is justified.
                    Timer::new(Duration::from_millis(100)).await;
                    *done.lock() = true;
                    processing_done_sender.send(()).unwrap();
                }
            );
        });
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    async fn test_shutdown_waits_for_channels() {
        use fuchsia_async as fasync;

        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let (rx, tx) = zx::Channel::create();
        let received_msg = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = futures::channel::oneshot::channel();
        {
            let received_msg = received_msg.clone();
            scope.spawn(async move {
                let mut msg_buf = zx::MessageBuf::new();
                msg_buf.ensure_capacity_bytes(64);
                let _ = sender.send(());
                let _ = fasync::Channel::from_channel(rx).recv_msg(&mut msg_buf).await;
                received_msg.store(true, Ordering::Relaxed);
            });
        }
        // Wait until the spawned future has been polled once.
        let _ = receiver.await;

        tx.write(b"hello", &mut []).expect("write failed");
        scope.shutdown();
        scope.wait().await;
        assert!(received_msg.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_force_shutdown() {
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let scope_clone = scope.clone();
        let ref_count = Arc::new(());
        let ref_count_clone = ref_count.clone();

        // Spawn a task that holds a reference.  When the task is dropped the reference will get
        // dropped with it.
        scope.spawn(async move {
            let _ref_count_clone = ref_count_clone;

            // Hold an active guard so that only a forced shutdown will work.
            let _guard = scope_clone.try_active_guard().unwrap();

            let _: () = std::future::pending().await;
        });

        scope.force_shutdown();
        scope.wait().await;

        // The task should have been dropped leaving us with the only reference.
        assert_eq!(Arc::strong_count(&ref_count), 1);

        // Test resurrection...
        scope.resurrect();

        let ref_count_clone = ref_count.clone();
        scope.spawn(async move {
            // Yield so that if the executor is in the shutdown state, it will kill this task.
            yield_to_executor().await;

            // Take another reference count so that we can check we got here below.
            let _ref_count = ref_count_clone.clone();

            let _: () = std::future::pending().await;
        });

        while Arc::strong_count(&ref_count) != 3 {
            yield_to_executor().await;
        }

        // Yield some more just to be sure the task isn't killed.
        for _ in 0..5 {
            yield_to_executor().await;
            assert_eq!(Arc::strong_count(&ref_count), 3);
        }
    }

    mod mocks {
        use futures::Future;
        use futures::channel::oneshot;
        use futures::task::{Context, Poll};
        use std::pin::Pin;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        pub(super) struct TaskCounters {
            poll_call_count: Arc<AtomicUsize>,
            drop_call_count: Arc<AtomicUsize>,
        }

        impl TaskCounters {
            fn new() -> (Arc<AtomicUsize>, Arc<AtomicUsize>, Self) {
                let poll_call_count = Arc::new(AtomicUsize::new(0));
                let drop_call_count = Arc::new(AtomicUsize::new(0));

                (
                    poll_call_count.clone(),
                    drop_call_count.clone(),
                    Self { poll_call_count, drop_call_count },
                )
            }

            pub(super) fn poll_call(&self) -> usize {
                self.poll_call_count.load(Ordering::Relaxed)
            }

            pub(super) fn drop_call(&self) -> usize {
                self.drop_call_count.load(Ordering::Relaxed)
            }
        }

        pub(super) struct ImmediateTask {
            poll_call_count: Arc<AtomicUsize>,
            drop_call_count: Arc<AtomicUsize>,
            done_sender: Option<oneshot::Sender<()>>,
        }

        impl ImmediateTask {
            pub(super) fn new(done_sender: oneshot::Sender<()>) -> (TaskCounters, Self) {
                let (poll_call_count, drop_call_count, counters) = TaskCounters::new();
                (
                    counters,
                    Self { poll_call_count, drop_call_count, done_sender: Some(done_sender) },
                )
            }
        }

        impl Future for ImmediateTask {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                self.poll_call_count.fetch_add(1, Ordering::Relaxed);

                if let Some(sender) = self.done_sender.take() {
                    sender.send(()).unwrap();
                }

                Poll::Ready(())
            }
        }

        impl Drop for ImmediateTask {
            fn drop(&mut self) {
                self.drop_call_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        impl Unpin for ImmediateTask {}

        pub(super) struct ControlledTask {
            poll_call_count: Arc<AtomicUsize>,
            drop_call_count: Arc<AtomicUsize>,

            drop_sender: Option<oneshot::Sender<()>>,
            future: Pin<Box<dyn Future<Output = ()> + Send>>,
        }

        impl ControlledTask {
            pub(super) fn new(
                poll_sender: oneshot::Sender<()>,
                processing_complete: oneshot::Receiver<()>,
                drop_sender: oneshot::Sender<()>,
            ) -> (TaskCounters, Self) {
                let (poll_call_count, drop_call_count, counters) = TaskCounters::new();
                (
                    counters,
                    Self {
                        poll_call_count,
                        drop_call_count,
                        drop_sender: Some(drop_sender),
                        future: Box::pin(async move {
                            poll_sender.send(()).unwrap();
                            processing_complete.await.unwrap();
                        }),
                    },
                )
            }
        }

        impl Future for ControlledTask {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                self.poll_call_count.fetch_add(1, Ordering::Relaxed);
                self.future.as_mut().poll(cx)
            }
        }

        impl Drop for ControlledTask {
            fn drop(&mut self) {
                self.drop_call_count.fetch_add(1, Ordering::Relaxed);
                self.drop_sender.take().unwrap().send(()).unwrap();
            }
        }
    }
}
