// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::*;
use pin_init::stack_pin_init;
use zx_status::Status;

fn verify_counts(
    bitmap: &RleBitmap,
    rng_expected: usize,
    bit_expected: usize,
    mut cb: impl FnMut(usize, usize, usize),
) {
    let mut rng_count = 0;
    let mut bit_count = 0;
    for range in bitmap.into_iter() {
        assert_eq!(range.bitoff, range.start());
        assert_eq!(range.bitoff + range.bitlen, range.end());
        cb(rng_count, range.bitoff, range.bitlen);
        rng_count += 1;
        bit_count += range.bitlen;
    }

    assert_eq!(rng_count, rng_expected);
    assert_eq!(rng_count, bitmap.num_ranges());
    assert_eq!(bit_count, bit_expected);
    assert_eq!(bit_count, bitmap.num_bits());
}

#[test]
fn test_rle_initialized_empty() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };
    assert!(!bitmap.get(5, 6).all_set);
    let mut iter = bitmap.into_iter();
    assert!(iter.next().is_none());
}

#[test]
fn test_rle_single_bit() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };
    assert!(!bitmap.get(2, 3).all_set);

    assert_eq!(bitmap.set(2, 3), Ok(()));
    assert!(bitmap.get(2, 3).all_set);
    assert_eq!(bitmap.num_bits(), 1);

    verify_counts(bitmap, 1, 1, |_index, bitoff, bitlen| {
        assert_eq!(bitoff, 2);
        assert_eq!(bitlen, 1);
    });

    assert_eq!(bitmap.clear(2, 3), Ok(()));
    assert!(!bitmap.get(2, 3).all_set);
    verify_counts(bitmap, 0, 0, |_, _, _| {});
}

#[test]
fn test_rle_set_twice() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    assert_eq!(bitmap.set_one(2), Ok(()));
    assert!(bitmap.get_one(2));
    assert_eq!(bitmap.num_bits(), 1);

    assert_eq!(bitmap.set_one(2), Ok(()));
    assert!(bitmap.get_one(2));
    assert_eq!(bitmap.num_bits(), 1);

    verify_counts(bitmap, 1, 1, |_, bitoff, bitlen| {
        assert_eq!(bitoff, 2);
        assert_eq!(bitlen, 1);
    });
}

#[test]
fn test_rle_clear_twice() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    assert_eq!(bitmap.set_one(2), Ok(()));
    assert_eq!(bitmap.num_bits(), 1);

    assert_eq!(bitmap.clear_one(2), Ok(()));
    assert!(!bitmap.get_one(2));
    assert_eq!(bitmap.num_bits(), 0);

    assert_eq!(bitmap.clear_one(2), Ok(()));
    assert!(!bitmap.get_one(2));
    assert_eq!(bitmap.num_bits(), 0);

    let mut iter = bitmap.into_iter();
    assert!(iter.next().is_none());
}

#[test]
fn test_rle_get_return_arg() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    let res = bitmap.get(2, 3);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 2);

    assert_eq!(bitmap.set_one(2), Ok(()));
    let res = bitmap.get(2, 3);
    assert!(res.all_set);
    assert_eq!(res.first_unset, 3);

    let res = bitmap.get(2, 4);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 3);

    assert_eq!(bitmap.set(3, 4), Ok(()));
    let res = bitmap.get(2, 5);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 4);

    verify_counts(bitmap, 1, 2, |_, bitoff, bitlen| {
        assert_eq!(bitoff, 2);
        assert_eq!(bitlen, 2);
    });
}

