// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::process_dispatcher::ProcessDispatcher;
use super::thread_dispatcher_ffi::{
    cpp_sys_thread_legacy_yield, cpp_sys_thread_raise_exception, cpp_thread_dispatcher_create,
    cpp_thread_dispatcher_exit_current, cpp_thread_dispatcher_get_exception_report,
    cpp_thread_dispatcher_get_info_for_userspace, cpp_thread_dispatcher_get_runtime_stats,
    cpp_thread_dispatcher_get_stats_for_userspace, cpp_thread_dispatcher_initialize,
    cpp_thread_dispatcher_is_current, cpp_thread_dispatcher_kill,
    cpp_thread_dispatcher_kill_current, cpp_thread_dispatcher_read_state,
    cpp_thread_dispatcher_restricted_kick, cpp_thread_dispatcher_resume,
    cpp_thread_dispatcher_set_base_profile, cpp_thread_dispatcher_set_soft_affinity,
    cpp_thread_dispatcher_start, cpp_thread_dispatcher_suspend, cpp_thread_dispatcher_write_state,
};
use crate::kernel::scheduler_state::SchedulerStateBaseProfile;
use core::mem::MaybeUninit;
use zx_status::Status;
use zx_types::{
    zx_exception_context_t, zx_excp_type_t, zx_info_task_runtime_t, zx_info_thread_stats_t,
    zx_info_thread_t, zx_rights_t, zx_vaddr_t,
};

pub use crate::kernel::types::cpu_mask_t;

crate::object::dispatcher::impl_dispatcher_facade!(
    pub struct ThreadDispatcher,
    zx_types::ZX_OBJ_TYPE_THREAD
);

zr::static_assert!(core::mem::size_of::<ThreadDispatcher>() == 0);

impl ThreadDispatcher {
    /// Default rights assigned to a newly created ThreadDispatcher handle.
    pub const DEFAULT_RIGHTS: zx_rights_t = zx_types::ZX_RIGHT_TRANSFER
        | zx_types::ZX_RIGHT_DUPLICATE
        | zx_types::ZX_RIGHT_WAIT
        | zx_types::ZX_RIGHT_INSPECT
        | zx_types::ZX_RIGHT_READ
        | zx_types::ZX_RIGHT_WRITE
        | zx_types::ZX_RIGHT_GET_PROPERTY
        | zx_types::ZX_RIGHT_SET_PROPERTY
        | zx_types::ZX_RIGHT_DESTROY
        | zx_types::ZX_RIGHT_SIGNAL
        | zx_types::ZX_RIGHT_MANAGE_THREAD;

    /// Returns default rights for a thread handle.
    pub const fn default_rights() -> zx_rights_t {
        Self::DEFAULT_RIGHTS
    }

    /// Creates a new child `ThreadDispatcher` under the given parent process.
    pub fn create(
        process: fbl::RefPtr<ProcessDispatcher>,
        flags: u32,
        name: &[u8],
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let process_raw = fbl::RefPtr::into_raw(process) as *mut _;
        // SAFETY: `process_raw` is a valid pointer carrying an acquired refcount, and
        // `cpp_thread_dispatcher_create` initializes `handle` and `rights` on success.
        unsafe {
            KernelHandle::create_with_rights(|handle_out, rights_out| {
                cpp_thread_dispatcher_create(
                    process_raw,
                    flags,
                    name.as_ptr().cast(),
                    name.len(),
                    handle_out,
                    rights_out,
                )
            })
        }
    }

    /// Initializes a newly created `ThreadDispatcher`.
    pub fn initialize(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        let status = unsafe { cpp_thread_dispatcher_initialize(self as *const _ as *mut _) };
        Status::ok(status)
    }

