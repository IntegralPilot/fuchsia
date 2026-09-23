// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::arch_vm_aspace::{ArchMmuFlags, ArchVmAspace, NonTerminalAction, TerminalAction};
use super::vm_address_region::VmAddressRegion;
use super::vm_mapping::VmMapping;
use super::vm_object::VmObject;
use crate::kernel::thread::{Thread, ThreadPtr};
use crate::kernel::types::PAddr;
use core::ffi::{CStr, c_char, c_void};
use fbl::RefPtr;
use vm_aspace_bindings as bindings;
use zr::ToMutPtr;
use zx_status::Status;

/// For region creation routines
pub mod vmm_flag {
    use super::bindings;

    /// allocate at specific address
    pub const VALLOC_SPECIFIC: u32 = bindings::VmAspace_VMM_FLAG_VALLOC_SPECIFIC;

    /// commit memory up front (no demand paging)
    pub const COMMIT: u32 = bindings::VmAspace_VMM_FLAG_COMMIT;
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    User = 0,
    Kernel = 1,
    /// You probably do not want to use `LowKernel`. It is primarily used for SMP bootstrap or mexec
    /// to allow mappings of very low memory using the standard VMM subsystem.
    LowKernel = 2,
    /// Used to construct an address space representing hypervisor guest memory.
    GuestPhysical = 3,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareOpt {
    /// A normal independent address space initialized using [`ArchVmAspace::init`].
    None = 0,
    /// A restricted address space whose underlying [`ArchVmAspace`] will be initialized using
    /// `init_restricted`.
    Restricted = 1,
    /// A shared address space whose underlying [`ArchVmAspace`] will be initialized using
    /// `init_shared`.
    Shared = 2,
}

unsafe extern "C" {
    fn cpp_vm_aspace_get_ref_counted(aspace: *mut VmAspace) -> *mut fbl::RefCounted;
    fn cpp_vm_aspace_create(type_: Type, name: *const c_char) -> *mut VmAspace;
    fn cpp_vm_aspace_create_with_opts(
        base: usize,
        size: usize,
        type_: Type,
        name: *const c_char,
        share_opt: ShareOpt,
    ) -> *mut VmAspace;
    fn cpp_vm_aspace_create_unified(
        shared: *mut VmAspace,
        restricted: *mut VmAspace,
        name: *const c_char,
    ) -> *mut VmAspace;
    fn cpp_vm_aspace_arch_aspace(aspace: *mut VmAspace) -> *mut ArchVmAspace;
    fn cpp_vm_aspace_kernel_aspace() -> *mut VmAspace;
    fn cpp_vm_aspace_root_vmar(aspace: *mut VmAspace) -> *mut VmAddressRegion;
    fn cpp_vm_aspace_base(aspace: *mut VmAspace) -> usize;
    fn cpp_vm_aspace_size(aspace: *mut VmAspace) -> usize;
    fn cpp_vm_aspace_name(aspace: *mut VmAspace) -> *const c_char;
    fn cpp_vm_aspace_is_user(aspace: *mut VmAspace) -> bool;
    fn cpp_vm_aspace_is_aslr_enabled(aspace: *mut VmAspace) -> bool;
    fn cpp_vm_aspace_is_destroyed(aspace: *mut VmAspace) -> bool;
    fn cpp_vm_aspace_destroy(aspace: *mut VmAspace) -> i32;
    fn cpp_vm_aspace_rename(aspace: *mut VmAspace, name: *const c_char);
    fn cpp_vm_aspace_dump(aspace: *mut VmAspace, verbose: bool);
    fn cpp_vm_aspace_attach_to_thread(aspace: *mut VmAspace, thread: *mut Thread);
    fn cpp_vm_aspace_vdso_base_address(aspace: *mut VmAspace) -> usize;
    fn cpp_vm_aspace_vdso_code_address(aspace: *mut VmAspace) -> usize;
    fn cpp_vm_aspace_is_high_memory_priority(aspace: *mut VmAspace) -> bool;
    fn cpp_vm_aspace_accessed_fault(aspace: *mut VmAspace, va: usize) -> i32;
    fn cpp_vm_aspace_page_fault(aspace: *mut VmAspace, va: usize, flags: u32) -> i32;
    fn cpp_vm_aspace_soft_fault(aspace: *mut VmAspace, va: usize, flags: u32) -> i32;
    fn cpp_vm_aspace_soft_fault_in_range(
        aspace: *mut VmAspace,
        va: usize,
        flags: u32,
        len: usize,
    ) -> i32;
    fn cpp_vm_aspace_drop_user_page_tables(aspace: *mut VmAspace);
    fn cpp_vm_aspace_drop_all_user_page_tables();
    fn cpp_vm_aspace_dump_all_aspaces(verbose: bool);
    fn cpp_vm_aspace_harvest_all_user_accessed_bits(
        non_terminal_action: NonTerminalAction,
        terminal_action: TerminalAction,
    );
    fn cpp_vm_aspace_alloc_physical(
        aspace: *mut VmAspace,
        name: *const c_char,
        size: usize,
        ptr: *mut *mut c_void,
        align_pow2: u8,
        paddr: PAddr,
        vmm_flags: u32,
        arch_mmu_flags: ArchMmuFlags,
    ) -> i32;
    fn cpp_vm_aspace_alloc_contiguous(
        aspace: *mut VmAspace,
        name: *const c_char,
        size: usize,
        ptr: *mut *mut c_void,
        align_pow2: u8,
        vmm_flags: u32,
        arch_mmu_flags: ArchMmuFlags,
    ) -> i32;
    fn cpp_vm_aspace_map_object_internal(
        aspace: *mut VmAspace,
        vmo: *mut VmObject,
        name: *const c_char,
        offset: u64,
        size: usize,
        ptr: *mut *mut c_void,
        align_pow2: u8,
        vmm_flags: u32,
        arch_mmu_flags: ArchMmuFlags,
    ) -> i32;
    fn cpp_vm_aspace_free_region(aspace: *mut VmAspace, va: usize) -> i32;
    fn cpp_vm_aspace_free(aspace: *mut VmAspace);
    fn cpp_vm_aspace_find_mapping(aspace: *mut VmAspace, vaddr: usize) -> *mut VmMapping;
}

fbl::impl_opaque_ref_counted_facade!(
    /// An address space in the kernel or user space.
    pub struct VmAspace,
    cpp_vm_aspace_free,
    cpp_vm_aspace_get_ref_counted,
);

impl VmAspace {
    /// Creates an address space of the type specified in `type_` with name `name`.
    ///
    /// Although reference counted, the returned [`VmAspace`] must be explicitly destroyed via
    /// [`destroy`](Self::destroy).
    ///
    /// Returns `None` on failure (e.g. due to resource starvation).
    pub fn create(type_: Type, name: &CStr) -> Option<RefPtr<VmAspace>> {
        unsafe { RefPtr::try_from_raw(cpp_vm_aspace_create(type_, name.as_ptr())) }
    }

