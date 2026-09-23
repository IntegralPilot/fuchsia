// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::dispatcher::DispatcherOps;
use super::handle::{HandleOwner, HandleValue, KernelHandle};
use super::job_dispatcher::JobDispatcher;
#[cfg(target_arch = "x86_64")]
use super::process_dispatcher_ffi::cpp_process_dispatcher_hw_trace_context_id;
use super::process_dispatcher_ffi::{
    cpp_process_add_initialized_thread, cpp_process_attach_aspace_to_thread,
    cpp_process_dispatcher_current, cpp_process_dispatcher_enforce_basic_policy,
    cpp_process_dispatcher_get_debug_addr, cpp_process_dispatcher_get_dyn_break_on_load,
    cpp_process_dispatcher_get_info, cpp_process_dispatcher_is_current,
    cpp_process_dispatcher_kill, cpp_process_dispatcher_make_and_add_handle,
    cpp_process_dispatcher_resume, cpp_process_dispatcher_set_critical_to_job,
    cpp_process_dispatcher_set_debug_addr, cpp_process_dispatcher_set_dyn_break_on_load,
    cpp_process_dispatcher_start, cpp_process_dispatcher_suspend,
    cpp_process_dispatcher_vdso_base_address, cpp_process_futex_grow_pool,
    cpp_process_futex_shrink_pool, cpp_process_get_job_koid, cpp_process_remove_thread,
};
use super::thread_dispatcher::ThreadDispatcher;
use super::vm_address_region_dispatcher::VmAddressRegionDispatcher;
use crate::arch_rs::UserEntryState;
use crate::kernel::thread::ThreadPtr;
use core::mem::MaybeUninit;
use pin_init::{PinInit, pin_data, pin_init};
use zx_status::Status;
use zx_types::{zx_info_process_t, zx_rights_t, zx_vaddr_t};

/// Lock class tag for the handle table's reader-writer lock.
pub struct HandleTableLockClass;

impl ksync::LockClass for HandleTableLockClass {
    const ID: *mut core::ffi::c_void = core::ptr::null_mut();
}

crate::object::dispatcher::impl_dispatcher_facade!(
    pub struct ProcessDispatcher,
    zx_types::ZX_OBJ_TYPE_PROCESS
);

impl ProcessDispatcher {
    /// Executes the given function with a reference to the current process.
    pub fn with_current<R>(f: impl FnOnce(&ProcessDispatcher) -> R) -> R {
        // SAFETY: The current process is guaranteed to be valid for the duration of the call.
        let proc = unsafe { &*cpp_process_dispatcher_current() };
        f(proc)
    }

