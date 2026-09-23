// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use fbl::RefPtr;
use zx_types::{zx_info_stream_t, zx_off_t, zx_status_t};

use super::KernelHandle;
use super::stream_dispatcher::{StreamDispatcher, StreamDispatcherState, StreamOptions};
use super::vm_object_dispatcher::VmObjectDispatcher;
use crate::vm::stream_size_manager::StreamSizeManager;
use crate::vm::vm_object_paged::VmObjectPaged;
use core::mem::MaybeUninit;

// C++ FFI declarations
#[allow(improper_ctypes)]
unsafe extern "C" {
    pub(crate) fn cpp_stream_dispatcher_create(
        options: u32,
        vmo_dispatcher: *const VmObjectDispatcher,
        seek: zx_off_t,
        handle_out: *mut MaybeUninit<KernelHandle<StreamDispatcher>>,
    ) -> zx_status_t;
}

// Trampoline callbacks from C++ into Rust StreamDispatcherState

/// Initializes a `StreamDispatcherState` in place.
///
/// # Safety
///
/// `state` must point to uninitialized memory of at least `StreamDispatcherState` size and align.
/// `vmo` and `stream_size_manager` must be valid pointers with ownership of one RefPtr count
/// transferred.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_stream_dispatcher_state_init(
    state: *mut StreamDispatcherState,
    _dispatcher: *mut StreamDispatcher,
    options: u32,
    seek: zx_off_t,
    vmo: *mut VmObjectPaged,
    stream_size_manager: *mut StreamSizeManager,
) {
    // SAFETY: The C++ constructor passes an uninitialized `opaque_storage_` buffer of sufficient
    // size and alignment, along with exported `RefPtr` pointers for vmo and stream_size_manager.
    unsafe {
        let vmo = VmObjectPaged::from_raw(vmo.cast()).expect("vmo must not be null");
        let stream_size_manager = RefPtr::try_from_raw(stream_size_manager)
            .expect("stream_size_manager must not be null");
        let _ = pin_init::PinInit::__pinned_init(
            StreamDispatcherState::init(
                StreamOptions::from(options),
                seek,
                vmo,
                stream_size_manager,
            ),
            state,
        );
    }
}

/// Returns whether the stream is in append mode.
#[unsafe(no_mangle)]
pub extern "C" fn rust_stream_dispatcher_is_in_append_mode(dispatcher: &StreamDispatcher) -> bool {
    dispatcher.is_in_append_mode()
}

/// Sets whether the stream is in append mode.
#[unsafe(no_mangle)]
pub extern "C" fn rust_stream_dispatcher_set_append_mode(
    dispatcher: &StreamDispatcher,
    value: bool,
) {
    dispatcher.set_append_mode(value);
}

/// Returns whether the stream can resize the underlying VMO.
#[unsafe(no_mangle)]
pub extern "C" fn rust_stream_dispatcher_can_resize_vmo(dispatcher: &StreamDispatcher) -> bool {
    dispatcher.can_resize_vmo()
}

/// Returns diagnostic info for the stream dispatcher.
#[unsafe(no_mangle)]
pub extern "C" fn rust_stream_dispatcher_get_info(
    dispatcher: &StreamDispatcher,
) -> zx_info_stream_t {
    dispatcher.get_info()
}
