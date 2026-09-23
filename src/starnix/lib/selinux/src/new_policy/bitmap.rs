// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::marker::PhantomData;

use selinux_policy_derive::{Parse, Serialize};

use super::NewPolicy;
use super::error::{ParseError, SerializeError, ValidateError};
use super::parser::{Array, PolicyCursor, PolicyWriter};
use super::traits::{Parse, PolicyId, Serialize, Validate};

type MapNode = u64;

/// Number of bits represented by a single node in the extensible bitmap.
pub const MAP_NODE_BITS: u32 = MapNode::BITS;

#[inline(always)]
const fn node_start_bit(index: u32) -> u32 {
    index & !(MAP_NODE_BITS - 1)
}

#[inline(always)]
const fn node_bit_index(index: u32) -> u32 {
    index & (MAP_NODE_BITS - 1)
}

/// Binary map item in the extensible bitmap.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Parse, Serialize)]
pub(super) struct BinaryMapItem {
    start_bit: u32,
    map: MapNode,
}

impl Validate for BinaryMapItem {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        if self.map == 0 {
            return Err(ValidateError::InvalidExtensibleBitmapItem);
        }
        if self.start_bit & (MAP_NODE_BITS - 1) != 0 {
            return Err(ValidateError::MisalignedExtensibleBitmapItemStartBit {
                found_start_bit: self.start_bit,
                found_size: MAP_NODE_BITS,
            });
        }
        Ok(())
    }
}

/// Extensible bitmap for storing potentially large, sparse bitmaps.
///
/// This representation allows memory-efficient storage of sparse bitmaps while
/// providing efficient bit lookup, iteration, and round-trip serialization.
///
/// Under the hood, this is stored as a sorted [`Array`] of [`BinaryMapItem`]s.
#[derive(Parse, Serialize)]
struct BinaryExtensibleBitmapMetadata {
    map_item_size_bits: u32,
    high_bit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensibleBitmap {
    items: Array<BinaryMapItem>,
}

impl Parse for ExtensibleBitmap {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let meta = BinaryExtensibleBitmapMetadata::parse(cursor)?;
        if meta.map_item_size_bits != MAP_NODE_BITS {
            return Err(ParseError::InvalidExtensibleBitmapItemSize {
                found_size: meta.map_item_size_bits,
            });
        }
        let items = Array::<BinaryMapItem>::parse(cursor)?;
        let bitmap = Self { items };
        let calculated_high_bit = bitmap.high_bit();
        if meta.high_bit != calculated_high_bit {
            return Err(ParseError::InvalidExtensibleBitmapHighBit {
                expected: calculated_high_bit,
                found: meta.high_bit,
            });
        }
        Ok(bitmap)
    }
}

impl Serialize for ExtensibleBitmap {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let meta = BinaryExtensibleBitmapMetadata {
            map_item_size_bits: MAP_NODE_BITS,
            high_bit: self.high_bit(),
        };
        meta.serialize(writer)?;
        self.items.serialize(writer)?;
        Ok(())
    }
}

impl ExtensibleBitmap {
    /// Returns the high bit limit of this bitmap (rounded up to the next node boundary).
    #[inline]
    pub fn high_bit(&self) -> u32 {
        self.items.last().map_or(0, |item| item.start_bit + MAP_NODE_BITS)
    }

    /// Returns `true` if the `index`'th bit in this bitmap is a 1-bit.
    pub fn is_set(&self, index: u32) -> bool {
        if index >= self.high_bit() {
            return false;
        }
        let start_bit = node_start_bit(index);
        let bit_index = node_bit_index(index);

        match self.items.binary_search_by_key(&start_bit, |item| item.start_bit) {
            Ok(idx) => {
                let map = self.items[idx].map;
                (map & ((1 as MapNode) << bit_index)) != 0
            }
            Err(_) => false,
        }
    }

