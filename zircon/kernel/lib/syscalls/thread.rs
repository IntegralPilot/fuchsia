// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{
    Dispatcher, HandleValue, ProcessDispatcher, ThreadDispatcher, VmObjectDispatcher,
};
use crate::user_copy::{UserInPtr, UserOutPtr};
use boot_options::BootOptions;
use core::mem::MaybeUninit;
use debug::{ltrace_entry, ltracef};
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_HANDLE_INVALID, ZX_MAX_NAME_LEN, ZX_RIGHT_DUPLICATE, ZX_RIGHT_MANAGE_THREAD, ZX_RIGHT_READ,
    ZX_RIGHT_WRITE, ZX_THREAD_STATE_DEBUG_REGS, zx_exception_context_t, zx_excp_type_t, zx_rseq_t,
    zx_status_t,
};

unsafe extern "C" {
    /// Resets the restartable sequence (rseq) registration for the calling thread.
    ///
    /// # Safety
    ///
    /// Must only be called in a valid thread context.
    fn cpp_thread_reset_rseq();

    /// Registers a restartable sequence (rseq) for the calling thread backed by `vmo_dispatcher`.
    ///
    /// # Safety
    ///
    /// `vmo_dispatcher` must be a valid reference to an initialized `VmObjectDispatcher`.
    fn cpp_thread_set_rseq(vmo_dispatcher: &VmObjectDispatcher, offset: u64) -> zx_status_t;
}

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_thread_create(
    process_handle: HandleValue,
    name_ptr: UserInPtr<u8>,
    name_len: usize,
    options: u32,
    out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("process handle {:#x}, options {:#x}\n", process_handle.raw_value(), options);

    // currently, the only valid option value is 0
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    if name_ptr.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    // copy out the name
    let mut name_buf = [MaybeUninit::<u8>::uninit(); ZX_MAX_NAME_LEN];
    let slice = name_ptr.copy_user_string(name_len, &mut name_buf)?;

    ltracef!("name {}\n", core::str::from_utf8(slice).unwrap_or("<non-utf8>"));

    // convert process handle to process dispatcher
    let process =
        Dispatcher::get_with_rights::<ProcessDispatcher>(process_handle, ZX_RIGHT_MANAGE_THREAD)?;
    // create the thread dispatcher
    let (handle, rights) = ThreadDispatcher::create(process, options, slice)?;
    handle.dispatcher().initialize()?;

    *out = ProcessDispatcher::with_current(|up| up.make_and_add_handle(handle, rights))?;
    Ok(())
}

#[syscall]
pub fn sys_thread_start_regs(
    handle: HandleValue,
    thread_entry: u64,
    stack: u64,
    arg1: u64,
    arg2: u64,
    tp: u64,
    abi_reg: u64,
) -> Result<(), Status> {
    ltracef!(
        "handle {:#x}, entry {:#x}, sp {:#x}, arg1 {:#x}, arg2 {:#x}\n",
        handle.raw_value(),
        thread_entry,
        stack,
        arg1,
        arg2
    );

    let thread = Dispatcher::get_with_rights::<ThreadDispatcher>(handle, ZX_RIGHT_MANAGE_THREAD)?;

    #[cfg(target_arch = "x86_64")]
    {
        // A noncanonical address cannot be written into the MSR.
        if !crate::arch_rs::x86::is_vaddr_canonical(tp) {
            return Err(Status::INVALID_ARGS);
        }
    }

    thread.start(thread_entry as usize, stack as usize, arg1, arg2, tp, abi_reg, false)?;
    Ok(())
}

#[syscall]
pub fn sys_thread_exit() {
    ltrace_entry!();
    ThreadDispatcher::exit_current();
}

#[syscall]
pub fn sys_thread_read_state(
    handle: HandleValue,
    kind: u32,
    buffer: UserOutPtr<u8>,
    buffer_size: usize,
) -> Result<(), Status> {
    ltracef!("handle {:#x}, kind {}\n", handle.raw_value(), kind);

    // TODO(https://fxbug.dev/42105831): debug rights
    let thread = Dispatcher::get_with_rights::<ThreadDispatcher>(handle, ZX_RIGHT_READ)?;
    thread.read_state(kind, buffer.reinterpret::<core::ffi::c_void>().as_ptr(), buffer_size)?;
    Ok(())
}

#[syscall]
pub fn sys_thread_write_state(
    handle: HandleValue,
    kind: u32,
    buffer: UserInPtr<u8>,
    buffer_size: usize,
) -> Result<(), Status> {
    ltracef!("handle {:#x}, kind {}\n", handle.raw_value(), kind);

    if (kind & ZX_THREAD_STATE_DEBUG_REGS) != 0 && !BootOptions::get().enable_debugging_syscalls {
        return Err(Status::NOT_SUPPORTED);
    }

    // TODO(https://fxbug.dev/42105831): debug rights
    let thread = Dispatcher::get_with_rights::<ThreadDispatcher>(handle, ZX_RIGHT_WRITE)?;
    thread.write_state(kind, buffer.reinterpret::<core::ffi::c_void>().as_ptr(), buffer_size)?;
    Ok(())
}

#[syscall]
pub fn sys_thread_raise_exception(
    options: u32,
    exception_type: zx_excp_type_t,
    user_context_ptr: UserInPtr<zx_exception_context_t>,
) -> Result<(), Status> {
    ltracef!("options {:#x}, exception type {:#x}\n", options, exception_type);

    if user_context_ptr.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let mut context = MaybeUninit::<zx_exception_context_t>::uninit();
    let context_ref =
        user_context_ptr.copy_from_user(&mut context).map_err(|_| Status::INVALID_ARGS)?;

    ThreadDispatcher::raise_user_exception(options, exception_type, context_ref)?;
    Ok(())
}

#[syscall]
pub fn sys_thread_set_rseq(vmo_handle: HandleValue, offset: u64, size: u64) -> Result<(), Status> {
    ltracef!("vmo handle {:#x}, offset {}, size {}\n", vmo_handle.raw_value(), offset, size);

    // Is this an "unregister" operation?
    if vmo_handle.raw_value() == ZX_HANDLE_INVALID {
        if offset != 0 || size != 0 {
            return Err(Status::INVALID_ARGS);
        }

        // SAFETY: Resets the restartable sequence configuration on the current thread context.
        unsafe { cpp_thread_reset_rseq() };
        return Ok(());
    }

    // It's a register operation.
    //
    // Validate arguments.
    if size != core::mem::size_of::<zx_rseq_t>() as u64 {
        return Err(Status::INVALID_ARGS);
    }
    if !offset.is_multiple_of(core::mem::align_of::<zx_rseq_t>() as u64) {
        return Err(Status::INVALID_ARGS);
    }

    // Get the VMO dispatcher.
    let vmo_dispatcher = Dispatcher::get_with_rights::<VmObjectDispatcher>(
        vmo_handle,
        ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_DUPLICATE,
    )?;

    // SAFETY: `vmo_dispatcher` is a valid `VmObjectDispatcher` reference.
    let status = unsafe { cpp_thread_set_rseq(&vmo_dispatcher, offset) };
    Status::ok(status)?;
    Ok(())
}

#[syscall]
pub fn sys_thread_legacy_yield(options: u32) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    ThreadDispatcher::legacy_yield(options)?;
    Ok(())
}
