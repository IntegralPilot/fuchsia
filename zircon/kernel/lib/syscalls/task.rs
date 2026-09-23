// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{
    Dispatcher, HandleValue, JobDispatcher, ProcessDispatcher, SuspendTokenDispatcher,
    ThreadDispatcher,
};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_RIGHT_DESTROY, ZX_RIGHT_WRITE, ZX_TASK_RETCODE_SYSCALL_KILL};

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_task_kill(handle: HandleValue) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    let dispatcher = Dispatcher::get_with_rights::<Dispatcher>(handle, ZX_RIGHT_DESTROY)?;

    if let Some(job) = dispatcher.downcast::<JobDispatcher>() {
        job.kill(ZX_TASK_RETCODE_SYSCALL_KILL);
        Ok(())
    } else if let Some(process) = dispatcher.downcast::<ProcessDispatcher>() {
        process.kill(ZX_TASK_RETCODE_SYSCALL_KILL);
        Ok(())
    } else if dispatcher.downcast::<ThreadDispatcher>().is_some() {
        Err(Status::NOT_SUPPORTED)
    } else {
        Err(Status::WRONG_TYPE)
    }
}

#[syscall]
pub fn sys_task_suspend(handle: HandleValue, token: &mut HandleValue) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    let task = Dispatcher::get_with_rights::<Dispatcher>(handle, ZX_RIGHT_WRITE)?;
    let (new_token, rights) = SuspendTokenDispatcher::create(task)?;
    *token = ProcessDispatcher::with_current(|up| up.make_and_add_handle(new_token, rights))?;
    Ok(())
}

#[syscall]
pub fn sys_task_suspend_token(handle: HandleValue, token: &mut HandleValue) -> Result<(), Status> {
    sys_task_suspend(handle, token)
}