    /// Returns `true` if this bitmap is empty (no bits set).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns the number of set bits in this bitmap.
    pub fn count(&self) -> usize {
        self.items.iter().map(|item| item.map.count_ones() as usize).sum()
    }

    /// Returns an iterator that yields contiguous `(low, high)` ranges of set bit indices.
    pub fn spans(&self) -> BitSpansIter<'_> {
        let mut items = self.items.iter().peekable();
        let current = items.next().copied();
        BitSpansIter { items, current }
    }

    /// Returns an iterator that yields the indices of this bitmap's set bits.
    pub fn indices_of_set_bits(&self) -> BitIter<'_> {
        BitIter { spans: self.spans(), current: 1, high: 0 }
    }

    /// Returns `true` if all set bits in `self` are also set in `other`.
    ///
    /// This dedicated subset check is more efficient than `compare()` when only testing
    /// inclusion (`self <= other`), as it aborts immediately upon finding any bit in `self`
    /// that is missing from `other` without scanning the remainder or tracking superset status.
    pub fn is_subset(&self, other: &Self) -> bool {
        let mut other_iter = other.items.iter().peekable();
        for item in self.items.iter() {
            while let Some(o) = other_iter.peek() {
                if o.start_bit < item.start_bit {
                    other_iter.next();
                } else {
                    break;
                }
            }
            match other_iter.peek() {
                Some(o) if o.start_bit == item.start_bit => {
                    if (item.map & !o.map) != 0 {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Compares two bitmaps to determine their subset relationship.
    ///
    /// This single-pass comparison is more efficient for general comparisons where ordering or
    /// equality is needed, as it tracks both subset and superset relationships simultaneously
    /// in one traversal over the sorted items and aborts early if both become false (`None`).
    pub fn compare(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let mut self_iter = self.items.iter().peekable();
        let mut other_iter = other.items.iter().peekable();

        let mut is_subset = true;
        let mut is_superset = true;

        while is_subset || is_superset {
            match (self_iter.peek(), other_iter.peek()) {
                (Some(&self_item), Some(&other_item)) => {
                    if self_item.start_bit < other_item.start_bit {
                        is_subset = false;
                        self_iter.next();
                    } else if self_item.start_bit > other_item.start_bit {
                        is_superset = false;
                        other_iter.next();
                    } else {
                        let self_map = self_item.map;
                        let other_map = other_item.map;

                        if (self_map & !other_map) != 0 {
                            is_subset = false;
                        }
                        if (other_map & !self_map) != 0 {
                            is_superset = false;
                        }

                        self_iter.next();
                        other_iter.next();
                    }
                }
                (Some(_), None) => {
                    is_subset = false;
                    break;
                }
                (None, Some(_)) => {
                    is_superset = false;
                    break;
                }
                (None, None) => {
                    break;
                }
            }
        }

        match (is_subset, is_superset) {
            (true, true) => Some(std::cmp::Ordering::Equal),
            (true, false) => Some(std::cmp::Ordering::Less),
            (false, true) => Some(std::cmp::Ordering::Greater),
            (false, false) => None,
        }
    }
}

/// Builder for constructing [`ExtensibleBitmap`]s dynamically.
#[derive(Debug, Clone, Default)]
pub struct ExtensibleBitmapBuilder {
    items: Vec<BinaryMapItem>,
}

impl ExtensibleBitmapBuilder {
    /// Returns a new empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a single bit index into the builder.
    pub fn insert(&mut self, index: u32) {
        let start_bit = node_start_bit(index);
        let bit_index = node_bit_index(index);

        match self.items.binary_search_by_key(&start_bit, |item| item.start_bit) {
            Ok(idx) => {
                self.items[idx].map |= (1u64) << bit_index;
            }
            Err(idx) => {
                let item = BinaryMapItem { start_bit, map: (1u64) << bit_index };
                self.items.insert(idx, item);
            }
        }
    }

    /// Sets a range of bits [low, high] (inclusive) in the builder.
    pub fn set_range(&mut self, low: u32, high: u32) {
        if low > high {
            return;
        }
        let start_low = node_start_bit(low);
        let start_high = node_start_bit(high);

        if start_low == start_high {
            let bit_low = node_bit_index(low);
            let bit_high = node_bit_index(high);
            let mask = (MapNode::MAX >> ((MAP_NODE_BITS - 1) - (bit_high - bit_low))) << bit_low;
            self.or_mask(start_low, mask);
        } else {
            let first_bit_low = node_bit_index(low);
            let first_mask = MapNode::MAX << first_bit_low;
            self.or_mask(start_low, first_mask);

            let mut middle_start = start_low + MAP_NODE_BITS;
            while middle_start < start_high {
                self.or_mask(middle_start, MapNode::MAX);
                middle_start += MAP_NODE_BITS;
            }

            let last_bit_high = node_bit_index(high);
            let last_mask = MapNode::MAX >> ((MAP_NODE_BITS - 1) - last_bit_high);
            self.or_mask(start_high, last_mask);
        }
    }

    fn or_mask(&mut self, start_bit: u32, mask: u64) {
        match self.items.binary_search_by_key(&start_bit, |item| item.start_bit) {
            Ok(idx) => {
                self.items[idx].map |= mask;
            }
            Err(idx) => {
                let item = BinaryMapItem { start_bit, map: mask };
                self.items.insert(idx, item);
            }
        }
    }

    /// Builds the immutable [`ExtensibleBitmap`].
    pub fn build(self) -> ExtensibleBitmap {
        ExtensibleBitmap { items: Array::from(self.items) }
    }
}

impl Validate for ExtensibleBitmap {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        let mut min_start = 0;
        for item in self.items.iter() {
            item.validate(policy)?;

            let item_start_bit = item.start_bit;
            if item_start_bit < min_start {
                return Err(ValidateError::OutOfOrderExtensibleBitmapItems {
                    found_start_bit: item_start_bit,
                    min_start,
                });
            }
            min_start = item_start_bit + MAP_NODE_BITS;
        }

        Ok(())
    }
}

/// Iterator over contiguous spans of set bit indices `(low_idx, high_idx)` in an [`ExtensibleBitmap`].
#[derive(Clone)]
pub struct BitSpansIter<'a> {
    items: std::iter::Peekable<std::slice::Iter<'a, BinaryMapItem>>,
    current: Option<BinaryMapItem>,
}

impl<'a> Iterator for BitSpansIter<'a> {
    type Item = (u32, u32);

    fn next(&mut self) -> Option<Self::Item> {
        let mut item = self.current.as_mut()?;

        // Shift away any trailing zeros and begin our span in place.
        let zero_bits = item.map.trailing_zeros();
        item.map >>= zero_bits;
        item.start_bit += zero_bits;
        let low_idx = item.start_bit;

        loop {
            let one_bits = item.map.trailing_ones();
            item.map = item.map.checked_shr(one_bits).unwrap_or(0);
            item.start_bit += one_bits;

            if item.map != 0 {
                return Some((low_idx, item.start_bit - 1));
            }

            // Current item is exhausted. Proactively pull the next item into place.
            let end_bit = item.start_bit;
            self.current = self.items.next().copied();

            // If there is a next item and it is continuous with the next span then
            // check the first bit, to determine whether to continue the span.
            if let Some(next) = self.current.as_mut()
                && next.start_bit == end_bit
            {
                // Continue the span if the new `current` item is contiguous with the preceding one,
                // and has its first bit set.
                if (next.map & 1) == 1 {
                    item = next;
                    continue;
                }
            }

            return Some((low_idx, end_bit - 1));
        }
    }
}

/// Iterator over the indices of bits set in [`ExtensibleBitmap`].
#[derive(Clone)]
pub struct BitIter<'a> {
    spans: BitSpansIter<'a>,
    current: u32,
    high: u32,
}

impl<'a> Iterator for BitIter<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current > self.high {
            let (low, high) = self.spans.next()?;
            self.current = low;
            self.high = high;
        }
        let result = self.current;
        self.current += 1;
        Some(result)
    }
}

