// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::lsm_tree::types::{FuzzyHash, MaybeContainsKey};
use anyhow::{Error, Result};
use bit_vec::BitVec;
use std::marker::PhantomData;
use storage_ptr_slice::PtrByteSlice;

/// A bloom filter provides a probabilistic means of determining if a layer file *might* contain
/// records related to a given key.
///
/// A bloom filter is a bitmap with two associated constants `m` and `k`.  The bitmap is `m` bits
/// wide, and there are `k` associated hash functions with the filter.
///
/// There are two operations for a bloom filter:
/// - Adding a new entry.  Each of the `k` hash functions is computed for the entry, and this hash
///   corresponds to a single bit in the bitmap.  All `k` of these bits are set to 1.
/// - Querying for an entry.  Each of the `k` hash functions is computed for the entry, and if any
///   of the corresponding bits in the bitmap are set to 0, the bloom filter returns false, which
///   means the layer file definitely does not contain records for that key.  Note that false
///   positives are possible by design; all bits may have been set by unrelated entries, in which
///   case the query returns true.
///
/// # Efficiency & Tuning
///
/// As mentioned above, there are two parameters `k` and `m` which we can tune to optimize the
/// BloomFilter.  There are three factors to consider:
/// - How much space we use for the bloom filter (i.e. `m`).
/// - How efficient the operations are (i.e. `k`, since we have to compute `k` hashes for each
///   insert/query).
/// - Minimizing false positive rate.  Increasing `k` or `m` will both reduce the false positive
///   rate.
///
/// For simplicity we fix a target false-positive rate and derive `k` and `m`.  See estimate_params.

// Returns a tuple (bloom_filter_size_bits, num_hashes).  The bloom filter will always be sized up
// to the nearest power-of-two, which makes its usage later a bit more efficient (since we take
// indexes modulo the size of the filter).
// We target a false positive rate of 0.1%, from which we can derive the constants via relationships
// described in https://en.wikipedia.org/wiki/Bloom_filter#Optimal_number_of_hash_functions.
fn estimate_params(num_items: usize) -> (usize, usize) {
    if num_items == 0 {
        return (0, 0);
    }
    const TARGET_FP_RATE: f64 = 0.001;
    use std::f64::consts::LN_2;

    let n = num_items as f64;
    let bits = ((n * TARGET_FP_RATE.ln()) / (-LN_2 * LN_2)).ceil() as u64;
    // Round up to a power-of-two number of bytes.
    let bits = std::cmp::max(bits, 8).next_power_of_two();
    let m = bits as f64;

    // Then, solve for a minimal `k` such that the probability of FP is <= TARGET_FP_RATE.
    let mut k: f64 = 0.0;
    let mut fp_rate = 1.0;
    while fp_rate > TARGET_FP_RATE {
        k += 1.0;
        fp_rate = (1f64 - (-k / (m / n)).exp()).powf(k);
    }
    (bits as usize, k.round() as usize)
}

/// A read-only handle to a bloom filter.  To create a bloom filter, use `BloomFilterWriter`.
/// Note that the bloom filter is *not* versioned; this must be managed by the caller.
pub struct BloomFilterReader<V> {
    data: BitVec,
    seed: u64,
    num_hashes: usize,
    _type: PhantomData<V>,
}

pub struct BloomFilterStats {
    pub size: usize,
    pub num_hashes: usize,
    // The percentage (rounded up to the nearest whole) of the bits which are set.
    pub fill_percentage: usize,
}

/// The maximum number of fuzzy hash partitions that will be checked in a bloom filter query before
/// falling back to returning true.
pub const MAX_HASH_PARTITIONS: usize = 4;

impl<V: FuzzyHash> BloomFilterReader<V> {
    /// Creates a BloomFilterReader by reading the serialized contents from `buf`.
    /// `seed` and `num_hashes` must match the values passed into BloomFilterWriter.
    pub fn read(buf: PtrByteSlice<'_>, seed: u64, num_hashes: usize) -> Result<Self, Error> {
        Ok(Self {
            data: BitVec::from_bytes(&buf.to_vec()),
            seed,
            num_hashes,
            _type: PhantomData::default(),
        })
    }

