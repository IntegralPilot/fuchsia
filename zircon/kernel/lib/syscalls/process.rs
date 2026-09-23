// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{Dispatcher, HandleValue, JobDispatcher, ProcessDispatcher, ThreadDispatcher};
use crate::user_copy::{UserInPtr, UserOutPtr};
use crate::userabi::VDso;
use boot_options::BootOptions;
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_HANDLE_INVALID, ZX_MAX_NAME_LEN, ZX_OBJ_TYPE_PROCESS, ZX_POL_NEW_PROCESS, ZX_PROCESS_SHARED,
    ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_MANAGE_PROCESS, ZX_RIGHT_READ, ZX_RIGHT_WRITE,
};

const LOCAL_TRACE: u32 = 0;
const MAX_DEBUG_READ_BLOCK: usize = 64 * 1024 * 1024;
const MAX_DEBUG_WRITE_BLOCK: usize = 64 * 1024 * 1024;

#[syscall]
pub fn sys_process_create(
    job_handle: HandleValue,
    name_ptr: UserInPtr<u8>,
    name_len: usize,
    options: u32,
    proc_handle: &mut HandleValue,
    vmar_handle: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("job handle {:#x}, options {:#x}\n", job_handle.raw_value(), options);

    // currently, the only valid option values are 0 or ZX_PROCESS_SHARED
    if options != 0 && options != ZX_PROCESS_SHARED {
        return Err(Status::INVALID_ARGS);
    }

    // We check the policy against the process calling zx_process_create, which
    // is the operative policy, rather than against |job_handle|. Access to
    // |job_handle| is controlled by the rights associated with the handle.
    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_PROCESS))?;

    // copy out the name
    let mut buf = [core::mem::MaybeUninit::<u8>::uninit(); ZX_MAX_NAME_LEN];
    let sp = name_ptr.copy_user_string(name_len, &mut buf)?;
    ltracef!("name {}\n", zr::from_utf8_lossy(sp));

    let job = Dispatcher::get_with_rights::<JobDispatcher>(job_handle, ZX_RIGHT_MANAGE_PROCESS)?;
    let (new_proc, proc_rights, new_vmar, vmar_rights) =
        ProcessDispatcher::create(job, sp, options)?;

    crate::ktrace_rs::kernel_object!(
        "kernel:meta",
        new_proc.dispatcher().get_koid(),
        ZX_OBJ_TYPE_PROCESS,
        sp,
        "job" => crate::ktrace_rs::Koid(new_proc.dispatcher().job().map(|j| j.get_koid()).unwrap_or(0)),
    );
    // GCC workaround - in the GCC config the ktrace_rs::kernel_object!() macro is disabled
    // so the parameters are not used and GCC complains that ZX_OBJ_TYPE_PROCESS is unused.
    let _ = ZX_OBJ_TYPE_PROCESS;

    let (p_handle, v_handle) = ProcessDispatcher::with_current(|up| -> Result<_, Status> {
        let p = up.make_and_add_handle(new_proc, proc_rights)?;
        let v = up.make_and_add_handle(new_vmar, vmar_rights)?;
        Ok((p, v))
    })?;

    *proc_handle = p_handle;
    *vmar_handle = v_handle;
    Ok(())
}

#[syscall]
pub fn sys_process_create_shared(
    shared_proc_handle: HandleValue,
    options: u32,
    name_ptr: UserInPtr<u8>,
    name_len: usize,
    proc_handle: &mut HandleValue,
    restricted_vmar_handle: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("shared_proc {:#x}, options {:#x}\n", shared_proc_handle.raw_value(), options);

    // currently, the only valid option value is 0
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // We check the policy against the process calling zx_process_create, which
    // is the operative policy.
    // TODO(https://fxbug.dev/42181309): Figure out which policy check makes sense here.
    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_PROCESS))?;

    // copy out the name
    let mut buf = [core::mem::MaybeUninit::<u8>::uninit(); ZX_MAX_NAME_LEN];
    let sp = name_ptr.copy_user_string(name_len, &mut buf)?;
    ltracef!("name {}\n", zr::from_utf8_lossy(sp));

    // create a new process dispatcher
    let shared_proc = Dispatcher::get_with_rights::<ProcessDispatcher>(
        shared_proc_handle,
        ZX_RIGHT_MANAGE_PROCESS | ZX_RIGHT_GET_PROPERTY,
    )?;

    let (new_proc, proc_rights, new_vmar, vmar_rights) =
        ProcessDispatcher::create_shared(shared_proc, sp, options)?;

    crate::ktrace_rs::kernel_object!(
        "kernel:meta",
        new_proc.dispatcher().get_koid(),
        ZX_OBJ_TYPE_PROCESS,
        sp,
        "job" => crate::ktrace_rs::Koid(new_proc.dispatcher().job().map(|j| j.get_koid()).unwrap_or(0)),
    );

    let (p_handle, v_handle) = ProcessDispatcher::with_current(|up| -> Result<_, Status> {
        let p = up.make_and_add_handle(new_proc, proc_rights)?;
        let v = up.make_and_add_handle(new_vmar, vmar_rights)?;
        Ok((p, v))
    })?;

    *proc_handle = p_handle;
    *restricted_vmar_handle = v_handle;
    Ok(())
}

