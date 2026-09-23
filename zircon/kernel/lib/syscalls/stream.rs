// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{Dispatcher, HandleValue, StreamDispatcher, VmObjectDispatcher};
use crate::user_copy::{UserInPtr, UserOutPtr, make_user_in_iovec, make_user_out_iovec};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_RIGHT_READ, ZX_RIGHT_RESIZE, ZX_RIGHT_WRITE, ZX_STREAM_APPEND, ZX_STREAM_CREATE_MASK,
    zx_iovec_t, zx_off_t, zx_stream_seek_origin_t,
};

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_stream_create(
    options: u32,
    vmo_handle: HandleValue,
    seek: zx_off_t,
    out_stream: &mut HandleValue,
) -> Result<(), Status> {
    if (options & !ZX_STREAM_CREATE_MASK) != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let (mut stream_options, desired_vmo_rights) =
        StreamDispatcher::parse_create_syscall_flags(options)?;

    let (vmo_disp, actual_vmo_rights) = Dispatcher::get_with_rights_and_actual::<VmObjectDispatcher>(
        vmo_handle,
        desired_vmo_rights,
    )?;

    // Cannot create a stream from a physical or contiguous VMO.
    if !vmo_disp.vmo().is_paged() || vmo_disp.vmo().is_contiguous() {
        return Err(Status::WRONG_TYPE);
    }

    // Remember whether this stream can resize the underlying VMO when required. Note that it
    // might be possible for stream writes / appends to proceed by manipulating only the stream
    // size, without having to change the VMO size. This flag will be checked only for stream
    // operations that would require manipulating the VMO size, more specifically when expanding
    // the VMO size if the requested stream size needs to surpass the current VMO size.
    if (actual_vmo_rights & ZX_RIGHT_RESIZE) != 0 {
        stream_options.set_can_resize_vmo(true);
    }

    let (kernel_handle, rights) = StreamDispatcher::create(stream_options, &vmo_disp, seek)?;
    let user_handle = kernel_handle.make_and_add_handle(rights)?;
    *out_stream = user_handle;
    Ok(())
}

#[syscall]
pub fn sys_stream_writev(
    handle: HandleValue,
    options: u32,
    vector: UserInPtr<zx_iovec_t>,
    vector_count: usize,
    out_actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    if (options & !ZX_STREAM_APPEND) != 0 {
        return Err(Status::INVALID_ARGS);
    }
    if vector.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let stream = Dispatcher::get_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_WRITE)?;
    let user_data = make_user_in_iovec(vector, vector_count);
    let actual = if (options & ZX_STREAM_APPEND) != 0 {
        stream.append_vector(user_data)?
    } else {
        stream.write_vector(user_data)?
    };

    if !out_actual.is_null() {
        out_actual.write(actual)?;
    }
    Ok(())
}

#[syscall]
pub fn sys_stream_writev_at(
    handle: HandleValue,
    options: u32,
    offset: zx_off_t,
    vector: UserInPtr<zx_iovec_t>,
    vector_count: usize,
    out_actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }
    if vector.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let stream = Dispatcher::get_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_WRITE)?;
    let actual = stream.write_vector_at(make_user_in_iovec(vector, vector_count), offset)?;

    if !out_actual.is_null() {
        out_actual.write(actual)?;
    }
    Ok(())
}

#[syscall]
pub fn sys_stream_readv(
    handle: HandleValue,
    options: u32,
    vector: UserOutPtr<zx_iovec_t>,
    vector_count: usize,
    out_actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }
    if vector.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let stream = Dispatcher::get_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_READ)?;
    let actual = stream.read_vector(make_user_out_iovec(vector, vector_count))?;

    if !out_actual.is_null() {
        out_actual.write(actual)?;
    }
    Ok(())
}

#[syscall]
pub fn sys_stream_readv_at(
    handle: HandleValue,
    options: u32,
    offset: zx_off_t,
    vector: UserOutPtr<zx_iovec_t>,
    vector_count: usize,
    out_actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }
    if vector.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let stream = Dispatcher::get_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_READ)?;
    let actual = stream.read_vector_at(make_user_out_iovec(vector, vector_count), offset)?;

    if !out_actual.is_null() {
        out_actual.write(actual)?;
    }
    Ok(())
}

#[syscall]
pub fn sys_stream_seek(
    handle: HandleValue,
    whence: zx_stream_seek_origin_t,
    offset: i64,
    out_seek: UserOutPtr<zx_off_t>,
) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    let (stream, rights) = Dispatcher::get_and_rights::<StreamDispatcher>(handle)?;
    if (rights & (ZX_RIGHT_READ | ZX_RIGHT_WRITE)) == 0 {
        return Err(Status::ACCESS_DENIED);
    }

    let seek = stream.seek(whence, offset)?;
    if !out_seek.is_null() {
        out_seek.write(seek)?;
    }
    Ok(())
}
