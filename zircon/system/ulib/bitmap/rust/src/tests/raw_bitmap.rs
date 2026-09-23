// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::*;
use zx_status::Status;

fn test_initialized_empty<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(0), Ok(()));
    assert_eq!(bitmap.size(), 0);

    assert!(bitmap.get_one(0));
    assert_eq!(bitmap.set_one(0), Err(Status::INVALID_ARGS));
    assert_eq!(bitmap.clear_one(0), Err(Status::INVALID_ARGS));

    assert_eq!(bitmap.reset(1), Ok(()));
    assert!(!bitmap.get_one(0));
    assert_eq!(bitmap.set_one(0), Ok(()));
    assert!(bitmap.get_one(0));
    assert_eq!(bitmap.clear_one(0), Ok(()));
    assert!(!bitmap.get_one(0));
}

#[test]
fn test_raw_initialized_empty_default() {
    test_initialized_empty(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_initialized_empty_vmo() {
    test_initialized_empty(VmoStorage::new());
}

fn test_single_bit<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert!(!bitmap.get_one(2));

    assert_eq!(bitmap.set_one(2), Ok(()));
    assert!(bitmap.get_one(2));

    assert_eq!(bitmap.clear_one(2), Ok(()));
    assert!(!bitmap.get_one(2));
}

#[test]
fn test_raw_single_bit_default() {
    test_single_bit(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_single_bit_vmo() {
    test_single_bit(VmoStorage::new());
}

fn test_set_twice<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert_eq!(bitmap.set_one(2), Ok(()));
    assert!(bitmap.get_one(2));

    assert_eq!(bitmap.set_one(2), Ok(()));
    assert!(bitmap.get_one(2));
}

#[test]
fn test_raw_set_twice_default() {
    test_set_twice(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_set_twice_vmo() {
    test_set_twice(VmoStorage::new());
}

fn test_clear_twice<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert_eq!(bitmap.set_one(2), Ok(()));

    assert_eq!(bitmap.clear_one(2), Ok(()));
    assert!(!bitmap.get_one(2));

    assert_eq!(bitmap.clear_one(2), Ok(()));
    assert!(!bitmap.get_one(2));
}

#[test]
fn test_raw_clear_twice_default() {
    test_clear_twice(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_clear_twice_vmo() {
    test_clear_twice(VmoStorage::new());
}

fn test_get_return_arg<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

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

    assert_eq!(bitmap.set_one(3), Ok(()));
    let res = bitmap.get(2, 5);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 4);
}

#[test]
fn test_raw_get_return_arg_default() {
    test_get_return_arg(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_get_return_arg_vmo() {
    test_get_return_arg(VmoStorage::new());
}

fn test_set_range<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert_eq!(bitmap.set(2, 100), Ok(()));

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

    let first_unset = bitmap.scan(0, 100, true);
    assert_eq!(first_unset, Some(0));

    let last_unset = bitmap.reverse_scan(0, 100, true);
    assert_eq!(last_unset, Some(1));

    let first_set = bitmap.scan(0, 100, false);
    assert_eq!(first_set, Some(2));

    let last_set = bitmap.reverse_scan(0, 100, false);
    assert_eq!(last_set, Some(99));

    assert_eq!(bitmap.scan(2, 100, true), None);
    assert_eq!(bitmap.reverse_scan(2, 100, true), None);

    let first_unset = bitmap.scan(2, 100, false);
    assert_eq!(first_unset, Some(2));

    let last_unset = bitmap.reverse_scan(2, 100, false);
    assert_eq!(last_unset, Some(99));

    assert_eq!(bitmap.scan(50, 80, true), None);
    assert_eq!(bitmap.reverse_scan(50, 80, true), None);

    assert_eq!(bitmap.scan(100, 200, false), None);
    assert_eq!(bitmap.reverse_scan(100, 200, false), None);
}

#[test]
fn test_raw_set_range_default() {
    test_set_range(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_set_range_vmo() {
    test_set_range(VmoStorage::new());
}

fn test_find_simple<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    // Invalid finds
    assert_eq!(bitmap.find(false, 0, 0, 1), Err(Status::INVALID_ARGS));
    assert_eq!(bitmap.reverse_find(false, 0, 0, 1), Err(Status::INVALID_ARGS));
    assert_eq!(bitmap.find(false, 1, 0, 1), Err(Status::INVALID_ARGS));
    assert_eq!(bitmap.reverse_find(false, 1, 0, 1), Err(Status::INVALID_ARGS));

    // Finds from offset zero
    assert_eq!(bitmap.find(false, 0, 100, 1), Ok(0));
    assert_eq!(bitmap.reverse_find(false, 0, 100, 1), Ok(99));

    assert_eq!(bitmap.find(true, 0, 100, 1), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(true, 0, 100, 1), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(false, 0, 100, 5), Ok(0));
    assert_eq!(bitmap.reverse_find(false, 0, 100, 5), Ok(95));

    assert_eq!(bitmap.find(true, 0, 100, 5), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(true, 0, 100, 5), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(false, 0, 100, 100), Ok(0));
    assert_eq!(bitmap.reverse_find(false, 0, 100, 100), Ok(0));

    assert_eq!(bitmap.find(true, 0, 100, 100), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(true, 0, 100, 100), Err(Status::NO_RESOURCES));

    // Finds at an offset
    assert_eq!(bitmap.find(false, 50, 100, 3), Ok(50));
    assert_eq!(bitmap.reverse_find(false, 50, 100, 3), Ok(97));

    assert_eq!(bitmap.find(true, 50, 100, 3), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(true, 50, 100, 3), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(false, 90, 100, 10), Ok(90));
    assert_eq!(bitmap.reverse_find(false, 90, 100, 10), Ok(90));

    // Invalid scans (no space)
    assert_eq!(bitmap.find(false, 0, 100, 101), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 0, 100, 101), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.find(false, 91, 100, 10), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 91, 100, 10), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.find(false, 90, 100, 11), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 90, 100, 11), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.find(false, 90, 95, 6), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 90, 95, 6), Err(Status::NO_RESOURCES));

    // Fill the bitmap partly
    assert_eq!(bitmap.set(5, 10), Ok(()));
    assert_eq!(bitmap.set(20, 30), Ok(()));
    assert_eq!(bitmap.set(32, 35), Ok(()));
    assert_eq!(bitmap.set(90, 95), Ok(()));
    assert_eq!(bitmap.set(70, 80), Ok(()));
    assert_eq!(bitmap.set(65, 68), Ok(()));

    assert_eq!(bitmap.find(false, 0, 50, 5), Ok(0));
    assert_eq!(bitmap.reverse_find(false, 50, 100, 5), Ok(95));

    assert_eq!(bitmap.find(false, 0, 50, 10), Ok(10));
    assert_eq!(bitmap.reverse_find(false, 50, 100, 10), Ok(80));

    assert_eq!(bitmap.find(false, 0, 50, 15), Ok(35));
    assert_eq!(bitmap.reverse_find(false, 50, 100, 15), Ok(50));

    assert_eq!(bitmap.find(false, 0, 50, 16), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 50, 100, 16), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(false, 5, 20, 10), Ok(10));
    assert_eq!(bitmap.reverse_find(false, 80, 95, 10), Ok(80));

    assert_eq!(bitmap.find(false, 5, 25, 10), Ok(10));
    assert_eq!(bitmap.reverse_find(false, 75, 95, 10), Ok(80));

    assert_eq!(bitmap.find(false, 5, 15, 6), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 85, 95, 6), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(true, 0, 15, 2), Ok(5));
    assert_eq!(bitmap.reverse_find(true, 85, 100, 2), Ok(93));

    assert_eq!(bitmap.find(true, 0, 15, 6), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(true, 85, 100, 6), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(false, 32, 35, 3), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 65, 68, 3), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(false, 32, 35, 4), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 65, 68, 4), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(true, 32, 35, 4), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(true, 65, 68, 4), Err(Status::NO_RESOURCES));

    // Fill the whole bitmap
    assert_eq!(bitmap.set(0, 128), Ok(()));

    assert_eq!(bitmap.find(false, 0, 1, 1), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 0, 1, 1), Err(Status::NO_RESOURCES));

    assert_eq!(bitmap.find(false, 0, 128, 1), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.reverse_find(false, 0, 128, 1), Err(Status::NO_RESOURCES));
}

