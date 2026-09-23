// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::types::PAddr;
use core::marker::{PhantomData, PhantomPinned};
use zr::ToMutPtr;
use zx_status::Status;

// TODO(https://fxbug.dev/529507187): Use bitflags! or equivalent once available.
pub type ArchMmuFlags = u8;

#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_CACHED: ArchMmuFlags = 0 << 0;
pub const ARCH_MMU_FLAG_UNCACHED: ArchMmuFlags = 1 << 0;
#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_UNCACHED_DEVICE: ArchMmuFlags = 2 << 0;
#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_WRITE_COMBINING: ArchMmuFlags = 3 << 0;
#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_CACHE_MASK: ArchMmuFlags = 3 << 0;
pub const ARCH_MMU_FLAG_PERM_USER: ArchMmuFlags = 1 << 2;
pub const ARCH_MMU_FLAG_PERM_READ: ArchMmuFlags = 1 << 3;
pub const ARCH_MMU_FLAG_PERM_WRITE: ArchMmuFlags = 1 << 4;
pub const ARCH_MMU_FLAG_PERM_EXECUTE: ArchMmuFlags = 1 << 5;
pub const ARCH_MMU_FLAG_PERM_RWX_MASK: ArchMmuFlags =
    ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE;
pub const ARCH_MMU_FLAG_NS: ArchMmuFlags = 1 << 6;
pub const ARCH_MMU_FLAG_INVALID: ArchMmuFlags = 1 << 7;

// TODO(https://fxbug.dev/529507187): Use bitflags! or equivalent once available.
pub type ArchAspaceFlags = u8;

pub const ARCH_ASPACE_FLAG_KERNEL: ArchAspaceFlags = 1 << 0;
pub const ARCH_ASPACE_FLAG_GUEST: ArchAspaceFlags = 1 << 1;

// TODO(https://fxbug.dev/529507187): Use bitflags! or equivalent once available.
// Options for unmapping the given virtual address range.
pub type ArchUnmapOptions = u8;

pub const ARCH_UNMAP_OPTION_NONE: ArchUnmapOptions = 0;
// Controls whether the unmap region can be extended to be larger, or if only the exact region may
// be unmapped. The unmap region might be extended, even if only temporarily, if large pages need to
// be split.
pub const ARCH_UNMAP_OPTION_ENLARGE: ArchUnmapOptions = 1 << 0;
// Requests that the accessed bit be harvested, and the page queues updated.
pub const ARCH_UNMAP_OPTION_HARVEST: ArchUnmapOptions = 1 << 1;

