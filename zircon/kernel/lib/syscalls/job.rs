// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_JOB_CRITICAL_PROCESS_RETCODE_NONZERO, ZX_JOB_POL_ABSOLUTE, ZX_JOB_POL_RELATIVE,
    ZX_RIGHT_DESTROY, ZX_RIGHT_MANAGE_JOB, ZX_RIGHT_SET_POLICY, ZX_RIGHT_WAIT,
    zx_policy_basic_v1_t, zx_policy_basic_v2_t, zx_policy_timer_slack_t,
};

use crate::object::{Dispatcher, HandleValue, JobDispatcher, ProcessDispatcher};
use crate::user_copy::UserInPtr;

const LOCAL_TRACE: u32 = 0;
const MAX_POLICY_COUNT: usize = 32;

fn job_set_policy_basic_v1(
    handle: HandleValue,
    options: u32,
    policy_ptr: UserInPtr<u8>,
    count: u32,
) -> Result<(), Status> {
    if options != ZX_JOB_POL_RELATIVE && options != ZX_JOB_POL_ABSOLUTE {
        return Err(Status::INVALID_ARGS);
    }
    if policy_ptr.is_null() || count == 0 || count as usize > MAX_POLICY_COUNT {
        return Err(Status::INVALID_ARGS);
    }

    let mut storage = [MaybeUninit::<zx_policy_basic_v1_t>::uninit(); MAX_POLICY_COUNT];
    let slice = &mut storage[..count as usize];
    let policy = policy_ptr
        .reinterpret::<zx_policy_basic_v1_t>()
        .copy_slice_from_user(slice)
        .map_err(|_| Status::INVALID_ARGS)?;

    let job = Dispatcher::get_with_rights::<JobDispatcher>(handle, ZX_RIGHT_SET_POLICY)?;
    job.set_basic_policy_v1(options, policy)?;
    Ok(())
}

fn job_set_policy_basic_v2(
    handle: HandleValue,
    options: u32,
    policy_ptr: UserInPtr<u8>,
    count: u32,
) -> Result<(), Status> {
    if options != ZX_JOB_POL_RELATIVE && options != ZX_JOB_POL_ABSOLUTE {
        return Err(Status::INVALID_ARGS);
    }
    if policy_ptr.is_null() || count == 0 || count as usize > MAX_POLICY_COUNT {
        return Err(Status::INVALID_ARGS);
    }

    let mut storage = [MaybeUninit::<zx_policy_basic_v2_t>::uninit(); MAX_POLICY_COUNT];
    let slice = &mut storage[..count as usize];
    let policy = policy_ptr
        .reinterpret::<zx_policy_basic_v2_t>()
        .copy_slice_from_user(slice)
        .map_err(|_| Status::INVALID_ARGS)?;

    let job = Dispatcher::get_with_rights::<JobDispatcher>(handle, ZX_RIGHT_SET_POLICY)?;
    job.set_basic_policy_v2(options, policy)?;
    Ok(())
}

fn job_set_policy_timer_slack(
    handle: HandleValue,
    options: u32,
    policy_ptr: UserInPtr<u8>,
    count: u32,
) -> Result<(), Status> {
    if options != ZX_JOB_POL_RELATIVE {
        return Err(Status::INVALID_ARGS);
    }
    if policy_ptr.is_null() || count != 1 {
        return Err(Status::INVALID_ARGS);
    }

    let mut uninit_policy = MaybeUninit::<zx_policy_timer_slack_t>::uninit();
    let slack_policy = policy_ptr
        .reinterpret::<zx_policy_timer_slack_t>()
        .copy_from_user(&mut uninit_policy)
        .map_err(|_| Status::INVALID_ARGS)?;

    let job = Dispatcher::get_with_rights::<JobDispatcher>(handle, ZX_RIGHT_SET_POLICY)?;
    job.set_timer_slack_policy(slack_policy)?;
    Ok(())
}

#[syscall]
pub fn sys_job_create(
    parent_job: HandleValue,
    options: u32,
    out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("parent: {:#x}\n", parent_job.raw_value());

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let parent = Dispatcher::get_with_rights::<JobDispatcher>(parent_job, ZX_RIGHT_MANAGE_JOB)?;
    let (handle, rights) = JobDispatcher::create(options, parent)?;
    *out = ProcessDispatcher::with_current(|up| up.make_and_add_handle(handle, rights))?;
    Ok(())
}

#[syscall]
pub fn sys_job_set_policy(
    handle: HandleValue,
    options: u32,
    topic: u32,
    policy_ptr: UserInPtr<u8>,
    count: u32,
) -> Result<(), Status> {
    ltracef!("handle {:#x}, options {}, topic {}\n", handle.raw_value(), options, topic);

    match topic {
        zx_types::ZX_JOB_POL_BASIC_V1 => {
            job_set_policy_basic_v1(handle, options, policy_ptr, count)
        }
        zx_types::ZX_JOB_POL_BASIC_V2 => {
            job_set_policy_basic_v2(handle, options, policy_ptr, count)
        }
        zx_types::ZX_JOB_POL_TIMER_SLACK => {
            job_set_policy_timer_slack(handle, options, policy_ptr, count)
        }
        _ => Err(Status::INVALID_ARGS),
    }
}

#[syscall]
pub fn sys_job_set_critical(
    job_handle: HandleValue,
    options: u32,
    process_handle: HandleValue,
) -> Result<(), Status> {
    ltracef!(
        "job_handle {:#x}, options {}, process_handle {:#x}\n",
        job_handle.raw_value(),
        options,
        process_handle.raw_value()
    );

    let retcode_nonzero = if options == ZX_JOB_CRITICAL_PROCESS_RETCODE_NONZERO {
        true
    } else if options != 0 {
        return Err(Status::INVALID_ARGS);
    } else {
        false
    };

    let job = Dispatcher::get_with_rights::<JobDispatcher>(job_handle, ZX_RIGHT_DESTROY)?;
    let process = Dispatcher::get_with_rights::<ProcessDispatcher>(process_handle, ZX_RIGHT_WAIT)?;
    process.set_critical_to_job(job, retcode_nonzero)?;
    Ok(())
}