/// Type-safe wrapper around [`ExtensibleBitmap`] for holding a set of strongly-typed IDs.
///
/// If `WITH_ID_ZERO` is `false` (default), the zeroth bit of the bitmap corresponds to
/// identifier value one. If `true` (including a slot for ID 0), the first bit (index 1) corresponds to identifier value one.
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize)]
pub struct IdSet<T: PolicyId, const WITH_ID_ZERO: bool = false> {
    bitmap: ExtensibleBitmap,
    _phantom: PhantomData<T>,
}

impl<T: PolicyId, const WITH_ID_ZERO: bool> IdSet<T, WITH_ID_ZERO> {
    /// Constructs an empty [`IdSet`].
    pub fn empty() -> Self {
        IdSetBuilder::new().build()
    }

    /// Constructs an [`IdSet`] wrapping the supplied `bitmap`.
    pub fn from_bitmap(bitmap: ExtensibleBitmap) -> Self {
        Self { bitmap, _phantom: PhantomData }
    }

    /// Constructs an [`IdSet`] from an iterator of IDs.
    pub fn from_ids<I: IntoIterator<Item = T>>(ids: I) -> Self {
        let mut builder = IdSetBuilder::new();
        for id in ids {
            builder.insert(id);
        }
        builder.build()
    }