// Note: This is used to start the main thread (as opposed to using
// sys_thread_start for that) for a few reasons:
// - less easily exploitable
//   We want to make sure we can't generically transfer handles to a process.
//   This has the nice property of restricting the evil (transferring handle
//   to new process) to exactly one spot, and can be called exactly once per
//   process, since it also pushes it into a new state.
// - maintains the state machine invariant that 'started' processes have one
//   thread running
#[syscall]
pub fn sys_process_start(
    process_handle: HandleValue,
    thread_handle: HandleValue,
    entry: usize,
    stack: usize,
    arg1_handle: HandleValue,
    arg2: usize,
) -> Result<(), Status> {
    ltracef!(
        "phandle {:#x}, thandle {:#x}, entry {:#x}, stack {:#x}, arg1 {:#x}, arg2 {:#x}\n",
        process_handle.raw_value(),
        thread_handle.raw_value(),
        entry,
        stack,
        arg1_handle.raw_value(),
        arg2
    );

    let process =
        match Dispatcher::get_with_rights::<ProcessDispatcher>(process_handle, ZX_RIGHT_WRITE) {
            Ok(proc) => proc,
            Err(err) => {
                if arg1_handle.raw_value() != ZX_HANDLE_INVALID {
                    let _ = ProcessDispatcher::with_current(|up| up.remove_handle(arg1_handle));
                }
                return Err(err);
            }
        };

    let thread =
        match Dispatcher::get_with_rights::<ThreadDispatcher>(thread_handle, ZX_RIGHT_WRITE) {
            Ok(t) => t,
            Err(err) => {
                if arg1_handle.raw_value() != ZX_HANDLE_INVALID {
                    let _ = ProcessDispatcher::with_current(|up| up.remove_handle(arg1_handle));
                }
                return Err(err);
            }
        };

    let arg_handle = if arg1_handle.raw_value() != ZX_HANDLE_INVALID {
        ProcessDispatcher::with_current(|up| up.remove_handle(arg1_handle))
    } else {
        None
    };

    process.start(thread, entry, stack, arg_handle, arg2)?;
    Ok(())
}

#[syscall]
pub fn sys_process_exit(retcode: i64) {
    ltracef!("retcode {}\n", retcode);
    ProcessDispatcher::exit_current(retcode)
}

#[syscall]
pub fn sys_process_read_memory(
    handle: HandleValue,
    vaddr: usize,
    buffer: UserOutPtr<u8>,
    buffer_size: usize,
    actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("vaddr {:#x}, size {}\n", vaddr, buffer_size);

    if buffer.is_null() || buffer_size == 0 || buffer_size > MAX_DEBUG_READ_BLOCK {
        return Err(Status::INVALID_ARGS);
    }

    let process =
        Dispatcher::get_with_rights::<ProcessDispatcher>(handle, ZX_RIGHT_READ | ZX_RIGHT_WRITE)?;

    let aspace = process.aspace_at(vaddr).ok_or(Status::BAD_STATE)?;

    let vm_mapping = aspace.find_mapping(vaddr).ok_or(Status::NOT_FOUND)?;

    let vmo = vm_mapping.vmo().ok_or(Status::NOT_FOUND)?;

    let offset = (vaddr - vm_mapping.base()) as u64 + vm_mapping.object_offset();
    // TODO(https://fxbug.dev/42106495): While this limits reading to the mapped address space of
    // this VMO, it should be reading from multiple VMOs, not a single one.
    // Additionally, it is racy with the mapping going away.
    let buffer_size = core::cmp::min(buffer_size, vm_mapping.size() - (vaddr - vm_mapping.base()));

    let out_actual = vmo.read_user(buffer, offset, buffer_size)?;
    if out_actual == 0 {
        // If our partial read returned 0 bytes, it means that offset is past the end of the VMO.
        return Err(Status::OUT_OF_RANGE);
    }

    // Do not write |out_actual| to |actual| on error
    actual.copy_to_user(&out_actual)?;
    Ok(())
}

#[syscall]
pub fn sys_process_write_memory(
    handle: HandleValue,
    vaddr: usize,
    buffer: UserInPtr<u8>,
    buffer_size: usize,
    actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("vaddr {:#x}, size {}\n", vaddr, buffer_size);

    if !BootOptions::get().enable_debugging_syscalls {
        return Err(Status::NOT_SUPPORTED);
    }

    if buffer.is_null() || buffer_size == 0 || buffer_size > MAX_DEBUG_WRITE_BLOCK {
        return Err(Status::INVALID_ARGS);
    }

    let process = Dispatcher::get_with_rights::<ProcessDispatcher>(handle, ZX_RIGHT_WRITE)?;

    let aspace = process.aspace_at(vaddr).ok_or(Status::BAD_STATE)?;

    let vm_mapping = aspace.find_mapping(vaddr).ok_or(Status::NOT_FOUND)?;

    // TODO(https://fxbug.dev/42106188): Inform the mapping that we are going to ignore its
    // permissions and perform a write. This provides it the chance to ensure our writes go to a
    // mapping local clone to avoid corrupting a potentially read-only VMO.
    let vm_mapping = vm_mapping.force_writable()?;

    let vmo = vm_mapping.vmo().ok_or(Status::NOT_FOUND)?;

    if VDso::vmo_is_vdso(&vmo) {
        // Don't allow writes to the vDSO.
        return Err(Status::ACCESS_DENIED);
    }

    let offset = (vaddr - vm_mapping.base()) as u64 + vm_mapping.object_offset();
    // TODO(https://fxbug.dev/42106495): While this limits writing to the mapped address space of
    // this VMO, it should be reading from multiple VMOs, not a single one.
    // Additionally, it is racy with the mapping going away.
    let buffer_size = core::cmp::min(buffer_size, vm_mapping.size() - (vaddr - vm_mapping.base()));

    let out_actual = vmo.write_user(buffer, offset, buffer_size)?;
    if out_actual == 0 {
        // If our partial write returned 0 bytes, it means that offset is past the end of the VMO.
        return Err(Status::OUT_OF_RANGE);
    }

    // Do not write |out_actual| to |actual| on error
    actual.copy_to_user(&out_actual)?;
    Ok(())
}