#[test]
fn test_rle_set_range() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };
    assert_eq!(bitmap.set(2, 100), Ok(()));
    assert_eq!(bitmap.num_bits(), 98);

    let res = bitmap.get(2, 3);
    assert!(res.all_set);
    assert_eq!(res.first_unset, 3);

    let res = bitmap.get(99, 100);
    assert!(res.all_set);
    assert_eq!(res.first_unset, 100);

    let res = bitmap.get(1, 2);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 1);

    let res = bitmap.get(100, 101);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 100);

    let res = bitmap.get(2, 100);
    assert!(res.all_set);
    assert_eq!(res.first_unset, 100);

    let res = bitmap.get(50, 80);
    assert!(res.all_set);
    assert_eq!(res.first_unset, 80);
}

#[test]
fn test_rle_clear_all() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    assert_eq!(bitmap.set(2, 100), Ok(()));

    bitmap.clear_all();

    let mut iter = bitmap.into_iter();
    assert!(iter.next().is_none());

    assert_eq!(bitmap.set(2, 100), Ok(()));

    for range in bitmap.into_iter() {
        assert_eq!(range.bitoff, 2);
        assert_eq!(range.bitlen, 98);
    }

    verify_counts(bitmap, 1, 98, |_, bitoff, bitlen| {
        assert_eq!(bitoff, 2);
        assert_eq!(bitlen, 98);
    });
}

#[test]
fn test_rle_clear_subrange() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    assert_eq!(bitmap.set(2, 100), Ok(()));
    assert_eq!(bitmap.num_bits(), 98);
    assert_eq!(bitmap.clear(50, 80), Ok(()));
    assert_eq!(bitmap.num_bits(), 68);

    let res = bitmap.get(2, 100);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 50);

    let res = bitmap.get(2, 50);
    assert!(res.all_set);
    assert_eq!(res.first_unset, 50);

    let res = bitmap.get(80, 100);
    assert!(res.all_set);
    assert_eq!(res.first_unset, 100);

    let res = bitmap.get(50, 80);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 50);

    verify_counts(bitmap, 2, 68, |index, bitoff, bitlen| {
        if index == 0 {
            assert_eq!(bitoff, 2);
            assert_eq!(bitlen, 48);
        } else {
            assert_eq!(bitoff, 80);
            assert_eq!(bitlen, 20);
        }
    });
}

#[test]
fn test_rle_merge_ranges() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    const MAX_VAL: usize = 100;

    for i in (0..MAX_VAL).step_by(2) {
        assert_eq!(bitmap.set_one(i), Ok(()));
    }

    verify_counts(bitmap, MAX_VAL / 2, MAX_VAL / 2, |index, bitoff, bitlen| {
        assert_eq!(bitoff, 2 * index);
        assert_eq!(bitlen, 1);
    });

    for i in (1..MAX_VAL).step_by(4) {
        assert_eq!(bitmap.set_one(i), Ok(()));
    }

    verify_counts(bitmap, MAX_VAL / 4, 3 * MAX_VAL / 4, |index, bitoff, bitlen| {
        assert_eq!(bitoff, 4 * index);
        assert_eq!(bitlen, 3);
    });
}

#[test]
fn test_rle_split_ranges() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    const MAX_VAL: usize = 100;
    assert_eq!(bitmap.set(0, MAX_VAL), Ok(()));

    for i in (1..MAX_VAL).step_by(4) {
        assert_eq!(bitmap.clear_one(i), Ok(()));
    }

    verify_counts(bitmap, MAX_VAL / 4 + 1, 3 * MAX_VAL / 4, |index, bitoff, bitlen| {
        if index == 0 {
            assert_eq!(bitoff, 0);
            assert_eq!(bitlen, 1);
        } else {
            let offset = 4 * index - 2;
            let len = core::cmp::min(3, MAX_VAL - offset);
            assert_eq!(bitoff, offset);
            assert_eq!(bitlen, len);
        }
    });

    for i in (0..MAX_VAL).step_by(2) {
        assert_eq!(bitmap.clear_one(i), Ok(()));
    }

    verify_counts(bitmap, MAX_VAL / 4, MAX_VAL / 4, |index, bitoff, bitlen| {
        assert_eq!(bitoff, 4 * index + 3);
        assert_eq!(bitlen, 1);
    });
}

