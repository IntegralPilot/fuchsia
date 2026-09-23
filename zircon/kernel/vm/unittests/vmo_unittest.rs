// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// VMO tests duplicated from vmo_unittest.cc.
#[cfg(ktest)]
#[unittest::suite]
mod vmo_rs {
    use crate::kernel::thread::{self, ThreadPtr};
    use crate::kernel::types::PAddr;
    use crate::platform_rs::timer::InstantMono;
    use crate::user_memory::UserMemory;
    use crate::vm::arch_vm_aspace::{
        ARCH_MMU_FLAG_CACHE_MASK, ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE,
        ARCH_MMU_FLAG_UNCACHED, ARCH_MMU_FLAG_UNCACHED_DEVICE,
    };
    use crate::vm::attribution::{self, AttributionCounts};
    use crate::vm::compressor::VmCompressor;
    use crate::vm::discardable_vmo_tracker::{DiscardablePageCounts, DiscardableVmoTracker};
    use crate::vm::fault;
    use crate::vm::page::VmPagePtr;
    use crate::vm::page_queues::PageQueues;
    use crate::vm::page_source::MultiPageRequest;
    use crate::vm::physical_page_borrowing_config::ScopedLoaningEnabled;
    use crate::vm::physmap::paddr_to_physmap;
    use crate::vm::pinned_vm_object::PinnedVmObject;
    use crate::vm::pmm::{self, ALLOC_FLAG_ANY, paddr_to_vm_page};
    use crate::vm::pmm_node::PmmOptDelayReuse;
    use crate::vm::scanner::AutoVmScannerDisable;
    use crate::vm::vm_aspace::{VmAspace, vmm_flag};
    use crate::vm::vm_cow_pages::{EvictionAction, VmCowPages, VmCowRange, VmCowReclaimFailure};
    use crate::vm::vm_object::{EvictionHint, Resizability, SnapshotType, SupplyOptions, VmObject};
    use crate::vm::vm_object_paged::VmObjectPaged;
    use crate::vm::vm_object_physical::VmObjectPhysical;
    use crate::vm::vm_page_list::VmPageSpliceList;
    use crate::vm_unittests::test_helper::{
        ARCH_RW_FLAGS, TestRand, fill_and_test, fill_region, make_committed_pager_vmo,
        make_partially_committed_pager_vmo, make_private_attribution_counts,
        make_uncommitted_pager_vmo, supply_pager_vmo_pages, test_region,
        verify_continuous_attribution_bytes,
    };
    use core::ffi::c_void;
    use core::mem::MaybeUninit;
    use core::{ptr, slice};
    use debug::dprintf;
    use fbl::{RefPtr, Vector};
    use page::SIZE as PAGE_SIZE_USIZE;
    use pin_init::stack_pin_init;
    use rand::RngExt as _;
    use unittest::{
        assert_eq, assert_false, assert_ge, assert_le, assert_lt, assert_ok, assert_true,
        expect_eq, expect_false, expect_gt, expect_le, expect_ne, expect_ok, expect_true,
        unwrap_ok,
    };
    use zx_status::Status;
    use zx_types::ZX_KOID_KERNEL;

    const PAGE_SIZE: u64 = PAGE_SIZE_USIZE as u64;

    /// Helper that tests if all pages in a VMO in the specified range pass the given predicate.
    ///
    /// # Safety
    ///
    /// The caller must ensure that pages attached to the VM object remain attached for the duration
    /// of the callback.
    unsafe fn all_pages_match<F>(vmo: &VmObject, offset: u64, len: u64, mut pred: F) -> bool
    where
        F: FnMut(VmPagePtr) -> bool,
    {
        let mut pred_matches = true;
        let res = vmo.lookup(
            offset,
            len,
            &mut (&mut pred, &mut pred_matches),
            |_, pa, (pred, pred_matches)| {
                let page = paddr_to_vm_page(pa).expect("paddr must map to vm_page");
                if !pred(page) {
                    **pred_matches = false;
                    Err(Status::STOP)
                } else {
                    Err(Status::NEXT)
                }
            },
        );
        res.is_ok() && pred_matches
    }

    /// # Safety
    ///
    /// The caller must ensure that pages attached to `vmo` remain attached during the check.
    unsafe fn pages_in_wired_queue(vmo: &VmObject, offset: u64, len: u64) -> bool {
        // SAFETY: Caller guarantees pages stay attached to the VMO.
        unsafe {
            all_pages_match(vmo, offset, len, |page| pmm::page_queues().debug_page_is_wired(page))
        }
    }

    /// # Safety
    ///
    /// The caller must ensure that pages attached to `vmo` remain attached during the check.
    unsafe fn pages_in_any_anonymous_queue(vmo: &VmObject, offset: u64, len: u64) -> bool {
        // SAFETY: Caller guarantees pages stay attached to the VMO.
        unsafe {
            all_pages_match(vmo, offset, len, |page| {
                pmm::page_queues().debug_page_is_any_anonymous(page)
            })
        }
    }

    /// # Safety
    ///
    /// `page` must be associated with a VM object.
    unsafe fn evict_loaned_page(vmo: &VmObjectPaged, page: VmPagePtr, offset: u64) -> bool {
        // SAFETY: Caller guarantees `page` is associated with a VM object.
        let status = unsafe {
            vmo.debug_get_cow_pages()
                .expect("paged VMO has backing cow pages")
                .evict_loaned_page(page, offset)
        };
        status.is_ok()
    }

    /// Helper wrapper around reclaiming a page that returns the pages to the pmm.
    ///
    /// # Safety
    ///
    /// The caller must know that it is sound to reclaim `page` at `offset`.
    unsafe fn reclaim_cow_pages(
        cow_pages: &VmCowPages,
        page: VmPagePtr,
        offset: u64,
        hint_action: EvictionAction,
        compressor: Option<&VmCompressor>,
    ) -> u64 {
        // SAFETY: Caller knows it is sound to reclaim `page` at `offset`.
        let reclaimed = unsafe { cow_pages.reclaim_page(page, offset, hint_action, compressor) };
        if let Ok(success) = reclaimed { success.num_pages } else { 0 }
    }

