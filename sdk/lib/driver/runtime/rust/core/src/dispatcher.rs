// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for the driver runtime dispatcher stable ABI

use fdf_sys::*;
use libasync_dispatcher::{
    AsAsyncDispatcherRef, AsyncDispatcher, AsyncDispatcherRef, GetAsyncDispatcher, JoinHandle,
    OnDispatcher, Task,
};

use core::cell::RefCell;
use core::ffi;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ptr::{NonNull, null_mut};

use zx::Status;

use crate::shutdown_observer::ShutdownObserver;

pub use fdf_sys::fdf_dispatcher_t;

/// A marker trait for a function type that can be used as a shutdown observer for [`Dispatcher`].
pub trait ShutdownObserverFn: FnOnce(DriverDispatcherRef<'_>) + Send + 'static {}
impl<T> ShutdownObserverFn for T where T: FnOnce(DriverDispatcherRef<'_>) + Send + 'static {}

/// A builder for [`Dispatcher`]s
#[derive(Default)]
pub struct DispatcherBuilder {
    #[doc(hidden)]
    pub options: u32,
    #[doc(hidden)]
    pub name: String,
    #[doc(hidden)]
    pub scheduler_role: String,
    #[doc(hidden)]
    pub shutdown_observer: Option<Box<dyn ShutdownObserverFn>>,
}

impl DispatcherBuilder {
    /// See `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED` in the C API
    pub(crate) const UNSYNCHRONIZED: u32 = fdf_sys::FDF_DISPATCHER_OPTION_UNSYNCHRONIZED;
    /// See `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS` in the C API
    pub(crate) const ALLOW_THREAD_BLOCKING: u32 = fdf_sys::FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS;
    /// See `FDF_DISPATCHER_OPTION_NO_THREAD_MIGRATION` in the C API
    pub(crate) const NO_THREAD_MIGRATION: u32 = fdf_sys::FDF_DISPATCHER_OPTION_NO_THREAD_MIGRATION;

    /// Creates a new [`DispatcherBuilder`] that can be used to configure a new dispatcher.
    /// For more information on the threading-related flags for the dispatcher, see
    /// https://fuchsia.dev/fuchsia-src/concepts/drivers/driver-dispatcher-and-threads
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets whether parallel callbacks in the callbacks set in the dispatcher are allowed. May
    /// not be set with [`Self::allow_thread_blocking`].
    ///
    /// See https://fuchsia.dev/fuchsia-src/concepts/drivers/driver-dispatcher-and-threads
    /// for more information on the threading model of driver dispatchers.
    pub fn unsynchronized(mut self) -> Self {
        assert!(
            !self.allows_thread_blocking(),
            "you may not create an unsynchronized dispatcher that allows synchronous calls"
        );
        self.options |= Self::UNSYNCHRONIZED;
        self
    }

    /// Whether or not this is an unsynchronized dispatcher
    pub fn is_unsynchronized(&self) -> bool {
        (self.options & Self::UNSYNCHRONIZED) == Self::UNSYNCHRONIZED
    }

    /// This dispatcher may not share zircon threads with other drivers. May not be set with
    /// [`Self::unsynchronized`].
    ///
    /// See https://fuchsia.dev/fuchsia-src/concepts/drivers/driver-dispatcher-and-threads
    /// for more information on the threading model of driver dispatchers.
    pub fn allow_thread_blocking(mut self) -> Self {
        assert!(
            !self.is_unsynchronized(),
            "you may not create an unsynchronized dispatcher that allows synchronous calls"
        );
        self.options |= Self::ALLOW_THREAD_BLOCKING;
        self
    }

    /// Whether or not this dispatcher allows synchronous calls
    pub fn allows_thread_blocking(&self) -> bool {
        (self.options & Self::ALLOW_THREAD_BLOCKING) == Self::ALLOW_THREAD_BLOCKING
    }

    /// This dispatcher may not run on more than one thread. This can only be set if the
    /// dispatcher is being run on a scheduler role that does not allow sync calls on
    /// any of its dispatchers.
    ///
    /// See https://fuchsia.dev/fuchsia-src/concepts/drivers/driver-dispatcher-and-threads
    /// for more information on the threading model of driver dispatchers.
    pub fn no_thread_migration(mut self) -> Self {
        self.options |= Self::NO_THREAD_MIGRATION;
        self
    }

    /// Whether or not this dispatcher is allowed to run on multiple threads
    pub fn allows_thread_migration(&self) -> bool {
        (self.options & Self::NO_THREAD_MIGRATION) == 0
    }

    /// A descriptive name for this dispatcher that is used in debug output and process
    /// lists.
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// A hint string for the runtime that may or may not impact the priority the work scheduled
    /// by this dispatcher is handled at. It may or may not impact the ability for other drivers
    /// to share zircon threads with the dispatcher.
    pub fn scheduler_role(mut self, role: &str) -> Self {
        self.scheduler_role = role.to_string();
        self
    }

    /// A callback to be called before after the dispatcher has completed asynchronous shutdown.
    pub fn shutdown_observer<F: ShutdownObserverFn>(mut self, shutdown_observer: F) -> Self {
        self.shutdown_observer = Some(Box::new(shutdown_observer));
        self
    }

    /// Create the dispatcher as configured by this object. This must be called from a
    /// thread managed by the driver runtime. The dispatcher returned is owned by the caller,
    /// and will initiate asynchronous shutdown when the object is dropped unless
    /// [`Dispatcher::release`] is called on it to convert it into an unowned [`DispatcherRef`].
    pub fn create(self) -> Result<Dispatcher, Status> {
        let mut out_dispatcher = null_mut();
        let options = self.options;
        let name = self.name.as_ptr() as *mut ffi::c_char;
        let name_len = self.name.len();
        let scheduler_role = self.scheduler_role.as_ptr() as *mut ffi::c_char;
        let scheduler_role_len = self.scheduler_role.len();
        let observer =
            ShutdownObserver::new(self.shutdown_observer.unwrap_or_else(|| Box::new(|_| {})))
                .into_ptr();
        // SAFETY: all arguments point to memory that will be available for the duration
        // of the call, except `observer`, which will be available until it is unallocated
        // by the dispatcher exit handler.
        Status::ok(unsafe {
            fdf_dispatcher_create(
                options,
                name,
                name_len,
                scheduler_role,
                scheduler_role_len,
                observer,
                &mut out_dispatcher,
            )
        })?;
        // SAFETY: `out_dispatcher` is valid by construction if `fdf_dispatcher_create` returns
        // ZX_OK.
        Ok(Dispatcher(unsafe { NonNull::new_unchecked(out_dispatcher) }))
    }

    /// As with [`Self::create`], this creates a new dispatcher as configured by this object, but
    /// instead of returning an owned reference it immediately releases the reference to be
    /// managed by the driver runtime.
    pub fn create_released(self) -> Result<AutoReleaseDispatcher, Status> {
        self.create().map(Dispatcher::release)
    }
}

/// An owned handle for a dispatcher managed by the driver runtime.
#[derive(Debug)]
pub struct Dispatcher(pub(crate) NonNull<fdf_dispatcher_t>);

// SAFETY: The api of fdf_dispatcher_t is thread safe.
unsafe impl Send for Dispatcher {}
unsafe impl Sync for Dispatcher {}
thread_local! {
    pub(crate) static OVERRIDE_DISPATCHER: RefCell<Option<NonNull<fdf_dispatcher_t>>> = const { RefCell::new(None) };
}

impl Dispatcher {
    /// Creates a dispatcher ref from a raw handle.
    ///
    /// # Safety
    ///
    /// Caller is responsible for ensuring that the given handle is valid and
    /// not owned by any other wrapper that will free it at an arbitrary
    /// time.
    pub unsafe fn from_raw(handle: NonNull<fdf_dispatcher_t>) -> Self {
        Self(handle)
    }

    fn get_raw_flags(&self) -> u32 {
        // SAFETY: the inner fdf_dispatcher_t is valid by construction
        unsafe { fdf_dispatcher_get_options(self.0.as_ptr()) }
    }

    /// Whether this dispatcher's tasks and futures can run on multiple threads at the same time.
    pub fn is_unsynchronized(&self) -> bool {
        (self.get_raw_flags() & DispatcherBuilder::UNSYNCHRONIZED) != 0
    }

    /// Whether this dispatcher is allowed to call blocking functions or not
    pub fn allows_thread_blocking(&self) -> bool {
        (self.get_raw_flags() & DispatcherBuilder::ALLOW_THREAD_BLOCKING) != 0
    }

    /// Whether this dispatcher is allowed to migrate threads, in which case it can't
    /// be used for non-[`Send`] tasks.
    pub fn allows_thread_migration(&self) -> bool {
        (self.get_raw_flags() & DispatcherBuilder::NO_THREAD_MIGRATION) == 0
    }

    /// Whether this is the dispatcher the current thread is running on
    pub fn is_current_dispatcher(&self) -> bool {
        // SAFETY: we don't do anything with the dispatcher pointer, and NULL is returned if this
        // isn't a dispatcher-managed thread.
        self.0.as_ptr() == unsafe { fdf_dispatcher_get_current_dispatcher() }
    }

    /// Releases ownership over this dispatcher and returns a [`DispatcherRef`]
    /// that can be used to access it. The lifetime of this reference is static because it will
    /// exist so long as this current driver is loaded, but the driver runtime will shut it down
    /// when the driver is unloaded.
    pub fn release(self) -> AutoReleaseDispatcher {
        AutoReleaseDispatcher { dispatcher: ManuallyDrop::new(self) }
    }

    /// Returns a [`DispatcherRef`] that references this dispatcher with a lifetime constrained by
    /// `self`.
    pub fn as_dispatcher_ref(&self) -> DriverDispatcherRef<'_> {
        DriverDispatcherRef(ManuallyDrop::new(Dispatcher(self.0)), PhantomData)
    }
}

impl AsAsyncDispatcherRef for Dispatcher {
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_> {
        let async_dispatcher =
            NonNull::new(unsafe { fdf_dispatcher_get_async_dispatcher(self.0.as_ptr()) })
                .expect("No async dispatcher on driver dispatcher");
        unsafe { AsyncDispatcherRef::from_raw(async_dispatcher) }
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        // SAFETY: we only ever provide an owned `Dispatcher` to one owner, so when
        // that one is dropped we can invoke the shutdown of the dispatcher
        unsafe { fdf_dispatcher_shutdown_async(self.0.as_mut()) }
    }
}

/// An owned reference to a driver runtime dispatcher that auto-releases when dropped. This gives
/// you the best of both worlds of having an `Arc<Dispatcher>` and a `DispatcherRef<'static>`
/// created by [`Dispatcher::release`]:
///
/// - You can vend [`Weak`]-like pointers to it that will not cause memory access errors if used
///   after the dispatcher has shut down, like an [`Arc`].
/// - You can tie its terminal lifetime to that of the driver itself.
///
/// This is particularly useful in tests.
#[derive(Debug)]
pub struct AutoReleaseDispatcher {
    dispatcher: ManuallyDrop<Dispatcher>,
}

impl AutoReleaseDispatcher {
    /// Creates a dispatcher ref from a raw handle.
    ///
    /// # Safety
    ///
    /// Caller is responsible for ensuring that the given handle is valid and
    /// not owned by any other wrapper that will free it at an arbitrary
    /// time.
    pub unsafe fn from_raw(dispatcher: NonNull<fdf_dispatcher_t>) -> Self {
        let dispatcher = ManuallyDrop::new(Dispatcher(dispatcher));
        Self { dispatcher }
    }

    /// Returns a weakened reference to this dispatcher. This weak reference will only be valid so
    /// long as the dispatcher is shutting down, after which it will no longer be usable to spawn
    /// tasks on.
    pub fn as_async_dispatcher(&self) -> AsyncDispatcher {
        AsyncDispatcher::new(self)
    }

    /// Returns a [`DispatcherRef`] that references this dispatcher with a lifetime constrained by
    /// `self`.
    pub fn as_dispatcher_ref(&self) -> DriverDispatcherRef<'_> {
        DriverDispatcherRef(ManuallyDrop::new(Dispatcher(self.dispatcher.0)), PhantomData)
    }

    /// Returns the Always-On interface of this dispatcher.
    pub fn always_on_dispatcher(&self) -> AutoReleaseDispatcher {
        // SAFETY: `self.dispatcher.0` is a valid, active `fdf_dispatcher_t` pointer owned by this
        // `AutoReleaseDispatcher`.
        let dispatcher_ref = unsafe { DriverDispatcherRef::from_raw(self.dispatcher.0) };
        // SAFETY: The always-on dispatcher pointer returned by the runtime is guaranteed to remain
        // valid for at least as long as the parent dispatcher is alive. Since this is an
        // `AutoReleaseDispatcher`, the underlying dispatcher will not be shut down when dropped,
        // and we wrap the new dispatcher in `ManuallyDrop` to ensure the same.
        let dispatcher = unsafe { Dispatcher::from_raw(dispatcher_ref.always_on_dispatcher().0.0) };
        Self { dispatcher: ManuallyDrop::new(dispatcher) }
    }
}

