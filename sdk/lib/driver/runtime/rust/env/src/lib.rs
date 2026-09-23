// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for driver runtime environment.

#![deny(missing_docs)]

use fdf_sys::*;
use zx::sys::zx_duration_mono_t;

use core::ffi;
use core::marker::PhantomData;
use core::ptr::{NonNull, null_mut};

use zx::Status;

use fdf_core::dispatcher::{Dispatcher, DispatcherBuilder, DriverDispatcherRef};
use fdf_core::shutdown_observer::ShutdownObserver;

pub mod test;

/// Create the dispatcher as configured by this object. This must be called from a
/// thread managed by the driver runtime. The dispatcher returned is owned by the caller,
/// and will initiate asynchronous shutdown when the object is dropped unless
/// [`Dispatcher::release`] is called on it to convert it into an unowned [`DispatcherRef`].
fn create_with_driver<'a>(
    dispatcher: DispatcherBuilder,
    driver: DriverRefTypeErased<'a>,
) -> Result<Dispatcher, Status> {
    let mut out_dispatcher = null_mut();
    let owner = driver.0;
    let options = dispatcher.options;
    let name = dispatcher.name.as_ptr() as *mut ffi::c_char;
    let name_len = dispatcher.name.len();
    let scheduler_role = dispatcher.scheduler_role.as_ptr() as *mut ffi::c_char;
    let scheduler_role_len = dispatcher.scheduler_role.len();
    let observer =
        ShutdownObserver::new(dispatcher.shutdown_observer.unwrap_or_else(|| Box::new(|_| {})))
            .into_ptr();
    // SAFETY: all arguments point to memory that will be available for the duration
    // of the call, except `observer`, which will be available until it is unallocated
    // by the dispatcher exit handler.
    Status::ok(unsafe {
        fdf_env_dispatcher_create_with_owner(
            owner,
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
    Ok(unsafe { Dispatcher::from_raw(NonNull::new_unchecked(out_dispatcher)) })
}

/// A marker trait for a function that can be used as a driver shutdown observer with
/// [`Driver::shutdown`].
pub trait DriverShutdownObserverFn<T: 'static>:
    FnOnce(DriverRef<'static, T>) + Send + Sync + 'static
{
}
impl<T, U: 'static> DriverShutdownObserverFn<U> for T where
    T: FnOnce(DriverRef<'static, U>) + Send + Sync + 'static
{
}

/// A shutdown observer for [`fdf_dispatcher_create`] that can call any kind of callback instead of
/// just a C-compatible function when a dispatcher is shutdown.
///
/// # Safety
///
/// This object relies on a specific layout to allow it to be cast between a
/// `*mut fdf_dispatcher_shutdown_observer` and a `*mut ShutdownObserver`. To that end,
/// it is important that this struct stay both `#[repr(C)]` and that `observer` be its first member.
#[repr(C)]
struct DriverShutdownObserver<T: 'static> {
    observer: fdf_env_driver_shutdown_observer,
    shutdown_fn: Box<dyn DriverShutdownObserverFn<T>>,
    driver: Driver<T>,
}

impl<T: 'static> DriverShutdownObserver<T> {
    /// Creates a new [`ShutdownObserver`] with `f` as the callback to run when a dispatcher
    /// finishes shutting down.
    fn new<F: DriverShutdownObserverFn<T>>(driver: Driver<T>, f: F) -> Self {
        let shutdown_fn = Box::new(f);
        Self {
            observer: fdf_env_driver_shutdown_observer { handler: Some(Self::handler) },
            shutdown_fn,
            driver,
        }
    }

    /// Begins the driver shutdown procedure.
    /// Turns this object into a stable pointer suitable for passing to
    /// [`fdf_env_shutdown_dispatchers_async`] by wrapping it in a [`Box`] and leaking it
    /// to be reconstituded by [`Self::handler`] when the dispatcher is shut down.
    fn begin(self) -> Result<(), Status> {
        let driver = self.driver.inner.as_ptr() as *const _;
        // Note: this relies on the assumption that `self.observer` is at the beginning of the
        // struct.
        let this = Box::into_raw(Box::new(self)) as *mut _;
        // SAFETY: driver is owned by the driver framework and will be kept alive until the handler
        // callback is triggered
        if let Err(e) = Status::ok(unsafe { fdf_env_shutdown_dispatchers_async(driver, this) }) {
            // SAFETY: The framework didn't actually take ownership of the object if the call
            // fails, so we can recover it to avoid leaking.
            let _ = unsafe { Box::from_raw(this as *mut DriverShutdownObserver<T>) };
            return Err(e);
        }
        Ok(())
    }

    /// The callback that is registered with the driver that will be called when the driver
    /// is shut down.
    ///
    /// # Safety
    ///
    /// This function should only ever be called by the driver runtime at dispatcher shutdown
    /// time, must only ever be called once for any given [`ShutdownObserver`] object, and
    /// that [`ShutdownObserver`] object must have previously been made into a pointer by
    /// [`Self::into_ptr`].
    unsafe extern "C" fn handler(
        driver: *const ffi::c_void,
        observer: *mut fdf_env_driver_shutdown_observer_t,
    ) {
        // SAFETY: The driver framework promises to only call this function once, so we can
        // safely take ownership of the [`Box`] and deallocate it when this function ends.
        let observer = unsafe { Box::from_raw(observer as *mut DriverShutdownObserver<T>) };
        (observer.shutdown_fn)(DriverRef(driver as *const T, PhantomData));
    }
}

/// An owned handle to a Driver instance that can be used to create initial dispatchers.
#[derive(Debug)]
pub struct Driver<T> {
    pub(crate) inner: NonNull<T>,
    shutdown_triggered: bool,
}

/// An unowned handle to the driver that is returned through certain environment APIs like
/// |get_driver_on_thread_koid|.
pub struct UnownedDriver {
    inner: *const ffi::c_void,
}

/// SAFETY: This inner pointer is movable across threads.
unsafe impl<T: Send> Send for Driver<T> {}

impl<T: 'static> Driver<T> {
    /// Constructs a dispatcher from the given builder on this driver. Note that this dispatcher
    /// cannot outlive the driver and is only capable of being stopped by shutting down the driver.
    /// It is meant to be created to serve as the initial or default dispatcher for a driver.
    ///
    /// The caller should make sure that the dispatcher is released so the driver runtime will
    /// manage shutting it down, but that may be done differently in test contexts so it does not
    /// force it.
    pub fn new_dispatcher(&self, dispatcher: DispatcherBuilder) -> Result<Dispatcher, Status> {
        create_with_driver(dispatcher, self.as_ref_type_erased())
    }

    /// Run a closure in the context of a driver.
    pub fn enter<R>(&mut self, f: impl FnOnce() -> R) -> R {
        unsafe { fdf_env_register_driver_entry(self.inner.as_ptr() as *const _) };
        let res = f();
        unsafe { fdf_env_register_driver_exit() };
        res
    }

    /// Adds an allowed scheduler role to the driver
    pub fn add_allowed_scheduler_role(&self, scheduler_role: &str) {
        let driver_ptr = self.inner.as_ptr() as *const _;
        let scheduler_role_ptr = scheduler_role.as_ptr() as *mut ffi::c_char;
        let scheduler_role_len = scheduler_role.len();
        unsafe {
            fdf_env_add_allowed_scheduler_role_for_driver(
                driver_ptr,
                scheduler_role_ptr,
                scheduler_role_len,
            )
        };
    }

    /// Registers a callback which is triggered whenever the runtime needs to be resumed.
    /// Returns a registration handle that unregisters and frees the requester when dropped.
    pub fn register_resume_requester(
        &self,
        requester: ResumeRequester,
    ) -> ResumeRequesterRegistration {
        let driver_ptr = self.inner.as_ptr() as *const _;
        let requester_ptr = requester.into_ptr();

        // SAFETY: requester_ptr is used by the driver runtime as a callback function.
        // The driver runtime does not manage this object's lifetime. driver_ptr is not modified
        // by the runtime.
        unsafe {
            fdf_sys::fdf_env_register_resume_requester(driver_ptr, requester_ptr);
        }
        ResumeRequesterRegistration { driver_ptr, requester_ptr }
    }

    /// Asynchronously suspends the dispatchers owned by the driver.
    pub fn driver_suspend(&self, completer: SuspendCompleter) {
        unsafe {
            fdf_sys::fdf_env_driver_suspend(self.inner.as_ptr() as *const _, completer.into_ptr());
        }
    }

    /// Resumes the dispatchers owned by the driver.
    pub fn driver_resume(&self) {
        unsafe {
            fdf_sys::fdf_env_driver_resume(self.inner.as_ptr() as *const _);
        }
    }

    /// Asynchronously shuts down all dispatchers owned by |driver|.
    /// |f| will be called once shutdown completes. This is guaranteed to be
    /// after all the dispatcher's shutdown observers have been called, and will be running
    /// on the thread of the final dispatcher which has been shutdown.
    pub fn shutdown<F: DriverShutdownObserverFn<T>>(mut self, f: F) {
        self.shutdown_triggered = true;
        // It should be impossible for this to fail as we ensure we are the only caller of this
        // API, so it cannot be triggered twice nor before the driver has been registered with the
        // framework.
        DriverShutdownObserver::new(self, f)
            .begin()
            .expect("Unexpectedly failed start shutdown procedure")
    }

    /// Create a reference to a driver without ownership. The returned reference lacks the ability
    /// to perform most actions available to the owner of the driver, therefore it doesn't need to
    /// have it's lifetime tracked closely.
    fn as_ref_type_erased<'a>(&'a self) -> DriverRefTypeErased<'a> {
        DriverRefTypeErased(self.inner.as_ptr() as *const _, PhantomData)
    }

    /// Releases ownership of this driver instance, allowing it to be shut down when the runtime
    /// shuts down.
    pub fn release(self) -> DriverRef<'static, T> {
        DriverRef(self.inner.as_ptr() as *const _, PhantomData)
    }
}

