// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::process_dispatcher::ProcessDispatcher;
use super::thread_dispatcher::ThreadDispatcher;
use core::mem::MaybeUninit;
use zx_types::{
    zx_exception_context_t, zx_excp_type_t, zx_info_task_runtime_t, zx_info_thread_stats_t,
    zx_info_thread_t, zx_rights_t, zx_status_t, zx_vaddr_t,
};

unsafe extern "C" {
    /// Checks if the given ThreadDispatcher is the current thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_is_current(thread: *const ThreadDispatcher) -> bool;

    /// Creates a new ThreadDispatcher.
    ///
    /// # Safety
    ///
    /// `process` must point to a valid `ProcessDispatcher` whose reference count was transferred.
    /// `name_ptr` must point to at least `name_len` readable bytes.
    /// `out_handle` must point to valid uninitialized memory for a `KernelHandle<ThreadDispatcher>`.
    /// `out_rights` must point to valid uninitialized memory for a `zx_rights_t`.
    pub(crate) fn cpp_thread_dispatcher_create(
        process: *mut ProcessDispatcher,
        flags: u32,
        name_ptr: *const core::ffi::c_char,
        name_len: usize,
        out_handle: *mut MaybeUninit<KernelHandle<ThreadDispatcher>>,
        out_rights: *mut MaybeUninit<zx_rights_t>,
    ) -> zx_status_t;

    /// Initializes a ThreadDispatcher.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_initialize(thread: *mut ThreadDispatcher) -> zx_status_t;

    /// Starts execution of a thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_start(
        thread: *mut ThreadDispatcher,
        entry: zx_vaddr_t,
        stack: zx_vaddr_t,
        arg1: u64,
        arg2: u64,
        tp: u64,
        abi_reg: u64,
        ensure_initial_thread: bool,
    ) -> zx_status_t;

    /// Terminates current thread.
    ///
    /// # Safety
    ///
    /// Must only be called on a running user-mode thread. Terminates current thread execution.
    pub(crate) fn cpp_thread_dispatcher_exit_current();

    /// Marks current thread for termination.
    ///
    /// # Safety
    ///
    /// Must only be called on a running user-mode thread.
    pub(crate) fn cpp_thread_dispatcher_kill_current();

    /// Kills a thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_kill(thread: *mut ThreadDispatcher);

    /// Calls into C++ implementation to suspend a thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_suspend(thread: *mut ThreadDispatcher) -> zx_status_t;

    /// Calls into C++ implementation to resume a thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_resume(thread: *mut ThreadDispatcher);

    /// Calls into C++ implementation to kick a thread out of restricted mode.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_restricted_kick(
        thread: *mut ThreadDispatcher,
    ) -> zx_status_t;

    /// Reads thread architectural state.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    /// `buffer` must point to at least `buffer_size` writable bytes.
    pub(crate) fn cpp_thread_dispatcher_read_state(
        thread: *mut ThreadDispatcher,
        state_kind: u32,
        buffer: *mut core::ffi::c_void,
        buffer_size: usize,
    ) -> zx_status_t;

    /// Writes thread architectural state.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    /// `buffer` must point to at least `buffer_size` readable bytes.
    pub(crate) fn cpp_thread_dispatcher_write_state(
        thread: *mut ThreadDispatcher,
        state_kind: u32,
        buffer: *const core::ffi::c_void,
        buffer_size: usize,
    ) -> zx_status_t;

    /// Calls into C++ implementation to set the base profile of a thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    /// `profile` must point to a valid `SchedulerStateBaseProfile`.
    pub(crate) fn cpp_thread_dispatcher_set_base_profile(
        thread: *mut ThreadDispatcher,
        profile: *const crate::kernel::scheduler_state::SchedulerStateBaseProfile,
    ) -> zx_status_t;

    /// Calls into C++ implementation to set the soft affinity of a thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    pub(crate) fn cpp_thread_dispatcher_set_soft_affinity(
        thread: *mut ThreadDispatcher,
        mask: crate::kernel::types::cpu_mask_t,
    ) -> zx_status_t;

    /// Gets info struct for userspace.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    /// `out_info` must point to valid uninitialized memory for `zx_info_thread_t`.
    pub(crate) fn cpp_thread_dispatcher_get_info_for_userspace(
        thread: *const ThreadDispatcher,
        out_info: *mut zx_info_thread_t,
    );

    /// Gets stats struct for userspace.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    /// `out_info` must point to valid uninitialized memory for `zx_info_thread_stats_t`.
    pub(crate) fn cpp_thread_dispatcher_get_stats_for_userspace(
        thread: *mut ThreadDispatcher,
        out_info: *mut zx_info_thread_stats_t,
    ) -> zx_status_t;

    /// Gets runtime stats struct.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    /// `out_info` must point to valid uninitialized memory for `zx_info_task_runtime_t`.
    pub(crate) fn cpp_thread_dispatcher_get_runtime_stats(
        thread: *const ThreadDispatcher,
        out_info: *mut zx_info_task_runtime_t,
    );

    /// Raises an exception for job debugger.
    ///
    /// # Safety
    ///
    /// `user_context` must point to a valid initialized `zx_exception_context_t`.
    pub(crate) fn cpp_sys_thread_raise_exception(
        options: u32,
        exception_type: zx_excp_type_t,
        user_context: &zx_exception_context_t,
    ) -> zx_status_t;

    /// Legacy thread yield.
    ///
    /// # Safety
    ///
    /// Yields execution of the current thread to the scheduler.
    pub(crate) fn cpp_sys_thread_legacy_yield(options: u32) -> zx_status_t;
    /// Sets the thread blocked reason and returns the previous blocked reason.
    pub(crate) fn cpp_thread_dispatcher_set_blocked_reason(
        reason: super::thread_dispatcher::Blocked,
    ) -> super::thread_dispatcher::Blocked;

    /// Retrieves exception report from thread.
    ///
    /// # Safety
    ///
    /// `thread` must point to a valid `ThreadDispatcher`.
    /// `out_report` must point to valid uninitialized memory for `zx_exception_report_t`.
    pub(crate) fn cpp_thread_dispatcher_get_exception_report(
        thread: *const ThreadDispatcher,
        out_report: *mut zx_types::zx_exception_report_t,
    ) -> zx_status_t;
}