impl AsAsyncDispatcherRef for AutoReleaseDispatcher {
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_> {
        self.dispatcher.as_async_dispatcher_ref()
    }
}

impl From<Dispatcher> for AutoReleaseDispatcher {
    fn from(dispatcher: Dispatcher) -> Self {
        Self { dispatcher: ManuallyDrop::new(dispatcher) }
    }
}

/// An unowned reference to a driver runtime dispatcher such as is produced by calling
/// [`Dispatcher::release`]. When this object goes out of scope it won't shut down the dispatcher,
/// leaving that up to the driver runtime or another owner.
#[derive(Debug)]
pub struct DriverDispatcherRef<'a>(ManuallyDrop<Dispatcher>, PhantomData<&'a Dispatcher>);

impl<'a> DriverDispatcherRef<'a> {
    /// Creates a dispatcher ref from a raw handle.
    ///
    /// # Safety
    ///
    /// Caller is responsible for ensuring that the given handle is valid for
    /// the lifetime `'a`.
    pub unsafe fn from_raw(handle: NonNull<fdf_dispatcher_t>) -> Self {
        // SAFETY: Caller promises the handle is valid.
        Self(ManuallyDrop::new(unsafe { Dispatcher::from_raw(handle) }), PhantomData)
    }

    /// Creates a dispatcher ref from an [`AsyncDispatcherRef`].
    ///
    /// # Panics
    ///
    /// Note that this will cause an assert if the [`AsyncDispatcherRef`] was not created from a
    /// driver dispatcher in the first place.
    pub fn from_async_dispatcher(dispatcher: AsyncDispatcherRef<'a>) -> Self {
        let handle = NonNull::new(unsafe {
            fdf_dispatcher_downcast_async_dispatcher(dispatcher.inner().as_ptr())
        })
        .unwrap();
        unsafe { Self::from_raw(handle) }
    }

