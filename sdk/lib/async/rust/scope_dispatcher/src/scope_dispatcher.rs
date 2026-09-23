// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ptr::{NonNull, null};
use core::sync::atomic::{self, AtomicBool};
use fuchsia_async::{EHandle, MonotonicInstant, Scope, Timer, WakeupTime};
use fuchsia_sync::Mutex;
use futures::task::AtomicWaker;
use libasync_dispatcher::{AsAsyncDispatcherRef, AsyncDispatcherRef};
use libasync_sys::async_dispatcher_t;
use pin_project_lite::pin_project;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use zx::Status;

use crate::ops;
use crate::ops::v1::{PendingWaits, Task, TaskQueue};

/// Implements a C++-compatible [`async_dispatcher_t`] around a [`fuchsia_async::Scope`].
#[derive(Debug)]
#[repr(C)]
pub struct ScopeDispatcher {
    // Safety Note: this must go first in this struct for the callbacks to work correctly.
    dispatcher: async_dispatcher_t,
    task_queue: Mutex<TaskQueue>,
    pub(crate) pending_waits: Mutex<PendingWaits>,
    shutting_down: AtomicBool,
    shutdown_complete_waker: AtomicWaker,
    service_waker: AtomicWaker,
    shutdown_guard: AtomicBool,
    executor: EHandle,
    scope: Scope,
}

// SAFETY: The API of async_dispatcher_t is expected to be thread safe.
unsafe impl Send for ScopeDispatcher {}
// SAFETY: The API of async_dispatcher_t is expected to be thread safe.
unsafe impl Sync for ScopeDispatcher {}

impl ScopeDispatcher {
    /// Creates a new [`ScopeDispatcher`] on the currently running fuchsia-async executor.
    ///
    /// # Panics
    ///
    /// Panics if this is not run on a fuchsia-async executor context.
    pub fn new() -> Arc<Self> {
        Self::new_on_executor(EHandle::local())
    }

    /// Creates a new [`ScopeDispatcher`] with a new [`Scope`] on the given `executor`.
    pub fn new_on_executor(executor: EHandle) -> Arc<Self> {
        let scope = executor.global_scope().new_child();
        let scope_handle = scope.as_handle().clone();
        let dispatcher = async_dispatcher_t { ops: &ops::ASYNC_OPS };
        let task_queue = Mutex::new(TaskQueue::default());
        let pending_waits = Mutex::new(PendingWaits::default());
        let shutting_down = AtomicBool::new(false);
        let service_waker = AtomicWaker::new();
        let shutdown_complete_waker = AtomicWaker::new();
        let shutdown_guard = AtomicBool::new(false);
        let this = Arc::new(Self {
            dispatcher,
            task_queue,
            pending_waits,
            shutting_down,
            shutdown_complete_waker,
            service_waker,
            shutdown_guard,
            executor,
            scope,
        });

        scope_handle.spawn(this.clone().service_loop());

        this
    }

    /// Get the pointer to the dispatcher callback struct for passing through FFI layers.
    pub fn as_ptr(&self) -> *const async_dispatcher_t {
        (self as *const Self).cast()
    }

    /// Gets the global executor handle for this dispatcher.
    pub fn global_handle(&self) -> &EHandle {
        &self.executor
    }

    /// Gets the [`fuchsia_async::Scope`] of this dispatcher.
    pub fn as_scope(&self) -> &Scope {
        &self.scope
    }

