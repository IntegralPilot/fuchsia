// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// VmPageList tests duplicated from vmpl_unittest.cc.
#[cfg(ktest)]
#[unittest::suite]
mod vmpl_rs {
    use crate::vm::page::VmPagePtr;
    use crate::vm::pmm;
    use crate::vm::vm_page_list::{
        BatchInserter, IntervalHandling, ReferenceValue, VmPageList, VmPageListNode,
        VmPageOrMarker, ZeroRangeDirtyState,
    };
    use page::SIZE as PAGE_SIZE_USIZE;
    use unittest::{expect_eq, expect_false, expect_gt, expect_ok, expect_true, unwrap_ok};
    use zx_status::Status;

    const PAGE_SIZE: u64 = PAGE_SIZE_USIZE as u64;

    fn get_pages<const N: usize>() -> [VmPagePtr; N] {
        core::array::from_fn(|_| pmm::alloc_page(0).expect("pmm_alloc_page failed").0)
    }

    fn free_pages<const N: usize>(pages: [VmPagePtr; N]) {
        for page in pages {
            // SAFETY: `page` was allocated by `pmm::alloc_page` in `get_pages`.
            unsafe { pmm::free_page(page) };
        }
    }

    fn add_page(pl: &mut VmPageList, page: VmPagePtr, offset: u64) -> bool {
        let (slot, is_interval) = pl.lookup_or_allocate(offset, IntervalHandling::SplitInterval);
        let Some(slot) = slot else {
            return false;
        };
        if !slot.is_empty() && !slot.is_interval_slot() {
            return false;
        }
        assert!(slot.is_empty() || is_interval);
        *slot = VmPageOrMarker::from_page(page);
        true
    }

    fn add_marker(pl: &mut VmPageList, offset: u64) -> bool {
        let (slot, is_interval) = pl.lookup_or_allocate(offset, IntervalHandling::SplitInterval);
        let Some(slot) = slot else {
            return false;
        };
        if !slot.is_empty() && !slot.is_interval_slot() {
            return false;
        }
        assert!(slot.is_empty() || is_interval);
        *slot = VmPageOrMarker::marker();
        true
    }

    fn add_reference(pl: &mut VmPageList, ref_val: ReferenceValue, offset: u64) -> bool {
        let (slot, is_interval) = pl.lookup_or_allocate(offset, IntervalHandling::SplitInterval);
        let Some(slot) = slot else {
            return false;
        };
        if !slot.is_empty() && !slot.is_interval_slot() {
            return false;
        }
        assert!(slot.is_empty() || is_interval);
        *slot = VmPageOrMarker::from_reference(ref_val);
        true
    }

    const fn test_reference(v: u32) -> u32 {
        v << ReferenceValue::ALIGN_BITS
    }

    /// Basic test that checks adding and removing a page.
    #[test]
    fn vmpl_add_remove_page_test() {
        let mut pl = VmPageList::new();

        let (test_page, _paddr) = unwrap_ok!(pmm::alloc_page(0), "pmm alloc page failed");

        expect_true!(add_page(&mut pl, test_page, 0));

        expect_true!(pl.lookup(0).is_some_and(|p| p.page() == test_page));
        expect_false!(pl.is_empty());
        expect_false!(pl.has_no_page_or_ref());

        let mut removed = pl.remove_content(0);
        expect_true!(removed.is_page());
        let remove_page = removed.release_page();
        expect_true!(test_page == remove_page);
        expect_true!(pl.remove_content(0).is_empty());

        expect_true!(pl.is_empty());
        expect_true!(pl.has_no_page_or_ref());

        // SAFETY: `test_page` was allocated above and removed from `pl`.
        unsafe { pmm::free_page(test_page) };
    }

    /// Basic test of setting and getting markers.
    #[test]
    fn vmpl_basic_marker_test() {
        let mut pl = VmPageList::new();

        expect_true!(pl.is_empty());
        expect_true!(pl.has_no_page_or_ref());

        expect_true!(add_marker(&mut pl, 0));

        expect_true!(pl.lookup(0).expect("lookup marker").is_marker());

        expect_false!(pl.is_empty());
        expect_true!(pl.has_no_page_or_ref());

        let removed = pl.remove_content(0);
        expect_true!(removed.is_marker());

        expect_true!(pl.has_no_page_or_ref());
        expect_true!(pl.is_empty());
    }

    /// Basic test of setting and getting references.
    #[test]
    fn vmpl_basic_reference_test() {
        let mut pl = VmPageList::new();

        expect_true!(pl.is_empty());
        expect_true!(pl.has_no_page_or_ref());

        // The zero ref is valid.
        let ref0 = ReferenceValue::new(0);
        expect_true!(add_reference(&mut pl, ref0, 0));

        expect_false!(pl.is_empty());
        expect_false!(pl.has_no_page_or_ref());

        // A non-zero ref.
        let ref1 = ReferenceValue::new(test_reference(1));
        expect_true!(add_reference(&mut pl, ref1, PAGE_SIZE));

        let mut removed0 = pl.remove_content(0);
        expect_true!(removed0.is_reference());
        expect_eq!(removed0.release_reference().value(), ref0.value());

        expect_false!(pl.is_empty());
        expect_false!(pl.has_no_page_ref_or_marker());

        let mut removed1 = pl.remove_content(PAGE_SIZE);
        expect_true!(removed1.is_reference());
        expect_eq!(removed1.release_reference().value(), ref1.value());

        expect_true!(pl.is_empty());
        expect_true!(pl.has_no_page_ref_or_marker());
    }

    /// Test for freeing a range of pages.
    #[test]
    fn vmpl_free_pages_test() {
        let mut pl = VmPageList::new();
        const COUNT: usize = 3 * VmPageListNode::PAGE_FAN_OUT;

        let test_pages = get_pages::<COUNT>();

        // Install alternating pages and markers.
        for (i, &page) in test_pages.iter().enumerate() {
            expect_true!(add_page(&mut pl, page, (i as u64) * 2 * PAGE_SIZE));
            expect_true!(add_marker(&mut pl, ((i as u64) * 2 + 1) * PAGE_SIZE));
        }

        let mut list = fbl::Vector::<VmPagePtr>::new();
        let res =
            pl.remove_pages(PAGE_SIZE * 2, ((COUNT as u64) - 1) * 2 * PAGE_SIZE, |slot, _off| {
                if slot.is_page() {
                    let p = slot.release_page();
                    list.push_back(p).expect("vector push");
                }
                *slot = VmPageOrMarker::empty();
                Status::NEXT
            });
        expect_ok!(res);

        for &page in &test_pages[1..COUNT - 1] {
            expect_true!(list.contains(&page), "Not in free list");
        }

        for (i, &page) in test_pages.iter().enumerate() {
            let mut remove_page = pl.remove_content((i as u64) * 2 * PAGE_SIZE);
            let remove_marker = pl.remove_content(((i as u64) * 2 + 1) * PAGE_SIZE);
            if i == 0 || i == COUNT - 1 {
                expect_true!(remove_page.is_page());
                expect_true!(remove_marker.is_marker());
                expect_true!(page == remove_page.release_page());
            } else {
                expect_true!(remove_page.is_empty());
                expect_true!(remove_marker.is_empty());
            }
        }

        free_pages(test_pages);
    }