    /// Returns whether this `ProcessDispatcher` is the current process.
    pub fn is_current(&self) -> bool {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_is_current(self.as_ffi()) }
    }

    /// Starts execution of this process.
    pub fn start(
        &self,
        thread: fbl::RefPtr<ThreadDispatcher>,
        pc: zx_vaddr_t,
        sp: zx_vaddr_t,
        arg_handle: Option<HandleOwner>,
        arg2: usize,
    ) -> Result<(), Status> {
        let arg_handle_ptr = match arg_handle {
            Some(h) => h.release(),
            None => core::ptr::null_mut(),
        };
        let raw_thread = fbl::RefPtr::into_raw(thread) as *mut _;
        // SAFETY: `self` is a valid reference, `raw_thread` transfers an acquired refcount,
        // and `arg_handle_ptr` ownership is transferred to C++.
        let status = unsafe {
            cpp_process_dispatcher_start(
                self.as_ffi_mut(),
                raw_thread,
                pc,
                sp,
                arg_handle_ptr,
                arg2,
            )
        };
        Status::ok(status)
    }

    /// Removes a handle from this process's handle table and returns the owned handle if found.
    pub fn remove_handle(&self, handle: HandleValue) -> Option<HandleOwner> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let raw = unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_remove_handle(
                self.as_ffi_mut(),
                handle.raw_value(),
            )
        };
        // SAFETY: `raw` was exported by C++ `RemoveHandle.release()` or is null.
        unsafe { HandleOwner::from_raw(raw) }
    }

    /// Kills this process with the given return code.
    pub fn kill(&self, retcode: i64) {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_kill(self.as_ffi_mut(), retcode) }
    }

    /// Suspends execution of this process.
    ///
    /// # Errors
    ///
    /// - `ZX_ERR_BAD_STATE` if the process is dying or dead.
    pub fn suspend(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let status = unsafe { cpp_process_dispatcher_suspend(self.as_ffi_mut()) };
        Status::ok(status)
    }

    /// Resumes execution of this process.
    pub fn resume(&self) {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_resume(self.as_ffi_mut()) }
    }

    /// Creates a handle for the given dispatcher in this process's handle table.
    pub fn make_and_add_handle<T>(
        &self,
        handle: KernelHandle<T>,
        rights: zx_rights_t,
    ) -> Result<HandleValue, Status>
    where
        T: fbl::HasRefCount + fbl::Recyclable + DispatcherOps,
    {
        let mut handle = handle.cast();
        let mut out = HandleValue::default();
        // SAFETY: `self` is a valid `ProcessDispatcher`, `handle` is a valid `KernelHandle`, and
        // `out` points to writable memory.
        let status = unsafe {
            cpp_process_dispatcher_make_and_add_handle(
                self.as_ffi_mut(),
                &mut handle,
                rights,
                &mut out,
            )
        };
        Status::ok(status)?;
        Ok(out)
    }

    /// Creates a handle for the given dispatcher reference in this process's handle table.
    pub fn make_and_add_handle_from_ref<T>(
        &self,
        dispatcher: fbl::RefPtr<T>,
        rights: zx_rights_t,
    ) -> Result<HandleValue, Status>
    where
        T: fbl::HasRefCount + fbl::Recyclable + DispatcherOps,
    {
        // SAFETY: T implements DispatcherOps and is layout-compatible with Dispatcher.
        let raw_dispatcher =
            fbl::RefPtr::into_raw(unsafe { dispatcher.cast::<super::Dispatcher>() });
        let mut out = HandleValue::default();
        // SAFETY: `self` is a valid `ProcessDispatcher`, `raw_dispatcher` carries an acquired reference count
        // transferred to C++, and `out` points to writable memory.
        let status = unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_make_and_add_handle_from_ref(
                self.as_ffi_mut(),
                raw_dispatcher as *mut _,
                rights,
                &mut out,
            )
        };
        Status::ok(status)?;
        Ok(out)
    }

    /// Enforces basic policy for this process.
    pub fn enforce_basic_policy(&self, policy: u32) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let status =
            unsafe { cpp_process_dispatcher_enforce_basic_policy(self.as_ffi_mut(), policy) };
        Status::ok(status)
    }

    /// Returns the timer slack policy amount for this process.
    pub fn get_timer_slack_policy_amount(&self) -> i64 {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_get_timer_slack_policy_amount(
                self.as_ffi(),
            )
        }
    }

    /// Returns the timer slack policy for this process.
    pub fn get_timer_slack_policy(&self) -> crate::kernel::deadline::TimerSlack {
        let mut slack = crate::kernel::deadline::TimerSlack::none();
        // SAFETY: `self` is a valid `ProcessDispatcher` reference and `slack` points to valid memory.
        unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_get_timer_slack_policy(
                self.as_ffi(),
                &mut slack,
            );
        }
        slack
    }

    /// Returns a reference to the handle table's priority-inheriting reader-writer lock.
    #[inline]
    pub fn handle_table_lock(&self) -> &ksync::BrwLockPi<HandleTableLockClass> {
        // SAFETY: `self` is a valid `ProcessDispatcher`, and its handle table lock is a valid `BrwLockPi`.
        unsafe {
            let lock_ptr = super::process_dispatcher_ffi::cpp_process_dispatcher_handle_table_lock(
                self.as_ffi(),
            );
            &*(lock_ptr as *const ksync::BrwLockPi<HandleTableLockClass>)
        }
    }

    /// Returns information about this process.
    pub fn get_info(&self) -> zx_info_process_t {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_get_info(self.as_ffi()) }
    }

    /// Sets this process as critical to the given job.
    pub fn set_critical_to_job(
        &self,
        job: fbl::RefPtr<JobDispatcher>,
        retcode_nonzero: bool,
    ) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference, and `job` transfers an acquired
        // reference count into C++.
        let status = unsafe {
            cpp_process_dispatcher_set_critical_to_job(
                self.as_ffi_mut(),
                fbl::RefPtr::into_raw(job) as *mut _,
                retcode_nonzero,
            )
        };
        Status::ok(status)
    }

    /// Default rights assigned to a newly created ProcessDispatcher handle.
    pub const DEFAULT_RIGHTS: zx_rights_t = zx_types::ZX_RIGHT_TRANSFER
        | zx_types::ZX_RIGHT_DUPLICATE
        | zx_types::ZX_RIGHT_WAIT
        | zx_types::ZX_RIGHT_INSPECT
        | zx_types::ZX_RIGHT_READ
        | zx_types::ZX_RIGHT_WRITE
        | zx_types::ZX_RIGHT_GET_PROPERTY
        | zx_types::ZX_RIGHT_SET_PROPERTY
        | zx_types::ZX_RIGHT_ENUMERATE
        | zx_types::ZX_RIGHT_DESTROY
        | zx_types::ZX_RIGHT_SIGNAL
        | zx_types::ZX_RIGHT_MANAGE_PROCESS
        | zx_types::ZX_RIGHT_MANAGE_THREAD;

    /// Returns the default rights for a process handle.
    pub const fn default_rights() -> zx_rights_t {
        Self::DEFAULT_RIGHTS
    }

    /// Creates a new `ProcessDispatcher`.
    pub fn create(
        job: fbl::RefPtr<JobDispatcher>,
        name: &[u8],
        flags: u32,
    ) -> Result<
        (KernelHandle<Self>, zx_rights_t, KernelHandle<VmAddressRegionDispatcher>, zx_rights_t),
        Status,
    > {
        let mut proc_handle = MaybeUninit::uninit();
        let mut proc_rights = MaybeUninit::<zx_rights_t>::uninit();
        let mut vmar_handle = MaybeUninit::uninit();
        let mut vmar_rights = MaybeUninit::<zx_rights_t>::uninit();

        // SAFETY: `job` is a valid `RefPtr<JobDispatcher>` whose refcount is transferred to C++,
        // `name` points to readable memory of length `name.len()`, and output pointers point to valid uninitialized storage.
        let status = unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_create(
                fbl::RefPtr::into_raw(job) as *mut _,
                name.as_ptr().cast(),
                name.len(),
                flags,
                &raw mut proc_handle,
                &raw mut proc_rights,
                &raw mut vmar_handle,
                &raw mut vmar_rights,
            )
        };
        Status::ok(status)?;
        // SAFETY: Initialized by C++ on success.
        unsafe {
            Ok((
                proc_handle.assume_init(),
                proc_rights.assume_init(),
                vmar_handle.assume_init(),
                vmar_rights.assume_init(),
            ))
        }
    }

    /// Creates a new `ProcessDispatcher` that shares state with `shared_proc`.
    pub fn create_shared(
        shared_proc: fbl::RefPtr<Self>,
        name: &[u8],
        flags: u32,
    ) -> Result<
        (KernelHandle<Self>, zx_rights_t, KernelHandle<VmAddressRegionDispatcher>, zx_rights_t),
        Status,
    > {
        let mut proc_handle = MaybeUninit::uninit();
        let mut proc_rights = MaybeUninit::<zx_rights_t>::uninit();
        let mut vmar_handle = MaybeUninit::uninit();
        let mut vmar_rights = MaybeUninit::<zx_rights_t>::uninit();

        // SAFETY: `shared_proc` is a valid `RefPtr<ProcessDispatcher>` whose refcount is transferred to C++.
        let status = unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_create_shared(
                fbl::RefPtr::into_raw(shared_proc) as *mut _,
                name.as_ptr().cast(),
                name.len(),
                flags,
                &raw mut proc_handle,
                &raw mut proc_rights,
                &raw mut vmar_handle,
                &raw mut vmar_rights,
            )
        };
        Status::ok(status)?;
        // SAFETY: Initialized by C++ on success.
        unsafe {
            Ok((
                proc_handle.assume_init(),
                proc_rights.assume_init(),
                vmar_handle.assume_init(),
                vmar_rights.assume_init(),
            ))
        }
    }

    /// Exits the current process with the given return code.
    pub fn exit_current(retcode: i64) -> ! {
        // SAFETY: Terminating current process execution within valid thread context.
        unsafe { super::process_dispatcher_ffi::cpp_process_dispatcher_exit_current(retcode) }
    }

    /// Returns a reference to this process's address space at the given virtual address.
    pub fn aspace_at(&self, va: usize) -> Option<&crate::vm::vm_aspace::VmAspace> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let aspace_ptr = unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_aspace_at(self.as_ffi_mut(), va)
        };
        // SAFETY: `aspace_ptr` is either null or points to a valid `VmAspace` managed by the process.
        unsafe { aspace_ptr.as_ref() }
    }

    /// Returns the job associated with this process.
    pub fn job(&self) -> Option<fbl::RefPtr<JobDispatcher>> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let ptr =
            unsafe { super::process_dispatcher_ffi::cpp_process_dispatcher_job(self.as_ffi_mut()) };
        // SAFETY: `ptr` is exported via `fbl::ExportToRawPtr` with an acquired refcount.
        unsafe { fbl::RefPtr::try_from_raw(ptr) }
    }

    /// Returns the debug address of the dynamic loader (`_dl_debug_addr`) for this process.
    pub fn get_debug_addr(&self) -> usize {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_get_debug_addr(self.as_ffi()) }
    }

    /// Sets the debug address of the dynamic loader (`_dl_debug_addr`) for this process.
    pub fn set_debug_addr(&self, addr: usize) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let status = unsafe { cpp_process_dispatcher_set_debug_addr(self.as_ffi_mut(), addr) };
        Status::ok(status)
    }

    /// Returns the dynamic break-on-load state for this process.
    pub fn get_dyn_break_on_load(&self) -> usize {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_get_dyn_break_on_load(self.as_ffi()) }
    }

    /// Sets the dynamic break-on-load state for this process.
    pub fn set_dyn_break_on_load(&self, break_on_load: usize) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let status = unsafe {
            cpp_process_dispatcher_set_dyn_break_on_load(self.as_ffi_mut(), break_on_load)
        };
        Status::ok(status)
    }

    /// Returns the base address of the vDSO mapping for this process.
    pub fn vdso_base_address(&self) -> usize {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_vdso_base_address(self.as_ffi()) }
    }

    /// Returns the hardware trace context ID for this process.
    #[cfg(target_arch = "x86_64")]
    pub fn hw_trace_context_id(&self) -> usize {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        unsafe { cpp_process_dispatcher_hw_trace_context_id(self.as_ffi()) }
    }

    /// Returns a `*const ProcessDispatcher` pointer suitable for passing to FFI routines.
    #[inline]
    pub fn as_ffi(&self) -> *const Self {
        self as *const Self
    }

    /// Returns a `*mut ProcessDispatcher` pointer suitable for passing to FFI routines that
    /// take a non-const `ProcessDispatcher*`.
    #[inline]
    pub fn as_ffi_mut(&self) -> *mut Self {
        self.as_ffi() as *mut Self
    }

    /// Grows the futex state pool for this process.
    pub fn futex_context_grow_pool(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let status = unsafe { cpp_process_futex_grow_pool(self.as_ffi()) };
        Status::ok(status)
    }

    /// Shrinks the futex state pool for this process.
    pub fn futex_context_shrink_pool(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher` reference.
        let status = unsafe { cpp_process_futex_shrink_pool(self.as_ffi()) };
        Status::ok(status)
    }

    /// Attaches the normal address space of this process to `core_thread`.
    pub fn attach_normal_aspace_to_thread(&self, core_thread: ThreadPtr) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ProcessDispatcher`, and `core_thread` upholds the
        // `ThreadPtr` invariant that it points to a live kernel thread.
        let status =
            unsafe { cpp_process_attach_aspace_to_thread(self.as_ffi(), core_thread.as_raw()) };
        Status::ok(status)
    }

    /// Returns the parent job koid.
    pub fn job_koid(&self) -> zx_types::zx_koid_t {
        // SAFETY: `self` is a valid `ProcessDispatcher`.
        unsafe { cpp_process_get_job_koid(self.as_ffi()) }
    }

    /// Transitions `thread` from the initialized state to a runnable state and adds it to the
    /// thread list of this process.
    ///
    /// If `ensure_initial_thread` is true, this fails unless `thread` is the initial thread in
    /// the process.
    pub fn add_initialized_thread(
        &self,
        thread: &ThreadDispatcher,
        ensure_initial_thread: bool,
        entry: &UserEntryState,
    ) -> Result<(), Status> {
        // SAFETY: `self` and `thread` are valid references.
        let status = unsafe {
            cpp_process_add_initialized_thread(
                self.as_ffi(),
                thread as *const _,
                ensure_initial_thread,
                entry,
            )
        };
        Status::ok(status)
    }

    /// Removes `thread` from the thread list of this process.
    pub fn remove_thread(&self, thread: &ThreadDispatcher) {
        // SAFETY: `self` and `thread` are valid references.
        unsafe { cpp_process_remove_thread(self.as_ffi(), thread as *const _) }
    }
}