#[test]
fn test_rle_boundary_arguments() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    assert_eq!(bitmap.set(0, 0), Ok(()));
    assert_eq!(bitmap.set(5, 4), Err(Status::INVALID_ARGS));
    assert_eq!(bitmap.set(5, 5), Ok(()));

    assert_eq!(bitmap.clear(0, 0), Ok(()));
    assert_eq!(bitmap.clear(5, 4), Err(Status::INVALID_ARGS));
    assert_eq!(bitmap.clear(5, 5), Ok(()));

    assert!(bitmap.get(0, 0).all_set);
    assert!(bitmap.get(5, 4).all_set);
    assert!(bitmap.get(5, 5).all_set);
}

#[test]
fn test_rle_no_alloc() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    stack_pin_init!(let free_list_pinned = FreeList::<usize>::new());
    // SAFETY: We do not move the FreeList out of its pinned location during the test.
    let free_list = unsafe { free_list_pinned.as_mut().get_unchecked_mut() };

    assert_eq!(bitmap.set_no_alloc(0, 65536, free_list), Err(Status::NO_MEMORY));

    let elem = Element::new(0, 0);
    free_list.push_back(fbl::UniquePtr::try_new(elem).unwrap());

    assert_eq!(bitmap.set_no_alloc(0, 65536, free_list), Ok(()));
    assert!(bitmap.get(0, 65536).all_set);
    assert!(free_list.is_empty());

    assert_eq!(bitmap.clear_no_alloc(1, 65535, free_list), Err(Status::NO_MEMORY));

    let elem = Element::new(0, 0);
    free_list.push_back(fbl::UniquePtr::try_new(elem).unwrap());

    assert_eq!(bitmap.clear_no_alloc(1, 65535, free_list), Ok(()));
    let res = bitmap.get(0, 65536);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 1);
    assert!(free_list.is_empty());

    let elem = Element::new(0, 0);
    free_list.push_back(fbl::UniquePtr::try_new(elem).unwrap());

    assert_eq!(bitmap.set_no_alloc(1, 65535, free_list), Ok(()));
    // Check free list size is 2 after merge
    let mut count = 0;
    for _ in free_list.iter() {
        count += 1;
    }
    assert_eq!(count, 2);

    assert_eq!(bitmap.clear_no_alloc(0, 65536, free_list), Ok(()));
    let mut count = 0;
    for _ in free_list.iter() {
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
fn test_rle_set_out_of_order() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };
    assert_eq!(bitmap.set(0x64, 0x65), Ok(()));
    assert_eq!(bitmap.set(0x60, 0x61), Ok(()));
    assert_eq!(bitmap.num_ranges(), 2);
    assert_eq!(bitmap.num_bits(), 2);
    assert!(bitmap.get(0x64, 0x65).all_set);
    assert!(bitmap.get(0x60, 0x61).all_set);
}

fn verify_range(bitmap: &RleBitmap, bitoff: usize, bitmax: usize, min_val: usize, max_val: usize) {
    assert!(bitmap.get(bitoff, bitmax).all_set);
    assert_eq!(bitmap.find(false, min_val, max_val, bitoff - min_val), Ok(min_val));
    assert_eq!(bitmap.find(false, min_val, max_val, max_val - bitmax), Ok(bitmax));
    assert_eq!(bitmap.num_bits(), bitmax - bitoff);
}

fn verify_cleared(bitmap: &RleBitmap, min_val: usize, max_val: usize) {
    assert_eq!(bitmap.find(false, min_val, max_val, max_val - min_val), Ok(min_val));
    assert_eq!(bitmap.num_bits(), 0);
}