    /// Returns whether the bloom filter *might* contain the given value (or any part of it, for
    /// range-based keys).
    pub fn maybe_contains(&self, value: &V) -> MaybeContainsKey {
        let hashes = value.fuzzy_hash();
        if hashes.len() > MAX_HASH_PARTITIONS {
            return MaybeContainsKey::RangeKeyTooLarge;
        }
        for hash in hashes {
            if self.maybe_contains_inner(hash) {
                return MaybeContainsKey::Maybe;
            }
        }
        MaybeContainsKey::False
    }

    fn maybe_contains_inner(&self, initial_hash: u64) -> bool {
        for i in 0..self.num_hashes as u64 {
            let seed = self.seed.wrapping_add(i);
            let hash = crate::stable_hash::stable_hash_seeded(initial_hash, seed);
            let idx = hash as usize % self.data.len();
            if !self.data.get(idx).unwrap() {
                return false;
            }
        }
        true
    }

    /// Call sparingly; this is expensive to compute.
    /// Note that the return value can be trivially cached since the reader is immutable.
    pub fn stats(&self) -> BloomFilterStats {
        BloomFilterStats {
            size: self.data.len().div_ceil(8),
            num_hashes: self.num_hashes,
            fill_percentage: self.compute_fill_percentage(),
        }
    }

    // Use sparingly; this is expensive to compute.
    fn compute_fill_percentage(&self) -> usize {
        if self.data.is_empty() {
            return 0;
        }
        let count = self.data.blocks().map(|b| b.count_ones() as u64).sum::<u64>();
        ((100 * count).div_ceil(self.data.len() as u64)) as usize
    }

    #[cfg(test)]
    fn new_empty(num_items: usize) -> Self {
        let (bits, num_hashes) = estimate_params(num_items);
        Self {
            data: BitVec::from_elem(bits, false),
            seed: 0,
            num_hashes,
            _type: PhantomData::default(),
        }
    }

    #[cfg(test)]
    fn new_full(num_items: usize) -> Self {
        let (bits, num_hashes) = estimate_params(num_items);
        Self {
            data: BitVec::from_elem(bits, true),
            seed: 0,
            num_hashes,
            _type: PhantomData::default(),
        }
    }
}

/// A helper to build a bloom filter in-memory before serializing it.
/// Note that the bloom filter is *not* versioned; this must be managed by the caller.
pub struct BloomFilterWriter<V> {
    data: BitVec,
    seed: u64,
    num_hashes: usize,
    _type: PhantomData<V>,
}

impl<V: FuzzyHash> BloomFilterWriter<V> {
    /// Creates a new bloom filter suitable for the given input size.  See module comments for the
    /// heuristics used.
    /// `seed` is a value which is mixed into hashes in the bloom filter.  Note that it is not used
    /// in a secure manner so should not contain any secrets.  It should be unpredictable to prevent
    /// timing attacks.
    pub fn new(seed: u64, num_items: usize) -> Self {
        let (bits, num_hashes) = estimate_params(num_items);
        Self {
            data: BitVec::from_elem(bits, false),
            seed,
            num_hashes,
            _type: PhantomData::default(),
        }
    }

    /// Returns the size the bloom filter will occupy when serialized.
    pub fn serialized_size(&self) -> usize {
        self.data.len() / 8
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn num_hashes(&self) -> usize {
        self.num_hashes
    }

    pub fn write<W>(&self, writer: &mut W) -> Result<(), Error>
    where
        W: std::io::Write,
    {
        Ok(writer.write_all(&self.data.to_bytes()[..])?)
    }

    pub fn insert(&mut self, value: &V) {
        for hash in value.fuzzy_hash() {
            self.insert_inner(hash);
        }
    }

    fn insert_inner(&mut self, initial_hash: u64) {
        for i in 0..self.num_hashes as u64 {
            let seed = self.seed.wrapping_add(i);
            let hash = crate::stable_hash::stable_hash_seeded(initial_hash, seed);
            let idx = hash as usize % self.data.len();
            self.data.set(idx, true);
        }
    }

    // Clear the bits in the bloom filter. Intended to be used for testing, e.g., to create an
    // invalid bloom filter.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.data.fill(false);
    }
}