    /// Creates an address space of the type specified in `type_` with name `name`.
    ///
    /// The returned aspace will start at `base` and span `size`.
    ///
    /// If `share_opt` is [`ShareOpt::Shared`], we're creating a shared address space, and the
    /// underlying [`ArchVmAspace`] will be initialized using the
    /// [`init_shared`](ArchVmAspace::init_shared) method instead of the normal
    /// [`init`](ArchVmAspace::init) method.
    ///
    /// If `share_opt` is [`ShareOpt::Restricted`], we're creating a restricted address space, and
    /// the underlying [`ArchVmAspace`] will be initialized using the
    /// [`init_restricted`](ArchVmAspace::init_restricted) method.
    ///
    /// Although reference counted, the returned [`VmAspace`] must be explicitly destroyed via
    /// [`destroy`](Self::destroy).
    ///
    /// Returns `None` on failure (e.g. due to resource starvation).
    pub fn create_with_opts(
        base: usize,
        size: usize,
        type_: Type,
        name: &CStr,
        share_opt: ShareOpt,
    ) -> Option<RefPtr<VmAspace>> {
        unsafe {
            RefPtr::try_from_raw(cpp_vm_aspace_create_with_opts(
                base,
                size,
                type_,
                name.as_ptr(),
                share_opt,
            ))
        }
    }

