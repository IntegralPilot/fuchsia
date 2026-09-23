// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::lsm_tree::types::{OrdLowerBound, OrdUpperBound};
use crate::serialized_types::serialized_key::{KeyDeserializer, KeySerializer, SerializeKey};
use crate::serialized_types::varint::Buffer;
use anyhow::Context as _;
use fprint::TypeFingerprint;
use serde::{Deserialize, Serialize};
use std::cmp::{max, min};
use std::hash::Hash;
use std::ops::Range;
use storage_units::BlockSize;

/// Minimum block size supported by Fxfs.
/// Extents must be aligned to block size and must be a multiple of this.
pub const MIN_BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;

/// We use serde's try_from attribute here to allow deserialization to fail if
/// an unaligned extent is encountered.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct SerdeExtent(Range<u64>);

impl TryFrom<SerdeExtent> for Extent {
    type Error = &'static str;

    fn try_from(val: SerdeExtent) -> Result<Self, Self::Error> {
        if !MIN_BLOCK_SIZE.is_aligned(&val.0) {
            return Err("Extent bounds must be aligned to MIN_BLOCK_SIZE");
        }
        Ok(Extent(val.0))
    }
}

/// Extent represents a physical or logical range of bytes, aligned to a 512-byte boundary.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, TypeFingerprint)]
#[serde(try_from = "SerdeExtent")]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct Extent(pub Range<u64>);

impl Extent {
    /// Returns the range of bytes common between this extent and |other|.
    pub fn overlap(&self, other: &Extent) -> Option<Range<u64>> {
        if self.end <= other.start || self.start >= other.end {
            None
        } else {
            Some(max(self.start, other.start)..min(self.end, other.end))
        }
    }

    /// Returns the search key for this extent; that is, a key which is <= this key under
    /// OrdUpperBound.
    /// This would be used when searching for an extent with |find| (when we want to find any
    /// overlapping extent, which could include extents that start earlier).
    /// For example, if the tree has extents 50..150 and 150..200 and we wish to read 100..200,
    /// we'd search for 100..101 which would set the iterator to 50..150.
    pub fn search_key(&self) -> Self {
        assert_ne!(self.start, self.end);
        Extent::search_key_from_offset(self.start)
    }

    /// Similar to previous, but from an offset.  Returns a search key that will find the first
    /// extent that touches offset..
    pub fn search_key_from_offset(offset: u64) -> Self {
        Self(offset..offset + MIN_BLOCK_SIZE)
    }

    /// Returns the merge key for this extent; that is, a key which is <= this extent and any other
    /// possibly overlapping or touching extent, under OrdUpperBound. This is used to set the hint
    /// for |merge_into|.
    ///
    /// For example, if the tree has extents 0..50, 50..150 and 150..200 and we wish to insert
    /// 100..150, we'd use a merge hint of 100..100 which would set the iterator to 50..150
    /// (the first element >= 100..100 under OrdUpperBound).
    pub fn key_for_merge_into(&self) -> Self {
        Self(self.start..self.start)
    }

    /// Returns an iterator over the Extent partitions which overlap this key (see `FuzzyHash`).
    pub fn fuzzy_hash_partition(&self) -> ExtentPartitionIterator {
        ExtentPartitionIterator {
            range: EXTENT_HASH_BUCKET_SIZE.align_down(self.start)
                ..EXTENT_HASH_BUCKET_SIZE.align_up(self.end).unwrap_or(u64::MAX),
        }
    }

    pub fn overlaps(&self, other: &Extent) -> bool {
        self.start < other.end && self.end > other.start
    }

    pub fn is_search_key(&self) -> bool {
        self.0.end == self.0.start + MIN_BLOCK_SIZE
    }
}

impl SerializeKey for Extent {
    #[inline]
    fn serialize_key_to<B: Buffer>(&self, serializer: &mut KeySerializer<'_, B>) {
        assert!(
            MIN_BLOCK_SIZE.is_aligned(&self.0),
            "Extent bounds must be aligned to MIN_BLOCK_SIZE"
        );
        assert!(self.0.start <= self.0.end, "Extent length cannot be negative");
        serializer.write_u64(self.0.end / MIN_BLOCK_SIZE);
        serializer.write_u64((self.0.end - self.0.start) / MIN_BLOCK_SIZE);
    }

