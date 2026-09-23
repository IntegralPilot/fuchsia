// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Implementation of the `zx_object_get_property` and `zx_object_set_property` syscalls in Rust.
//!
//! Handles property queries and modifications for dispatcher types ported to Rust (e.g. `Dispatcher`,
//! `ProcessDispatcher`, `SocketDispatcher`, `JobDispatcher`), delegating unported or subsystem-specific
//! property queries to C++ FFI helpers.

use crate::object::{Dispatcher, HandleValue, JobDispatcher, ProcessDispatcher, SocketDispatcher};
use crate::user_copy::{UserInPtr, UserOutPtr};
use boot_options::BootOptions;
use core::mem::MaybeUninit;
use syscalls_macro::syscall;
use zerocopy::{FromBytes, Immutable, IntoBytes};
use zx_status::Status;
use zx_types::{
    ZX_MAX_NAME_LEN, ZX_PROP_JOB_KILL_ON_OOM, ZX_PROP_NAME, ZX_PROP_PROCESS_BREAK_ON_LOAD,
    ZX_PROP_PROCESS_DEBUG_ADDR, ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID,
    ZX_PROP_PROCESS_VDSO_BASE_ADDRESS, ZX_PROP_SOCKET_RX_THRESHOLD, ZX_PROP_SOCKET_TX_THRESHOLD,
    ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_SET_PROPERTY, zx_rights_t, zx_status_t,
};

const LOCAL_TRACE: u32 = 0;

#[cfg(target_arch = "x86_64")]
use crate::arch_rs::x86::registers::{X86_MSR_IA32_FS_BASE, X86_MSR_IA32_KERNEL_GS_BASE};

#[cfg(target_arch = "x86_64")]
use zx_types::{ZX_PROP_REGISTER_FS, ZX_PROP_REGISTER_GS};

/// Copies a scalar value `val` of type `T` into the user buffer `value`.
///
/// # Errors
///
/// * [`Status::BUFFER_TOO_SMALL`]: If `size < core::mem::size_of::<T>()`.
fn copy_scalar_to_user<T: IntoBytes + Immutable>(
    value: UserOutPtr<u8>,
    size: usize,
    val: T,
) -> Result<(), Status> {
    if size < core::mem::size_of::<T>() {
        return Err(Status::BUFFER_TOO_SMALL);
    }
    value.reinterpret::<T>().write(val)
}

/// Copies a scalar value of type `T` from the user buffer `value`.
///
/// # Errors
///
/// * [`Status::BUFFER_TOO_SMALL`]: If `size < core::mem::size_of::<T>()`.
fn copy_scalar_from_user<T: FromBytes>(value: UserInPtr<u8>, size: usize) -> Result<T, Status> {
    if size < core::mem::size_of::<T>() {
        return Err(Status::BUFFER_TOO_SMALL);
    }
    value.reinterpret::<T>().read()
}

/// Validates that the given dispatcher refers to the currently executing thread.
///
/// On x86_64, reading or writing FS/GS base registers via `ZX_PROP_REGISTER_FS` and
/// `ZX_PROP_REGISTER_GS` is restricted to the caller's own current thread.
#[cfg(target_arch = "x86_64")]
fn require_current_thread(dispatcher: &Dispatcher) -> Result<(), Status> {
    let thread =
        dispatcher.downcast::<crate::object::ThreadDispatcher>().ok_or(Status::WRONG_TYPE)?;
    if !thread.is_current() {
        return Err(Status::ACCESS_DENIED);
    }
    Ok(())
}

unsafe extern "C" {
    /// C++ fallback for getting properties on dispatcher types not yet ported to Rust.
    fn cpp_object_get_property_cpp_types(
        dispatcher: *const Dispatcher,
        property: u32,
        value: *mut core::ffi::c_void,
        size: usize,
    ) -> zx_status_t;

    /// C++ fallback for setting properties on dispatcher types not yet ported to Rust.
    fn cpp_object_set_property_cpp_types(
        dispatcher: *const Dispatcher,
        property: u32,
        value: *const core::ffi::c_void,
        size: usize,
        rights: zx_rights_t,
    ) -> zx_status_t;
}