    /// Simulates the reclamation thread.
    ///
    /// # Safety
    ///
    /// The caller must know that it is sound to reclaim `page` at `offset`.
    unsafe fn reclaim(
        vmo: &VmObjectPaged,
        page: VmPagePtr,
        offset: u64,
        hint_action: EvictionAction,
    ) -> u64 {
        // Move to 'DontNeed' unless the page is dirty, as dirty pages should never be in the
        // isolate queue.
        // SAFETY: `page` is attached to `vmo`.
        if !unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) } {
            // SAFETY: `page` is attached to `vmo`.
            unsafe {
                pmm::page_queues().move_to_reclaim_dont_need(page);
            }
        }
        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        // SAFETY: Caller knows it is sound to reclaim `page` at `offset`.
        unsafe { reclaim_cow_pages(&cow_pages, page, offset, hint_action, None) }
    }

    /// Creates a vm object.
    #[test]
    fn vmo_create_test() {
        // Creates a vm object.
        let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));
        // vmo is not contig
        expect_false!(vmo.is_contiguous());
        // vmo is not resizable
        expect_false!(vmo.is_resizable());
    }

    /// Tests creating a VMO with maximum size and larger than maximum size.
    #[test]
    fn vmo_create_maximum_size() {
        let vmo = VmObjectPaged::create(ALLOC_FLAG_ANY, 0, VmObject::MAX_SIZE);
        // should be ok
        expect_ok!(vmo.map(|_| ()));

        let vmo = VmObjectPaged::create(ALLOC_FLAG_ANY, 0, VmObject::MAX_SIZE + PAGE_SIZE);
        // should be too large
        expect_eq!(Status::result_into_raw(vmo.map(|_| ())), Status::OUT_OF_RANGE.into_raw());
    }

    /// Creates a vm object, commits memory.
    #[test]
    fn vmo_commit_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = PAGE_SIZE * 16;
        // vmobject creation
        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size));

        let ret = vmo.commit_range(0, alloc_size);
        // committing vm object
        expect_ok!(ret);
        expect_true!(make_private_attribution_counts(alloc_size, 0) == vmo.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo, alloc_size));
        // SAFETY: Pages remain attached to `vmo` during assertion.
        expect_true!(unsafe { pages_in_any_anonymous_queue(&vmo, 0, alloc_size) });
    }

    /// Checks that VMOs must be page aligned sizes.
    #[test]
    fn vmo_unaligned_size_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = 15;
        let result = VmObjectPaged::create(ALLOC_FLAG_ANY, 0, alloc_size);
        assert_eq!(Status::result_into_raw(result.map(|_| ())), Status::INVALID_ARGS.into_raw());
    }

    /// Checks that attribution via reference doesn't attribute pages unless specifically requested.
    #[test]
    fn vmo_reference_attribution_commit_test() {
        // Creates a vm object, checks that attribution via reference doesn't attribute pages
        // unless we specifically request it.
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = 8 * PAGE_SIZE;
        let vmo = unwrap_ok!(
            VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size),
            "vmobject creation\n"
        );

        let (vmo_reference, _first_child) = unwrap_ok!(
            vmo.create_child_reference(Resizability::NonResizable, 0, 0, true),
            "vmobject reference creation\n"
        );

        let ret = vmo.commit_range(0, alloc_size);
        expect_ok!(ret, "committing vm object\n");
        expect_true!(make_private_attribution_counts(alloc_size, 0) == vmo.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo, alloc_size));

        expect_true!(
            attribution::zero() == vmo_reference.get_attributed_memory(),
            "vmo_reference attribution\n"
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo_reference, alloc_size));

        expect_true!(
            make_private_attribution_counts(alloc_size, 0)
                == vmo_reference.get_attributed_memory_in_reference_owner(),
            "vmo_reference explicit reference attribution\n"
        );
    }

    /// Tests creating a physical VMO and checking its initial properties.
    #[test]
    fn vmo_create_physical_test() {
        // vm page allocation
        let (vm_page, pa) = unwrap_ok!(pmm::alloc_page(0));

        // vmobject creation
        let vmo = unwrap_ok!(VmObjectPhysical::create(pa, PAGE_SIZE_USIZE));
        let cache_policy = vmo.get_mapping_cache_policy();
        // check initial cache policy
        expect_eq!(ARCH_MMU_FLAG_UNCACHED, cache_policy);
        // check contiguous
        expect_true!(vmo.is_contiguous());

        drop(vmo);
        // SAFETY: vm_page was allocated via alloc_page above and is no longer referenced by vmo.
        unsafe { pmm::free_page(vm_page) };
    }

    /// Tests pinning ranges in a physical VMO.
    #[test]
    fn vmo_physical_pin_test() {
        let (vm_page, pa) = unwrap_ok!(pmm::alloc_page(0));

        let vmo = unwrap_ok!(VmObjectPhysical::create(pa, PAGE_SIZE_USIZE));

        // Validate we can pin the range.
        expect_ok!(vmo.commit_range_pinned(0, PAGE_SIZE, false));

        // Pinning out side should fail.
        expect_eq!(
            Status::result_into_raw(vmo.commit_range_pinned(PAGE_SIZE, PAGE_SIZE, false)),
            Status::OUT_OF_RANGE.into_raw()
        );

        // Unpin for physical VMOs does not currently do anything, but still call it to be API correct.
        vmo.unpin(0, PAGE_SIZE);

        drop(vmo);
        // SAFETY: vm_page was allocated via alloc_page above and is no longer referenced by vmo.
        unsafe { pmm::free_page(vm_page) };
    }

    /// Creates a vm object that commits contiguous memory.
    #[test]
    fn vmo_create_contiguous_test() {
        let alloc_size = PAGE_SIZE * 16;
        let vmo = unwrap_ok!(
            VmObjectPaged::create_contiguous(pmm::ALLOC_FLAG_ANY, alloc_size, 0),
            "vmobject creation\n"
        );

        expect_true!(vmo.is_contiguous(), "vmo is contig\n");

        // Contiguous VMOs are not pinned, but they are notionally wired as they will not be
        // automatically manipulated by the kernel.
        // SAFETY: Test owns `vmo` and pages remain attached during check.
        expect_true!(unsafe { pages_in_wired_queue(&vmo, 0, alloc_size) });

        let mut last_pa = PAddr(0);
        let lookup_func = |offset, pa: PAddr, last_pa: &mut PAddr| {
            if offset != 0 && PAddr(last_pa.0 + PAGE_SIZE as usize) != pa {
                return Err(Status::BAD_STATE);
            }
            *last_pa = pa;
            Err(Status::NEXT)
        };
        let status = vmo.lookup(0, alloc_size, &mut last_pa, lookup_func);
        expect_ok!(status, "vmo lookup\n");
        let first_pa = unwrap_ok!(vmo.lookup_contiguous(0, alloc_size));
        expect_eq!(first_pa.0 + alloc_size as usize - PAGE_SIZE as usize, last_pa.0);
        let second_pa = unwrap_ok!(vmo.lookup_contiguous(PAGE_SIZE, PAGE_SIZE));
        expect_eq!(first_pa.0 + PAGE_SIZE as usize, second_pa.0);
        expect_eq!(
            Status::INVALID_ARGS.into_raw(),
            Status::result_into_raw(vmo.lookup_contiguous(42, PAGE_SIZE).map(|_| ()))
        );
        expect_eq!(
            Status::OUT_OF_RANGE.into_raw(),
            Status::result_into_raw(
                vmo.lookup_contiguous(alloc_size - PAGE_SIZE, PAGE_SIZE * 2).map(|_| ())
            )
        );
    }

    /// Tests pinning and decommitting ranges in a Paged VMO.
    #[test]
    fn vmo_pin_test() {
        // Creates paged VMOs, pins them, and tries operations that should unpin.
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = PAGE_SIZE * 16;
        for is_loaning_enabled in [false, true] {
            let _loaning_guard = ScopedLoaningEnabled::new(is_loaning_enabled);

            // vmobject creation
            let vmo = unwrap_ok!(VmObjectPaged::create(
                ALLOC_FLAG_ANY,
                VmObjectPaged::RESIZABLE,
                alloc_size
            ));

            // pinning out of range
            expect_eq!(
                Status::result_into_raw(vmo.commit_range_pinned(PAGE_SIZE, alloc_size, false)),
                Status::OUT_OF_RANGE.into_raw()
            );
            // pinning range of len 0
            expect_eq!(
                Status::result_into_raw(vmo.commit_range_pinned(PAGE_SIZE, 0, false)),
                Status::INVALID_ARGS.into_raw()
            );

            // pinning range
            expect_ok!(vmo.commit_range_pinned(PAGE_SIZE, 3 * PAGE_SIZE, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, PAGE_SIZE, 3 * PAGE_SIZE) });

            // decommitting pinned range
            expect_eq!(
                Status::result_into_raw(vmo.decommit_range(PAGE_SIZE, 3 * PAGE_SIZE)),
                Status::BAD_STATE.into_raw()
            );
            // decommitting pinned range
            expect_eq!(
                Status::result_into_raw(vmo.decommit_range(PAGE_SIZE, PAGE_SIZE)),
                Status::BAD_STATE.into_raw()
            );
            // decommitting pinned range
            expect_eq!(
                Status::result_into_raw(vmo.decommit_range(3 * PAGE_SIZE, PAGE_SIZE)),
                Status::BAD_STATE.into_raw()
            );

            vmo.unpin(PAGE_SIZE, 3 * PAGE_SIZE);
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_any_anonymous_queue(&vmo, PAGE_SIZE, 3 * PAGE_SIZE) });

            // decommitting unpinned range
            expect_ok!(vmo.decommit_range(PAGE_SIZE, 3 * PAGE_SIZE));

            // pinning range after decommit
            expect_ok!(vmo.commit_range_pinned(PAGE_SIZE, 3 * PAGE_SIZE, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, PAGE_SIZE, 3 * PAGE_SIZE) });

            // resizing pinned range
            expect_eq!(Status::result_into_raw(vmo.resize(0)), Status::BAD_STATE.into_raw());

            vmo.unpin(PAGE_SIZE, 3 * PAGE_SIZE);

            // resizing unpinned range
            expect_ok!(vmo.resize(0));
        }
    }

    /// Creates contiguous VMOs, pins them, and tries operations that should unpin.
    #[test]
    fn vmo_pin_contiguous_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = PAGE_SIZE * 16;
        for is_loaning_enabled in [false, true] {
            let _loaning_guard = ScopedLoaningEnabled::new(is_loaning_enabled);

            // vmobject creation
            let vmo = unwrap_ok!(VmObjectPaged::create_contiguous(ALLOC_FLAG_ANY, alloc_size, 0));

            // pinning out of range
            expect_eq!(
                Status::result_into_raw(vmo.commit_range_pinned(PAGE_SIZE, alloc_size, false)),
                Status::OUT_OF_RANGE.into_raw()
            );
            // pinning range of len 0
            expect_eq!(
                Status::result_into_raw(vmo.commit_range_pinned(PAGE_SIZE, 0, false)),
                Status::INVALID_ARGS.into_raw()
            );

            // pinning range
            expect_ok!(vmo.commit_range_pinned(PAGE_SIZE, 3 * PAGE_SIZE, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, PAGE_SIZE, 3 * PAGE_SIZE) });

            // decommitting pinned range
            let status = vmo.decommit_range(PAGE_SIZE, 3 * PAGE_SIZE);
            if !is_loaning_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_eq!(Status::result_into_raw(status), Status::BAD_STATE.into_raw());
            }
            let status = vmo.decommit_range(PAGE_SIZE, PAGE_SIZE);
            if !is_loaning_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_eq!(Status::result_into_raw(status), Status::BAD_STATE.into_raw());
            }
            let status = vmo.decommit_range(3 * PAGE_SIZE, PAGE_SIZE);
            if !is_loaning_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_eq!(Status::result_into_raw(status), Status::BAD_STATE.into_raw());
            }

            vmo.unpin(PAGE_SIZE, 3 * PAGE_SIZE);
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, PAGE_SIZE, 3 * PAGE_SIZE) });

            // decommitting unpinned range
            let status = vmo.decommit_range(PAGE_SIZE, 3 * PAGE_SIZE);
            if !is_loaning_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_ok!(status);
            }

            // pinning range after decommit
            expect_ok!(vmo.commit_range_pinned(PAGE_SIZE, 3 * PAGE_SIZE, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, PAGE_SIZE, 3 * PAGE_SIZE) });

            vmo.unpin(PAGE_SIZE, 3 * PAGE_SIZE);
        }
    }

    /// Tests multiple pin calls on the same pages up to the maximum pin count.
    #[test]
    fn vmo_multiple_pin_test() {
        // Creates a page VMO and pins the same pages multiple times.
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = PAGE_SIZE * 16;
        for is_ppb_enabled in [false, true] {
            let _loaning_guard = ScopedLoaningEnabled::new(is_ppb_enabled);

            // vmobject creation
            let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, alloc_size));

            // pinning whole range
            expect_ok!(vmo.commit_range_pinned(0, alloc_size, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, 0, alloc_size) });
            // pinning subrange
            expect_ok!(vmo.commit_range_pinned(PAGE_SIZE, 4 * PAGE_SIZE, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, 0, alloc_size) });

            for _ in 1..crate::vm::page::object::MAX_PIN_COUNT {
                // pinning first page max times
                expect_ok!(vmo.commit_range_pinned(0, PAGE_SIZE, false));
            }
            // page is pinned too much
            expect_eq!(
                Status::result_into_raw(vmo.commit_range_pinned(0, PAGE_SIZE, false)),
                Status::UNAVAILABLE.into_raw()
            );

            vmo.unpin(0, alloc_size);
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, PAGE_SIZE, 4 * PAGE_SIZE) });
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe {
                pages_in_any_anonymous_queue(&vmo, 5 * PAGE_SIZE, alloc_size - 5 * PAGE_SIZE)
            });
            // decommitting pinned range
            expect_eq!(
                Status::result_into_raw(vmo.decommit_range(PAGE_SIZE, 4 * PAGE_SIZE)),
                Status::BAD_STATE.into_raw()
            );
            // decommitting unpinned range
            expect_ok!(vmo.decommit_range(5 * PAGE_SIZE, alloc_size - 5 * PAGE_SIZE));

            vmo.unpin(PAGE_SIZE, 4 * PAGE_SIZE);
            // decommitting unpinned range
            expect_ok!(vmo.decommit_range(PAGE_SIZE, 4 * PAGE_SIZE));

            for _ in 2..crate::vm::page::object::MAX_PIN_COUNT {
                vmo.unpin(0, PAGE_SIZE);
            }
            // decommitting unpinned range
            expect_eq!(
                Status::result_into_raw(vmo.decommit_range(0, PAGE_SIZE)),
                Status::BAD_STATE.into_raw()
            );

            vmo.unpin(0, PAGE_SIZE);
            // decommitting unpinned range
            expect_ok!(vmo.decommit_range(0, PAGE_SIZE));
        }
    }

    /// Creates a contiguous VMO and pins the same pages multiple times.
    #[test]
    fn vmo_multiple_pin_contiguous_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = PAGE_SIZE * 16;
        for is_ppb_enabled in [false, true] {
            let _loaning_guard = ScopedLoaningEnabled::new(is_ppb_enabled);

            // vmobject creation
            let vmo = unwrap_ok!(VmObjectPaged::create_contiguous(ALLOC_FLAG_ANY, alloc_size, 0));

            // pinning whole range
            expect_ok!(vmo.commit_range_pinned(0, alloc_size, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, 0, alloc_size) });
            // pinning subrange
            expect_ok!(vmo.commit_range_pinned(PAGE_SIZE, 4 * PAGE_SIZE, false));
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, 0, alloc_size) });

            for _ in 1..crate::vm::page::object::MAX_PIN_COUNT {
                // pinning first page max times
                expect_ok!(vmo.commit_range_pinned(0, PAGE_SIZE, false));
            }
            // page is pinned too much
            expect_eq!(
                Status::result_into_raw(vmo.commit_range_pinned(0, PAGE_SIZE, false)),
                Status::UNAVAILABLE.into_raw()
            );

            vmo.unpin(0, alloc_size);
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe { pages_in_wired_queue(&vmo, PAGE_SIZE, 4 * PAGE_SIZE) });
            // SAFETY: Test owns vmo and pages remain attached during check.
            expect_true!(unsafe {
                pages_in_wired_queue(&vmo, 5 * PAGE_SIZE, alloc_size - 5 * PAGE_SIZE)
            });

            let status = vmo.decommit_range(PAGE_SIZE, 4 * PAGE_SIZE);
            if !is_ppb_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_eq!(Status::result_into_raw(status), Status::BAD_STATE.into_raw());
            }
            let status = vmo.decommit_range(5 * PAGE_SIZE, alloc_size - 5 * PAGE_SIZE);
            if !is_ppb_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_ok!(status);
            }

            vmo.unpin(PAGE_SIZE, 4 * PAGE_SIZE);
            let status = vmo.decommit_range(PAGE_SIZE, 4 * PAGE_SIZE);
            if !is_ppb_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_ok!(status);
            }

            for _ in 2..crate::vm::page::object::MAX_PIN_COUNT {
                vmo.unpin(0, PAGE_SIZE);
            }
            let status = vmo.decommit_range(0, PAGE_SIZE);
            if !is_ppb_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_eq!(Status::result_into_raw(status), Status::BAD_STATE.into_raw());
            }

            vmo.unpin(0, PAGE_SIZE);
            let status = vmo.decommit_range(0, PAGE_SIZE);
            if !is_ppb_enabled {
                expect_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
            } else {
                expect_ok!(status);
            }
        }
    }

    /// Creates a vm object, maps it, drops ref before unmapping.
    #[test]
    fn vmo_dropped_ref_test() {
        let alloc_size = 16 * PAGE_SIZE_USIZE;
        let vmo = unwrap_ok!(
            VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size as u64),
            "vmobject creation\n"
        );

        let ka = VmAspace::kernel_aspace();
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo),
                    c"test",
                    0,
                    alloc_size,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object"
            )
        };
        let ptr: *mut MaybeUninit<u8> = ptr.cast();
        // SAFETY: `ptr` points to `alloc_size` bytes of memory mapped into `ka`.
        let ptr = unsafe { slice::from_raw_parts_mut(ptr, alloc_size) };

        // fill with known pattern and test
        let (ptr, result) = fill_and_test(ptr);
        expect_true!(result);

        // SAFETY: `ptr.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        let err = unsafe { ka.free_region(ptr.as_ptr() as usize) };
        expect_ok!(err, "unmapping object");
    }

    /// Creates a vm object, maps it, fills it with data, unmaps, maps again somewhere else.
    #[test]
    fn vmo_remap_test() {
        let alloc_size = 16 * PAGE_SIZE_USIZE;
        let vmo = unwrap_ok!(
            VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size as u64),
            "vmobject creation\n"
        );

        let ka = VmAspace::kernel_aspace();
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo.clone()),
                    c"test",
                    0,
                    alloc_size,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object"
            )
        };
        let ptr: *mut MaybeUninit<u8> = ptr.cast();
        // SAFETY: `ptr` points to `alloc_size` bytes of memory mapped into `ka`.
        let ptr = unsafe { slice::from_raw_parts_mut(ptr, alloc_size) };

        // fill with known pattern and test.  The initial virtual address will be used
        // to generate the seed which is used to generate the fill pattern.  Make sure
        // we save it off right now to use when we test the fill pattern later on
        // after re-mapping.
        let fill_seed = ptr.as_ptr().addr();
        let (ptr, result) = fill_and_test(ptr);
        expect_true!(result);

        // SAFETY: `ptr.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        let err = unsafe { ka.free_region(ptr.as_ptr() as usize) };
        expect_ok!(err, "unmapping object");

        // map it again
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo),
                    c"test",
                    0,
                    alloc_size,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object"
            )
        };
        let ptr: *mut u8 = ptr.cast();
        // SAFETY: `ptr` points to `alloc_size` bytes of memory mapped into `ka`.
        let ptr = unsafe { slice::from_raw_parts(ptr, alloc_size) };

        // test that the pattern is still valid.  Be sure to use the original seed we
        // saved off earlier when verifying.
        let result = test_region(fill_seed, ptr);
        expect_true!(result, "testing region for corruption");

        // SAFETY: `ptr.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        let err = unsafe { ka.free_region(ptr.as_ptr() as usize) };
        expect_ok!(err, "unmapping object");
    }

    /// Tests mapping a VMO multiple times simultaneously.
    #[test]
    fn vmo_double_remap_test() {
        // Creates a vm object, maps it, fills it with data, maps it a second time and
        // third time somwehere else.
        let alloc_size = 16 * PAGE_SIZE_USIZE;
        let vmo = unwrap_ok!(
            VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size as u64),
            "vmobject creation\n"
        );

        let ka = VmAspace::kernel_aspace();
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo.clone()),
                    c"test0",
                    0,
                    alloc_size,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object"
            )
        };
        let ptr: *mut MaybeUninit<u8> = ptr.cast();
        // SAFETY: `ptr` points to `alloc_size` bytes of memory mapped into `ka`.
        let ptr = unsafe { slice::from_raw_parts_mut(ptr, alloc_size) };

        // fill with known pattern and test
        let (ptr, result) = fill_and_test(ptr);
        expect_true!(result);

        // map it again
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr2 = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo.clone()),
                    c"test1",
                    0,
                    alloc_size,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object second time"
            )
        };
        let ptr2: *mut u8 = ptr2.cast();
        // SAFETY: `ptr2` points to `alloc_size` bytes of memory mapped into `ka`.
        let ptr2 = unsafe { slice::from_raw_parts(ptr2, alloc_size) };
        expect_ne!(ptr.as_ptr(), ptr2.as_ptr(), "second mapping is different");

        // test that the pattern is still valid
        let result = test_region(ptr.as_ptr().addr(), ptr2);
        expect_true!(result, "testing region for corruption");

        // map it a third time with an offset
        let alloc_offset = PAGE_SIZE_USIZE;
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr3 = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo),
                    c"test2",
                    alloc_offset as u64,
                    alloc_size - alloc_offset,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object third time"
            )
        };
        let ptr3: *mut u8 = ptr3.cast();
        // SAFETY: `ptr3` points to `alloc_size - alloc_offset` bytes of memory mapped into `ka`.
        let ptr3 = unsafe { slice::from_raw_parts(ptr3, alloc_size - alloc_offset) };
        expect_ne!(ptr3.as_ptr(), ptr2.as_ptr(), "third mapping is different");
        expect_ne!(ptr3.as_ptr(), ptr.as_ptr(), "third mapping is different");

        // test that the pattern is still valid
        expect_true!(
            ptr[alloc_offset..alloc_size] == ptr3[..alloc_size - alloc_offset],
            "testing region for corruption"
        );

        // SAFETY: `ptr3.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        let ret = unsafe { ka.free_region(ptr3.as_ptr() as usize) };
        expect_ok!(ret, "unmapping object third time");

        // SAFETY: `ptr2.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        let ret = unsafe { ka.free_region(ptr2.as_ptr() as usize) };
        expect_ok!(ret, "unmapping object second time");

        // SAFETY: `ptr.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        let ret = unsafe { ka.free_region(ptr.as_ptr() as usize) };
        expect_ok!(ret, "unmapping object");
    }

    /// Tests basic read, write, and kernel mapping operations on a paged VMO.
    #[test]
    fn vmo_read_write_smoke_test() {
        let alloc_size = 16 * PAGE_SIZE_USIZE;

        // create object
        let vmo = unwrap_ok!(
            VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size as u64),
            "vmobject creation\n"
        );

        // create test buffer
        let mut a = Vector::<MaybeUninit<u8>>::new();
        assert_true!(a.resize_with(alloc_size + 47, MaybeUninit::uninit).is_ok());
        let a = fill_region(99, &mut a);

        // write to it, make sure it seems to work with valid args
        let err = vmo.write(0, &a[..0]);
        expect_ok!(err, "writing to object");

        let err = vmo.write(0, &a[..37]);
        expect_ok!(err, "writing to object");

        let err = vmo.write(99, &a[..37]);
        expect_ok!(err, "writing to object");

        // can't write past end
        let err = vmo.write(0, &a[..alloc_size + 47]);
        expect_eq!(
            Status::result_into_raw(err),
            Status::OUT_OF_RANGE.into_raw(),
            "writing to object"
        );

        // can't write past end
        let err = vmo.write(31, &a[..alloc_size + 47]);
        expect_eq!(
            Status::result_into_raw(err),
            Status::OUT_OF_RANGE.into_raw(),
            "writing to object"
        );

        // should return an error because out of range
        let err = vmo.write((alloc_size + 99) as u64, &a[..42]);
        expect_eq!(
            Status::result_into_raw(err),
            Status::OUT_OF_RANGE.into_raw(),
            "writing to object"
        );

        // map the object
        let ka = VmAspace::kernel_aspace();
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo.clone()),
                    c"test",
                    0,
                    alloc_size,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object"
            )
        };
        let ptr: *mut u8 = ptr.cast();
        // SAFETY: `ptr` points to `alloc_size` bytes of memory mapped into `ka`.
        let ptr = unsafe { slice::from_raw_parts(ptr, alloc_size) };

        // write to it at odd offsets
        let err = vmo.write(31, &a[..4197]);
        expect_ok!(err, "writing to object");
        expect_true!(ptr[31..31 + 4197] == a[..4197], "reading from object");

        // write to it, filling the object completely
        let err = vmo.write(0, &a[..alloc_size]);
        expect_ok!(err, "writing to object");

        // test that the data was actually written to it
        let result = test_region(99, ptr);
        expect_true!(result, "writing to object");

        // unmap it
        // SAFETY: `ptr.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        expect_ok!(unsafe { ka.free_region(ptr.as_ptr() as usize) });

        // test that we can read from it
        let mut b = Vector::<MaybeUninit<u8>>::new();
        assert_true!(b.resize_with(alloc_size, MaybeUninit::uninit).is_ok());

        let b_init = unwrap_ok!(vmo.read(0, &mut b), "reading from object");

        // validate the buffer is valid
        expect_true!(b_init == &a[..alloc_size], "reading from object");

        // read from it at an offset
        let b_init = unwrap_ok!(vmo.read(31, &mut b[..4197]), "reading from object");
        expect_true!(b_init == &a[31..31 + 4197], "reading from object");
    }

    /// Tests setting and querying mapping cache policy on physical VMOs.
    #[test]
    fn vmo_cache_test() {
        let (vm_page, pa) = unwrap_ok!(pmm::alloc_page(0));
        let ka = VmAspace::kernel_aspace();
        let cache_policy = ARCH_MMU_FLAG_UNCACHED_DEVICE;

        // Test that the flags set/get properly
        {
            let vmo =
                unwrap_ok!(VmObjectPhysical::create(pa, PAGE_SIZE_USIZE), "vmobject creation\n");
            let mut cache_policy_get = vmo.get_mapping_cache_policy();
            expect_ne!(cache_policy, cache_policy_get, "check initial cache policy");
            // SAFETY: `vmo` has no future mappings.
            expect_ok!(unsafe { vmo.set_mapping_cache_policy(cache_policy) }, "try set");
            cache_policy_get = vmo.get_mapping_cache_policy();
            expect_eq!(cache_policy, cache_policy_get, "compare flags");
        }

        // Test valid flags
        for i in 0..=ARCH_MMU_FLAG_CACHE_MASK {
            let vmo =
                unwrap_ok!(VmObjectPhysical::create(pa, PAGE_SIZE_USIZE), "vmobject creation\n");
            // SAFETY: `vmo` has no future mappings.
            expect_ok!(unsafe { vmo.set_mapping_cache_policy(i) }, "try setting valid flags");
        }

        // Test invalid flags
        for i in (ARCH_MMU_FLAG_CACHE_MASK + 1)..32 {
            let vmo =
                unwrap_ok!(VmObjectPhysical::create(pa, PAGE_SIZE_USIZE), "vmobject creation\n");
            // SAFETY: `vmo` has no future mappings.
            expect_eq!(
                Status::result_into_raw(unsafe { vmo.set_mapping_cache_policy(i) }),
                Status::INVALID_ARGS.into_raw(),
                "try set with invalid flags"
            );
        }

        // Test valid flags with invalid flags
        // SAFETY: `vmo` has no future mappings.
        {
            let vmo =
                unwrap_ok!(VmObjectPhysical::create(pa, PAGE_SIZE_USIZE), "vmobject creation\n");
            expect_eq!(
                Status::result_into_raw(unsafe {
                    vmo.set_mapping_cache_policy(cache_policy | 0x5)
                }),
                Status::INVALID_ARGS.into_raw(),
                "bad 0x5"
            );
            expect_eq!(
                Status::result_into_raw(unsafe {
                    vmo.set_mapping_cache_policy(cache_policy | 0xa)
                }),
                Status::INVALID_ARGS.into_raw(),
                "bad 0xA"
            );
            expect_eq!(
                Status::result_into_raw(unsafe {
                    vmo.set_mapping_cache_policy(cache_policy | 0x55)
                }),
                Status::INVALID_ARGS.into_raw(),
                "bad 0x55"
            );
            expect_eq!(
                Status::result_into_raw(unsafe {
                    vmo.set_mapping_cache_policy(cache_policy | 0xaa)
                }),
                Status::INVALID_ARGS.into_raw(),
                "bad 0xAA"
            );
        }

        // Test that changing policy while mapped is blocked
        {
            let vmo =
                unwrap_ok!(VmObjectPhysical::create(pa, PAGE_SIZE_USIZE), "vmobject creation\n");
            // SAFETY: The flags and range are appropriate for creating this mapping.
            let ptr = unsafe {
                unwrap_ok!(
                    ka.map_object_internal(
                        VmObjectPhysical::into_vm_object(vmo.clone()),
                        c"test",
                        0,
                        PAGE_SIZE_USIZE,
                        0,
                        vmm_flag::COMMIT,
                        ARCH_RW_FLAGS,
                    ),
                    "map vmo"
                )
            };
            // SAFETY: Cache policy changes are rejected while the VMO has active mappings.
            expect_eq!(
                Status::result_into_raw(unsafe { vmo.set_mapping_cache_policy(cache_policy) }),
                Status::BAD_STATE.into_raw(),
                "set flags while mapped"
            );
            // SAFETY: `ptr as usize` is a valid virtual address previously returned by
            // `map_object_internal` in `ka` that has not yet been freed.
            expect_ok!(unsafe { ka.free_region(ptr as usize) }, "unmap vmo");
            // SAFETY: `cache_policy` is appropriate for future mappings of `vmo`.
            expect_ok!(
                unsafe { vmo.set_mapping_cache_policy(cache_policy) },
                "set flags after unmapping"
            );
            // SAFETY: The flags and range are appropriate for creating this mapping.
            let ptr = unsafe {
                unwrap_ok!(
                    ka.map_object_internal(
                        VmObjectPhysical::into_vm_object(vmo),
                        c"test",
                        0,
                        PAGE_SIZE_USIZE,
                        0,
                        vmm_flag::COMMIT,
                        ARCH_RW_FLAGS,
                    ),
                    "map vmo again"
                )
            };
            // SAFETY: `ptr as usize` is a valid virtual address previously returned by
            // `map_object_internal` in `ka` that has not yet been freed.
            expect_ok!(unsafe { ka.free_region(ptr as usize) }, "unmap vmo");
        }

        // SAFETY: `vm_page` is a valid allocated PMM page from `pmm::alloc_page` that has not been
        // freed.
        unsafe { pmm::free_page(vm_page) };
    }

    /// Tests lookup and contiguous lookup on uncommitted and committed VMO ranges.
    #[test]
    fn vmo_lookup_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = PAGE_SIZE * 16;
        let vmo = unwrap_ok!(
            VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size),
            "vmobject creation\n"
        );

        let mut pages_seen = 0;
        let lookup_fn = |_offset: u64, _pa: PAddr, pages_seen: &mut usize| {
            *pages_seen += 1;
            Err(Status::NEXT)
        };
        expect_ok!(vmo.lookup(0, alloc_size, &mut pages_seen, lookup_fn));
        expect_eq!(0, pages_seen, "lookup on uncommitted pages\n");
        pages_seen = 0;

        let status = vmo.commit_range(PAGE_SIZE, PAGE_SIZE);
        expect_ok!(status, "committing vm object\n");
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0) == vmo.get_attributed_memory(),
            "committing vm object\n"
        );
        expect_true!(
            verify_continuous_attribution_bytes(&vmo, PAGE_SIZE),
            "committing vm object\n"
        );

        // Should not see any pages in the early range.
        expect_ok!(vmo.lookup(0, PAGE_SIZE, &mut pages_seen, lookup_fn));
        expect_eq!(0, pages_seen, "lookup on partially committed pages\n");
        pages_seen = 0;

        // Should see a committed page if looking at any range covering the committed.
        expect_ok!(vmo.lookup(0, alloc_size, &mut pages_seen, lookup_fn));
        expect_eq!(1, pages_seen, "lookup on partially committed pages\n");
        pages_seen = 0;

        expect_ok!(vmo.lookup(PAGE_SIZE, alloc_size - PAGE_SIZE, &mut pages_seen, lookup_fn));
        expect_eq!(1, pages_seen, "lookup on partially committed pages\n");
        pages_seen = 0;

        expect_ok!(vmo.lookup(PAGE_SIZE, PAGE_SIZE, &mut pages_seen, lookup_fn));
        expect_eq!(1, pages_seen, "lookup on partially committed pages\n");
        pages_seen = 0;

        // Contiguous lookups of single pages should also succeed
        let status = vmo.lookup_contiguous(PAGE_SIZE, PAGE_SIZE).map(|_| ());
        expect_ok!(status, "contiguous lookup of single page\n");

        // Commit the rest
        let status = vmo.commit_range(0, alloc_size);
        expect_ok!(status, "committing vm object\n");
        expect_true!(
            make_private_attribution_counts(alloc_size, 0) == vmo.get_attributed_memory(),
            "committing vm object\n"
        );
        expect_true!(
            verify_continuous_attribution_bytes(&vmo, alloc_size),
            "committing vm object\n"
        );

        let status = vmo.lookup(0, alloc_size, &mut pages_seen, lookup_fn);
        expect_ok!(status, "lookup on partially committed pages\n");
        expect_eq!(
            (alloc_size / PAGE_SIZE) as usize,
            pages_seen,
            "lookup on partially committed pages\n"
        );
        let status = vmo.lookup_contiguous(0, PAGE_SIZE).map(|_| ());
        expect_ok!(status, "contiguous lookup of single page\n");
        let status = vmo.lookup_contiguous(0, alloc_size).map(|_| ());
        expect_false!(status.is_ok(), "contiguous lookup of multiple pages\n");
    }

    /// Tests that looking up pages in a child slice translates offsets relative to the slice.
    #[test]
    fn vmo_lookup_slice_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let alloc_size = PAGE_SIZE * 16;
        let commit_offset = PAGE_SIZE * 4;
        let slice_offset = PAGE_SIZE;
        let slice_size = alloc_size - slice_offset;
        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size));

        // Commit a page in the vmo.
        assert_ok!(vmo.commit_range(commit_offset, PAGE_SIZE));

        // Create a slice that is offset slightly.
        let slice = unwrap_ok!(vmo.create_child_slice(slice_offset, slice_size, false));

        // Query the slice and validate we see one page at the offset relative to us, not the parent
        // it is committed in.
        let mut offset_seen = u64::MAX;

        let lookup_fn = |offset, _pa: PAddr, offset_seen: &mut u64| {
            assert!(*offset_seen == u64::MAX);
            *offset_seen = offset;
            Err(Status::NEXT)
        };
        expect_ok!(slice.lookup(0, slice_size, &mut offset_seen, lookup_fn));

        expect_eq!(offset_seen, commit_offset - slice_offset);
    }

    /// Tests lookup physical address isolation on COW snapshot clone hierarchies.
    #[test]
    fn vmo_lookup_clone_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        const PAGE_COUNT: usize = 4;
        let alloc_size: u64 = PAGE_SIZE * PAGE_COUNT as u64;
        // vmobject creation
        let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, alloc_size));

        vmo.set_user_id(ZX_KOID_KERNEL);

        // Commit the whole original VMO and the first and last page of the clone.
        // vmobject creation
        assert_ok!(vmo.commit_range(0, alloc_size));

        // vmobject creation
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            alloc_size,
            false,
        ));

        clone.set_user_id(ZX_KOID_KERNEL);

        // vmobject creation
        assert_ok!(clone.commit_range(0, PAGE_SIZE));
        // vmobject creation
        assert_ok!(clone.commit_range(alloc_size - PAGE_SIZE, PAGE_SIZE));

        // Lookup the paddrs for both VMOs.
        let mut vmo_lookup = [0u64; PAGE_COUNT];
        let mut clone_lookup = [0u64; PAGE_COUNT];
        let vmo_lookup_func = |offset, pa: PAddr, vmo_lookup: &mut [u64; PAGE_COUNT]| {
            vmo_lookup[(offset / PAGE_SIZE) as usize] = pa.into();
            Err(Status::NEXT)
        };
        let clone_lookup_func = |offset, pa: PAddr, clone_lookup: &mut [u64; PAGE_COUNT]| {
            clone_lookup[(offset / PAGE_SIZE) as usize] = pa.into();
            Err(Status::NEXT)
        };
        // vmo lookup
        expect_ok!(vmo.lookup(0, alloc_size, &mut vmo_lookup, vmo_lookup_func));
        // vmo lookup
        expect_ok!(clone.lookup(0, alloc_size, &mut clone_lookup, clone_lookup_func));

        // The original VMO is now copy-on-write so we should see none of its pages,
        // and we should only see the two pages that explicitly committed into the clone.
        for i in 0..PAGE_COUNT {
            // Bad paddr
            expect_eq!(0u64, vmo_lookup[i]);
            if i == 0 || i == PAGE_COUNT - 1 {
                // Bad paddr
                expect_true!(clone_lookup[i] != 0);
            }
        }
    }

    /// Tests that cloning a VMO removes write permissions from existing mappings.
    #[test]
    fn vmo_clone_removes_write_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create and map a VMO.
        let vmo =
            unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE), "vmo create");
        let vmo = VmObjectPaged::into_vm_object(vmo);

        // Use UserMemory to map the VMO, instead of mapping into the kernel aspace, so that we can
        // freely cause the mappings to modified as a consequence of the clone operation. Causing
        // kernel mappings to get modified in such a way is preferably avoided.
        let mapping =
            unwrap_ok!(UserMemory::create_from_vmo(vmo.clone(), 0, 0).ok_or(Status::NO_MEMORY));
        expect_ok!(mapping.commit_and_map(0..PAGE_SIZE_USIZE));

        // Query the aspace and validate there is a writable mapping.
        let aspace = mapping.aspace();
        let (paddr_writable, mmu_flags) =
            unwrap_ok!(aspace.arch_aspace().query(mapping.base()), "query aspace");

        expect_true!((mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) != 0, "mapping is writable check");

        // Clone the VMO, which causes the parent to have to downgrade any mappings to read-only so
        // that copy-on-write can take place. Need to set a fake user id so that the COW creation
        // code is happy.
        vmo.set_user_id(42);
        let _clone = unwrap_ok!(
            vmo.create_clone(Resizability::NonResizable, SnapshotType::Full, 0, PAGE_SIZE, true),
            "create clone"
        );

        // Aspace should now have a read only mapping with the same underlying page.
        let (paddr_readable, mmu_flags) =
            unwrap_ok!(aspace.arch_aspace().query(mapping.base()), "query aspace");
        expect_false!((mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) != 0, "mapping is read only check");
        expect_eq!(paddr_writable.0, paddr_readable.0, "mapping has same page");
    }

    /// Creates a vm object, maps it, precommitted.
    #[test]
    fn vmo_precommitted_map_test() {
        let alloc_size = 16 * PAGE_SIZE_USIZE;
        let vmo = unwrap_ok!(
            VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size as u64),
            "vmobject creation\n"
        );

        let ka = VmAspace::kernel_aspace();
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr = unsafe {
            unwrap_ok!(
                ka.map_object_internal(
                    VmObjectPaged::into_vm_object(vmo),
                    c"test",
                    0,
                    alloc_size,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                ),
                "mapping object"
            )
        };
        let ptr: *mut MaybeUninit<u8> = ptr.cast();
        // SAFETY: `ptr` points to `alloc_size` bytes of memory mapped into `ka`.
        let ptr = unsafe { slice::from_raw_parts_mut(ptr, alloc_size) };

        // fill with known pattern and test
        let (ptr, result) = fill_and_test(ptr);
        expect_true!(result);

        // SAFETY: `ptr.as_ptr() as usize` is a valid virtual address previously returned by
        // `map_object_internal` in `ka` that has not yet been freed.
        let err = unsafe { ka.free_region(ptr.as_ptr() as usize) };
        expect_ok!(err, "unmapping object");
    }

    /// Verifies that accessing a page in a pager-backed VMO promotes its LRU position.
    #[test]
    fn vmo_move_pages_on_access_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(false, false));

        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        // If we lookup the page then it should be moved to specifically the first page queue.
        unwrap_ok!(vmo.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Rotate the queues and check the page moves.
        pmm::page_queues().rotate_reclaim_queues();
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(1, queue);

        // Touching the page should move it back to the first queue.
        unwrap_ok!(vmo.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Touching pages in a child should also move the page to the front of the queues.
        let child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            PAGE_SIZE,
            true
        ));

        unwrap_ok!(child.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);
        pmm::page_queues().rotate_reclaim_queues();
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(1, queue);
        unwrap_ok!(child.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);
    }

    /// Tests memory attribution under various cloning behaviors.
    #[test]
    fn vmo_attribution_clones_test() {
        // Tests memory attribution under various cloning behaviors - creation of snapshot clones
        // and slices, removal of clones, committing pages in the original vmo and in the clones.
        let _scanner_disable = AutoVmScannerDisable::new();

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, 4 * PAGE_SIZE));
        // Fake user id to keep the cloning code happy.
        vmo.set_user_id(0xff);

        expect_true!(vmo.get_attributed_memory() == attribution::zero());
        expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

        // Commit the first two pages.
        let status = vmo.commit_range(0, 2 * PAGE_SIZE);
        assert_ok!(status);
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 2 * PAGE_SIZE));

        // Create a clone that sees the second and third pages.
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            PAGE_SIZE,
            2 * PAGE_SIZE,
            true,
        ));
        clone.set_user_id(0xfc);

        expect_true!(
            vmo.get_attributed_memory()
                == AttributionCounts {
                    uncompressed_bytes: 2 * PAGE_SIZE_USIZE,
                    private_uncompressed_bytes: PAGE_SIZE_USIZE,
                    scaled_uncompressed_bytes: attribution::fractional_bytes_add(
                        attribution::fractional_bytes_from_fraction(PAGE_SIZE, 2),
                        attribution::fractional_bytes_from_whole(PAGE_SIZE),
                    ),
                    ..attribution::zero()
                }
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 2 * PAGE_SIZE));

        expect_true!(
            clone.get_attributed_memory()
                == AttributionCounts {
                    uncompressed_bytes: PAGE_SIZE_USIZE,
                    scaled_uncompressed_bytes: attribution::fractional_bytes_from_fraction(
                        PAGE_SIZE, 2
                    ),
                    ..attribution::zero()
                }
        );
        expect_true!(verify_continuous_attribution_bytes(&clone, PAGE_SIZE));

        // Commit both pages in the clone.
        let status = clone.commit_range(0, 2 * PAGE_SIZE);
        assert_ok!(status);
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(
            clone.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 2 * PAGE_SIZE));
        expect_true!(verify_continuous_attribution_bytes(&clone, 2 * PAGE_SIZE));

        // Commit the last page in the original vmo.
        let status = vmo.commit_range(3 * PAGE_SIZE, PAGE_SIZE);
        assert_ok!(status);
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(3 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 3 * PAGE_SIZE));

        // Create a slice that sees all four pages of the original vmo.
        let slice = unwrap_ok!(vmo.create_child_slice(0, 4 * PAGE_SIZE, true));
        slice.set_user_id(0xf5);

        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(3 * PAGE_SIZE, 0)
        );
        expect_true!(
            clone.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 3 * PAGE_SIZE));
        expect_true!(verify_continuous_attribution_bytes(&clone, 2 * PAGE_SIZE));
        expect_true!(slice.get_attributed_memory() == attribution::zero());

        // Committing the slice's last page is a no-op (as the page is already committed).
        let status = slice.commit_range(3 * PAGE_SIZE, PAGE_SIZE);
        assert_ok!(status);
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(3 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 3 * PAGE_SIZE));

        // Committing the remaining 3 pages in the slice will commit pages in the original vmo.
        let status = slice.commit_range(0, 4 * PAGE_SIZE);
        assert_ok!(status);
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );
        expect_true!(
            clone.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 4 * PAGE_SIZE));
        expect_true!(verify_continuous_attribution_bytes(&clone, 2 * PAGE_SIZE));
        expect_true!(slice.get_attributed_memory() == attribution::zero());

        drop(clone);
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 4 * PAGE_SIZE));
        expect_true!(slice.get_attributed_memory() == attribution::zero());

        drop(slice);
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 4 * PAGE_SIZE));
    }

    /// Tests memory attribution under various operations.
    #[test]
    fn vmo_attribution_ops_test() {
        // Tests that memory attribution behaves as expected under various operations performed on
        // the vmo that can change its page list - committing / decommitting pages, reading /
        // writing, zero range, resizing.
        let _scanner_disable = AutoVmScannerDisable::new();

        for is_ppb_enabled in [false, true] {
            dprintf!(INFO, "is_ppb_enabled: {}\n", u32::from(is_ppb_enabled));

            let _scoped_loaning = ScopedLoaningEnabled::new(is_ppb_enabled);

            let vmo = unwrap_ok!(VmObjectPaged::create(
                pmm::ALLOC_FLAG_ANY,
                VmObjectPaged::RESIZABLE,
                4 * PAGE_SIZE,
            ));

            let mut expected_attribution_counts = attribution::zero();
            expected_attribution_counts.uncompressed_bytes = 0;
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

            let status = vmo.commit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(4 * PAGE_SIZE, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 4 * PAGE_SIZE));

            // Committing the same range again will be a no-op.
            let status = vmo.commit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 4 * PAGE_SIZE));

            let status = vmo.decommit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(0, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

            let status = vmo.commit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(4 * PAGE_SIZE, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 4 * PAGE_SIZE));

            let status = vmo.decommit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(0, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

            let mut buf = Vector::<MaybeUninit<u8>>::new();
            assert_true!(buf.resize_with(2 * PAGE_SIZE_USIZE, MaybeUninit::uninit).is_ok());

            // Read the first two pages.
            let data = unwrap_ok!(vmo.read(0, &mut buf[..PAGE_SIZE_USIZE * 2]));
            // Since these are zero pages being read, this won't commit any pages in
            // the vmo.
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

            // Write the first two pages, committing them.
            let status = vmo.write(0, data);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(2 * PAGE_SIZE, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 2 * PAGE_SIZE));

            // Write the last two pages, committing them.
            let status = vmo.write(2 * PAGE_SIZE, data);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(4 * PAGE_SIZE, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 4 * PAGE_SIZE));

            let status = vmo.resize(2 * PAGE_SIZE);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(2 * PAGE_SIZE, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 2 * PAGE_SIZE));

            // Zero'ing the range will decommit pages.
            let status = vmo.zero_range(0, 2 * PAGE_SIZE);
            assert_ok!(status);
            expected_attribution_counts = make_private_attribution_counts(0, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(&vmo, 0));
        }
    }

    /// Tests memory attribution under various operations on contiguous VMOs.
    #[test]
    fn vmo_attribution_ops_contiguous_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        for is_ppb_enabled in [false, true] {
            dprintf!(INFO, "is_ppb_enabled: {}\n", u32::from(is_ppb_enabled));

            let _loaning_enabled = ScopedLoaningEnabled::new(is_ppb_enabled);

            let vmo = unwrap_ok!(VmObjectPaged::create_contiguous(
                pmm::ALLOC_FLAG_ANY,
                4 * PAGE_SIZE,
                /*alignment_log2=*/ 0,
            ));

            let mut expected_attribution_counts = make_private_attribution_counts(4 * PAGE_SIZE, 0);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            let status = vmo.commit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            // Committing the same range again will be a no-op.
            let status = vmo.commit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            let status = vmo.decommit_range(0, 4 * PAGE_SIZE);
            if !is_ppb_enabled {
                assert_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
                // No change because DecommitRange() failed (as expected).
                debug_assert_eq!(
                    expected_attribution_counts.uncompressed_bytes,
                    4 * PAGE_SIZE_USIZE
                );
            } else {
                assert_ok!(status);
                expected_attribution_counts = make_private_attribution_counts(0, 0);
            }
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            let status = vmo.commit_range(0, 4 * PAGE_SIZE);
            assert_ok!(status);
            if !is_ppb_enabled {
                // expected_attribution_counts don't change because the pages are already present.
                debug_assert_eq!(
                    expected_attribution_counts.uncompressed_bytes,
                    4 * PAGE_SIZE_USIZE
                );
            } else {
                expected_attribution_counts = make_private_attribution_counts(4 * PAGE_SIZE, 0);
            }
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            let status = vmo.decommit_range(0, 4 * PAGE_SIZE);
            if !is_ppb_enabled {
                assert_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
                // and expected_attribution_counts don't change because we're zeroing not
                // decommitting.
                debug_assert_eq!(
                    expected_attribution_counts.uncompressed_bytes,
                    4 * PAGE_SIZE_USIZE
                );
            } else {
                assert_ok!(status);
                expected_attribution_counts = make_private_attribution_counts(0, 0);
            }
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            let mut buf = Vector::<MaybeUninit<u8>>::new();
            assert_true!(buf.resize_with(2 * PAGE_SIZE_USIZE, MaybeUninit::uninit).is_ok());

            // Read the first two pages. Reading will still cause pages to get committed.
            let data = unwrap_ok!(vmo.read(0, &mut buf[..2 * PAGE_SIZE_USIZE]));
            if !is_ppb_enabled {
                // and expected_attribution_counts don't change because the pages are already
                // present.
                debug_assert_eq!(
                    expected_attribution_counts.uncompressed_bytes,
                    4 * PAGE_SIZE_USIZE
                );
            } else {
                expected_attribution_counts = make_private_attribution_counts(2 * PAGE_SIZE, 0);
            }
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            // Write the last two pages, committing them.
            let status = vmo.write(2 * PAGE_SIZE, &data[..2 * PAGE_SIZE_USIZE]);
            assert_ok!(status);
            if !is_ppb_enabled {
                // and expected_attribution_counts don't change because the pages are already present.
                debug_assert_eq!(
                    expected_attribution_counts.uncompressed_bytes,
                    4 * PAGE_SIZE_USIZE
                );
            } else {
                expected_attribution_counts = make_private_attribution_counts(4 * PAGE_SIZE, 0);
            }
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            // Zero'ing the range will decommit pages. In the case of contiguous VMOs, we don't
            // decommit pages (so far).
            let status = vmo.zero_range(0, 2 * PAGE_SIZE);
            assert_ok!(status);
            // Zeroing doesn't decommit pages of contiguous VMOs (nor does it commit pages).
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            let status = vmo.decommit_range(0, 2 * PAGE_SIZE);
            if !is_ppb_enabled {
                assert_eq!(Status::result_into_raw(status), Status::NOT_SUPPORTED.into_raw());
                debug_assert_eq!(
                    expected_attribution_counts.uncompressed_bytes,
                    4 * PAGE_SIZE_USIZE
                );
            } else {
                assert_ok!(status);
                // We were able to decommit two pages.
                expected_attribution_counts = make_private_attribution_counts(2 * PAGE_SIZE, 0);
            }
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));

            // Zero'ing a decommitted range (if is_ppb_enabled is true) should not commit any new
            // pages. Empty slots in a decommitted contiguous VMO are zero by default, as the
            // physical page provider will zero these pages on supply.
            let status = vmo.zero_range(0, 2 * PAGE_SIZE);
            assert_ok!(status);
            // The attribution counts should remain unchanged.
            expect_true!(vmo.get_attributed_memory() == expected_attribution_counts);
            expect_true!(verify_continuous_attribution_bytes(
                &vmo,
                attribution::total_bytes(&expected_attribution_counts)
            ));
        }
    }

    /// Tests memory attribution for pager-backed VMO operations.
    #[test]
    fn vmo_attribution_pager_test() {
        // Tests that memory attribution behaves as expected for operations specific to pager-backed
        // vmo's - supplying pages, creating COW clones.
        let _scanner_disable = AutoVmScannerDisable::new();

        const NUM_PAGES: usize = 2;
        let alloc_size = (NUM_PAGES as u64) * PAGE_SIZE;
        let vmo = unwrap_ok!(make_uncommitted_pager_vmo(
            NUM_PAGES, /*trap_dirty=*/ false, /*resizable=*/ false
        ));
        // Fake user id to keep the cloning code happy.
        vmo.set_user_id(0xff);

        expect_true!(vmo.get_attributed_memory() == attribution::zero());
        expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

        // Create an aux VMO to transfer pages into the pager-backed vmo.
        let aux_vmo = unwrap_ok!(VmObjectPaged::create(
            pmm::ALLOC_FLAG_ANY,
            VmObjectPaged::RESIZABLE,
            alloc_size
        ));

        expect_true!(aux_vmo.get_attributed_memory() == attribution::zero());
        expect_true!(verify_continuous_attribution_bytes(&aux_vmo, 0));

        let status = aux_vmo.commit_range(0, alloc_size);
        assert_ok!(status);
        expect_true!(
            aux_vmo.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&aux_vmo, 2 * PAGE_SIZE));

        stack_pin_init!(let page_list = VmPageSpliceList::new());
        let status = aux_vmo.take_pages(0, PAGE_SIZE, page_list.as_mut());
        assert_ok!(status);
        expect_true!(
            aux_vmo.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&aux_vmo, PAGE_SIZE));
        expect_true!(vmo.get_attributed_memory() == attribution::zero());
        expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

        let status = vmo.supply_pages(0, PAGE_SIZE, page_list.as_mut(), SupplyOptions::PagerSupply);
        assert_ok!(status);
        expect_true!(vmo.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0));
        expect_true!(verify_continuous_attribution_bytes(&vmo, PAGE_SIZE));
        expect_true!(
            aux_vmo.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&aux_vmo, PAGE_SIZE));

        drop(aux_vmo);

        // Create a COW clone that sees the first page.
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            PAGE_SIZE,
            true
        ));
        clone.set_user_id(0xfc);

        expect_true!(vmo.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0));
        expect_true!(verify_continuous_attribution_bytes(&vmo, PAGE_SIZE));
        expect_true!(clone.get_attributed_memory() == attribution::zero());
        expect_true!(verify_continuous_attribution_bytes(&clone, 0));

        let status = clone.commit_range(0, PAGE_SIZE);
        assert_ok!(status);
        expect_true!(vmo.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0));
        expect_true!(verify_continuous_attribution_bytes(&vmo, PAGE_SIZE));
        expect_true!(
            clone.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&clone, PAGE_SIZE));

        drop(clone);
        expect_true!(vmo.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0));
        expect_true!(verify_continuous_attribution_bytes(&vmo, PAGE_SIZE));
    }

    /// Tests that memory attribution behaves as expected when zero pages are deduped.
    #[test]
    fn vmo_attribution_dedup_test() {
        // Tests that memory attribution behaves as expected when zero pages are deduped, changing
        // the no. of committed pages in the vmo.
        let _scanner_disable = AutoVmScannerDisable::new();

        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, 2 * PAGE_SIZE));

        expect_true!(vmo.get_attributed_memory() == attribution::zero());
        expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

        assert_ok!(vmo.commit_range(0, 2 * PAGE_SIZE));
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 2 * PAGE_SIZE));

        let (page, _pa) = unwrap_ok!(vmo.get_page_blocking(0, 0));

        // Dedupe the first page.
        let cow = vmo.debug_get_cow_pages().unwrap();
        assert_true!(cow.dedup_zero_page(page, 0));
        expect_true!(vmo.get_attributed_memory() == make_private_attribution_counts(PAGE_SIZE, 0));
        expect_true!(verify_continuous_attribution_bytes(&vmo, PAGE_SIZE));

        // Dedupe the second page.
        let (page, _pa) = unwrap_ok!(vmo.get_page_blocking(PAGE_SIZE, 0));
        assert_true!(cow.dedup_zero_page(page, PAGE_SIZE));
        expect_true!(vmo.get_attributed_memory() == attribution::zero());
        expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

        // Commit the range again.
        assert_ok!(vmo.commit_range(0, 2 * PAGE_SIZE));
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(verify_continuous_attribution_bytes(&vmo, 2 * PAGE_SIZE));
    }

    /// Tests parent merging and user ID updates when VMO hierarchies collapse.
    #[test]
    fn vmo_parent_merge_test() {
        // Test that a VmObjectPaged that is only referenced by its children gets removed by effectively
        // merging into its parent and re-homing all the children. This should also drop any VmCowPages
        // being held open.
        let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));
        // Set a user ID for testing.
        vmo.set_user_id(42);

        let child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            PAGE_SIZE,
            false
        ));
        child.set_user_id(43);

        expect_eq!(vmo.parent_user_id(), 0);
        expect_eq!(vmo.user_id(), 42);
        expect_eq!(child.user_id(), 43);
        expect_eq!(child.parent_user_id(), 42);

        // Dropping the parent should re-home the child to an empty parent.
        drop(vmo);
        expect_eq!(child.user_id(), 43);
        expect_eq!(child.parent_user_id(), 0);

        drop(child);

        // Recreate a more interesting 3 level hierarchy with vmo->child->(child2,child3)

        let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));
        vmo.set_user_id(42);
        let child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            PAGE_SIZE,
            false
        ));
        child.set_user_id(43);
        let child2 = unwrap_ok!(child.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            PAGE_SIZE,
            false
        ));
        child2.set_user_id(44);
        let child3 = unwrap_ok!(child.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            PAGE_SIZE,
            false
        ));
        child3.set_user_id(45);

        expect_eq!(vmo.parent_user_id(), 0);
        expect_eq!(child.parent_user_id(), 42);
        expect_eq!(child2.parent_user_id(), 43);
        expect_eq!(child3.parent_user_id(), 43);

        // Drop the intermediate child, child2+3 should get re-homed to vmo
        drop(child);
        expect_eq!(child2.parent_user_id(), 42);
        expect_eq!(child3.parent_user_id(), 42);
    }

    /// Test that the discardable VMO's lock count is updated as expected via lock and unlock ops.
    #[test]
    fn vmo_lock_count_test() {
        // Create a vmo to lock and unlock from multiple threads.
        const K_SIZE: u64 = 3 * PAGE_SIZE;
        let vmo = unwrap_ok!(VmObjectPaged::create(
            pmm::ALLOC_FLAG_ANY,
            VmObjectPaged::DISCARDABLE,
            K_SIZE,
        ));

        const K_NUM_THREADS: usize = 5;
        let mut threads: [Option<ThreadPtr>; K_NUM_THREADS] = [None; K_NUM_THREADS];
        struct ThreadState {
            vmo: *const VmObjectPaged,
            did_unlock: bool,
        }
        let mut state =
            [const { ThreadState { vmo: core::ptr::null(), did_unlock: false } }; K_NUM_THREADS];

        extern "C" fn worker(arg: *mut c_void) -> i32 {
            let state: *mut ThreadState = arg.cast();
            // SAFETY: `state` is a valid pointer to a `ThreadState` that lives for the duration
            // of the thread.
            let state = unsafe { state.as_mut_unchecked() };
            // SAFETY: `state.vmo` points to a live `VmObjectPaged` that outlives this thread.
            let vmo = unsafe { state.vmo.as_ref_unchecked() };
            let mut rand = TestRand::new(state.vmo.addr() as u32);

            // Randomly decide between try-lock and lock.
            if rand.random_bool(0.5) {
                if let Err(status) = vmo.try_lock_range(0, K_SIZE) {
                    return status.into_raw();
                }
            } else {
                if let Err(status) = vmo.lock_range(0, K_SIZE) {
                    return status.into_raw();
                }
            }

            // Randomly decide whether to unlock, or leave the vmo locked.
            if rand.random_bool(0.5) {
                if let Err(status) = vmo.unlock_range(0, K_SIZE) {
                    return status.into_raw();
                }
                state.did_unlock = true;
            }

            0
        }

        for i in 0..K_NUM_THREADS {
            state[i].vmo = &*vmo;
            state[i].did_unlock = false;

            let state_ptr: *mut ThreadState = &mut state[i];
            let arg: *mut c_void = state_ptr.cast();
            // SAFETY: `worker` is a valid entry point and `arg` points to a live `ThreadState`.
            threads[i] =
                Some(unwrap_ok!(unsafe { thread::create(c"worker".as_ptr(), worker, arg) }));
        }

        for t in &threads {
            // SAFETY: `t` is a valid thread created above and has not been joined or destroyed.
            unsafe { t.unwrap().resume() };
        }

        for t in &threads {
            // SAFETY: `t` is a valid thread that has not yet been joined.
            let ret = unwrap_ok!(unsafe { t.unwrap().join(InstantMono::INFINITE) });
            expect_eq!(0, ret);
        }

        let mut expected_lock_count = K_NUM_THREADS as u64;
        for s in &state {
            if s.did_unlock {
                expected_lock_count -= 1;
            }
        }

        expect_eq!(
            expected_lock_count,
            vmo.debug_get_cow_pages()
                .expect("vmo has cow pages")
                .debug_get_discardable_tracker()
                .expect("cow has discardable tracker")
                .debug_get_lock_count()
        );
    }

    /// Tests the state transitions for a discardable VMO.
    #[test]
    fn vmo_discardable_states_test() {
        // Tests the state transitions for a discardable VMO. Verifies that a discardable VMO is
        // discarded only when unlocked, and can be locked / unlocked again after the discard.
        let _scanner_disable = AutoVmScannerDisable::new();

        let k_size = 3 * PAGE_SIZE;
        let vmo = unwrap_ok!(VmObjectPaged::create(
            pmm::ALLOC_FLAG_ANY,
            VmObjectPaged::DISCARDABLE,
            k_size
        ));

        let cow = vmo.debug_get_cow_pages().expect("vmo has cow pages");
        let tracker = cow.debug_get_discardable_tracker().expect("cow has discardable tracker");

        // A newly created discardable vmo is not on any list yet.
        expect_false!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_reclaimable());
        expect_false!(tracker.debug_is_discarded());

        // Lock and commit all pages.
        expect_ok!(vmo.try_lock_range(0, k_size));
        expect_ok!(vmo.commit_range(0, k_size));
        expect_true!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_reclaimable());
        expect_false!(tracker.debug_is_discarded());

        // Cannot discard when locked.
        let (page, _) = unwrap_ok!(vmo.get_page_blocking(0, 0));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: It is sound to reclaim `page` at offset 0.
        let reclaimed = unsafe { cow.reclaim_page(page, 0, EvictionAction::FollowHint, None) };
        expect_true!(reclaimed.is_err());

        // Unlock.
        expect_ok!(vmo.unlock_range(0, k_size));
        expect_true!(tracker.debug_is_reclaimable());
        expect_false!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_discarded());
        if pmm::page_queues().reclaim_is_only_pager_backed() {
            // SAFETY: `page` is attached to `vmo`.
            expect_true!(unsafe { pmm::page_queues().debug_page_is_anonymous(page) });
        } else {
            // SAFETY: `page` is attached to `vmo`.
            expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        }

        // Should be able to discard now.
        // SAFETY: It is sound to reclaim `page` at offset 0.
        let reclaimed = unsafe { cow.reclaim_page(page, 0, EvictionAction::FollowHint, None) };
        assert_true!(reclaimed.is_ok());
        expect_eq!(k_size / PAGE_SIZE, reclaimed.as_ref().unwrap().num_pages);
        expect_true!(tracker.debug_is_discarded());
        expect_false!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_reclaimable());

        // Try lock should fail after discard.
        expect_eq!(
            Status::result_into_raw(vmo.try_lock_range(0, k_size)),
            Status::UNAVAILABLE.into_raw()
        );

        // Lock should succeed.
        let lock_state = unwrap_ok!(vmo.lock_range(0, k_size));
        expect_true!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_reclaimable());
        expect_false!(tracker.debug_is_discarded());

        // Verify the lock state returned.
        expect_eq!(0, lock_state.offset);
        expect_eq!(k_size, lock_state.size);
        expect_eq!(0, lock_state.discarded_offset);
        expect_eq!(k_size, lock_state.discarded_size);

        expect_ok!(vmo.commit_range(0, k_size));
        let (page, _) = unwrap_ok!(vmo.get_page_blocking(0, 0));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        // Try lock should succeed now.
        expect_ok!(vmo.try_lock_range(0, k_size));
        expect_true!(tracker.debug_is_unreclaimable());
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        // Lock count 2->1. So no change in reclaimable state.
        expect_ok!(vmo.unlock_range(0, k_size));
        expect_true!(tracker.debug_is_unreclaimable());
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        // Unlock.
        expect_ok!(vmo.unlock_range(0, k_size));
        expect_true!(tracker.debug_is_reclaimable());
        expect_false!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_discarded());
        let (page, _) = unwrap_ok!(vmo.get_page_blocking(0, 0));
        if pmm::page_queues().reclaim_is_only_pager_backed() {
            // SAFETY: `page` is attached to `vmo`.
            expect_true!(unsafe { pmm::page_queues().debug_page_is_anonymous(page) });
        } else {
            // SAFETY: `page` is attached to `vmo`.
            expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        }

        // Lock again and verify the lock state returned without a discard.
        let lock_state = unwrap_ok!(vmo.lock_range(0, k_size));
        expect_true!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_reclaimable());
        expect_false!(tracker.debug_is_discarded());
        let (page, _) = unwrap_ok!(vmo.get_page_blocking(0, 0));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        expect_eq!(0, lock_state.offset);
        expect_eq!(k_size, lock_state.size);
        expect_eq!(0, lock_state.discarded_offset);
        expect_eq!(0, lock_state.discarded_size);

        // Unlock and discard again.
        expect_ok!(vmo.unlock_range(0, k_size));
        expect_true!(tracker.debug_is_reclaimable());
        expect_false!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_discarded());
        let (page, _) = unwrap_ok!(vmo.get_page_blocking(0, 0));
        if pmm::page_queues().reclaim_is_only_pager_backed() {
            // SAFETY: `page` is attached to `vmo`.
            expect_true!(unsafe { pmm::page_queues().debug_page_is_anonymous(page) });
        } else {
            // SAFETY: `page` is attached to `vmo`.
            expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        }

        let (page, _) = unwrap_ok!(vmo.get_page_blocking(0, 0));
        // SAFETY: It is sound to reclaim `page` at offset 0.
        let reclaimed = unsafe { cow.reclaim_page(page, 0, EvictionAction::FollowHint, None) };
        assert_true!(reclaimed.is_ok());
        expect_eq!(k_size / PAGE_SIZE, reclaimed.as_ref().unwrap().num_pages);
        expect_true!(tracker.debug_is_discarded());
        expect_false!(tracker.debug_is_unreclaimable());
        expect_false!(tracker.debug_is_reclaimable());
    }

    /// Tests discardable page counts.
    #[test]
    fn vmo_discardable_counts_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        const NUM_VMOS: usize = 10;
        let mut vmos: [Option<RefPtr<VmObjectPaged>>; NUM_VMOS] = Default::default();

        // Create some discardable vmos.
        for (i, vmo) in vmos.iter_mut().enumerate() {
            *vmo = Some(unwrap_ok!(VmObjectPaged::create(
                pmm::ALLOC_FLAG_ANY,
                VmObjectPaged::DISCARDABLE,
                (i as u64 + 1) * PAGE_SIZE,
            )));
        }

        let mut rand = TestRand::new(ptr::from_ref(vmos[0].as_ref().unwrap()).addr() as u32);
        let mut expected = DiscardablePageCounts { locked: 0, unlocked: 0 };

        // Lock all vmos. Unlock a few. And discard a few unlocked ones.
        // Compute the expected page counts as a result of these operations.
        for (i, vmo) in vmos.iter().enumerate() {
            let vmo = vmo.as_ref().unwrap();
            expect_ok!(vmo.try_lock_range(0, (i as u64 + 1) * PAGE_SIZE));
            expect_ok!(vmo.commit_range(0, (i as u64 + 1) * PAGE_SIZE));

            if rand.random_bool(0.5) {
                expect_ok!(vmo.unlock_range(0, (i as u64 + 1) * PAGE_SIZE));

                if rand.random_bool(0.5) {
                    // Discarded pages won't show up under locked or unlocked counts.
                    let (page, _) = unwrap_ok!(vmo.get_page_blocking(0, 0));
                    let cow = vmo.debug_get_cow_pages().expect("vmo has cow pages");
                    // SAFETY: It is sound to reclaim `page` at offset 0.
                    let reclaimed =
                        unsafe { cow.reclaim_page(page, 0, EvictionAction::FollowHint, None) };
                    assert_true!(reclaimed.is_ok());
                    expect_eq!((i + 1) as u64, reclaimed.unwrap().num_pages);
                } else {
                    // Unlocked but not discarded.
                    expected.unlocked += (i + 1) as u64;
                }
            } else {
                // Locked.
                expected.locked += (i + 1) as u64;
            }
        }

        let counts = DiscardableVmoTracker::debug_discardable_page_counts();
        // There might be other discardable vmos in the rest of the system, so the actual page
        // counts might be higher than the expected counts.
        expect_le!(expected.locked, counts.locked);
        expect_le!(expected.unlocked, counts.unlocked);
    }

    /// Tests dirty pages with eviction hints.
    #[test]
    fn vmo_dirty_pages_with_hints_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a pager-backed VMO with a single page.
        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ true, /*resizable=*/ false
        ));

        // Newly created page should be in the first pager backed page queue.
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) };
        expect_true!(queue.is_some());
        expect_eq!(0, queue.unwrap());

        // Now simulate a write to the page. This should move the page to the dirty queue.
        assert_ok!(vmo.dirty_pages(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Hint DontNeed on the page. It should remain in the dirty queue.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(page) });
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Should not be able to evict a dirty page.
        // SAFETY: It is sound to attempt to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 0);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Hint AlwaysNeed on the page. It should remain in the dirty queue.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(page) });
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Clean the page.
        assert_ok!(vmo.writeback_begin(0, PAGE_SIZE, false));
        assert_ok!(vmo.writeback_end(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) };
        expect_true!(queue.is_some());
        expect_eq!(0, queue.unwrap());

        // Eviction should fail still because we hinted AlwaysNeed previously.
        // SAFETY: It is sound to attempt to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 0);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) };
        expect_true!(queue.is_some());
        expect_eq!(0, queue.unwrap());

        // Eviction should succeed if we ignore the hint.
        // SAFETY: It is sound to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::IgnoreHint) }, 1);
        expect_true!(attribution::zero() == vmo.get_attributed_memory_in_range(0, PAGE_SIZE));

        // Reset the vmo and retry some of the same actions as before, this time dirtying
        // the page *after* hinting.
        drop(vmo);

        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ true, /*resizable=*/ false
        ));

        // Newly created page should be in the first pager backed page queue.
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) };
        expect_true!(queue.is_some());
        expect_eq!(0, queue.unwrap());

        // Hint DontNeed on the page. This should move the page to the Isolate queue.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(page) });

        // Write to the page now. This should move it to the dirty queue.
        assert_ok!(vmo.dirty_pages(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(page) });
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Should not be able to evict a dirty page.
        // SAFETY: It is sound to attempt to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 0);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );
    }

    /// Tests that pinning pager-backed pages retains backlink information.
    #[test]
    fn vmo_pinning_backlink_test() {
        // Disable the page scanner as this test would be flaky if our pages get
        // evicted by someone else.
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a pager-backed VMO with two pages, so we can verify a non-zero offset value.
        let (vmo, [page0, page1]) = unwrap_ok!(make_committed_pager_vmo(false, false));

        // SAFETY: `page0` and `page1` are attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page0) }.is_some());
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page1) }.is_some());

        // Verify backlink information.
        let cow = vmo.debug_get_cow_pages().expect("cow pages must exist");
        let cow_ptr = cow.as_raw().cast::<core::ffi::c_void>();
        // SAFETY: `page0` and `page1` are attached to `vmo`.
        expect_eq!(unsafe { page0.get_object() }, cow_ptr);
        expect_eq!(unsafe { page0.get_page_offset() }, 0);
        expect_eq!(unsafe { page1.get_object() }, cow_ptr);
        expect_eq!(unsafe { page1.get_page_offset() }, PAGE_SIZE);

        // Pin the pages.
        let status = vmo.commit_range_pinned(0, 2 * PAGE_SIZE, false);
        expect_ok!(status);

        // Pages might get swapped out on pinning if they were loaned. Look them up again.
        let page0 = vmo.debug_get_page(0).expect("page 0 must exist");
        let page1 = vmo.debug_get_page(PAGE_SIZE).expect("page 1 must exist");

        // SAFETY: `page0` and `page1` are attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_wired(page0) });
        expect_true!(unsafe { pmm::page_queues().debug_page_is_wired(page1) });
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page0) }.is_some());
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page1) }.is_some());

        // Moving to the wired queue should retain backlink information.
        // SAFETY: `page0` and `page1` are attached to `vmo`.
        expect_eq!(unsafe { page0.get_object() }, cow_ptr);
        expect_eq!(unsafe { page0.get_page_offset() }, 0);
        expect_eq!(unsafe { page1.get_object() }, cow_ptr);
        expect_eq!(unsafe { page1.get_page_offset() }, PAGE_SIZE);

        // Unpin the pages.
        vmo.unpin(0, 2 * PAGE_SIZE);

        // Pages should be back in the pager queue.
        // SAFETY: `page0` and `page1` are attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_wired(page0) });
        expect_false!(unsafe { pmm::page_queues().debug_page_is_wired(page1) });
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page0) }.is_some());
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page1) }.is_some());

        // Verify backlink information again.
        // SAFETY: `page0` and `page1` are attached to `vmo`.
        expect_eq!(unsafe { page0.get_object() }, cow_ptr);
        expect_eq!(unsafe { page0.get_page_offset() }, 0);
        expect_eq!(unsafe { page1.get_object() }, cow_ptr);
        expect_eq!(unsafe { page1.get_page_offset() }, PAGE_SIZE);
    }

    /// Tests updating dirty state of pages while they are pinned.
    #[test]
    fn vmo_pinning_dirty_state_test() {
        // Disable the page scanner as this test would be flaky if our pages get
        // evicted by someone else.
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a pager-backed VMO with a single page.
        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(true, false));

        // Page should be in the pager queue.
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        // Pin the page.
        let status = vmo.commit_range_pinned(0, PAGE_SIZE, false);
        expect_ok!(status);

        // Pages might get swapped out on pinning if they were loaned. Look up again.
        let page = vmo.debug_get_page(0).expect("page 0 must exist");

        // Page should be in the wired queue.
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_wired(page) });
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        // Dirty the page while pinned. So this tests a transition to Dirty with pin count > 0. This
        // should retain the page in the wired queue.
        let status = vmo.dirty_pages(0, PAGE_SIZE);
        expect_ok!(status);
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_wired(page) });

        // Unpin the page.
        vmo.unpin(0, PAGE_SIZE);

        // Page should be back in the pager dirty queue.
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Start writeback on the page so that its state changes to AwaitingClean. It should still
        // be in the dirty queue.
        let status = vmo.writeback_begin(0, PAGE_SIZE, false);
        expect_ok!(status);
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Pin for read, so that the dirty state is not changed. But since it is pinned, it should
        // move to the wired queue.
        let status = vmo.commit_range_pinned(0, PAGE_SIZE, false);
        expect_ok!(status);
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_wired(page) });

        // Now end the writeback so that the page is cleaned. So this tests a transition to Clean
        // with pin count > 0.
        let status = vmo.writeback_end(0, PAGE_SIZE);
        expect_ok!(status);

        // Page should still be in the wired queue.
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_wired(page) });

        // Unpin the page.
        vmo.unpin(0, PAGE_SIZE);

        // Pages should be back in the pager reclaim queue.
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_wired(page) });

        // The only remaining transition is to AwaitingClean with pin count > 0. This cannot happen
        // because we can only move to AwaitingClean from Dirty, but if a page is Dirty with pin
        // count > 0, it will never leave the Dirty state.
    }

    /// Tests that writing to a VMO does not commit pages in its clone.
    #[test]
    fn vmo_write_does_not_commit_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a vmo and commit a page to it.
        let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));

        let val: u64 = 42;
        expect_ok!(vmo.write(0, &val.to_le_bytes()));

        // Create a CoW clone of the vmo.
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            PAGE_SIZE,
            false
        ));

        // Querying the page for read in the clone should return it.
        unwrap_ok!(clone.get_page_blocking(0, 0));

        // Querying for write, without any fault flags, should not work as the page is not committed in
        // the clone.
        expect_eq!(
            Status::result_into_raw(clone.get_page_blocking(0, fault::flag::WRITE).map(|_| ())),
            Status::NOT_FOUND.into_raw()
        );

        // Adding a fault flag should cause the lookup to succeed.
        unwrap_ok!(clone.get_page_blocking(0, fault::flag::WRITE | fault::flag::SW_FAULT));
    }

    /// Tests dirty page tracking and queue transitions in pager-backed VMOs.
    #[test]
    fn vmo_dirty_pages_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a pager-backed VMO with a single page.
        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ true, /*resizable=*/ false
        ));

        // Newly created page should be in the first pager backed page queue.
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) };
        expect_true!(queue.is_some());
        expect_eq!(0, queue.unwrap());

        // Rotate the queues and check the page moves.
        pmm::page_queues().rotate_reclaim_queues();
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) };
        expect_true!(queue.is_some());
        expect_eq!(1, queue.unwrap());

        // Accessing the page should move it back to the first queue.
        unwrap_ok!(vmo.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) };
        expect_true!(queue.is_some());
        expect_eq!(0, queue.unwrap());

        // Now simulate a write to the page. This should move the page to the dirty queue.
        unwrap_ok!(vmo.dirty_pages(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });
        expect_gt!(pmm::page_queues().queue_counts().pager_backed_dirty, 0);

        // Should not be able to evict a dirty page.
        // SAFETY: It is sound to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 0);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Accessing the page again should not move the page out of the dirty queue.
        unwrap_ok!(vmo.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });
    }

    /// Tests dirty pages writeback behavior.
    #[test]
    fn vmo_dirty_pages_writeback_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a pager-backed VMO with a single page.
        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ true, /*resizable=*/ false
        ));

        // Newly created page should be in the first pager backed page queue.
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Now simulate a write to the page. This should move the page to the dirty queue.
        assert_ok!(vmo.dirty_pages(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Should not be able to evict a dirty page.
        // SAFETY: It is sound to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 0);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Begin writeback on the page. This should still keep the page in the dirty queue.
        assert_ok!(vmo.writeback_begin(0, PAGE_SIZE, false));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Should not be able to evict a dirty page.
        // SAFETY: It is sound to attempt to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 0);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Accessing the page should not move the page out of the dirty queue either.
        unwrap_ok!(vmo.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Should not be able to evict a dirty page.
        // SAFETY: It is sound to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 0);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // End writeback on the page. This should finally move the page out of the dirty queue.
        assert_ok!(vmo.writeback_end(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // We should be able to rotate the page as usual.
        pmm::page_queues().rotate_reclaim_queues();
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(1, queue);

        // Another write moves the page back to the Dirty queue.
        assert_ok!(vmo.dirty_pages(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // Clean the page again, and try to evict it.
        assert_ok!(vmo.writeback_begin(0, PAGE_SIZE, false));
        assert_ok!(vmo.writeback_end(0, PAGE_SIZE));
        // SAFETY: `page` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });
        // SAFETY: `page` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(page) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // We should now be able to evict the page.
        // SAFETY: It is sound to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 1);
        expect_true!(attribution::zero() == vmo.get_attributed_memory_in_range(0, PAGE_SIZE));
    }

    /// Tests that decommitting from a contiguous VMO fails when loaning is disabled.
    #[test]
    fn vmo_contiguous_decommit_disabled_test() {
        let _enable_loaning = ScopedLoaningEnabled::new(false);

        let alloc_size = PAGE_SIZE * 16;
        let vmo = unwrap_ok!(VmObjectPaged::create_contiguous(
            ALLOC_FLAG_ANY,
            alloc_size,
            /*alignment_log2*/ 0
        ));

        // decommit fails as expected
        assert_eq!(
            Status::result_into_raw(vmo.decommit_range(PAGE_SIZE, 4 * PAGE_SIZE)),
            Status::NOT_SUPPORTED.into_raw()
        );
        // decommit fails as expected
        assert_eq!(
            Status::result_into_raw(vmo.decommit_range(0, 4 * PAGE_SIZE)),
            Status::NOT_SUPPORTED.into_raw()
        );
        // decommit fails as expected
        assert_eq!(
            Status::result_into_raw(vmo.decommit_range(alloc_size - PAGE_SIZE, PAGE_SIZE)),
            Status::NOT_SUPPORTED.into_raw()
        );
    }

    /// Tests that contiguous VMOs can be decommitted when ppb is enabled.
    #[test]
    fn vmo_contiguous_decommit_enabled_test() {
        let _loaning_enabled = ScopedLoaningEnabled::new(true);

        let alloc_size: u64 = PAGE_SIZE * 16;
        let vmo = unwrap_ok!(VmObjectPaged::create_contiguous(pmm::ALLOC_FLAG_ANY, alloc_size, 0));

        // Scope the memsetting so that the kernel mapping does not keep existing to the point that
        // the Decommits happen below. As those decommits would need to perform unmaps, and we
        // prefer to not modify kernel mappings in this way, we just remove the kernel region.
        {
            let ka = VmAspace::kernel_aspace();
            let vmo = VmObjectPaged::into_vm_object(vmo.clone());
            // SAFETY: The flags and range are appropriate for creating this mapping.
            let ptr = unwrap_ok!(unsafe {
                ka.map_object_internal(
                    vmo,
                    c"test",
                    0,
                    alloc_size as usize,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
                )
            });

            struct DeferCleanupMapping(*mut c_void);
            impl Drop for DeferCleanupMapping {
                fn drop(&mut self) {
                    // SAFETY: self.0 was allocated via map_object_internal and is not yet freed.
                    let res = unsafe { VmAspace::kernel_aspace().free_region(self.0 as usize) };
                    assert!(res.is_ok());
                }
            }
            let _defer_cleanup_mapping = DeferCleanupMapping(ptr);
            let base: *mut u8 = ptr.cast();
            // SAFETY: `base` points to a committed mapping of `alloc_size` bytes.
            let base = unsafe { slice::from_raw_parts_mut(base, alloc_size as usize) };

            for offset in (0..alloc_size as usize).step_by(PAGE_SIZE_USIZE) {
                base[offset..offset + PAGE_SIZE_USIZE].fill(0x42);
            }
        }

        let mut base_pa = PAddr::from(!0);
        assert_ok!(vmo.lookup(0, PAGE_SIZE, &mut base_pa, |offset, pa, base_pa| {
            debug_assert_eq!(offset, 0);
            *base_pa = pa;
            Err(Status::NEXT)
        }));
        assert_true!(base_pa != PAddr::from(!0));

        // decommit pretends to work
        assert_ok!(vmo.decommit_range(PAGE_SIZE, 4 * PAGE_SIZE));
        assert_ok!(vmo.decommit_range(0, 4 * PAGE_SIZE));
        assert_ok!(vmo.decommit_range(alloc_size - PAGE_SIZE, PAGE_SIZE));

        // Make sure decommit removed pages.  Make sure pages which are present are the correct
        // physical address.
        for offset in (0..alloc_size).step_by(PAGE_SIZE_USIZE) {
            let mut page_absent = true;
            let _ = vmo.lookup(
                offset,
                PAGE_SIZE,
                &mut (&mut page_absent, base_pa, offset),
                |lookup_offset, pa, (page_absent, base_pa, offset)| {
                    **page_absent = false;
                    debug_assert_eq!(*offset, lookup_offset);
                    debug_assert_eq!(base_pa.0 + *offset as usize, pa.0);
                    Err(Status::NEXT)
                },
            );
            let absent_expected = (offset < 5 * PAGE_SIZE) || (offset == alloc_size - PAGE_SIZE);
            assert_eq!(absent_expected, page_absent);
        }
    }

    /// Tests eviction hints on a VMO.
    #[test]
    fn vmo_eviction_hints_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a pager-backed VMO with two pages.
        let (vmo, mut pages) = unwrap_ok!(make_committed_pager_vmo::<2>(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));

        // Newly created page should be in the first pager backed page queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Hint that first page is not needed.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));

        // The page should now have moved to the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Hint that the page is always needed.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));

        // If the page was loaned, it will be replaced with a non-loaned page now.
        pages[0] = vmo.debug_get_page(0).expect("vmo should have a page at offset 0");

        // The page should now have moved to the first LRU queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // We should not be able to evict the page.
        // SAFETY: `pages[0]` is attached to `vmo` at offset 0.
        assert_lt!(unsafe { reclaim(&vmo, pages[0], 0, EvictionAction::FollowHint) }, 2);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Hint that the page is not needed again.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));

        // HintRange() is allowed to replace the page.
        pages[0] = vmo.debug_get_page(0).expect("vmo should have a page at offset 0");

        // The page should now have moved to the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // We should still not be able to evict the page, the AlwaysNeed hint is sticky.
        // SAFETY: `pages[0]` is attached to `vmo` at offset 0.
        assert_lt!(unsafe { reclaim(&vmo, pages[0], 0, EvictionAction::FollowHint) }, 2);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Accessing the page should move it out of the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Verify that the page can be rotated as normal.
        pmm::page_queues().rotate_reclaim_queues();
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(1, queue);

        // Touching the page should move it back to the first queue.
        unwrap_ok!(vmo.get_page_blocking(0, fault::flag::SW_FAULT));
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // We should be able to evict first page when told to override the hint.
        // SAFETY: `pages[0]` is attached to `vmo` at offset 0.
        assert_ge!(unsafe { reclaim(&vmo, pages[0], 0, EvictionAction::IgnoreHint) }, 1);
        expect_true!(attribution::zero() == vmo.get_attributed_memory_in_range(0, PAGE_SIZE));

        // Re-supply pages.
        let mut pages = unwrap_ok!(supply_pager_vmo_pages::<2>(&vmo, 0, 2));

        // Hint that second page is always needed.
        assert_ok!(vmo.hint_range(PAGE_SIZE, PAGE_SIZE, EvictionHint::AlwaysNeed));
        // If the page was loaned, it will be replaced with a non-loaned page now.
        pages[1] =
            vmo.debug_get_page(PAGE_SIZE).expect("vmo should have a page at offset PAGE_SIZE");
        let _ = pages;
    }

    /// Tests HintRange(AlwaysNeed) evicts loaned pages.
    #[test]
    fn vmo_always_need_evicts_loaned_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Depending on which loaned page we get, it may not still be loaned at
        // the time HintRange() is called, so try a few times and make sure we
        // see non-loaned after HintRange() for all the tries.
        let try_count = 30;
        for _try_ordinal in 0..try_count {
            let _enable_loaning = ScopedLoaningEnabled::new(true);

            // create a contiguous VMO so that we are guaranteed to have a place to borrow from
            let contiguous_vmo = unwrap_ok!(VmObjectPaged::create_contiguous(
                ALLOC_FLAG_ANY,
                PAGE_SIZE,
                /*alignment_log2*/ 0
            ));
            assert_ok!(contiguous_vmo.decommit_range(0, PAGE_SIZE));

            // we will replace the only page in vmo with a loaned page
            let (vmo, [before_page]) = unwrap_ok!(make_committed_pager_vmo(
                /*trap_dirty*/ false, /*resizable*/ false
            ));
            let offset = 0;
            let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
            assert_ok!(cow_pages.replace_page_with_loaned(before_page, offset));
            // The call to ReplacePageWithLoaned may loan vmo's page to a VMO that's
            // not contiguous_vmo. So, it might get called back, and the rest of the
            // test must tolerate the vmo's page becoming unloaned at any time.

            // Hint that the page is always needed.
            assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));

            // If the page was still loaned, it will be replaced with a non-loaned page now.
            let page =
                vmo.debug_get_page(0).expect("vmo should have a page at offset 0 after hint_range");

            assert_false!(unsafe { page.is_loaned() });
        }
    }

    /// Tests eviction hints on clones.
    #[test]
    fn vmo_eviction_hints_clone_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create a pager-backed VMO with two pages. We will fork a page in a clone later.
        let (vmo, mut pages) = unwrap_ok!(make_committed_pager_vmo::<2>(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));

        // Newly created pages should be in the first pager backed page queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);
        // SAFETY: `pages[1]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[1]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Create a clone.
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            2 * PAGE_SIZE,
            true
        ));

        // Use the clone to perform a bunch of hinting operations on the first page.
        // Hint that the page is not needed.
        assert_ok!(clone.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));

        // The page should now have moved to the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Hint that the page is always needed.
        assert_ok!(clone.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));

        // If the page was loaned, it will be replaced with a non-loaned page now.
        pages[0] = vmo.debug_get_page(0).expect("vmo should have a page at offset 0");

        // The page should now have moved to the first LRU queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Evicting the page should fail.
        // SAFETY: `pages[0]` is attached to `vmo` at offset 0.
        assert_lt!(unsafe { reclaim(&vmo, pages[0], 0, EvictionAction::FollowHint) }, 2);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Hinting should also work via a clone of a clone.
        let clone2 = unwrap_ok!(clone.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            2 * PAGE_SIZE,
            true
        ));

        // Hint that the page is not needed.
        assert_ok!(clone2.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));

        // The page should now have moved to the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Hint that the page is always needed.
        assert_ok!(clone2.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));

        // If the page was loaned, it will be replaced with a non-loaned page now.
        pages[0] = vmo.debug_get_page(0).expect("vmo should have a page at offset 0");

        // The page should now have moved to the first LRU queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });
        // SAFETY: `pages[0]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Evicting the page should fail.
        // SAFETY: `pages[0]` is attached to `vmo` at offset 0.
        assert_lt!(unsafe { reclaim(&vmo, pages[0], 0, EvictionAction::FollowHint) }, 2);
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0)
                == vmo.get_attributed_memory_in_range(0, PAGE_SIZE)
        );

        // Re supply the second page, in case it was evicted.
        let [second_page] = unwrap_ok!(supply_pager_vmo_pages(&vmo, 1, 1));
        pages[1] = second_page;

        // SAFETY: `pages[1]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[1]) }.is_some());

        // Verify that hinting still works via the parent VMO.
        // Hint that the page is not needed again.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));

        // The page should now have moved to the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Fork the page in the clone. And make sure hints no longer apply.
        let data: u64 = 0xff;
        assert_ok!(clone.write(0, &data.to_ne_bytes()));
        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0) == clone.get_attributed_memory()
        );
        expect_true!(verify_continuous_attribution_bytes(&clone, PAGE_SIZE));

        // The write will have moved the page to the first page queue, because the page is still
        // accessed in order to perform the fork. So hint using the parent again to move to the
        // Isolate queue.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));

        // The page should now have moved to the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Hint that the page is always needed via the clone.
        assert_ok!(clone.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));

        // The page should still be in the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Hint that the page is always needed via the second level clone.
        assert_ok!(clone2.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));

        // This should move the page out of the the Isolate queue. Since we forked the page in the
        // intermediate clone *after* this clone was created, it will still refer to the original
        // page, which is the same as the page in the root.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Create another clone that sees the forked page.
        // Hinting through this clone should have no effect, since it will see the forked page.
        let clone3 = unwrap_ok!(clone.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            2 * PAGE_SIZE,
            true
        ));

        // Move the page back to the Isolate queue first.
        assert_ok!(vmo.hint_range(0, PAGE_SIZE, EvictionHint::DontNeed));

        // The page should now have moved to the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Hint through clone3.
        assert_ok!(clone3.hint_range(0, PAGE_SIZE, EvictionHint::AlwaysNeed));

        // The page should still be in the Isolate queue.
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[0]) }.is_some());
        // SAFETY: `pages[0]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[0]) });

        // Hint on the second page using clone3. This page hasn't been forked by the intermediate
        // clone. So clone3 should still be able to see the root page.
        // First verify that the page is still in queue 0.
        // SAFETY: `pages[1]` is attached to `vmo`.
        let queue = unsafe { pmm::page_queues().debug_page_is_reclaim(pages[1]) }
            .expect("page is in reclaim queue");
        expect_eq!(0, queue);

        // Hint DontNeed through clone 3.
        assert_ok!(clone3.hint_range(PAGE_SIZE, PAGE_SIZE, EvictionHint::DontNeed));

        // The page should have moved to the Isolate queue.
        // SAFETY: `pages[1]` is attached to `vmo`.
        expect_false!(unsafe { pmm::page_queues().debug_page_is_reclaim(pages[1]) }.is_some());
        // SAFETY: `pages[1]` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim_isolate(pages[1]) });
    }

    /// Tests unloaning and evicting loaned pages from VMOs.
    #[test]
    fn vmo_unloan_test() {
        // Disable the page scanner as this test would be flaky if our pages get evicted by someone
        // else.
        let _scanner_disable = AutoVmScannerDisable::new();

        let _enable_loaning = ScopedLoaningEnabled::new(true);

        let contiguous_vmo =
            unwrap_ok!(VmObjectPaged::create_contiguous(ALLOC_FLAG_ANY, 2 * PAGE_SIZE, 0));
        assert_ok!(contiguous_vmo.decommit_range(0, 2 * PAGE_SIZE));

        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));
        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        assert_ok!(cow_pages.replace_page_with_loaned(page, 0));
        let page = vmo.debug_get_page(0).expect("vmo should have a page at offset 0");
        assert_true!(unsafe { page.is_loaned() });

        let (vmo2, [page2]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));
        assert_ok!(
            vmo2.debug_get_cow_pages()
                .expect("paged VMO has backing cow pages")
                .replace_page_with_loaned(page2, 0)
        );
        let page2 = vmo2.debug_get_page(0).expect("vmo2 should have a page at offset 0");
        assert_true!(unsafe { page2.is_loaned() });

        // Shouldn't be able to evict pages from the wrong VMO.
        // SAFETY: `page2` is associated with `vmo2`.
        assert_false!(unsafe { evict_loaned_page(&vmo, page2, 0) });
        // SAFETY: `page` is associated with `vmo`.
        assert_false!(unsafe { evict_loaned_page(&vmo2, page, 0) });

        // Evicting a loaned page should drop the number of committed pages.
        expect_true!(make_private_attribution_counts(PAGE_SIZE, 0) == vmo2.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo2, PAGE_SIZE));
        // SAFETY: `page2` is associated with `vmo2`.
        assert_true!(unsafe { evict_loaned_page(&vmo2, page2, 0) });
        expect_true!(attribution::zero() == vmo2.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo2, 0));

        // Pinned pages should not be evictable.
        expect_ok!(vmo.commit_range_pinned(0, PAGE_SIZE, false));
        // SAFETY: `page` is associated with `vmo`.
        assert_false!(unsafe { evict_loaned_page(&vmo, page, 0) });
        vmo.unpin(0, PAGE_SIZE);
    }

    /// # Safety
    ///
    /// Must be able to get the paddr for `page`.
    unsafe fn is_page_zero(page: VmPagePtr) -> bool {
        // SAFETY: We can obtain the paddr for `page`.
        let base: *const u64 = paddr_to_physmap(unsafe { page.paddr() }).0 as *const u64;
        let len = PAGE_SIZE_USIZE / core::mem::size_of::<u64>();
        // SAFETY: `base` is page-aligned (which satisfies u64 alignment) and contains
        // `len * size_of::<u64>() == PAGE_SIZE_USIZE` bytes.
        let page_slice: &[u64] = unsafe { slice::from_raw_parts(base, len) };
        for &word in page_slice {
            if word != 0 {
                return false;
            }
        }
        true
    }

    /// Tests that ZeroRange does not remove pinned pages.
    #[test]
    fn vmo_zero_pinned_test() {
        // Tests that ZeroRange does not remove pinned pages. Regression test for
        // https://fxbug.dev/42052452.

        // Create a non pager-backed VMO.
        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE));

        // Pin the page for write.
        assert_ok!(vmo.commit_range_pinned(0, PAGE_SIZE, true));

        // Write non-zero content to the page.
        let page = vmo.debug_get_page(0).expect("page must exist");
        let ptr: *mut u8 = paddr_to_physmap(unsafe { page.paddr() }).0 as *mut u8;
        // SAFETY: `ptr` points to the kernel physmap for a valid committed page.
        unsafe { *ptr = 0xff };

        // Zero the page and check that it is not removed.
        assert_ok!(vmo.zero_range(0, PAGE_SIZE));
        expect_true!(Some(page) == vmo.debug_get_page(0));

        // The page should be zero.
        // SAFETY: We can obtain the paddr for `page`.
        expect_true!(unsafe { is_page_zero(page) });

        vmo.unpin(0, PAGE_SIZE);

        // Create a pager-backed VMO.
        let (pager_vmo, [_old_page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ false, /*resizable=*/ true,
        ));

        // Pin the page for write.
        assert_ok!(pager_vmo.commit_range_pinned(0, PAGE_SIZE, true));

        // Write non-zero content to the page. Lookup the page again, as pinning might have switched
        // out the page if it was originally loaned.
        let old_page = pager_vmo.debug_get_page(0).expect("page must exist");
        let ptr: *mut u8 = paddr_to_physmap(unsafe { old_page.paddr() }).0 as *mut u8;
        // SAFETY: `ptr` points to the kernel physmap for a valid committed page.
        unsafe { *ptr = 0xff };

        // Zero the page and check that it is not removed.
        assert_ok!(pager_vmo.zero_range(0, PAGE_SIZE));
        expect_true!(Some(old_page) == pager_vmo.debug_get_page(0));

        // The page should be zero.
        // SAFETY: We can obtain the paddr for `old_page`.
        expect_true!(unsafe { is_page_zero(old_page) });

        // Resize the VMO up, and pin a page in the newly extended range.
        assert_ok!(pager_vmo.resize(2 * PAGE_SIZE));
        assert_ok!(pager_vmo.commit_range_pinned(PAGE_SIZE, PAGE_SIZE, true));

        // Write non-zero content to the page.
        let new_page = pager_vmo.debug_get_page(PAGE_SIZE).expect("page must exist");
        let ptr: *mut u8 = paddr_to_physmap(unsafe { new_page.paddr() }).0 as *mut u8;
        // SAFETY: `ptr` points to the kernel physmap for a valid committed page.
        unsafe { *ptr = 0xff };

        // Zero the new page, and ensure that it is not removed.
        assert_ok!(pager_vmo.zero_range(PAGE_SIZE, PAGE_SIZE));
        expect_true!(Some(new_page) == pager_vmo.debug_get_page(PAGE_SIZE));

        // The page should be zero.
        // SAFETY: We can obtain the paddr for `new_page`.
        expect_true!(unsafe { is_page_zero(new_page) });

        pager_vmo.unpin(0, 2 * PAGE_SIZE);
    }

    /// Tests VMO page reclamation.
    #[test]
    fn vmo_reclamation_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        const NUM_PAGES: usize = 2;
        let alloc_size = (NUM_PAGES as u64) * PAGE_SIZE;

        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));

        // Reclamation should drop the number of committed pages.
        expect_true!(make_private_attribution_counts(PAGE_SIZE, 0) == vmo.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo, PAGE_SIZE));
        // SAFETY: It is sound to reclaim `page` at offset 0.
        assert_eq!(unsafe { reclaim(&vmo, page, 0, EvictionAction::FollowHint) }, 1);
        expect_true!(attribution::zero() == vmo.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo, 0));
        expect_gt!(vmo.reclamation_event_count(), 0);

        // Pinned pages should not be reclaimable.
        let (vmo, pages) = unwrap_ok!(make_committed_pager_vmo::<NUM_PAGES>(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));

        expect_ok!(vmo.commit_range_pinned(0, alloc_size / 2, false));
        // SAFETY: It is sound to reclaim `pages[0]` at offset 0.
        assert_le!(unsafe { reclaim(&vmo, pages[0], 0, EvictionAction::FollowHint) }, 2);
        vmo.unpin(0, alloc_size / 2);

        // Trying to reclaim from a VMO with no pages in isolate is considered an 'evict accesed'
        // failure.
        let (vmo, pages) = unwrap_ok!(make_committed_pager_vmo::<NUM_PAGES>(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));

        for &page in &pages {
            // SAFETY: `page` is attached to `vmo`.
            expect_false!(unsafe { PageQueues::is_page_reclaimable(page) });
        }

        // SAFETY: It is sound to reclaim `pages[0]` at offset 0.
        let reclaimed = unsafe {
            vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages").reclaim_page(
                pages[0],
                0,
                EvictionAction::FollowHint,
                None,
            )
        };

        expect_true!(reclaimed.is_err());
        expect_eq!(reclaimed.unwrap_err(), VmCowReclaimFailure::EvictAccessed);
    }

    /// Tests PinnedVmObject creation, move semantics, and RAII unpinning.
    #[test]
    fn vmo_pinned_wrapper_test() {
        // Porting note: This is a verbatim conversion of the C++ test `vmo_pinned_wrapper_test`.
        // It preserves move and assignment constructs from the C++ source that are trivial in Rust.

        {
            let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));
            let vmo = VmObjectPaged::into_vm_object(vmo);

            let mut pinned = unwrap_ok!(PinnedVmObject::create(vmo.clone(), 0, PAGE_SIZE, true));
            pinned = unwrap_ok!(PinnedVmObject::create(vmo, 0, PAGE_SIZE, true));
            drop(pinned);
        }

        {
            let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));
            let vmo = VmObjectPaged::into_vm_object(vmo);

            let mut pinned = Some(unwrap_ok!(PinnedVmObject::create(vmo, 0, PAGE_SIZE, true)));
            assert!(pinned.is_some());
            let empty: Option<PinnedVmObject> = None;
            pinned = empty;
            drop(pinned);
        }

        {
            let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));
            let vmo = VmObjectPaged::into_vm_object(vmo);

            let pinned = unwrap_ok!(PinnedVmObject::create(vmo, 0, PAGE_SIZE, true));
            let mut empty: Option<PinnedVmObject> = None;
            assert!(empty.is_none());
            empty = Some(pinned);
            drop(empty);
        }

        {
            let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));
            let vmo = VmObjectPaged::into_vm_object(vmo);

            let mut pinned1 = unwrap_ok!(PinnedVmObject::create(vmo.clone(), 0, PAGE_SIZE, true));
            let pinned2 = unwrap_ok!(PinnedVmObject::create(vmo, 0, PAGE_SIZE, true));
            pinned1 = pinned2;
            drop(pinned1);
        }
    }

    /// Tests zeroing a range that has pages mapped in the kernel after committed pages.
    #[test]
    fn vmo_zero_partially_pinned_range_test() {
        // Regression test for https://fxbug.dev/504708573. Attempt to zero a range that has pages
        // mapped in the kernel after committed pages.

        // Ensure that we do not compress pages before ZeroRange acquires the VmCowPage lock, as
        // this would prevent the unmap round-up optimization from being triggered.
        let _scanner_disable = AutoVmScannerDisable::new();

        // TODO(https://fxbug.dev/547981705): Improve the ergonomics of `test_vmo` when we have a
        // convenient way of creating subtests with //zircon/kernel/lib/unittest.
        let test_vmo = |vmo: RefPtr<VmObject>| -> bool {
            // Commit a page to force an unmap when the range is zeroed.
            if vmo.commit_range(0, PAGE_SIZE).is_err() {
                return false;
            }

            let ka = VmAspace::kernel_aspace();
            // SAFETY: The flags and range are appropriate for creating this mapping.
            let ptr = match unsafe {
                ka.map_object_internal(
                    vmo.clone(),
                    c"test",
                    /*offset=*/ PAGE_SIZE,
                    /*size=*/ PAGE_SIZE_USIZE,
                    0,
                    vmm_flag::COMMIT,
                    ARCH_RW_FLAGS,
                )
            } {
                Ok(ptr) => ptr,
                Err(_) => return false,
            };
            struct DeferCleanupMapping(*mut c_void);
            impl Drop for DeferCleanupMapping {
                fn drop(&mut self) {
                    // SAFETY: self.0 was allocated via map_object_internal and is not yet freed.
                    let res = unsafe { VmAspace::kernel_aspace().free_region(self.0 as usize) };
                    assert!(res.is_ok());
                }
            }
            let _cleanup_mapping = DeferCleanupMapping(ptr);

            vmo.zero_range(0, 2 * PAGE_SIZE).is_ok()
        };

        {
            let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, 2 * PAGE_SIZE));
            expect_true!(test_vmo(VmObjectPaged::into_vm_object(vmo)));
        }

        {
            let (vmo, _) = unwrap_ok!(make_committed_pager_vmo::<2>(
                /*trap_dirty=*/ false, /*resizable=*/ false,
            ));
            expect_true!(test_vmo(VmObjectPaged::into_vm_object(vmo)));
        }

        {
            let (vmo, _) = unwrap_ok!(make_committed_pager_vmo::<2>(
                /*trap_dirty=*/ false, /*resizable=*/ false,
            ));
            let unidirectional_clone = unwrap_ok!(vmo.create_clone(
                Resizability::NonResizable,
                SnapshotType::OnWrite,
                0,
                2 * PAGE_SIZE,
                false,
            ));
            expect_true!(test_vmo(unidirectional_clone));
        }

        {
            // Same as above; use the parent instead of the child though.
            let (vmo, _) = unwrap_ok!(make_committed_pager_vmo::<2>(
                /*trap_dirty=*/ false, /*resizable=*/ false,
            ));
            let _unidirectional_clone = unwrap_ok!(vmo.create_clone(
                Resizability::NonResizable,
                SnapshotType::OnWrite,
                0,
                2 * PAGE_SIZE,
                false,
            ));
            expect_true!(test_vmo(VmObjectPaged::into_vm_object(vmo)));
        }

        {
            let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, 2 * PAGE_SIZE));
            assert_ok!(vmo.commit_range(0, 2 * PAGE_SIZE));

            let bidirectional_clone = unwrap_ok!(vmo.create_clone(
                Resizability::NonResizable,
                SnapshotType::Full,
                0,
                2 * PAGE_SIZE,
                false,
            ));
            expect_true!(test_vmo(bidirectional_clone));
        }
    }

    /// Tests that dirty pages cannot be deduped.
    #[test]
    fn vmo_dedup_dirty_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(false, false));

        // Our page should now be in a pager backed page queue.
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_reclaim(page) }.is_some());

        // The page is clean. We should be able to dedup the page.
        expect_true!(vmo.debug_get_cow_pages().unwrap().dedup_zero_page(page, 0));

        // No committed pages remaining.
        expect_true!(attribution::zero() == vmo.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo, 0));

        // Write to the page making it dirty.
        let data = 0xffu8;
        assert_ok!(vmo.write(0, &[data]));

        // The page should now be dirty.
        let page = vmo.debug_get_page(0).unwrap();
        // SAFETY: `page` is attached to `vmo`.
        expect_true!(unsafe { pmm::page_queues().debug_page_is_pager_backed_dirty(page) });

        // We should not be able to dedup the page.
        expect_false!(vmo.debug_get_cow_pages().unwrap().dedup_zero_page(page, 0));
        expect_true!(make_private_attribution_counts(PAGE_SIZE, 0) == vmo.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo, PAGE_SIZE));
    }

    /// Tests that snapshot modified behaves as expected
    #[test]
    fn vmo_snapshot_modified_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // Create 3 page, pager-backed VMO.
        const NUM_PAGES: usize = 3;
        let alloc_size = (NUM_PAGES as u64) * PAGE_SIZE;

        let (vmo, _pages) = unwrap_ok!(make_committed_pager_vmo::<NUM_PAGES>(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));
        vmo.set_user_id(42);

        // Snapshot-modified all 3 pages of root.
        let clone = unwrap_ok!(
            vmo.create_clone(
                Resizability::NonResizable,
                SnapshotType::Modified,
                0,
                alloc_size,
                false,
            ),
            "vmobject full clone\n"
        );
        clone.set_user_id(43);

        // Hang another snapshot-modified clone off root that only sees the first page.
        let clone2 = unwrap_ok!(
            vmo.create_clone(
                Resizability::NonResizable,
                SnapshotType::Modified,
                0,
                PAGE_SIZE,
                false,
            ),
            "vmobject partial clone\n"
        );
        clone2.set_user_id(44);

        // Ensures all pages are attributed to the root VMO, and not the clones, as the root VMO is
        // not a hidden node.
        expect_true!(make_private_attribution_counts(alloc_size, 0) == vmo.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&vmo, alloc_size));
        expect_true!(attribution::zero() == clone.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&clone, 0));
        expect_true!(attribution::zero() == clone2.get_attributed_memory());
        expect_true!(verify_continuous_attribution_bytes(&clone2, 0));

        // COW page into clone & check that it is attributed.
        let data = 0xffu8;
        assert_ok!(clone.write(0, &[data]));

        expect_true!(
            make_private_attribution_counts(PAGE_SIZE, 0) == clone.get_attributed_memory()
        );
        expect_true!(verify_continuous_attribution_bytes(&clone, PAGE_SIZE));

        // Try to COW a page into clone2 that it doesn't see.
        let status = clone2.write(PAGE_SIZE, &[data]);
        assert_eq!(Status::OUT_OF_RANGE.into_raw(), Status::result_into_raw(status));

        // Call snapshot-modified again on the full clone, which will create a hidden parent.
        let snapshot = unwrap_ok!(
            clone.create_clone(
                Resizability::NonResizable,
                SnapshotType::Modified,
                0,
                PAGE_SIZE * (NUM_PAGES as u64),
                false,
            ),
            "vmobject snapshot-modified\n"
        );

        // Pages in hidden parent will be attributed to both children.
        expect_true!(
            (attribution::AttributionCounts {
                uncompressed_bytes: PAGE_SIZE as usize,
                scaled_uncompressed_bytes: attribution::fractional_bytes_from_fraction(
                    PAGE_SIZE, 2
                ),
                ..attribution::zero()
            }) == clone.get_attributed_memory()
        );
        expect_true!(
            (attribution::AttributionCounts {
                uncompressed_bytes: PAGE_SIZE as usize,
                scaled_uncompressed_bytes: attribution::fractional_bytes_from_fraction(
                    PAGE_SIZE, 2
                ),
                ..attribution::zero()
            }) == snapshot.get_attributed_memory()
        );

        // Calling CreateClone directly with SnapshotAtLeastOnWrite should upgrade to
        // snapshot-modified.
        let _atleastonwrite = unwrap_ok!(
            clone.create_clone(
                Resizability::NonResizable,
                SnapshotType::OnWrite,
                0,
                alloc_size,
                false,
            ),
            "vmobject snapshot-at-least-on-write clone.\n"
        );

        // Create a slice of the first two pages of the root VMO.
        let slice_size = 2 * PAGE_SIZE;
        let slice = unwrap_ok!(vmo.create_child_slice(0, slice_size, false), "slice root vmo");
        slice.set_user_id(45);

        // The oot VMO should have 3 children at this point.
        assert_eq!(vmo.num_children(), 3);

        // Snapshot-modified of root-slice should work.
        let slicesnapshot = unwrap_ok!(
            slice.create_clone(
                Resizability::NonResizable,
                SnapshotType::Modified,
                0,
                slice_size,
                false,
            ),
            "snapshot-modified root-slice\n"
        );
        slicesnapshot.set_user_id(46);

        // At the VMO level, the slice should see the snapshot as a child.
        assert_eq!(vmo.num_children(), 3);
        assert_eq!(slice.num_children(), 1);

        // The cow pages, however, should be hung off the root VMO.
        let slicesnapshot_p = VmObject::downcast_paged(slicesnapshot.clone()).expect("is paged");
        let vmo_cow_pages = vmo.debug_get_cow_pages().expect("vmo has cow pages");
        let slicesnapshot_cow_pages =
            slicesnapshot_p.debug_get_cow_pages().expect("slicesnapshot has cow pages");

        assert_eq!(
            slicesnapshot_cow_pages.debug_get_parent().map(|p| p.as_raw()).unwrap_or_default(),
            vmo_cow_pages.as_raw()
        );

        // Create a slice of the clone of the root-slice.
        let slicesnapshot_slice = unwrap_ok!(
            slicesnapshot.create_clone(
                Resizability::NonResizable,
                SnapshotType::Modified,
                0,
                slice_size,
                false,
            ),
            "slice snapshot-modified-root-slice\n"
        );
        slicesnapshot_slice.set_user_id(47);

        // Check that snapshot-modified will work again on the snapshot-modified clone of the slice.
        let slicesnapshot2 = unwrap_ok!(
            slicesnapshot.create_clone(
                Resizability::NonResizable,
                SnapshotType::Modified,
                0,
                slice_size,
                false,
            ),
            "snapshot-modified root-slice-snapshot\n"
        );
        slicesnapshot2.set_user_id(48);

        // Create a slice of a clone
        let cloneslice =
            unwrap_ok!(clone.create_child_slice(0, slice_size, false), "slice root vmo");

        // Snapshot-modified should not be allowed on a slice of a clone.
        let status = cloneslice.create_clone(
            Resizability::NonResizable,
            SnapshotType::Modified,
            0,
            slice_size,
            false,
        );
        assert_eq!(
            Status::NOT_SUPPORTED.into_raw(),
            Status::result_into_raw(status.map(|_| ())),
            "snapshot-modified clone-slice\n"
        );

        // Tests that SnapshotModified will be upgraded to Snapshot when used on an anonymous VMO.
        let anon_vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size));
        anon_vmo.set_user_id(0x49);

        let anon_clone = unwrap_ok!(anon_vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Modified,
            0,
            PAGE_SIZE,
            true,
        ));
        anon_clone.set_user_id(0x50);

        // Check that a hidden, common cow pages was made.
        let anon_clone_p = VmObject::downcast_paged(anon_clone.clone()).expect("is paged");
        let anon_vmo_cow_pages = anon_vmo.debug_get_cow_pages().expect("anon_vmo has cow pages");
        let anon_clone_cow_pages =
            anon_clone_p.debug_get_cow_pages().expect("anon_clone has cow pages");

        assert_eq!(
            anon_clone_cow_pages.debug_get_parent().map(|p| p.as_raw()).unwrap_or_default(),
            anon_vmo_cow_pages.debug_get_parent().map(|p| p.as_raw()).unwrap_or_default()
        );

        // Snapshot-modified should also be upgraded when used on a SNAPSHOT clone.
        let anon_snapshot = unwrap_ok!(anon_clone.create_clone(
            Resizability::NonResizable,
            SnapshotType::Modified,
            0,
            PAGE_SIZE,
            true,
        ));
        anon_snapshot.set_user_id(0x51);

        // Snapshot-modified should not be allowed on a unidirectional chain of length > 2
        let chain1 = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            PAGE_SIZE,
            true,
        ));
        chain1.set_user_id(0x52);
        let data1 = 42u64;
        expect_ok!(chain1.write(0, &data1.to_ne_bytes()[..core::mem::size_of_val(&data)]));

        let chain2 = unwrap_ok!(chain1.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            0,
            PAGE_SIZE,
            true,
        ));
        chain2.set_user_id(0x51);
        let data2 = 43u64;
        expect_ok!(chain2.write(0, &data2.to_ne_bytes()[..core::mem::size_of_val(&data)]));

        let chain_snap = chain2.create_clone(
            Resizability::NonResizable,
            SnapshotType::Modified,
            0,
            PAGE_SIZE,
            true,
        );
        assert_eq!(
            Status::NOT_SUPPORTED.into_raw(),
            Status::result_into_raw(chain_snap.map(|_| ())),
            "snapshot-modified unidirectional chain\n"
        );
    }

    /// Tests concurrent pinning of different ranges in a contiguous VMO with loaned pages.
    #[test]
    fn vmo_pin_race_loaned_test() {
        // Regression test for https://fxbug.dev/42080926. Concurrent pinning of different ranges in
        // a contiguous VMO that has its pages loaned.

        let _scanner_disable = AutoVmScannerDisable::new();

        let try_count = 5000;
        for _try_ordinal in 0..try_count {
            let _enable_loaning = ScopedLoaningEnabled::new(true);

            const NUM_LOANED: usize = 10;
            let contiguous_vmo = unwrap_ok!(VmObjectPaged::create_contiguous(
                pmm::ALLOC_FLAG_ANY,
                ((NUM_LOANED + 1) as u64) * PAGE_SIZE,
                /*alignment_log2=*/ 0,
            ));
            let mut pages: [Option<VmPagePtr>; NUM_LOANED] = [None; NUM_LOANED];
            for (i, page) in pages.iter_mut().enumerate() {
                *page = contiguous_vmo.debug_get_page(((i + 1) as u64) * PAGE_SIZE);
            }
            let status = contiguous_vmo.decommit_range(PAGE_SIZE, (NUM_LOANED as u64) * PAGE_SIZE);
            assert_true!(status.is_ok());

            let mut iteration_count = 0;
            let max_iterations = 1000;
            let mut loaned = 0;
            loop {
                // Create a pager-backed VMO with a single page.
                let (vmo, [page]) = unwrap_ok!(make_committed_pager_vmo(
                    /*trap_dirty=*/ false, /*resizable=*/ false,
                ));

                // make_committed_pager_vmo is not enough to ensure vmo's only page is loaned.
                // We must explicitly call replace_page_with_loaned.
                let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
                let offset = 0;
                assert_ok!(cow_pages.replace_page_with_loaned(page, offset));

                // vmo's page should be a new page since we replaced the old one with
                // a loaned page.
                let page = vmo.debug_get_page(0).expect("vmo should have a page at offset 0");

                iteration_count += 1;
                for &saved_page in &pages {
                    if page == saved_page.expect("pages populated") {
                        // SAFETY: `page` is attached to `vmo` and its loaned state is not mutated
                        // in parallel.
                        assert_true!(unsafe { page.is_loaned() });
                        loaned += 1;
                    }
                }

                if !(loaned < NUM_LOANED && iteration_count < max_iterations) {
                    break;
                }
            }

            // If we hit this iteration count, something almost certainly went wrong...
            assert_true!(iteration_count < max_iterations);
            assert_eq!(NUM_LOANED, loaned);

            let mut threads: [Option<ThreadPtr>; NUM_LOANED] = [None; NUM_LOANED];
            struct ThreadState {
                vmo: *const VmObjectPaged,
                index: usize,
            }
            let mut states =
                [const { ThreadState { vmo: core::ptr::null(), index: 0 } }; NUM_LOANED];

            extern "C" fn worker(arg: *mut c_void) -> i32 {
                let state_ptr: *const ThreadState = arg.cast();
                // SAFETY: `state_ptr` points to a valid, initialized `ThreadState` on the stack
                // that outlives the thread.
                let state = unsafe { state_ptr.as_ref_unchecked() };
                // SAFETY: `state.vmo` points to `contiguous_vmo` which outlives the thread.
                let vmo = unsafe { state.vmo.as_ref_unchecked() };

                let status = if state.index == 0 {
                    vmo.commit_range_pinned(0, 2 * PAGE_SIZE, false)
                } else {
                    vmo.commit_range_pinned(
                        ((state.index + 1) as u64) * PAGE_SIZE,
                        PAGE_SIZE,
                        false,
                    )
                };
                if status.is_err() {
                    return -1;
                }
                0
            }

            for i in 0..NUM_LOANED {
                states[i].vmo = &*contiguous_vmo;
                states[i].index = i;
                let state_ptr: *mut ThreadState = &mut states[i];
                let arg: *mut c_void = state_ptr.cast();
                // SAFETY: `worker` is a valid entry point and `arg` points to `states[i]` which
                // outlives the thread.
                threads[i] =
                    Some(unwrap_ok!(unsafe { thread::create(c"worker".as_ptr(), worker, arg) }));
            }

            for thread in &threads {
                // SAFETY: `thread` is a valid thread pointer and has not been joined or
                // destroyed.
                unsafe { thread.unwrap().resume() };
            }

            for thread in &threads {
                // SAFETY: `thread` is a valid thread pointer and has not been joined yet.
                let ret = unwrap_ok!(unsafe { thread.unwrap().join(InstantMono::INFINITE) });
                expect_eq!(0, ret);
            }

            for (i, &page) in pages.iter().enumerate() {
                expect_true!(page == contiguous_vmo.debug_get_page(((i + 1) as u64) * PAGE_SIZE));
            }
            contiguous_vmo.unpin(0, ((NUM_LOANED + 1) as u64) * PAGE_SIZE);
        }
    }

    /// Tests supplying pages to a pager-backed VMO and its clones.
    #[test]
    fn vmo_pager_supply_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        const NUM_PAGES: usize = 4;
        let alloc_size = (NUM_PAGES as u64) * PAGE_SIZE;
        let half_size = alloc_size / 2;
        let alloc_size_usize = NUM_PAGES * PAGE_SIZE_USIZE;
        let half_size_usize = alloc_size_usize / 2;

        // Aux VMO.
        let aux_vmo = unwrap_ok!(VmObjectPaged::create(
            pmm::ALLOC_FLAG_ANY,
            VmObjectPaged::RESIZABLE,
            alloc_size,
        ));

        // Pager-backed VMO.
        let vmo = unwrap_ok!(make_uncommitted_pager_vmo(
            NUM_PAGES, /*trap_dirty=*/ false, /*resizable=*/ false,
        ));
        vmo.set_user_id(0x42);

        // Supply pager VMO with 2 pages of random data.
        let mut buf_rand1 = Vector::<MaybeUninit<u8>>::new();
        assert_true!(buf_rand1.resize_with(half_size_usize, MaybeUninit::uninit).is_ok());
        let buf_rand1 = fill_region(0x77, &mut buf_rand1[..half_size_usize]);
        assert_ok!(aux_vmo.write(0, &buf_rand1[..half_size_usize]));

        stack_pin_init!(let sl = VmPageSpliceList::new());
        expect_ok!(aux_vmo.take_pages(0, half_size, sl.as_mut()));
        assert_ok!(vmo.supply_pages(0, half_size, sl.as_mut(), SupplyOptions::PagerSupply));
        debug_assert!(sl.is_processed());

        // Change data in aux vmo.
        let mut buf_rand2 = Vector::<MaybeUninit<u8>>::new();
        assert_true!(buf_rand2.resize_with(alloc_size_usize, MaybeUninit::uninit).is_ok());
        let buf_rand2 = fill_region(0x88, &mut buf_rand2[..alloc_size_usize]);
        expect_ok!(aux_vmo.write(0, &buf_rand2[..alloc_size_usize]));

        // Supply 4 pages of new data to the VMO.
        stack_pin_init!(let sl2 = VmPageSpliceList::new());
        expect_ok!(aux_vmo.take_pages(0, alloc_size, sl2.as_mut()));
        assert_ok!(vmo.supply_pages(0, alloc_size, sl2.as_mut(), SupplyOptions::PagerSupply));
        debug_assert!(sl2.is_processed());

        let mut buf_check = Vector::<MaybeUninit<u8>>::new();
        assert_true!(buf_check.resize_with(alloc_size_usize, MaybeUninit::uninit).is_ok());
        let buf_check_init = unwrap_ok!(vmo.read(0, &mut buf_check[..alloc_size_usize]));

        // First two shouldn't have been overwritten.
        let mut cmpres = buf_rand1[..half_size_usize] == buf_check_init[..half_size_usize];
        expect_true!(cmpres);

        // Second 2 pages should have new data.
        cmpres = buf_rand2[half_size_usize..half_size_usize + half_size_usize]
            == buf_check_init[half_size_usize..half_size_usize + half_size_usize];
        expect_true!(cmpres);

        // VMO should have 4 attributed pages.
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );

        // Clone pager-backed VMO.
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Modified,
            0,
            alloc_size,
            true,
        ));
        clone.set_user_id(0x43);

        // Vmo is attributed all pages
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );
        expect_true!(clone.get_attributed_memory() == make_private_attribution_counts(0, 0));

        // New random data in aux_vmo.
        let mut buf_rand3 = Vector::<MaybeUninit<u8>>::new();
        assert_true!(buf_rand3.resize_with(alloc_size_usize, MaybeUninit::uninit).is_ok());
        let buf_rand3 = fill_region(0x99, &mut buf_rand3[..alloc_size_usize]);
        expect_ok!(aux_vmo.write(0, &buf_rand3[..alloc_size_usize]));

        // Supply 2 pages into the middle of the clone.
        stack_pin_init!(let sl3 = VmPageSpliceList::new());
        expect_ok!(aux_vmo.take_pages(PAGE_SIZE, half_size, sl3.as_mut()));
        assert_ok!(clone.supply_pages(
            PAGE_SIZE,
            half_size,
            sl3.as_mut(),
            SupplyOptions::TransferData
        ));
        debug_assert!(sl3.is_processed());

        // Clone is attributed both pages, VMO is unchanged.
        expect_true!(
            clone.get_attributed_memory() == make_private_attribution_counts(2 * PAGE_SIZE, 0)
        );
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );

        let buf_check_init = unwrap_ok!(clone.read(0, &mut buf_check[..alloc_size_usize]));

        // First and last page in clone should be read from parent.
        cmpres = buf_rand1[..PAGE_SIZE_USIZE] == buf_check_init[..PAGE_SIZE_USIZE];
        expect_true!(cmpres);
        cmpres = buf_rand2[(alloc_size_usize - PAGE_SIZE_USIZE)
            ..(alloc_size_usize - PAGE_SIZE_USIZE) + PAGE_SIZE_USIZE]
            == buf_check_init[(alloc_size_usize - PAGE_SIZE_USIZE)
                ..(alloc_size_usize - PAGE_SIZE_USIZE) + PAGE_SIZE_USIZE];
        expect_true!(cmpres);

        // Middle pages should be new.
        cmpres = buf_rand3[PAGE_SIZE_USIZE..PAGE_SIZE_USIZE + half_size_usize]
            == buf_check_init[PAGE_SIZE_USIZE..PAGE_SIZE_USIZE + half_size_usize];
        expect_true!(cmpres);

        // Parent should be unchanged.
        let buf_check_init = unwrap_ok!(vmo.read(0, &mut buf_check[..alloc_size_usize]));
        cmpres = buf_rand1[..half_size_usize] == buf_check_init[..half_size_usize];
        expect_true!(cmpres);
        cmpres = buf_rand2[half_size_usize..half_size_usize + half_size_usize]
            == buf_check_init[half_size_usize..half_size_usize + half_size_usize];
        expect_true!(cmpres);

        // New random data in aux_vmo.
        let mut buf_rand4 = Vector::<MaybeUninit<u8>>::new();
        assert_true!(buf_rand4.resize_with(alloc_size_usize, MaybeUninit::uninit).is_ok());
        let buf_rand4 = fill_region(0x99, &mut buf_rand4[..alloc_size_usize]);
        expect_ok!(aux_vmo.write(0, &buf_rand4[..alloc_size_usize]));

        // Supply new data to all pages of clone.
        stack_pin_init!(let sl4 = VmPageSpliceList::new());
        expect_ok!(aux_vmo.take_pages(0, alloc_size, sl4.as_mut()));
        assert_ok!(clone.supply_pages(0, alloc_size, sl4.as_mut(), SupplyOptions::TransferData));
        debug_assert!(sl4.is_processed());

        // Clone should have new data.
        let buf_check_init = unwrap_ok!(clone.read(0, &mut buf_check[..alloc_size_usize]));
        cmpres = buf_rand4[..alloc_size_usize] == buf_check_init[..alloc_size_usize];
        expect_true!(cmpres);

        // Parent should be unchanged.
        let buf_check_init = unwrap_ok!(vmo.read(0, &mut buf_check[..alloc_size_usize]));
        cmpres = buf_rand1[..half_size_usize] == buf_check_init[..half_size_usize];
        expect_true!(cmpres);
        cmpres = buf_rand2[half_size_usize..half_size_usize + half_size_usize]
            == buf_check_init[half_size_usize..half_size_usize + half_size_usize];
        expect_true!(cmpres);

        // Each have 4 attributed pages.
        expect_true!(
            clone.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );
        expect_true!(
            vmo.get_attributed_memory() == make_private_attribution_counts(4 * PAGE_SIZE, 0)
        );

        // Clone the clone, which should create a hidden node.
        let clone2 = unwrap_ok!(clone.create_clone(
            Resizability::NonResizable,
            SnapshotType::Modified,
            0,
            alloc_size,
            true,
        ));
        clone2.set_user_id(0x44);

        // Private attribution counts 0 because pages were moved into hidden node.
        expect_true!(attribution::total_private_bytes(&clone.get_attributed_memory()) == 0);
        expect_true!(attribution::total_private_bytes(&clone2.get_attributed_memory()) == 0);

        // Each clone has 2 pages of scaled bytes, as they share 4 pages.
        expect_true!(
            attribution::total_scaled_bytes(&clone.get_attributed_memory())
                == attribution::fractional_bytes_from_whole(2 * PAGE_SIZE)
        );
        expect_true!(
            attribution::total_scaled_bytes(&clone2.get_attributed_memory())
                == attribution::fractional_bytes_from_whole(2 * PAGE_SIZE)
        );

        // Change data in aux VMO.
        let mut buf_rand5 = Vector::<MaybeUninit<u8>>::new();
        assert_true!(buf_rand5.resize_with(alloc_size_usize, MaybeUninit::uninit).is_ok());
        let buf_rand5 = fill_region(0xaa, &mut buf_rand5[..alloc_size_usize]);
        expect_ok!(aux_vmo.write(0, &buf_rand5[..alloc_size_usize]));

        // Supply 2 pages to Clone2.
        stack_pin_init!(let sl5 = VmPageSpliceList::new());
        expect_ok!(aux_vmo.take_pages(0, half_size, sl5.as_mut()));
        assert_ok!(clone2.supply_pages(0, half_size, sl5.as_mut(), SupplyOptions::TransferData));
        debug_assert!(sl5.is_processed());

        // Clone2 should have the two private pages and 3 scaled pages.
        expect_true!(
            attribution::total_private_bytes(&clone2.get_attributed_memory()) == 2 * PAGE_SIZE
        );
        expect_true!(
            attribution::total_scaled_bytes(&clone2.get_attributed_memory())
                == attribution::fractional_bytes_from_whole(3 * PAGE_SIZE)
        );

        // Clone should now have 3 scaled pages as two are no longer seen by clone2.
        expect_true!(
            attribution::total_scaled_bytes(&clone.get_attributed_memory())
                == attribution::fractional_bytes_from_whole(3 * PAGE_SIZE)
        );
    }

    /// Test that unmaps propagated to copy-on-write children are not applied to kernel mappings.
    #[test]
    fn vmo_apply_unmap_to_child_with_kernel_mapping_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let (vmo, _) = unwrap_ok!(make_committed_pager_vmo::<4>(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));

        let unidirectional_clone_no_paged = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            PAGE_SIZE,
            3 * PAGE_SIZE,
            false,
        ));
        let unidirectional_clone = VmObject::downcast_paged(unidirectional_clone_no_paged);
        assert_true!(unidirectional_clone.is_some());
        let unidirectional_clone = unidirectional_clone.unwrap();

        let ka = VmAspace::kernel_aspace();
        // SAFETY: The flags and range are appropriate for creating this mapping.
        let ptr = unwrap_ok!(unsafe {
            ka.map_object_internal(
                VmObjectPaged::into_vm_object(unidirectional_clone.clone()),
                c"test",
                /*offset=*/ PAGE_SIZE,
                /*size=*/ PAGE_SIZE as usize,
                0,
                vmm_flag::COMMIT,
                ARCH_RW_FLAGS,
            )
        });
        struct DeferCleanupMapping(usize);
        impl Drop for DeferCleanupMapping {
            fn drop(&mut self) {
                // SAFETY: self.0 was allocated via map_object_internal and is not yet freed.
                let status = unsafe { VmAspace::kernel_aspace().free_region(self.0) };
                assert!(status.is_ok());
            }
        }
        let _cleanup_mapping = DeferCleanupMapping(ptr as usize);

        // Show that this is indeed a unidirectional clone, and that the kernel mapping will be
        // subject to the attempted unmap.
        let page = unidirectional_clone
            .debug_get_cow_pages()
            .expect("clone has cow pages")
            .debug_get_page(PAGE_SIZE);
        assert_true!(page.is_some());
        let page = page.unwrap();
        // SAFETY: page is attached to unidirectional_clone.
        expect_gt!(unsafe { page.get_pin_count() }, 0);
        expect_true!(unidirectional_clone.debug_get_cow_pages().unwrap().debug_is_empty(0));
        expect_true!(
            unidirectional_clone.debug_get_cow_pages().unwrap().debug_is_empty(2 * PAGE_SIZE)
        );
        expect_eq!(
            unidirectional_clone
                .debug_get_cow_pages()
                .unwrap()
                .debug_get_parent()
                .unwrap()
                .as_raw(),
            vmo.debug_get_cow_pages().unwrap().as_raw()
        );

        // This does not crash.
        expect_ok!(vmo.zero_range(0, 4 * PAGE_SIZE));
    }

    /// Tests creating a zero-sized always-pinned VMO fails gracefully.
    #[test]
    fn vmo_always_pinned_with_no_pages_test() {
        // Verify that we don't trigger a panic during destruction of an always-pinned, but empty VMO.
        //
        // This is a regression test for https://fxbug.dev/511552403.

        let vmo = VmObjectPaged::create(ALLOC_FLAG_ANY, VmObjectPaged::ALWAYS_PINNED, 0);
        // Note that this call will fail.  That's because we've requested a zero-sized always-pinned
        // VMO, which is not a valid request.  However, under the hood, we'll make it far enough to create
        // the VMO even thought it will be destroyed before the call returns.
        assert_eq!(Status::result_into_raw(vmo.map(|_| ())), Status::INVALID_ARGS.into_raw());
    }

    /// Verify that LookupReadableLocked works for a simple VMO with all pages committed.
    #[test]
    fn vmo_lookup_readable_simple_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        let page_count = 4;
        let alloc_size = PAGE_SIZE * page_count;

        // Create a VMO.
        let vmo = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size));

        // Commit the whole VMO.
        let status = vmo.commit_range(0, alloc_size);
        assert_ok!(status);

        // Lookup readable on the VMO should find all 4 pages.
        let mut pages_seen = 0;
        let lookup_fn = |_offset: u64, _pa: PAddr, pages_seen: &mut u64| {
            *pages_seen += 1;
            Err(Status::NEXT)
        };

        let vmo_cow = vmo.debug_get_cow_pages().expect("vmo has cow pages");
        let status = vmo_cow.debug_lookup_readable(
            VmCowRange { offset: 0, len: alloc_size },
            &mut pages_seen,
            lookup_fn,
        );
        expect_ok!(status);
        expect_eq!(page_count, pages_seen);
    }

    /// Tests that LookupReadableLocked works when a parent VMO lookup segment is split.
    #[test]
    fn vmo_lookup_readable_clone_test() {
        // Verify that LookupReadableLocked works when a parent VMO lookup segment is
        // immediately followed by a page committed locally in the clone VMO (which splits
        // the parent lookup). This is a regression test for https://fxbug.dev/513654391.
        let _scanner_disable = AutoVmScannerDisable::new();

        let page_count = 4;
        let alloc_size = PAGE_SIZE * page_count;

        // Create a parent VMO.
        let parent = unwrap_ok!(VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, alloc_size));

        parent.set_user_id(42);

        // Commit the whole parent VMO.
        assert_ok!(parent.commit_range(0, alloc_size));

        // Create a COW clone of the parent.
        let clone_no_paged = unwrap_ok!(parent.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            alloc_size,
            false,
        ));

        clone_no_paged.set_user_id(43);
        let clone = VmObject::downcast_paged(clone_no_paged);
        assert_true!(clone.is_some());
        let clone = clone.unwrap();

        // Commit page 1 in the clone to split the parent lookup.
        assert_ok!(clone.commit_range(PAGE_SIZE, PAGE_SIZE));

        // Lookup readable on the clone VMO should find all 4 pages.
        let mut pages_seen = 0;
        let lookup_fn = |_offset: u64, _pa: PAddr, pages_seen: &mut u64| {
            *pages_seen += 1;
            Err(Status::NEXT)
        };

        let clone_cow = clone.debug_get_cow_pages().unwrap();
        let status = clone_cow.debug_lookup_readable(
            VmCowRange { offset: 0, len: alloc_size },
            &mut pages_seen,
            lookup_fn,
        );
        expect_ok!(status);
        expect_eq!(page_count, pages_seen);
    }

    /// Tests accessing all offsets of a VMO via GetPage.
    #[test]
    fn vmo_get_page_offset_test() {
        // Test that all offsets of a VMO are accessible via GetPage.
        //
        // This is a regression test for https://fxbug.dev/515752748.
        let _scanner_disable = AutoVmScannerDisable::new();

        let size = 10 * PAGE_SIZE;
        // Request zero committed pages with [].
        let (vmo, []) = unwrap_ok!(make_partially_committed_pager_vmo(
            10, /*trap_dirty=*/ false, /*resizable=*/ false,
            /*ignore_requests=*/ true
        ));

        for i in (0..size).step_by(PAGE_SIZE_USIZE) {
            // Use fault::flag::FAULT_MASK so that GetPage attempts to acquire a page if none is
            // present in the local page list.
            stack_pin_init!(let page_request = MultiPageRequest::new());
            // SAFETY: Test owns the VMO and page request, meeting all underlying safety
            // obligations.
            let status =
                unsafe { vmo.get_page(i, fault::flag::FAULT_MASK, Some(page_request.as_mut())) };
            if status == Err(Status::SHOULD_WAIT) {
                // The stub page provider does not support waiting.
                page_request.cancel_requests();
            } else {
                expect_ok!(status.map(|_| ()));
            }
        }
    }

    /// Tests that snapshot creation inherits ever_pinned_ into the hidden parent.
    #[test]
    fn vmo_ever_pinned_hidden_parent_creation_test() {
        // Tests that when creating a bidirectional clone (snapshot) of a once-pinned VMO,
        // the newly created hidden parent correctly inherits the `ever_pinned_` flag.
        let _scanner_disable = AutoVmScannerDisable::new();

        let vmo_size = PAGE_SIZE;

        // Create root VMO.
        let vmo = unwrap_ok!(VmObjectPaged::create(0, 0, vmo_size));

        // Commit a page at offset 0.
        let val: u32 = 0x42;
        assert_ok!(vmo.write(0, &val.to_le_bytes()));

        let cow = vmo.debug_get_cow_pages().expect("vmo has cow pages");

        // Initially, ever_pinned_ should be false.
        expect_true!(cow.should_delay_reuse_on_free() == PmmOptDelayReuse::Default);

        // Pin the page.
        assert_ok!(vmo.commit_range_pinned(0, vmo_size, true));

        // ever_pinned_ should be true.
        expect_true!(cow.should_delay_reuse_on_free() == PmmOptDelayReuse::Yes);

        // Unpin the page.
        vmo.unpin(0, vmo_size);

        // ever_pinned_ should still be true.
        expect_true!(cow.should_delay_reuse_on_free() == PmmOptDelayReuse::Yes);

        // Create a bidirectional clone (snapshot) of the root VMO.
        let _clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            vmo_size,
            true
        ));

        // Retrieve the hidden parent.
        let h_cow = cow.debug_get_parent().expect("cow has parent");

        expect_eq!(PmmOptDelayReuse::Yes, h_cow.should_delay_reuse_on_free());
        expect_eq!(PmmOptDelayReuse::Default, cow.should_delay_reuse_on_free());
    }

    /// Tests that page migration into a sibling clone inherits the ever_pinned_ flag.
    #[test]
    fn vmo_ever_pinned_page_migration_test() {
        // Tests that when a once-pinned page is migrated into a sibling clone during copy-on-write
        // page migration, the sibling clone correctly inherits the `ever_pinned_` flag.
        let _scanner_disable = AutoVmScannerDisable::new();

        let vmo_size = PAGE_SIZE;

        // Create root VMO.
        let vmo = unwrap_ok!(VmObjectPaged::create(0, 0, vmo_size));

        // Commit a page at offset 0.
        let val: u32 = 0x42;
        assert_ok!(vmo.write(0, &val.to_le_bytes()));

        let _cow = vmo.debug_get_cow_pages().expect("vmo has cow pages");

        // Pin and unpin the page.
        assert_ok!(vmo.commit_range_pinned(0, vmo_size, true));
        vmo.unpin(0, vmo_size);

        // Create a bidirectional clone (snapshot) of the root VMO.
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            vmo_size,
            true
        ));

        let c_cow = VmObject::downcast_paged(clone.clone())
            .expect("is paged")
            .debug_get_cow_pages()
            .expect("clone has cow pages");

        // The sibling clone is created with ever_pinned_ = false.
        expect_true!(c_cow.should_delay_reuse_on_free() == PmmOptDelayReuse::Default);

        // Write to the root VMO to fork the page. The original once-pinned page in the hidden
        // parent is now only visible to the clone.
        let val2: u32 = 0x43;
        assert_ok!(vmo.write(0, &val2.to_le_bytes()));

        // Write to the clone to trigger page migration from the hidden parent to the clone.
        assert_ok!(clone.write(0, &val.to_le_bytes()));

        // The clone should now have ever_pinned_ = true since the once-pinned page was migrated
        // into it.
        expect_eq!(PmmOptDelayReuse::Yes, c_cow.should_delay_reuse_on_free());
    }

    /// Tests that hidden parent collapse and page merge into a child clone inherits ever_pinned_.
    #[test]
    fn vmo_ever_pinned_parent_merge_test() {
        // Tests that when a hidden parent collapses and merges its pages into a child clone, the
        // child clone correctly inherits the `ever_pinned_` flag.
        let _scanner_disable = AutoVmScannerDisable::new();

        let vmo_size = PAGE_SIZE;

        // Create root VMO.
        let vmo = unwrap_ok!(VmObjectPaged::create(0, 0, vmo_size));

        // Commit a page at offset 0.
        let val: u32 = 0x42;
        assert_ok!(vmo.write(0, &val.to_le_bytes()));

        let _cow = vmo.debug_get_cow_pages().expect("vmo has cow pages");

        // Pin and unpin the page.
        assert_ok!(vmo.commit_range_pinned(0, vmo_size, true));
        vmo.unpin(0, vmo_size);

        // Create a bidirectional clone (snapshot) of the root VMO.
        let clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            vmo_size,
            true
        ));

        let c_cow = VmObject::downcast_paged(clone.clone())
            .expect("is paged")
            .debug_get_cow_pages()
            .expect("clone has cow pages");

        // The sibling clone is created with ever_pinned_ = false.
        expect_true!(c_cow.should_delay_reuse_on_free() == PmmOptDelayReuse::Default);

        // Close the root VMO. This merges the hidden parent's pages into the clone.
        drop(vmo);

        // The clone should now have ever_pinned_ = true since the hidden parent collapsed and
        // merged its pages into the clone.
        expect_eq!(PmmOptDelayReuse::Yes, c_cow.should_delay_reuse_on_free());
    }

    /// Tests that setting user stream size on a VMO does not alter VMO size.
    #[test]
    fn vmo_user_stream_size_test() {
        let _scanner_disable = AutoVmScannerDisable::new();

        // 4 page VMO.
        let vmo = unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 4 * PAGE_SIZE));

        expect_eq!(vmo.size(), 4 * PAGE_SIZE);

        // Give VMO a user-defined stream size of 2 pages.
        let ssm =
            unwrap_ok!(crate::vm::stream_size_manager::StreamSizeManager::create(2 * PAGE_SIZE));
        vmo.set_user_stream_size(ssm.clone());

        expect_eq!(ssm.get_stream_size(), 2 * PAGE_SIZE);

        // VMO size should be unchanged.
        expect_eq!(vmo.size(), 4 * PAGE_SIZE);
    }
}