impl<T> Drop for Driver<T> {
    fn drop(&mut self) {
        assert!(self.shutdown_triggered, "Cannot drop driver, must call shutdown method instead");
    }
}

impl<T> PartialEq<UnownedDriver> for Driver<T> {
    fn eq(&self, other: &UnownedDriver) -> bool {
        self.inner.as_ptr() as *const _ == other.inner
    }
}

// Note that inner type is not guaranteed to not be null.
#[derive(Clone, Copy, PartialEq)]
struct DriverRefTypeErased<'a>(*const ffi::c_void, PhantomData<&'a u32>);

impl Default for DriverRefTypeErased<'_> {
    fn default() -> Self {
        DriverRefTypeErased(std::ptr::null(), PhantomData)
    }
}

/// A lifetime-bound reference to a driver handle.
pub struct DriverRef<'a, T>(pub *const T, PhantomData<&'a Driver<T>>);

/// A marker trait for a function type that can be used as a stall scanner.
pub trait StallScannerFn: Fn(zx_duration_mono_t) + Send + Sync + 'static {}
impl<T> StallScannerFn for T where T: Fn(zx_duration_mono_t) + Send + Sync + 'static {}

/// A stall scanner for [`fdf_env_register_stall_scanner`] that can call any kind of callback instead of
/// just a C-compatible function when a dispatcher is shutdown.
///
/// # Safety
///
/// This object relies on a specific layout to allow it to be cast between a
/// `*mut fdf_env_stall_scanner` and a `*mut StallScanner`. To that end,
/// it is important that this struct stay both `#[repr(C)]` and that `scanner` be its first member.
#[repr(C)]
#[doc(hidden)]
pub struct StallScanner {
    scanner: fdf_env_stall_scanner,
    begin_fn: Box<dyn StallScannerFn>,
}