    #[inline]
    fn deserialize_key_from(deserializer: &mut KeyDeserializer<'_>) -> Result<Self, anyhow::Error> {
        let end = deserializer
            .read_u64()?
            .checked_mul(MIN_BLOCK_SIZE.get())
            .ok_or(FxfsError::Inconsistent)
            .context("Overflow")?;
        let len_raw = deserializer.read_u64()?;
        if len_raw == 0 {
            return Err(FxfsError::Inconsistent).context("Zero-length extent");
        }
        let len = len_raw
            .checked_mul(MIN_BLOCK_SIZE.get())
            .ok_or(FxfsError::Inconsistent)
            .context("Overflow")?;
        let start = end.checked_sub(len).ok_or(FxfsError::Inconsistent).context("Underflow")?;
        Ok(Self(start..end))
    }
}

impl std::ops::Deref for Extent {
    type Target = Range<u64>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Extent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Range<u64>> for Extent {
    fn from(range: Range<u64>) -> Self {
        Self(range)
    }
}

impl From<Extent> for Range<u64> {
    fn from(key: Extent) -> Self {
        key.0
    }
}

impl<T: storage_units::BlockSizeSpec> storage_units::IsAligned<T> for &Extent {
    #[inline(always)]
    fn is_aligned(self, block_size: storage_units::GenericBlockSize<T>) -> bool {
        block_size.is_aligned(&self.0)
    }
}

impl<T: storage_units::BlockSizeSpec> storage_units::IsAligned<T> for Extent {
    #[inline(always)]
    fn is_aligned(self, block_size: storage_units::GenericBlockSize<T>) -> bool {
        block_size.is_aligned(&self.0)
    }
}

const EXTENT_HASH_BUCKET_SIZE: BlockSize = BlockSize::SIZE_1MIB;

pub struct ExtentPartitionIterator {
    range: Range<u64>,
}

impl Iterator for ExtentPartitionIterator {
    type Item = Range<u64>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.start >= self.range.end {
            None
        } else {
            let start = self.range.start;
            self.range.start = start.saturating_add(EXTENT_HASH_BUCKET_SIZE.get());
            let end = std::cmp::min(self.range.start, self.range.end);
            Some(start..end)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = if self.range.start >= self.range.end {
            0
        } else {
            let diff = self.range.end - self.range.start;
            let count = EXTENT_HASH_BUCKET_SIZE.align_up_to_blocks(diff);
            usize::try_from(count).unwrap_or(usize::MAX)
        };
        (len, Some(len))
    }
}

impl ExactSizeIterator for ExtentPartitionIterator {}

// OrdUpperBound compares the end of the extent first, breaking ties by comparing the start
// descending (i.e. shorter extents sort first). This matches the serialized (end, len) layout
// and allows search routines to find overlapping extents using search_key().
impl OrdUpperBound for Extent {
    fn cmp_upper_bound(&self, other: &Extent) -> std::cmp::Ordering {
        // The comparison uses the end of the range so that we can more easily do queries. Ties
        // are broken by comparing the range start descending to match (end, len) layout.
        // This prepares for key serialization where extents are encoded as (end, len) (enabling
        // cheap varint length encoding) so byte-wise serialized comparisons match cmp_upper_bound.
        //
        // Well-formed layer files never contain overlapping extents, so ties on `end` never
        // occur within a single layer, ensuring existing layer ordering is unaffected.
        self.end.cmp(&other.end).then(other.start.cmp(&self.start))
    }
}

impl OrdLowerBound for Extent {
    // Orders by the start of the range rather than the end. This is used exclusively by the
    // merger min-heap to stream keys out in left-to-right (lower-bound) order.
    fn cmp_lower_bound(&self, other: &Extent) -> std::cmp::Ordering {
        self.start.cmp(&other.start)
    }
}

impl Ord for Extent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // We expect cmp_upper_bound and cmp_lower_bound to be used mostly, but ObjectKey needs an
        // Ord method in order to compare other enum variants, and Transaction requires an ObjectKey
        // to implement Ord.
        self.start.cmp(&other.start).then(self.end.cmp(&other.end))
    }
}

