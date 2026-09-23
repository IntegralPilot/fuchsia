// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::pinned_memory_token_dispatcher_ffi::*;
use core::mem::MaybeUninit;
use zx_status::Status;

/// Type used to refer to virtual addresses presented in virtual address space
/// presented to a device by the IOMMU.
#[allow(non_camel_case_types)]
pub type dev_vaddr_t = u64;

/// Result returned from querying a PMT's device virtual address mapping.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryAddressResult {
    pub device_vaddr: dev_vaddr_t,
    pub size: usize,
}

zr::static_assert!(core::mem::size_of::<QueryAddressResult>() == 16);
zr::static_assert!(core::mem::align_of::<QueryAddressResult>() == 8);

fbl::impl_opaque_ref_counted_facade!(
    /// Facade type representing the C++ `iommu::Pmt` object.
    pub struct Pmt,
    cpp_pmt_recycle,
    cpp_pmt_get_ref_counted,
);

impl Pmt {
    /// Returns a raw pointer to this PMT for FFI calls.
    #[inline]
    pub fn as_ffi(&self) -> *const Self {
        self as *const Self
    }

    /// Returns a mutable raw pointer to this PMT for FFI calls.
    #[inline]
    pub fn as_ffi_mut(&self) -> *mut Self {
        self as *const Self as *mut Self
    }

    /// Queries the information of the pinned VMO. Attempts to find the (single)
    /// continuous range from `[offset, offset + size)` in the device's address space
    /// for the pinned VMO managed by this token.
    ///
    /// On success, returned range might be less than, equal or greater than the
    /// queried size. In the case of being less than, additional contiguous ranges
    /// can be found by calling again with a new `offset`.
    pub fn query_address(&self, offset: u64, size: usize) -> Result<QueryAddressResult, Status> {
        let mut result = MaybeUninit::<QueryAddressResult>::uninit();
        // SAFETY: `self` is a valid `Pmt` facade and `result` points to uninitialized storage.
        let status =
            unsafe { cpp_pmt_query_address(self.as_ffi_mut(), offset, size, &raw mut result) };
        Status::ok(status)?;
        // SAFETY: `cpp_pmt_query_address` succeeded and initialized `result`.
        unsafe { Ok(result.assume_init()) }
    }

    /// Unmap the memory managed by the PMT from its device's address space,
    /// revoking device access in the process. Then release the reference to the
    /// memory held by the internal `PinnedVmObject` instance, potentially unpinning
    /// the memory and returning it to the PMM in the process.
    pub fn release_pinned_memory(&self) {
        // SAFETY: `self` is a valid `Pmt` facade.
        unsafe { cpp_pmt_release_pinned_memory(self.as_ffi_mut()) }
    }

    /// Called when the dispatcher which owns this PMT reaches the end of its
    /// user-mode life. Lets the driver level know in case something special needs
    /// to be done (such as quarantining the memory).
    pub fn on_dispatcher_zero_handles(&self) {
        // SAFETY: `self` is a valid `Pmt` facade.
        unsafe { cpp_pmt_on_dispatcher_zero_handles(self.as_ffi_mut()) }
    }

    /// Returns the size in bytes of the pinned VMO.
    pub fn size(&self) -> u64 {
        // SAFETY: `self` is a valid `Pmt` facade.
        unsafe { cpp_pmt_size(self.as_ffi()) }
    }
}
