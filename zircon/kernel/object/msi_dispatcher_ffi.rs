// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::msi_allocation::MsiAllocation;
use super::msi_dispatcher::{MsiDispatcher, MsiDispatcherState};
use core::mem::MaybeUninit;
use fbl::RefPtr;
use zx_types::{zx_info_msi_t, zx_status_t};

// C++ FFI declarations
unsafe extern "C" {
    pub(crate) fn cpp_msi_dispatcher_create(
        msi_alloc: *mut MsiAllocation,
        handle_out: *mut MaybeUninit<KernelHandle<MsiDispatcher>>,
    ) -> zx_status_t;
}

// FFI trampolines for C++ calling into Rust MsiDispatcherState

/// Initializes an `MsiDispatcherState` in-place.
///
/// # Safety
///
/// `state` must point to valid uninitialized memory for `MsiDispatcherState`.
/// `dispatcher` must be a valid pointer to an `MsiDispatcher`.
/// `msi_alloc_raw` must be a valid raw pointer exported from an `fbl::RefPtr<MsiAllocation>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_msi_dispatcher_state_init(
    state: *mut MsiDispatcherState,
    dispatcher: *const MsiDispatcher,
    msi_alloc_raw: *mut MsiAllocation,
) {
    // SAFETY: `msi_alloc_raw` is a valid raw pointer exported from `fbl::RefPtr<MsiAllocation>`.
    let msi_alloc = unsafe { RefPtr::from_raw(msi_alloc_raw) };
    let init = MsiDispatcherState::init(dispatcher, msi_alloc);
    // SAFETY: `state` points to uninitialized memory allocated for `MsiDispatcherState`.
    unsafe {
        let _ = pin_init::PinInit::__pinned_init(init, state);
    }
}

/// FFI trampoline to get `zx_info_msi_t` from an `MsiDispatcher`.
///
/// # Safety
///
/// `disp` must be a valid reference to an initialized `MsiDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_msi_dispatcher_get_info(disp: &MsiDispatcher) -> zx_info_msi_t {
    disp.get_info()
}
