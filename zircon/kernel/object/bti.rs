// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::bus_transaction_initiator_dispatcher_ffi::*;
use super::pmt::Pmt;
use core::mem::MaybeUninit;
use fbl::RefPtr;
use zx_status::Status;
use zx_types::ZX_MAX_NAME_LEN;

pub const IOMMU_FLAG_PERM_READ: u32 = 1 << 0;
pub const IOMMU_FLAG_PERM_WRITE: u32 = 1 << 1;
pub const IOMMU_FLAG_PERM_EXECUTE: u32 = 1 << 2;

fbl::impl_opaque_ref_counted_facade!(
    /// Facade type representing the C++ `iommu::Bti` object.
    pub struct Bti,
    cpp_bti_recycle,
    cpp_bti_get_ref_counted,
);

impl Bti {
    /// Returns a raw pointer to this BTI for FFI calls.
    #[inline]
    pub fn as_ffi(&self) -> *const Self {
        self as *const Self
    }

    /// Returns a mutable raw pointer to this BTI for FFI calls.
    #[inline]
    pub fn as_ffi_mut(&self) -> *mut Self {
        self as *const Self as *mut Self
    }

    /// Releases all quarantined PMTs.
    pub fn release_quarantine(&self) {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_release_quarantine(self.as_ffi_mut()) }
    }

    /// Informs the underlying driver that all user handles to the dispatcher have been closed.
    pub fn on_dispatcher_zero_handles(&self) {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_on_dispatcher_zero_handles(self.as_ffi_mut()) }
    }

    /// Returns the minimum contiguity guarantee in bytes.
    pub fn minimum_contiguity(&self) -> u64 {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_minimum_contiguity(self.as_ffi()) }
    }

    /// Returns the total size of the address space.
    pub fn aspace_size(&self) -> u64 {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_aspace_size(self.as_ffi()) }
    }

    /// Returns the current total count of active and quarantined PMTs.
    pub fn pmo_count(&self) -> u64 {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_pmo_count(self.as_ffi()) }
    }

    /// Returns the count of quarantined PMTs.
    pub fn quarantine_count(&self) -> u64 {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_quarantine_count(self.as_ffi()) }
    }

    /// Returns true when the underlying BTI is in a fault state.
    pub fn in_fault_state(&self) -> bool {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_in_fault_state(self.as_ffi()) }
    }

    /// Returns the hardware transaction ID represented by this BTI.
    pub fn bti_id(&self) -> u64 {
        // SAFETY: `self` is a valid `Bti` facade.
        unsafe { cpp_bti_bti_id(self.as_ffi()) }
    }

    /// Sets the debug name of this BTI.
    pub fn set_name(&self, name: &[u8]) -> Result<(), Status> {
        // SAFETY: `self` is a valid `Bti` facade, and `name` is a valid slice of bytes.
        let status = unsafe {
            cpp_bti_set_name(
                self.as_ffi_mut(),
                name.as_ptr().cast::<core::ffi::c_char>(),
                name.len(),
            )
        };
        Status::ok(status)
    }

    /// Gets the debug name of this BTI.
    pub fn get_name(&self, out_name: &mut [u8; ZX_MAX_NAME_LEN]) -> Result<(), Status> {
        // SAFETY: `self` is a valid `Bti` facade, and `out_name` is a valid output buffer.
        let status = unsafe {
            cpp_bti_get_name(self.as_ffi(), out_name.as_mut_ptr().cast::<core::ffi::c_char>())
        };
        Status::ok(status)
    }

    /// Grant the device access to the range of pages represented by `pinned_vmo`.
    pub fn map(
        &self,
        pinned_vmo: &mut crate::vm::pinned_vm_object::PinnedVmObject,
        perms: u32,
        require_contiguous: bool,
    ) -> Result<RefPtr<Pmt>, Status> {
        let mut pmt = MaybeUninit::<RefPtr<Pmt>>::uninit();
        // SAFETY: `self` is a valid `Bti` facade and `pinned_vmo` is a valid `PinnedVmObject`.
        let status = unsafe {
            cpp_bti_map(self.as_ffi_mut(), pinned_vmo, perms, require_contiguous, &raw mut pmt)
        };
        Status::ok(status)?;
        // SAFETY: `cpp_bti_map` initialized `pmt` with an `fbl::RefPtr<Pmt>`.
        unsafe { Ok(pmt.assume_init()) }
    }
}
