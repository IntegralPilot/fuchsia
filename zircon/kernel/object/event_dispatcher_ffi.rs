// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::event_dispatcher::{EventDispatcher, EventDispatcherState};
use super::handle::KernelHandle;
use core::mem::MaybeUninit;
use zx_types::{zx_duration_mono_t, zx_rights_t, zx_status_t};

// C++ FFI declarations
unsafe extern "C" {
    /// Calls into C++ implementation to create an event dispatcher.
    ///
    /// # Safety
    ///
    /// `handle_out` must point to uninitialized memory for a `KernelHandle<EventDispatcher>`.
    pub(crate) fn cpp_event_dispatcher_create(
        options: u32,
        handle_out: *mut MaybeUninit<KernelHandle<EventDispatcher>>,
    ) -> zx_status_t;

    /// Retrieves a reference to the kernel-owned memory pressure event dispatcher for the given kind.
    ///
    /// # Safety
    ///
    /// `out_event` must point to uninitialized memory for a `fbl::RefPtr<EventDispatcher>`.
    pub(crate) fn cpp_event_dispatcher_get_mem_pressure_event(
        kind: u32,
        out_event: *mut fbl::RefPtr<EventDispatcher>,
    );

    /// Calls into C++ implementation to create a memory stall event dispatcher.
    ///
    /// # Safety
    ///
    /// `out_handle` must point to uninitialized memory for a `KernelHandle<EventDispatcher>`.
    /// `out_rights` must point to writable memory.
    pub(crate) fn cpp_memory_stall_event_dispatcher_create(
        kind: u32,
        threshold: zx_duration_mono_t,
        window: zx_duration_mono_t,
        out_handle: *mut MaybeUninit<KernelHandle<EventDispatcher>>,
        out_rights: *mut MaybeUninit<zx_rights_t>,
    ) -> zx_status_t;
}

// FFI trampolines for C++ calling into Rust EventDispatcherState

crate::object::dispatcher::impl_dispatcher_state_init!(EventDispatcher, EventDispatcherState);

/// FFI trampoline for creating an EventDispatcher from C++.
///
/// # Safety
///
/// `handle_out` and `rights_out` must point to valid, non-null, writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_event_dispatcher_create(
    options: u32,
    handle_out: *mut MaybeUninit<KernelHandle<EventDispatcher>>,
    rights_out: *mut zx_types::zx_rights_t,
) -> zx_types::zx_status_t {
    // SAFETY: `handle_out` and `rights_out` are valid non-null writable pointers.
    unsafe {
        match EventDispatcher::create(options) {
            Ok((handle, rights)) => {
                (*handle_out).write(handle);
                rights_out.write(rights);
                zx_types::ZX_OK
            }
            Err(status) => status.into_raw(),
        }
    }
}
