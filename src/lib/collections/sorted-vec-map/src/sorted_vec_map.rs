// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::{Bound, Range, RangeBounds};
use std::{cmp, iter, slice};

/// An ordered map built on a `Vec`.
///
/// `SortedVecMap` provides a memory-efficient alternative to `BTreeMap` when the data
/// is read-heavy and changes rarely. It stores entries in a single contiguous `Vec`,
/// eliminating pointer overhead and improving cache locality.
///
/// # Complexity
///
/// | Operation | Time Complexity | Space Complexity |
/// |---|---|---|
/// | `new` / `with_capacity` | `O(1)` | `O(1)` |
/// | `get` / `contains_key` | `O(log N)` | `O(1)` |
/// | `insert` | `O(N)` | `O(1)` amortized |
/// | `remove` | `O(N)` | `O(1)` |
/// | `Extend::extend` / `FromIterator` | `O(N)` (sorted) / `O(N log N)` (unsorted) | `O(N)` |
/// | `range` | `O(log N)` setup, `O(1)` per step | `O(1)` |
///
/// # When to use
///
/// - **Read-heavy workloads**: Lookups are fast (`O(log N)`) and cache-friendly.
/// - **Memory constraints**: Stores data contiguously without node allocation overhead.
/// - **Batch construction**: Building from an iterator is efficient using `SortedVecMapBuilder`.
///
/// # When to avoid
///
/// - **Frequent mutations**: Single insertions and removals take linear time (`O(N)`) due to element shifting.
#[derive(Eq, PartialEq, PartialOrd, Ord, Hash, Clone)]
pub struct SortedVecMap<K, V> {
    vec: Vec<(K, V)>,
}

impl<K, V> SortedVecMap<K, V> {
    /// Returns a new builder for `SortedVecMap`.
    pub fn builder() -> SortedVecMapBuilder<K, V> {
        SortedVecMapBuilder::new()
    }

    /// Returns a new builder for `SortedVecMap` with at least the specified capacity.
    pub fn builder_with_capacity(capacity: usize) -> SortedVecMapBuilder<K, V> {
        SortedVecMapBuilder::with_capacity(capacity)
    }

    /// Constructs a new, empty `SortedVecMap`.
    pub fn new() -> Self {
        Self { vec: Vec::new() }
    }

