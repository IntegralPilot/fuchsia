// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::packed_map::PackedMap;
use crate::{PackedItem, PackedVec};
use std::borrow::Borrow;
use std::collections::BTreeMap;

/// A builder for `PackedMap` that allows incrementally appending potentially
/// unsorted key-value pairs.
///
/// # Algorithm
///
/// `PackedMapBuilder` uses an LSM-tree (Log-Structured Merge-tree) inspired approach
/// to handle out-of-order insertions efficiently while maintaining a compact,
/// immutable representation for the final map.
///
/// It maintains two structures:
/// 1. `packed`: A `PackedMap` containing elements that are already sorted and packed.
/// 2. `unpacked`: A `BTreeMap` serving as a buffer for new, potentially out-of-order insertions.
///
/// ## Operations
///
/// - **Insertion**:
///   - We first attempt to insert the key-value pair directly into the `packed` map.
///     `PackedMap::insert` succeeds if the key already exists (overwriting it) or
///     if the key is greater than or equal to the last key in the map (maintaining sorted order).
///     This fast path takes `O(log N)` or `O(1)` time.
///   - If the key is out of order, the `packed` map rejects it. We then insert it into
///     the `unpacked` buffer (`BTreeMap`).
/// - **Lookup**: To find a key, we check both the `packed` map and the `unpacked` buffer.
/// - **Build**: When `build()` is called, any remaining elements in the `unpacked` buffer
///   are merged with the `packed` map to produce the final `PackedMap`.
///
/// This design optimizes for the common case where data is mostly sorted, while still
/// supporting full unpacked insertion without severe performance degradation.
pub struct PackedMapBuilder<K: ?Sized + Ord + PackedItem, V>
where
    K: ToOwned,
    K::Owned: Ord,
{
    packed: PackedMap<K, V>,
    unpacked: BTreeMap<K::Owned, V>,
}

impl<K, V> PackedMapBuilder<K, V>
where
    K: ?Sized + Ord + PackedItem + ToOwned,
    K::Owned: Ord,
{
    /// Creates a new empty `PackedMapBuilder`.
    pub fn new() -> Self {
        Self { packed: PackedMap::new(), unpacked: BTreeMap::new() }
    }

    /// Creates a new `PackedMapBuilder` with specified capacity.
    ///
    /// The `element_capacity` argument specifies the number of slices that can be
    /// stored without reallocating the offsets vector. The `buffer_capacity`
    /// argument specifies the cumulative length of slices that can be stored
    /// without reallocating the data vector.
    pub fn with_capacity(element_capacity: usize, buffer_capacity: usize) -> Self {
        Self {
            packed: PackedMap::with_capacity(element_capacity, buffer_capacity),
            unpacked: BTreeMap::new(),
        }
    }

    /// Returns `true` if the builder contains the given key.
    pub fn contains_key(&self, key: &K) -> bool {
        self.packed.contains_key(key) || self.unpacked.contains_key(key)
    }

    /// Returns a reference to the value corresponding to the key.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.packed.get(key).or_else(|| self.unpacked.get(key))
    }

    /// Inserts a key-value pair into the builder.
    ///
    /// # Complexity
    ///
    /// - `O(1)` if the key is greater than or equal to the last key in the map.
    /// - `O(log N)` otherwise, where `N` is the number of elements in the map.
    pub fn insert(&mut self, key: &K, value: V) -> Option<V> {
        match self.packed.append_or_update(key, value) {
            Ok(old_value) => old_value,
            Err(value) => self.unpacked.insert(key.to_owned(), value),
        }
    }

    /// Builds the `PackedMap`.
    pub fn build(self) -> PackedMap<K, V> {
        if self.unpacked.is_empty() { self.packed } else { merge(self.packed, self.unpacked) }
    }
}

