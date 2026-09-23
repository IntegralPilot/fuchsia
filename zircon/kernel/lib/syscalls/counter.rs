// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{CounterDispatcher, Dispatcher, HandleValue};
use crate::user_copy::UserOutPtr;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_RIGHT_READ, ZX_RIGHT_WRITE};

#[syscall]
pub fn sys_counter_create(options: u32, out: &mut HandleValue) -> Result<(), Status> {
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // TODO(https://fxbug.dev/387324141): Add/enforce ZX_POL_NEW_COUNTER policy.
    let (kernel_handle, rights) = CounterDispatcher::create()?;
    let user_handle = kernel_handle.make_and_add_handle(rights)?;
    *out = user_handle;
    Ok(())
}

#[syscall]
pub fn sys_counter_add(handle: HandleValue, value: i64) -> Result<(), Status> {
    // Both read and write rights are required for add because the resulting signal state and error
    // code can be used to determine the counter's value.
    let counter =
        Dispatcher::get_with_rights::<CounterDispatcher>(handle, ZX_RIGHT_READ | ZX_RIGHT_WRITE)?;

    counter.add(value)?;
    Ok(())
}

#[syscall]
pub fn sys_counter_read(handle: HandleValue, value_out: UserOutPtr<i64>) -> Result<(), Status> {
    if value_out.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let counter = Dispatcher::get_with_rights::<CounterDispatcher>(handle, ZX_RIGHT_READ)?;
    let value = counter.value();
    value_out.write(value)?;
    Ok(())
}

#[syscall]
pub fn sys_counter_write(handle: HandleValue, value: i64) -> Result<(), Status> {
    let counter = Dispatcher::get_with_rights::<CounterDispatcher>(handle, ZX_RIGHT_WRITE)?;
    counter.set_value(value);
    Ok(())
}
