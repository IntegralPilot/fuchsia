// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{
    Dispatcher, Disposition, HandleValue, ProcessDispatcher, ReadType, SocketDispatcher,
};
use crate::user_copy::{UserInPtr, UserOutPtr};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_POL_NEW_SOCKET, ZX_RIGHT_MANAGE_SOCKET, ZX_RIGHT_READ, ZX_RIGHT_WRITE};

pub const ZX_SOCKET_PEEK: u32 = 1 << 3;

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_socket_create(
    options: u32,
    out0: &mut HandleValue,
    out1: &mut HandleValue,
) -> Result<(), Status> {
    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_SOCKET))?;

    let (handle0, handle1, rights) = SocketDispatcher::create(options)?;
    let user_handle0 = handle0.make_and_add_handle(rights)?;
    let user_handle1 = handle1.make_and_add_handle(rights)?;
    *out0 = user_handle0;
    *out1 = user_handle1;
    Ok(())
}

#[syscall]
pub fn sys_socket_write(
    handle: HandleValue,
    options: u32,
    buffer: UserInPtr<u8>,
    size: usize,
    actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {handle:?}\n");

    if size > 0 && buffer.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let socket = Dispatcher::get_with_rights::<SocketDispatcher>(handle, ZX_RIGHT_WRITE)?;
    let nwritten = socket.write(buffer.reinterpret(), size)?;

    // Caller may ignore results if desired.
    if !actual.is_null() {
        actual.write(nwritten)?;
    }
    Ok(())
}

#[syscall]
pub fn sys_socket_read(
    handle: HandleValue,
    options: u32,
    buffer: UserOutPtr<u8>,
    size: usize,
    actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {handle:?}\n");

    if buffer.is_null() && size > 0 {
        return Err(Status::INVALID_ARGS);
    }
    if options & !ZX_SOCKET_PEEK != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let socket = Dispatcher::get_with_rights::<SocketDispatcher>(handle, ZX_RIGHT_READ)?;
    let read_type =
        if (options & ZX_SOCKET_PEEK) != 0 { ReadType::Peek } else { ReadType::Consume };

    let nread = socket.read(read_type, buffer.reinterpret(), size)?;

    // Caller may ignore results if desired.
    if !actual.is_null() {
        actual.write(nread)?;
    }
    Ok(())
}

#[syscall]
pub fn sys_socket_set_disposition(
    handle: HandleValue,
    disposition: u32,
    disposition_peer: u32,
) -> Result<(), Status> {
    let disp = Disposition::try_from(disposition)?;
    let disp_peer = Disposition::try_from(disposition_peer)?;

    let socket = Dispatcher::get_with_rights::<SocketDispatcher>(handle, ZX_RIGHT_MANAGE_SOCKET)?;
    socket.set_disposition(disp, disp_peer)?;
    Ok(())
}