    /// Returns the number of elements the map can hold without reallocating.
    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }

    /// Constructs a new, empty `SortedVecMap` with at least the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { vec: Vec::with_capacity(capacity) }
    }

    /// Reserves capacity for at least `additional` more elements in the `SortedVecMap`.
    ///
    /// The collection may reserve more space than necessary to avoid reallocations.
    pub fn reserve(&mut self, additional: usize) {
        self.vec.reserve(additional);
    }

    /// Shrinks the capacity of the map as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.vec.shrink_to_fit();
    }

    /// Returns the number of elements in the map.
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Returns true if there are no entries in the map.
    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }

    /// Returns true if the map contains an entry for the given key.
    ///
    /// Complexity: `O(log n)` time.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        self.index_of(key).is_ok()
    }

    /// Returns a reference to the value corresponding to the key.
    ///
    /// Complexity: `O(log n)` time.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        if let Ok(index) = self.index_of(key) { Some(&self.vec[index].1) } else { None }
    }

    /// Returns a mutable reference to the value corresponding to the key.
    ///
    /// Complexity: `O(log n)` time.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        if let Ok(index) = self.index_of(key) { Some(&mut self.vec[index].1) } else { None }
    }

    /// Inserts a key-value pair into the map. If the map did not have this key present, `None` is
    /// returned. If the map did have this key present, the value is updated, and the old value is
    /// returned. The key is not updated.
    ///
    /// Complexity: `O(log n)` search time, plus `O(n)` time to insert the element if it is not present.
    /// Note that inserting N elements one by one takes `O(N^2)` time. Use `SortedVecMapBuilder`
    /// for efficient construction from multiple elements.
    pub fn insert(&mut self, key: K, value: V) -> Option<V>
    where
        K: Ord,
    {
        match self.index_of(&key) {
            Ok(index) => {
                let old = std::mem::replace(&mut self.vec[index].1, value);
                Some(old)
            }
            Err(index) => {
                self.vec.insert(index, (key, value));
                None
            }
        }
    }

    /// Removes a key from the map, returning the value at the key if the key was previously in the map.
    ///
    /// Complexity: `O(log n)` search time, plus `O(n)` time to remove the element if it is present.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        if let Ok(index) = self.index_of(key) {
            let (_, value) = self.vec.remove(index);
            Some(value)
        } else {
            None
        }
    }

    /// Returns an iterator over the entries of the map, sorted by key.
    pub fn iter(&self) -> Iter<'_, K, V> {
        self.vec.iter().map(|entry| (&entry.0, &entry.1))
    }

    /// Returns an iterator over the entries of the map, sorted by key.
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        self.vec.iter_mut().map(|entry| (&entry.0, &mut entry.1))
    }

    /// Returns an iterator over the keys of the map, in sorted order.
    pub fn keys(&self) -> Keys<'_, K, V> {
        self.vec.iter().map(|e| &e.0)
    }

    /// Returns an iterator over the values of the map, sorted by key.
    pub fn values(&self) -> Values<'_, K, V> {
        self.vec.iter().map(|e| &e.1)
    }

    /// Returns a mutable iterator over the values of the map, sorted by key.
    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        self.vec.iter_mut().map(|e| &mut e.1)
    }

    /// Constructs a double-ended iterator over a sub-range of elements in the map.
    ///
    /// Complexity: `O(log n)` to find the start and end of the range.
    pub fn range<Q, R>(&self, range: R) -> Iter<'_, K, V>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
        R: RangeBounds<Q>,
    {
        let indices = self.range_indices(range);
        self.vec[indices].iter().map(|entry| (&entry.0, &entry.1))
    }

    /// Constructs a mutable double-ended iterator over a sub-range of elements in the map.
    ///
    /// Complexity: `O(log n)` to find the start and end of the range.
    pub fn range_mut<Q, R>(&mut self, range: R) -> IterMut<'_, K, V>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
        R: RangeBounds<Q>,
    {
        let indices = self.range_indices(range);
        self.vec[indices].iter_mut().map(|entry| (&entry.0, &mut entry.1))
    }

    fn range_indices<Q, R>(&self, range: R) -> Range<usize>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
        R: RangeBounds<Q>,
    {
        let start = match range.start_bound() {
            Bound::Included(key) => self.index_of(key).unwrap_or_else(|e| e),
            Bound::Excluded(key) => match self.index_of(key) {
                Ok(idx) => idx + 1,
                Err(idx) => idx,
            },
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(key) => match self.index_of(key) {
                Ok(idx) => idx + 1,
                Err(idx) => idx,
            },
            Bound::Excluded(key) => self.index_of(key).unwrap_or_else(|e| e),
            Bound::Unbounded => self.vec.len(),
        };

        let start = cmp::min(start, self.vec.len());
        let end = cmp::max(start, cmp::min(end, self.vec.len()));

        start..end
    }

    /// Searches for the key in the map.
    ///
    /// If the key is found then `Result::Ok` is returned with the index of the entry. If the key is
    /// not found then `Result::Err` is return, containing the index where an entry with that key
    /// could be inserted while maintaining sorted order.
    fn index_of<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        self.vec.binary_search_by(|probe| probe.0.borrow().cmp(key))
    }
}

impl<K, V> Default for SortedVecMap<K, V>
where
    K: Ord,
{
    fn default() -> SortedVecMap<K, V> {
        SortedVecMap::new()
    }
}

impl<K: Debug, V: Debug> Debug for SortedVecMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, Q, V> std::ops::Index<&Q> for SortedVecMap<K, V>
where
    K: Borrow<Q> + Ord,
    Q: Ord + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &Self::Output {
        self.get(key).expect("no entry found for key")
    }
}

