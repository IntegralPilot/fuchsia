// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::job_dispatcher::JobDispatcher;
use core::mem::MaybeUninit;
use zx_types::{
    zx_info_job_t, zx_policy_basic_v1_t, zx_policy_basic_v2_t, zx_policy_timer_slack_t,
    zx_rights_t, zx_status_t,
};

unsafe extern "C" {
    /// Checks if the given JobDispatcher is the root job dispatcher.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    pub(crate) fn cpp_job_dispatcher_is_root(job: *const JobDispatcher) -> bool;

    /// Creates a new child JobDispatcher under the given parent job.
    ///
    /// # Safety
    ///
    /// `parent` must point to a valid `JobDispatcher` whose reference count was transferred.
    /// `handle` must point to valid uninitialized memory for a `KernelHandle<JobDispatcher>`.
    /// `rights` must point to valid uninitialized memory for a `zx_rights_t`.
    pub(crate) fn cpp_job_dispatcher_create(
        flags: u32,
        parent: *mut JobDispatcher,
        handle: *mut MaybeUninit<KernelHandle<JobDispatcher>>,
        rights: *mut MaybeUninit<zx_rights_t>,
    ) -> zx_status_t;

    /// Sets basic policy (v1) on the given job.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    /// `policy` must point to `count` valid elements of `zx_policy_basic_v1_t`.
    pub(crate) fn cpp_job_dispatcher_set_basic_policy_v1(
        job: *mut JobDispatcher,
        mode: u32,
        policy: *const zx_policy_basic_v1_t,
        count: usize,
    ) -> zx_status_t;

    /// Sets basic policy (v2) on the given job.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    /// `policy` must point to `count` valid elements of `zx_policy_basic_v2_t`.
    pub(crate) fn cpp_job_dispatcher_set_basic_policy_v2(
        job: *mut JobDispatcher,
        mode: u32,
        policy: *const zx_policy_basic_v2_t,
        count: usize,
    ) -> zx_status_t;

    /// Sets timer slack policy on the given job.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    /// `policy` must point to a valid `zx_policy_timer_slack_t`.
    pub(crate) fn cpp_job_dispatcher_set_timer_slack_policy(
        job: *mut JobDispatcher,
        policy: *const zx_policy_timer_slack_t,
    ) -> zx_status_t;

    /// Returns a reference-counted pointer to the root job dispatcher.
    pub(crate) fn cpp_job_dispatcher_get_root_job() -> *mut JobDispatcher;

    /// Returns a reference-counted pointer to the parent job dispatcher, or null if root.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    pub(crate) fn cpp_job_dispatcher_parent(job: *const JobDispatcher) -> *mut JobDispatcher;

    /// Returns the maximum height allowed for child jobs under this job.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    pub(crate) fn cpp_job_dispatcher_max_height(job: *const JobDispatcher) -> u32;

    /// Kills the job and all child processes and child jobs.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    pub(crate) fn cpp_job_dispatcher_kill(job: *mut JobDispatcher, return_code: i64) -> bool;

    /// Sets whether this job should be killed on out-of-memory events.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    pub(crate) fn cpp_job_dispatcher_set_kill_on_oom(job: *mut JobDispatcher, value: bool);

    /// Returns whether this job should be killed on out-of-memory events.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    pub(crate) fn cpp_job_dispatcher_get_kill_on_oom(job: *const JobDispatcher) -> bool;

    /// Kills the lowest child job that has kill_on_oom set.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    pub(crate) fn cpp_job_dispatcher_kill_job_with_kill_on_oom(job: *mut JobDispatcher) -> bool;

    /// Gets info struct for the job.
    ///
    /// # Safety
    ///
    /// `job` must point to a valid `JobDispatcher`.
    /// `info_out` must point to writable memory for `zx_info_job_t`.
    pub(crate) fn cpp_job_dispatcher_get_info(
        job: *const JobDispatcher,
        info_out: *mut zx_info_job_t,
    );

    pub(crate) fn cpp_job_dispatcher_get_runtime_stats(
        job: *const JobDispatcher,
    ) -> zx_types::zx_info_task_runtime_t;

    pub(crate) fn cpp_job_dispatcher_enumerate_children(
        job: *const JobDispatcher,
        user_koids: *mut zx_types::zx_koid_t,
        max: usize,
        is_jobs: bool,
        out_count: *mut usize,
        out_avail: *mut usize,
    ) -> zx_status_t;
}