    /// Starts execution of this thread.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        entry: zx_vaddr_t,
        stack: zx_vaddr_t,
        arg1: u64,
        arg2: u64,
        tp: u64,
        abi_reg: u64,
        ensure_initial_thread: bool,
    ) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        let status = unsafe {
            cpp_thread_dispatcher_start(
                self as *const _ as *mut _,
                entry,
                stack,
                arg1,
                arg2,
                tp,
                abi_reg,
                ensure_initial_thread,
            )
        };
        Status::ok(status)
    }

    /// Terminates the current thread. Does not return.
    pub fn exit_current() -> ! {
        // SAFETY: Terminates the current execution thread cleanly.
        unsafe {
            cpp_thread_dispatcher_exit_current();
        }
        unreachable!()
    }

    /// Marks the current thread for termination.
    pub fn kill_current() {
        // SAFETY: Sets the current thread's state to dying.
        unsafe {
            cpp_thread_dispatcher_kill_current();
        }
    }

    /// Kills this thread.
    pub fn kill(&self) {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        unsafe { cpp_thread_dispatcher_kill(self as *const _ as *mut _) }
    }

    /// Returns whether this `ThreadDispatcher` is the current thread.
    pub fn is_current(&self) -> bool {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        unsafe { cpp_thread_dispatcher_is_current(self as *const _) }
    }

    /// Suspends execution of this thread.
    pub fn suspend(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        let status = unsafe { cpp_thread_dispatcher_suspend(self as *const _ as *mut _) };
        Status::ok(status)
    }

    /// Resumes execution of this thread.
    pub fn resume(&self) {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        unsafe { cpp_thread_dispatcher_resume(self as *const _ as *mut _) }
    }

    /// Kicks this thread out of restricted mode.
    pub fn restricted_kick(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        Status::ok(unsafe { cpp_thread_dispatcher_restricted_kick(self as *const _ as *mut _) })
    }

    /// Reads architectural state from this thread into `buffer`.
    pub fn read_state(
        &self,
        state_kind: u32,
        buffer: *mut core::ffi::c_void,
        buffer_size: usize,
    ) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        let status = unsafe {
            cpp_thread_dispatcher_read_state(
                self as *const _ as *mut _,
                state_kind,
                buffer,
                buffer_size,
            )
        };
        Status::ok(status)
    }

    /// Writes architectural state to this thread from `buffer`.
    pub fn write_state(
        &self,
        state_kind: u32,
        buffer: *const core::ffi::c_void,
        buffer_size: usize,
    ) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        let status = unsafe {
            cpp_thread_dispatcher_write_state(
                self as *const _ as *mut _,
                state_kind,
                buffer,
                buffer_size,
            )
        };
        Status::ok(status)
    }

    /// Sets the base profile for this thread.
    pub fn set_base_profile(&self, profile: &SchedulerStateBaseProfile) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference and `profile` points to
        // an opaque `SchedulerState::BaseProfile`.
        let status = unsafe {
            cpp_thread_dispatcher_set_base_profile(
                self as *const _ as *mut _,
                profile.get() as *const _,
            )
        };
        Status::ok(status)
    }

    /// Sets soft CPU affinity for this thread.
    pub fn set_soft_affinity(&self, mask: cpu_mask_t) -> Result<(), Status> {
        // SAFETY: `self` is a valid `ThreadDispatcher` reference.
        let status =
            unsafe { cpp_thread_dispatcher_set_soft_affinity(self as *const _ as *mut _, mask) };
        Status::ok(status)
    }

    /// Returns thread info for userspace.
    pub fn get_info_for_userspace(&self) -> zx_info_thread_t {
        let mut info = MaybeUninit::<zx_info_thread_t>::uninit();
        // SAFETY: `self` is valid and `info` points to uninitialized stack memory.
        unsafe {
            cpp_thread_dispatcher_get_info_for_userspace(self as *const _, info.as_mut_ptr());
            info.assume_init()
        }
    }

    /// Returns thread CPU stats for userspace.
    pub fn get_stats_for_userspace(&self) -> Result<zx_info_thread_stats_t, Status> {
        let mut info = MaybeUninit::<zx_info_thread_stats_t>::uninit();
        // SAFETY: `self` is valid and `info` points to uninitialized stack memory.
        let status = unsafe {
            cpp_thread_dispatcher_get_stats_for_userspace(
                self as *const _ as *mut _,
                info.as_mut_ptr(),
            )
        };
        Status::ok(status)?;
        // SAFETY: `cpp_thread_dispatcher_get_stats_for_userspace` initialized `info` on success.
        unsafe { Ok(info.assume_init()) }
    }

    /// Returns thread task runtime stats.
    pub fn get_runtime_stats(&self) -> zx_info_task_runtime_t {
        let mut info = MaybeUninit::<zx_info_task_runtime_t>::uninit();
        // SAFETY: `self` is valid and `info` points to uninitialized stack memory.
        unsafe {
            cpp_thread_dispatcher_get_runtime_stats(self as *const _, info.as_mut_ptr());
            info.assume_init()
        }
    }

    /// Returns the thread's current exception report.
    pub fn get_exception_report(&self) -> Result<zx_types::zx_exception_report_t, Status> {
        let mut report = MaybeUninit::<zx_types::zx_exception_report_t>::uninit();
        // SAFETY: self is a valid ThreadDispatcher and report points to uninitialized stack memory.
        let status = unsafe {
            cpp_thread_dispatcher_get_exception_report(self as *const _, report.as_mut_ptr())
        };
        Status::ok(status)?;
        // SAFETY: cpp_thread_dispatcher_get_exception_report succeeded and initialized report.
        Ok(unsafe { report.assume_init() })
    }

    /// Raises a user exception for the job debugger.
    pub fn raise_user_exception(
        options: u32,
        exception_type: zx_excp_type_t,
        user_context: &zx_exception_context_t,
    ) -> Result<(), Status> {
        // SAFETY: `user_context` is a valid reference.
        let status =
            unsafe { cpp_sys_thread_raise_exception(options, exception_type, user_context) };
        Status::ok(status)
    }

    /// Yields execution of the current thread.
    pub fn legacy_yield(options: u32) -> Result<(), Status> {
        // SAFETY: `cpp_sys_thread_legacy_yield` yields the current thread if options is 0.
        let status = unsafe { cpp_sys_thread_legacy_yield(options) };
        Status::ok(status)
    }
}