#[cfg(test)]
impl<T> From<BloomFilterWriter<T>> for BloomFilterReader<T> {
    fn from(writer: BloomFilterWriter<T>) -> Self {
        Self {
            data: writer.data,
            seed: writer.seed,
            num_hashes: writer.num_hashes,
            _type: PhantomData::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lsm_tree::bloom_filter::{BloomFilterReader, BloomFilterWriter, estimate_params};
    use crate::lsm_tree::types::MaybeContainsKey;
    use crate::object_store::allocator::AllocatorKey;
    use storage_ptr_slice::PtrByteSlice;

    #[test]
    fn estimated_params() {
        // Compare to https://hur.st/bloomfilter (keeping in mind the rounding-up of the size of the
        // bloom filter).
        assert_eq!(estimate_params(0), (0, 0));
        assert_eq!(estimate_params(1), (16, 6));
        assert_eq!(estimate_params(5000), (131072, 4));
        assert_eq!(estimate_params(50000), (1048576, 4));
        assert_eq!(estimate_params(5_000_000), (134217728, 4));
    }

    const TEST_KEYS: [i32; 4] = [0, 65535, i32::MAX, i32::MIN];

    #[test]
    fn test_empty() {
        let filter = BloomFilterReader::new_empty(TEST_KEYS.len());
        for key in &TEST_KEYS {
            assert_eq!(filter.maybe_contains(key), MaybeContainsKey::False);
        }
    }

    #[test]
    fn test_full() {
        let filter = BloomFilterReader::new_full(TEST_KEYS.len());
        for key in &TEST_KEYS {
            assert_eq!(filter.maybe_contains(key), MaybeContainsKey::Maybe);
        }
    }

    #[test]
    fn test_insert() {
        for key in &TEST_KEYS {
            // Use a new filter each time so we don't get false positives.
            let mut filter = BloomFilterWriter::new(0, TEST_KEYS.len());
            filter.insert(key);
            let filter = BloomFilterReader::from(filter);
            assert_eq!(filter.maybe_contains(key), MaybeContainsKey::Maybe);
        }
    }

    #[test]
    fn test_range_key() {
        let mut filter = BloomFilterWriter::new(0, 2);
        filter.insert(&AllocatorKey { device_range: (0..2097152).into() });
        filter.insert(&AllocatorKey { device_range: (4194304..4194816).into() });
        let filter = BloomFilterReader::from(filter);

        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (0..512).into() }),
            MaybeContainsKey::Maybe
        );
        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (2096640..2097152).into() }),
            MaybeContainsKey::Maybe
        );
        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (2097152..2097664).into() }),
            MaybeContainsKey::False
        );
        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (3145216..3145728).into() }),
            MaybeContainsKey::False
        );
        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (4193792..4194816).into() }),
            MaybeContainsKey::Maybe
        );

        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (0..2097664).into() }),
            MaybeContainsKey::Maybe
        );
        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (2097152..4194816).into() }),
            MaybeContainsKey::Maybe
        );
        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (104857600..104858112).into() }),
            MaybeContainsKey::False
        );
    }

    #[test]
    fn test_serde() {
        for key in &TEST_KEYS {
            // Use a new filter each time so we don't get false positives.
            let mut filter = BloomFilterWriter::new(0, 1);
            filter.insert(key);
            let num_hashes = filter.num_hashes();
            let mut buf = vec![];
            {
                let mut cursor = std::io::Cursor::new(&mut buf);
                filter.write(&mut cursor).expect("write failed");
            }
            let filter = BloomFilterReader::read(PtrByteSlice::from(&buf[..]), 0, num_hashes)
                .expect("read failed");
            assert_eq!(filter.maybe_contains(key), MaybeContainsKey::Maybe);
        }
    }

    #[test]
    fn test_excessive_partitions() {
        let mut filter = BloomFilterWriter::new(0, 1);
        filter.insert(&AllocatorKey { device_range: (0..512).into() });
        let filter = BloomFilterReader::from(filter);

        // A range with > MAX_HASH_PARTITIONS partitions (e.g. 5 MiB = 5 partitions) that does not
        // match should defensively return RangeKeyTooLarge without panicking.
        assert_eq!(
            filter.maybe_contains(&AllocatorKey { device_range: (104857600..110100480).into() }),
            MaybeContainsKey::RangeKeyTooLarge
        );
    }
}
