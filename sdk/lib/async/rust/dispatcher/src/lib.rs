// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for the C libasync async dispatcher library

#![deny(missing_docs, clippy::undocumented_unsafe_blocks)]

use libasync_sys::*;

use core::cell::UnsafeCell;
use core::future::Future;
use core::marker::PhantomData;
use core::ptr::NonNull;
use std::sync::Arc;

use zx_status::Status;
use zx_types::zx_time_t;

mod current_dispatcher;
mod detect_dispatcher;
mod task;

pub use current_dispatcher::*;
pub use detect_dispatcher::*;
pub use task::*;

/// A reference to a dispatcher that supports the v4 async api's reference counting operations,
/// and so can be held safely without a lifetime.
#[derive(Debug)]
pub struct AsyncDispatcher(NonNull<async_dispatcher_t>);

// SAFETY: It is safe to access an `async_dispatcher_t` from any thread per the libasync C api.
unsafe impl Send for AsyncDispatcher {}
// SAFETY: It is safe to access an `async_dispatcher_t` from any thread per the libasync C api.
unsafe impl Sync for AsyncDispatcher {}

impl AsyncDispatcher {
    /// Converts from something that implements [`AsAsyncDispatcherRef`] to an [`AsyncDispatcher`]
    /// if it implements the v4 async api's reference counting.
    ///
    /// # Panics
    ///
    /// This will panic if the implementation does not support reference counting. If you need to be
    /// able to deal with a dispatcher that might not implement this api, you can use
    /// [`AsyncDispatcher::new`].
    pub fn new(dispatcher: &impl AsAsyncDispatcherRef) -> Self {
        Self::try_new(dispatcher).expect("Dispatcher does not implement reference counting")
    }

    /// Converts from something that implements [`AsAsyncDispatcherRef`] to an [`AsyncDispatcher`]
    /// if it implements the v4 async api's reference counting.
    ///
    /// Returns [`Status::UNSUPPORTED`] if the dispatcher does not support refcounting.
    pub fn try_new(dispatcher: &impl AsAsyncDispatcherRef) -> Result<Self, Status> {
        let dispatcher = dispatcher.as_async_dispatcher_ref();
        // SAFETY: The dispatcher is a valid reference to a live dispatcher by construction, and
        // we will only return a new Self if the call succeeds, so we will not release an invalid
        // reference.
        Status::ok(unsafe { libasync_sys::async_acquire_shared_ref(dispatcher.0.as_ptr()) })?;
        Ok(Self(dispatcher.0))
    }

    /// Returns the current time on the dispatcher's timeline
    pub fn now(&self) -> zx_time_t {
        let async_dispatcher = self.as_ptr().as_ptr();
        // SAFETY: The dispatcher is a valid reference to a live dispatcher by construction, and
        // this function does not touch any rust memory.
        unsafe { async_now(async_dispatcher) }
    }

    /// Gets the inner pointer to the dispatcher struct.
    pub fn as_ptr(&self) -> NonNull<async_dispatcher_t> {
        self.0
    }
}

impl Clone for AsyncDispatcher {
    fn clone(&self) -> Self {
        Self::new(self)
    }
}

impl Drop for AsyncDispatcher {
    fn drop(&mut self) {
        // SAFETY: The dispatcher is a valid reference to a live dispatcher by construction, and
        // we have already successfully acquired the shared reference to it in [`Self::try_new`].
        Status::ok(unsafe { libasync_sys::async_release_shared_ref(self.0.as_ptr()) })
            .expect("attempted to release shared dispatcher ref that doesn't support refcounting");
    }
}

impl AsAsyncDispatcherRef for AsyncDispatcher {
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_> {
        AsyncDispatcherRef(self.0, PhantomData)
    }
}

/// An unowned reference to a driver runtime dispatcher such as is produced by calling
/// [`AsyncDispatcher::release`]. When this object goes out of scope it won't shut down the dispatcher,
/// leaving that up to the driver runtime or another owner.
#[derive(Debug, Copy, Clone)]
pub struct AsyncDispatcherRef<'a>(NonNull<async_dispatcher_t>, PhantomData<&'a async_dispatcher_t>);

// SAFETY: It is safe to access an `async_dispatcher_t` from any thread per the libasync C api.
unsafe impl<'a> Send for AsyncDispatcherRef<'a> {}
// SAFETY: It is safe to access an `async_dispatcher_t` from any thread per the libasync C api.
unsafe impl<'a> Sync for AsyncDispatcherRef<'a> {}