    /// Tests freeing the last page in a list.
    #[test]
    fn vmpl_free_pages_last_page_test() {
        let (page, _paddr) = unwrap_ok!(pmm::alloc_page(0), "pmm alloc page");

        let mut pl = VmPageList::new();
        expect_true!(add_page(&mut pl, page, 0));

        expect_true!(pl.lookup(0).is_some_and(|p| p.page() == page));

        let mut list = fbl::Vector::<VmPagePtr>::new();
        pl.remove_all_content(|mut p| {
            if p.is_page() {
                list.push_back(p.release_page()).expect("vector push");
            }
        });
        expect_true!(pl.is_empty());

        expect_eq!(list.len(), 1);
        expect_true!(list[0] == page);

        // SAFETY: `page` was allocated above and removed from `pl`.
        unsafe { pmm::free_page(page) };
    }

    /// Tests allocation and freeing near the u64::MAX boundary.
    #[test]
    fn vmpl_near_last_offset_free() {
        let (page, _paddr) = unwrap_ok!(pmm::alloc_page(0), "pmm alloc page");

        let mut at_least_one = false;
        let mut addr = 0xffff_ffff_fff0_0000u64;
        while addr != 0 {
            let mut pl = VmPageList::new();
            if add_page(&mut pl, page, addr) {
                at_least_one = true;
                expect_true!(pl.lookup(addr).is_some_and(|p| p.page() == page));

                let mut list = fbl::Vector::<VmPagePtr>::new();
                pl.remove_all_content(|mut p| {
                    if p.is_page() {
                        list.push_back(p.release_page()).expect("vector push");
                    }
                });

                expect_eq!(list.len(), 1);
                expect_true!(list[0] == page);
                expect_true!(pl.is_empty());
            }
            addr = addr.wrapping_add(PAGE_SIZE);
        }
        expect_true!(at_least_one);

        let mut pl2 = VmPageList::new();
        expect_true!(
            pl2.lookup_or_allocate(0xffff_ffff_ffff_0000, IntervalHandling::NoIntervals)
                .0
                .is_none()
        );

        // SAFETY: `page` was allocated above and removed from all lists.
        unsafe { pmm::free_page(page) };
    }