fn check_overlap(
    bitoff1: usize,
    bitmax1: usize,
    bitoff2: usize,
    bitmax2: usize,
    min_val: usize,
    max_val: usize,
) {
    assert!(bitoff1 >= min_val);
    assert!(bitoff2 >= min_val);
    assert!(bitmax1 <= max_val);
    assert!(bitmax2 <= max_val);

    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };
    let min_off = core::cmp::min(bitoff1, bitoff2);
    let max_max = core::cmp::max(bitmax1, bitmax2);
    assert_eq!(bitmap.set(bitoff1, bitmax1), Ok(()));
    assert_eq!(bitmap.set(bitoff2, bitmax2), Ok(()));
    verify_range(bitmap, min_off, max_max, min_val, max_val);
    assert_eq!(bitmap.clear(min_off, max_max), Ok(()));
    verify_cleared(bitmap, min_val, max_val);
}

#[test]
fn test_rle_set_overlap() {
    check_overlap(5, 6, 4, 5, 0, 100);
    check_overlap(3, 5, 1, 4, 0, 100);
    check_overlap(1, 6, 3, 5, 0, 100);
    check_overlap(20, 30, 10, 20, 0, 100);
    check_overlap(20, 30, 15, 25, 0, 100);
    check_overlap(10, 20, 15, 20, 0, 100);
    check_overlap(10, 20, 15, 25, 0, 100);
    check_overlap(10, 30, 15, 25, 0, 100);
    check_overlap(15, 25, 10, 30, 0, 100);
}

#[test]
fn test_rle_find_range() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<usize>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };

    assert_eq!(bitmap.set(5, 10), Ok(()));
    assert_eq!(bitmap.num_bits(), 5);

    // Find unset run before range
    assert_eq!(bitmap.find(false, 0, 15, 5), Ok(0));
    // Find unset run after range
    assert_eq!(bitmap.find(false, 1, 15, 5), Ok(10));
    // Unset range too large
    assert_eq!(bitmap.find(false, 0, 15, 6), Err(Status::NO_RESOURCES));
    // Find entire set range
    assert_eq!(bitmap.find(true, 0, 15, 5), Ok(5));
    // Find set run within range
    assert_eq!(bitmap.find(true, 6, 15, 3), Ok(6));
    // Set range too large
    assert_eq!(bitmap.find(true, 0, 15, 6), Err(Status::NO_RESOURCES));
    // Set range too large
    assert_eq!(bitmap.find(true, 0, 8, 4), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.set(20, 30), Ok(()));
    assert_eq!(bitmap.num_bits(), 15);
    // Find unset run after both ranges
    assert_eq!(bitmap.find(false, 0, 50, 11), Ok(30));
    // Unset range too large
    assert_eq!(bitmap.find(false, 0, 40, 11), Err(Status::NO_RESOURCES));
    // Find set run in first range
    assert_eq!(bitmap.find(true, 0, 50, 5), Ok(5));
    // Find set run in second range
    assert_eq!(bitmap.find(true, 0, 50, 7), Ok(20));
    // Find set run in second range
    assert_eq!(bitmap.find(true, 7, 50, 5), Ok(20));
    // Set range too large
    assert_eq!(bitmap.find(true, 0, 50, 11), Err(Status::NO_RESOURCES));
    // Set range too large
    assert_eq!(bitmap.find(true, 35, 50, 6), Err(Status::NO_RESOURCES));
}

#[test]
fn test_rle_different_offset_type() {
    stack_pin_init!(let bitmap_pinned = RleBitmapBase::<u32>::new());
    // SAFETY: We do not move the RleBitmapBase out of its pinned location during the test.
    let bitmap = unsafe { bitmap_pinned.as_mut().get_unchecked_mut() };
    assert_eq!(bitmap.set(5, 10), Ok(()));
    assert_eq!(bitmap.num_bits(), 5);
    assert_eq!(bitmap.clear(5, 10), Ok(()));
    assert_eq!(bitmap.num_bits(), 0);
    assert_eq!(bitmap.set(1000, u32::MAX), Ok(()));
    assert_eq!(bitmap.num_bits(), u32::MAX - 1000);
}
