// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Unit tests for the continuous attribution tracker.
#[cfg(ktest)]
#[unittest::suite(name = "continuous_attribution_rust")]
mod tests {
    use crate::vm::attribution;
    use crate::vm::compression::CompressorGuard;
    use crate::vm::continuous_attribution_tracker::{
        ContinuousAttributionTracker, StubContinuousAttributionTracker,
    };
    use crate::vm::page::VmPageDoublyLinkedList;
    use crate::vm::page_source::MultiPageRequest;
    use crate::vm::physical_page_borrowing_config::ScopedLoaningEnabled;
    use crate::vm::pmm::{self, ALLOC_FLAG_ANY};
    use crate::vm::scanner::AutoVmScannerDisable;
    use crate::vm::vm_cow_pages::{
        CanOverwriteSlot, DeferredOps, EvictionAction, VmCowPages, VmCowRange,
    };
    use crate::vm::vm_object::{Resizability, SnapshotType, VmObject};
    use crate::vm::vm_object_paged::VmObjectPaged;
    use crate::vm::vm_page_list::VmPageSpliceList;
    use crate::vm_unittests::test_helper::{
        change_vmo_high_priority_count, make_committed_pager_vmo,
        make_partially_committed_pager_vmo,
    };
    use core::num::NonZeroI64;
    use core::pin::Pin;
    use fbl::{RefPtr, Vector};
    use kprint::kprintln;
    use ksync::lock;
    use page::SIZE as PAGE_SIZE_USIZE;
    use pin_init::stack_pin_init;
    use unittest::{
        assert_ok, assert_true, expect_eq, expect_err, expect_ok, expect_true, unwrap_ok,
    };
    use zx_status::Status;

    const PAGE_SIZE: u64 = PAGE_SIZE_USIZE as u64;

    macro_rules! should_skip_no_feature {
        () => {
            if cfg!(feature = "experimental_continuous_per_vmo_attribution_enabled") {
                false
            } else {
                kprintln!(
                    "Skipping {:s}:{:u}; no support for continuous attribution feature detected.",
                    file!(),
                    line!()
                );
                true
            }
        };
    }

    /// Writes `num_pages` pages worth of non-zero data to `vmo` starting at offset zero.
    fn write_non_zero_pages(vmo: &VmObject, num_pages: usize) -> Result<(), Status> {
        let mut data = Vector::<u8>::new();
        data.resize(num_pages * PAGE_SIZE_USIZE, 42u8).map_err(|_| Status::NO_MEMORY)?;
        vmo.write(0, &data[..])
    }

    /// Test that the continuous attribution tracker supports a "stubbed out" state.
    #[test]
    fn stub() {
        let mut tracker = StubContinuousAttributionTracker::new();

        tracker.increment(1);
        tracker.decrement(1);

        tracker.increment(100);
        tracker.decrement(100);

        tracker.increment(100);
        tracker.increment(100);
        tracker.increment(100);

        tracker.decrement(150);
        tracker.decrement(150);

        // Overflow is okay.
        tracker.decrement(3000);

        // Do not call stub FetchCurrent and FetchHwmAndReset methods, as these unconditionally
        // panic.

        let assigned = tracker;
        let moved = assigned;
        let _ = moved;
    }

    /// Test that the initial state of the ContinuousAttributionTracker is zero.
    #[test]
    fn create() {
        let mut tracker = ContinuousAttributionTracker::new();
        expect_eq!(0, tracker.fetch_current());
        expect_eq!(0, tracker.fetch_hwm_and_reset());
    }

    /// Test that the move and assignment transfers data to the new tracker object.
    #[test]
    fn transfer() {
        let mut tracker = ContinuousAttributionTracker::new();

        tracker.increment(5);

        expect_eq!(5, tracker.fetch_current());

        let mut assigned_stats = ContinuousAttributionTracker::new();
        assigned_stats.take_from(&mut tracker);

        // The old one has nothing...
        expect_eq!(0, tracker.fetch_current());

        // but the new one has the data.
        expect_eq!(5, assigned_stats.fetch_current());

        let mut constructed_stats = ContinuousAttributionTracker::new();
        constructed_stats.take_from(&mut assigned_stats);

        // The old one has nothing...
        expect_eq!(0, assigned_stats.fetch_current());

        // but the new one has the data.
        expect_eq!(5, constructed_stats.fetch_current());

        // Test that core::mem::take behaves identically to take_from.
        let mut taken_stats = core::mem::take(&mut constructed_stats);
        expect_eq!(0, constructed_stats.fetch_current());
        expect_eq!(5, taken_stats.fetch_current());

        // Only inspect the high-water mark down here because if we checked before it would have
        // been reset.
        expect_eq!(5, taken_stats.fetch_hwm_and_reset());
    }