    /// Gets the raw handle from this dispatcher ref.
    ///
    /// # Safety
    ///
    /// Caller is responsible for ensuring that the dispatcher handle is used safely.
    pub unsafe fn as_raw(&mut self) -> *mut fdf_dispatcher_t {
        unsafe { self.0.0.as_mut() }
    }

    /// Returns a [`DispatcherRef`] for the always-on dispatcher associated with this dispatcher,
    /// preserving the lifetime parameter of the parent dispatcher.
    pub fn always_on_dispatcher(&self) -> DriverDispatcherRef<'a> {
        // SAFETY: The pointer being passed in is valid as its coming from a DispatcherRef.
        let ptr = unsafe { fdf_dispatcher_get_always_on_dispatcher(self.0.0.as_ptr()) };
        DriverDispatcherRef(
            ManuallyDrop::new(Dispatcher(NonNull::new(ptr).expect("Always-on dispatcher is NULL"))),
            PhantomData,
        )
    }
}

/// Used to wrap a non-send future as send when we've dynamically checked that the dispatcher
/// we're going to spawn it on is non-[`Send`]-safe.
///
/// This should only ever be used after validating that the dispatcher is the currently running
/// one and that the dispatcher does not migrate threads.
///
/// This is an internal implementation detail and should never be made public.
struct AddSendFuture<T>(T);