    /// Returns `true` if `self` is a subset of `other` (all elements in `self` are in `other`).
    pub fn is_subset(&self, other: &Self) -> bool {
        self.bitmap.is_subset(&other.bitmap)
    }

    /// Returns `true` if `self` is a superset of `other` (all elements in `other` are in `self`).
    pub fn is_superset(&self, other: &Self) -> bool {
        other.bitmap.is_subset(&self.bitmap)
    }

    /// Compares two [`IdSet`]s to determine their subset relationship.
    ///
    /// Returns `Some(Ordering::Equal)` if they are equal.
    /// Returns `Some(Ordering::Greater)` if `self` is a strict superset of `other`.
    /// Returns `Some(Ordering::Less)` if `self` is a strict subset of `other`.
    /// Returns `None` if they are incomparable (neither is a subset of the other).
    pub fn compare(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.bitmap.compare(&other.bitmap)
    }

    /// Returns `true` if the set contains the specified ID.
    pub fn contains(&self, id: T) -> bool {
        self.bitmap.is_set(Self::id_to_index(id))
    }

    /// Returns `true` if this set is empty.
    pub fn is_empty(&self) -> bool {
        self.bitmap.is_empty()
    }

    /// Returns the number of IDs in this set.
    pub fn count(&self) -> usize {
        self.bitmap.count()
    }

    /// Returns an iterator over the IDs in this set.
    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        self.bitmap
            .indices_of_set_bits()
            .map(|index| Self::index_to_id(index).expect("bitmap index must be valid ID"))
    }

    /// Returns an iterator over the contiguous spans of IDs in this set.
    pub fn spans(&self) -> SpansIter<'_, T, WITH_ID_ZERO> {
        SpansIter { spans: self.bitmap.spans(), _phantom: PhantomData }
    }

    /// Converts a logical ID to a physical bitmap index.
    fn id_to_index(id: T) -> u32 {
        if WITH_ID_ZERO { id.as_u32() } else { id.as_u32() - 1 }
    }

    /// Converts a physical bitmap index to a logical ID.
    fn index_to_id(index: u32) -> Option<T> {
        let id_val = if WITH_ID_ZERO { index } else { index + 1 };
        T::from_u32(id_val)
    }
}

/// Builder for constructing [`IdSet`]s dynamically.
#[derive(Debug, Clone)]
pub struct IdSetBuilder<T: PolicyId, const WITH_ID_ZERO: bool = false> {
    builder: ExtensibleBitmapBuilder,
    _phantom: PhantomData<T>,
}