    /// Test that the high-water mark accumulates values since last reset.
    #[test]
    fn high_water_mark() {
        let mut tracker = ContinuousAttributionTracker::new();

        tracker.increment(5);
        tracker.decrement(5);

        // The high-water mark is reset by the below.
        expect_eq!(5, tracker.fetch_hwm_and_reset());

        tracker.increment(2);
        tracker.decrement(2);
        tracker.increment(3);
        tracker.decrement(2);
        tracker.decrement(1);
        tracker.increment(2);

        expect_eq!(2, tracker.fetch_current());

        // The high-water mark is 3 even though the current value is 2, since that was the highest
        // since last reset.
        expect_eq!(3, tracker.fetch_hwm_and_reset());
    }

    /// Test that the continuous attribution tracker supports large counts.
    #[test]
    fn extreme() {
        let mut tracker = ContinuousAttributionTracker::new();
        tracker.increment(u32::MAX);
        expect_eq!(u32::MAX, tracker.fetch_current());
    }

    /// Test that writing to a VMO populates its slots.
    #[test]
    fn populate_vmo() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));

        {
            let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
            // There is no content.
            expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());
        }

        // Write a non-zero value to the first two pages.
        assert_ok!(write_non_zero_pages(&vmo, 2));

        {
            let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
            // There are two populated pages.
            expect_eq!(2u32, cow_pages.debug_get_populated_slots_count());
        }
    }

    /// Test that the correct tracker is provided to the unidirectional clone and parent.
    #[test]
    fn unidirectional_child() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let (vmo, [_page0, _page1]) = unwrap_ok!(make_partially_committed_pager_vmo(
            3, /*trap_dirty=*/ false, /*resizable=*/ false,
            /*ignore_requests=*/ false
        ));

        // Create a unidirectional clone.
        let child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::OnWrite,
            /*offset=*/ 0,
            /*size=*/ 3 * PAGE_SIZE,
            /*copy_name=*/ false
        ));
        let child = VmObject::downcast_paged(child).expect("clone is a paged VMO");

        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        // Assert there is no hidden parent (true unidirectional).
        assert_true!(cow_pages.debug_get_parent().is_none());

        // There are two pages committed in the parent.
        expect_eq!(2u32, cow_pages.debug_get_populated_slots_count());

        let child_cow_pages = child.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        // There are no parent content markers in this hierarchy to track, as intended.
        expect_eq!(0u32, child_cow_pages.debug_get_populated_slots_count());
    }

    /// Test that the correct tracker is provided to the bidirectional clone and parent.
    #[test]
    fn bidirectional_child() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));

        // Write a non-zero value to the first two pages.
        assert_ok!(write_non_zero_pages(&vmo, 2));

        // Create a bidirectional clone.
        let child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            /*offset=*/ 0,
            /*size=*/ 3 * PAGE_SIZE,
            /*copy_name=*/ false
        ));
        let child = VmObject::downcast_paged(child).expect("clone is a paged VMO");

        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        // Assert there is a hidden parent (true bidirectional).
        let hidden_parent =
            cow_pages.debug_get_parent().expect("bidirectional clone has a hidden parent");

        expect_eq!(2u32, cow_pages.debug_get_populated_slots_count());

        let child_cow_pages = child.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        expect_eq!(2u32, child_cow_pages.debug_get_populated_slots_count());

        // There are two pages committed in the parent.
        expect_eq!(2u32, hidden_parent.debug_get_populated_slots_count());
    }

    /// Zeroes the first page of `cow_pages`, returning the number of bytes zeroed.
    ///
    /// Directly calls the lower-level interface, as opposed to the method on VmObject. The
    /// attribution for the higher-level method is incomplete.
    fn zero_first_page(cow_pages: &VmCowPages) -> (Result<(), Status>, u64) {
        stack_pin_init!(let page_request = MultiPageRequest::new());
        stack_pin_init!(let deferred = DeferredOps::new(cow_pages));
        lock!(let guard = cow_pages.lock());

        cow_pages.zero_pages_locked(
            guard.token(),
            VmCowRange { offset: 0, len: PAGE_SIZE },
            /*dirty_track=*/ false,
            deferred.as_mut(),
            page_request.as_mut(),
        )
    }

    /// Test that zeroing an anonymous VMO decreases the populated slots count.
    #[test]
    fn zero_anonymous() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));
        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        // Write a non-zero value to the first two pages.
        assert_ok!(write_non_zero_pages(&vmo, 2));

        expect_eq!(2u32, cow_pages.debug_get_populated_slots_count());

        // Clear out one page, so that afterwards the VMO will only have one populated page.
        let (status, zeroed_bytes) = zero_first_page(&cow_pages);
        expect_eq!(PAGE_SIZE, zeroed_bytes);
        expect_ok!(status);

        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that zeroing a pager-backed VMO decreases the populated slots count.
    #[test]
    fn zero_pager_backed() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let (vmo, [_page0, _page1]) = unwrap_ok!(make_partially_committed_pager_vmo(
            3, /*trap_dirty=*/ false, /*resizable=*/ false,
            /*ignore_requests=*/ false
        ));
        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        expect_eq!(2u32, cow_pages.debug_get_populated_slots_count());

        // Clear out one page, so that afterwards the VMO will only have one populated page.
        let (status, zeroed_bytes) = zero_first_page(&cow_pages);
        expect_eq!(PAGE_SIZE, zeroed_bytes);
        expect_ok!(status);

        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that zeroing a child of a pager-backed VMO correctly updates the populated bytes count.
    #[test]
    fn zero_pager_clone() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let (vmo, [_page0, _page1]) = unwrap_ok!(make_partially_committed_pager_vmo(
            3, /*trap_dirty=*/ false, /*resizable=*/ false,
            /*ignore_requests=*/ false
        ));

        let child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Modified,
            /*offset=*/ 0,
            /*size=*/ 3 * PAGE_SIZE,
            /*copy_name=*/ false
        ));
        let child = VmObject::downcast_paged(child).expect("clone is a paged VMO");

        let child_cow_pages = child.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        expect_eq!(0u32, child_cow_pages.debug_get_populated_slots_count());

        assert_ok!(child.commit_range(0, 2 * PAGE_SIZE));

        expect_eq!(2u32, child_cow_pages.debug_get_populated_slots_count());

        // Clear out one page, so that afterwards the VMO will only have one populated page.
        let (status, zeroed_bytes) = zero_first_page(&child_cow_pages);
        expect_eq!(PAGE_SIZE, zeroed_bytes);
        expect_ok!(status);

        expect_eq!(1u32, child_cow_pages.debug_get_populated_slots_count());
    }

    /// Test that removing a page from a hidden parent decrements populated bytes count.
    #[test]
    fn require_move_page() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        // Set up a hidden parent with two pages and children with no content.

        let child1: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));

        // Write a non-zero value to the first two pages.
        assert_ok!(write_non_zero_pages(&child1, 2));

        // Create a bidirectional clone.
        let child2 = unwrap_ok!(child1.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            /*offset=*/ 0,
            /*size=*/ 3 * PAGE_SIZE,
            /*copy_name=*/ false
        ));
        let child2 = VmObject::downcast_paged(child2).expect("clone is a paged VMO");
        let child2_cow = child2.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        // Assert there is a hidden parent (true bidirectional).
        let hidden_parent_cow =
            child2_cow.debug_get_parent().expect("bidirectional clone has a hidden parent");

        // Decrement the share count for the first page by making child1 copy-on-write the first
        // page.
        expect_ok!(child1.commit_range(0, PAGE_SIZE));

        // Now the share count for the first page is just one in the hidden parent.

        // The content is attributed to both the parent and the child because the pages are resident
        // in the hidden parent and the child has parent content markers.
        expect_eq!(2u32, hidden_parent_cow.debug_get_populated_slots_count());
        expect_eq!(2u32, child2_cow.debug_get_populated_slots_count());

        expect_ok!(child2.commit_range(0, PAGE_SIZE));

        // The hidden parent's attribution count is decremented because it no longer has the page
        // resident (it has been moved to the child).
        expect_eq!(1u32, hidden_parent_cow.debug_get_populated_slots_count());
        expect_eq!(2u32, child2_cow.debug_get_populated_slots_count());
    }

    /// Test that removing parent content markers in hidden VMOs decrements populated slots.
    #[test]
    fn hidden_no_parent_content() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        // We will create a bidirectional clone chain with a 1) hidden root, 2) a hidden child of
        // that root, and 3) a visible child of that child. When we create VMO #3, it will get a
        // hidden parent whose parent content markers must be deleted from its page list. Ensure
        // that it also decrements its populated slots count.

        let vmo1: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));

        assert_ok!(vmo1.commit_range(0, 2 * PAGE_SIZE));

        let vmo2 = unwrap_ok!(vmo1.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            /*offset=*/ 0,
            /*size=*/ 3 * PAGE_SIZE,
            /*copy_name=*/ false
        ));
        let vmo2 = VmObject::downcast_paged(vmo2).expect("clone is a paged VMO");

        assert_ok!(vmo2.commit_range(0, PAGE_SIZE));

        let vmo3 = unwrap_ok!(vmo2.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            /*offset=*/ 0,
            /*size=*/ 3 * PAGE_SIZE,
            /*copy_name=*/ false
        ));
        let vmo3 = VmObject::downcast_paged(vmo3).expect("clone is a paged VMO");

        let parent = vmo3
            .debug_get_cow_pages()
            .expect("paged VMO has backing cow pages")
            .debug_get_parent()
            .expect("bidirectional clone has a hidden parent");
        expect_eq!(1u32, parent.debug_get_populated_slots_count());
    }

    /// Test that the populated slots count is decremented when pages are evicted from VmCowPages.
    #[test]
    fn reclaim_page() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let (vmo, [committed_page]) = unwrap_ok!(make_partially_committed_pager_vmo(
            3, /*trap_dirty=*/ false, /*resizable=*/ false,
            /*ignore_requests=*/ false
        ));
        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());

        // Only pages in an isolate queue can be reclaimed, so move the page there as the scanner
        // would.
        // SAFETY: `committed_page` is attached to `vmo`.
        unsafe {
            pmm::page_queues().move_to_reclaim_dont_need(committed_page);
        }

        // SAFETY: `committed_page` is the page committed at offset 0 of `vmo` and has just been
        // moved to a reclamation queue, which is the state the scanner reclaims pages from.
        let result =
            unsafe { cow_pages.reclaim_page(committed_page, 0, EvictionAction::FollowHint, None) };
        assert_true!(result.is_ok());
        expect_eq!(1u64, result.as_ref().unwrap().num_pages);

        expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that evicting a loaned page decrements the populated slots count.
    #[test]
    fn evict_loaned_page() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let _enable_loaning = ScopedLoaningEnabled::new(true);

        // Provide a place for replace_page_with_loaned to borrow from.
        let contiguous_vmo = unwrap_ok!(VmObjectPaged::create_contiguous(
            ALLOC_FLAG_ANY,
            PAGE_SIZE,
            /*alignment_log2=*/ 0
        ));
        assert_ok!(contiguous_vmo.decommit_range(0, PAGE_SIZE));

        let (vmo, [committed_page]) = unwrap_ok!(make_partially_committed_pager_vmo(
            3, /*trap_dirty=*/ false, /*resizable=*/ false,
            /*ignore_requests=*/ false
        ));

        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        assert_ok!(cow_pages.replace_page_with_loaned(committed_page, /*offset=*/ 0));

        let loaned_page = vmo.debug_get_page(0).expect("vmo has a page at offset 0");
        // SAFETY: `loaned_page` is attached to `vmo`.
        assert_true!(unsafe { loaned_page.is_loaned() });

        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());

        // SAFETY: `loaned_page` is the object-associated page at offset 0 of `vmo`.
        assert_ok!(unsafe { cow_pages.evict_loaned_page(loaned_page, 0) });

        expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that compression clearing a zero page slot decrements the populated slots count.
    #[test]
    fn zero_page_compression() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));

        assert_ok!(vmo.commit_range(0, PAGE_SIZE));

        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());

        let page = vmo.debug_get_page(0).expect("vmo has a page at offset 0");

        // Returning early here would discard any failure recorded above, so only skip the part of
        // the test that needs a compressor.
        if let Some(compression) = pmm::node().get_page_compression() {
            stack_pin_init!(let compressor = CompressorGuard::new(compression));
            assert_ok!(compressor.as_mut().get().arm());

            // SAFETY: `page` is the page committed at offset 0 of `vmo`.
            let result = unsafe {
                cow_pages.reclaim_page(
                    page,
                    0,
                    EvictionAction::FollowHint,
                    Some(compressor.as_mut().get()),
                )
            };
            assert_true!(result.is_ok());

            expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());
        }
    }

    /// Test that zero page deduplication decrements the populated slots count.
    #[test]
    fn zero_page_deduplication() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));

        assert_ok!(vmo.commit_range(0, PAGE_SIZE));

        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());

        let page = vmo.debug_get_page(0).expect("vmo has a page at offset 0");

        assert_true!(cow_pages.dedup_zero_page(page, 0));

        expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that content removed from a hidden parent updates the populated slots count.
    #[test]
    fn release_hidden() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 2 * PAGE_SIZE));

        assert_ok!(vmo.commit_range(0, 2 * PAGE_SIZE));

        let child = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            /*offset=*/ 0,
            /*size=*/ 2 * PAGE_SIZE,
            /*copy_name=*/ false
        ));

        // Commit the pages in the child to remove its share count of the content.
        assert_ok!(child.commit_range(0, 2 * PAGE_SIZE));

        let vmo_cow = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        let parent_cow =
            vmo_cow.debug_get_parent().expect("bidirectional clone has a hidden parent");
        expect_eq!(2u32, parent_cow.debug_get_populated_slots_count());

        // Remove the remaining share count for the first page, which will trigger the hidden parent
        // to remove the content from its local page list. Zeroing pages calls
        // DecrementCowContentShareCount, which is what we're interested in tracking.
        let (status, zeroed_bytes) = zero_first_page(&vmo_cow);
        expect_eq!(PAGE_SIZE, zeroed_bytes);
        expect_ok!(status);

        expect_eq!(1u32, parent_cow.debug_get_populated_slots_count());
    }

    /// Test that DecommitRange decrements the populated slots count for the pages it removes.
    #[test]
    fn decommit_range() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 2 * PAGE_SIZE));
        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());

        assert_ok!(vmo.commit_range(0, 2 * PAGE_SIZE));

        expect_eq!(2u32, cow_pages.debug_get_populated_slots_count());

        assert_ok!(vmo.decommit_range(0, PAGE_SIZE));

        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that DetachSource decrements the populated slots count for removed clean content.
    #[test]
    fn detach_source() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let (vmo, [_page0, _page1]) = unwrap_ok!(make_partially_committed_pager_vmo(
            3, /*trap_dirty=*/ false, /*resizable=*/ false,
            /*ignore_requests=*/ false
        ));
        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        expect_eq!(2u32, cow_pages.debug_get_populated_slots_count());

        vmo.detach_source();

        expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that upgrading a VMO to high priority removes loaned pages from the slots count.
    #[test]
    fn remove_loaned_high_priority() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let _enable_loaning = ScopedLoaningEnabled::new(true);

        // Provide a place for replace_page_with_loaned to borrow from.
        let contiguous_vmo = unwrap_ok!(VmObjectPaged::create_contiguous(
            ALLOC_FLAG_ANY,
            PAGE_SIZE,
            /*alignment_log2=*/ 0
        ));
        assert_ok!(contiguous_vmo.decommit_range(0, PAGE_SIZE));

        let (vmo, [before_page]) = unwrap_ok!(make_committed_pager_vmo(
            /*trap_dirty=*/ false, /*resizable=*/ false
        ));

        let cow_pages = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        assert_ok!(cow_pages.replace_page_with_loaned(before_page, /*offset=*/ 0));

        expect_eq!(1u32, cow_pages.debug_get_populated_slots_count());

        const ADD_HIGH_PRIORITY: NonZeroI64 = NonZeroI64::new(1).unwrap();
        const REMOVE_HIGH_PRIORITY: NonZeroI64 = NonZeroI64::new(-1).unwrap();

        change_vmo_high_priority_count(&vmo, ADD_HIGH_PRIORITY);

        expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());

        change_vmo_high_priority_count(&vmo, REMOVE_HIGH_PRIORITY);

        expect_eq!(0u32, cow_pages.debug_get_populated_slots_count());
    }

    /// Test that failing to add a sequence of pages updates the populated slots count on cleanup.
    #[test]
    fn add_pages() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 4 * PAGE_SIZE));
        let vmo_cow: RefPtr<VmCowPages> = vmo.debug_get_cow_pages().unwrap();

        expect_eq!(0u32, vmo_cow.debug_get_populated_slots_count());

        assert_ok!(vmo.commit_range(PAGE_SIZE, PAGE_SIZE));

        expect_eq!(1u32, vmo_cow.debug_get_populated_slots_count());

        {
            // The list has to stay borrowed by `add_new_pages_locked` below, so it cannot be moved
            // into a `zr::defer` closure; use an explicit guard instead.
            struct CleanupList<'a> {
                list: Pin<&'a mut VmPageDoublyLinkedList>,
            }

            impl Drop for CleanupList<'_> {
                fn drop(&mut self) {
                    // SAFETY: `add_new_pages_locked` takes ownership of the pages it consumed
                    // regardless of its return value, so whatever remains on the list is still a
                    // set of valid allocated PMM pages that have not been freed.
                    unsafe {
                        pmm::free_list(self.list.as_mut());
                    }
                }
            }

            stack_pin_init!(let deferred = DeferredOps::new(&vmo_cow));
            lock!(let guard = vmo_cow.lock());

            stack_pin_init!(let list = VmPageDoublyLinkedList::new());
            let count: usize = 3;
            assert_ok!(pmm::alloc_pages(count, 0, list.as_mut()));
            let mut cleanup = CleanupList { list };

            expect_err!(
                vmo_cow.add_new_pages_locked(
                    guard.token(),
                    0,
                    cleanup.list.as_mut(),
                    CanOverwriteSlot::Empty,
                    /*zero=*/ true,
                    deferred.as_mut(),
                ),
                Status::ALREADY_EXISTS
            );
        }

        expect_eq!(1u32, vmo_cow.debug_get_populated_slots_count());
    }

    /// Test that a spurious parent content marker transferred to a child can be decommitted.
    #[test]
    fn merge_spurious_parent_content() {
        // Regression test for https://fxbug.dev/483815044.
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let child: RefPtr<VmObjectPaged>;
        // Make |child|'s only slot hold a parent content marker.
        {
            let vmo: RefPtr<VmObjectPaged> =
                unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 2 * PAGE_SIZE));

            // Ensure we get a bidirectional clone.
            assert_ok!(vmo.commit_range(0, 2 * PAGE_SIZE));

            let clone = unwrap_ok!(vmo.create_clone(
                Resizability::NonResizable,
                SnapshotType::Full,
                0,
                PAGE_SIZE,
                /*copy_name=*/ false
            ));
            child = VmObject::downcast_paged(clone).expect("clone is a paged VMO");

            let hidden_parent = vmo
                .debug_get_cow_pages()
                .expect("paged VMO has backing cow pages")
                .debug_get_parent()
                .expect("bidirectional clone has a hidden parent");

            let page =
                hidden_parent.debug_get_page(0).expect("hidden parent has a page at offset 0");

            // Everything below depends on compressing the hidden parent's page. Returning here is
            // safe only because nothing above records a failure and continues: the checks so far
            // are all short-circuiting, so there is no accumulated result for `return true` to
            // discard.
            let Some(compression) = pmm::node().get_page_compression() else {
                kprintln!("Skipping {:s}:{:u}; no page compression configured.", file!(), line!());
                return true;
            };

            stack_pin_init!(let compressor = CompressorGuard::new(compression));
            assert_ok!(compressor.as_mut().get().arm());

            // SAFETY: `page` is the page at offset 0 of `hidden_parent`.
            let result = unsafe {
                hidden_parent.reclaim_page(
                    page,
                    0,
                    EvictionAction::IgnoreHint,
                    Some(compressor.as_mut().get()),
                )
            };
            assert_true!(result.is_ok());
            expect_eq!(1u64, result.as_ref().unwrap().num_pages);

            // The content was removed (we actually compressed the zero page).
            expect_true!(hidden_parent.debug_is_empty(0));

            let child_cow = child.debug_get_cow_pages().expect("paged VMO has backing cow pages");

            // There is now a spurious parent content marker.
            expect_true!(child_cow.debug_is_parent_content(0));

            // TODO(https://fxbug.dev/504652289): The continuous attribution system and the
            // populated slots count should eventually agree here that there are zero populated
            // slots and zero populated bytes.
            expect_eq!(1u32, child_cow.debug_get_populated_slots_count());
            expect_eq!(0u64, attribution::total_bytes(&child.get_attributed_memory()));

            // Let's drop |vmo| to trigger the hidden parent to merge into |child|. That will allow
            // |child| to have no parent while still having a spurious parent content marker.
        }

        let child_cow = child.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        // There is 1 parent content marker.
        expect_true!(child_cow.debug_is_parent_content(0));
        expect_eq!(1u32, child_cow.debug_get_populated_slots_count());

        expect_ok!(child.decommit_range(0, PAGE_SIZE));

        expect_true!(child_cow.debug_is_empty(0));
        expect_eq!(0u32, child_cow.debug_get_populated_slots_count());
    }

    /// Test that a VMO's dead transition redistributes content between parents and children.
    #[test]
    fn merge_into_child() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        // Construct a copy-on-write hierarchy, and selectively destroy one leaf.

        let _b: RefPtr<VmObjectPaged>;
        let _c: RefPtr<VmObjectPaged>;
        // Held past |a|'s death so that |a|'s VmCowPages outlives it, as in the C++ test.
        let _a_cow: RefPtr<VmCowPages>;
        let b_c_hidden_parent: RefPtr<VmCowPages>;
        let a_hidden_parent: RefPtr<VmCowPages>;
        {
            let a: RefPtr<VmObjectPaged> =
                unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));

            assert_ok!(a.commit_range(0, PAGE_SIZE));

            let b = unwrap_ok!(a.create_clone(
                Resizability::NonResizable,
                SnapshotType::Full,
                /*offset=*/ 0,
                /*size=*/ 3 * PAGE_SIZE,
                /*copy_name=*/ false
            ));
            let b = VmObject::downcast_paged(b).expect("clone is a paged VMO");

            // Ensure that we get an extra level in the hierarchy.
            assert_ok!(b.commit_range(PAGE_SIZE, PAGE_SIZE));

            let c = unwrap_ok!(b.create_clone(
                Resizability::NonResizable,
                SnapshotType::Full,
                /*offset=*/ 0,
                /*size=*/ 3 * PAGE_SIZE,
                /*copy_name=*/ false
            ));
            let c = VmObject::downcast_paged(c).expect("clone is a paged VMO");

            // b and c have the same parent.
            let b_cow = b.debug_get_cow_pages().expect("paged VMO has backing cow pages");
            let c_cow = c.debug_get_cow_pages().expect("paged VMO has backing cow pages");
            let b_parent = b_cow.debug_get_parent().expect("clone has a hidden parent");
            let c_parent = c_cow.debug_get_parent().expect("clone has a hidden parent");
            expect_eq!(b_parent.as_raw(), c_parent.as_raw());
            b_c_hidden_parent = b_parent;

            // b and c's parent's parent is the same as a's parent.
            let a_cow = a.debug_get_cow_pages().expect("paged VMO has backing cow pages");
            let b_c_grandparent =
                b_c_hidden_parent.debug_get_parent().expect("clone has a hidden parent");
            let a_parent = a_cow.debug_get_parent().expect("clone has a hidden parent");
            expect_eq!(b_c_grandparent.as_raw(), a_parent.as_raw());
            a_hidden_parent = a_parent;

            // Watch |b_c_hidden_parent|'s and |a_hidden_parent|'s populated slots as |a| dies.
            expect_eq!(1u32, b_c_hidden_parent.debug_get_populated_slots_count());
            expect_eq!(1u32, a_hidden_parent.debug_get_populated_slots_count());

            _a_cow = a_cow;
            _b = b;
            _c = c;
        }
        expect_eq!(2u32, b_c_hidden_parent.debug_get_populated_slots_count());
        expect_eq!(0u32, a_hidden_parent.debug_get_populated_slots_count());
    }

    /// Test that ReleaseOwnedPagesRangeLocked updates the populated slot count locally.
    #[test]
    fn release_owned_self() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> = unwrap_ok!(VmObjectPaged::create(
            ALLOC_FLAG_ANY,
            VmObjectPaged::RESIZABLE,
            10 * PAGE_SIZE
        ));
        let vmo_cow = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        assert_ok!(vmo.commit_range(0, 10 * PAGE_SIZE));

        expect_eq!(10u32, vmo_cow.debug_get_populated_slots_count());

        // Call ReleaseOwnedPagesRangeLocked indirectly through Resize.
        assert_ok!(vmo.resize(4 * PAGE_SIZE));

        expect_eq!(4u32, vmo_cow.debug_get_populated_slots_count());
    }

    /// Test that ReleaseOwnedPagesRangeLocked updates the slot count in hidden parents.
    #[test]
    fn release_owned_parent() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let a: RefPtr<VmObjectPaged> = unwrap_ok!(VmObjectPaged::create(
            ALLOC_FLAG_ANY,
            VmObjectPaged::RESIZABLE,
            2 * PAGE_SIZE
        ));

        assert_ok!(a.commit_range(0, 2 * PAGE_SIZE));

        let b = unwrap_ok!(a.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            /*offset=*/ 0,
            /*size=*/ 2 * PAGE_SIZE,
            /*copy_name=*/ false
        ));
        let b = VmObject::downcast_paged(b).expect("clone is a paged VMO");

        // Decrement the share count of the parent content.
        assert_ok!(b.commit_range(PAGE_SIZE, PAGE_SIZE));

        let a_cow = a.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        let parent_cow = a_cow.debug_get_parent().expect("clone has a hidden parent");

        expect_eq!(2u32, a_cow.debug_get_populated_slots_count());
        expect_eq!(2u32, parent_cow.debug_get_populated_slots_count());

        // Call ReleaseOwnedPagesRangeLocked indirectly through Resize.
        assert_ok!(a.resize(PAGE_SIZE));

        expect_eq!(1u32, a_cow.debug_get_populated_slots_count());
        expect_eq!(1u32, parent_cow.debug_get_populated_slots_count());
    }

    /// Test that TakePages decrements the populated slots count for the content it removes.
    #[test]
    fn take_pages() {
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));
        let vmo_cow = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        assert_ok!(vmo.commit_range(0, 3 * PAGE_SIZE));

        expect_eq!(3u32, vmo_cow.debug_get_populated_slots_count());

        stack_pin_init!(let page_list = VmPageSpliceList::new());
        assert_ok!(vmo.take_pages(0, 3 * PAGE_SIZE, page_list.as_mut()));

        expect_eq!(0u32, vmo_cow.debug_get_populated_slots_count());
    }

    /// Test that TakePages decrements the populated slots count when there is a parent.
    #[test]
    fn take_pages_parent() {
        if should_skip_no_feature!() {
            return true;
        }

        // This is a separate test from take_pages because it triggers an independent branch in
        // VmCowPages::TakePages.

        let _disable_scanner = AutoVmScannerDisable::new();

        let a: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, 3 * PAGE_SIZE));

        assert_ok!(a.commit_range(0, 3 * PAGE_SIZE));

        let _b = unwrap_ok!(a.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            /*offset=*/ 0,
            /*size=*/ 3 * PAGE_SIZE,
            /*copy_name=*/ false
        ));

        assert_ok!(a.commit_range(0, 3 * PAGE_SIZE));

        let a_cow = a.debug_get_cow_pages().expect("paged VMO has backing cow pages");

        expect_eq!(3u32, a_cow.debug_get_populated_slots_count());

        stack_pin_init!(let page_list = VmPageSpliceList::new());
        assert_ok!(a.take_pages(0, 3 * PAGE_SIZE, page_list.as_mut()));

        expect_eq!(0u32, a_cow.debug_get_populated_slots_count());
    }

    /// Test the attribution disconnect from deduplicating a zero page in a hidden parent.
    #[test]
    fn dedup_hidden_parent_disconnect() {
        // Deduplicating a zero page in a hidden parent creates a persistent disconnect between the
        // child's attribution tracker and GetAttributedMemory.
        //
        // TODO(https://fxbug.dev/504652289): Update this test when the disconnect is repaired.
        if should_skip_no_feature!() {
            return true;
        }

        let _disable_scanner = AutoVmScannerDisable::new();

        let vmo: RefPtr<VmObjectPaged> =
            unwrap_ok!(VmObjectPaged::create(ALLOC_FLAG_ANY, 0, PAGE_SIZE));

        // Ensure there is a resident (zero) page.
        assert_ok!(vmo.commit_range(0, PAGE_SIZE));

        let _clone = unwrap_ok!(vmo.create_clone(
            Resizability::NonResizable,
            SnapshotType::Full,
            0,
            PAGE_SIZE,
            false
        ));

        let vmo_cow = vmo.debug_get_cow_pages().expect("paged VMO has backing cow pages");
        let hidden_parent =
            vmo_cow.debug_get_parent().expect("bidirectional clone has a hidden parent");

        expect_eq!(1u32, hidden_parent.debug_get_populated_slots_count());
        expect_eq!(PAGE_SIZE, attribution::total_bytes(&vmo.get_attributed_memory()));

        let page = hidden_parent.debug_get_page(0).expect("hidden parent has a page at offset 0");
        assert_true!(hidden_parent.dedup_zero_page(page, 0));

        // Note the disconnect between the continuously tracked populated slots count and
        // GetAttributedMemory.
        expect_eq!(1u32, vmo_cow.debug_get_populated_slots_count());
        expect_eq!(0u64, attribution::total_bytes(&vmo.get_attributed_memory()));
    }
}
