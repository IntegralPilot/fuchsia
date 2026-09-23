// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use fbl::RefPtr;
use zx_status::Status;
use zx_types::{zx_info_msi_t, zx_status_t};

unsafe extern "C" {
    pub(crate) fn cpp_msi_allocation_create(
        count: u32,
        alloc_out: *mut *mut MsiAllocation,
    ) -> zx_status_t;
    pub(crate) fn cpp_msi_allocation_get_info(alloc: *const MsiAllocation) -> zx_info_msi_t;
    pub(crate) fn cpp_msi_allocation_recycle(msi_alloc: *mut MsiAllocation);
}

fbl::impl_opaque_ref_counted_facade!(
    /// Facade type representing the C++ `MsiAllocation` object.
    pub struct MsiAllocation,
    cpp_msi_allocation_recycle,
);

impl MsiAllocation {
    /// Creates a new C++ `MsiAllocation` object with `count` interrupts.
    pub fn create(count: u32) -> Result<RefPtr<Self>, Status> {
        let mut alloc_raw = core::ptr::null_mut();
        // SAFETY: `&mut alloc_raw` is a valid non-null pointer to receive the raw
        // `MsiAllocation` pointer.
        let status = unsafe { cpp_msi_allocation_create(count, &mut alloc_raw) };
        Status::ok(status)?;
        // SAFETY: `cpp_msi_allocation_create` succeeded and returned an exported raw pointer.
        unsafe { Ok(RefPtr::from_raw(alloc_raw)) }
    }

    /// Returns `zx_info_msi_t` for this allocation.
    pub fn get_info(&self) -> zx_info_msi_t {
        // SAFETY: `self` is a valid `MsiAllocation` reference.
        unsafe { cpp_msi_allocation_get_info(self as *const _) }
    }
}
