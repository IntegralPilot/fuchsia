// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::vm_address_region_dispatcher_ffi::{
    cpp_vmar_dispatcher_allocate, cpp_vmar_dispatcher_map, cpp_vmar_dispatcher_set_memory_priority,
};
use crate::vm::vm_object::VmObject;
use zx_status::Status;
use zx_types::zx_rights_t;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPriority {
    Default = 0,
    High = 1,
}

crate::object::dispatcher::impl_dispatcher_facade!(
    pub struct VmAddressRegionDispatcher,
    zx_types::ZX_OBJ_TYPE_VMAR
);

impl VmAddressRegionDispatcher {
    /// Sets memory priority for this VMAR dispatcher.
    pub fn set_memory_priority(&self, priority: MemoryPriority) -> Result<(), Status> {
        // SAFETY: `self` is a valid `VmAddressRegionDispatcher` reference.
        let status = unsafe { cpp_vmar_dispatcher_set_memory_priority(self, priority as u32) };
        Status::ok(status)
    }

    /// Allocates a sub-VMAR within this VMAR.
    pub fn allocate(
        &self,
        offset: usize,
        size: usize,
        flags: u32,
    ) -> Result<(KernelHandle<VmAddressRegionDispatcher>, zx_rights_t), Status> {
        // SAFETY: `self` is a valid `VmAddressRegionDispatcher` reference, and
        // `cpp_vmar_dispatcher_allocate` initializes `handle` and `rights` on success.
        unsafe {
            KernelHandle::create_with_rights(|handle_out, rights_out| {
                cpp_vmar_dispatcher_allocate(self, offset, size, flags, handle_out, rights_out)
            })
        }
    }

    /// Maps a VMO into this VMAR.
    pub fn map(
        &self,
        vmar_offset: usize,
        vmo: &VmObject,
        vmo_offset: u64,
        len: usize,
        flags: u32,
    ) -> Result<usize, Status> {
        let mut out_base = 0;
        // SAFETY: `self` is a valid `VmAddressRegionDispatcher` reference.
        // `vmo` is a valid `VmObject` reference.
        // `out_base` points to valid writable stack memory.
        let status = unsafe {
            cpp_vmar_dispatcher_map(self, vmar_offset, vmo, vmo_offset, len, flags, &mut out_base)
        };
        Status::ok(status)?;
        Ok(out_base)
    }

    /// Returns information about this VMAR.
    pub fn get_vmar_info(&self) -> zx_types::zx_info_vmar_t {
        // SAFETY: self is a valid VmAddressRegionDispatcher reference.
        unsafe {
            super::vm_address_region_dispatcher_ffi::cpp_vmar_dispatcher_get_vmar_info(
                self as *const _,
            )
        }
    }
}
