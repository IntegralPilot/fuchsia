// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;
use zx_status::Status;
use zx_types::{
    ZX_OBJECT_SIGNAL_6, ZX_RIGHT_DESTROY, ZX_RIGHT_DUPLICATE, ZX_RIGHT_ENUMERATE,
    ZX_RIGHT_GET_POLICY, ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_INSPECT, ZX_RIGHT_MANAGE_JOB,
    ZX_RIGHT_MANAGE_PROCESS, ZX_RIGHT_MANAGE_THREAD, ZX_RIGHT_READ, ZX_RIGHT_SET_POLICY,
    ZX_RIGHT_SET_PROPERTY, ZX_RIGHT_SIGNAL, ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT, ZX_RIGHT_WRITE,
    zx_policy_basic_v1_t, zx_policy_basic_v2_t, zx_policy_timer_slack_t, zx_rights_t, zx_signals_t,
};

use super::handle::KernelHandle;
use super::job_dispatcher_ffi::{
    cpp_job_dispatcher_create, cpp_job_dispatcher_enumerate_children, cpp_job_dispatcher_get_info,
    cpp_job_dispatcher_get_kill_on_oom, cpp_job_dispatcher_get_root_job,
    cpp_job_dispatcher_get_runtime_stats, cpp_job_dispatcher_is_root, cpp_job_dispatcher_kill,
    cpp_job_dispatcher_kill_job_with_kill_on_oom, cpp_job_dispatcher_max_height,
    cpp_job_dispatcher_parent, cpp_job_dispatcher_set_basic_policy_v1,
    cpp_job_dispatcher_set_basic_policy_v2, cpp_job_dispatcher_set_kill_on_oom,
    cpp_job_dispatcher_set_timer_slack_policy,
};

/// Default rights assigned to a newly created JobDispatcher handle.
pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_READ
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_GET_PROPERTY
    | ZX_RIGHT_SET_PROPERTY
    | ZX_RIGHT_GET_POLICY
    | ZX_RIGHT_SET_POLICY
    | ZX_RIGHT_ENUMERATE
    | ZX_RIGHT_DESTROY
    | ZX_RIGHT_SIGNAL
    | ZX_RIGHT_MANAGE_JOB
    | ZX_RIGHT_MANAGE_PROCESS
    | ZX_RIGHT_MANAGE_THREAD;

/// Maximum height of the root job hierarchy.
pub const ROOT_JOB_MAX_HEIGHT: u32 = 32;

/// Typical inline capacity assumed for basic policy updates.
pub const POLICY_BASIC_INLINE_COUNT: usize = 8;

/// Job signal active when a job has no child jobs and no child processes.
///
/// TODO(https://fxbug.dev/42131457): This is a temporary signal that we don't want userspace using
/// (yet?). Either expose this signal to userspace in "zircon/types.h", or remove this signal.
pub const ZX_JOB_NO_CHILDREN: zx_signals_t = ZX_OBJECT_SIGNAL_6;

zr::static_assert!(core::mem::size_of::<zx_policy_basic_v1_t>() == 8);
zr::static_assert!(core::mem::align_of::<zx_policy_basic_v1_t>() == 4);
zr::static_assert!(core::mem::size_of::<zx_policy_basic_v2_t>() == 12);
zr::static_assert!(core::mem::align_of::<zx_policy_basic_v2_t>() == 4);
zr::static_assert!(core::mem::size_of::<zx_policy_timer_slack_t>() == 16);
zr::static_assert!(core::mem::align_of::<zx_policy_timer_slack_t>() == 8);

crate::object::dispatcher::impl_dispatcher_facade!(
    pub struct JobDispatcher,
    zx_types::ZX_OBJ_TYPE_JOB
);

zr::static_assert!(core::mem::size_of::<JobDispatcher>() == 0);

impl JobDispatcher {
    /// Returns the default rights for a JobDispatcher handle.
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a child `JobDispatcher` under `parent`.
    pub fn create(
        flags: u32,
        parent: fbl::RefPtr<JobDispatcher>,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        // SAFETY: `parent` transfers an acquired reference count into C++, and
        // `cpp_job_dispatcher_create` initializes `handle` and `rights` on success.
        unsafe {
            KernelHandle::create_with_rights(|handle_out, rights_out| {
                cpp_job_dispatcher_create(
                    flags,
                    fbl::RefPtr::into_raw(parent) as *mut _,
                    handle_out,
                    rights_out,
                )
            })
        }
    }

    /// Sets basic policy (v1) on this job.
    pub fn set_basic_policy_v1(
        &self,
        mode: u32,
        policy: &[zx_policy_basic_v1_t],
    ) -> Result<(), Status> {
        // SAFETY: `self` is a valid `JobDispatcher` and `policy` points to `policy.len()` elements.
        let status = unsafe {
            cpp_job_dispatcher_set_basic_policy_v1(
                self as *const _ as *mut _,
                mode,
                policy.as_ptr(),
                policy.len(),
            )
        };
        Status::ok(status)
    }

