// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cmp::min;
use std::sync::atomic::{AtomicU64, Ordering};

const BITS: u64 = u64::BITS as u64;

/// An atomic bit-vector.
pub struct AtomicBitVec {
    storage: Box<[AtomicU64]>,
    nbits: u64,
}

impl AtomicBitVec {
    /// Creates a new `AtomicBitVec` with all bits set to false.
    pub fn new(nbits: u64) -> Self {
        let nwords = nbits.div_ceil(BITS);
        let storage = (0..nwords).map(|_| AtomicU64::new(0)).collect();
        Self { storage, nbits }
    }

    /// Sets the bits between `start_bit` (included) and `end_bit` (excluded), and returns the
    /// number of bits that were already set.
    pub fn test_and_set_range(&self, start_bit: u64, end_bit: u64) -> u64 {
        assert!(start_bit < end_bit);
        assert!(end_bit <= self.nbits);

        let mut counter = 0;
        let mut current_bit = start_bit;

        while current_bit < end_bit {
            let current_word_index = current_bit / BITS;
            let current_word_start_bit = current_word_index * BITS;
            let mask =
                Self::get_mask(current_bit % BITS, min(end_bit - current_word_start_bit, BITS));
            let old_word =
                &self.storage[current_word_index as usize].fetch_or(mask, Ordering::Relaxed);
            counter += (old_word & mask).count_ones();
            current_bit = current_word_start_bit + BITS;
        }

        counter.into()
    }

    pub fn len(&self) -> u64 {
        self.nbits
    }

    pub fn get(&self) -> Vec<bool> {
        let fetch = |bit: u64| {
            let word = bit / BITS;
            let bit_mask = 1 << (bit % BITS);
            self.storage[word as usize].fetch_or(0, Ordering::Relaxed) & bit_mask != 0
        };
        (0..self.nbits).map(fetch).collect()
    }

    fn get_mask(start_bit: u64, end_bit: u64) -> u64 {
        let left_mask = u64::MAX << start_bit;
        let right_mask = u64::MAX >> (BITS - end_bit);
        left_mask & right_mask
    }
}

impl Clone for AtomicBitVec {
    fn clone(&self) -> Self {
        let new_storage =
            self.storage.iter().map(|a| AtomicU64::new(a.load(Ordering::Relaxed))).collect();
        Self { storage: new_storage, nbits: self.nbits }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let vec = AtomicBitVec::new(0);
        assert_eq!(vec.nbits, 0);
        assert_eq!(vec.storage.len(), 0);

        let vec = AtomicBitVec::new(1);
        assert_eq!(vec.nbits, 1);
        assert_eq!(vec.storage.len(), 1);
        assert_eq!(vec.storage[0].load(Ordering::Relaxed), 0);

        let vec = AtomicBitVec::new(64);
        assert_eq!(vec.nbits, 64);
        assert_eq!(vec.storage.len(), 1);
        assert_eq!(vec.storage[0].load(Ordering::Relaxed), 0);

        let vec = AtomicBitVec::new(65);
        assert_eq!(vec.nbits, 65);
        assert_eq!(vec.storage.len(), 2);
        assert_eq!(vec.storage[0].load(Ordering::Relaxed), 0);
        assert_eq!(vec.storage[1].load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_test_and_set() {
        let vec = AtomicBitVec::new(320);

        // Set bits and check they were not set before.
        assert_eq!(vec.test_and_set_range(10, 20), 0);
        // Check they are set now.
        assert_eq!(vec.test_and_set_range(10, 20), 10);

        // Check another range, partially overlapping.
        assert_eq!(vec.test_and_set_range(15, 25), 5);

        // A range across two words.
        assert_eq!(vec.test_and_set_range(60, 70), 0);
        // Only 10 more.
        assert_eq!(vec.test_and_set_range(55, 75), 10);

        // Large range.
        assert_eq!(vec.test_and_set_range(50, 300), 20);
        assert_eq!(vec.test_and_set_range(50, 300), 250);
    }

    #[test]
    fn test_test_and_set2() {
        let vec = AtomicBitVec::new(320);
        assert_eq!(vec.test_and_set_range(64, 128), 0);
        assert_eq!(vec.test_and_set_range(64, 128), 64);
    }

    #[test]
    fn test_test_and_set3() {
        let vec = AtomicBitVec::new(150);

        assert_eq!(vec.test_and_set_range(0, 1), 0);
        assert_eq!(vec.test_and_set_range(63, 64), 0);
        // 0 and 63 are already set.
        assert_eq!(vec.test_and_set_range(0, 64), 2);
        assert_eq!(vec.test_and_set_range(64, 65), 0);
        // 63 and 64 are already set.
        assert_eq!(vec.test_and_set_range(63, 65), 2);

        // 63 and 64 are already set.
        assert_eq!(vec.test_and_set_range(63, 128), 2);
        // 63 to 127 (included) are already set.
        assert_eq!(vec.test_and_set_range(63, 129), 65);
    }

    #[test]
    #[should_panic]
    fn test_and_set_out_of_bounds() {
        let vec = AtomicBitVec::new(10);
        vec.test_and_set_range(10, 20);
    }

    #[test]
    fn test_clone() {
        let vec = AtomicBitVec::new(100);
        vec.test_and_set_range(10, 20);
        vec.test_and_set_range(50, 60);

        let clone = vec.clone();
        assert_eq!(clone.nbits, vec.nbits);
        assert_eq!(clone.test_and_set_range(10, 20), 10);
        assert_eq!(clone.test_and_set_range(50, 60), 10);
        assert_eq!(clone.test_and_set_range(20, 30), 0);

        // Check the original is unaffected.
        assert_eq!(vec.test_and_set_range(20, 30), 0);
    }
}