impl StallScanner {
    /// Creates a new [`StallScanner`] with `f` as the callback to run when a dispatcher
    /// finishes shutting down.
    pub fn new<F: StallScannerFn>(f: F) -> Self {
        let begin_fn = Box::new(f);
        Self { scanner: fdf_env_stall_scanner { handler: Some(Self::handler) }, begin_fn }
    }

    /// Turns this object into a stable pointer suitable for passing to
    /// [`fdf_env_register_stall_scanner`] by wrapping it in a [`Box`] and leaking it to be reconstituded
    /// by [`Self::handler`] when the scanner is triggered.
    pub fn into_ptr(self) -> *mut fdf_env_stall_scanner {
        // Note: this relies on the assumption that `self.scanner` is at the beginning of the
        // struct.
        Box::leak(Box::new(self)) as *mut _ as *mut _
    }

    /// The callback that is registered with the dispatcher that will be called when the stall
    /// scanner should begin a scan.
    ///
    /// # Safety
    ///
    /// The [`StallScanner`] object must have previously been made into a pointer by
    /// [`Self::into_ptr`].
    unsafe extern "C" fn handler(
        scanner: *mut fdf_env_stall_scanner,
        duration: zx_duration_mono_t,
    ) {
        let scanner = scanner as *mut StallScanner;

        unsafe {
            ((*scanner).begin_fn)(duration);
        }
    }
}