    /// Sets basic policy (v2) on this job.
    pub fn set_basic_policy_v2(
        &self,
        mode: u32,
        policy: &[zx_policy_basic_v2_t],
    ) -> Result<(), Status> {
        // SAFETY: `self` is a valid `JobDispatcher` and `policy` points to `policy.len()` elements.
        let status = unsafe {
            cpp_job_dispatcher_set_basic_policy_v2(
                self as *const _ as *mut _,
                mode,
                policy.as_ptr(),
                policy.len(),
            )
        };
        Status::ok(status)
    }

    /// Sets timer slack policy on this job.
    pub fn set_timer_slack_policy(&self, policy: &zx_policy_timer_slack_t) -> Result<(), Status> {
        // SAFETY: `self` is a valid `JobDispatcher` and `policy` points to a valid struct.
        let status = unsafe {
            cpp_job_dispatcher_set_timer_slack_policy(
                self as *const _ as *mut _,
                policy as *const _,
            )
        };
        Status::ok(status)
    }

    /// Returns whether this `JobDispatcher` is the root job.
    pub fn is_root(&self) -> bool {
        // SAFETY: `self` is a valid `JobDispatcher` reference.
        unsafe { cpp_job_dispatcher_is_root(self as *const _) }
    }

    /// Returns a reference to the root job dispatcher.
    pub fn get_root_job() -> fbl::RefPtr<JobDispatcher> {
        // SAFETY: `cpp_job_dispatcher_get_root_job` returns a raw pointer carrying an acquired refcount.
        unsafe { fbl::RefPtr::from_raw(cpp_job_dispatcher_get_root_job()) }
    }

    /// Returns the parent job dispatcher, or `None` if this is the root job.
    pub fn parent(&self) -> Option<fbl::RefPtr<JobDispatcher>> {
        // SAFETY: `self` is a valid `JobDispatcher` reference.
        let raw = unsafe { cpp_job_dispatcher_parent(self as *const _) };
        if raw.is_null() {
            None
        } else {
            // SAFETY: `raw` is a valid pointer carrying an acquired refcount.
            Some(unsafe { fbl::RefPtr::from_raw(raw) })
        }
    }

    /// Returns the maximum allowed height for child jobs under this job.
    pub fn max_height(&self) -> u32 {
        // SAFETY: `self` is a valid `JobDispatcher` reference.
        unsafe { cpp_job_dispatcher_max_height(self as *const _) }
    }

    /// Terminates this job and all child jobs and child processes.
    pub fn kill(&self, return_code: i64) -> bool {
        // SAFETY: `self` is a valid `JobDispatcher` reference.
        unsafe { cpp_job_dispatcher_kill(self as *const _ as *mut _, return_code) }
    }

    /// Sets whether this job should be killed during out-of-memory events.
    pub fn set_kill_on_oom(&self, value: bool) {
        // SAFETY: `self` is a valid `JobDispatcher` reference.
        unsafe { cpp_job_dispatcher_set_kill_on_oom(self as *const _ as *mut _, value) }
    }

    /// Returns whether this job is configured to be killed on out-of-memory events.
    pub fn get_kill_on_oom(&self) -> bool {
        // SAFETY: `self` is a valid `JobDispatcher` reference.
        unsafe { cpp_job_dispatcher_get_kill_on_oom(self as *const _) }
    }

    /// Kills the lowest child job that has `kill_on_oom` set.
    pub fn kill_job_with_kill_on_oom(&self) -> bool {
        // SAFETY: `self` is a valid `JobDispatcher` reference.
        unsafe { cpp_job_dispatcher_kill_job_with_kill_on_oom(self as *const _ as *mut _) }
    }

    /// Returns information about the job.
    pub fn get_info(&self) -> zx_types::zx_info_job_t {
        let mut info = MaybeUninit::<zx_types::zx_info_job_t>::uninit();
        // SAFETY: `self` is valid and `info` points to uninitialized stack memory.
        unsafe {
            cpp_job_dispatcher_get_info(self as *const _, info.as_mut_ptr());
            info.assume_init()
        }
    }

    /// Returns aggregate runtime stats for this job and child processes.
    pub fn get_runtime_stats(&self) -> zx_types::zx_info_task_runtime_t {
        // SAFETY: self is a valid JobDispatcher reference.
        unsafe { cpp_job_dispatcher_get_runtime_stats(self as *const _) }
    }

    /// Enumerates child jobs or child processes into user buffer and returns (actual_count, available_count).
    pub fn enumerate_children(
        &self,
        user_koids: crate::user_copy::UserOutPtr<zx_types::zx_koid_t>,
        max: usize,
        is_jobs: bool,
    ) -> Result<(usize, usize), Status> {
        let mut count = 0usize;
        let mut avail = 0usize;
        // SAFETY: self is a valid JobDispatcher reference and user_koids is a user pointer.
        let status = unsafe {
            cpp_job_dispatcher_enumerate_children(
                self as *const _,
                user_koids.as_ptr(),
                max,
                is_jobs,
                &mut count,
                &mut avail,
            )
        };
        Status::ok(status)?;
        Ok((count, avail))
    }
}