    /// Creates a unified address space that consists of the given constituent address spaces.
    ///
    /// The passed in address spaces must meet the following criteria:
    /// 1. They must manage non-overlapping regions.
    /// 2. The shared [`VmAspace`] must have been created with the shared argument set to true.
    ///
    /// Although reference counted, the returned [`VmAspace`] must be explicitly destroyed via
    /// [`destroy`](Self::destroy). Note that it must be destroyed before the shared and
    /// restricted [`VmAspace`]s; destroying the constituent [`VmAspace`]s before destroying
    /// this one will trigger asserts.
    ///
    /// Returns `None` on failure (e.g. due to resource starvation).
    ///
    /// # Safety
    ///
    /// The caller must ensure that `shared` and `restricted` are valid pointers to address
    /// spaces initialized as shared and restricted respectively.
    pub unsafe fn create_unified(
        shared: *mut VmAspace,
        restricted: *mut VmAspace,
        name: &CStr,
    ) -> Option<RefPtr<VmAspace>> {
        unsafe {
            RefPtr::try_from_raw(cpp_vm_aspace_create_unified(shared, restricted, name.as_ptr()))
        }
    }

    /// Destroys this address space.
    ///
    /// `destroy` does not free this object, but rather allows it to be freed when the last
    /// retaining `RefPtr` is destroyed.
    pub fn destroy(&self) -> Result<(), Status> {
        Status::ok(unsafe { cpp_vm_aspace_destroy(self.to_mut_ptr()) })
    }

    /// Renames this address space.
    pub fn rename(&self, name: &CStr) {
        unsafe { cpp_vm_aspace_rename(self.to_mut_ptr(), name.as_ptr()) }
    }

    /// Returns the base virtual address of this address space.
    pub fn base(&self) -> usize {
        unsafe { cpp_vm_aspace_base(self.to_mut_ptr()) }
    }

    /// Returns the size in bytes of this address space.
    pub fn size(&self) -> usize {
        unsafe { cpp_vm_aspace_size(self.to_mut_ptr()) }
    }

