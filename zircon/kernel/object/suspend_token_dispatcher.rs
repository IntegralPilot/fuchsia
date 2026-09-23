// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use fbl::Canary;
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{ZX_OBJ_TYPE_SUSPEND_TOKEN, ZX_RIGHT_INSPECT, ZX_RIGHT_TRANSFER, zx_rights_t};

use super::KernelHandle;
use super::dispatcher::Dispatcher;
use super::process_dispatcher::ProcessDispatcher;
use super::suspend_token_dispatcher_ffi::cpp_suspend_token_dispatcher_create;
use super::thread_dispatcher::ThreadDispatcher;

use object_constants_rs as object_constants;

/// Default rights assigned to a newly created SuspendTokenDispatcher handle.
pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER | ZX_RIGHT_INSPECT;

zr::static_assert_size_and_align!(
    SuspendTokenDispatcherState,
    object_constants::kSuspendTokenDispatcherStateSize,
    object_constants::kSuspendTokenDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_SUSPEND_TOKEN_CREATE_COUNT, "dispatcher.suspend_token.create", Sum);
define_kcounter!(DISPATCHER_SUSPEND_TOKEN_DESTROY_COUNT, "dispatcher.suspend_token.destroy", Sum);

/// Suspends a process or thread.
///
/// # Errors
///
/// - `ZX_ERR_NOT_SUPPORTED` if `task` is trying to suspend itself or its current process.
/// - `ZX_ERR_WRONG_TYPE` if `task` is not a thread or process dispatcher.
/// - `ZX_ERR_BAD_STATE` if `task` is dying or dead.
fn suspend_task(task: &Dispatcher) -> Result<(), Status> {
    if let Some(thread) = task.downcast::<ThreadDispatcher>() {
        if thread.is_current() {
            return Err(Status::NOT_SUPPORTED);
        }
        return thread.suspend();
    }

    if let Some(process) = task.downcast::<ProcessDispatcher>() {
        if process.is_current() {
            return Err(Status::NOT_SUPPORTED);
        }
        return process.suspend();
    }

    Err(Status::WRONG_TYPE)
}

/// Resumes a process or thread.
///
/// # Panics
///
/// Panics if `task` is neither a `ThreadDispatcher` nor a `ProcessDispatcher`.
fn resume_task(task: &Dispatcher) {
    if let Some(thread) = task.downcast::<ThreadDispatcher>() {
        thread.resume();
        return;
    }

    if let Some(process) = task.downcast::<ProcessDispatcher>() {
        process.resume();
        return;
    }

    unreachable!();
}

/// Internal state storage for `SuspendTokenDispatcher`.
#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct SuspendTokenDispatcherState {
    /// Magic canary value validating structural integrity.
    canary: Canary<{ fbl::magic(b"SUTD") }>,

    /// The suspended task (thread or process).
    #[guarded_by(lock)]
    task: Option<fbl::RefPtr<Dispatcher>>,

    /// Mutex guarding state operations.
    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl SuspendTokenDispatcherState {
    /// Initializes the `SuspendTokenDispatcherState`.
    pub fn init(
        _dispatcher: *const SuspendTokenDispatcher,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_SUSPEND_TOKEN_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            task: None.into(),
            lock <- KMutex::init(),
        })
    }

    /// Stores the suspended task into state.
    pub fn set_task(&self, task: fbl::RefPtr<Dispatcher>) {
        ksync::lock!(let mut guard = self.lock_lock());
        *guard.as_mut().task_mut() = Some(task);
    }

    /// Takes and returns the task stored in state.
    pub fn take_task(&self) -> Option<fbl::RefPtr<Dispatcher>> {
        ksync::lock!(let mut guard = self.lock_lock());
        guard.as_mut().task_mut().take()
    }
}

#[pinned_drop]
impl PinnedDrop for SuspendTokenDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_SUSPEND_TOKEN_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    /// A SuspendTokenDispatcher suspends a process or thread when created and resumes it when
    /// destroyed.
    pub struct SuspendTokenDispatcher,
    SuspendTokenDispatcherState,
    ZX_OBJ_TYPE_SUSPEND_TOKEN,
    object_constants::kSuspendTokenDispatcherStateOffset
);

impl SuspendTokenDispatcher {
    /// Returns the default rights for a SuspendTokenDispatcher handle.
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new `SuspendTokenDispatcher` suspending `task`.
    ///
    /// # Errors
    ///
    /// - `ZX_ERR_NO_MEMORY` if allocation of the dispatcher failed.
    /// - `ZX_ERR_WRONG_TYPE` if `task` is not a thread or process dispatcher.
    /// - `ZX_ERR_BAD_STATE` if `task` is dying or dead.
    /// - `ZX_ERR_NOT_SUPPORTED` if `task` is attempting to suspend itself.
    pub fn create(
        task: fbl::RefPtr<Dispatcher>,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let rights = Self::default_rights();
        // SAFETY: `cpp_suspend_token_dispatcher_create` initializes `handle` on success.
        let handle =
            unsafe { KernelHandle::create(|out| cpp_suspend_token_dispatcher_create(out)) }?;

        // Suspend the task after creating the dispatcher handle. If suspension fails,
        // `handle` is dropped without setting `task`, so `on_zero_handles()` does nothing.
        suspend_task(&task)?;

        handle.dispatcher().state().set_task(task);

        Ok((handle, rights))
    }

    /// Callback invoked when all handles to this dispatcher are closed.
    pub fn on_zero_handles(&self) {
        if let Some(task) = self.state().take_task() {
            resume_task(&task);
        }
    }
}