impl<T: Future> Future for AddSendFuture<T> {
    type Output = T::Output;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: self.0 is pinned if self is.
        let fut = unsafe { self.map_unchecked_mut(|fut| &mut fut.0) };
        fut.poll(cx)
    }
}

// SAFETY: We are forcing this future to be [`Send`] even though the inner future is not because
// we validate at runtime before spawning the task that the dispatcher is correctly configured to
// do the right thing with it.
unsafe impl<T> Send for AddSendFuture<T> {}

/// Makes available additional functionality available on driver dispatchers on top of what's
/// available on [`OnDispatcher`].
pub trait OnDriverDispatcher: OnDispatcher {
    /// Spawn an asynchronous local task on this dispatcher. If this returns [`Ok`] then the task
    /// has successfully been scheduled and will run or be cancelled and dropped when the dispatcher
    /// shuts down. The returned future's result will be [`Ok`] if the future completed
    /// successfully, or an [`Err`] if the task did not complete for some reason (like the
    /// dispatcher shut down).
    ///
    /// Unlike [`OnDispatcher::spawn`], this will accept a future that does not implement [`Send`]. If
    /// called from a thread other than the one the dispatcher is running on or the dispatcher
    /// is not guaranteed to always poll from the same thread, this will return
    /// [`Status::BAD_STATE`].
    ///
    /// Returns a [`JoinHandle`] that will detach the future when dropped.
    fn spawn_local(&self, future: impl Future<Output = ()> + 'static) -> JoinHandle<()>
    where
        Self: 'static,
    {
        self.compute_local(future).detach_on_drop()
    }

    /// Spawn a local asynchronous task that outputs type 'T' on this dispatcher. The returned future's
    /// result will be [`Ok`] if the task was started and completed successfully, or an [`Err`] if
    /// the task couldn't be started or failed to complete (for example because the dispatcher was
    /// shutting down).
    ///
    /// Returns a [`Task`] that will cancel the future when dropped.
    ///
    /// Unlike [`OnDispatcher::compute`], this will accept a future that does not implement [`Send`]. If
    /// called from a thread other than the one the dispatcher is running on or the dispatcher
    /// is not guaranteed to always poll from the same thread, this will return
    /// [`Status::BAD_STATE`].
    ///
    /// TODO(470088116): This may be the cause of some flakes, so care should be used with it
    /// in critical paths for now.
    fn compute_local<T: Send + 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T>
    where
        Self: 'static,
    {
        let Some(dispatcher) = self.try_get_async_dispatcher() else {
            return Task::new_failed(Status::BAD_STATE);
        };
        let dispatcher =
            DriverDispatcherRef::from_async_dispatcher(dispatcher.as_async_dispatcher_ref());
        if dispatcher.0.is_current_dispatcher() && !dispatcher.0.allows_thread_migration() {
            OnDispatcher::compute(self, AddSendFuture(future))
        } else {
            Task::new_failed(Status::BAD_STATE)
        }
    }
}

impl<'a> AsAsyncDispatcherRef for DriverDispatcherRef<'a> {
    fn as_async_dispatcher_ref(&self) -> AsyncDispatcherRef<'_> {
        self.0.as_async_dispatcher_ref()
    }
}