    /// Returns the name of this address space.
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(cpp_vm_aspace_name(self.to_mut_ptr())) }
    }

    /// Returns a reference to the architecturally specific part of the address space
    /// (`ArchVmAspace`). This is internally locked and does not need to be guarded by `lock_`.
    pub fn arch_aspace(&self) -> &ArchVmAspace {
        unsafe { &*cpp_vm_aspace_arch_aspace(self.to_mut_ptr()) }
    }

    /// Returns true if this is a user address space (`Type::User`).
    pub fn is_user(&self) -> bool {
        unsafe { cpp_vm_aspace_is_user(self.to_mut_ptr()) }
    }

    /// Returns true if ASLR is enabled for this address space.
    pub fn is_aslr_enabled(&self) -> bool {
        unsafe { cpp_vm_aspace_is_aslr_enabled(self.to_mut_ptr()) }
    }

    /// Returns true if this address space has been destroyed.
    pub fn is_destroyed(&self) -> bool {
        unsafe { cpp_vm_aspace_is_destroyed(self.to_mut_ptr()) }
    }

    /// Returns the singleton kernel address space.
    pub fn kernel_aspace() -> &'static VmAspace {
        unsafe { &*cpp_vm_aspace_kernel_aspace() }
    }

    /// Returns the root address region (`RootVmar`) for this address space.
    pub fn root_vmar(&self) -> Option<RefPtr<VmAddressRegion>> {
        unsafe { RefPtr::try_from_raw(cpp_vm_aspace_root_vmar(self.to_mut_ptr())) }
    }

    /// Sets the per-thread address space pointer to this address space.
    pub fn attach_to_thread(&self, thread: ThreadPtr) {
        unsafe { cpp_vm_aspace_attach_to_thread(self.to_mut_ptr(), thread.as_raw()) }
    }

    /// Dumps information about this address space to the debug log.
    pub fn dump(&self, verbose: bool) {
        unsafe { cpp_vm_aspace_dump(self.to_mut_ptr(), verbose) }
    }

    /// Drops all user page tables across all user address spaces.
    pub fn drop_all_user_page_tables() {
        unsafe { cpp_vm_aspace_drop_all_user_page_tables() }
    }

    /// Drops all user page tables for this address space.
    pub fn drop_user_page_tables(&self) {
        unsafe { cpp_vm_aspace_drop_user_page_tables(self.to_mut_ptr()) }
    }

    /// Dumps all address spaces in the system.
    pub fn dump_all_aspaces(verbose: bool) {
        unsafe { cpp_vm_aspace_dump_all_aspaces(verbose) }
    }

    /// Harvests all accessed information across all user mappings and updates any page age
    /// information for terminal mappings, and potentially harvests page tables depending on the
    /// passed in action.
    ///
    /// This requires holding `aspaces_list_lock_` over the entire duration and
    /// whilst not a commonly used lock this function should still only be called infrequently to
    /// avoid monopolizing the lock.
    pub fn harvest_all_user_accessed_bits(
        non_terminal_action: NonTerminalAction,
        terminal_action: TerminalAction,
    ) {
        unsafe {
            cpp_vm_aspace_harvest_all_user_accessed_bits(non_terminal_action, terminal_action)
        }
    }

    /// Generates a soft fault against this address space.
    ///
    /// This is similar to `page_fault` except:
    /// * This address space may not currently be active and this does not have to be called from
    ///   the hardware exception handler.
    /// * May be invoked spuriously in situations where the hardware mappings would have prevented a
    ///   real `page_fault` from occurring.
    ///
    /// May block on page requests and must be called without locks held.
    pub fn soft_fault(&self, va: usize, flags: u32) -> Result<(), Status> {
        Status::ok(unsafe { cpp_vm_aspace_soft_fault(self.to_mut_ptr(), va, flags) })
    }

    /// Similar to `soft_fault`, but additionally takes a length indicating that the range of
    /// `[va, va+len)` is expected to be accessed with `flags` after resolving this fault. The
    /// address space can take this range as a hint to attempt to preemptively avoid future faults.
    ///
    /// There are no alignment restrictions on `va` or `len`, although it is assumed that `len` is
    /// greater than zero.
    pub fn soft_fault_in_range(&self, va: usize, flags: u32, len: usize) -> Result<(), Status> {
        Status::ok(unsafe { cpp_vm_aspace_soft_fault_in_range(self.to_mut_ptr(), va, flags, len) })
    }

    /// Generates an accessed flag fault against this address space.
    ///
    /// This is a specialized version of `soft_fault` that will only resolve a potential missing
    /// access flag and nothing else.
    pub fn accessed_fault(&self, va: usize) -> Result<(), Status> {
        Status::ok(unsafe { cpp_vm_aspace_accessed_fault(self.to_mut_ptr(), va) })
    }

    /// Page fault routine.
    ///
    /// Should only be called by the hypervisor or by `Thread::Current::Fault`.
    pub fn page_fault(&self, va: usize, flags: u32) -> Result<(), Status> {
        Status::ok(unsafe { cpp_vm_aspace_page_fault(self.to_mut_ptr(), va, flags) })
    }

    /// Legacy function to assist in the transition to VMARs.
    ///
    /// Assumes a flat VMAR structure in which all VMOs are mapped as children of the root.
    /// Will assert if used on user address spaces.
    ///
    /// # Safety
    ///
    /// The caller must ensure `ptr` points to a valid memory location that can hold the allocated
    /// address or specific starting address.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn alloc_physical(
        &self,
        name: &CStr,
        size: usize,
        ptr: *mut *mut c_void,
        align_pow2: u8,
        paddr: PAddr,
        vmm_flags: u32,
        arch_mmu_flags: ArchMmuFlags,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_vm_aspace_alloc_physical(
                self.to_mut_ptr(),
                name.as_ptr(),
                size,
                ptr,
                align_pow2,
                paddr,
                vmm_flags,
                arch_mmu_flags,
            )
        })
    }

    /// Legacy function to assist in the transition to VMARs.
    ///
    /// Assumes a flat VMAR structure in which all VMOs are mapped as children of the root.
    /// Will assert if used on user address spaces.
    ///
    /// # Safety
    ///
    /// The caller must ensure `ptr` points to a valid memory location that can hold the allocated
    /// address or specific starting address.
    pub unsafe fn alloc_contiguous(
        &self,
        name: &CStr,
        size: usize,
        ptr: *mut *mut c_void,
        align_pow2: u8,
        vmm_flags: u32,
        arch_mmu_flags: ArchMmuFlags,
    ) -> Result<(), Status> {
        Status::ok(unsafe {
            cpp_vm_aspace_alloc_contiguous(
                self.to_mut_ptr(),
                name.as_ptr(),
                size,
                ptr,
                align_pow2,
                vmm_flags,
                arch_mmu_flags,
            )
        })
    }

    /// Legacy function to assist in the transition to VMARs.
    ///
    /// Assumes a flat VMAR structure in which all VMOs are mapped as children of the root.
    /// Will assert if used on user address spaces.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the virtual address range being freed is no longer in use.
    pub unsafe fn free_region(&self, va: usize) -> Result<(), Status> {
        Status::ok(unsafe { cpp_vm_aspace_free_region(self.to_mut_ptr(), va) })
    }

    /// Internal use function for mapping VMOs.  Do not use.  This is exposed in
    /// the public API purely for tests.
    ///
    /// # Safety
    ///
    /// The caller must ensure that creating a mapping with the specified range and flags is sound.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn map_object_internal(
        &self,
        vmo: RefPtr<VmObject>,
        name: &CStr,
        offset: u64,
        size: usize,
        align_pow2: u8,
        vmm_flags: u32,
        arch_mmu_flags: ArchMmuFlags,
    ) -> Result<*mut c_void, Status> {
        let mut ptr = core::ptr::null_mut();
        Status::ok(unsafe {
            cpp_vm_aspace_map_object_internal(
                self.to_mut_ptr(),
                RefPtr::into_raw(vmo).cast_mut(),
                name.as_ptr(),
                offset,
                size,
                &mut ptr,
                align_pow2,
                vmm_flags,
                arch_mmu_flags,
            )
        })?;
        Ok(ptr)
    }

    /// Returns the vDSO base address for this address space.
    pub fn vdso_base_address(&self) -> usize {
        unsafe { cpp_vm_aspace_vdso_base_address(self.to_mut_ptr()) }
    }

    /// Returns the vDSO code address for this address space.
    pub fn vdso_code_address(&self) -> usize {
        unsafe { cpp_vm_aspace_vdso_code_address(self.to_mut_ptr()) }
    }

    /// Returns whether this address space is currently set to be a high memory priority.
    pub fn is_high_memory_priority(&self) -> bool {
        unsafe { cpp_vm_aspace_is_high_memory_priority(self.to_mut_ptr()) }
    }

    /// Finds the memory mapping for `vaddr` in this address space.
    pub fn find_mapping(&self, vaddr: usize) -> Option<RefPtr<VmMapping>> {
        // SAFETY: `self.to_mut_ptr()` is a valid pointer to this `VmAspace`.
        let ptr = unsafe { cpp_vm_aspace_find_mapping(self.to_mut_ptr(), vaddr) };
        // SAFETY: `ptr` is either null or was exported with an acquired refcount via `fbl::ExportToRawPtr`.
        unsafe { RefPtr::try_from_raw(ptr) }
    }
}
