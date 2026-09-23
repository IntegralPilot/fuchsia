// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::restricted::{restricted_enter, thread_current_set_restricted_state};
use crate::kernel::restricted_state::RestrictedState;
use crate::object::{
    Dispatcher, HandleValue, InitialMutability, ProcessDispatcher, ThreadDispatcher,
    VmObjectDispatcher,
};
use crate::user_copy::UserOutPtr;
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_POL_NEW_VMO, ZX_RIGHT_MANAGE_THREAD, zx_exception_report_t};

// Disable local tracing by default for this file.
const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_restricted_enter(
    options: u32,
    vector_table_ptr: usize,
    context: usize,
) -> Result<(), Status> {
    ltracef!("options {:#x} vector {:#x} context {:#x}\n", options, vector_table_ptr, context);

    // Reject invalid option bits.
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }
    restricted_enter(vector_table_ptr, context)?;
    Ok(())
}

#[syscall]
pub fn sys_restricted_bind_state(
    options: u32,
    out: &mut HandleValue,
    out_exception: UserOutPtr<zx_exception_report_t>,
) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    // No options allowed.
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // Are we allowed to create a VMO?
    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_VMO))?;

    // Check if user provided an exception report pointer.
    let exception_ptr =
        if !out_exception.is_null() { out_exception.as_ptr() } else { core::ptr::null_mut() };

    // Create the restricted state.
    let rs = RestrictedState::create_from_raw(exception_ptr)?;

    // Wrap the VMO in a VmObjectDispatcher so we can give a handle back to the user.
    let vmo = rs.vmo();
    let size = vmo.size();
    let (kernel_handle, rights) =
        VmObjectDispatcher::create(vmo, size, InitialMutability::Mutable)?;

    // Wrap the VmObjectDispatcher in a Handle in the process's handle table.
    let user_handle = kernel_handle.make_and_add_handle(rights)?;

    // Finally, set this thread's restricted state. Note, it's possible the copy-out of the new
    // handle will fail, but that's OK. If that happens a ZX_EXCP_POLICY_CODE_HANDLE_LEAK will
    // be generated, at which point the caller will either be terminated or will need to handle
    // the exception (likely by retrying the operation with a valid out buffer).
    thread_current_set_restricted_state(Some(rs));

    *out = user_handle;
    Ok(())
}

#[syscall]
pub fn sys_restricted_unbind_state(options: u32) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    // No options allowed.
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    thread_current_set_restricted_state(None);
    Ok(())
}

#[syscall]
pub fn sys_restricted_kick(handle: HandleValue, options: u32) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    // No options allowed.
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // TODO(https://fxbug.dev/42077353): Decide if this is the correct right for this operation.
    let thread = Dispatcher::get_with_rights::<ThreadDispatcher>(handle, ZX_RIGHT_MANAGE_THREAD)?;
    thread.restricted_kick()?;
    Ok(())
}