impl<'a> Clone for DriverDispatcherRef<'a> {
    fn clone(&self) -> Self {
        Self(ManuallyDrop::new(Dispatcher(self.0.0)), PhantomData)
    }
}

impl<'a> core::ops::Deref for DriverDispatcherRef<'a> {
    type Target = Dispatcher;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> core::ops::DerefMut for DriverDispatcherRef<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Note: This may panic (or assert in C++) if its methods are run on a dispatcher that is not
/// a driver dispatcher.
impl<T> OnDriverDispatcher for T where T: AsAsyncDispatcherRef + Clone {}

/// A placeholder for the currently active dispatcher.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CurrentDispatcher;

impl GetAsyncDispatcher for CurrentDispatcher {
    fn try_get_async_dispatcher(&self) -> Option<AsyncDispatcher> {
        OVERRIDE_DISPATCHER
            .with(|global| *global.borrow())
            .or_else(|| {
                // SAFETY: NonNull::new will null-check that we have a current dispatcher.
                NonNull::new(unsafe { fdf_dispatcher_get_current_dispatcher() })
            })
            .map(|dispatcher| {
                // SAFETY: We constrain the lifetime of the `DispatcherRef` we provide to the
                // function below to the span of the current function. Since we are running on
                // the dispatcher, or another dispatcher that is bound to the same lifetime (through
                // override_dispatcher), we can be sure that the dispatcher will not be shut
                // down before that function completes.
                let async_dispatcher = NonNull::new(unsafe {
                    fdf_dispatcher_get_async_dispatcher(dispatcher.as_ptr())
                })
                .expect("No async dispatcher on driver dispatcher");
                AsyncDispatcher::new(&unsafe { AsyncDispatcherRef::from_raw(async_dispatcher) })
            })
    }
}

impl OnDriverDispatcher for CurrentDispatcher {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Once, mpsc};