impl<K, V> Default for PackedMapBuilder<K, V>
where
    K: ?Sized + Ord + PackedItem + ToOwned,
    K::Owned: Ord,
{
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn merge<K, V>(
    mut packed: PackedMap<K, V>,
    unpacked: BTreeMap<K::Owned, V>,
) -> PackedMap<K, V>
where
    K: ?Sized + Ord + PackedItem + ToOwned,
    K::Owned: Ord,
{
    let len1 = packed.keys.len();
    let len2 = unpacked.len();

    let buffer_len1 = packed.buffer_len();
    let buffer_len2: usize = unpacked.keys().map(|key| key.borrow().as_bytes().len()).sum();

    let mut out_keys = PackedVec::with_capacity(len1 + len2, buffer_len1 + buffer_len2);
    let mut out_values = Vec::with_capacity(len1 + len2);

    let mut drain1 = packed.drain();
    let mut drain2 = unpacked.into_iter();

    let mut next1 = drain1.next();
    let mut next2 = drain2.next();

    loop {
        match (next1, next2) {
            (Some((k1, v1)), Some((k2, v2))) => match k1.cmp(k2.borrow()) {
                std::cmp::Ordering::Less => {
                    out_keys.push(k1);
                    out_values.push(v1);
                    next1 = drain1.next();
                    next2 = Some((k2, v2));
                }
                std::cmp::Ordering::Greater => {
                    out_keys.push(k2.borrow());
                    out_values.push(v2);
                    next1 = Some((k1, v1));
                    next2 = drain2.next();
                }
                std::cmp::Ordering::Equal => {
                    out_keys.push(k1);
                    out_values.push(v2);
                    next1 = drain1.next();
                    next2 = drain2.next();
                }
            },
            (Some((k1, v1)), None) => {
                out_keys.push(k1);
                out_values.push(v1);
                while let Some((k1, v1)) = drain1.next() {
                    out_keys.push(k1);
                    out_values.push(v1);
                }
                break;
            }
            (None, Some((k2, v2))) => {
                out_keys.push(k2.borrow());
                out_values.push(v2);
                while let Some((k2, v2)) = drain2.next() {
                    out_keys.push(k2.borrow());
                    out_values.push(v2);
                }
                break;
            }
            (None, None) => break,
        }
    }

    PackedMap::from_parts(out_keys, out_values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packed_map_builder_lsm() {
        let mut builder = PackedMapBuilder::new();
        for i in 0..129 {
            let s = format!("{:03}", 128 - i);
            builder.insert(s.as_str(), i); // Out of order!
        }
        let map = builder.build();
        assert_eq!(map.element_len(), 129);
        assert_eq!(map.get("000"), Some(&128));
        assert_eq!(map.get(&format!("{}", 128)), Some(&0));
    }

    #[test]
    fn test_builder_insert() {
        let mut builder = PackedMapBuilder::new();

        assert_eq!(builder.insert("a", 1), None);
        assert_eq!(builder.insert("a", 2), Some(1));

        builder.insert("b", 3);
        builder.insert("c", 4);

        assert_eq!(builder.insert("b", 3), Some(3));
        assert_eq!(builder.insert("c", 4), Some(4));

        assert!(!builder.contains_key("aa"));
        assert_eq!(builder.get("a"), Some(&2));
        assert_eq!(builder.get("aa"), None);

        builder.insert("e", 6);
        builder.insert("d", 5);

        let map = builder.build();
        assert_eq!(map.get("aa"), None);
        assert_eq!(map.get("a"), Some(&2));
        assert_eq!(map.get("b"), Some(&3));
        assert_eq!(map.get("c"), Some(&4));
        assert_eq!(map.get("d"), Some(&5));
        assert_eq!(map.get("e"), Some(&6));
    }

    #[test]
    fn test_insert_out_of_order_sorts() {
        let mut builder = PackedMapBuilder::new();

        builder.insert("a", 1);
        builder.insert("c", 3);
        builder.insert("b", 2);

        let map = builder.build();
        let collected: Vec<_> = map.iter().collect();
        assert_eq!(collected, vec![("a", &1), ("b", &2), ("c", &3)]);

        let keys: Vec<_> = map.keys.iter().collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_insert_comprehensive() {
        let mut builder = PackedMapBuilder::new();

        builder.insert("a", 1);
        builder.insert("c", 3);
        builder.insert("b", 2);

        assert_eq!(builder.insert("a", 10), Some(1));
        assert_eq!(builder.insert("a", 10), Some(10));

        assert_eq!(builder.insert("b", 20), Some(2));
        assert_eq!(builder.insert("b", 20), Some(20));

        assert!(!builder.contains_key("d"));

        assert_eq!(builder.insert("d", 4), None);
        assert_eq!(builder.insert("d", 4), Some(4));

        let map = builder.build();
        assert_eq!(map.get("a"), Some(&10));
        assert_eq!(map.get("b"), Some(&20));
        assert_eq!(map.get("c"), Some(&3));
        assert_eq!(map.get("d"), Some(&4));
    }

    #[test]
    fn test_packed_map_merge() {
        let mut map1 = PackedMap::new();
        map1.append_or_update("a", 1).unwrap();
        map1.append_or_update("c", 3).unwrap();

        let mut map2 = BTreeMap::new();
        assert_eq!(map2.insert("b".into(), 2), None);
        assert_eq!(map2.insert("c".into(), 30), None);

        let merged = merge(map1, map2);
        assert_eq!(merged.element_len(), 3);
        assert_eq!(merged.get("a"), Some(&1));
        assert_eq!(merged.get("b"), Some(&2));
        assert_eq!(merged.get("c"), Some(&30));
    }

    #[derive(
        Debug, Clone, Copy, Eq, zerocopy::IntoBytes, zerocopy::Immutable, zerocopy::Unaligned,
    )]
    #[repr(C)]
    struct TestKey {
        id: u8,
        metadata: u8,
    }

    impl PartialEq for TestKey {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }

    impl Ord for TestKey {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.id.cmp(&other.id)
        }
    }

    impl PartialOrd for TestKey {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl PackedItem for TestKey {
        unsafe fn from_bytes(bytes: &[u8]) -> &Self {
            unsafe { &*(bytes.as_ptr() as *const TestKey) }
        }
    }

    #[test]
    fn test_insert_keeps_old_key_packed() {
        let mut builder = PackedMapBuilder::new();
        let key1 = TestKey { id: 1, metadata: 10 };
        let key2 = TestKey { id: 1, metadata: 20 };

        builder.insert(&key1, 1);
        builder.insert(&key2, 2); // Equal, should go to packed

        let map = builder.build();
        let (k, v) = map.iter().next().unwrap();
        assert_eq!(k.metadata, 10);
        assert_eq!(*v, 2);
    }

    #[test]
    fn test_insert_keeps_old_key_unpacked() {
        let mut builder = PackedMapBuilder::new();
        let key1 = TestKey { id: 2, metadata: 10 };
        let key2 = TestKey { id: 1, metadata: 20 };
        let key3 = TestKey { id: 1, metadata: 30 };

        builder.insert(&key1, 1);
        builder.insert(&key2, 2); // Out of order, goes to unpacked
        builder.insert(&key3, 3); // Out of order, goes to unpacked

        let map = builder.build();
        let mut iter = map.iter();
        let (k1, v1) = iter.next().unwrap();
        assert_eq!(k1.id, 1);
        assert_eq!(k1.metadata, 20); // Keeps old key from unpacked
        assert_eq!(*v1, 3); // Keeps new value
    }
}
