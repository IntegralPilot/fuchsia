// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::SortedVecMap;
use crate::sorted_vec_map::{Iter as MapIter, Keys};
use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::ops::{Bound, RangeBounds};

/// An ordered set built on a `SortedVecMap`.
///
/// `SortedVecSet` provides a memory-efficient alternative to `BTreeSet` by wrapping
/// `SortedVecMap<T, ()>`.
///
/// # Complexity
///
/// | Operation | Time Complexity | Space Complexity |
/// |---|---|---|
/// | `new` / `with_capacity` | `O(1)` | `O(1)` |
/// | `contains` / `get` | `O(log N)` | `O(1)` |
/// | `insert` | `O(N)` | `O(1)` amortized |
/// | `remove` | `O(N)` | `O(1)` |
/// | `union` / `difference` | `O(N + M)` | `O(1)` |
///
/// # When to use
///
/// - Similar to `SortedVecMap`, it is ideal for read-heavy, memory-constrained sets.
#[derive(Eq, PartialEq, PartialOrd, Ord, Hash, Clone)]
pub struct SortedVecSet<T> {
    map: SortedVecMap<T, ()>,
}

impl<T> SortedVecSet<T> {
    /// Returns a new builder for `SortedVecSet`.
    pub fn builder() -> SortedVecSetBuilder<T> {
        SortedVecSetBuilder::new()
    }

    /// Returns a new builder for `SortedVecSet` with at least the specified capacity.
    pub fn builder_with_capacity(capacity: usize) -> SortedVecSetBuilder<T> {
        SortedVecSetBuilder::with_capacity(capacity)
    }

    /// Constructs a new, empty `SortedVecSet`.
    pub fn new() -> Self {
        Self { map: SortedVecMap::new() }
    }

    /// Returns the number of elements the set can hold without reallocating.
    pub fn capacity(&self) -> usize {
        self.map.capacity()
    }

    /// Constructs a new, empty `SortedVecSet` with at least the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { map: SortedVecMap::with_capacity(capacity) }
    }

    /// Reserves capacity for at least `additional` more elements in the `SortedVecSet`.
    pub fn reserve(&mut self, additional: usize) {
        self.map.reserve(additional);
    }

    /// Shrinks the capacity of the set as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.map.shrink_to_fit();
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns true if there are no elements in the set.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Returns true if the set contains the given value.
    ///
    /// Complexity: `O(log n)` time.
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        T: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        self.map.contains_key(value)
    }

    /// Returns a reference to the value in the set, if any, that is equal to the given value.
    ///
    /// Complexity: `O(log n)` time.
    pub fn get<Q>(&self, value: &Q) -> Option<&T>
    where
        T: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        self.map.range((Bound::Included(value), Bound::Included(value))).next().map(|(k, _v)| k)
    }

    /// Inserts a value into the set. Returns true if the value was not already present.
    ///
    /// Complexity: `O(log n)` search time, plus `O(n)` time to insert the element if it is not present.
    pub fn insert(&mut self, value: T) -> bool
    where
        T: Ord,
    {
        self.map.insert(value, ()).is_none()
    }

    /// Removes a value from the set. Returns true if the value was present.
    ///
    /// Complexity: `O(log n)` search time, plus `O(n)` time to remove the element if it is present.
    pub fn remove<Q>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
    {
        self.map.remove(value).is_some()
    }

    /// Returns an iterator over the elements of the set, in sorted order.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter { inner: self.map.keys() }
    }

    /// Constructs a double-ended iterator over a sub-range of elements in the set.
    ///
    /// Complexity: `O(log n)` to find the start and end of the range.
    pub fn range<Q, R>(&self, range: R) -> Range<'_, T>
    where
        T: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
        R: RangeBounds<Q>,
    {
        Range { inner: self.map.range(range) }
    }

    /// Returns an iterator yielding elements from both sets in sorted order, without duplicates.
    ///
    /// Complexity: `O(N + M)` where N is the number of elements in `self` and M is the number of elements in `other`.
    pub fn union<'a>(&'a self, other: &'a Self) -> Union<'a, T>
    where
        T: Ord,
    {
        let mut iter1 = self.iter();
        let mut iter2 = other.iter();
        Union { next1: iter1.next(), next2: iter2.next(), iter1, iter2 }
    }

    /// Returns an iterator yielding elements in `self` that are not in `other`.
    ///
    /// Complexity: `O(N + M)` where N is the number of elements in `self` and M is the number of elements in `other`.
    pub fn difference<'a>(&'a self, other: &'a Self) -> Difference<'a, T>
    where
        T: Ord,
    {
        let mut iter1 = self.iter();
        let mut iter2 = other.iter();
        Difference { next1: iter1.next(), next2: iter2.next(), iter1, iter2 }
    }
}