zr::static_assert!(core::mem::size_of::<ProcessDispatcher>() == 0);

/// RAII reader lock guard for a process's handle table.
///
/// Encapsulates the reader lock on the handle table, ensuring that handle lookups and rights
/// checks can only occur while the lock is held.
#[pin_data]
pub struct HandleTableReadGuard<'a> {
    process: &'a ProcessDispatcher,
    #[pin]
    guard: ksync::BrwLockPiReadGuard<'a, HandleTableLockClass>,
}

impl<'a> HandleTableReadGuard<'a> {
    /// Creates a stack-pinned handle table reader lock guard for `process`.
    pub fn new(process: &'a ProcessDispatcher) -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            process,
            guard <- process.handle_table_lock().read_lock(),
        })
    }

    /// Retrieves a handle reference while holding the handle table lock.
    pub fn get_handle(&self, handle_value: HandleValue) -> Option<super::handle::HandleRef<'_>> {
        // SAFETY: `self.process` is valid and the handle table lock is held for the duration of `self`.
        let ptr = unsafe {
            super::process_dispatcher_ffi::cpp_process_dispatcher_handle_table_get_handle_locked(
                self.process.as_ffi(),
                handle_value.raw_value(),
            )
        };
        core::ptr::NonNull::new(ptr).map(|ptr| unsafe { super::handle::HandleRef::from_raw(ptr) })
    }
}