impl<T: PolicyId, const WITH_ID_ZERO: bool> Default for IdSetBuilder<T, WITH_ID_ZERO> {
    fn default() -> Self {
        Self { builder: ExtensibleBitmapBuilder::new(), _phantom: PhantomData }
    }
}

impl<T: PolicyId, const WITH_ID_ZERO: bool> IdSetBuilder<T, WITH_ID_ZERO> {
    /// Returns a new empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a single ID into the builder.
    pub fn insert(&mut self, id: T) {
        self.builder.insert(IdSet::<T, WITH_ID_ZERO>::id_to_index(id));
    }

    /// Inserts a range of IDs [low, high] (inclusive) into the builder.
    pub fn insert_range(&mut self, low: T, high: T) {
        let low_index = IdSet::<T, WITH_ID_ZERO>::id_to_index(low);
        let high_index = IdSet::<T, WITH_ID_ZERO>::id_to_index(high);
        if low_index > high_index {
            return;
        }
        self.builder.set_range(low_index, high_index);
    }

    /// Builds the immutable [`IdSet`].
    pub fn build(self) -> IdSet<T, WITH_ID_ZERO> {
        IdSet { bitmap: self.builder.build(), _phantom: PhantomData }
    }
}

impl<T: PolicyId + Validate, const WITH_ID_ZERO: bool> Validate for IdSet<T, WITH_ID_ZERO> {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        self.bitmap.validate(policy)?;
        for index in self.bitmap.indices_of_set_bits() {
            let id = Self::index_to_id(index).ok_or(ValidateError::InvalidIdSetIndex { index })?;
            id.validate(policy)?;
        }
        Ok(())
    }
}

/// Contiguous range of IDs, inclusive of both bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdSpan<T> {
    low: T,
    high: T,
}

impl<T: Copy> IdSpan<T> {
    /// Constructs a new [`IdSpan`].
    pub fn new(low: T, high: T) -> Self {
        Self { low, high }
    }

    /// Returns the lower bound ID.
    pub fn low(&self) -> T {
        self.low
    }

    /// Returns the upper bound ID.
    pub fn high(&self) -> T {
        self.high
    }
}

/// Iterator over contiguous spans of IDs in an [`IdSet`].
#[derive(Clone)]
pub struct SpansIter<'a, T: PolicyId, const WITH_ID_ZERO: bool> {
    spans: BitSpansIter<'a>,
    _phantom: PhantomData<T>,
}

impl<'a, T: PolicyId, const WITH_ID_ZERO: bool> Iterator for SpansIter<'a, T, WITH_ID_ZERO> {
    type Item = IdSpan<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let (low_idx, high_idx) = self.spans.next()?;
        let low = IdSet::<T, WITH_ID_ZERO>::index_to_id(low_idx)?;
        let high = IdSet::<T, WITH_ID_ZERO>::index_to_id(high_idx)?;
        Some(IdSpan::new(low, high))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::TypeId;
    use crate::new_policy::error::ParseError;
    use crate::new_policy::parser::PolicyCursor;
    use crate::new_policy::traits::Parse;

    fn parse_bitmap(bytes: &[u8]) -> Result<ExtensibleBitmap, ParseError> {
        let mut cursor = PolicyCursor::new(bytes);
        ExtensibleBitmap::parse(&mut cursor)
    }

    #[test]
    fn test_empty_bitmap() {
        let bytes = [
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // count = 0
        ];
        let bitmap = parse_bitmap(&bytes).unwrap();
        assert!(bitmap.is_empty());
        assert_eq!(bitmap.high_bit(), 0);
        assert!(!bitmap.is_set(0));
        assert!(!bitmap.is_set(100));

        let bits: Vec<u32> = bitmap.indices_of_set_bits().collect();
        assert!(bits.is_empty());
    }