impl<T: Debug> Debug for SortedVecSet<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<T> Default for SortedVecSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<SortedVecSet<T>> for Vec<T> {
    fn from(set: SortedVecSet<T>) -> Self {
        set.into_iter().collect()
    }
}

impl<T: Ord> From<Vec<T>> for SortedVecSet<T> {
    fn from(vec: Vec<T>) -> Self {
        Self::from_iter(vec)
    }
}

impl<T: Ord, const N: usize> From<[T; N]> for SortedVecSet<T> {
    fn from(arr: [T; N]) -> Self {
        Self::from_iter(arr)
    }
}

impl<T: Ord> From<BTreeSet<T>> for SortedVecSet<T> {
    fn from(set: BTreeSet<T>) -> Self {
        Self::from_iter(set)
    }
}

/// An iterator over the items of a `SortedVecSet`.
pub struct Iter<'a, T> {
    inner: Keys<'a, T, ()>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

/// An owning iterator over the items of a `SortedVecSet`.
pub struct IntoIter<T> {
    inner: std::vec::IntoIter<(T, ())>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, _v)| k)
    }
}

impl<T> IntoIterator for SortedVecSet<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { inner: self.map.into_iter() }
    }
}

impl<'a, T> IntoIterator for &'a SortedVecSet<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Ord> FromIterator<T> for SortedVecSet<T> {
    /// Constructs a `SortedVecSet` from an iterator.
    ///
    /// Complexity: `O(n log n)` where n is the number of elements in the iterator,
    /// or `O(n)` if the elements are already sorted.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        SortedVecSetBuilder::from_iter(iter).build()
    }
}

impl<T: Ord> Extend<T> for SortedVecSet<T> {
    /// Extends the set with the contents of an iterator.
    ///
    /// Complexity: `O(n log n)` where n is the total number of elements, or `O(n)` if the
    /// iterator yields elements in sorted order and they are all greater than the existing
    /// elements.
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.map.extend(iter.into_iter().map(|v| (v, ())));
    }
}

/// An iterator over a sub-range of items in a `SortedVecSet`.
pub struct Range<'a, T> {
    inner: MapIter<'a, T, ()>,
}

impl<'a, T> Iterator for Range<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, _v)| k)
    }
}

impl<'a, T> DoubleEndedIterator for Range<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back().map(|(k, _v)| k)
    }
}

/// An iterator yielding elements from the union of two sets.
pub struct Union<'a, T> {
    iter1: Iter<'a, T>,
    iter2: Iter<'a, T>,
    next1: Option<&'a T>,
    next2: Option<&'a T>,
}

impl<'a, T: Ord> Iterator for Union<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.next1, self.next2) {
            (Some(v1), Some(v2)) => match v1.cmp(v2) {
                std::cmp::Ordering::Less => {
                    let res = v1;
                    self.next1 = self.iter1.next();
                    Some(res)
                }
                std::cmp::Ordering::Equal => {
                    let res = v1;
                    self.next1 = self.iter1.next();
                    self.next2 = self.iter2.next();
                    Some(res)
                }
                std::cmp::Ordering::Greater => {
                    let res = v2;
                    self.next2 = self.iter2.next();
                    Some(res)
                }
            },
            (Some(v1), None) => {
                let res = v1;
                self.next1 = self.iter1.next();
                Some(res)
            }
            (None, Some(v2)) => {
                let res = v2;
                self.next2 = self.iter2.next();
                Some(res)
            }
            (None, None) => None,
        }
    }
}