impl PartialOrd for Extent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::{EXTENT_HASH_BUCKET_SIZE, Extent, MIN_BLOCK_SIZE};
    use crate::lsm_tree::types::{OrdLowerBound, OrdUpperBound};
    use crate::serialized_types::serialized_key::{KeyDeserializer, SerializeKey};
    use std::cmp::Ordering;

    #[test]
    fn test_extent_key_serialization() {
        let key = Extent(512..2048);
        let mut buf = Vec::new();

        // Serialize
        {
            let mut ser =
                crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, Some(0));
            key.serialize_key_to(&mut ser);
            ser.finalize().unwrap();
        }

        let (mut deser, length) = KeyDeserializer::new(&buf, Some(0)).unwrap();
        assert_eq!(length, buf.len());
        let decoded_key = Extent::deserialize_key_from(&mut deser).unwrap();

        assert_eq!(key, decoded_key);

        // Verify bytes:
        // end = 2048 / 512 = 4.
        // len = 1536 / 512 = 3.
        // Delta encoding applies to first field (end = 4). Base is 0. 4 - 0 = 4.
        // Second field is len = 3. Base is None (taken). So writes 3.
        // Buffer should be [2, 4, 3] with 1-byte varint length prefix (len = 2 bytes).
        assert_eq!(buf, vec![2, 4, 3]);
    }

    #[test]
    fn test_extent_key_deserialization_overflow() {
        let mut buf = Vec::new();
        {
            let mut ser =
                crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, None);
            ser.write_u64(u64::MAX);
            ser.write_u64(u64::MAX);
            ser.finalize().unwrap();
        }
        let (mut deser, length) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(length, buf.len());
        let result = Extent::deserialize_key_from(&mut deser);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Overflow");
    }

    #[test]
    fn test_extent_key_deserialization_underflow() {
        let mut buf = Vec::new();
        {
            let mut ser =
                crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, None);
            ser.write_u64(1); // end = 512
            ser.write_u64(2); // len = 1024 (len > end)
            ser.finalize().unwrap();
        }
        let (mut deser, length) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(length, buf.len());
        let result = Extent::deserialize_key_from(&mut deser);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Underflow");
    }

    #[test]
    #[should_panic(expected = "Extent bounds must be aligned to MIN_BLOCK_SIZE")]
    fn test_extent_key_serialization_unaligned_end_panics() {
        let key = Extent(1024..2049);
        let mut buf = Vec::new();
        let mut ser = crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, None);
        key.serialize_key_to(&mut ser);
    }

    #[test]
    #[should_panic(expected = "Extent bounds must be aligned to MIN_BLOCK_SIZE")]
    fn test_extent_key_serialization_unaligned_start_panics() {
        let key = Extent(1025..2048);
        let mut buf = Vec::new();
        let mut ser = crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, None);
        key.serialize_key_to(&mut ser);
    }

    #[test]
    fn test_extent_cmp() {
        let extent = Extent(100..150);
        assert_eq!(extent.cmp_upper_bound(&Extent(0..100)), Ordering::Greater);
        assert_eq!(extent.cmp_upper_bound(&Extent(0..110)), Ordering::Greater);
        assert_eq!(extent.cmp_upper_bound(&Extent(0..150)), Ordering::Less);
        assert_eq!(extent.cmp_upper_bound(&Extent(99..150)), Ordering::Less);
        assert_eq!(extent.cmp_upper_bound(&Extent(100..150)), Ordering::Equal);
        assert_eq!(extent.cmp_upper_bound(&Extent(0..151)), Ordering::Less);
        assert_eq!(extent.cmp_upper_bound(&Extent(100..151)), Ordering::Less);
        assert_eq!(extent.cmp_upper_bound(&Extent(150..1000)), Ordering::Less);
        assert_eq!(extent.cmp_upper_bound(&Extent(101..150)), Ordering::Greater);
    }

    #[test]
    fn test_extent_cmp_lower_bound() {
        let extent = Extent(100..150);
        assert_eq!(extent.cmp_lower_bound(&Extent(0..100)), Ordering::Greater);
        assert_eq!(extent.cmp_lower_bound(&Extent(0..110)), Ordering::Greater);
        assert_eq!(extent.cmp_lower_bound(&Extent(0..150)), Ordering::Greater);
        assert_eq!(extent.cmp_lower_bound(&Extent(0..1000)), Ordering::Greater);
        assert_eq!(extent.cmp_lower_bound(&Extent(99..1000)), Ordering::Greater);
        assert_eq!(extent.cmp_lower_bound(&Extent(100..150)), Ordering::Equal);
        // cmp_lower_bound does not check the upper bound of the range
        assert_eq!(extent.cmp_lower_bound(&Extent(100..1000)), Ordering::Equal);
        assert_eq!(extent.cmp_lower_bound(&Extent(101..102)), Ordering::Less);
    }

    #[test]
    fn test_extent_search_and_insertion_key() {
        let extent = Extent(MIN_BLOCK_SIZE.get()..3 * MIN_BLOCK_SIZE);
        assert!(!extent.is_search_key());
        assert_eq!(extent.search_key(), Extent(MIN_BLOCK_SIZE.get()..2 * MIN_BLOCK_SIZE));
        assert!(extent.search_key().is_search_key());
        assert_eq!(extent.cmp_lower_bound(&extent.search_key()), Ordering::Equal);
        assert_eq!(extent.cmp_upper_bound(&extent.search_key()), Ordering::Greater);
        assert_eq!(extent.key_for_merge_into(), Extent(MIN_BLOCK_SIZE.get()..MIN_BLOCK_SIZE.get()));
        assert_eq!(extent.cmp_lower_bound(&extent.key_for_merge_into()), Ordering::Equal);
        assert_eq!(extent.cmp_upper_bound(&extent.key_for_merge_into()), Ordering::Greater);

        // A search key must always be <= the key it came from under OrdUpperBound.
        let extent = Extent(MIN_BLOCK_SIZE.get()..2 * MIN_BLOCK_SIZE);
        assert!(extent.is_search_key());
        assert_eq!(extent.search_key(), Extent(MIN_BLOCK_SIZE.get()..2 * MIN_BLOCK_SIZE));
        assert_eq!(extent.cmp_lower_bound(&extent.search_key()), Ordering::Equal);
        assert_eq!(extent.cmp_upper_bound(&extent.search_key()), Ordering::Equal);
    }

    #[test]
    fn test_extent_cmp_same_end_descending_start() {
        // If ends are identical, a higher start offset (shorter len) sorts BEFORE
        // a lower start offset (longer len) to match the (end, len) serialization layout.
        let short_extent = Extent(100 * MIN_BLOCK_SIZE..200 * MIN_BLOCK_SIZE);
        let long_extent = Extent(50 * MIN_BLOCK_SIZE..200 * MIN_BLOCK_SIZE);
        assert_eq!(short_extent.cmp_upper_bound(&long_extent), Ordering::Less);
        assert_eq!(long_extent.cmp_upper_bound(&short_extent), Ordering::Greater);
    }

    #[test]
    fn test_extent_serialization_compatibility() {
        let extent = Extent(50 * 512..200 * 512);

        let mut buf = Vec::new();
        {
            let mut ser =
                crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, None);
            extent.serialize_key_to(&mut ser);
            ser.finalize().unwrap();
        }

        let (mut deser, length) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(length, buf.len());
        let decoded = Extent::deserialize_key_from(&mut deser).unwrap();
        assert_eq!(extent, decoded);
    }

    #[test]
    fn test_extent_partition_iterator_len() {
        let mut iter = Extent(0..0).fuzzy_hash_partition();
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.next(), None);

        let mut iter = Extent(0..MIN_BLOCK_SIZE.get()).fuzzy_hash_partition();
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));
        assert_eq!(iter.next(), Some(0..EXTENT_HASH_BUCKET_SIZE.get()));
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.next(), None);

        let mut iter = Extent(0..3 * EXTENT_HASH_BUCKET_SIZE).fuzzy_hash_partition();
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert!(iter.next().is_some());
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert!(iter.next().is_some());
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));
        assert!(iter.next().is_some());
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_extent_key_serialization_zero_length() {
        let key = Extent(2 * MIN_BLOCK_SIZE..2 * MIN_BLOCK_SIZE);
        let mut buf = Vec::new();
        key.serialize_key_into(&mut buf, None).unwrap();
        let (mut deser, length) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(length, buf.len());
        let result = Extent::deserialize_key_from(&mut deser);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Zero-length extent");
    }

    #[test]
    #[should_panic(expected = "Extent length cannot be negative")]
    #[allow(clippy::reversed_empty_ranges)]
    fn test_extent_key_serialization_inverted_panics() {
        let key = Extent(2 * MIN_BLOCK_SIZE..MIN_BLOCK_SIZE.get());
        let mut buf = Vec::new();
        let mut ser = crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, None);
        key.serialize_key_to(&mut ser);
    }

    #[test]
    fn test_extent_key_deserialization_zero_length_fails() {
        let mut buf = Vec::new();
        {
            let mut ser =
                crate::serialized_types::serialized_key::KeySerializer::new(&mut buf, None);
            ser.write_u64(4); // end = 2048 (4 * 512)
            ser.write_u64(0); // len = 0
            ser.finalize().unwrap();
        }
        let (mut deser, length) = KeyDeserializer::new(&buf, None).unwrap();
        assert_eq!(length, buf.len());
        let result = Extent::deserialize_key_from(&mut deser);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Zero-length extent");
    }

    #[test]
    fn test_extent_serde_deserialization_unaligned_fails() {
        use bincode::Options as _;
        let options = bincode::DefaultOptions::new().allow_trailing_bytes();

        let unaligned_start_bytes = options.serialize(&(1u64..1024u64)).unwrap();
        assert!(options.deserialize::<Extent>(&unaligned_start_bytes).is_err());

        let unaligned_end_bytes = options.serialize(&(0u64..1000u64)).unwrap();
        assert!(options.deserialize::<Extent>(&unaligned_end_bytes).is_err());

        let aligned_bytes = options.serialize(&(512u64..1024u64)).unwrap();
        assert_eq!(options.deserialize::<Extent>(&aligned_bytes).unwrap(), Extent(512..1024));
    }
}