pub type Iter<'a, K, V> = iter::Map<slice::Iter<'a, (K, V)>, fn(&(K, V)) -> (&K, &V)>;
pub type IterMut<'a, K, V> = iter::Map<slice::IterMut<'a, (K, V)>, fn(&mut (K, V)) -> (&K, &mut V)>;
pub type Keys<'a, K, V> = iter::Map<slice::Iter<'a, (K, V)>, fn(&(K, V)) -> &K>;
pub type Values<'a, K, V> = iter::Map<slice::Iter<'a, (K, V)>, fn(&(K, V)) -> &V>;
pub type ValuesMut<'a, K, V> = iter::Map<slice::IterMut<'a, (K, V)>, fn(&mut (K, V)) -> &mut V>;

impl<K: Ord, V> Extend<(K, V)> for SortedVecMap<K, V> {
    /// Extends the map with the contents of an iterator.
    ///
    /// This is more efficient than inserting elements one by one.
    ///
    /// Complexity: `O(n log n)` where n is the total number of elements, or `O(n)` if the
    /// iterator yields elements in sorted order and they are all greater than the existing
    /// elements.
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        let vec = std::mem::take(&mut self.vec);
        let mut builder = SortedVecMapBuilder { vec, is_sorted_and_deduped: true };
        builder.extend(iter);
        *self = builder.build();
    }
}

impl<K, V> IntoIterator for SortedVecMap<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.vec.into_iter()
    }
}

impl<'a, K, V> IntoIterator for &'a SortedVecMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut SortedVecMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for SortedVecMap<K, V> {
    /// Constructs a `SortedVecMap` from an iterator.
    ///
    /// Complexity: `O(n log n)` where n is the number of elements in the iterator,
    /// or `O(n)` if the elements are already sorted.
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        SortedVecMapBuilder::from_iter(iter).build()
    }
}

impl<K: Ord, V, const N: usize> From<[(K, V); N]> for SortedVecMap<K, V> {
    /// Constructs a `SortedVecMap` from an iterator.
    ///
    /// Complexity: `O(n log n)` where n is the number of elements in the iterator,
    /// or `O(n)` if the elements are already sorted.
    fn from(arr: [(K, V); N]) -> Self {
        SortedVecMapBuilder::from_iter(arr).build()
    }
}

impl<'de, K, V> serde::Deserialize<'de> for SortedVecMap<K, V>
where
    K: Ord + serde::Deserialize<'de>,
    V: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor<K, V> {
            _map_type: PhantomData<SortedVecMap<K, V>>,
        }
        impl<'de, K, V> serde::de::Visitor<'de> for Visitor<K, V>
        where
            K: Ord + serde::Deserialize<'de>,
            V: serde::Deserialize<'de>,
        {
            type Value = SortedVecMap<K, V>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut builder = match map.size_hint() {
                    Some(hint) => SortedVecMapBuilder::with_capacity(hint),
                    None => SortedVecMapBuilder::new(),
                };
                while let Some(entry) = map.next_entry()? {
                    builder.insert(entry.0, entry.1);
                }
                Ok(builder.build())
            }
        }
        deserializer.deserialize_map(Visitor { _map_type: PhantomData })
    }
}

impl<K: serde::Serialize, V: serde::Serialize> serde::Serialize for SortedVecMap<K, V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.iter())
    }
}

/// A builder for `SortedVecMap`.
///
/// This builder allows for efficient construction of a `SortedVecMap` by tracking whether the
/// entries are inserted in sorted order. If they are, sorting can be skipped.
pub struct SortedVecMapBuilder<K, V> {
    vec: Vec<(K, V)>,
    is_sorted_and_deduped: bool,
}

impl<K, V> SortedVecMapBuilder<K, V> {
    /// Creates a new, empty builder.
    pub fn new() -> Self {
        Self { vec: Vec::new(), is_sorted_and_deduped: true }
    }