impl<'a> AsyncDispatcherRef<'a> {
    /// Creates a dispatcher ref from a raw ptr.
    ///
    /// # Safety
    ///
    /// Caller is responsible for ensuring that the given ptr is valid for
    /// the lifetime `'a`.
    pub unsafe fn from_raw(ptr: NonNull<async_dispatcher_t>) -> Self {
        // SAFETY: Caller promises the ptr is valid.
        Self(ptr, PhantomData)
    }

    /// Gets the inner pointer to the dispatcher struct.
    pub fn inner(&self) -> NonNull<async_dispatcher_t> {
        self.0
    }

    /// Returns the current time on the dispatcher's timeline
    pub fn now(&self) -> zx_time_t {
        let async_dispatcher = self.inner().as_ptr();
        // SAFETY: The dispatcher is a valid reference to a live dispatcher by construction, and
        // this function does not touch any rust memory.
        unsafe { async_now(async_dispatcher) }
    }
}

/// A trait for things that can be represented as an [`AsyncDispatcherRef`].
pub trait AsAsyncDispatcherRef: Send + Sync {
    /// Gets an [`AsyncDispatcherRef`] corresponding to this object.
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_>;

    /// Schedules the callback [`p`] to be run on this dispatcher later.
    fn post_task_sync(&self, p: impl TaskCallback) -> Result<(), Status> {
        #[expect(clippy::arc_with_non_send_sync)]
        let task_arc = Arc::new(UnsafeCell::new(TaskFunc {
            task: async_task { handler: Some(TaskFunc::call), ..Default::default() },
            func: Box::new(p),
        }));

        let task_cell = Arc::into_raw(task_arc);
        // SAFETY: we need a raw mut pointer to give to async_post_task. From
        // when we call that function to when the task is cancelled or the
        // callback is called, the driver runtime owns the contents of that
        // object and we will not manipulate it. So even though the Arc only
        // gives us a shared reference, it's fine to give the runtime a
        // mutable pointer to it.
        let res = unsafe {
            let task_ptr = &raw mut (*UnsafeCell::raw_get(task_cell)).task;
            Status::ok(async_post_task(self.as_async_dispatcher_ref().0.as_ptr(), task_ptr))
        };
        if res.is_err() {
            // SAFETY: `TaskFunc::call` will never be called now so dispose of
            // the long-lived reference we just created.
            unsafe { Arc::decrement_strong_count(task_cell) }
        }
        res
    }
}

impl<T> AsAsyncDispatcherRef for Arc<T>
where
    T: AsAsyncDispatcherRef,
{
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_> {
        (**self).as_async_dispatcher_ref()
    }
}

impl<'a> AsAsyncDispatcherRef for AsyncDispatcherRef<'a> {
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_> {
        *self
    }
}

/// A trait for things that can be represented as an [`AsyncDispatcher`].
///
/// This is automatically implemented for things that implement [`AsAsyncDispatcherRef`],
/// but may be implemented by other things that have more logic to how they obtain the correct
/// dispatcher object.
pub trait GetAsyncDispatcher {
    /// Returns a refcounted handle to the active dispatcher for this object, if there is one.
    /// Some types of dispatchers (like for the current dispatcher of a thread) may not always have
    /// an active dispatcher, so it is returned as an option.
    fn try_get_async_dispatcher(&self) -> Option<AsyncDispatcher>;

    /// Returns a refcounted handle to the active dispatcher for this object.
    ///
    /// # Panics
    ///
    /// Some types of dispatchers (like for the current dispatcher of a thread) may not always have
    /// an active dispatcher, in which case this will panic. If you need to be able to handle there
    /// not being an active dispatcher, use [`Self::try_get_async_dispatcher`].
    fn get_async_dispatcher(&self) -> AsyncDispatcher {
        self.try_get_async_dispatcher().expect("No current async dispatcher")
    }
}

impl<T> GetAsyncDispatcher for T
where
    T: AsAsyncDispatcherRef,
{
    fn try_get_async_dispatcher(&self) -> Option<AsyncDispatcher> {
        Some(AsyncDispatcher::new(self))
    }
}

