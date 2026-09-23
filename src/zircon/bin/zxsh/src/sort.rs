// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cmp::Ordering;

/// Sorts the slice in-place using a custom quicksort algorithm.
///
/// This implementation uses significantly less size in the binary than the implementation in the
/// Rust standard library.
pub fn quick_sort<T>(mut slice: &mut [T], compare: &impl Fn(&T, &T) -> Ordering) {
    while slice.len() > 1 {
        let (left_idx, right_idx) = partition_three_way(slice, compare);

        let left_len = left_idx;
        let right_len = slice.len() - right_idx;

        if left_len < right_len {
            // Recurse on the left partition (which is smaller)
            let (left, rest) = slice.split_at_mut(left_idx);
            quick_sort(left, compare);
            // Loop on the right partition
            let (_, right) = rest.split_at_mut(right_idx - left_idx);
            slice = right;
        } else {
            // Recurse on the right partition (which is smaller)
            let (left, rest) = slice.split_at_mut(left_idx);
            let (_, right) = rest.split_at_mut(right_idx - left_idx);
            quick_sort(right, compare);
            // Loop on the left partition
            slice = left;
        }
    }
}

pub fn partition_three_way<T>(
    slice: &mut [T],
    compare: &impl Fn(&T, &T) -> Ordering,
) -> (usize, usize) {
    let len = slice.len();
    let pivot_idx = len / 2;
    slice.swap(0, pivot_idx);

    let mut lt = 0;
    let mut i = 1;
    let mut gt = len - 1;

    while i <= gt {
        match compare(&slice[i], &slice[lt]) {
            Ordering::Less => {
                slice.swap(lt, i);
                lt += 1;
                i += 1;
            }
            Ordering::Greater => {
                slice.swap(i, gt);
                gt -= 1;
            }
            Ordering::Equal => {
                i += 1;
            }
        }
    }
    (lt, gt + 1)
}