/// An iterator yielding elements in one set that are not in another.
pub struct Difference<'a, T> {
    iter1: Iter<'a, T>,
    iter2: Iter<'a, T>,
    next1: Option<&'a T>,
    next2: Option<&'a T>,
}

impl<'a, T: Ord> Iterator for Difference<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.next1, self.next2) {
                (Some(v1), Some(v2)) => match v1.cmp(v2) {
                    std::cmp::Ordering::Less => {
                        let res = v1;
                        self.next1 = self.iter1.next();
                        return Some(res);
                    }
                    std::cmp::Ordering::Equal => {
                        self.next1 = self.iter1.next();
                        self.next2 = self.iter2.next();
                    }
                    std::cmp::Ordering::Greater => {
                        self.next2 = self.iter2.next();
                    }
                },
                (Some(v1), None) => {
                    let res = v1;
                    self.next1 = self.iter1.next();
                    return Some(res);
                }
                (None, _) => return None,
            }
        }
    }
}

impl<T: serde::Serialize> serde::Serialize for SortedVecSet<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.iter())
    }
}

impl<'de, T> serde::Deserialize<'de> for SortedVecSet<T>
where
    T: Ord + serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor<T> {
            _marker: std::marker::PhantomData<SortedVecSet<T>>,
        }
        impl<'de, T> serde::de::Visitor<'de> for Visitor<T>
        where
            T: Ord + serde::Deserialize<'de>,
        {
            type Value = SortedVecSet<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a sequence")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut builder = match seq.size_hint() {
                    Some(hint) => SortedVecSetBuilder::with_capacity(hint),
                    None => SortedVecSetBuilder::new(),
                };
                while let Some(value) = seq.next_element()? {
                    builder.insert(value);
                }
                Ok(builder.build())
            }
        }
        deserializer.deserialize_seq(Visitor { _marker: std::marker::PhantomData })
    }
}

/// A builder for `SortedVecSet`.
pub struct SortedVecSetBuilder<T> {
    map_builder: crate::sorted_vec_map::SortedVecMapBuilder<T, ()>,
}

impl<T> SortedVecSetBuilder<T> {
    /// Creates a new, empty builder.
    pub fn new() -> Self {
        Self { map_builder: crate::sorted_vec_map::SortedVecMapBuilder::new() }
    }

    /// Creates a new builder with at least the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { map_builder: crate::sorted_vec_map::SortedVecMapBuilder::with_capacity(capacity) }
    }

    /// Adds a value to the set.
    ///
    /// Complexity: `O(1)` time (amortized).
    pub fn insert(&mut self, value: T) -> &mut Self
    where
        T: Ord,
    {
        self.map_builder.insert(value, ());
        self
    }

    /// Builds the `SortedVecSet`.
    ///
    /// Complexity: `O(n)` time if already sorted, `O(n log n)` otherwise, where n is the number of
    /// elements.
    pub fn build(self) -> SortedVecSet<T>
    where
        T: Ord,
    {
        SortedVecSet { map: self.map_builder.build() }
    }
}

impl<T: Ord> Extend<T> for SortedVecSetBuilder<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.map_builder.extend(iter.into_iter().map(|v| (v, ())));
    }
}