/// A trait that can be used to access a lifetime-constrained dispatcher in a generic way.
pub trait OnDispatcher: GetAsyncDispatcher + Clone + Send + Sync {
    /// Runs the function `f` with a lifetime-bound [`AsyncDispatcherRef`] for this object's dispatcher.
    /// If the dispatcher is no longer valid, the callback will be given [`None`].
    ///
    /// Note that it is *very important* that no blocking work be done in this callback to prevent
    /// long lived strong references to dispatchers that might be shutting down.
    fn on_dispatcher<R>(&self, f: impl FnOnce(Option<AsyncDispatcherRef<'_>>) -> R) -> R;

    /// Helper version of [`OnDispatcher::on_dispatcher`] that translates an invalidated dispatcher
    /// handle into a [`Status::BAD_STATE`] error instead of giving the callback [`None`].
    ///
    /// Note that it is *very important* that no blocking work be done in this callback to prevent
    /// long lived strong references to dispatchers that might be shutting down.
    fn on_maybe_dispatcher<R, E: From<Status>>(
        &self,
        f: impl FnOnce(AsyncDispatcherRef<'_>) -> Result<R, E>,
    ) -> Result<R, E>;

    /// Spawn an asynchronous task on this dispatcher. If this returns [`Ok`] then the task has
    /// successfully been scheduled and will run or be cancelled and dropped when the dispatcher
    /// shuts down. The returned future's result will be [`Ok`] if the future completed
    /// successfully, or an [`Err`] if the task did not complete for some reason (like the
    /// dispatcher shut down).
    ///
    /// Returns a [`JoinHandle`] that will detach the future when dropped.
    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()>
    where
        Self: 'static;

    /// Spawn an asynchronous task that outputs type 'T' on this dispatcher. The returned future's
    /// result will be [`Ok`] if the task was started and completed successfully, or an [`Err`] if
    /// the task couldn't be started or failed to complete (for example because the dispatcher was
    /// shutting down).
    ///
    /// Returns a [`Task`] that will cancel the future when dropped.
    ///
    /// TODO(470088116): This may be the cause of some flakes, so care should be used with it
    /// in critical paths for now.
    fn compute<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T>
    where
        Self: 'static;
}

impl<D: GetAsyncDispatcher + Clone + Send + Sync> OnDispatcher for D {
    fn on_dispatcher<R>(&self, f: impl FnOnce(Option<AsyncDispatcherRef<'_>>) -> R) -> R {
        if let Some(dispatcher) = self.try_get_async_dispatcher() {
            f(Some(dispatcher.as_async_dispatcher_ref()))
        } else {
            f(None)
        }
    }

    fn on_maybe_dispatcher<R, E: From<Status>>(
        &self,
        f: impl FnOnce(AsyncDispatcherRef<'_>) -> Result<R, E>,
    ) -> Result<R, E> {
        self.on_dispatcher(|dispatcher| {
            let dispatcher = dispatcher.ok_or(Status::BAD_STATE)?;
            f(dispatcher)
        })
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()>
    where
        Self: 'static,
    {
        self.compute(future).detach_on_drop()
    }

    fn compute<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T>
    where
        Self: 'static,
    {
        match self.try_get_async_dispatcher() {
            Some(dispatcher) => Task::start(future, dispatcher),
            None => Task::new_failed(Status::BAD_STATE),
        }
    }
}

/// A marker trait for a callback that can be used with [`Dispatcher::post_task_sync`].
pub trait TaskCallback: FnOnce(Result<(), Status>) + 'static + Send {}
impl<T> TaskCallback for T where T: FnOnce(Result<(), Status>) + 'static + Send {}

#[repr(C)]
struct TaskFunc {
    task: async_task,
    func: Box<dyn TaskCallback>,
}

impl TaskFunc {
    extern "C" fn call(dispatcher: *mut async_dispatcher, task: *mut async_task, status: i32) {
        // SAFETY: The async api will only call this function on a valid dispatcher (even if it's
        // shutting down).
        let dispatcher =
            unsafe { AsyncDispatcherRef::from_raw(NonNull::new_unchecked(dispatcher)) };
        // SAFETY: the async api promises that this function will only be called
        // up to once, so we can reconstitute the `Arc` and let it get dropped.
        let task = unsafe { Arc::from_raw(task as *const UnsafeCell<Self>) };
        // SAFETY: if we can't get a mut ref from the arc, then the task is already
        // being cancelled, so we don't want to call it.
        if let Ok(task) = Arc::try_unwrap(task) {
            CurrentDispatcher::with(&dispatcher, move || {
                (task.into_inner().func)(Status::ok(status));
            });
        }
    }
}