    /// Tests for_every_page and for_every_page_in_range traversals.
    #[test]
    fn vmpl_for_every_page_test() {
        let mut list = VmPageList::new();

        const COUNT: usize = 5;
        let test_pages = get_pages::<COUNT>();

        let offsets: [u64; COUNT] = [
            0,
            PAGE_SIZE,
            (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE - PAGE_SIZE,
            (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE,
            (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE + PAGE_SIZE,
        ];

        for (i, &page) in test_pages.iter().enumerate() {
            if i % 2 != 0 {
                expect_true!(add_page(&mut list, page, offsets[i]));
            } else {
                expect_true!(add_marker(&mut list, offsets[i]));
            }
        }

        let mut matched = true;
        let mut idx = 0;
        let res = list.for_every_page(|p, off| {
            if off != offsets[idx] {
                matched = false;
            }
            if idx % 2 != 0 {
                if !p.is_page() || p.page() != test_pages[idx] {
                    matched = false;
                }
            } else if !p.is_marker() {
                matched = false;
            }
            idx += 1;
            Status::NEXT
        });
        expect_ok!(res);
        expect_true!(matched);
        expect_eq!(idx, offsets.len());

        let mut matched_range = true;
        idx = 1;
        let res = list.for_every_page_in_range(offsets[1], offsets[COUNT - 1], |p, off| {
            if off != offsets[idx] {
                matched_range = false;
            }
            if idx % 2 != 0 {
                if !p.is_page() || p.page() != test_pages[idx] {
                    matched_range = false;
                }
            } else if !p.is_marker() {
                matched_range = false;
            }
            idx += 1;
            Status::NEXT
        });
        expect_ok!(res);
        expect_true!(matched_range);
        expect_eq!(idx, offsets.len() - 1);

        list.remove_all_content(|mut p| {
            if p.is_page() {
                let _ = p.release_page();
            }
        });

        free_pages(test_pages);
    }

    fn vmpl_page_gap_iter_test_body(pages: &[Option<VmPagePtr>], count: usize, stop_idx: usize) {
        let mut list = VmPageList::new();
        for (i, page_opt) in pages.iter().enumerate().take(count) {
            if let Some(page) = page_opt {
                assert!(add_page(&mut list, *page, (i as u64) * PAGE_SIZE));
            }
        }

        let idx = core::cell::Cell::new(0usize);
        let s = list.for_every_page_and_gap_in_range(
            0,
            (count as u64) * PAGE_SIZE,
            |p, off| {
                let cur = idx.get();
                if off != (cur as u64) * PAGE_SIZE || !p.is_page() || pages[cur] != Some(p.page()) {
                    return Status::BAD_STATE;
                }
                if cur == stop_idx {
                    return Status::STOP;
                }
                idx.set(cur + 1);
                Status::NEXT
            },
            |gap_start, gap_end| {
                let mut o = gap_start;
                while o < gap_end {
                    let cur = idx.get();
                    if o != (cur as u64) * PAGE_SIZE || pages[cur].is_some() {
                        return Status::BAD_STATE;
                    }
                    if cur == stop_idx {
                        return Status::STOP;
                    }
                    idx.set(cur + 1);
                    o += PAGE_SIZE;
                }
                Status::NEXT
            },
        );
        assert_eq!(s, Ok(()));
        assert_eq!(stop_idx, idx.get());

        let mut free_list = fbl::Vector::<VmPagePtr>::new();
        list.remove_all_content(|mut p| {
            if p.is_page() {
                free_list.push_back(p.release_page()).expect("vector push");
            }
        });
        assert!(list.is_empty());
    }

    /// Tests `for_every_page_and_gap_in_range` against all lists of size 4.
    #[test]
    fn vmpl_page_gap_iter_test() {
        const COUNT: usize = 4;
        let pages = get_pages::<COUNT>();

        let mut page_list: [Option<VmPagePtr>; COUNT] = [None; COUNT];
        for i in 0..COUNT {
            for j in 0..(1 << COUNT) {
                for k in 0..COUNT {
                    if (j & (1 << k)) != 0 {
                        page_list[k] = Some(pages[k]);
                    } else {
                        page_list[k] = None;
                    }
                }
                vmpl_page_gap_iter_test_body(&page_list, COUNT, i);
            }
        }

        free_pages(pages);
    }

    /// Tests that stopping early in `for_every_page_and_gap_in_range` skips trailing gaps.
    #[test]
    fn vmpl_skip_last_gap_test() {
        let mut list = VmPageList::new();
        let (test_page, _paddr) = unwrap_ok!(pmm::alloc_page(0), "pmm alloc page");

        expect_true!(add_page(&mut list, test_page, PAGE_SIZE));

        let mut saw_gap_start = 0;
        let mut saw_gap_end = 0;
        let mut gaps_seen = 0;
        let res = list.for_every_page_and_gap_in_range(
            0,
            PAGE_SIZE * 3,
            |_slot, _offset| Status::STOP,
            |gap_start, gap_end| {
                saw_gap_start = gap_start;
                saw_gap_end = gap_end;
                gaps_seen += 1;
                Status::NEXT
            },
        );
        expect_ok!(res);

        // Validate we saw one gap, and it was the correct gap.
        expect_eq!(1, gaps_seen);
        expect_eq!(0, saw_gap_start);
        expect_eq!(PAGE_SIZE, saw_gap_end);

        let mut free_list = fbl::Vector::<VmPagePtr>::new();
        list.remove_all_content(|mut p| {
            if p.is_page() {
                free_list.push_back(p.release_page()).expect("vector push");
            }
        });
        expect_true!(list.is_empty());
        // SAFETY: `test_page` was removed from `list`.
        unsafe { pmm::free_page(test_page) };
    }

    /// Tests BatchInserter sequential and out-of-order allocations.
    #[test]
    fn vmpl_batch_inserter_test() {
        let mut pl = VmPageList::new();
        {
            let mut inserter = BatchInserter::new(&mut pl);
            // Sequentially insert pages across 3 nodes (48 pages)
            for i in 0..(VmPageListNode::PAGE_FAN_OUT * 3) {
                let offset = (i as u64) * PAGE_SIZE;
                let slot = inserter.lookup_or_allocate(offset).unwrap();
                expect_true!(slot.is_empty());
                *slot = VmPageOrMarker::marker();
            }
        }

        // Verify all pages are populated
        for i in 0..(VmPageListNode::PAGE_FAN_OUT * 3) {
            let offset = (i as u64) * PAGE_SIZE;
            let slot = pl.lookup(offset).unwrap();
            expect_true!(slot.is_marker());
        }

        pl.remove_all_content(|_| {});

        // Test non-sequential insertions and reset
        {
            let mut inserter = BatchInserter::new(&mut pl);
            let high_offset = VmPageListNode::NODE_SPAN_BYTES * 10;
            let slot = inserter.lookup_or_allocate(high_offset).unwrap();
            expect_true!(slot.is_empty());
            *slot = VmPageOrMarker::marker();

            let slot_low = inserter.lookup_or_allocate(PAGE_SIZE).unwrap();
            expect_true!(slot_low.is_empty());
            *slot_low = VmPageOrMarker::marker();

            inserter.reset();
        }

        pl.remove_all_content(|_| {});
    }

    /// Tests any_pages_or_intervals_in_range and any_owned_pages_or_intervals_in_range.
    #[test]
    fn vmpl_any_pages_in_range_test() {
        let mut pl = VmPageList::new();

        expect_false!(pl.any_pages_or_intervals_in_range(0, PAGE_SIZE));
        *pl.lookup_or_allocate(0, IntervalHandling::NoIntervals).0.unwrap() =
            VmPageOrMarker::marker();
        expect_true!(pl.any_pages_or_intervals_in_range(0, PAGE_SIZE));
        expect_true!(pl.any_owned_pages_or_intervals_in_range(0, PAGE_SIZE));

        pl.remove_all_content(|_| {});
    }

    /// Tests adding a single page zero interval.
    #[test]
    fn vmpl_add_zero_interval_single_page_test() {
        let mut pl = VmPageList::new();

        expect_false!(pl.is_offset_in_zero_interval(0));
        let res = pl.add_zero_interval(0, PAGE_SIZE, ZeroRangeDirtyState::Dirty);
        expect_ok!(res);

        expect_true!(pl.is_offset_in_zero_interval(0));
        expect_false!(pl.is_offset_in_zero_interval(PAGE_SIZE));

        let slot = pl.lookup(0).unwrap();
        expect_true!(slot.is_interval_slot());
        expect_true!(slot.is_interval_zero());
        expect_true!(slot.zero_interval_dirty_state() == ZeroRangeDirtyState::Dirty);

        pl.remove_all_content(|_| {});
    }

    /// Tests adding adjacent zero intervals and automatic range coalescing.
    #[test]
    fn vmpl_add_zero_interval_coalesce_test() {
        let mut pl = VmPageList::new();
        let span = VmPageListNode::NODE_SPAN_BYTES;

        // Add left interval [0, 64 KiB)
        expect_ok!(pl.add_zero_interval(0, span, ZeroRangeDirtyState::Dirty));
        // Add right interval [128 KiB, 192 KiB)
        expect_ok!(pl.add_zero_interval(span * 2, span * 3, ZeroRangeDirtyState::Dirty));

        expect_true!(pl.lookup(0).unwrap().is_interval_start());
        expect_true!(pl.lookup(span - PAGE_SIZE).unwrap().is_interval_end());
        expect_true!(pl.lookup(span * 2).unwrap().is_interval_start());
        expect_true!(pl.lookup(span * 3 - PAGE_SIZE).unwrap().is_interval_end());

        // Add middle bridging interval [64 KiB, 128 KiB)
        expect_ok!(pl.add_zero_interval(span, span * 2, ZeroRangeDirtyState::Dirty));

        // Now intervals should be coalesced into a single interval [0, 192 KiB)
        expect_true!(pl.lookup(0).unwrap().is_interval_start());
        expect_true!(pl.lookup(span * 3 - PAGE_SIZE).unwrap().is_interval_end());
        // The old boundary sentinels should have been cleaned up
        expect_true!(pl.lookup(span - PAGE_SIZE).is_none_or(|p| p.is_empty()));
        expect_true!(pl.lookup(span).is_none_or(|p| p.is_empty()));
        expect_true!(pl.lookup(span * 2 - PAGE_SIZE).is_none_or(|p| p.is_empty()));
        expect_true!(pl.lookup(span * 2).is_none_or(|p| p.is_empty()));

        // Check query methods across the entire coalesced range
        for off in [0, 4096, span, span + 4096, span * 2, span * 3 - PAGE_SIZE] {
            expect_true!(pl.is_offset_in_zero_interval(off));
        }
        expect_false!(pl.is_offset_in_zero_interval(span * 3));

        pl.remove_all_content(|_| {});
    }

    /// Tests multi-node zero interval creation and boundary sentinels.
    #[test]
    fn vmpl_multi_node_zero_interval_test() {
        let mut pl = VmPageList::new();
        let span = VmPageListNode::NODE_SPAN_BYTES;

        // Multi-node interval spanning [0, 128 KiB) (2 nodes)
        expect_ok!(pl.add_zero_interval(0, span * 2, ZeroRangeDirtyState::Dirty));
        let start_off = 0;
        let end_off = span * 2 - PAGE_SIZE;

        let start_slot = pl.lookup(start_off).unwrap();
        expect_true!(start_slot.is_interval_start());
        expect_true!(start_slot.is_interval_zero());

        let end_slot = pl.lookup(end_off).unwrap();
        expect_true!(end_slot.is_interval_end());
        expect_true!(end_slot.is_interval_zero());

        // Offsets within the range are in the interval
        expect_true!(pl.is_offset_in_zero_interval(start_off));
        expect_true!(pl.is_offset_in_zero_interval(span));
        expect_true!(pl.is_offset_in_zero_interval(end_off));
        expect_false!(pl.is_offset_in_zero_interval(span * 2));

        pl.remove_all_content(|_| {});
    }

    /// Interval [1, 3] in a single page list node.
    #[test]
    fn vmpl_interval_single_node_test() {
        let mut list = VmPageList::new();

        let expected_start = 1;
        let expected_end = 3;
        let size = VmPageListNode::PAGE_FAN_OUT as u64;
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        let mut start = 0;
        let mut end = 0;
        let res = list.for_every_page(|p, off| {
            if !(p.is_interval_start() || p.is_interval_end()) {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() {
                start = off;
            } else if p.is_interval_end() {
                end = off;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(expected_start * PAGE_SIZE, start);
        expect_eq!(expected_end * PAGE_SIZE, end);

        let expected_gaps = [0, expected_start, expected_end + 1, size];
        let mut gaps = [0u64; 4];
        let mut index = 0;
        start = 0;
        end = 0;
        let res = list.for_every_page_and_gap_in_range(
            0,
            size * PAGE_SIZE,
            |p, off| {
                if !(p.is_interval_start() || p.is_interval_end()) {
                    return Status::BAD_STATE;
                }
                if !p.is_zero_interval_dirty() {
                    return Status::BAD_STATE;
                }
                if p.is_interval_start() {
                    start = off;
                } else if p.is_interval_end() {
                    end = off;
                }
                Status::NEXT
            },
            |begin, end_gap| {
                if index < 4 {
                    gaps[index] = begin;
                    gaps[index + 1] = end_gap;
                    index += 2;
                }
                Status::NEXT
            },
        );
        expect_ok!(res);

        expect_eq!(expected_start * PAGE_SIZE, start);
        expect_eq!(expected_end * PAGE_SIZE, end);

        expect_eq!(4, index);
        for i in 0..index {
            expect_eq!(expected_gaps[i] * PAGE_SIZE, gaps[i]);
        }

        list.remove_all_content(|_| {});
    }

    /// Tests multi-node interval spanning across 3 nodes with unpopulated middle node.
    #[test]
    fn vmpl_interval_multiple_nodes_test() {
        let mut list = VmPageList::new();

        let expected_start = 1;
        let expected_end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);

        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        let mut start = 0;
        let mut end = 0;
        let mut valid = true;
        let res = list.for_every_page(|p, off| {
            if !(p.is_interval_start() || p.is_interval_end()) {
                valid = false;
            }
            if !p.is_zero_interval_dirty() {
                valid = false;
            }
            if p.is_interval_start() {
                start = off;
            } else if p.is_interval_end() {
                end = off;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_true!(valid);
        expect_eq!(expected_start * PAGE_SIZE, start);
        expect_eq!(expected_end * PAGE_SIZE, end);

        let expected_gaps = [0, expected_start, expected_end + 1, size];
        let mut gaps = [0u64; 4];
        let mut index = 0;
        start = 0;
        end = 0;
        let res = list.for_every_page_and_gap_in_range(
            0,
            size * PAGE_SIZE,
            |p, off| {
                if !(p.is_interval_start() || p.is_interval_end()) {
                    return Status::BAD_STATE;
                }
                if !p.is_zero_interval_dirty() {
                    return Status::BAD_STATE;
                }
                if p.is_interval_start() {
                    start = off;
                } else if p.is_interval_end() {
                    end = off;
                }
                Status::NEXT
            },
            |begin, end_gap| {
                if index < 4 {
                    gaps[index] = begin;
                    gaps[index + 1] = end_gap;
                    index += 2;
                }
                Status::NEXT
            },
        );
        expect_ok!(res);

        expect_eq!(expected_start * PAGE_SIZE, start);
        expect_eq!(expected_end * PAGE_SIZE, end);

        expect_eq!(4, index);
        for i in 0..index {
            expect_eq!(expected_gaps[i] * PAGE_SIZE, gaps[i]);
        }

        list.remove_all_content(|_| {});
    }

    /// Tests `for_every_page_and_gap_in_range` with partial interval range traversals.
    #[test]
    fn vmpl_interval_traversal_test() {
        let mut list = VmPageList::new();

        // Interval spanning across 3 nodes, with the middle one unpopulated.
        let expected_start = 1;
        let expected_end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        // End traversal partway into the interval.
        // Should only see the gap before the interval start.
        let expected_gaps = [0, expected_start];
        let mut gaps = [0u64; 2];
        let mut index = 0;
        let mut start = 0;
        let mut end = 0;
        let res = list.for_every_page_and_gap_in_range(
            0,
            (expected_end - 1) * PAGE_SIZE,
            |p, off| {
                if !(p.is_interval_start() || p.is_interval_end()) {
                    return Status::BAD_STATE;
                }
                if !p.is_zero_interval_dirty() {
                    return Status::BAD_STATE;
                }
                if p.is_interval_start() {
                    start = off;
                } else if p.is_interval_end() {
                    end = off;
                }
                Status::NEXT
            },
            |begin, end_gap| {
                if index < 2 {
                    gaps[index] = begin;
                    gaps[index + 1] = end_gap;
                    index += 2;
                }
                Status::NEXT
            },
        );
        expect_ok!(res);

        expect_eq!(expected_start * PAGE_SIZE, start);
        // We should not have seen the end of the interval.
        expect_eq!(0, end);

        expect_eq!(2, index);
        for i in 0..index {
            expect_eq!(expected_gaps[i] * PAGE_SIZE, gaps[i]);
        }

        // Start traversal partway into the interval.
        // Should only see the gap after the interval end.
        let expected_gaps2 = [expected_end + 1, size];
        index = 0;
        start = 0;
        end = 0;
        let res = list.for_every_page_and_gap_in_range(
            (expected_start + 1) * PAGE_SIZE,
            size * PAGE_SIZE,
            |p, off| {
                if !(p.is_interval_start() || p.is_interval_end()) {
                    return Status::BAD_STATE;
                }
                if !p.is_zero_interval_dirty() {
                    return Status::BAD_STATE;
                }
                if p.is_interval_start() {
                    start = off;
                } else if p.is_interval_end() {
                    end = off;
                }
                Status::NEXT
            },
            |begin, end_gap| {
                if index < 2 {
                    gaps[index] = begin;
                    gaps[index + 1] = end_gap;
                    index += 2;
                }
                Status::NEXT
            },
        );
        expect_ok!(res);

        // We should not have seen the start of the interval.
        expect_eq!(0, start);
        expect_eq!(expected_end * PAGE_SIZE, end);

        expect_eq!(2, index);
        for i in 0..index {
            expect_eq!(expected_gaps2[i] * PAGE_SIZE, gaps[i]);
        }

        // Start traversal partway into the interval, and also end before the interval end.
        // Should not see any gaps or pages either.
        index = 0;
        start = 0;
        end = 0;
        let res = list.for_every_page_and_gap_in_range(
            (expected_start + 1) * PAGE_SIZE,
            (expected_end - 1) * PAGE_SIZE,
            |p, off| {
                if !(p.is_interval_start() || p.is_interval_end()) {
                    return Status::BAD_STATE;
                }
                if !p.is_zero_interval_dirty() {
                    return Status::BAD_STATE;
                }
                if p.is_interval_start() {
                    start = off;
                } else if p.is_interval_end() {
                    end = off;
                }
                Status::NEXT
            },
            |begin, end_gap| {
                if index < 2 {
                    gaps[index] = begin;
                    gaps[index + 1] = end_gap;
                    index += 2;
                }
                Status::NEXT
            },
        );
        expect_ok!(res);

        expect_eq!(0, start);
        expect_eq!(0, end);
        expect_eq!(0, index);

        list.remove_all_content(|_| {});
    }

    /// Tests adding intervals to the left and right of an existing interval and merging them.
    #[test]
    fn vmpl_interval_merge_test() {
        let mut list = VmPageList::new();

        // Interval [7, 12].
        let expected_start = 7;
        let expected_end = 12;
        let size = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        // Add intervals to the left and right of the existing interval and verify that they are
        // merged into a single interval.
        let new_expected_start = 3;
        let new_expected_end = 20;
        expect_gt!(size, new_expected_end);
        // Interval [3, 6].
        expect_ok!(list.add_zero_interval(
            new_expected_start * PAGE_SIZE,
            expected_start * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));
        // Interval [13, 20].
        expect_ok!(list.add_zero_interval(
            (expected_end + 1) * PAGE_SIZE,
            (new_expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        let mut start = 0;
        let mut end = 0;
        let res = list.for_every_page(|p, off| {
            if !(p.is_interval_start() || p.is_interval_end()) {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() {
                start = off;
            } else if p.is_interval_end() {
                end = off;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(new_expected_start * PAGE_SIZE, start);
        expect_eq!(new_expected_end * PAGE_SIZE, end);

        let expected_gaps = [0, new_expected_start, new_expected_end + 1, size];
        let mut gaps = [0u64; 4];
        let mut index = 0;
        start = 0;
        end = 0;
        let res = list.for_every_page_and_gap_in_range(
            0,
            size * PAGE_SIZE,
            |p, off| {
                if !(p.is_interval_start() || p.is_interval_end()) {
                    return Status::BAD_STATE;
                }
                if !p.is_zero_interval_dirty() {
                    return Status::BAD_STATE;
                }
                if p.is_interval_start() {
                    start = off;
                } else if p.is_interval_end() {
                    end = off;
                }
                Status::NEXT
            },
            |begin, end_gap| {
                if index < 4 {
                    gaps[index] = begin;
                    gaps[index + 1] = end_gap;
                    index += 2;
                }
                Status::NEXT
            },
        );
        expect_ok!(res);

        expect_eq!(new_expected_start * PAGE_SIZE, start);
        expect_eq!(new_expected_end * PAGE_SIZE, end);

        expect_eq!(4, index);
        for i in 0..index {
            expect_eq!(expected_gaps[i] * PAGE_SIZE, gaps[i]);
        }

        list.remove_all_content(|_| {});
    }

    /// Adding a page in the interval should split the interval.
    #[test]
    fn vmpl_interval_add_page_test() {
        let mut list = VmPageList::new();
        let expected_start = 1;
        let expected_end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        let (page, _paddr) = unwrap_ok!(pmm::alloc_page(0), "pmm alloc page");
        let page_offset = VmPageListNode::PAGE_FAN_OUT as u64;
        expect_true!(add_page(&mut list, page, page_offset * PAGE_SIZE));

        let expected_intervals = [
            expected_start * PAGE_SIZE,
            (page_offset - 1) * PAGE_SIZE,
            (page_offset + 1) * PAGE_SIZE,
            expected_end * PAGE_SIZE,
        ];
        let mut intervals = [0u64; 4];
        let mut interval_index = 0;
        let mut page_off = 0;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !(p.is_interval_start() || p.is_interval_end() || p.is_page()) {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() {
                if interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
            } else if p.is_interval_end() {
                if interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
            } else if p.is_page() {
                page_off = off;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(4, interval_index);
        expect_true!(expected_intervals == intervals);
        expect_eq!(page_offset * PAGE_SIZE, page_off);

        let mut free_list = fbl::Vector::<VmPagePtr>::new();
        list.remove_all_content(|mut p| {
            if p.is_page() {
                free_list.push_back(p.release_page()).expect("vector push");
            }
        });
        expect_eq!(1, free_list.len());

        // SAFETY: `page` was removed from `list`.
        unsafe { pmm::free_page(page) };
    }

    /// 3 page interval such that adding a page in the middle creates two distinct slots.
    #[test]
    fn vmpl_interval_add_page_slots_test() {
        let mut list = VmPageList::new();
        let expected_start = 0;
        let expected_end = 2;
        let size = VmPageListNode::PAGE_FAN_OUT as u64;
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        let (page, _paddr) = unwrap_ok!(pmm::alloc_page(0), "pmm alloc page");
        let page_offset = 1;
        expect_true!(add_page(&mut list, page, page_offset * PAGE_SIZE));

        let expected_intervals = [expected_start * PAGE_SIZE, expected_end * PAGE_SIZE];
        let mut intervals = [0u64; 2];
        let mut interval_index = 0;
        let mut page_off = 0;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !(p.is_interval_slot() || p.is_page()) {
                return Status::BAD_STATE;
            }
            if p.is_interval_slot() {
                intervals[interval_index] = off;
                interval_index += 1;
            } else if p.is_page() {
                page_off = off;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(2, interval_index);
        expect_true!(expected_intervals == intervals);
        expect_eq!(page_offset * PAGE_SIZE, page_off);

        let mut free_list = fbl::Vector::<VmPagePtr>::new();
        list.remove_all_content(|mut p| {
            if p.is_page() {
                free_list.push_back(p.release_page()).expect("vector push");
            }
        });
        expect_eq!(1, free_list.len());

        // SAFETY: `page` was removed from `list`.
        unsafe { pmm::free_page(page) };
    }

    /// Tests adding pages at the start of an interval.
    #[test]
    fn vmpl_interval_add_page_start_test() {
        let mut list = VmPageList::new();
        let expected_start = 0;
        let expected_end = 2;
        let size = VmPageListNode::PAGE_FAN_OUT as u64;
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        let pages = get_pages::<2>();

        // Add a page at the start of the interval.
        expect_true!(add_page(&mut list, pages[0], expected_start * PAGE_SIZE));

        let expected_intervals = [(expected_start + 1) * PAGE_SIZE, expected_end * PAGE_SIZE];
        let mut intervals = [0u64; 2];
        let mut interval_index = 0;
        let mut page_off = size * PAGE_SIZE;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !(p.is_interval_start() || p.is_interval_end() || p.is_page()) {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() {
                if interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
            } else if p.is_interval_end() {
                if interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
            } else if p.is_page() {
                page_off = off;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(2, interval_index);
        expect_true!(expected_intervals == intervals);
        expect_eq!(expected_start * PAGE_SIZE, page_off);

        // Add another page at the start of the new interval.
        expect_true!(add_page(&mut list, pages[1], (expected_start + 1) * PAGE_SIZE));

        let expected_page_offsets = [expected_start * PAGE_SIZE, (expected_start + 1) * PAGE_SIZE];
        let mut page_offsets = [0u64; 2];
        let mut page_index = 0;
        interval_index = 0;
        let mut interval_slot = 0;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !(p.is_interval_slot() || p.is_page()) {
                return Status::BAD_STATE;
            }
            if p.is_interval_slot() {
                interval_slot = off;
                interval_index += 1;
            } else if p.is_page() {
                page_offsets[page_index] = off;
                page_index += 1;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(1, interval_index);
        expect_eq!(expected_end * PAGE_SIZE, interval_slot);
        expect_eq!(2, page_index);
        expect_true!(expected_page_offsets == page_offsets);

        list.remove_all_content(|mut p| {
            if p.is_page() {
                let _ = p.release_page();
            }
        });
        free_pages(pages);
    }

    /// Tests adding pages at the end of an interval.
    #[test]
    fn vmpl_interval_add_page_end_test() {
        let mut list = VmPageList::new();
        let expected_start = 0;
        let expected_end = 2;
        let size = VmPageListNode::PAGE_FAN_OUT as u64;
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        let pages = get_pages::<2>();

        // Add a page at the end of the interval.
        expect_true!(add_page(&mut list, pages[0], expected_end * PAGE_SIZE));

        let expected_intervals = [expected_start * PAGE_SIZE, (expected_end - 1) * PAGE_SIZE];
        let mut intervals = [0u64; 2];
        let mut interval_index = 0;
        let mut page_off = 0;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !(p.is_interval_start() || p.is_interval_end() || p.is_page()) {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() {
                if interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
            } else if p.is_interval_end() {
                if interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
            } else if p.is_page() {
                page_off = off;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(2, interval_index);
        expect_true!(expected_intervals == intervals);
        expect_eq!(expected_end * PAGE_SIZE, page_off);

        // Add another page at the end of the new interval.
        expect_true!(add_page(&mut list, pages[1], (expected_end - 1) * PAGE_SIZE));

        let expected_page_offsets = [(expected_end - 1) * PAGE_SIZE, expected_end * PAGE_SIZE];
        let mut page_offsets = [0u64; 2];
        let mut page_index = 0;
        interval_index = 0;
        let mut interval_slot = 0;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !(p.is_interval_slot() || p.is_page()) {
                return Status::BAD_STATE;
            }
            if p.is_interval_slot() {
                interval_slot = off;
                interval_index += 1;
            } else if p.is_page() {
                page_offsets[page_index] = off;
                page_index += 1;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(1, interval_index);
        expect_eq!(expected_start * PAGE_SIZE, interval_slot);
        expect_eq!(2, page_index);
        expect_true!(expected_page_offsets == page_offsets);

        list.remove_all_content(|mut p| {
            if p.is_page() {
                let _ = p.release_page();
            }
        });
        free_pages(pages);
    }

    /// Tests replacing a single-page interval slot with a page.
    #[test]
    fn vmpl_interval_replace_slot_test() {
        let mut list = VmPageList::new();
        let expected_interval = 0;
        let size = VmPageListNode::PAGE_FAN_OUT as u64;
        expect_ok!(list.add_zero_interval(
            expected_interval * PAGE_SIZE,
            (expected_interval + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        let mut interval_off = size * PAGE_SIZE;
        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_interval_slot() {
                return Status::BAD_STATE;
            }
            interval_off = off;
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(expected_interval * PAGE_SIZE, interval_off);

        // Add a page in the interval slot.
        let (page, _paddr) = unwrap_ok!(pmm::alloc_page(0), "pmm alloc page");
        expect_true!(add_page(&mut list, page, expected_interval * PAGE_SIZE));

        let mut page_off = size * PAGE_SIZE;
        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_page() {
                return Status::BAD_STATE;
            }
            page_off = off;
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(expected_interval * PAGE_SIZE, page_off);

        list.remove_all_content(|mut p| {
            if p.is_page() {
                let _ = p.release_page();
            }
        });
        // SAFETY: `page` was removed from `list`.
        unsafe { pmm::free_page(page) };
    }
    /// Tests populating all slots across a 5-node interval.
    #[test]
    fn vmpl_interval_populate_full_test() {
        let mut list = VmPageList::new();
        let expected_start = 1;
        let expected_end = 4 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 5 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        // Populate the entire interval.
        expect_ok!(list.populate_slots_in_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE
        ));

        let mut next_off = expected_start * PAGE_SIZE;
        // We should only see interval slots.
        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_interval_slot() {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if off != next_off {
                return Status::OUT_OF_RANGE;
            }
            next_off += PAGE_SIZE;
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!((expected_end + 1) * PAGE_SIZE, next_off);

        list.remove_all_content(|_| {});
    }

    /// Tests populating slots in the middle of a multi-node interval.
    #[test]
    fn vmpl_interval_populate_partial_test() {
        let mut list = VmPageList::new();
        let expected_start = 1;
        let expected_end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        // Populate some slots in the middle of the interval.
        let slot_start = expected_start + 2;
        let slot_end = expected_end - 2;
        expect_gt!(slot_end, slot_start);
        expect_ok!(
            list.populate_slots_in_interval(slot_start * PAGE_SIZE, (slot_end + 1) * PAGE_SIZE)
        );

        let expected_intervals = [
            expected_start * PAGE_SIZE,
            (slot_start - 1) * PAGE_SIZE,
            (slot_end + 1) * PAGE_SIZE,
            expected_end * PAGE_SIZE,
        ];
        let mut intervals = [0u64; 4];
        let mut interval_index = 0;
        let mut slot = slot_start * PAGE_SIZE;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_interval() {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() || p.is_interval_end() {
                if p.is_interval_start() && interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                if p.is_interval_end() && interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
                return Status::NEXT;
            }
            if off != slot {
                return Status::BAD_STATE;
            }
            slot += PAGE_SIZE;
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!((slot_end + 1) * PAGE_SIZE, slot);
        expect_eq!(4, interval_index);
        expect_true!(expected_intervals == intervals);

        list.remove_all_content(|_| {});
    }

    /// Tests populating slots beginning at the start of an interval.
    #[test]
    fn vmpl_interval_populate_start_test() {
        let mut list = VmPageList::new();
        let expected_start = 1;
        let expected_end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        // Populate some slots beginning at the start of the interval.
        let slot_start = expected_start;
        let slot_end = expected_end - 2;
        expect_gt!(slot_end, slot_start);
        expect_ok!(
            list.populate_slots_in_interval(slot_start * PAGE_SIZE, (slot_end + 1) * PAGE_SIZE)
        );

        let expected_intervals = [(slot_end + 1) * PAGE_SIZE, expected_end * PAGE_SIZE];
        let mut intervals = [0u64; 2];
        let mut interval_index = 0;
        let mut slot = slot_start * PAGE_SIZE;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_interval() {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() || p.is_interval_end() {
                if p.is_interval_start() && interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                if p.is_interval_end() && interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
                return Status::NEXT;
            }
            if off != slot {
                return Status::BAD_STATE;
            }
            slot += PAGE_SIZE;
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!((slot_end + 1) * PAGE_SIZE, slot);
        expect_eq!(2, interval_index);
        expect_true!(expected_intervals == intervals);

        list.remove_all_content(|_| {});
    }

    /// Tests populating slots ending at the end of an interval.
    #[test]
    fn vmpl_interval_populate_end_test() {
        let mut list = VmPageList::new();
        let expected_start = 1;
        let expected_end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        // Populate some slots ending at the end of the interval.
        let slot_start = expected_start + 2;
        let slot_end = expected_end;
        expect_gt!(slot_end, slot_start);
        expect_ok!(
            list.populate_slots_in_interval(slot_start * PAGE_SIZE, (slot_end + 1) * PAGE_SIZE)
        );

        let expected_intervals = [expected_start * PAGE_SIZE, (slot_start - 1) * PAGE_SIZE];
        let mut intervals = [0u64; 2];
        let mut interval_index = 0;
        let mut slot = slot_start * PAGE_SIZE;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_interval() {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() || p.is_interval_end() {
                if p.is_interval_start() && interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                if p.is_interval_end() && interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
                return Status::NEXT;
            }
            if off != slot {
                return Status::BAD_STATE;
            }
            slot += PAGE_SIZE;
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!((slot_end + 1) * PAGE_SIZE, slot);
        expect_eq!(2, interval_index);
        expect_true!(expected_intervals == intervals);

        list.remove_all_content(|_| {});
    }

    /// Tests populating a single slot, idempotency, and returning it with return_interval_slot.
    #[test]
    fn vmpl_interval_populate_slot_test() {
        let mut list = VmPageList::new();
        let expected_start = 1;
        let expected_end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64);
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64);
        expect_gt!(size, expected_end);
        expect_ok!(list.add_zero_interval(
            expected_start * PAGE_SIZE,
            (expected_end + 1) * PAGE_SIZE,
            ZeroRangeDirtyState::Dirty
        ));

        expect_true!(list.any_pages_or_intervals_in_range(0, size * PAGE_SIZE));

        // Populate a single slot in the interval.
        let single_slot = expected_end - 3;
        expect_ok!(
            list.populate_slots_in_interval(single_slot * PAGE_SIZE, (single_slot + 1) * PAGE_SIZE)
        );

        let expected_intervals = [
            expected_start * PAGE_SIZE,
            (single_slot - 1) * PAGE_SIZE,
            (single_slot + 1) * PAGE_SIZE,
            expected_end * PAGE_SIZE,
        ];
        let mut intervals = [0u64; 4];
        let mut interval_index = 0;

        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_interval() {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() || p.is_interval_end() {
                if p.is_interval_start() && interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                if p.is_interval_end() && interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
                return Status::NEXT;
            }
            if off != single_slot * PAGE_SIZE {
                return Status::BAD_STATE;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(4, interval_index);
        expect_true!(expected_intervals == intervals);

        // Try to populate a slot over a single sentinel. This should be a no-op.
        expect_ok!(
            list.populate_slots_in_interval(single_slot * PAGE_SIZE, (single_slot + 1) * PAGE_SIZE)
        );
        interval_index = 0;
        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !p.is_interval() {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() || p.is_interval_end() {
                if p.is_interval_start() && interval_index % 2 == 1 {
                    return Status::BAD_STATE;
                }
                if p.is_interval_end() && interval_index % 2 == 0 {
                    return Status::BAD_STATE;
                }
                intervals[interval_index] = off;
                interval_index += 1;
                return Status::NEXT;
            }
            if off != single_slot * PAGE_SIZE {
                return Status::BAD_STATE;
            }
            Status::NEXT
        });
        expect_ok!(res);
        expect_eq!(4, interval_index);
        expect_true!(expected_intervals == intervals);

        // Try to return the single slot that we populated. This should return the interval to its
        // original state.
        list.return_interval_slot(single_slot * PAGE_SIZE);
        let res = list.for_every_page_in_range(0, size * PAGE_SIZE, |p, off| {
            if !(p.is_interval_start() || p.is_interval_end()) {
                return Status::BAD_STATE;
            }
            if !p.is_zero_interval_dirty() {
                return Status::BAD_STATE;
            }
            if p.is_interval_start() && off != expected_start * PAGE_SIZE {
                return Status::BAD_STATE;
            }
            if p.is_interval_end() && off != expected_end * PAGE_SIZE {
                return Status::BAD_STATE;
            }
            Status::NEXT
        });
        expect_ok!(res);

        list.remove_all_content(|_| {});
    }

    /// Tests awaiting clean length handling when splitting an interval.
    #[test]
    fn vmpl_awaiting_clean_split_test() {
        let mut list = VmPageList::new();
        let start = PAGE_SIZE;
        let end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        expect_gt!(size, end);
        expect_ok!(list.add_zero_interval(start, end + PAGE_SIZE, ZeroRangeDirtyState::Dirty));

        expect_true!(list.any_pages_or_intervals_in_range(0, size));

        // Set awaiting clean length.
        let expected_len = end - start + PAGE_SIZE;
        list.lookup_mut(start).unwrap().set_zero_interval_awaiting_clean_length(expected_len);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());

        // Split the interval in the middle.
        let mid = end - 2 * PAGE_SIZE;
        expect_ok!(list.populate_slots_in_interval(mid, mid + PAGE_SIZE));

        // Awaiting clean length remains unchanged.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(0, list.lookup(mid).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(0, list.lookup(mid + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length());

        // Split the interval at the end.
        expect_ok!(list.populate_slots_in_interval(end, end + PAGE_SIZE));

        // Awaiting clean length remains unchanged.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(0, list.lookup(mid).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(0, list.lookup(mid + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(0, list.lookup(end).unwrap().zero_interval_awaiting_clean_length());

        // Split the interval at the start.
        expect_ok!(list.populate_slots_in_interval(start, start + PAGE_SIZE));

        // Awaiting clean length now moves to the new start.
        expect_eq!(PAGE_SIZE, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            expected_len - PAGE_SIZE,
            list.lookup(start + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(0, list.lookup(mid).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(0, list.lookup(mid + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(0, list.lookup(end).unwrap().zero_interval_awaiting_clean_length());

        list.remove_all_content(|_| {});
    }

    /// Tests restoring awaiting clean length when returning a split slot.
    #[test]
    fn vmpl_awaiting_clean_return_slot_test() {
        let mut list = VmPageList::new();
        let start = PAGE_SIZE;
        let end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        expect_gt!(size, end);
        expect_ok!(list.add_zero_interval(start, end + PAGE_SIZE, ZeroRangeDirtyState::Dirty));

        expect_true!(list.any_pages_or_intervals_in_range(0, size));

        // Set awaiting clean length.
        let expected_len = end - start + PAGE_SIZE;
        list.lookup_mut(start).unwrap().set_zero_interval_awaiting_clean_length(expected_len);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());

        // Split the interval at the start.
        expect_ok!(list.populate_slots_in_interval(start, start + PAGE_SIZE));

        // Awaiting clean length now moves to the new start.
        expect_eq!(PAGE_SIZE, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            expected_len - PAGE_SIZE,
            list.lookup(start + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the populated slot.
        list.return_interval_slot(start);

        // Awaiting clean length is now restored.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());

        list.remove_all_content(|_| {});
    }

    /// Tests returning multiple split slots and verifying awaiting clean length merges.
    #[test]
    fn vmpl_awaiting_clean_return_slots_test() {
        let mut list = VmPageList::new();
        let start = PAGE_SIZE;
        let end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        expect_gt!(size, end);
        expect_ok!(list.add_zero_interval(start, end + PAGE_SIZE, ZeroRangeDirtyState::Dirty));

        expect_true!(list.any_pages_or_intervals_in_range(0, size));

        // Set awaiting clean length.
        let expected_len = end - start + PAGE_SIZE;
        list.lookup_mut(start).unwrap().set_zero_interval_awaiting_clean_length(expected_len);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());

        // Split the start multiple times, so that all the resultant slots have non-zero awaiting
        // clean lengths.
        expect_ok!(list.populate_slots_in_interval(start, start + PAGE_SIZE));
        expect_ok!(list.populate_slots_in_interval(start + PAGE_SIZE, start + 2 * PAGE_SIZE));
        expect_ok!(list.populate_slots_in_interval(start + 2 * PAGE_SIZE, start + 3 * PAGE_SIZE));

        // Verify awaiting clean lengths.
        expect_eq!(PAGE_SIZE, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            PAGE_SIZE,
            list.lookup(start + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            PAGE_SIZE,
            list.lookup(start + 2 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            expected_len - 3 * PAGE_SIZE,
            list.lookup(start + 3 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the first slot. This will combine the first two slots into an interval.
        list.return_interval_slot(start);
        expect_true!(list.lookup(start).unwrap().is_interval_start());
        expect_true!(list.lookup(start + PAGE_SIZE).unwrap().is_interval_end());

        // Verify awaiting clean lengths.
        expect_eq!(
            2 * PAGE_SIZE,
            list.lookup(start).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            PAGE_SIZE,
            list.lookup(start + 2 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            expected_len - 3 * PAGE_SIZE,
            list.lookup(start + 3 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the third slot. This will merge all the intervals and return everything to the
        // original state.
        list.return_interval_slot(start + 2 * PAGE_SIZE);
        // Awaiting clean length is restored.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        let res = list.for_every_page(|p, off| {
            if p.is_interval_start() {
                if off != start {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            if p.is_interval_end() {
                if off != end {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            Status::BAD_STATE
        });
        expect_ok!(res);

        list.remove_all_content(|_| {});
    }

    /// Tests populate_slots_in_interval with awaiting clean length and restoring slots.
    #[test]
    fn vmpl_awaiting_clean_populate_slots_test() {
        let mut list = VmPageList::new();
        let start = PAGE_SIZE;
        let end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        expect_gt!(size, end);
        expect_ok!(list.add_zero_interval(start, end + PAGE_SIZE, ZeroRangeDirtyState::Dirty));

        expect_true!(list.any_pages_or_intervals_in_range(0, size));

        // Set awaiting clean length.
        let expected_len = end - start + PAGE_SIZE;
        list.lookup_mut(start).unwrap().set_zero_interval_awaiting_clean_length(expected_len);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());

        // Populate some slots at the start.
        expect_ok!(list.populate_slots_in_interval(start, start + 3 * PAGE_SIZE));

        // Verify awaiting clean lengths.
        expect_eq!(PAGE_SIZE, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            PAGE_SIZE,
            list.lookup(start + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            PAGE_SIZE,
            list.lookup(start + 2 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            expected_len - 3 * PAGE_SIZE,
            list.lookup(start + 3 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the first slot. This will combine the first two slots into an interval.
        list.return_interval_slot(start);
        expect_true!(list.lookup(start).unwrap().is_interval_start());
        expect_true!(list.lookup(start + PAGE_SIZE).unwrap().is_interval_end());

        // Verify awaiting clean lengths.
        expect_eq!(
            2 * PAGE_SIZE,
            list.lookup(start).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            PAGE_SIZE,
            list.lookup(start + 2 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            expected_len - 3 * PAGE_SIZE,
            list.lookup(start + 3 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the third slot. This will merge all the intervals and return everything to the
        // original state.
        list.return_interval_slot(start + 2 * PAGE_SIZE);
        // Awaiting clean length is restored.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        let res = list.for_every_page(|p, off| {
            if p.is_interval_start() {
                if off != start {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            if p.is_interval_end() {
                if off != end {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            Status::BAD_STATE
        });
        expect_ok!(res);

        list.remove_all_content(|_| {});
    }

    /// Tests intersecting awaiting clean length with slot populate and return.
    #[test]
    fn vmpl_awaiting_clean_intersecting_test() {
        let mut list = VmPageList::new();
        let start = PAGE_SIZE;
        let end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        expect_gt!(size, end);
        expect_ok!(list.add_zero_interval(start, end + PAGE_SIZE, ZeroRangeDirtyState::Dirty));

        expect_true!(list.any_pages_or_intervals_in_range(0, size));

        // Set awaiting clean length to only a portion of the interval.
        let expected_len = 2 * PAGE_SIZE;
        list.lookup_mut(start).unwrap().set_zero_interval_awaiting_clean_length(expected_len);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());

        // Populate some slots at the start, some of them within the awaiting clean length, and some
        // outside.
        expect_ok!(list.populate_slots_in_interval(start, start + 3 * PAGE_SIZE));

        // Verify awaiting clean lengths.
        expect_eq!(PAGE_SIZE, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            PAGE_SIZE,
            list.lookup(start + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + 2 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + 3 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the first slot. This will combine the first two slots into an interval.
        list.return_interval_slot(start);
        expect_true!(list.lookup(start).unwrap().is_interval_start());
        expect_true!(list.lookup(start + PAGE_SIZE).unwrap().is_interval_end());

        // Verify awaiting clean lengths.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            0,
            list.lookup(start + 2 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + 3 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the third slot. This will merge all the intervals and return everything to the
        // original state.
        list.return_interval_slot(start + 2 * PAGE_SIZE);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        let res = list.for_every_page(|p, off| {
            if p.is_interval_start() {
                if off != start {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            if p.is_interval_end() {
                if off != end {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            Status::BAD_STATE
        });
        expect_ok!(res);

        // Populate a slot again, but starting partway into the interval.
        expect_ok!(list.populate_slots_in_interval(start + PAGE_SIZE, start + 2 * PAGE_SIZE));

        // The start's awaiting clean length should remain unchanged.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        // The awaiting clean length for the populated slot and the remaining interval is 0.
        expect_eq!(
            0,
            list.lookup(start + PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + 2 * PAGE_SIZE).unwrap().zero_interval_awaiting_clean_length()
        );

        // Return the slot. This should return to the original state.
        list.return_interval_slot(start + PAGE_SIZE);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        let res = list.for_every_page(|p, off| {
            if p.is_interval_start() {
                if off != start {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            if p.is_interval_end() {
                if off != end {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            Status::BAD_STATE
        });
        expect_ok!(res);

        list.remove_all_content(|_| {});
    }

    /// Tests non-intersecting awaiting clean length with slot populate and return.
    #[test]
    fn vmpl_awaiting_clean_non_intersecting_test() {
        let mut list = VmPageList::new();
        let start = PAGE_SIZE;
        let end = 2 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        let size = 3 * (VmPageListNode::PAGE_FAN_OUT as u64) * PAGE_SIZE;
        expect_gt!(size, end);
        expect_ok!(list.add_zero_interval(start, end + PAGE_SIZE, ZeroRangeDirtyState::Dirty));

        expect_true!(list.any_pages_or_intervals_in_range(0, size));

        // Set awaiting clean length to only a portion of the interval.
        let expected_len = 2 * PAGE_SIZE;
        list.lookup_mut(start).unwrap().set_zero_interval_awaiting_clean_length(expected_len);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());

        // Populate some slots that do not intersect with the awaiting clean length.
        expect_ok!(list.populate_slots_in_interval(
            start + expected_len,
            start + expected_len + 3 * PAGE_SIZE
        ));

        // Verify awaiting clean lengths.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            0,
            list.lookup(start + expected_len).unwrap().zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + expected_len + PAGE_SIZE)
                .unwrap()
                .zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + expected_len + 2 * PAGE_SIZE)
                .unwrap()
                .zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + expected_len + 3 * PAGE_SIZE)
                .unwrap()
                .zero_interval_awaiting_clean_length()
        );

        // Return the first slot. This will merge the first two slots back into the interval.
        list.return_interval_slot(start + expected_len);
        expect_true!(list.lookup(start + expected_len + PAGE_SIZE).unwrap().is_interval_end());

        // Verify awaiting clean lengths.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        expect_eq!(
            0,
            list.lookup(start + expected_len + 2 * PAGE_SIZE)
                .unwrap()
                .zero_interval_awaiting_clean_length()
        );
        expect_eq!(
            0,
            list.lookup(start + expected_len + 3 * PAGE_SIZE)
                .unwrap()
                .zero_interval_awaiting_clean_length()
        );

        // Return the third slot. This will merge all the intervals and return everything to the
        // original state.
        list.return_interval_slot(start + expected_len + 2 * PAGE_SIZE);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        let res = list.for_every_page(|p, off| {
            if p.is_interval_start() {
                if off != start {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            if p.is_interval_end() {
                if off != end {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            Status::BAD_STATE
        });
        expect_ok!(res);

        // Populate a slot again, this time at the end.
        expect_ok!(list.populate_slots_in_interval(end, end + PAGE_SIZE));

        // The start's awaiting clean length should remain unchanged.
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        // The awaiting clean length for the populated slot is 0.
        expect_eq!(0, list.lookup(end).unwrap().zero_interval_awaiting_clean_length());

        // Return the slot. This should return to the original state.
        list.return_interval_slot(end);
        expect_eq!(expected_len, list.lookup(start).unwrap().zero_interval_awaiting_clean_length());
        let res = list.for_every_page(|p, off| {
            if p.is_interval_start() {
                if off != start {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            if p.is_interval_end() {
                if off != end {
                    return Status::BAD_STATE;
                }
                return Status::NEXT;
            }
            Status::BAD_STATE
        });
        expect_ok!(res);

        list.remove_all_content(|_| {});
    }
}