    use futures::channel::mpsc as async_mpsc;
    use futures::{SinkExt, StreamExt};
    use zx::sys::ZX_OK;

    use core::ffi::{c_char, c_void};
    use core::ptr::null_mut;

    static GLOBAL_DRIVER_ENV: Once = Once::new();
    const NO_SYNC_CALLS_ROLE: &str = "no sync calls role";

    pub fn ensure_driver_env() {
        GLOBAL_DRIVER_ENV.call_once(|| {
            // SAFETY: calling fdf_env_start, which does not have any soundness
            // concerns for rust code, and this is only used in tests.
            unsafe {
                assert_eq!(fdf_env_start(0), ZX_OK);
                assert_eq!(
                    fdf_env_set_scheduler_role_opts(
                        NO_SYNC_CALLS_ROLE.as_ptr() as *const c_char,
                        NO_SYNC_CALLS_ROLE.len(),
                        FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS
                    ),
                    ZX_OK
                );
            }
        });
    }
    pub fn with_raw_dispatcher<T>(name: &str, p: impl FnOnce(AsyncDispatcher) -> T) -> T {
        with_raw_dispatcher_flags(name, DispatcherBuilder::ALLOW_THREAD_BLOCKING, "", p)
    }

    pub(crate) fn with_raw_dispatcher_flags<T>(
        name: &str,
        flags: u32,
        scheduler_role: &str,
        p: impl FnOnce(AsyncDispatcher) -> T,
    ) -> T {
        ensure_driver_env();

        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let mut dispatcher = null_mut();
        let mut observer = ShutdownObserver::new(move |dispatcher| {
            // SAFETY: we verify that the dispatcher has no tasks left queued in it,
            // just because this is testing code.
            assert!(!unsafe { fdf_env_dispatcher_has_queued_tasks(dispatcher.0.0.as_ptr()) });
            shutdown_tx.send(()).unwrap();
        })
        .into_ptr();
        let driver_ptr = &mut observer as *mut _ as *mut c_void;
        // SAFETY: The pointers we pass to this function are all stable for the
        // duration of this function, and are not available to copy or clone to
        // client code (only through a ref to the non-`Clone`` `Dispatcher`
        // wrapper).
        let res = unsafe {
            fdf_env_dispatcher_create_with_owner(
                driver_ptr,
                flags,
                name.as_ptr() as *const c_char,
                name.len(),
                scheduler_role.as_ptr() as *const c_char,
                scheduler_role.len(),
                observer,
                &mut dispatcher,
            )
        };
        assert_eq!(res, ZX_OK);
        let dispatcher = Dispatcher(NonNull::new(dispatcher).unwrap());

        let res = p(AsyncDispatcher::new(&dispatcher));

        drop(dispatcher);
        shutdown_rx.recv().unwrap();

        res
    }

