// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A memory-efficient map optimized for dynamically sized keys.
//!
//! `PackedMap` manages its keys sequentially in a contiguous region of memory through
//! `PackedVec`, mapping them to values stored in a conventional `Vec`. Because it
//! stores keys as a single allocation, it cannot be modified after construction.

use crate::packed_map_builder::PackedMapBuilder;
use crate::{PackedItem, PackedVec};
use std::borrow::Borrow;
use std::fmt::Debug;
use std::iter::FromIterator;

/// A map with keys stored in a sorted `PackedVec<K>` and values in a `Vec<V>`.
///
/// This map is optimized for reducing memory usage by using a packed representation
/// for keys. It does not support modification after creation.
pub struct PackedMap<K: ?Sized + Ord + PackedItem, V> {
    pub(crate) keys: PackedVec<K>,
    pub(crate) values: Vec<V>,
}

impl<K, V> PackedMap<K, V>
where
    K: ?Sized + Ord + PackedItem,
{
    /// Creates a new `PackedMapBuilder`.
    pub fn builder() -> PackedMapBuilder<K, V>
    where
        K: ToOwned,
        K::Owned: Ord,
    {
        PackedMapBuilder::new()
    }

    /// Creates a new `PackedMapBuilder` with the specified capacities.
    ///
    /// The `element_capacity` argument specifies the number of slices that can be
    /// stored without reallocating the offsets vector. The `buffer_capacity`
    /// argument specifies the cumulative length of slices that can be stored
    /// without reallocating the data vector.
    pub fn builder_with_capacity(
        element_capacity: usize,
        buffer_capacity: usize,
    ) -> PackedMapBuilder<K, V>
    where
        K: ToOwned,
        K::Owned: Ord,
    {
        PackedMapBuilder::with_capacity(element_capacity, buffer_capacity)
    }

    /// Creates a new empty `PackedMap`.
    pub fn new() -> Self {
        Self { keys: PackedVec::new(), values: vec![] }
    }

    /// Creates a new `PackedMap` with the specified capacities.
    ///
    /// The `element_capacity` argument specifies the number of slices that can be
    /// stored without reallocating the offsets vector. The `buffer_capacity`
    /// argument specifies the cumulative length of slices that can be stored
    /// without reallocating the data vector.
    pub fn with_capacity(element_capacity: usize, buffer_capacity: usize) -> Self {
        Self {
            keys: PackedVec::with_capacity(element_capacity, buffer_capacity),
            values: Vec::with_capacity(element_capacity),
        }
    }

    pub(crate) fn from_parts(keys: PackedVec<K>, values: Vec<V>) -> Self {
        Self { keys, values }
    }

    /// Returns true if the map contains no elements.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Returns the number of elements in the map.
    pub fn element_len(&self) -> usize {
        self.keys.len()
    }

    /// Returns the cumulative length of all keys in the map in bytes.
    pub fn buffer_len(&self) -> usize {
        self.keys.buffer_len()
    }

    /// Returns a reference to the value corresponding to the key.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if let Ok(index) = self.index_of(key) { Some(&self.values[index]) } else { None }
    }

    /// Returns a mutable reference to the value corresponding to the key.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if let Ok(index) = self.index_of(key) { Some(&mut self.values[index]) } else { None }
    }

    /// Returns true if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.index_of(key).is_ok()
    }

    /// Inserts a key-value pair into the map.
    ///
    /// If the key is already present, updates the value and returns `Ok(Some(old_value))`.
    /// If the key is greater than the last key, appends it and returns `Ok(None)`.
    /// Otherwise, returns `Err(value)`.
    ///
    /// # Complexity
    ///
    /// - `O(1)` if the key is greater than or equal to the last key in the map.
    /// - `O(log N)` otherwise, where `N` is the number of elements in the map.
    pub fn append_or_update(&mut self, key: &K, value: V) -> Result<Option<V>, V> {
        // Fast path for appends and updates to the last element. This avoids a
        // binary search when elements are inserted in sorted order.
        if let Some(last) = self.keys.last() {
            match key.cmp(last) {
                std::cmp::Ordering::Greater => {
                    self.keys.push(key);
                    self.values.push(value);
                    return Ok(None);
                }
                std::cmp::Ordering::Equal => {
                    let old_value = std::mem::replace(self.values.last_mut().unwrap(), value);
                    return Ok(Some(old_value));
                }
                std::cmp::Ordering::Less => {}
            }
        } else {
            self.keys.push(key);
            self.values.push(value);
            return Ok(None);
        }

        match self.keys.binary_search(key) {
            Ok(idx) => {
                let old_value = std::mem::replace(&mut self.values[idx], value);
                Ok(Some(old_value))
            }
            Err(_) => Err(value),
        }
    }

    /// Shrinks the capacity of the map as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.keys.shrink_to_fit();
        self.values.shrink_to_fit();
    }

    /// Returns an iterator over the entries of the map, sorted by key.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter { keys: self.keys.iter(), values: self.values.iter() }
    }

    /// Returns an iterator over the entries of the map, sorted by key, with mutable values.
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut { keys: self.keys.iter(), values: self.values.iter_mut() }
    }

    /// Drains all elements from the map, returning a lending iterator that yields them.
    ///
    /// The elements are yielded in sorted order by key.
    /// After the iterator is dropped, the map is left empty.
    ///
    /// # Leaked
    ///
    /// If the returned `Drain` iterator is leaked (e.g. via `std::mem::forget`),
    /// any elements that have not been yielded yet will be leaked (their destructors
    /// will not run). However, memory safety is still preserved as the map will be left empty.
    pub fn drain(&mut self) -> Drain<'_, K, V> {
        let keys = self.keys.drain();
        let values = self.values.drain(..);
        Drain { keys, values }
    }

    /// Constructs a double-ended iterator over a sub-range of elements in the map.
    pub fn range<'a, Q, R>(&'a self, range: R) -> Range<'a, K, V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized + 'a,
        R: std::ops::RangeBounds<&'a Q>,
    {
        let indices =
            crate::compute_range_indices(self.element_len(), range, |&key| self.index_of(key));
        Range { keys: self.keys.range(indices.clone()), values: self.values[indices].iter() }
    }

    /// Constructs a mutable double-ended iterator over a sub-range of elements in the map.
    pub fn range_mut<'a, Q, R>(&'a mut self, range: R) -> RangeMut<'a, K, V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized + 'a,
        R: std::ops::RangeBounds<&'a Q>,
    {
        let indices =
            crate::compute_range_indices(self.element_len(), range, |&key| self.index_of(key));
        RangeMut { keys: self.keys.range(indices.clone()), values: self.values[indices].iter_mut() }
    }

    /// Returns an iterator over the keys of the map, in sorted order.
    pub fn keys(&self) -> Keys<'_, K> {
        Keys { inner: self.keys.iter() }
    }

    /// Returns an iterator over the values of the map, sorted by key.
    pub fn values(&self) -> Values<'_, V> {
        Values { inner: self.values.iter() }
    }

    /// Returns a mutable iterator over the values of the map, sorted by key.
    pub fn values_mut(&mut self) -> ValuesMut<'_, V> {
        ValuesMut { inner: self.values.iter_mut() }
    }

    /// Returns an iterator that takes ownership of the map's values, sorted by key.
    pub fn into_values(self) -> IntoValues<V> {
        IntoValues { inner: self.values.into_iter() }
    }

    fn index_of<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.keys.binary_search_by(|probe| probe.borrow().cmp(key))
    }
}