/// Returns true if the MMU flags specify an uncached memory type.
#[inline]
pub const fn arch_mmu_flags_uncached(mmu_flags: ArchMmuFlags) -> bool {
    (mmu_flags & (ARCH_MMU_FLAG_UNCACHED | ARCH_MMU_FLAG_UNCACHED_DEVICE)) != 0
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingEntryAction {
    Skip = 0,
    Error = 1,
    Upgrade = 2,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonTerminalAction {
    /// If a non-terminal entry has no accessed information, unmap and free it. If it has accessed
    /// information, just remove the flag.
    FreeUnaccessed = 0,
    /// Retain both the non-terminal mappings and any accessed information.
    Retain = 1,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalAction {
    /// If the page is accessed update its age in the page queues, and remove the accessed flag.
    UpdateAgeAndHarvest = 0,
    /// If the page is accessed update its age in the page queues, but do not clear the flag.
    UpdateAge = 1,
}

unsafe extern "C" {
    fn cpp_arch_vm_aspace_init(aspace: *mut ArchVmAspace) -> i32;
    fn cpp_arch_vm_aspace_init_shared(aspace: *mut ArchVmAspace) -> i32;
    fn cpp_arch_vm_aspace_init_restricted(aspace: *mut ArchVmAspace) -> i32;
    fn cpp_arch_vm_aspace_init_unified(
        aspace: *mut ArchVmAspace,
        shared: *mut ArchVmAspace,
        restricted: *mut ArchVmAspace,
    ) -> i32;
    fn cpp_arch_vm_aspace_disable_updates(aspace: *mut ArchVmAspace);
    fn cpp_arch_vm_aspace_destroy(aspace: *mut ArchVmAspace) -> i32;
    fn cpp_arch_vm_aspace_map_contiguous(
        aspace: *mut ArchVmAspace,
        vaddr: usize,
        paddr: PAddr,
        count: usize,
        mmu_flags: ArchMmuFlags,
    ) -> i32;
    fn cpp_arch_vm_aspace_map(
        aspace: *mut ArchVmAspace,
        vaddr: usize,
        phys: *mut PAddr,
        count: usize,
        mmu_flags: ArchMmuFlags,
        existing_action: ExistingEntryAction,
    ) -> i32;
    fn cpp_arch_vm_aspace_unmap(
        aspace: *mut ArchVmAspace,
        vaddr: usize,
        count: usize,
        enlarge: ArchUnmapOptions,
    ) -> i32;
    fn cpp_arch_vm_aspace_unmap_only_enlarge_on_oom(aspace: *mut ArchVmAspace) -> bool;
    fn cpp_arch_vm_aspace_protect(
        aspace: *mut ArchVmAspace,
        vaddr: usize,
        count: usize,
        mmu_flags: ArchMmuFlags,
        enlarge: ArchUnmapOptions,
    ) -> i32;
    fn cpp_arch_vm_aspace_query(
        aspace: *mut ArchVmAspace,
        vaddr: usize,
        paddr: *mut PAddr,
        mmu_flags: *mut ArchMmuFlags,
    ) -> i32;
    fn cpp_arch_vm_aspace_pick_spot(
        aspace: *mut ArchVmAspace,
        base: usize,
        end: usize,
        align: usize,
        size: usize,
        mmu_flags: ArchMmuFlags,
    ) -> usize;
    fn cpp_arch_vm_aspace_harvest_accessed(
        aspace: *mut ArchVmAspace,
        vaddr: usize,
        count: usize,
        non_terminal_action: NonTerminalAction,
        terminal_action: TerminalAction,
    ) -> i32;
    fn cpp_arch_vm_aspace_mark_accessed(
        aspace: *mut ArchVmAspace,
        vaddr: usize,
        count: usize,
    ) -> i32;
    fn cpp_arch_vm_aspace_accessed_since_last_check(aspace: *mut ArchVmAspace, clear: bool)
    -> bool;
    fn cpp_arch_vm_aspace_arch_table_phys(aspace: *mut ArchVmAspace) -> PAddr;
}

#[repr(C)]
pub struct ArchVmAspace {
    // TODO(https://fxbug.dev/529915725): Use zx::OpaqueBytes with the correct size and alignment of
    // the underlying arch specific aspace structure to allow for correct Rust allocation.
    _data: [u8; 0],
    // Mark with PhantomPinned as this is a shared C++ struct and this object is not trivially
    // movable. Wrap it with PhantomData to allow for FFI usage.
    _marker: PhantomData<PhantomPinned>,
}

impl ArchVmAspace {
    /// This is used to create a regular address space with no special features. In
    /// architectures that do not support unified address spaces, it is also used to create
    /// shared and restricted address spaces. However, when unified address spaces are
    /// supported, the shared and restricted address spaces should be created with `init_shared`
    /// and `init_restricted`.
    pub fn init(&self) -> Result<(), Status> {
        Status::ok(unsafe { cpp_arch_vm_aspace_init(self.to_mut_ptr()) })
    }

    /// This is used to create a shared address space, whose contents can be
    /// accessed from multiple unified address spaces. These address spaces have a statically
    /// initialized top level page.
    pub fn init_shared(&self) -> Result<(), Status> {
        Status::ok(unsafe { cpp_arch_vm_aspace_init_shared(self.to_mut_ptr()) })
    }

    /// This is used to create a restricted address space, whose contents can be
    /// accessed from a single unified address space.
    pub fn init_restricted(&self) -> Result<(), Status> {
        Status::ok(unsafe { cpp_arch_vm_aspace_init_restricted(self.to_mut_ptr()) })
    }

    /// `init_unified`: This is used to create a unified address space. This type of address space
    /// owns no mappings of its own; rather, it is composed of a shared address space and a
    /// restricted address space. As a result, it expects `init_shared` to have been called
    /// on the shared address space, and expects `init_restricted` to have been called on the
    /// restricted address space.
    pub fn init_unified(
        &self,
        shared: &ArchVmAspace,
        restricted: &ArchVmAspace,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_arch_vm_aspace_init_unified(
                self.to_mut_ptr(),
                shared.to_mut_ptr(),
                restricted.to_mut_ptr(),
            )
        })
    }

    /// This method puts the instance into read-only mode and asserts that it contains no mappings.
    ///
    /// Note, this method may be a no-op on some architectures. See https://fxbug.dev/42159319.
    ///
    /// It is an error to call this method on an instance that contains mappings. Once called,
    /// subsequent operations that modify the page table will trigger a panic.
    ///
    /// The purpose of this method is to help enforce lifecycle and state transitions of VmAspace
    /// and ArchVmAspaceInterface.
    pub fn disable_updates(&self) {
        unsafe { cpp_arch_vm_aspace_disable_updates(self.to_mut_ptr()) }
    }

    /// Destroy expects the aspace to be fully unmapped, as any mapped regions indicate incomplete
    /// cleanup at the higher layers. Note that this does not apply to unified aspaces, which may
    /// still contain some mappings when destroy() is called.
    ///
    /// It is safe to call destroy even if init, init_shared, init_restricted, or init_unified
    /// failed. Once destroy has been called it is a user error to call any of the other methods on
    /// the aspace, unless specifically stated otherwise, and doing so may cause a panic.
    pub fn destroy(&self) -> Result<(), Status> {
        Status::ok(unsafe { cpp_arch_vm_aspace_destroy(self.to_mut_ptr()) })
    }

    /// Map a physically contiguous region into the virtual address space. This is allowed to use
    /// any page size the architecture allows given the input parameters.
    ///
    /// # Safety
    ///
    /// The caller must ensure the virtual and physical range are valid to map.
    pub unsafe fn map_contiguous(
        &self,
        vaddr: usize,
        paddr: PAddr,
        count: usize,
        mmu_flags: ArchMmuFlags,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_arch_vm_aspace_map_contiguous(self.to_mut_ptr(), vaddr, paddr, count, mmu_flags)
        })
    }

    // Map the given array of pages into the virtual address space starting at
    // `vaddr`, in the order they appear in `phys`.
    //
    // If any address in the range [vaddr, vaddr + count * kPageSize) is already
    // mapped when this is called, `existing_action` controls the behavior used:
    //  - `Skip` - Skip updating any existing mappings.
    //  - `Error` - Existing mappings result in a ZX_ERR_ALREADY_EXISTS error.
    //  - `Upgrade` - Upgrade any existing mappings, meaning a read-only mapping
    //                can be converted to read-write, or the mapping can have its
    //                paddr changed.
    //
    // On error none of the provided pages will be mapped. In the case of `Upgrade` the state of any
    // previous mappings is undefined, and could either still be present or be unmapped.
    ///
    /// # Safety
    ///
    /// The caller must ensure `phys` points to an array of at least `count` physical addresses and
    /// that the virtual and physical ranges are valid to map.
    pub unsafe fn map(
        &self,
        vaddr: usize,
        phys: *mut PAddr,
        count: usize,
        mmu_flags: ArchMmuFlags,
        existing_action: ExistingEntryAction,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_arch_vm_aspace_map(
                self.to_mut_ptr(),
                vaddr,
                phys,
                count,
                mmu_flags,
                existing_action,
            )
        })
    }

    /// Unmaps the given virtual address range.
    ///
    /// # Safety
    ///
    /// The caller must ensure the specified virtual range is no longer in use.
    pub unsafe fn unmap(
        &self,
        vaddr: usize,
        count: usize,
        enlarge: ArchUnmapOptions,
    ) -> Result<(), Status> {
        Status::ok(unsafe { cpp_arch_vm_aspace_unmap(self.to_mut_ptr(), vaddr, count, enlarge) })
    }

    /// Returns whether or not an unmap might need to enlarge an operation for reasons other than
    /// being out of memory. If this returns true, then unmapping a partial large page will fail
    /// always require an enlarged operation.
    pub fn unmap_only_enlarge_on_oom(&self) -> bool {
        unsafe { cpp_arch_vm_aspace_unmap_only_enlarge_on_oom(self.to_mut_ptr()) }
    }

    /// Change the page protections on the given virtual address range
    ///
    /// May return ZX_ERR_NO_MEMORY if the operation requires splitting
    /// a large page and the next level page table allocation fails. In
    /// this case, mappings in the input range may be a mix of the old and
    /// new flags.
    /// ArchUnmapOptions controls whether a larger range than requested is permitted to experience
    /// a temporary permissions change. A temporary change may be required if a break-before-make
    /// style unmap -> remap of the large page is required.
    ///
    /// # Safety
    ///
    /// The caller must ensure the permissions for the given virtual address range are valid.
    pub unsafe fn protect(
        &self,
        vaddr: usize,
        count: usize,
        mmu_flags: ArchMmuFlags,
        enlarge: ArchUnmapOptions,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_arch_vm_aspace_protect(self.to_mut_ptr(), vaddr, count, mmu_flags, enlarge)
        })
    }

    /// Queries the translation for `vaddr`.
    pub fn query(&self, vaddr: usize) -> Result<(PAddr, ArchMmuFlags), Status> {
        let mut paddr = PAddr(0);
        let mut mmu_flags: ArchMmuFlags = 0;
        Status::ok(unsafe {
            cpp_arch_vm_aspace_query(
                self.to_mut_ptr(),
                vaddr,
                &mut paddr as *mut PAddr,
                &mut mmu_flags as *mut ArchMmuFlags,
            )
        })
        .map(|_| (paddr, mmu_flags))
    }

    /// Picks a spot in the virtual address space.
    pub fn pick_spot(
        &self,
        base: usize,
        end: usize,
        align: usize,
        size: usize,
        mmu_flags: ArchMmuFlags,
    ) -> usize {
        unsafe {
            cpp_arch_vm_aspace_pick_spot(self.to_mut_ptr(), base, end, align, size, mmu_flags)
        }
    }

    /// Walks the given range of pages and for any pages that are mapped and have their access bit
    /// set:
    ///  * Tells the page queues it has been accessed via PageQueues::MarkAccessed
    ///  * Potentially removes the accessed flag.
    ///  * Potentially frees unaccessed page tables.
    pub fn harvest_accessed(
        &self,
        vaddr: usize,
        count: usize,
        non_terminal_action: NonTerminalAction,
        terminal_action: TerminalAction,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_arch_vm_aspace_harvest_accessed(
                self.to_mut_ptr(),
                vaddr,
                count,
                non_terminal_action,
                terminal_action,
            )
        })
    }

    /// Marks any pages in the given virtual address range as being accessed.
    pub fn mark_accessed(&self, vaddr: usize, count: usize) -> Result<(), Status> {
        Status::ok(unsafe { cpp_arch_vm_aspace_mark_accessed(self.to_mut_ptr(), vaddr, count) })
    }

    /// Returns whether or not this aspace might have additional accessed information since the last
    /// time this method was called with clear=true. If this returns `false` then, modulo races,
    /// harvest_accessed is defined to not find any set bits and not call PageQueues::MarkAccessed.
    ///
    /// This is intended for use by the harvester to avoid scanning for any accessed or dirty bits
    /// if the aspace has not been accessed at all.
    ///
    /// Note that restricted and shared ArchVmAspace's will report that they have been accessed if
    /// an associated unified ArchVmAspace has been accessed. However, the reverse is not true; the
    /// unified ArchVmAspace will not return true if the associated shared/restricted aspaces have
    /// been accessed.
    ///
    /// The `clear` flag controls whether the aspace having been accessed should be cleared or not.
    /// Not clearing makes this function const and not modify any state. If `clear` is true then
    /// this method is only thread-compatible and must be externally synchronized.
    pub fn accessed_since_last_check(&self, clear: bool) -> bool {
        unsafe { cpp_arch_vm_aspace_accessed_since_last_check(self.to_mut_ptr(), clear) }
    }

    /// Physical address of the backing data structure used for translation.
    ///
    /// This should be treated as an opaque value outside of
    /// architecture-specific components.
    pub fn arch_table_phys(&self) -> PAddr {
        unsafe { cpp_arch_vm_aspace_arch_table_phys(self.to_mut_ptr()) }
    }
}