#[test]
fn test_raw_find_simple_default() {
    test_find_simple(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_find_simple_vmo() {
    test_find_simple(VmoStorage::new());
}

fn test_clear_all<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert_eq!(bitmap.set(0, 100), Ok(()));

    bitmap.clear_all();

    let res = bitmap.get(2, 100);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 2);

    assert_eq!(bitmap.set(0, 99), Ok(()));
    let res = bitmap.get(0, 100);
    assert!(!res.all_set);
    assert_eq!(res.first_unset, 99);
}

#[test]
fn test_raw_clear_all_default() {
    test_clear_all(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_clear_all_vmo() {
    test_clear_all(VmoStorage::new());
}

fn test_clear_subrange<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert_eq!(bitmap.set(2, 100), Ok(()));
    assert_eq!(bitmap.clear(50, 80), Ok(()));

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
}

#[test]
fn test_raw_clear_subrange_default() {
    test_clear_subrange(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_clear_subrange_vmo() {
    test_clear_subrange(VmoStorage::new());
}

fn test_boundary_arguments<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

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
fn test_raw_boundary_arguments_default() {
    test_boundary_arguments(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_boundary_arguments_vmo() {
    test_boundary_arguments(VmoStorage::new());
}

fn test_set_out_of_order<S: Storage>(storage: S) {
    let mut bitmap = RawBitmapGeneric::new(storage);
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert_eq!(bitmap.set_one(0x64), Ok(()));
    assert_eq!(bitmap.set_one(0x60), Ok(()));

    assert!(bitmap.get_one(0x64));
    assert!(bitmap.get_one(0x60));
}

#[test]
fn test_raw_set_out_of_order_default() {
    test_set_out_of_order(DefaultStorage::new());
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_set_out_of_order_vmo() {
    test_set_out_of_order(VmoStorage::new());
}

#[test]
fn test_raw_move_construct_and_assign() {
    let mut src = RawBitmapGeneric::new(DefaultStorage::new());
    assert_eq!(src.reset(128), Ok(()));
    assert_eq!(src.set_one(0x64), Ok(()));
    assert!(src.get_one(0x64));

    let target = src;
    assert!(target.get_one(0x64));
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_raw_move_construct_and_assign_vmo() {
    let mut src = RawBitmapGeneric::new(VmoStorage::new());
    assert_eq!(src.reset(128), Ok(()));
    assert_eq!(src.set_one(0x64), Ok(()));
    assert!(src.get_one(0x64));

    let target = src;
    assert!(target.get_one(0x64));
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_grow_across_page() {
    let mut bitmap = RawBitmapGeneric::new(VmoStorage::new());
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert!(!bitmap.get_one(100));
    assert_eq!(bitmap.set_one(100), Ok(()));
    assert!(bitmap.get_one(100));

    assert_eq!(bitmap.find(true, 101, 128, 1), Err(Status::NO_RESOURCES));

    let page_size = zx::system_get_page_size() as usize;
    assert_ne!(bitmap.set_one(16 * page_size - 1), Ok(()));

    assert_eq!(bitmap.grow(16 * page_size), Ok(()));
    assert_eq!(bitmap.find(true, 101, 16 * page_size, 1), Err(Status::NO_RESOURCES));

    assert!(!bitmap.get_one(16 * page_size - 1));
    assert_eq!(bitmap.set_one(16 * page_size - 1), Ok(()));
    assert!(bitmap.get_one(16 * page_size - 1));

    assert!(bitmap.get_one(100));

    assert_eq!(bitmap.shrink(99), Ok(()));
    assert_eq!(bitmap.grow(16 * page_size), Ok(()));
    assert!(!bitmap.get_one(100));
    assert!(!bitmap.get_one(16 * page_size - 1));
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
#[test]
fn test_grow_shrink() {
    let mut bitmap = RawBitmapGeneric::new(VmoStorage::new());
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert!(!bitmap.get_one(100));
    assert_eq!(bitmap.set_one(100), Ok(()));
    assert!(bitmap.get_one(100));

    for i in 8..16 {
        for j in -16..=16 {
            let bitmap_size = ((1 << i) + j) as usize;

            for shrink_len in 1..32 {
                assert_eq!(bitmap.reset(bitmap_size), Ok(()));
                assert_eq!(bitmap.size(), bitmap_size);

                assert!(!bitmap.get_one(bitmap_size - shrink_len));
                assert_eq!(bitmap.set_one(bitmap_size - shrink_len), Ok(()));
                assert!(bitmap.get_one(bitmap_size - shrink_len));

                assert!(!bitmap.get_one(bitmap_size - shrink_len - 1));
                assert_eq!(bitmap.set_one(bitmap_size - shrink_len - 1), Ok(()));
                assert!(bitmap.get_one(bitmap_size - shrink_len - 1));

                assert_eq!(bitmap.shrink(bitmap_size - shrink_len), Ok(()));
                assert_eq!(bitmap.grow(bitmap_size), Ok(()));

                assert!(!bitmap.get_one(bitmap_size - shrink_len));
                assert!(bitmap.get_one(bitmap_size - shrink_len - 1));

                assert_eq!(
                    bitmap.find(true, bitmap_size - shrink_len, bitmap_size, 1),
                    Err(Status::NO_RESOURCES)
                );
            }
        }
    }
}

#[test]
fn test_grow_failure() {
    let mut bitmap = RawBitmapGeneric::new(DefaultStorage::new());
    assert_eq!(bitmap.reset(128), Ok(()));
    assert_eq!(bitmap.size(), 128);

    assert_eq!(bitmap.grow(64), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.grow(128), Err(Status::NO_RESOURCES));
    assert_eq!(bitmap.grow(128 + 1), Err(Status::NO_RESOURCES));
}
