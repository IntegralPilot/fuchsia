// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::arch_vm_aspace::ArchMmuFlags;
use super::vm_aspace::VmAspace;
use super::vm_mapping::VmMapping;
use super::vm_object::VmObject;
use crate::kernel::types::VAddr;
use core::ffi::{CStr, c_char};
use fbl::RefPtr;
use zr::ToMutPtr;
use zx_status::Status;

use vm_address_region_bindings as bindings;

pub mod flag {
    use vm_address_region_bindings as bindings;

    /// When randomly allocating subregions, reduce sprawl by placing allocations near each other.
    pub const COMPACT: u32 = bindings::VMAR_FLAG_COMPACT;
    /// Request that the new region be at the specified offset in its parent region.
    pub const SPECIFIC: u32 = bindings::VMAR_FLAG_SPECIFIC;
    /// Like `VMAR_FLAG_SPECIFIC`, but permits overwriting existing mappings.
    pub const SPECIFIC_OVERWRITE: u32 = bindings::VMAR_FLAG_SPECIFIC_OVERWRITE;
    /// Allow `VmMappings` to be created inside the new region with the `SPECIFIC` or
    /// `OFFSET_IS_UPPER_LIMIT` flag.
    pub const CAN_MAP_SPECIFIC: u32 = bindings::VMAR_FLAG_CAN_MAP_SPECIFIC;
    /// Allow `VmMappings` to be created inside the region with read permissions.
    pub const CAN_MAP_READ: u32 = bindings::VMAR_FLAG_CAN_MAP_READ;
    /// Allow `VmMappings` to be created inside the region with write permissions.
    pub const CAN_MAP_WRITE: u32 = bindings::VMAR_FLAG_CAN_MAP_WRITE;
    /// Allow `VmMappings` to be created inside the region with execute permissions.
    pub const CAN_MAP_EXECUTE: u32 = bindings::VMAR_FLAG_CAN_MAP_EXECUTE;
    /// Require that VMO backing the mapping is non-resizable.
    pub const REQUIRE_NON_RESIZABLE: u32 = bindings::VMAR_FLAG_REQUIRE_NON_RESIZABLE;
    /// Allow VMO backings that could result in faults.
    pub const ALLOW_FAULTS: u32 = bindings::VMAR_FLAG_ALLOW_FAULTS;
    /// Treat the offset as an upper limit when allocating a VMO or child VMAR.
    pub const OFFSET_IS_UPPER_LIMIT: u32 = bindings::VMAR_FLAG_OFFSET_IS_UPPER_LIMIT;
    /// Opt this VMAR out of certain debugging checks. This allows for kernel mappings that have a
    /// more dynamic management strategy, that the regular checks would otherwise spuriously trip
    /// on.
    pub const DEBUG_DYNAMIC_KERNEL_MAPPING: u32 = bindings::VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING;
    /// Memory accesses past the stream size rounded up to the page boundary will fault.
    pub const FAULT_BEYOND_STREAM_SIZE: u32 = bindings::VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE;

    /// Mask of read, write, and execute permission flags.
    pub const CAN_RWX_FLAGS: u32 = bindings::VMAR_CAN_RWX_FLAGS;
}

/// Memory priorities that can be applied to VMARs and mappings to propagate to VMOs and page
/// tables.
pub type MemoryPriority = bindings::VmAddressRegionOrMapping_MemoryPriority;
pub type VmAddressRegionOpChildren = bindings::VmAddressRegionOpChildren;
pub type RangeOpType = bindings::VmAddressRegion_RangeOpType;
pub type UnmapOptions = bindings::VmMapping_UnmapOptions;

/// Internal fully locked version of Destroy. Has controls to both skip the arch aspace unmapping
/// as well as removal from the parent subregions list. These controls facilitate the fine grained
/// control needed when splitting, merging and replacing mappings.
///
/// If unmap is `No` then this method is defined to never fail. `remove_region` does not impact
/// success or failure of the operation.
pub type DestroyUnmap = bindings::VmMapping_DestroyUnmap;
pub type DestroyRemoveFromParent = bindings::VmMapping_DestroyRemoveFromParent;