impl<T: Ord> FromIterator<T> for SortedVecSetBuilder<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut builder = Self::new();
        builder.extend(iter);
        builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(0 => 0; "empty_set")]
    #[test_case(10 => 10; "with_some_capacity")]
    fn test_with_capacity(cap: usize) -> usize {
        let set: SortedVecSet<i32> = SortedVecSet::with_capacity(cap);
        set.capacity()
    }

    #[test_case(vec![], 1 => false; "empty_set")]
    #[test_case(vec![1], 1 => true; "contains_element")]
    #[test_case(vec![0, 1], 0 => true; "contains_first")]
    #[test_case(vec![0, 1, 2], 3 => false; "does_not_contain")]
    fn test_contains(initial: Vec<i32>, lookup: i32) -> bool {
        let set: SortedVecSet<i32> = initial.into();
        set.contains(&lookup)
    }

    #[test_case(vec![], 1 => None; "empty_set_get")]
    #[test_case(vec![1], 1 => Some(1); "get_element")]
    #[test_case(vec![0, 1], 0 => Some(0); "get_first")]
    #[test_case(vec![0, 1, 2], 3 => None; "get_missing")]
    fn test_get(initial: Vec<i32>, lookup: i32) -> Option<i32> {
        let set: SortedVecSet<i32> = initial.into();
        set.get(&lookup).copied()
    }

    #[test_case(vec![], 50 => (true, vec![50]); "insert_empty")]
    #[test_case(vec![50], 47 => (true, vec![47, 50]); "insert_lesser")]
    #[test_case(vec![47, 50], 48 => (true, vec![47, 48, 50]); "insert_middle")]
    #[test_case(vec![47, 48, 50], 48 => (false, vec![47, 48, 50]); "insert_duplicate")]
    fn test_insert(initial: Vec<i32>, value: i32) -> (bool, Vec<i32>) {
        let mut set: SortedVecSet<i32> = initial.into();
        let inserted = set.insert(value);
        (inserted, set.into())
    }

    #[test_case(vec![], 1 => (false, vec![]); "remove_empty")]
    #[test_case(vec![1], 1 => (true, vec![]); "remove_only_element")]
    #[test_case(vec![0, 1], 0 => (true, vec![1]); "remove_first")]
    #[test_case(vec![0, 1], 1 => (true, vec![0]); "remove_last")]
    #[test_case(vec![0, 1], 2 => (false, vec![0, 1]); "remove_missing")]
    fn test_remove(initial: Vec<i32>, to_remove: i32) -> (bool, Vec<i32>) {
        let mut set: SortedVecSet<i32> = initial.into();
        let ret = set.remove(&to_remove);
        (ret, set.into())
    }

    #[test_case(vec![50, 47, 48, 50, 47], vec![47, 48, 50]; "duplicates_and_unsorted")]
    #[test_case(vec![], vec![]; "empty")]
    #[test_case(vec![1, 2, 3], vec![1, 2, 3]; "already_sorted")]
    fn test_from_iter(input: Vec<i32>, expected: Vec<i32>) {
        let set: SortedVecSet<i32> = input.into_iter().collect();
        let actual: Vec<i32> = set.into();
        assert_eq!(actual, expected);
    }

    #[test_case(vec![1, 2, 3], vec![1, 2, 3]; "simple_conversion")]
    #[test_case(vec![], vec![]; "empty_conversion")]
    fn test_into_vec(input: Vec<i32>, expected: Vec<i32>) {
        let set: SortedVecSet<i32> = input.into();
        let actual: Vec<i32> = set.into();
        assert_eq!(actual, expected);
    }

    #[test_case(vec![3, 1, 2, 2], vec![1, 2, 3]; "removes_duplicates_and_sorts")]
    #[test_case(vec![], vec![]; "empty_vec")]
    fn test_from_vec(input: Vec<i32>, expected: Vec<i32>) {
        let set: SortedVecSet<i32> = input.into();
        let actual: Vec<i32> = set.into();
        assert_eq!(actual, expected);
    }

    #[test_case([3, 1, 2, 2], vec![1, 2, 3]; "array_conversion")]
    fn test_from_array(input: [i32; 4], expected: Vec<i32>) {
        let set: SortedVecSet<i32> = input.into();
        let actual: Vec<i32> = set.into();
        assert_eq!(actual, expected);
    }

    #[test_case(vec![], None => 0; "empty_len")]
    #[test_case(vec![], Some(1) => 1; "insert_into_empty_len")]
    #[test_case(vec![1], Some(2) => 2; "insert_new_len")]
    #[test_case(vec![1, 2], Some(1) => 2; "insert_duplicate_len")]
    fn test_len(initial: Vec<i32>, to_insert: Option<i32>) -> usize {
        let mut set: SortedVecSet<i32> = initial.into();
        if let Some(v) = to_insert {
            set.insert(v);
        }
        set.len()
    }

    #[test_case(vec![1, 2, 3], vec![3, 4, 5], vec![1, 2, 3, 4, 5]; "overlapping")]
    #[test_case(vec![1, 2, 3], vec![4, 5, 6], vec![1, 2, 3, 4, 5, 6]; "disjoint")]
    #[test_case(vec![1, 2, 3], vec![], vec![1, 2, 3]; "empty_rhs")]
    #[test_case(vec![], vec![1, 2, 3], vec![1, 2, 3]; "empty_lhs")]
    fn test_union(lhs: Vec<i32>, rhs: Vec<i32>, expected: Vec<i32>) {
        let set1: SortedVecSet<i32> = lhs.into();
        let set2: SortedVecSet<i32> = rhs.into();
        let actual: Vec<i32> = set1.union(&set2).cloned().collect();
        assert_eq!(actual, expected);
    }

    #[test_case(vec![1, 2, 3], vec![3, 4, 5], vec![1, 2]; "lhs_difference")]
    #[test_case(vec![3, 4, 5], vec![1, 2, 3], vec![4, 5]; "rhs_difference")]
    #[test_case(vec![1, 2, 3], vec![1, 2, 3], vec![]; "identical_sets")]
    #[test_case(vec![1, 2, 3], vec![], vec![1, 2, 3]; "empty_rhs_diff")]
    #[test_case(vec![], vec![1, 2, 3], vec![]; "empty_lhs_diff")]
    fn test_difference(lhs: Vec<i32>, rhs: Vec<i32>, expected: Vec<i32>) {
        let set1: SortedVecSet<i32> = lhs.into();
        let set2: SortedVecSet<i32> = rhs.into();
        let actual: Vec<i32> = set1.difference(&set2).cloned().collect();
        assert_eq!(actual, expected);
    }

    #[test_case(vec![3, 1, 2], vec![1, 2, 3]; "btreeset_conversion")]
    #[test_case(vec![], vec![]; "empty_btreeset")]
    fn test_from_btreeset(input: Vec<i32>, expected: Vec<i32>) {
        let btree_set: BTreeSet<i32> = input.into_iter().collect();
        let set: SortedVecSet<i32> = btree_set.into();
        let actual: Vec<i32> = set.into();
        assert_eq!(actual, expected);
    }

    #[test_case(vec![56, 47, 53, 51, 49]; "normal_set")]
    #[test_case(vec![]; "empty_set_serde")]
    fn test_serialize_deserialize(input: Vec<i32>) {
        let set: SortedVecSet<i32> = input.into_iter().collect();
        let serialized = serde_json::to_vec(&set).unwrap();
        let deserialized: SortedVecSet<i32> = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(set, deserialized);
    }

    #[test]
    fn test_range() {
        let entries = [1, 3, 5, 7];
        let set = SortedVecSet::from(entries);
        let expected = BTreeSet::from(entries);

        for range in [
            (Bound::Unbounded, Bound::Unbounded),
            (Bound::Included(0), Bound::Unbounded),
            (Bound::Included(1), Bound::Unbounded),
            (Bound::Included(2), Bound::Unbounded),
            (Bound::Included(8), Bound::Unbounded),
            (Bound::Included(1), Bound::Excluded(7)),
            (Bound::Included(2), Bound::Excluded(6)),
            (Bound::Included(3), Bound::Excluded(5)),
            (Bound::Included(8), Bound::Excluded(10)),
            (Bound::Included(0), Bound::Included(8)),
            (Bound::Included(1), Bound::Included(7)),
            (Bound::Included(3), Bound::Included(5)),
            (Bound::Excluded(2), Bound::Unbounded),
            (Bound::Excluded(3), Bound::Excluded(7)),
        ] {
            assert_eq!(
                set.range(range.clone()).cloned().collect::<Vec<_>>(),
                expected.range(range).cloned().collect::<Vec<_>>(),
            );
        }
    }
}
