// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sort::{partition_three_way, quick_sort};

#[test]
fn test_empty() {
    let mut data: [i32; 0] = [];
    quick_sort(&mut data, &i32::cmp);
    assert_eq!(data, []);
}

#[test]
fn test_single_element() {
    let mut data = [42];
    quick_sort(&mut data, &i32::cmp);
    assert_eq!(data, [42]);
}

#[test]
fn test_already_sorted() {
    let mut data = [1, 2, 3, 4, 5];
    quick_sort(&mut data, &i32::cmp);
    assert_eq!(data, [1, 2, 3, 4, 5]);
}

#[test]
fn test_reverse_sorted() {
    let mut data = [5, 4, 3, 2, 1];
    quick_sort(&mut data, &i32::cmp);
    assert_eq!(data, [1, 2, 3, 4, 5]);
}

#[test]
fn test_duplicates() {
    let mut data = [3, 1, 2, 1, 3, 2, 2];
    quick_sort(&mut data, &i32::cmp);
    assert_eq!(data, [1, 1, 2, 2, 2, 3, 3]);
}

#[test]
fn test_large_random() {
    // Simple LCG for deterministic pseudo-random numbers
    struct Lcg {
        state: u32,
    }
    impl Lcg {
        fn next(&mut self) -> i32 {
            self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
            (self.state % 1000) as i32
        }
    }

    let mut rng = Lcg { state: 12345 };
    let mut data = Vec::new();
    for _ in 0..1000 {
        data.push(rng.next());
    }

    let mut expected = data.clone();
    expected.sort_unstable();

    quick_sort(&mut data, &i32::cmp);
    assert_eq!(data, expected);
}

#[test]
fn test_partition_basic() {
    let mut data = [3, 1, 2, 4, 3, 3, 0];
    let original_pivot = data[data.len() / 2]; // data[3] is 4
    let (lt, gt) = partition_three_way(&mut data, &i32::cmp);

    for i in 0..lt {
        assert!(
            data[i] < original_pivot,
            "data[{}] = {} should be < {}",
            i,
            data[i],
            original_pivot
        );
    }
    for i in lt..gt {
        assert_eq!(
            data[i], original_pivot,
            "data[{}] = {} should be == {}",
            i, data[i], original_pivot
        );
    }
    for i in gt..data.len() {
        assert!(
            data[i] > original_pivot,
            "data[{}] = {} should be > {}",
            i,
            data[i],
            original_pivot
        );
    }
}

#[test]
fn test_partition_all_equal() {
    let mut data = [2, 2, 2, 2, 2];
    let (lt, gt) = partition_three_way(&mut data, &i32::cmp);
    assert_eq!(lt, 0);
    assert_eq!(gt, 5);
}

#[test]
fn test_partition_already_partitioned() {
    let mut data = [1, 1, 2, 2, 3, 3];
    let original_pivot = data[data.len() / 2]; // data[3] is 2
    let (lt, gt) = partition_three_way(&mut data, &i32::cmp);
    for i in 0..lt {
        assert!(data[i] < original_pivot);
    }
    for i in lt..gt {
        assert_eq!(data[i], original_pivot);
    }
    for i in gt..data.len() {
        assert!(data[i] > original_pivot);
    }
}
