// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::debuglog_rs::{DLOG_MAX_DATA, ZX_LOG_FLAGS_MASK, dlog_record_t};
use crate::object::{Dispatcher, HandleValue, LogDispatcher, validate_resource_kind_base};
use crate::user_copy::{UserInPtr, UserOutPtr};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    DEBUGLOG_INFO, ZX_HANDLE_INVALID, ZX_LOG_FLAG_READABLE, ZX_RIGHT_READ, ZX_RIGHT_WRITE,
    ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_DEBUGLOG_BASE, zx_log_record_header_t,
};

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_debuglog_create(
    rsrc: HandleValue,
    options: u32,
    out_handle: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    // To support allowing the libc dynamic linker to emit log messages even
    // before process bootstrap is complete, we allow creating a debuglog with
    // options == 0 (write-only) without yet having a valid `rsrc` handle.
    // Otherwise, we should require a valid `rsrc` handle.
    if rsrc.raw_value() != ZX_HANDLE_INVALID {
        validate_resource_kind_base(rsrc, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_DEBUGLOG_BASE)?;
    } else if options != 0 {
        return Err(Status::BAD_HANDLE);
    }

    // Ensure only valid options were provided. Currently only ZX_LOG_FLAG_READABLE.
    if (options & ZX_LOG_FLAG_READABLE) != options {
        return Err(Status::INVALID_ARGS);
    }

    let (kernel_handle, rights) = LogDispatcher::create(options)?;
    *out_handle = kernel_handle.make_and_add_handle(rights)?;
    Ok(())
}

#[syscall]
pub fn sys_debuglog_write(
    log_handle: HandleValue,
    options: u32,
    ptr: UserInPtr<u8>,
    len: usize,
) -> Result<(), Status> {
    ltracef!("handle {:#x}, options {:#x}, len {}\n", log_handle.raw_value(), options, len);

    let len = core::cmp::min(len, DLOG_MAX_DATA);

    if (options & !ZX_LOG_FLAGS_MASK) != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let log = Dispatcher::get_with_rights::<LogDispatcher>(log_handle, ZX_RIGHT_WRITE)?;

    let mut buf = [core::mem::MaybeUninit::<u8>::uninit(); DLOG_MAX_DATA];
    let slice = ptr.copy_slice_from_user(&mut buf[..len])?;

    log.write(DEBUGLOG_INFO as u32, options, slice)?;
    Ok(())
}

// Converts a dlog_record_t into a zx_log_record_t and copies it out to user memory.
//
// Copies at most |len| bytes to |dst|.
fn copy_out_log_record(
    internal_record: &dlog_record_t,
    dst: UserOutPtr<u8>,
    mut len: usize,
) -> Result<usize, Status> {
    use zerocopy::IntoBytes;

    let mut external_record = zx_log_record_header_t::default();
    external_record.sequence = internal_record.hdr.sequence;
    external_record.datalen = internal_record.hdr.datalen;
    external_record.severity = internal_record.hdr.severity;
    external_record.flags = internal_record.hdr.flags;
    external_record.timestamp = internal_record.hdr.timestamp;
    external_record.pid = internal_record.hdr.pid;
    external_record.tid = internal_record.hdr.tid;

    let record_bytes = external_record.as_bytes();

    // The user's buffer may not be large enough to hold the zx_log_record_t let
    // alone the flexible array member that follows.
    if len < record_bytes.len() {
        // Not enough space to copy the whole struct so we must treat it as an array
        // of bytes instead.
        let to_copy = core::cmp::min(len, record_bytes.len());
        dst.copy_slice_to_user(&record_bytes[..to_copy])?;
        return Ok(to_copy);
    }

    // There's enough space for the struct so copy it as is.
    dst.copy_slice_to_user(record_bytes)?;

    let mut amount_copied = record_bytes.len();
    len -= amount_copied;

    // Copy out as much of the data as will fit.
    let to_copy = core::cmp::min(len, internal_record.hdr.datalen as usize);
    if to_copy > 0 {
        let data_bytes = internal_record.data[..to_copy].as_bytes();
        dst.byte_offset(amount_copied as isize).copy_slice_to_user(data_bytes)?;
        amount_copied += to_copy;
    }

    Ok(amount_copied)
}

#[syscall]
pub fn sys_debuglog_read(
    log_handle: HandleValue,
    options: u32,
    ptr: UserOutPtr<u8>,
    len: usize,
) -> Result<(), Status> {
    ltracef!("handle {:#x}, options {:#x}, len {}\n", log_handle.raw_value(), options, len);

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let log = Dispatcher::get_with_rights::<LogDispatcher>(log_handle, ZX_RIGHT_READ)?;

    let mut record = dlog_record_t::default();
    let mut actual = 0;
    log.read(options, &mut record, &mut actual)?;

    let copied = copy_out_log_record(&record, ptr, len)?;
    Status::ok(copied as zx_types::zx_status_t)
}