    #[test]
    fn test_bitmap_with_bits() {
        let bytes = [
            64, 0, 0, 0, // map_item_size_bits = 64
            128, 0, 0, 0, // high_bit = 128
            2, 0, 0, 0, // count = 2
            // Item 1
            0, 0, 0, 0, // start_bit = 0
            5, 0, 0, 0, 0, 0, 0, 0, // map = 5 (bits 0 and 2 set)
            // Item 2
            64, 0, 0, 0, // start_bit = 64
            2, 0, 0, 0, 0, 0, 0, 0, // map = 2 (bit 65 set)
        ];
        let bitmap = parse_bitmap(&bytes).unwrap();
        assert!(!bitmap.is_empty());
        assert_eq!(bitmap.high_bit(), 128);

        assert!(bitmap.is_set(0));
        assert!(!bitmap.is_set(1));
        assert!(bitmap.is_set(2));
        assert!(!bitmap.is_set(3));
        assert!(!bitmap.is_set(64));
        assert!(bitmap.is_set(65));
        assert!(!bitmap.is_set(66));
        assert!(!bitmap.is_set(128)); // Out of bounds

        let bits: Vec<u32> = bitmap.indices_of_set_bits().collect();
        assert_eq!(bits, vec![0, 2, 65]);
        assert_eq!(bitmap.count(), 3);
    }

    #[test]
    fn test_id_set() {
        let bytes = [
            64, 0, 0, 0, // map_item_size_bits = 64
            64, 0, 0, 0, // high_bit = 64
            1, 0, 0, 0, // count = 1
            0, 0, 0, 0, // start_bit = 0
            5, 0, 0, 0, 0, 0, 0, 0, // map = 5 (bits 0 and 2 set)
        ];

        let mut cursor = PolicyCursor::new(&bytes);
        let id_set = IdSet::<TypeId, false>::parse(&mut cursor).unwrap();

        assert_eq!(id_set.count(), 2);
        assert!(id_set.contains(TypeId::from_u32(1).unwrap()));
        assert!(!id_set.contains(TypeId::from_u32(2).unwrap()));
        assert!(id_set.contains(TypeId::from_u32(3).unwrap()));

        let ids: Vec<TypeId> = id_set.iter().collect();
        assert_eq!(ids, vec![TypeId::from_u32(1).unwrap(), TypeId::from_u32(3).unwrap()]);
    }