pub type Mergeable = bindings::VmMapping_Mergeable;

/// Fully locked version of Activate that can additionally control whether the region is installed
/// into the parent subregion list and vmo mapping list or not. This control exists to facilitate
/// the fine grained control needed for splitting, merging and replacing of mappings as when set to
/// `No` this method is defined as never failing.
pub type ActivateInsertRegions = bindings::VmMapping_ActivateInsertRegions;

/// Result of calling [`VmAddressRegion::create_vm_mapping`].
pub struct MapResult {
    /// The newly created mapping.
    pub mapping: RefPtr<VmMapping>,
    /// The virtual address of the mapping at creation time.
    pub base: usize,
}

unsafe extern "C" {
    fn cpp_vm_address_region_get_ref_counted(vmar: *mut VmAddressRegion) -> *mut fbl::RefCounted;
    fn cpp_vm_address_region_free(vmar: *mut VmAddressRegion);
    fn cpp_vm_address_region_destroy(vmar: *mut VmAddressRegion) -> i32;
    fn cpp_vm_address_region_aspace(vmar: *mut VmAddressRegion) -> *const RefPtr<VmAspace>;
    fn cpp_vm_address_region_base(vmar: *mut VmAddressRegion) -> VAddr;
    fn cpp_vm_address_region_size(vmar: *mut VmAddressRegion) -> usize;
    fn cpp_vm_address_region_flags(vmar: *mut VmAddressRegion) -> u32;
    fn cpp_vm_address_region_name(vmar: *mut VmAddressRegion) -> *const c_char;
    fn cpp_vm_address_region_has_parent(vmar: *mut VmAddressRegion) -> bool;
    fn cpp_vm_address_region_set_memory_priority(
        vmar: *mut VmAddressRegion,
        priority: MemoryPriority,
    ) -> i32;
    fn cpp_vm_address_region_unmap(
        vmar: *mut VmAddressRegion,
        base: VAddr,
        size: usize,
        op_children: VmAddressRegionOpChildren,
    ) -> i32;
    fn cpp_vm_address_region_protect(
        vmar: *mut VmAddressRegion,
        base: VAddr,
        size: usize,
        new_arch_mmu_flags: ArchMmuFlags,
        op_children: VmAddressRegionOpChildren,
    ) -> i32;
    fn cpp_vm_address_region_reserve_space(
        vmar: *mut VmAddressRegion,
        name: *const c_char,
        base: usize,
        size: usize,
        arch_mmu_flags: ArchMmuFlags,
    ) -> i32;
    fn cpp_vm_address_region_create_sub_vmar(
        vmar: *mut VmAddressRegion,
        offset: usize,
        size: usize,
        align_pow2: u8,
        vmar_flags: u32,
        name: *const c_char,
        out_status: *mut i32,
    ) -> *mut VmAddressRegion;
    fn cpp_vm_address_region_create_vm_mapping(
        vmar: *mut VmAddressRegion,
        mapping_offset: usize,
        size: usize,
        align_pow2: u8,
        vmar_flags: u32,
        vmo: *const VmObject,
        vmo_offset: u64,
        arch_mmu_flags: ArchMmuFlags,
        name: *const c_char,
        out_base: *mut usize,
        out_status: *mut i32,
    ) -> *mut VmMapping;
}

fbl::impl_opaque_ref_counted_facade!(
    /// A contiguous region of the virtual address space.
    pub struct VmAddressRegion,
    cpp_vm_address_region_free,
    cpp_vm_address_region_get_ref_counted,
);

impl VmAddressRegion {
    /// Creates a subregion of this region.
    pub fn create_sub_vmar(
        &self,
        offset: usize,
        size: usize,
        align_pow2: u8,
        vmar_flags: u32,
        name: &CStr,
    ) -> Result<RefPtr<VmAddressRegion>, Status> {
        let mut status = 0;
        let raw = unsafe {
            cpp_vm_address_region_create_sub_vmar(
                self.to_mut_ptr(),
                offset,
                size,
                align_pow2,
                vmar_flags,
                name.as_ptr(),
                &mut status,
            )
        };
        Status::ok(status)?;
        unsafe { RefPtr::try_from_raw(raw).ok_or(Status::NO_MEMORY) }
    }

