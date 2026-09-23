// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::DispatcherOps;
use super::handle::KernelHandle;
use super::log_dispatcher::{LogDispatcher, LogDispatcherState};
use core::mem::MaybeUninit;
use zx_types::{ZX_LOG_READABLE, zx_rights_t, zx_status_t};

// C++ FFI declarations
unsafe extern "C" {
    pub(crate) fn cpp_log_dispatcher_create(
        flags: u32,
        rights: zx_rights_t,
        handle_out: *mut MaybeUninit<KernelHandle<LogDispatcher>>,
    ) -> zx_status_t;
}

// Trampoline callbacks from C++ into Rust LogDispatcherState

crate::object::dispatcher::impl_dispatcher_state_init!(LogDispatcher, LogDispatcherState, flags: u32);

/// Trampoline callback for DlogReader notify.
///
/// # Safety
///
/// `cookie` must point to the `LogDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_log_dispatcher_notify(cookie: *mut core::ffi::c_void) {
    // SAFETY: `cookie` was passed during `DlogReader` initialization and
    // points to the `LogDispatcher`.
    unsafe {
        let dispatcher = cookie.cast::<LogDispatcher>();
        (*dispatcher).update_state(0, ZX_LOG_READABLE);
    }
}

/// Trampoline callback for LogDispatcher creation from C++.
///
/// # Safety
///
/// `handle_out` and `rights_out` must be valid writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_log_dispatcher_create(
    flags: u32,
    handle_out: *mut MaybeUninit<KernelHandle<LogDispatcher>>,
    rights_out: *mut zx_rights_t,
) -> zx_status_t {
    // SAFETY: `handle_out` and `rights_out` are valid non-null writable pointers.
    unsafe {
        match LogDispatcher::create(flags) {
            Ok((handle, rights)) => {
                (*handle_out).write(handle);
                rights_out.write(rights);
                zx_types::ZX_OK
            }
            Err(status) => status.into_raw(),
        }
    }
}
