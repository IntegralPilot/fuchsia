// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{EventDispatcher, EventPairDispatcher, HandleValue, ProcessDispatcher};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_POL_NEW_EVENT, ZX_POL_NEW_EVENTPAIR};

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_event_create(options: u32, out: &mut HandleValue) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_EVENT))?;

    let (kernel_handle, rights) = EventDispatcher::create(options)?;
    let user_handle = kernel_handle.make_and_add_handle(rights)?;
    *out = user_handle;
    Ok(())
}

#[syscall]
pub fn sys_eventpair_create(
    options: u32,
    out0: &mut HandleValue,
    out1: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    if options != 0 {
        return Err(Status::NOT_SUPPORTED);
    }

    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_EVENTPAIR))?;

    let (handle0, handle1, rights) = EventPairDispatcher::create()?;
    let user_handle0 = handle0.make_and_add_handle(rights)?;
    let user_handle1 = handle1.make_and_add_handle(rights)?;
    *out0 = user_handle0;
    *out1 = user_handle1;
    Ok(())
}
