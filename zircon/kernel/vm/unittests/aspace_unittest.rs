// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

mod sizes {
    pub const KB: usize = 1024;
    pub const MB: usize = 1024 * KB;
    pub const GB: usize = 1024 * MB;
}

/// Address space tests duplicated from aspace_unittest.cc.
#[cfg(ktest)]
#[unittest::suite]
mod aspace_rs {
    use super::sizes::GB;
    use crate::kernel::types::VAddr;
    use crate::user_memory::UserMemory;
    use crate::vm::arch_vm_aspace::{
        ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_USER, ArchMmuFlags,
    };
    use crate::vm::pmm;
    use crate::vm::scanner::AutoVmScannerDisable;
    use crate::vm::vm::vaddr_to_paddr;
    use crate::vm::vm_address_region::{self as vmar, MemoryPriority, VmAddressRegionOpChildren};
    use crate::vm::vm_aspace::{Type, VmAspace, vmm_flag};
    use crate::vm::vm_object::{Resizability, SnapshotType, VmObject};
    use crate::vm::vm_object_paged::VmObjectPaged;
    use crate::vm_unittests::test_helper::{
        ARCH_RW_FLAGS, ARCH_RW_USER_FLAGS, fill_and_test, make_committed_pager_vmo,
    };
    use core::mem::{MaybeUninit, size_of, size_of_val};
    use fbl::RefPtr;
    use kprint::kprintln;
    use page::SIZE as PAGE_SIZE_USIZE;
    use unittest::{
        assert_err, assert_nonnull, assert_ok, assert_true, expect_eq, expect_false, expect_ok,
        expect_true, unwrap_ok,
    };
    use zx_status::Status;

    const PAGE_SIZE: u64 = PAGE_SIZE_USIZE as u64;

    /// Allocates a contiguous region in kernel space, reads/writes it, then destroys it.
    #[test]
    fn vmm_alloc_contiguous_smoke_test() {
        let alloc_size = 256 * 1024;

        // allocate a region of memory
        let mut ptr = core::ptr::null_mut();
        let kaspace = VmAspace::kernel_aspace();
        let err = unsafe {
            kaspace.alloc_contiguous(
                c"test",
                alloc_size,
                &mut ptr,
                0,
                vmm_flag::COMMIT,
                ARCH_RW_FLAGS,
            )
        };
        assert_ok!(err, "VmAspace::AllocContiguous region of memory");
        assert_nonnull!(ptr, "VmAspace::AllocContiguous region of memory");

        // fill with known pattern and test
        let slice: *mut MaybeUninit<u8> = ptr.cast();
        // SAFETY: `ptr` points to `alloc_size` bytes of valid memory allocated in `kaspace`.
        let slice = unsafe { core::slice::from_raw_parts_mut(slice, alloc_size) };
        let (_buf, result) = fill_and_test(slice);
        expect_true!(result);

        // test that it is indeed contiguous
        kprintln!("testing that region is contiguous");
        let mut last_pa = 0u64;
        for i in 0..(alloc_size / PAGE_SIZE_USIZE) {
            // SAFETY: `ptr` points to a contiguous allocation of at least `alloc_size` bytes.
            let va = unsafe { ptr.add(i * PAGE_SIZE_USIZE) };
            let pa = u64::from(vaddr_to_paddr(va));
            if last_pa != 0 {
                expect_eq!(pa, last_pa + PAGE_SIZE, "region is contiguous");
            }
            last_pa = pa;
        }

        // free the region
        // SAFETY: `ptr as usize` is the base virtual address of the allocation in `kaspace`.
        let err = unsafe { kaspace.free_region(ptr as usize) };
        expect_ok!(err, "VmAspace::FreeRegion region of memory");
    }