    /// Creates a new, empty builder with at least the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { vec: Vec::with_capacity(capacity), is_sorted_and_deduped: true }
    }

    /// Inserts a key-value pair into the builder.
    ///
    /// If the key is the same as the last inserted key, the value is overwritten.
    ///
    /// Complexity: `O(1)` time (amortized).
    pub fn insert(&mut self, key: K, value: V) -> &mut Self
    where
        K: Ord,
    {
        if let Some(back) = self.vec.last_mut() {
            match back.0.cmp(&key) {
                Ordering::Equal => {
                    back.1 = value;
                    return self;
                }
                Ordering::Greater => {
                    self.is_sorted_and_deduped = false;
                }
                Ordering::Less => {}
            }
        }
        self.vec.push((key, value));
        self
    }

    /// Builds the `SortedVecMap`.
    ///
    /// If the entries were not inserted in strictly increasing order, they will be sorted and
    /// deduplicated.
    ///
    /// Complexity: `O(n)` time if already sorted, `O(n log n)` otherwise, where n is the number of
    /// elements.
    pub fn build(mut self) -> SortedVecMap<K, V>
    where
        K: Ord,
    {
        if !self.is_sorted_and_deduped {
            sort_and_dedup(&mut self.vec);
        }
        SortedVecMap { vec: self.vec }
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for SortedVecMapBuilder<K, V> {
    /// Constructs a `SortedVecMapBuilder` from an iterator.
    ///
    /// Complexity: `O(n)` time where n is the number of elements in the iterator.
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let (lower, upper) = iter.size_hint();
        let mut builder = SortedVecMapBuilder::with_capacity(upper.unwrap_or(lower));
        builder.extend(iter);
        builder
    }
}