/// The reason a thread is currently blocked, matching C++ `ThreadDispatcher::Blocked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Blocked(pub u32);

impl Blocked {
    pub const NONE: Self = Self(0);
    pub const EXCEPTION: Self = Self(1);
    pub const SLEEPING: Self = Self(2);
    pub const FUTEX: Self = Self(3);
    pub const PORT: Self = Self(4);
    pub const CHANNEL: Self = Self(5);
    pub const WAIT_ONE: Self = Self(6);
    pub const WAIT_MANY: Self = Self(7);
    pub const INTERRUPT: Self = Self(8);
    pub const PAGER: Self = Self(9);
}

/// RAII helper that sets the current thread's `blocked_reason_` and restores it on drop.
pub struct AutoBlocked {
    prev_reason: Blocked,
}

impl AutoBlocked {
    /// Sets the blocked reason on the current thread.
    pub fn new(reason: Blocked) -> Self {
        // SAFETY: `cpp_thread_dispatcher_set_blocked_reason` transitions the current thread's
        // blocked state on the current CPU and returns the previous blocked reason.
        let prev_reason = unsafe {
            super::thread_dispatcher_ffi::cpp_thread_dispatcher_set_blocked_reason(reason)
        };
        Self { prev_reason }
    }
}

impl Drop for AutoBlocked {
    fn drop(&mut self) {
        // SAFETY: Restoring the previous blocked reason on the current thread upon exiting
        // the blocking scope is always safe and restores previous thread state.
        unsafe {
            super::thread_dispatcher_ffi::cpp_thread_dispatcher_set_blocked_reason(
                self.prev_reason,
            );
        }
    }
}

zr::static_assert!(core::mem::size_of::<Blocked>() == 4);
zr::static_assert!(core::mem::align_of::<Blocked>() == 4);