/// A marker trait for a function type that can be used as a resume requester.
pub trait ResumeRequesterFn: Fn() -> Result<(), Status> + Send + Sync + 'static {}
impl<T> ResumeRequesterFn for T where T: Fn() -> Result<(), Status> + Send + Sync + 'static {}

/// A resume requester for [`fdf_env_register_resume_requester`] that can call any kind of callback.
///
/// # Safety
///
/// This object relies on a specific layout to allow it to be cast between a
/// `*mut fdf_env_resume_requester_t` and a `*mut ResumeRequester`. To that end,
/// it is important that this struct stay both `#[repr(C)]` and that `requester` be its first member.
#[repr(C)]
pub struct ResumeRequester {
    /// The underlying C structure.
    pub requester: fdf_env_resume_requester_t,
    resume_fn: Box<dyn ResumeRequesterFn>,
}

impl ResumeRequester {
    /// Creates a new [`ResumeRequester`] with `f` as the callback to run when the runtime needs to be resumed.
    pub fn new<F: ResumeRequesterFn>(f: F) -> Self {
        let resume_fn = Box::new(f);
        Self { requester: fdf_env_resume_requester_t { handler: Some(Self::handler) }, resume_fn }
    }

    /// Turns this object into a stable pointer suitable for passing to
    /// [`fdf_env_register_resume_requester`] by wrapping it in a [`Box`] and leaking it to be reconstituded
    /// by [`Self::handler`] when the runtime needs to be resumed.
    pub fn into_ptr(self) -> *mut fdf_env_resume_requester_t {
        Box::leak(Box::new(self)) as *mut _ as *mut _
    }

    /// The callback that is registered with the dispatcher that will be called when the runtime
    /// needs to be resumed.
    ///
    /// # Safety
    ///
    /// The [`ResumeRequester`] object must have previously been made into a pointer by
    /// [`Self::into_ptr`].
    unsafe extern "C" fn handler(requester: *mut fdf_env_resume_requester_t) -> i32 {
        let requester = requester as *mut ResumeRequester;
        unsafe {
            match ((*requester).resume_fn)() {
                Ok(()) => 0,
                Err(e) => e.into_raw(),
            }
        }
    }
}

/// A marker trait for a function type that can be used as a suspend completer.
pub trait SuspendCompleterFn: FnOnce() + Send + Sync + 'static {}
impl<T> SuspendCompleterFn for T where T: FnOnce() + Send + Sync + 'static {}

/// A suspend completer for [`fdf_env_driver_suspend`] that can call any kind of callback.
///
/// # Safety
///
/// This object relies on a specific layout to allow it to be cast between a
/// `*mut fdf_env_suspend_completer_t` and a `*mut SuspendCompleter`. To that end,
/// it is important that this struct stay both `#[repr(C)]` and that `completer` be its first member.
#[repr(C)]
pub struct SuspendCompleter {
    completer: fdf_env_suspend_completer_t,
    complete_fn: Box<dyn SuspendCompleterFn>,
}

impl SuspendCompleter {
    /// Creates a new [`SuspendCompleter`] with `f` as the callback to run when the runtime finishes suspending.
    pub fn new<F: SuspendCompleterFn>(f: F) -> Self {
        let complete_fn = Box::new(f);
        Self {
            completer: fdf_env_suspend_completer_t { handler: Some(Self::handler) },
            complete_fn,
        }
    }

    /// Turns this object into a stable pointer suitable for passing to
    /// [`fdf_env_driver_suspend`] by wrapping it in a [`Box`] and leaking it to be reconstituded
    /// by [`Self::handler`] when the runtime finishes suspending.
    pub fn into_ptr(self) -> *mut fdf_env_suspend_completer_t {
        Box::leak(Box::new(self)) as *mut _ as *mut _
    }

    /// The callback that is registered with the dispatcher that will be called when the runtime
    /// finishes suspending.
    ///
    /// # Safety
    ///
    /// The [`SuspendCompleter`] object must have previously been made into a pointer by
    /// [`Self::into_ptr`].
    unsafe extern "C" fn handler(completer: *mut fdf_env_suspend_completer_t) {
        let completer = completer as *mut SuspendCompleter;
        unsafe {
            let completer = Box::from_raw(completer);
            (completer.complete_fn)();
        }
    }
}