impl<K, V, Q> std::ops::Index<&Q> for PackedMap<K, V>
where
    K: ?Sized + Ord + PackedItem + Borrow<Q>,
    Q: Ord + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &Self::Output {
        self.get(key).expect("no entry found for key")
    }
}

// This manual implementation avoids an extra constraint of `K: Default` since we've
// encoded the keys into a byte string.
impl<K: ?Sized + Ord + PackedItem, V> Default for PackedMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

// This manual implementation avoids an extra constraint of `K: Clone` since
// we've encoded the keys into a byte string.
impl<K: ?Sized + Ord + PackedItem, V: Clone> Clone for PackedMap<K, V> {
    fn clone(&self) -> Self {
        Self { keys: self.keys.clone(), values: self.values.clone() }
    }
}

// This manual implementation avoids an extra constraint of `K: PartialEq` since
// we've encoded the keys into a byte string.
impl<K: ?Sized + Ord + PackedItem, V: PartialEq> PartialEq for PackedMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.keys == other.keys && self.values == other.values
    }
}

// This manual implementation avoids an extra constraint of `K: Eq` since we've
// encoded the keys into a byte string.
impl<K: ?Sized + Ord + PackedItem, V: Eq> Eq for PackedMap<K, V> {}

// This manual implementation avoids an extra constraint of `K: Hash` since we've
// encoded the keys into a byte string.
impl<K: ?Sized + Ord + PackedItem, V: std::hash::Hash> std::hash::Hash for PackedMap<K, V> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.keys.hash(state);
        self.values.hash(state);
    }
}

