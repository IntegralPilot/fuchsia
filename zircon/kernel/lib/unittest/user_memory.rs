// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;
use core::ops::Range;
use core::ptr::{with_exposed_provenance, with_exposed_provenance_mut};
use fbl::RefPtr;
use kprint::kprintln;
use zerocopy::{FromBytes, Immutable, IntoBytes};
use zx_status::Status;

use crate::kernel::thread;
use crate::user_copy::{UserInOutPtr, UserInPtr, UserOutPtr};
use crate::vm::arch_vm_aspace::{
    ARCH_MMU_FLAG_PERM_EXECUTE, ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_USER,
    ARCH_MMU_FLAG_PERM_WRITE, ArchMmuFlags,
};
use crate::vm::pmm;
use crate::vm::scanner::AutoVmScannerDisable;
use crate::vm::vm_address_region::{self, VmAddressRegion};
use crate::vm::vm_aspace::VmAspace;
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object::VmObject;
use crate::vm::vm_object_paged::VmObjectPaged;
use page::round_up;

/// UserMemory facilitates testing code that requires user memory.
///
/// Note: This struct does not share layout equivalence with the corresponding C++
/// `testing::UserMemory` class. This is intended to be constructed in and exclusively used
/// from Rust unittests.
///
/// # Example
/// ```rust
/// let mem = UserMemory::create(core::mem::size_of::<Thing>()).unwrap();
/// let mem_out = mem.user_out::<Thing>();
/// mem_out.copy_to_user(&thing).unwrap();
/// ```
pub struct UserMemory {
    mapping: RefPtr<VmMapping>,
    vmo: RefPtr<VmObject>,
    /// This is really only used on aarch64.
    tag: u8,
    /// User memory here is going to be touched directly by the kernel and will not have the option
    /// to fault in memory that should get reclaimed by the scanner. Therefore as long as we are
    /// using any UserMemory we should disable the scanner.
    _scanner_disable: AutoVmScannerDisable,
}