    #[test]
    fn test_bitmap_compare() {
        let empty = ExtensibleBitmapBuilder::new().build();

        let mut builder1 = ExtensibleBitmapBuilder::new();
        builder1.insert(0);
        builder1.insert(10);
        builder1.insert(65);
        let b1 = builder1.build();

        // 1. Empty bitmap comparisons
        assert_eq!(empty.compare(&empty), Some(std::cmp::Ordering::Equal));
        assert!(empty.is_subset(&empty));
        assert_eq!(empty.compare(&b1), Some(std::cmp::Ordering::Less));
        assert!(empty.is_subset(&b1));
        assert_eq!(b1.compare(&empty), Some(std::cmp::Ordering::Greater));
        assert!(!b1.is_subset(&empty));

        // 2. Equal bitmaps
        assert_eq!(b1.compare(&b1), Some(std::cmp::Ordering::Equal));
        assert!(b1.is_subset(&b1));

        // 3. Strict superset / subset within existing map nodes
        let mut builder2 = ExtensibleBitmapBuilder::new();
        builder2.insert(0);
        builder2.insert(10);
        builder2.insert(65);
        builder2.insert(100);
        let b2 = builder2.build();

        assert_eq!(b1.compare(&b2), Some(std::cmp::Ordering::Less));
        assert_eq!(b2.compare(&b1), Some(std::cmp::Ordering::Greater));
        assert!(b1.is_subset(&b2));
        assert!(!b2.is_subset(&b1));

        // 4. Strict superset / subset involving non-overlapping map nodes (iterator exhaustion & node skipping)
        let mut builder_node0 = ExtensibleBitmapBuilder::new();
        builder_node0.insert(5);
        let b_node0 = builder_node0.build();

        let mut builder_node0_and_128 = ExtensibleBitmapBuilder::new();
        builder_node0_and_128.insert(5);
        builder_node0_and_128.insert(130);
        let b_node0_and_128 = builder_node0_and_128.build();

        assert_eq!(b_node0.compare(&b_node0_and_128), Some(std::cmp::Ordering::Less));
        assert_eq!(b_node0_and_128.compare(&b_node0), Some(std::cmp::Ordering::Greater));
        assert!(b_node0.is_subset(&b_node0_and_128));
        assert!(!b_node0_and_128.is_subset(&b_node0));

        // 5. Disjoint / incomparable map nodes
        let mut builder_node128 = ExtensibleBitmapBuilder::new();
        builder_node128.insert(130);
        let b_node128 = builder_node128.build();

        assert_eq!(b_node0.compare(&b_node128), None);
        assert!(!b_node0.is_subset(&b_node128));
        assert!(!b_node128.is_subset(&b_node0));

        // 6. Multi-word contiguous ranges (set_range)
        let mut builder_range_wide = ExtensibleBitmapBuilder::new();
        builder_range_wide.set_range(0, 150);
        let b_range_wide = builder_range_wide.build();

        let mut builder_range_narrow = ExtensibleBitmapBuilder::new();
        builder_range_narrow.set_range(10, 100);
        let b_range_narrow = builder_range_narrow.build();

        assert_eq!(b_range_narrow.compare(&b_range_wide), Some(std::cmp::Ordering::Less));
        assert_eq!(b_range_wide.compare(&b_range_narrow), Some(std::cmp::Ordering::Greater));
        assert!(b_range_narrow.is_subset(&b_range_wide));
        assert!(!b_range_wide.is_subset(&b_range_narrow));
    }

    #[test]
    fn test_bitmap_set_range() {
        // 1. Single-word range (start_low == start_high)
        let mut builder = ExtensibleBitmapBuilder::new();
        builder.set_range(10, 20);
        let b1 = builder.build();
        for i in 0..128 {
            assert_eq!(b1.is_set(i), (10..=20).contains(&i), "bit {i} mismatch for range [10, 20]");
        }

        // 2. Full single word [0, 63]
        let mut builder = ExtensibleBitmapBuilder::new();
        builder.set_range(0, 63);
        let b2 = builder.build();
        for i in 0..128 {
            assert_eq!(b2.is_set(i), (0..=63).contains(&i), "bit {i} mismatch for range [0, 63]");
        }

        // 3. Two adjacent words (no middle word) [60, 70]
        let mut builder = ExtensibleBitmapBuilder::new();
        builder.set_range(60, 70);
        let b3 = builder.build();
        for i in 0..128 {
            assert_eq!(b3.is_set(i), (60..=70).contains(&i), "bit {i} mismatch for range [60, 70]");
        }

        // 4. Multi-word range with middle full words [10, 140]
        let mut builder = ExtensibleBitmapBuilder::new();
        builder.set_range(10, 140);
        let b4 = builder.build();
        for i in 0..200 {
            assert_eq!(
                b4.is_set(i),
                (10..=140).contains(&i),
                "bit {i} mismatch for range [10, 140]"
            );
        }

        // 5. Single bit via set_range (low == high)
        let mut builder = ExtensibleBitmapBuilder::new();
        builder.set_range(42, 42);
        let b5 = builder.build();
        assert!(b5.is_set(42));
        assert_eq!(b5.indices_of_set_bits().collect::<Vec<_>>(), vec![42]);

        // 6. Invalid range (low > high) should be a no-op
        let mut builder = ExtensibleBitmapBuilder::new();
        builder.set_range(50, 40);
        let b6 = builder.build();
        assert!(b6.is_empty());
    }
}