impl<K: ?Sized + Ord + PackedItem, V: Debug> Debug for PackedMap<K, V>
where
    K: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

pub struct Iter<'a, K: ?Sized + Ord + PackedItem, V> {
    keys: crate::packed_vec::Iter<'a, K>,
    values: std::slice::Iter<'a, V>,
}

impl<'a, K: ?Sized + Ord + PackedItem, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.keys.next()?;
        let value = self.values.next()?;
        Some((key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.keys.size_hint()
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> DoubleEndedIterator for Iter<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let key = self.keys.next_back()?;
        let value = self.values.next_back()?;
        Some((key, value))
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> ExactSizeIterator for Iter<'a, K, V> {}

pub struct IterMut<'a, K: ?Sized + Ord + PackedItem, V> {
    keys: crate::packed_vec::Iter<'a, K>,
    values: std::slice::IterMut<'a, V>,
}

impl<'a, K: ?Sized + Ord + PackedItem, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.keys.next()?;
        let value = self.values.next()?;
        Some((key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.keys.size_hint()
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> DoubleEndedIterator for IterMut<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let key = self.keys.next_back()?;
        let value = self.values.next_back()?;
        Some((key, value))
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> ExactSizeIterator for IterMut<'a, K, V> {}

pub struct Range<'a, K: ?Sized + Ord + PackedItem, V> {
    keys: crate::packed_vec::Iter<'a, K>,
    values: std::slice::Iter<'a, V>,
}

impl<'a, K: ?Sized + Ord + PackedItem, V> Iterator for Range<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.keys.next()?;
        let value = self.values.next()?;
        Some((key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.keys.size_hint()
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> DoubleEndedIterator for Range<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let key = self.keys.next_back()?;
        let value = self.values.next_back()?;
        Some((key, value))
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> ExactSizeIterator for Range<'a, K, V> {}

pub struct RangeMut<'a, K: ?Sized + Ord + PackedItem, V> {
    keys: crate::packed_vec::Iter<'a, K>,
    values: std::slice::IterMut<'a, V>,
}

impl<'a, K: ?Sized + Ord + PackedItem, V> Iterator for RangeMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.keys.next()?;
        let value = self.values.next()?;
        Some((key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.keys.size_hint()
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> DoubleEndedIterator for RangeMut<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let key = self.keys.next_back()?;
        let value = self.values.next_back()?;
        Some((key, value))
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> ExactSizeIterator for RangeMut<'a, K, V> {}

/// An iterator over the keys of a `PackedMap`.
pub struct Keys<'a, K: ?Sized + PackedItem> {
    inner: crate::packed_vec::Iter<'a, K>,
}

impl<'a, K: ?Sized + PackedItem> Iterator for Keys<'a, K> {
    type Item = &'a K;

    fn next(&mut self) -> Option<&'a K> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K: ?Sized + PackedItem> DoubleEndedIterator for Keys<'a, K> {
    fn next_back(&mut self) -> Option<&'a K> {
        self.inner.next_back()
    }
}

impl<'a, K: ?Sized + PackedItem> ExactSizeIterator for Keys<'a, K> {}

/// An iterator over the values of a `PackedMap`.
pub struct Values<'a, V> {
    inner: std::slice::Iter<'a, V>,
}

impl<'a, V> Iterator for Values<'a, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<&'a V> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, V> DoubleEndedIterator for Values<'a, V> {
    fn next_back(&mut self) -> Option<&'a V> {
        self.inner.next_back()
    }
}

impl<'a, V> ExactSizeIterator for Values<'a, V> {}

/// A mutable iterator over the values of a `PackedMap`.
pub struct ValuesMut<'a, V> {
    inner: std::slice::IterMut<'a, V>,
}

impl<'a, V> Iterator for ValuesMut<'a, V> {
    type Item = &'a mut V;

    fn next(&mut self) -> Option<&'a mut V> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, V> DoubleEndedIterator for ValuesMut<'a, V> {
    fn next_back(&mut self) -> Option<&'a mut V> {
        self.inner.next_back()
    }
}

impl<'a, V> ExactSizeIterator for ValuesMut<'a, V> {}

/// An iterator that takes ownership of a `PackedMap`'s values.
pub struct IntoValues<V> {
    inner: std::vec::IntoIter<V>,
}

impl<V> Iterator for IntoValues<V> {
    type Item = V;

    fn next(&mut self) -> Option<V> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<V> DoubleEndedIterator for IntoValues<V> {
    fn next_back(&mut self) -> Option<V> {
        self.inner.next_back()
    }
}

impl<V> ExactSizeIterator for IntoValues<V> {}

impl<'a, K: ?Sized + Ord + PackedItem, V> IntoIterator for &'a PackedMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K: ?Sized + Ord + PackedItem, V> IntoIterator for &'a mut PackedMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// A draining lending iterator for `PackedMap<K, V>`.
///
/// This `struct` is created by `PackedMap::drain`. See its documentation for more.
pub struct Drain<'a, K: ?Sized + Ord + PackedItem, V> {
    keys: crate::packed_vec::Drain<'a, K>,
    values: std::vec::Drain<'a, V>,
}

impl<'a, K: ?Sized + Ord + PackedItem, V> Drain<'a, K, V> {
    /// Returns the next element in the draining iterator.
    pub fn next(&mut self) -> Option<(&K, V)> {
        let key = self.keys.next()?;
        let value = self.values.next()?;
        Some((key, value))
    }

    /// Returns the next element from the back in the draining iterator.
    pub fn next_back(&mut self) -> Option<(&K, V)> {
        let key = self.keys.next_back()?;
        let value = self.values.next_back()?;
        Some((key, value))
    }

    /// Returns the number of elements remaining in the draining iterator.
    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

impl<K, V, U> FromIterator<(U, V)> for PackedMap<K, V>
where
    K: ?Sized + Ord + PackedItem + ToOwned,
    U: AsRef<K>,
    K::Owned: Ord,
{
    fn from_iter<T: IntoIterator<Item = (U, V)>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut builder = PackedMapBuilder::with_capacity(lower, 0);
        for (k, v) in iter {
            builder.insert(k.as_ref(), v);
        }
        builder.build()
    }
}

impl<K, V, U, const N: usize> From<[(U, V); N]> for PackedMap<K, V>
where
    K: ?Sized + Ord + PackedItem + ToOwned,
    U: AsRef<K>,
    K::Owned: Ord,
{
    fn from(arr: [(U, V); N]) -> Self {
        Self::from_iter(arr)
    }
}

impl<K: ?Sized + Ord + PackedItem, V> serde::Serialize for PackedMap<K, V>
where
    K: serde::Serialize,
    V: serde::Serialize,
{
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.element_len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de, K, V> serde::Deserialize<'de> for PackedMap<K, V>
where
    K: ?Sized + PackedItem + Ord + ToOwned,
    Box<K>: serde::Deserialize<'de> + Ord + AsRef<K>,
    V: serde::Deserialize<'de>,
    K::Owned: Ord,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<K: ?Sized + Ord + PackedItem, V>(
            std::marker::PhantomData<fn() -> (*const K, V)>,
        );

        impl<'de, K, V> serde::de::Visitor<'de> for MapVisitor<K, V>
        where
            K: ?Sized + PackedItem + Ord + ToOwned,
            Box<K>: serde::Deserialize<'de> + Ord + AsRef<K>,
            V: serde::Deserialize<'de>,
            K::Owned: Ord,
        {
            type Value = PackedMap<K, V>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map of items")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let size = map.size_hint().unwrap_or(0);
                let mut builder = PackedMapBuilder::with_capacity(size, 0);
                while let Some((k, v)) = map.next_entry::<Box<K>, V>()? {
                    builder.insert(k.as_ref(), v);
                }
                Ok(builder.build())
            }
        }

        deserializer.deserialize_map(MapVisitor(std::marker::PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::ops::Bound;

    #[test]
    fn test_from_iter_and_get() {
        let entries = vec![("c", 3), ("a", 1), ("b", 2), ("a", 100)];
        let map: PackedMap<str, i32> = entries.into_iter().collect();

        assert_eq!(map.element_len(), 3);
        assert_eq!(map.get("a"), Some(&100)); // Last one kept
        assert_eq!(map.get("b"), Some(&2));
        assert_eq!(map.get("c"), Some(&3));
        assert_eq!(map.get("d"), None);
    }

    #[test]
    fn test_append_or_update() {
        let mut map: PackedMap<str, i32> = PackedMap::new();
        assert_eq!(map.append_or_update("a", 1), Ok(None));
        assert_eq!(map.append_or_update("b", 2), Ok(None));
        assert_eq!(map.append_or_update("b", 20), Ok(Some(2))); // Update
        assert_eq!(map.append_or_update("a", 10), Ok(Some(1))); // Update existing key
        assert_eq!(map.append_or_update("aa", 5), Err(5)); // Out of order

        assert_eq!(map.get("a"), Some(&10));
        assert_eq!(map.get("b"), Some(&20));
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
        fn cmp(&self, other: &Self) -> Ordering {
            self.id.cmp(&other.id)
        }
    }

    impl PartialOrd for TestKey {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    impl PackedItem for TestKey {
        unsafe fn from_bytes(bytes: &[u8]) -> &Self {
            unsafe { &*(bytes.as_ptr() as *const TestKey) }
        }
    }

    #[test]
    fn test_append_or_update_keeps_old_key() {
        let mut map = PackedMap::new();
        let key1 = TestKey { id: 1, metadata: 10 };
        let key2 = TestKey { id: 1, metadata: 20 };

        map.append_or_update(&key1, 1).unwrap();
        map.append_or_update(&key2, 2).unwrap(); // Equal, should go to packed

        let items = map.iter().collect::<Vec<_>>();
        assert_eq!(items.len(), 1);

        let (k, v) = &items[0];
        assert_eq!(k.metadata, 10);
        assert_eq!(**v, 2);
    }

    #[test]
    fn test_iter() {
        let entries = vec![("b", 2), ("a", 1)];
        let map: PackedMap<str, i32> = entries.into_iter().collect();
        let collected: Vec<_> = map.iter().collect();
        assert_eq!(collected, vec![("a", &1), ("b", &2)]);
    }

    #[test]
    fn test_duplicate_keys() {
        let entries = vec![("a", 1), ("b", 2), ("a", 3), ("c", 4), ("b", 5)];
        let map: PackedMap<str, i32> = entries.into_iter().collect();

        assert_eq!(map.element_len(), 3);
        // Should keep the last value encountered for each key due to reverse then stable sort
        assert_eq!(map.get("a"), Some(&3));
        assert_eq!(map.get("b"), Some(&5));
        assert_eq!(map.get("c"), Some(&4));

        let collected: Vec<_> = map.iter().collect();
        assert_eq!(collected, vec![("a", &3), ("b", &5), ("c", &4)]);
    }

    #[test]
    fn test_drain() {
        let entries = vec![("a", 1), ("b", 2), ("c", 3)];
        let mut map: PackedMap<str, i32> = entries.into_iter().collect();

        let mut drain = map.drain();
        assert_eq!(drain.len(), 3);

        // Test next()
        assert_eq!(drain.next(), Some(("a", 1)));
        assert_eq!(drain.len(), 2);

        // Test next_back()
        assert_eq!(drain.next_back(), Some(("c", 3)));
        assert_eq!(drain.len(), 1);

        // Test next() again
        assert_eq!(drain.next(), Some(("b", 2)));
        assert_eq!(drain.len(), 0);

        // Test None
        assert_eq!(drain.next(), None);
        assert_eq!(drain.next_back(), None);
    }

    #[test]
    fn test_drain_drop_clears() {
        let entries = vec![("a", 1), ("b", 2)];
        let mut map: PackedMap<str, i32> = entries.into_iter().collect();

        {
            let mut drain = map.drain();
            assert_eq!(drain.next(), Some(("a", 1)));
            // Drop happens here
        }

        assert!(map.is_empty());
        assert_eq!(map.element_len(), 0);

        // Make sure both vectors were cleared, not just one.
        assert!(map.keys.is_empty());
        assert!(map.values.is_empty());
    }

    #[test]
    fn test_range() {
        let entries = vec![("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")];
        let map: PackedMap<str, &str> = entries.into_iter().collect();

        let items: Vec<_> = map.range("b"..).map(|(k, v)| (k, *v)).collect();
        assert_eq!(items, vec![("b", "B"), ("c", "C"), ("d", "D")]);

        let items: Vec<_> = map.range(.."c").map(|(k, v)| (k, *v)).collect();
        assert_eq!(items, vec![("a", "A"), ("b", "B")]);

        let items: Vec<_> = map.range(..="c").map(|(k, v)| (k, *v)).collect();
        assert_eq!(items, vec![("a", "A"), ("b", "B"), ("c", "C")]);

        let items: Vec<_> = map.range("b"..="c").map(|(k, v)| (k, *v)).collect();
        assert_eq!(items, vec![("b", "B"), ("c", "C")]);

        let items: Vec<_> = map.range("e"..).map(|(k, v)| (k, *v)).collect();
        assert_eq!(items, vec![]);

        let items: Vec<_> = map.range("a1".."c1").map(|(k, v)| (k, *v)).collect();
        assert_eq!(items, vec![("b", "B"), ("c", "C")]);

        let prefix_entries = vec![("dir/a", "A"), ("dir/b", "B"), ("file", "F")];
        let prefix_map: PackedMap<str, &str> = prefix_entries.into_iter().collect();
        let items: Vec<_> = prefix_map.range("dir/".."dir0").map(|(k, v)| (k, *v)).collect();
        assert_eq!(items, vec![("dir/a", "A"), ("dir/b", "B")]);
    }

    #[test]
    fn test_range_mut() {
        let entries = vec![("a", "a".to_string()), ("b", "b".to_string()), ("c", "c".to_string())];
        let mut map: PackedMap<str, String> = entries.into_iter().collect();

        for (_, v) in map.range_mut("b"..) {
            v.push('x');
        }

        for (_, v) in map.range_mut("a1".."c1") {
            v.push('y');
        }

        let items: Vec<_> = map.iter().map(|(k, v)| (k, v.clone())).collect();
        assert_eq!(
            items,
            vec![("a", "a".to_string()), ("b", "bxy".to_string()), ("c", "cxy".to_string())]
        );

        let mut prefix_map: PackedMap<str, String> =
            vec![("dir/a", "A".to_string()), ("dir/b", "B".to_string()), ("file", "F".to_string())]
                .into_iter()
                .collect();

        for (_, v) in prefix_map.range_mut("dir/".."dir0") {
            v.push('x');
        }
        let items: Vec<_> = prefix_map.iter().map(|(k, v)| (k, v.clone())).collect();
        assert_eq!(
            items,
            vec![
                ("dir/a", "Ax".to_string()),
                ("dir/b", "Bx".to_string()),
                ("file", "F".to_string()),
            ]
        );
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
        let map: PackedMap<str, i32> = entries.into_iter().collect();

        let mut it = map.range("meta/"..);
        assert_eq!(it.next(), Some(("meta/", &1)));
        assert_eq!(it.next(), Some(("meta/a", &2)));
        assert_eq!(it.next(), Some(("meta/b", &3)));
        assert_eq!(it.next(), Some(("meta/c", &4)));
        assert_eq!(it.next(), Some(("meta/dir/a", &5)));
        assert_eq!(it.next(), Some(("meta/dir/b", &6)));
        assert_eq!(it.next(), Some(("meta/dir/c", &7)));
        assert_eq!(it.next(), Some(("meta/e", &8)));
        assert_eq!(it.next(), Some(("metb/", &9)));
        assert_eq!(it.next(), None);

        let mut it = map.range((Bound::Excluded("meta/"), Bound::Unbounded));
        assert_eq!(it.next(), Some(("meta/a", &2)));

        let mut it = map.range("meta/"..);
        assert_eq!(it.next(), Some(("meta/", &1)));

        let mut it = map.range("meta/dir/"..);
        assert_eq!(it.next(), Some(("meta/dir/a", &5)));

        let mut it = map.range((Bound::Excluded("meta/dir/"), Bound::Unbounded));
        assert_eq!(it.next(), Some(("meta/dir/a", &5)));
    }

    #[test]
    fn test_serde() {
        let entries = vec![("a", 1), ("b", 2), ("c", 3)];
        let map: PackedMap<str, i32> = entries.into_iter().collect();
        let serialized = serde_json::to_string(&map).unwrap();
        let deserialized: PackedMap<str, i32> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(map, deserialized);
    }
}