impl UserMemory {
    pub fn create(size: usize) -> Option<Self> {
        let size = round_up(size);
        match VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, size as u64) {
            Ok(vmo) => {
                let vmo = VmObjectPaged::into_vm_object(vmo);
                Self::create_from_vmo(vmo, 0, 0)
            }
            Err(status) => {
                kprintln!("VmObjectPaged::create failed: {}", status.into_raw());
                None
            }
        }
    }

    pub fn create_from_vmo(vmo: RefPtr<VmObject>, tag: u8, align_pow2: u8) -> Option<Self> {
        // active_aspace should always return the normal aspace as this is only run in the
        // unittests, which do not run threads in restricted mode. We assert this to be true by
        // checking that the restricted state is not set on this thread.
        debug_assert!(thread::current_restricted_state().is_null());
        // SAFETY: The current thread's active aspace reference is valid for the duration of this
        // call.
        let aspace = unsafe { thread::current_active_aspace() };
        let aspace = aspace.expect("thread active aspace");
        Self::create_in_aspace(vmo, aspace, tag, align_pow2)
    }

    pub fn create_in_aspace(
        vmo: RefPtr<VmObject>,
        aspace: &VmAspace,
        tag: u8,
        align_pow2: u8,
    ) -> Option<Self> {
        let root_vmar = aspace.root_vmar();
        let root_vmar = root_vmar.expect("aspace has root vmar");
        Self::create_in_vmar(vmo, root_vmar, tag, align_pow2)
    }

    pub fn create_in_vmar(
        vmo: RefPtr<VmObject>,
        vmar: RefPtr<VmAddressRegion>,
        tag: u8,
        align_pow2: u8,
    ) -> Option<Self> {
        debug_assert!(vmar.aspace().is_user());
        let size = vmo.size() as usize;
        let vmar_flags = vm_address_region::flag::CAN_MAP_READ
            | vm_address_region::flag::CAN_MAP_WRITE
            | vm_address_region::flag::CAN_MAP_EXECUTE;
        let arch_mmu_flags =
            ARCH_MMU_FLAG_PERM_USER | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

        let mapping_result = vmar.create_vm_mapping(
            /* mapping_offset= */ 0,
            size,
            align_pow2,
            vmar_flags,
            vmo.clone(),
            /* vmo_offset= */ 0,
            arch_mmu_flags,
            c"unittest",
        );

        match mapping_result {
            Ok(mapping_result) => Some(Self {
                mapping: mapping_result.mapping,
                vmo,
                tag,
                _scanner_disable: AutoVmScannerDisable::new(),
            }),
            Err(status) => {
                kprintln!("create_vm_mapping failed: {}", status.into_raw());
                None
            }
        }
    }

    pub fn base(&self) -> usize {
        #[cfg_attr(not(target_arch = "aarch64"), expect(unused_mut))]
        let mut base = self.mapping.base();
        #[cfg(target_arch = "aarch64")]
        {
            base |= usize::from(self.tag) << arch_arm64_vm_bindings::kTbiBit;
        }
        base
    }

    pub fn tag(&self) -> u8 {
        self.tag
    }

    pub fn vmo(&self) -> &RefPtr<VmObject> {
        &self.vmo
    }

    pub fn mapping(&self) -> &RefPtr<VmMapping> {
        &self.mapping
    }

    pub fn aspace(&self) -> &RefPtr<VmAspace> {
        self.mapping.aspace()
    }

    pub fn put<T>(&self, val: T, i: usize) -> Result<(), Status>
    where
        T: IntoBytes + Immutable,
    {
        self.user_out::<T>().element_offset(i).copy_to_user(&val)
    }

    pub fn get<T>(&self, i: usize) -> Result<T, Status>
    where
        T: FromBytes,
    {
        self.user_in::<T>().element_offset(i).read()
    }

    pub fn user_out<T>(&self) -> UserOutPtr<T> {
        UserOutPtr::new(with_exposed_provenance_mut::<T>(self.base()))
    }

    pub fn user_in<T>(&self) -> UserInPtr<T> {
        UserInPtr::new(with_exposed_provenance::<T>(self.base()))
    }

    pub fn user_in_out<T>(&self) -> UserInOutPtr<T> {
        UserInOutPtr::new(with_exposed_provenance_mut::<T>(self.base()))
    }

    /// Ensures the mapping is committed and mapped such that usages will cause no faults.
    pub fn commit_and_map(&self, range: Range<usize>) -> Result<(), Status> {
        let offset = range.start;
        let len = range.end - range.start;
        self.mapping
            .map_range(offset, len, /* commit= */ true, /* ignore_existing= */ false)
    }

    /// Map in any pages that are committed in the vmo. Ignores existing mappings allowing this to
    /// be called multiple times.
    pub fn map_existing(&self, range: Range<usize>) -> Result<(), Status> {
        let offset = range.start;
        let len = range.end - range.start;
        self.mapping
            .map_range(offset, len, /* commit= */ false, /* ignore_existing= */ true)
    }

    pub fn protect(&self, flags: ArchMmuFlags, offset: usize) -> Result<(), Status> {
        assert!(offset < self.mapping.size());
        self.mapping.debug_protect(
            self.mapping.base() + offset,
            self.mapping.size() - offset,
            ARCH_MMU_FLAG_PERM_USER | flags,
        )
    }

    /// Changes the mapping permissions to be a Read-Only Executable mapping.
    pub fn make_rx(&self) -> Result<(), Status> {
        self.protect(ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_EXECUTE, 0)
    }

    /// Read the underlying VMO directly, bypassing the mapping.
    pub fn vmo_read<'a>(
        &self,
        data: &'a mut [MaybeUninit<u8>],
        offset: u64,
    ) -> Result<&'a mut [u8], Status> {
        self.vmo.read(offset, data)
    }

    /// Write to the underlying VMO directly, bypassing the mapping.
    pub fn vmo_write(&self, data: &[u8], offset: u64) -> Result<(), Status> {
        self.vmo.write(offset, data)
    }
}

impl Drop for UserMemory {
    fn drop(&mut self) {
        let status = self.mapping.destroy();
        debug_assert_eq!(status, Ok(()));
    }
}
