// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::c_char;

use boot_options::BootOptions;
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_RSRC_SYSTEM_DEBUG_BASE, ZX_RSRC_SYSTEM_TRACING_BASE};

use crate::ktrace_rs::KTrace;
use crate::object::{HandleValue, validate_system_resource};
use crate::platform_rs::debug::platform_dgetc;
use crate::user_copy::{UserInOutPtr, UserInPtr, UserOutPtr};

const LOCAL_TRACE: u32 = 0;

/// Maximum number of bytes that can be written in a single `zx_debug_write`
/// or `zx_debug_send_command` syscall.
pub const MAX_DEBUG_WRITE_SIZE: usize = 256;

#[syscall]
pub fn sys_debug_read(
    handle: HandleValue,
    ptr: UserOutPtr<u8>,
    max_len: usize,
    len: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("ptr {:p}\n", ptr.as_ptr());

    if BootOptions::get().enable_serial_syscalls != boot_options::SerialDebugSyscalls::Enabled {
        return Err(Status::NOT_SUPPORTED);
    }

    validate_system_resource(handle, ZX_RSRC_SYSTEM_DEBUG_BASE)?;

    let mut idx = 0;
    while idx < max_len {
        let mut c: c_char = 0;
        // Wait only on the first character.
        // The API for this function can return any number of characters up to the supplied buffer
        // length, however there is no notification mechanism for when there are bytes to read.
        // Hence, we need to read at least one character or applications will be forced to spin poll.
        // We avoid reading all the characters so that interactive applications can stay responsive
        // without losing efficiency by being forced to read one character at a time.
        let wait = idx == 0;
        let err = platform_dgetc(&mut c, wait);
        if err < 0 {
            Status::ok(err)?;
        } else if err == 0 {
            break;
        }

        let mut byte = c as u8;
        if byte == b'\r' {
            byte = b'\n';
        }

        ptr.element_offset(idx).copy_to_user(&byte)?;
        idx += 1;
    }
    len.copy_to_user(&idx)?;
    Ok(())
}

#[syscall]
pub fn sys_debug_write(ptr: UserInPtr<u8>, mut len: usize) -> Result<(), Status> {
    ltracef!("ptr {:p}, len {}\n", ptr.as_ptr(), len);

    let enable_serial = BootOptions::get().enable_serial_syscalls;
    if enable_serial != boot_options::SerialDebugSyscalls::Enabled
        && enable_serial != boot_options::SerialDebugSyscalls::OutputOnly
    {
        return Err(Status::NOT_SUPPORTED);
    }

    if len > MAX_DEBUG_WRITE_SIZE {
        len = MAX_DEBUG_WRITE_SIZE;
    }

    let mut buf = [core::mem::MaybeUninit::<u8>::uninit(); MAX_DEBUG_WRITE_SIZE];
    let slice = ptr.copy_slice_from_user(&mut buf[..len]).map_err(|_| Status::INVALID_ARGS)?;

    // Dump what we can into the persistent dlog, if we have one.
    crate::persistent_debuglog_rs::persistent_dlog_write(slice);

    // This path to serial out arbitrates with the debug log
    // drainer and/or kernel ll debug path to minimize interleaving
    // of serial output between various sources.
    crate::debuglog_rs::dlog_serial_write(slice);

    Ok(())
}

#[syscall]
pub fn sys_debug_send_command(
    resource: HandleValue,
    ptr: UserInPtr<u8>,
    len: usize,
) -> Result<(), Status> {
    ltracef!("ptr {:p}, len {}\n", ptr.as_ptr(), len);

    if !BootOptions::get().enable_debugging_syscalls {
        return Err(Status::NOT_SUPPORTED);
    }

    validate_system_resource(resource, ZX_RSRC_SYSTEM_DEBUG_BASE)?;

    if len > MAX_DEBUG_WRITE_SIZE {
        return Err(Status::INVALID_ARGS);
    }

    let mut buf = [core::mem::MaybeUninit::<u8>::uninit(); MAX_DEBUG_WRITE_SIZE];
    let slice = ptr.copy_slice_from_user(&mut buf[..len]).map_err(|_| Status::INVALID_ARGS)?;
    let cmd_str = core::str::from_utf8(slice).map_err(|_| Status::INVALID_ARGS)?;

    let status = crate::console_rust::console::console_run_script(cmd_str);
    Status::ok(status)?;
    Ok(())
}

#[syscall]
pub fn sys_ktrace_read(
    handle: HandleValue,
    data: UserOutPtr<u8>,
    offset: u32,
    len: usize,
    out_actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    validate_system_resource(handle, ZX_RSRC_SYSTEM_TRACING_BASE)?;

    let actual = KTrace::get_instance().read_user(data, offset, len)?;
    out_actual.copy_to_user(&actual)?;
    Ok(())
}

#[syscall]
pub fn sys_ktrace_control(
    handle: HandleValue,
    action: u32,
    options: u32,
    _ptr: UserInOutPtr<u8>,
) -> Result<(), Status> {
    validate_system_resource(handle, ZX_RSRC_SYSTEM_TRACING_BASE)?;

    KTrace::get_instance().control(action, options)
}