impl<K: Ord, V> Extend<(K, V)> for SortedVecMapBuilder<K, V> {
    /// Extends the builder with the contents of an iterator.
    ///
    /// Complexity: `O(n)` time where n is the number of elements in the iterator.
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        let iter = iter.into_iter();
        let (lower, upper) = iter.size_hint();
        self.vec.reserve(upper.unwrap_or(lower));
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

fn sort_and_dedup<K: Ord, V>(vec: &mut Vec<(K, V)>) {
    vec.sort_by(|a, b| a.0.cmp(&b.0));

    // Compares two consecutive elements and removes the second one if they have the same key.
    // We preserve the old key and the new value.
    vec.dedup_by(|a, b| {
        if a.0 == b.0 {
            std::mem::swap(&mut a.1, &mut b.1);
            true
        } else {
            false
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ops::Bound;
    use test_case::test_case;

    #[test_case(0 => 0; "empty_map")]
    #[test_case(10 => 10; "with_some_capacity")]
    fn test_with_capacity(cap: usize) -> usize {
        let map: SortedVecMap<i32, i32> = SortedVecMap::with_capacity(cap);
        map.capacity()
    }

    #[test_case(vec![], 1 => None; "empty_map")]
    #[test_case(vec![(1, 21)], 1 => Some(21); "contains_key")]
    #[test_case(vec![(1, 21), (0, 20)], 0 => Some(20); "contains_key_unsorted_init")]
    #[test_case(vec![(1, 21), (0, 20), (2, 22)], 2 => Some(22); "contains_key_middle")]
    #[test_case(vec![(1, 21), (0, 20), (2, 22)], 3 => None; "missing_key")]
    fn test_get(initial: Vec<(i32, i32)>, lookup: i32) -> Option<i32> {
        let map: SortedVecMap<i32, i32> = initial.into_iter().collect();
        map.get(&lookup).copied()
    }

    #[test_case(vec![], (50, 50), None, vec![(50, 50)]; "insert_empty")]
    #[test_case(vec![(50, 50)], (47, 47), None, vec![(47, 47), (50, 50)]; "insert_lesser")]
    #[test_case(vec![(47, 47), (50, 50)], (48, 48), None, vec![(47, 47), (48, 48), (50, 50)]; "insert_middle")]
    #[test_case(vec![(47, 47), (50, 50)], (51, 51), None, vec![(47, 47), (50, 50), (51, 51)]; "insert_greater")]
    #[test_case(vec![(47, 47), (48, 48), (50, 50)], (48, 88), Some(48), vec![(47, 47), (48, 88), (50, 50)]; "insert_overwrite")]
    fn test_insert(
        initial: Vec<(i32, i32)>,
        to_insert: (i32, i32),
        expected_return: Option<i32>,
        expected_vec: Vec<(i32, i32)>,
    ) {
        let mut map: SortedVecMap<i32, i32> = initial.into_iter().collect();
        let ret = map.insert(to_insert.0, to_insert.1);
        assert_eq!(ret, expected_return);
        assert_eq!(map.vec, expected_vec);
    }

    #[test_case(vec![], 1 => (None, vec![]); "remove_empty")]
    #[test_case(vec![(1, 21)], 1 => (Some(21), vec![]); "remove_only_element")]
    #[test_case(vec![(0, 20), (1, 21)], 0 => (Some(20), vec![(1, 21)]); "remove_first")]
    #[test_case(vec![(0, 20), (1, 21)], 1 => (Some(21), vec![(0, 20)]); "remove_last")]
    #[test_case(vec![(0, 20), (1, 21)], 2 => (None, vec![(0, 20), (1, 21)]); "remove_missing")]
    fn test_remove(initial: Vec<(i32, i32)>, to_remove: i32) -> (Option<i32>, Vec<(i32, i32)>) {
        let mut map: SortedVecMap<i32, i32> = initial.into_iter().collect();
        let ret = map.remove(&to_remove);
        (ret, map.vec)
    }

    #[test_case(vec![(56, 56), (47, 47), (53, 53), (51, 51), (49, 49)]; "normal_map")]
    #[test_case(vec![]; "empty_map")]
    fn test_serialize_deserialize(input: Vec<(i32, i32)>) {
        let map: SortedVecMap<i32, i32> = input.into_iter().collect();
        let serialized = serde_json::to_vec(&map).unwrap();
        let deserialized: SortedVecMap<i32, i32> = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(map, deserialized);
    }

    #[test_case(vec![(56, 56), (47, 47), (53, 53), (51, 51), (49, 49)]; "normal_map")]
    #[test_case(vec![]; "empty_map")]
    fn test_deserialize_from_btree_map(input: Vec<(i32, i32)>) {
        let map: BTreeMap<i32, i32> = input.into_iter().collect();
        let serialized = serde_json::to_vec(&map).unwrap();
        let deserialized: SortedVecMap<i32, i32> = serde_json::from_slice(&serialized).unwrap();

        let map_entries: Vec<(i32, i32)> = map.into_iter().collect();
        assert_eq!(map_entries, deserialized.vec);
    }

    #[derive(Debug, Clone, Eq)]
    struct TestKey {
        id: i32,
        metadata: &'static str,
    }

    impl PartialEq for TestKey {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }

    impl Ord for TestKey {
        fn cmp(&self, other: &Self) -> Ordering {
            self.id.cmp(&other.id)
        }
    }

    impl PartialOrd for TestKey {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    #[test]
    fn test_insert_keeps_old_key() {
        let mut map = SortedVecMap::new();
        let key1 = TestKey { id: 1, metadata: "old" };
        let key2 = TestKey { id: 1, metadata: "new" };

        map.insert(key1.clone(), 10);
        map.insert(key2, 20);

        let (k, v) = map.iter().next().unwrap();
        assert_eq!(k.metadata, "old");
        assert_eq!(*v, 20); // Value is updated
    }

    #[test]
    fn test_builder_keeps_new_value_old_key_consecutive() {
        let mut builder = SortedVecMapBuilder::new();
        let key1 = TestKey { id: 1, metadata: "old" };
        let key2 = TestKey { id: 1, metadata: "new" };
        builder.insert(key1, 10);
        builder.insert(key2, 20);
        let map = builder.build();
        let (k, v) = map.iter().next().unwrap();
        assert_eq!(k.metadata, "old");
        assert_eq!(*v, 20);
    }

    #[test]
    fn test_builder_keeps_new_value_old_key_unsorted() {
        let mut builder = SortedVecMapBuilder::new();
        let key1 = TestKey { id: 2, metadata: "two" };
        let key2 = TestKey { id: 1, metadata: "old" };
        let key3 = TestKey { id: 1, metadata: "new" };
        builder.insert(key1, 30);
        builder.insert(key2, 10);
        builder.insert(key3, 20);
        let map = builder.build();
        let mut iter = map.iter();
        let (k, v) = iter.next().unwrap();
        assert_eq!(k.id, 1);
        assert_eq!(k.metadata, "old");
        assert_eq!(*v, 20);
    }

    #[test_case([]; "empty_map")]
    #[test_case([(5, 50), (1, 10), (3, 30), (5, 55), (2, 20)]; "multiple_items")]
    fn test_from_iter<const N: usize>(entries: [(i32, i32); N]) {
        let map = SortedVecMap::from_iter(entries);
        assert_eq!(
            map.iter().collect::<Vec<_>>(),
            BTreeMap::from(entries).iter().collect::<Vec<_>>(),
        );
    }

    #[test_case([], []; "empty_map")]
    #[test_case([(1, 10)], [(5, 50), (1, 10), (3, 30), (5, 55), (2, 20)]; "multiple_items")]
    fn test_extend<const N: usize, const M: usize>(
        initial: [(i32, i32); N],
        entries: [(i32, i32); M],
    ) {
        let mut map = SortedVecMap::from(initial);
        map.extend(entries);

        assert_eq!(
            map.iter().collect::<Vec<_>>(),
            BTreeMap::from_iter(initial.into_iter().chain(entries)).iter().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn test_range() {
        let entries = [(1, 10), (3, 30), (5, 50), (7, 70)];
        let map = SortedVecMap::from(entries);
        let expected = BTreeMap::from(entries);

        for range in [
            // ..
            (Bound::Unbounded, Bound::Unbounded),
            // 0..
            (Bound::Included(0), Bound::Unbounded),
            // 1..
            (Bound::Included(1), Bound::Unbounded),
            // 2..
            (Bound::Included(2), Bound::Unbounded),
            // 8..
            (Bound::Included(8), Bound::Unbounded),
            // 1..7
            (Bound::Included(1), Bound::Excluded(7)),
            // 2..6
            (Bound::Included(2), Bound::Excluded(6)),
            // 3..5
            (Bound::Included(3), Bound::Excluded(5)),
            // 8..10
            (Bound::Included(8), Bound::Excluded(10)),
            // 0..=8
            (Bound::Included(0), Bound::Included(8)),
            // 1..=7
            (Bound::Included(1), Bound::Included(7)),
            // 3..=5
            (Bound::Included(3), Bound::Included(5)),
            (Bound::Excluded(2), Bound::Unbounded),
            (Bound::Excluded(3), Bound::Excluded(7)),
        ] {
            assert_eq!(
                map.range(range.clone()).collect::<Vec<_>>(),
                expected.range(range).collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn test_string_prefixes() {
        let entries = [
            ("meta/", 1),
            ("meta/a", 2),
            ("meta/b", 3),
            ("meta/c", 4),
            ("meta/dir/a", 5),
            ("meta/dir/b", 6),
            ("meta/dir/c", 7),
            ("meta/e", 8),
            ("metb/", 9),
        ];
        let map = SortedVecMap::from(entries);
        let expected = BTreeMap::from(entries);

        for range in [
            // ..
            (Bound::Unbounded, Bound::Unbounded),
            // "meta/"..
            (Bound::Included("meta/"), Bound::Unbounded),
            // "meta/c"..
            (Bound::Included("meta/c"), Bound::Unbounded),
            // "meta/dir/"..
            (Bound::Included("meta/dir/"), Bound::Unbounded),
            // "meta/dir/".."meta/dir/\u{10FFFF}"
            (Bound::Included("meta/dir/"), Bound::Included("meta/dir/\u{10FFFF}")),
        ] {
            assert_eq!(
                map.range::<str, _>(range.clone()).collect::<Vec<_>>(),
                expected.range::<str, _>(range).collect::<Vec<_>>(),
            );
        }
    }
}