    /// Returns true if the dispatcher is currently shutting down.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(atomic::Ordering::Acquire)
    }

    /// Starts the dispatcher shutdown. Resolves when all outstanding tasks have been completed or
    /// canceled.
    pub fn shutdown(&self) -> ShutdownCompletionFuture<'_> {
        // note: we might want to do more to prevent multiple attempts to shut the dispatcher down.
        self.shutting_down.store(true, atomic::Ordering::Release);
        self.service_waker.wake();
        ShutdownCompletionFuture(self)
    }

    /// Gets the Scope from a dispatcher pointer. Used in the callbacks.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the dispatcher pointer is a valid pointer originally obtained
    /// through [`ScopeDispatcher::as_ptr`], and is still alive.
    pub(crate) unsafe fn from_ptr<'a>(dispatcher_ptr: *mut async_dispatcher_t) -> &'a Self {
        let this = dispatcher_ptr.cast::<ScopeDispatcher>();
        // Safety: the caller promises that this is a valid pointer to what was originally a
        // ScopeDispatcher object.
        unsafe { this.as_ref() }.expect("null dispatcher pointer")
    }

    /// Gets the Scope from a dispatcher pointer. Used in the callbacks.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the dispatcher pointer is a valid pointer originally obtained
    /// through [`ScopeDispatcher::as_ptr`].
    pub(crate) unsafe fn arc_from_ptr(dispatcher_ptr: *mut async_dispatcher_t) -> Arc<Self> {
        let this = dispatcher_ptr.cast::<ScopeDispatcher>();
        // Safety: the caller promises that this is a valid pointer to what was originally a
        // ScopeDispatcher object.
        unsafe {
            Arc::increment_strong_count(this);
            Arc::from_raw(this)
        }
    }

    /// Posts a task to the dispatcher
    pub(crate) fn post_task(&self, task: Task) -> Result<(), Status> {
        // don't queue new tasks if we're shutting down.
        if self.is_shutting_down() {
            return Err(Status::BAD_STATE);
        }
        self.task_queue.lock().queue_task(task);
        self.service_waker.wake();
        Ok(())
    }

    /// Cancels a task queued on the dispatcher
    pub(crate) fn cancel_task(&self, task: Task) -> Result<(), Status> {
        // If we succeed at cancelling, we won't call the callback so this can be fairly simple.
        if self.task_queue.lock().take_pending_task(&task).is_some() {
            self.service_waker.wake();
            Ok(())
        } else {
            Err(Status::NOT_FOUND)
        }
    }

    async fn service_loop(self: Arc<Self>) {
        while let Some(next_task) = NextTaskFuture::new(&self).await {
            next_task.run(self.clone(), Ok(()));
        }
        // we're shutting down, so drain the queues of all outstanding tasks with
        // a status of CANCELED. Note that we don't really care about fanning these out to all
        // threads, so we just run them directly here.
        // Note also that we will not allow any new items to be added to the queues after the
        // shutdown flag has been set, so we don't have to worry about new things being added at
        // this point.
        for next_wait in self.pending_waits.lock().get_all_waits() {
            next_wait.run(self.clone(), null(), Err(Status::CANCELED));
        }
        while let Some(next_task) = self.task_queue.lock().next_task(MonotonicInstant::INFINITE) {
            next_task.run(self.clone(), Err(Status::CANCELED));
        }
        self.shutdown_guard.store(true, atomic::Ordering::Release);
        self.shutdown_complete_waker.wake();
    }
}

impl AsAsyncDispatcherRef for ScopeDispatcher {
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_> {
        // SAFETY: We know this pointer is valid because it is a member of `&self`, which is a valid
        // reference.
        let ptr = unsafe { NonNull::new_unchecked(self.as_ptr().cast_mut()) };
        // SAFETY: The dispatcher ref's lifetime is tied to `self`, of which the dispatcher
        // structure and callbacks are members, so will not outlive them.
        unsafe { AsyncDispatcherRef::from_raw(ptr) }
    }
}

impl Drop for ScopeDispatcher {
    fn drop(&mut self) {
        assert!(
            self.shutdown_guard.load(atomic::Ordering::Acquire),
            "Dispatcher not properly shut down before dropping. Call ScopeDispatcher::shutdown()."
        );
    }
}

/// A future which resolves when the dispatcher has been shut down by [`ScopeDispatcher::shutdown`].
#[must_use = "a future that is never awaited on will never run"]
pub struct ShutdownCompletionFuture<'a>(&'a ScopeDispatcher);

impl<'a> Future for ShutdownCompletionFuture<'a> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.shutdown_complete_waker.register(ctx.waker());
        if self.0.shutdown_guard.load(atomic::Ordering::Acquire) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

pin_project! {
    #[must_use = "a future that is never awaited on will never run"]
    struct NextTaskFuture<'a> {
        dispatcher: &'a Arc<ScopeDispatcher>,
        #[pin]
        next_timeout: Timer,
    }
}

impl<'a> NextTaskFuture<'a> {
    fn new(dispatcher: &'a Arc<ScopeDispatcher>) -> Self {
        let next_timeout = MonotonicInstant::INFINITE.into_timer();
        Self { dispatcher, next_timeout }
    }
}

impl<'a> Future for NextTaskFuture<'a> {
    type Output = Option<Task>;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut task_queue = self.dispatcher.task_queue.lock();
        // if we are shutting down, the service handler will do the work of canceling the remaining
        // tasks, so return None to indicate that it should start doing that.
        if self.dispatcher.shutting_down.load(atomic::Ordering::Acquire) {
            return Poll::Ready(None);
        }
        let now = self.dispatcher.executor.now();
        if let Some(task) = task_queue.next_task(now) {
            Poll::Ready(Some(task))
        } else {
            let next_deadline = if let Some(task) = task_queue.peek_next_task() {
                task.deadline().unwrap_or(MonotonicInstant::INFINITE)
            } else {
                MonotonicInstant::INFINITE
            };
            self.dispatcher.service_waker.register(ctx.waker());
            let mut this = self.project();
            this.next_timeout.as_mut().reset(next_deadline);
            // Note that we don't really care about resolving the timer, we just want to use its
            // waker to re-awaken this future when we're ready.
            let _: Poll<()> = this.next_timeout.poll(ctx);
            Poll::Pending
        }
    }
}