/// The driver runtime environment
pub struct Environment;

impl Environment {
    /// Whether the environment should enforce scheduler roles. Used with [`Self::start`].
    pub const ENFORCE_ALLOWED_SCHEDULER_ROLES: u32 = 1;
    /// Whether the environment should dynamically spawn threads on-demand for sync call dispatchers.
    /// Used with [`Self::start`].
    pub const DYNAMIC_THREAD_SPAWNING: u32 = 2;

    /// Start the driver runtime. This sets up the initial thread that the dispatchers run on.
    pub fn start(options: u32) -> Result<Environment, Status> {
        // SAFETY: calling fdf_env_start, which does not have any soundness
        // concerns for rust code. It may be called multiple times without any problems.
        Status::ok(unsafe { fdf_env_start(options) })?;
        Ok(Self)
    }

    /// Creates a new driver. It is expected that the driver passed in is a leaked pointer which
    /// will only be recovered by triggering the shutdown method on the driver.
    ///
    /// # Panics
    ///
    /// This method will panic if |driver| is not null.
    pub fn new_driver<T>(&self, driver: *const T) -> Driver<T> {
        // We cast to *mut because there is not equivlaent version of NonNull for *const T.
        Driver {
            inner: NonNull::new(driver as *mut _).expect("driver must not be null"),
            shutdown_triggered: false,
        }
    }

    // TODO: Consider tracking all drivers and providing a method to shutdown all outstanding
    // drivers and block until they've all finished shutting down.

    /// Returns whether the current thread is managed by the driver runtime or not.
    fn current_thread_managed_by_driver_runtime() -> bool {
        // Safety: Calling fdf_dispatcher_get_current_dispatcher from any thread is safe. Because
        // we are not actually using the dispatcher, we don't need to worry about it's lifetime.
        !unsafe { fdf_dispatcher_get_current_dispatcher().is_null() }
    }

    /// Resets the driver runtime to zero threads. This may only be called when there are no
    /// existing dispatchers.
    ///
    /// # Panics
    ///
    /// This method should not be called from a thread managed by the driver runtime,
    /// such as from tasks or ChannelRead callbacks.
    pub fn reset(&self) {
        assert!(
            !Self::current_thread_managed_by_driver_runtime(),
            "reset must be called from a thread not managed by the driver runtime"
        );
        // SAFETY: calling fdf_env_reset, which does not have any soundness
        // concerns for rust code. It may be called multiple times without any problems.
        unsafe { fdf_env_reset() };
    }

    /// Destroys all dispatchers in the process and blocks the current thread
    /// until each runtime dispatcher in the process is observed to have been destroyed.
    ///
    /// This should only be used called after all drivers have been shutdown.
    ///
    /// # Panics
    ///
    /// This method should not be called from a thread managed by the driver runtime,
    /// such as from tasks or ChannelRead callbacks.
    pub fn destroy_all_dispatchers(&self) {
        assert!(
            !Self::current_thread_managed_by_driver_runtime(),
            "destroy_all_dispatchers must be called from a thread not managed by the driver runtime"
        );
        unsafe { fdf_env_destroy_all_dispatchers() };
    }

    /// Returns whether the dispatcher has any queued tasks.
    pub fn dispatcher_has_queued_tasks(&self, dispatcher: DriverDispatcherRef<'_>) -> bool {
        unsafe {
            fdf_env_dispatcher_has_queued_tasks(fdf_core::dispatcher_ptr(&dispatcher).as_ptr())
        }
    }

    /// Returns the current maximum number of threads which will be spawned for thread pool associated
    /// with the given scheduler role.
    ///
    /// |scheduler_role| is the name of the role which is passed when creating dispatchers.
    pub fn get_thread_limit(&self, scheduler_role: &str) -> u32 {
        let scheduler_role_ptr = scheduler_role.as_ptr() as *mut ffi::c_char;
        let scheduler_role_len = scheduler_role.len();
        unsafe { fdf_env_get_thread_limit(scheduler_role_ptr, scheduler_role_len) }
    }