    /// Creates a [`VmMapping`] within this region.
    ///
    /// To avoid leaks, this should be paired with a call to [`VmMapping::destroy`] if desired;
    /// dropping `MapResult::mapping` will not destroy the mapping.
    #[allow(clippy::too_many_arguments)]
    pub fn create_vm_mapping(
        &self,
        mapping_offset: usize,
        size: usize,
        align_pow2: u8,
        vmar_flags: u32,
        vmo: RefPtr<VmObject>,
        vmo_offset: u64,
        arch_mmu_flags: ArchMmuFlags,
        name: &CStr,
    ) -> Result<MapResult, Status> {
        let mut base = 0;
        let mut status = 0;
        let raw = unsafe {
            cpp_vm_address_region_create_vm_mapping(
                self.to_mut_ptr(),
                mapping_offset,
                size,
                align_pow2,
                vmar_flags,
                RefPtr::into_raw(vmo),
                vmo_offset,
                arch_mmu_flags,
                name.as_ptr(),
                &mut base,
                &mut status,
            )
        };
        Status::ok(status)?;
        let mapping = unsafe { RefPtr::try_from_raw(raw).ok_or(Status::NO_MEMORY)? };
        Ok(MapResult { mapping, base })
    }

    /// Destroys this region and recursively destroys child VMARs.
    pub fn destroy(&self) -> Result<(), Status> {
        Status::ok(unsafe { cpp_vm_address_region_destroy(self.to_mut_ptr()) })
    }

    /// Returns a reference to the address space this region belongs to.
    pub fn aspace(&self) -> &RefPtr<VmAspace> {
        // SAFETY: `vmar->aspace()` returns a reference to `vmar->aspace_`, which is non-null
        // and lives for the lifetime of `self`.
        unsafe { &*cpp_vm_address_region_aspace(self.to_mut_ptr()) }
    }

    /// Returns the base address of this region.
    pub fn base(&self) -> VAddr {
        unsafe { cpp_vm_address_region_base(self.to_mut_ptr()) }
    }

    /// Returns the size in bytes of this region.
    pub fn size(&self) -> usize {
        unsafe { cpp_vm_address_region_size(self.to_mut_ptr()) }
    }

    /// Returns the creation flags of this region.
    pub fn flags(&self) -> u32 {
        unsafe { cpp_vm_address_region_flags(self.to_mut_ptr()) }
    }

    /// Returns the name of this region.
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(cpp_vm_address_region_name(self.to_mut_ptr())) }
    }

    /// Returns true if this region has a parent region.
    pub fn has_parent(&self) -> bool {
        unsafe { cpp_vm_address_region_has_parent(self.to_mut_ptr()) }
    }

    /// Applies the given memory priority to this region and all subregions.
    pub fn set_memory_priority(&self, priority: MemoryPriority) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_vm_address_region_set_memory_priority(self.to_mut_ptr(), priority)
        })
    }

    /// Unmaps a subset of the region of memory in the containing address space.
    ///
    /// # Safety
    ///
    /// Caller must ensure the specified virtual address region to unmap is no longer used.
    pub unsafe fn unmap(
        &self,
        base: VAddr,
        size: usize,
        op_children: VmAddressRegionOpChildren,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_vm_address_region_unmap(self.to_mut_ptr(), base, size, op_children)
        })
    }

    /// Changes protections on a subset of the region of memory in the containing address space.
    pub fn protect(
        &self,
        base: VAddr,
        size: usize,
        new_arch_mmu_flags: ArchMmuFlags,
        op_children: VmAddressRegionOpChildren,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_vm_address_region_protect(
                self.to_mut_ptr(),
                base,
                size,
                new_arch_mmu_flags,
                op_children,
            )
        })
    }

    /// Reserves a memory region within this VMAR without allocating physical pages.
    pub fn reserve_space(
        &self,
        name: &CStr,
        base: usize,
        size: usize,
        arch_mmu_flags: ArchMmuFlags,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_vm_address_region_reserve_space(
                self.to_mut_ptr(),
                name.as_ptr(),
                base,
                size,
                arch_mmu_flags,
            )
        })
    }
}
