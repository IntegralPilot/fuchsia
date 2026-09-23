// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{Dispatcher, FifoDispatcher, HandleValue, ProcessDispatcher};
use crate::user_copy::{UserInPtr, UserOutPtr};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_POL_NEW_FIFO, ZX_RIGHT_READ, ZX_RIGHT_WRITE};

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_fifo_create(
    elem_count: usize,
    elem_size: usize,
    options: u32,
    out0: &mut HandleValue,
    out1: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("elem_count {elem_count}, elem_size {elem_size}, options {options:#x}\n");

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_FIFO))?;

    let (handle0, handle1, rights) = FifoDispatcher::create(elem_count, elem_size)?;
    let user_handle0 = handle0.make_and_add_handle(rights)?;
    let user_handle1 = handle1.make_and_add_handle(rights)?;
    *out0 = user_handle0;
    *out1 = user_handle1;
    Ok(())
}

#[syscall]
pub fn sys_fifo_write(
    handle: HandleValue,
    elem_size: usize,
    data: UserInPtr<u8>,
    count: usize,
    actual_count: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {handle:?}, elem_size {elem_size}, count {count}\n");

    let fifo = Dispatcher::get_with_rights::<FifoDispatcher>(handle, ZX_RIGHT_WRITE)?;
    let mut actual = 0usize;
    fifo.write_from_user(elem_size, data, count, &mut actual)?;

    if !actual_count.is_null() {
        actual_count.write(actual)?;
    }
    Ok(())
}

#[syscall]
pub fn sys_fifo_read(
    handle: HandleValue,
    elem_size: usize,
    data: UserOutPtr<u8>,
    count: usize,
    actual_count: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {handle:?}, elem_size {elem_size}, count {count}\n");

    let fifo = Dispatcher::get_with_rights::<FifoDispatcher>(handle, ZX_RIGHT_READ)?;
    let mut actual = 0usize;
    fifo.read_to_user(elem_size, data, count, &mut actual)?;

    if !actual_count.is_null() {
        actual_count.write(actual)?;
    }
    Ok(())
}