    #[test]
    fn start_test_dispatcher() {
        with_raw_dispatcher("testing", |dispatcher| {
            println!("hello {dispatcher:?}");
        })
    }

    #[test]
    fn post_task_on_dispatcher() {
        with_raw_dispatcher("testing task", |dispatcher| {
            let (tx, rx) = mpsc::channel();
            dispatcher
                .post_task_sync(move |status| {
                    assert_eq!(status, Ok(()));
                    tx.send(status).unwrap();
                })
                .unwrap();
            assert_eq!(rx.recv().unwrap(), Ok(()));
        });
    }

    #[test]
    fn post_task_on_subdispatcher() {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        with_raw_dispatcher("testing task top level", move |dispatcher| {
            let (tx, rx) = mpsc::channel();
            let (inner_tx, inner_rx) = mpsc::channel();
            dispatcher
                .post_task_sync(move |status| {
                    assert_eq!(status, Ok(()));
                    let inner = DispatcherBuilder::new()
                        .name("testing task second level")
                        .scheduler_role("")
                        .allow_thread_blocking()
                        .shutdown_observer(move |_dispatcher| {
                            println!("shutdown observer called");
                            shutdown_tx.send(1).unwrap();
                        })
                        .create()
                        .unwrap();
                    inner
                        .post_task_sync(move |status| {
                            assert_eq!(status, Ok(()));
                            tx.send(status).unwrap();
                        })
                        .unwrap();
                    // we want to make sure the inner dispatcher lives long
                    // enough to run the task, so we sent it out to the outer
                    // closure.
                    inner_tx.send(inner).unwrap();
                })
                .unwrap();
            assert_eq!(rx.recv().unwrap(), Ok(()));
            inner_rx.recv().unwrap();
        });
        assert_eq!(shutdown_rx.recv().unwrap(), 1);
    }

    #[test]
    fn spawn_local_fails_on_normal_dispatcher() {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        with_raw_dispatcher("spawn local failures", move |dispatcher| {
            let inside_dispatcher = dispatcher.clone();
            dispatcher.spawn(async move {
                assert_eq!(
                    inside_dispatcher.spawn_local(futures::future::ready(())).await.unwrap_err(),
                    Status::BAD_STATE
                );
                assert_eq!(
                    inside_dispatcher.compute_local(futures::future::ready(())).await.unwrap_err(),
                    Status::BAD_STATE
                );
                shutdown_tx.send(()).unwrap();
            });
            shutdown_rx.recv().unwrap();
        });
    }

    #[test]
    #[ignore = "Pending resolution of b/488397193"]
    fn spawn_local_succeeds_on_no_thread_migration_dispatcher() {
        let (tx, rx) = mpsc::channel();
        with_raw_dispatcher_flags(
            "spawn local success",
            FDF_DISPATCHER_OPTION_NO_THREAD_MIGRATION,
            NO_SYNC_CALLS_ROLE,
            move |dispatcher| {
                let inside_dispatcher = dispatcher.clone();
                dispatcher.spawn(async move {
                    let tx_clone = tx.clone();
                    inside_dispatcher.spawn_local(async move {
                        tx_clone.send(()).unwrap();
                    });
                    inside_dispatcher
                        .compute_local(async move {
                            tx.send(()).unwrap();
                        })
                        .await
                        .unwrap();
                });
                // one empty object received each for spawn and compute _local.
                rx.recv().unwrap();
                rx.recv().unwrap();
            },
        );
    }