    /// Sets the number of threads which will be spawned for thread pool associated with the given
    /// scheduler role. It cannot shrink the limit less to a value lower than the current number of
    /// threads in the thread pool.
    ///
    /// |scheduler_role| is the name of the role which is passed when creating dispatchers.
    /// |max_threads| is the number of threads to use as new limit.
    pub fn set_thread_limit(&self, scheduler_role: &str, max_threads: u32) -> Result<(), Status> {
        let scheduler_role_ptr = scheduler_role.as_ptr() as *mut ffi::c_char;
        let scheduler_role_len = scheduler_role.len();
        Status::ok(unsafe {
            fdf_env_set_thread_limit(scheduler_role_ptr, scheduler_role_len, max_threads)
        })
    }
    /// Returns the currently set options for the scheduler role as a uint32_t bitmask.
    ///
    /// |scheduler_role| is the name of the role which is passed when creating dispatchers.
    pub fn get_scheduler_role_opts(&self, scheduler_role: &str) -> u32 {
        let scheduler_role_ptr = scheduler_role.as_ptr() as *mut ffi::c_char;
        let scheduler_role_len = scheduler_role.len();
        unsafe { fdf_env_get_scheduler_role_opts(scheduler_role_ptr, scheduler_role_len) }
    }

    /// When used with [`Self::set_scheduler_role_opts`], this will not allow any dispatchers on the
    /// scheduler role to be created with `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS`.
    pub const SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS: u32 = FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS;

    /// Sets the options for the given scheduler role. This can be used to enforce restrictions
    /// on the kinds of dispatchers that can be created on this scheduler role.
    ///
    /// |scheduler_role| is the name of the role which is passed when creating dispatchers.
    /// |options| is the new options for the scheduler role.
    ///
    /// # Errors
    ///
    /// [`Status::INVALID_ARGS`]: |options| contains unknown or invalid options.
    /// [`Status::ERR_NOT_SUPPORTED`]: |options| contains an option that wouldn't allow a dispatcher
    /// that already exists on this scheduler role.
    pub fn set_scheduler_role_opts(
        &self,
        scheduler_role: &str,
        options: u32,
    ) -> Result<(), Status> {
        let scheduler_role_ptr = scheduler_role.as_ptr() as *mut ffi::c_char;
        let scheduler_role_len = scheduler_role.len();
        Status::ok(unsafe {
            fdf_env_set_scheduler_role_opts(scheduler_role_ptr, scheduler_role_len, options)
        })
    }

    /// Gets the driver currently running on the thread identified by |thread_koid|, if the thread
    /// is running on this driver host with a driver.
    pub fn get_driver_on_thread_koid(&self, thread_koid: zx::Koid) -> Option<UnownedDriver> {
        let mut driver = std::ptr::null();
        unsafe {
            Status::ok(fdf_env_get_driver_on_tid(thread_koid.raw_koid(), &mut driver)).ok()?;
        }
        if driver.is_null() { None } else { Some(UnownedDriver { inner: driver }) }
    }

    /// Registers a callback which is triggered whenever the stall scanner should run.
    pub fn register_stall_scanner(&self, scanner: StallScanner) {
        unsafe {
            fdf_env_register_stall_scanner(scanner.into_ptr());
        }
    }
}

/// A registration handle returned by [`Driver::register_resume_requester`].
/// The user MUST call `unregister` to unregister the resume requester when it is no longer valid.
#[derive(Debug)]
pub struct ResumeRequesterRegistration {
    driver_ptr: *const ffi::c_void,
    requester_ptr: *mut fdf_env_resume_requester_t,
}

// SAFETY: The runtime API that we call in this object (fdf_env_register_resume_requester)
// is thread-safe and can be called from any thread. We are also the exclusive maintainer of
// requester_ptr's lifetime.
unsafe impl Send for ResumeRequesterRegistration {}

impl ResumeRequesterRegistration {
    /// Unregisters the resume requester from the runtime and frees the memory associated with it.
    pub fn unregister(mut self) {
        // Unregister the callback from the runtime.
        // SAFETY: The null pointer is handled correctly by the runtime. If driver_ptr is no longer valid
        // in the driver runtime, it will be treated as a no-op.
        unsafe {
            fdf_sys::fdf_env_register_resume_requester(self.driver_ptr, null_mut());
        }

        // Reconstitute the box and free it.
        // SAFETY: requester_ptr was created using Box::leak(Box::new(self)).
        // This is the only location that we re-create the Box, with exclusive ownership of self.
        let requester = unsafe { Box::from_raw(self.requester_ptr as *mut ResumeRequester) };
        drop(requester);

        self.requester_ptr = null_mut();
    }
}