    /// Checks that AllocContiguous fails when missing VMM_FLAG_COMMIT.
    #[test]
    fn vmm_alloc_contiguous_missing_flag_commit_fails() {
        // should have VmAspace::VMM_FLAG_COMMIT
        let zero_vmm_flags = 0u32;
        let mut ptr = core::ptr::null_mut();
        // SAFETY: `ptr` points to a valid pointer storage for `alloc_contiguous`.
        let err = unsafe {
            VmAspace::kernel_aspace().alloc_contiguous(
                c"test",
                PAGE_SIZE_USIZE,
                &mut ptr,
                0,
                zero_vmm_flags,
                ARCH_RW_FLAGS,
            )
        };
        assert_err!(err, Status::INVALID_ARGS);
    }

    /// Checks that AllocContiguous fails with zero size.
    #[test]
    fn vmm_alloc_contiguous_zero_size_fails() {
        let zero_size = 0usize;
        let mut ptr = core::ptr::null_mut();
        // SAFETY: `ptr` points to a valid pointer storage for `alloc_contiguous`.
        let err = unsafe {
            VmAspace::kernel_aspace().alloc_contiguous(
                c"test",
                zero_size,
                &mut ptr,
                0,
                vmm_flag::COMMIT,
                ARCH_RW_FLAGS,
            )
        };
        assert_err!(err, Status::INVALID_ARGS);
    }

    /// Allocates a vm address space object directly, allows it to go out of scope.
    #[test]
    fn vmaspace_create_smoke_test() {
        let aspace = VmAspace::create(Type::User, c"test aspace").expect("VmAspace::create failed");
        let err = aspace.destroy();
        expect_ok!(err, "VmAspace::Destroy");
    }

    /// Tests sparse VM mappings with an empty backing VMO.
    #[test]
    fn vm_mapping_sparse_mapping_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a large memory mapping with an empty backing VMO. Although this is a large virtual
        // address range, our later attempts to map it should be efficient.
        let memory_size = 16 * GB;
        let memory = UserMemory::create(memory_size).unwrap();

        // Memory backing the user memory is currently empty, so attempting to map in it should
        // succeed, albeit with nothing populated.
        expect_ok!(memory.map_existing(0..memory_size));

        // Commit a page in the middle, then re-map the whole thing and ensure the mapping is there.
        let val = 42u64;
        expect_ok!(
            memory.vmo_write(&val.to_ne_bytes()[..size_of_val(&val)], (memory_size / 2) as u64,)
        );
        expect_ok!(memory.map_existing(0..memory_size));
        expect_eq!(val, unwrap_ok!(memory.get::<u64>(memory_size / 2 / size_of::<u64>())));