    #[test]
    #[ignore = "Pending resolution of b/488397193"]
    fn spawn_local_fails_on_no_thread_migration_dispatcher_from_different_thread() {
        with_raw_dispatcher_flags(
            "spawn local success",
            FDF_DISPATCHER_OPTION_NO_THREAD_MIGRATION,
            NO_SYNC_CALLS_ROLE,
            move |dispatcher| {
                let mut executor = fuchsia_async::LocalExecutor::default();
                executor.run_singlethreaded(async {
                    // we are not currently running in any driver dispatcher here, so this is a
                    // context where the 'current dispatcher' is definitely not the one in question.
                    assert_eq!(
                        dispatcher.spawn_local(futures::future::ready(())).await.unwrap_err(),
                        Status::BAD_STATE
                    );
                    assert_eq!(
                        dispatcher.compute_local(futures::future::ready(())).await.unwrap_err(),
                        Status::BAD_STATE
                    );
                });
            },
        );
    }

    async fn ping(mut tx: async_mpsc::Sender<u8>, mut rx: async_mpsc::Receiver<u8>) {
        println!("starting ping!");
        tx.send(0).await.unwrap();
        while let Some(next) = rx.next().await {
            println!("ping! {next}");
            tx.send(next + 1).await.unwrap();
        }
    }

    async fn pong(
        fin_tx: std::sync::mpsc::Sender<()>,
        mut tx: async_mpsc::Sender<u8>,
        mut rx: async_mpsc::Receiver<u8>,
    ) {
        println!("starting pong!");
        while let Some(next) = rx.next().await {
            println!("pong! {next}");
            if next > 10 {
                println!("bye!");
                break;
            }
            tx.send(next + 1).await.unwrap();
        }
        fin_tx.send(()).unwrap();
    }

    #[test]
    fn async_ping_pong() {
        with_raw_dispatcher("async ping pong", |dispatcher| {
            let (fin_tx, fin_rx) = mpsc::channel();
            let (ping_tx, pong_rx) = async_mpsc::channel(10);
            let (pong_tx, ping_rx) = async_mpsc::channel(10);
            dispatcher.spawn(ping(ping_tx, ping_rx));
            dispatcher.spawn(pong(fin_tx, pong_tx, pong_rx));

            fin_rx.recv().expect("to receive final value");
        });
    }

    async fn slow_pong(
        fin_tx: std::sync::mpsc::Sender<()>,
        mut tx: async_mpsc::Sender<u8>,
        mut rx: async_mpsc::Receiver<u8>,
    ) {
        use zx::MonotonicDuration;
        println!("starting pong!");
        while let Some(next) = rx.next().await {
            println!("pong! {next}");
            fuchsia_async::Timer::new(fuchsia_async::MonotonicInstant::after(
                MonotonicDuration::from_seconds(1),
            ))
            .await;
            if next > 10 {
                println!("bye!");
                break;
            }
            tx.send(next + 1).await.unwrap();
        }
        fin_tx.send(()).unwrap();
    }

    #[test]
    fn mixed_executor_async_ping_pong() {
        with_raw_dispatcher("async ping pong", |dispatcher| {
            let (fin_tx, fin_rx) = mpsc::channel();
            let (ping_tx, pong_rx) = async_mpsc::channel(10);
            let (pong_tx, ping_rx) = async_mpsc::channel(10);

            // spawn ping on the driver dispatcher
            dispatcher.spawn(ping(ping_tx, ping_rx));

            // and run pong on the fuchsia_async executor
            let mut executor = fuchsia_async::LocalExecutor::default();
            executor.run_singlethreaded(slow_pong(fin_tx, pong_tx, pong_rx));

            fin_rx.recv().expect("to receive final value");
        });
    }
}
