// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::iommu::Iommu;
use super::iommu_dispatcher::{IommuDispatcher, IommuDispatcherState};
use core::mem::MaybeUninit;
use fbl::RefPtr;
use zx_types::zx_status_t;

// C++ FFI declarations
unsafe extern "C" {
    pub(crate) fn cpp_iommu_dispatcher_create(
        type_param: u32,
        desc_ptr: *const u8,
        desc_len: usize,
        handle_out: *mut MaybeUninit<KernelHandle<IommuDispatcher>>,
    ) -> zx_status_t;
    pub(crate) fn cpp_iommu_recycle(iommu: *mut Iommu);
    pub(crate) fn cpp_iommu_get_ref_counted(iommu: *mut Iommu) -> *mut ();
}

// FFI trampolines for C++ calling into Rust IommuDispatcherState

/// Initializes an `IommuDispatcherState` in-place.
///
/// # Safety
///
/// `state` must point to valid uninitialized memory for `IommuDispatcherState`.
/// `iommu_raw` must be a valid raw pointer exported from an `fbl::RefPtr<Iommu>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_iommu_dispatcher_state_init(
    state: *mut IommuDispatcherState,
    dispatcher: *const IommuDispatcher,
    iommu_raw: *mut Iommu,
) {
    // SAFETY: `iommu_raw` is a valid raw pointer exported from `fbl::RefPtr<Iommu>`.
    let iommu = unsafe { RefPtr::from_raw(iommu_raw) };
    let init = IommuDispatcherState::init(dispatcher, iommu);
    // SAFETY: `state` points to uninitialized memory allocated for `IommuDispatcherState`.
    unsafe {
        let _ = pin_init::PinInit::__pinned_init(init, state);
    }
}

/// Returns the raw pointer to the underlying C++ `iommu::Iommu` object.
///
/// # Safety
///
/// `disp` must point to a valid `IommuDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_iommu_dispatcher_get_iommu(
    disp: *const IommuDispatcher,
) -> *mut Iommu {
    // SAFETY: `disp` points to a valid `IommuDispatcher`.
    unsafe {
        let iommu_ref = (*disp).iommu();
        RefPtr::as_ptr(iommu_ref).cast_mut()
    }
}