#[syscall]
pub fn sys_object_get_property(
    handle_value: HandleValue,
    property: u32,
    value: UserOutPtr<u8>,
    size: usize,
) -> Result<(), Status> {
    if value.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let dispatcher =
        Dispatcher::get_with_rights::<Dispatcher>(handle_value, ZX_RIGHT_GET_PROPERTY)?;

    macro_rules! get_scalar_property {
        ($type:ty, $getter:ident) => {{
            let obj = dispatcher.downcast::<$type>().ok_or(Status::WRONG_TYPE)?;
            copy_scalar_to_user(value, size, obj.$getter())
        }};
    }

    match property {
        ZX_PROP_NAME => {
            if size < ZX_MAX_NAME_LEN {
                return Err(Status::BUFFER_TOO_SMALL);
            }
            let mut name = [0u8; ZX_MAX_NAME_LEN];
            dispatcher.get_name(&mut name)?;
            value.copy_slice_to_user(&name)
        }
        ZX_PROP_PROCESS_DEBUG_ADDR => get_scalar_property!(ProcessDispatcher, get_debug_addr),
        ZX_PROP_PROCESS_BREAK_ON_LOAD => {
            get_scalar_property!(ProcessDispatcher, get_dyn_break_on_load)
        }
        ZX_PROP_PROCESS_VDSO_BASE_ADDRESS => {
            get_scalar_property!(ProcessDispatcher, vdso_base_address)
        }
        ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID => {
            if !BootOptions::get().enable_debugging_syscalls {
                return Err(Status::NOT_SUPPORTED);
            }
            #[cfg(target_arch = "x86_64")]
            {
                get_scalar_property!(ProcessDispatcher, hw_trace_context_id)
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                Err(Status::NOT_SUPPORTED)
            }
        }
        ZX_PROP_SOCKET_RX_THRESHOLD => get_scalar_property!(SocketDispatcher, get_read_threshold),
        ZX_PROP_SOCKET_TX_THRESHOLD => get_scalar_property!(SocketDispatcher, get_write_threshold),
        #[cfg(target_arch = "x86_64")]
        ZX_PROP_REGISTER_FS | ZX_PROP_REGISTER_GS => {
            require_current_thread(&dispatcher)?;
            // SAFETY: Reading valid Model Specific Registers on x86_64 hardware.
            let val = if property == ZX_PROP_REGISTER_FS {
                unsafe { crate::arch_rs::x86::x86::read_msr(X86_MSR_IA32_FS_BASE) }
            } else {
                unsafe { crate::arch_rs::x86::x86::read_msr(X86_MSR_IA32_KERNEL_GS_BASE) }
            };
            copy_scalar_to_user(value, size, val as usize)
        }
        // C++-only dispatchers (e.g. ExceptionDispatcher, StreamDispatcher, VmObjectDispatcher)
        _ => {
            // SAFETY: Call C++ FFI helper for properties on C++ dispatchers.
            let status = unsafe {
                cpp_object_get_property_cpp_types(
                    &*dispatcher,
                    property,
                    value.as_ptr() as *mut core::ffi::c_void,
                    size,
                )
            };
            Status::ok(status)
        }
    }
}

#[syscall]
pub fn sys_object_set_property(
    handle_value: HandleValue,
    property: u32,
    value: UserInPtr<u8>,
    size: usize,
) -> Result<(), Status> {
    if value.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let (dispatcher, rights) = Dispatcher::get_dispatcher_and_rights(handle_value)?;
    if (rights & ZX_RIGHT_SET_PROPERTY) == 0 {
        return Err(Status::ACCESS_DENIED);
    }

    macro_rules! set_scalar_property {
        ($type:ty, $setter:ident) => {{
            let obj = dispatcher.downcast::<$type>().ok_or(Status::WRONG_TYPE)?;
            let val = copy_scalar_from_user(value, size)?;
            obj.$setter(val)
        }};
    }

    match property {
        ZX_PROP_NAME => {
            let mut name_buf = [MaybeUninit::uninit(); ZX_MAX_NAME_LEN];
            let name_slice = value.copy_user_string(size, &mut name_buf)?;
            dispatcher.set_name(name_slice)
        }
        #[cfg(target_arch = "x86_64")]
        ZX_PROP_REGISTER_FS | ZX_PROP_REGISTER_GS => {
            require_current_thread(&dispatcher)?;
            let addr = copy_scalar_from_user::<usize>(value, size)?;
            if !crate::arch_rs::x86::is_vaddr_canonical(addr as u64) {
                return Err(Status::INVALID_ARGS);
            }
            // SAFETY: Writing canonical virtual address to valid MSR on current thread.
            if property == ZX_PROP_REGISTER_FS {
                unsafe { crate::arch_rs::x86::x86::write_msr(X86_MSR_IA32_FS_BASE, addr as u64) };
            } else {
                unsafe {
                    crate::arch_rs::x86::x86::write_msr(X86_MSR_IA32_KERNEL_GS_BASE, addr as u64)
                };
            }
            Ok(())
        }
        ZX_PROP_PROCESS_DEBUG_ADDR => {
            set_scalar_property!(ProcessDispatcher, set_debug_addr)
        }
        ZX_PROP_PROCESS_BREAK_ON_LOAD => {
            set_scalar_property!(ProcessDispatcher, set_dyn_break_on_load)
        }
        ZX_PROP_SOCKET_RX_THRESHOLD => {
            set_scalar_property!(SocketDispatcher, set_read_threshold)
        }
        ZX_PROP_SOCKET_TX_THRESHOLD => {
            set_scalar_property!(SocketDispatcher, set_write_threshold)
        }
        ZX_PROP_JOB_KILL_ON_OOM => {
            let job = dispatcher.downcast::<JobDispatcher>().ok_or(Status::WRONG_TYPE)?;
            let val = copy_scalar_from_user::<usize>(value, size)?;
            if val == 0 {
                job.set_kill_on_oom(false);
            } else if val == 1 {
                job.set_kill_on_oom(true);
            } else {
                return Err(Status::INVALID_ARGS);
            }
            Ok(())
        }
        // C++-only dispatchers (e.g. ExceptionDispatcher, StreamDispatcher, VmObjectDispatcher)
        _ => {
            // SAFETY: Call C++ FFI helper for properties on C++ dispatchers.
            let status = unsafe {
                cpp_object_set_property_cpp_types(
                    &*dispatcher,
                    property,
                    value.as_ptr() as *const core::ffi::c_void,
                    size,
                    rights,
                )
            };
            Status::ok(status)
        }
    }
}