        // Do the same test, but this time with the pages at the start and end of the range.
        expect_ok!(memory.vmo_write(&val.to_ne_bytes()[..size_of_val(&val)], 0));
        expect_ok!(memory.vmo_write(
            &val.to_ne_bytes()[..size_of_val(&val)],
            (memory_size - PAGE_SIZE_USIZE) as u64,
        ));
        expect_ok!(memory.map_existing(0..memory_size));
        expect_eq!(val, unwrap_ok!(memory.get::<u64>(0)));
        expect_eq!(
            val,
            unwrap_ok!(memory.get::<u64>((memory_size - PAGE_SIZE_USIZE) / size_of::<u64>()))
        );
    }

    /// Tests memory priority propagation with pager-backed VMOs and clones.
    #[test]
    fn vmaspace_priority_pager_test() {
        let aspace = VmAspace::create(Type::User, c"test-aspace");
        assert_true!(aspace.is_some());
        let aspace = aspace.unwrap();

        let vmar = unwrap_ok!(aspace.root_vmar().unwrap().create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 64,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test vmar",
        ));

        let status = vmar.set_memory_priority(MemoryPriority::HIGH);
        expect_ok!(status);

        let (vmo, _) = unwrap_ok!(make_committed_pager_vmo::<1>(false, false));

        // Create a clone of the VMO.
        let vmo_child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            PAGE_SIZE,
            true,
        ));
        let childp = VmObject::downcast_paged(vmo_child.clone()).expect("is paged");

        // Map in the clone.
        let _mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            PAGE_SIZE_USIZE,
            PAGE_SIZE_USIZE,
            0,
            vmar::flag::SPECIFIC_OVERWRITE,
            vmo_child.clone(),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));

        // Validate the root and clone received the priority.
        expect_true!(childp.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        // Create a second child of the root.
        let vmo_child2: Option<RefPtr<VmObject>> = None;
        let vmo_child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            PAGE_SIZE,
            true,
        ));
        let childp2 = VmObject::downcast_paged(vmo_child.clone()).expect("is paged");

        // This child should not have any priority.
        expect_false!(childp2.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        // Destroy it should leave the rest of the tree unchanged.
        drop(vmo_child2);
        expect_true!(childp.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        // Remove priority and validate.
        expect_ok!(vmar.set_memory_priority(MemoryPriority::DEFAULT));

        expect_false!(childp.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_false!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        expect_ok!(aspace.destroy());
    }

    /// Tests memory priority propagation for VMO references.
    #[test]
    fn vmaspace_priority_reference_test() {
        let aspace = VmAspace::create(Type::User, c"test-aspace").expect("VmAspace::create failed");

        let vmar = unwrap_ok!(aspace.root_vmar().unwrap().create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 64,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test vmar",
        ));

        let status = vmar.set_memory_priority(MemoryPriority::HIGH);
        expect_ok!(status);

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE * 2));

        let mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            PAGE_SIZE_USIZE,
            PAGE_SIZE_USIZE,
            0,
            vmar::flag::SPECIFIC_OVERWRITE,
            VmObjectPaged::into_vm_object(vmo.clone()),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));

        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        // Create a reference of the VMO.
        let (vmo_reference, _first_child) =
            unwrap_ok!(vmo.create_child_reference(Resizability::NonResizable, 0, 0, true));
        let refp = VmObject::downcast_paged(vmo_reference.clone()).expect("is paged");

        // Reference should have same priority.
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(refp.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        // Remove the original mapping.
        let _ = mapping_result.mapping.destroy();
        expect_false!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_false!(refp.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        // Now map in the reference.
        let mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            PAGE_SIZE_USIZE,
            PAGE_SIZE_USIZE,
            0,
            vmar::flag::SPECIFIC_OVERWRITE,
            vmo_reference,
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));
        let _ = mapping_result;

        // Reference and vmo should have same priority.
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(refp.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        expect_ok!(aspace.destroy());
    }

    /// Tests memory priority propagation through hierarchies.
    #[test]
    fn vmaspace_priority_propagation_test() {
        // Test that memory priority gets propagated through hierarchies and into newly created
        // objects.
        let aspace = VmAspace::create(Type::User, c"test-aspace");
        assert_true!(aspace.is_some());
        let aspace = aspace.unwrap();

        // Create VMAR and a VMO and map it in.
        let vmar = unwrap_ok!(aspace.root_vmar().unwrap().create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 64,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test vmar",
        ));

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE * 4));

        let _mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            0,
            PAGE_SIZE_USIZE * 4,
            0,
            0,
            VmObjectPaged::into_vm_object(vmo.clone()),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));

        // Set the priority in our vmar and validate it propagates to the VMO and the aspace.
        let status = vmar.set_memory_priority(MemoryPriority::HIGH);
        expect_ok!(status);

        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        // Create a new VMAR and VMO and map them into the high priority vmar. Memory priority
        // should propagate.
        let sub_vmar = unwrap_ok!(vmar.create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 16,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test sub-vmar",
        ));

        let vmo2 = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE * 4));

        let _mapping2_result = unwrap_ok!(sub_vmar.create_vm_mapping(
            0,
            PAGE_SIZE_USIZE * 4,
            0,
            0,
            VmObjectPaged::into_vm_object(vmo2.clone()),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));
        expect_true!(vmo2.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        // Change the priority of the sub vmar. It should not effect the original vmar / vmo
        // priority.
        let status = sub_vmar.set_memory_priority(MemoryPriority::DEFAULT);
        expect_ok!(status);
        expect_false!(vmo2.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        expect_ok!(vmar.destroy());
        expect_false!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_ok!(aspace.destroy());
    }

    /// Test that overwriting a mapping maintains priority counts.
    #[test]
    fn vmaspace_priority_mapping_overwrite_test() {
        let aspace = VmAspace::create(Type::User, c"test-aspace");
        assert_true!(aspace.is_some());
        let aspace = aspace.unwrap();

        // Create VMAR and a VMO and map it in.
        let vmar = unwrap_ok!(aspace.root_vmar().unwrap().create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 64,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test vmar",
        ));

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE));

        let mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            0,
            PAGE_SIZE_USIZE,
            0,
            0,
            VmObjectPaged::into_vm_object(vmo.clone()),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));
        let mapping = mapping_result.mapping;

        let status = vmar.set_memory_priority(MemoryPriority::HIGH);
        expect_ok!(status);

        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        // Overwrite the mapping with a new one from a new VMO.
        let vmo2 = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE));

        let _mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            mapping.base() - vmar.base().0,
            mapping.size(),
            0,
            vmar::flag::SPECIFIC_OVERWRITE,
            VmObjectPaged::into_vm_object(vmo2.clone()),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping2",
        ));

        // Original VMO should have lost its priority, and the VMO for our new mapping should have
        // gained.
        expect_false!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(vmo2.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        expect_ok!(aspace.destroy());
    }

    /// Test that unmapping parts of a mapping preserves priority.
    #[test]
    fn vmaspace_priority_unmap_test() {
        let aspace = VmAspace::create(Type::User, c"test-aspace");
        assert_true!(aspace.is_some());
        let aspace = aspace.unwrap();

        // Create VMAR and a VMO and map it in.
        let vmar = unwrap_ok!(aspace.root_vmar().unwrap().create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 64,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test vmar",
        ));

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE * 8));

        let mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            0,
            PAGE_SIZE_USIZE * 8,
            0,
            0,
            VmObjectPaged::into_vm_object(vmo.clone()),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));

        // Set the priority in our vmar and validate it propagates to the VMO and the aspace.
        let status = vmar.set_memory_priority(MemoryPriority::HIGH);
        expect_ok!(status);

        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        let base = mapping_result.base;

        // Unmap one page from either end of the mapping, ensuring memory priority did not change.
        expect_ok!(unsafe {
            vmar.unmap(VAddr(base), PAGE_SIZE_USIZE, VmAddressRegionOpChildren::No)
        });
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        expect_ok!(unsafe {
            vmar.unmap(
                VAddr(base + PAGE_SIZE_USIZE * 7),
                PAGE_SIZE_USIZE,
                VmAddressRegionOpChildren::No,
            )
        });
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        // Unmap a page from the middle. This will split this into two mappings.
        expect_ok!(unsafe {
            vmar.unmap(
                VAddr(base + PAGE_SIZE_USIZE * 4),
                PAGE_SIZE_USIZE,
                VmAddressRegionOpChildren::No,
            )
        });
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());
        // Now completely unmap one portion. This will destroy one of the mappings, but the VMO
        // should still have priority from the other mapping that was previously split.
        expect_ok!(unsafe {
            vmar.unmap(
                VAddr(base + PAGE_SIZE_USIZE),
                PAGE_SIZE_USIZE * 3,
                VmAddressRegionOpChildren::No,
            )
        });
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        // Unmapping the rest of the other portion should finally cause the priority to be removed.
        expect_ok!(unsafe {
            vmar.unmap(
                VAddr(base + PAGE_SIZE_USIZE * 5),
                PAGE_SIZE_USIZE * 2,
                VmAddressRegionOpChildren::No,
            )
        });
        expect_false!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        expect_ok!(aspace.destroy());
    }

    /// Tests memory priority propagation to a child VMO slice.
    #[test]
    fn vmaspace_priority_slice_test() {
        let aspace = VmAspace::create(Type::User, c"test-aspace");
        assert_true!(aspace.is_some());
        let aspace = aspace.unwrap();

        let vmar = unwrap_ok!(aspace.root_vmar().unwrap().create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 64,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test vmar",
        ));

        let status = vmar.set_memory_priority(MemoryPriority::HIGH);
        expect_ok!(status);

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE * 2));

        let _mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            PAGE_SIZE_USIZE,
            PAGE_SIZE_USIZE,
            0,
            vmar::flag::SPECIFIC_OVERWRITE,
            VmObjectPaged::into_vm_object(vmo.clone()),
            0,
            ARCH_RW_USER_FLAGS,
            c"test-mapping",
        ));

        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        // Create a slice of the VMO.
        let vmo_slice = unwrap_ok!(vmo.create_child_slice(0, PAGE_SIZE, true));
        let slicep = vmo_slice.as_paged().unwrap();

        // Slice inherits priority.
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(slicep.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        // Change priority of the VMAR should remove from the VMO.
        expect_ok!(vmar.set_memory_priority(MemoryPriority::DEFAULT));
        expect_false!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_false!(aspace.is_high_memory_priority());

        // Re-enable priority and verify.
        expect_ok!(vmar.set_memory_priority(MemoryPriority::HIGH));
        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(slicep.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
        expect_true!(aspace.is_high_memory_priority());

        // Destroy slice and unmap.
        drop(vmo_slice);

        expect_true!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());

        expect_ok!(aspace.destroy());
        expect_false!(vmo.debug_get_cow_pages().unwrap().debug_is_high_memory_priority());
    }

    /// Attempt force a high priority region to be writeable.
    #[test]
    fn vm_mapping_force_writeable_high_priority() {
        let aspace = VmAspace::create(Type::User, c"test-aspace");
        assert_true!(aspace.is_some());
        let aspace = aspace.unwrap();

        // Create VMAR and a VMO and map it in.
        let vmar = unwrap_ok!(aspace.root_vmar().unwrap().create_sub_vmar(
            0,
            PAGE_SIZE_USIZE * 64,
            0,
            vmar::flag::CAN_MAP_SPECIFIC | vmar::flag::CAN_MAP_READ | vmar::flag::CAN_MAP_WRITE,
            c"test vmar",
        ));

        struct CleanupSubVmar<'a>(&'a vmar::VmAddressRegion);
        impl Drop for CleanupSubVmar<'_> {
            fn drop(&mut self) {
                let _ = self.0.destroy();
            }
        }
        let _cleanup_sub_vmar = CleanupSubVmar(&vmar);

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE * 4));

        // Create a read-only user mapping. Since there is no ARCH_MMU_FLAG_PERM_WRITE,
        // force_writable won't trivially succeed.
        let arch_read_user_flags: ArchMmuFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_USER;
        let mapping_result = unwrap_ok!(vmar.create_vm_mapping(
            0,
            PAGE_SIZE_USIZE * 4,
            0,
            0,
            VmObjectPaged::into_vm_object(vmo),
            0,
            arch_read_user_flags,
            c"test-mapping",
        ));

        let status = vmar.set_memory_priority(vmar::MemoryPriority::HIGH);
        expect_ok!(status);

        let force_result = mapping_result.mapping.force_writable();
        expect_ok!(force_result.map(|_| ()));
    }

    /// Doesn't do anything, just prints all aspaces.
    #[test]
    fn dump_all_aspaces() {
        // Doesn't do anything, just prints all aspaces.
        // Should be run after all other tests so that people can manually comb
        // through the output for leaked test aspaces.

        // Set to true for debugging.
        if false {
            kprintln!("verify there are no test aspaces left around");
            VmAspace::dump_all_aspaces(/*verbose*/ true);
        }
    }
}
