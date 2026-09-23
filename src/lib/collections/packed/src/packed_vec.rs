// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A memory-efficient vector that stores dynamically sized items in a single contiguous buffer.

use crate::PackedItem;
use std::borrow::Cow;
use std::iter::{Extend, FromIterator};
use std::ops::Index;

/// A packed vector that stores slices of an element type in a single contiguous buffer.
///
/// This behaves somewhat like a `Vec<Box<[T]>>` or `Vec<Vec<T>>`, but allows
/// for better memory locality and fewer allocations by storing all elements
/// in one `Vec<T::Item>`.
pub struct PackedVec<T: ?Sized + PackedItem> {
    data: Vec<u8>,
    // offsets stores the end index of each slice.
    // The slice at index i is data[offsets[i-1]..offsets[i]] (with offsets[-1] implicitly 0).
    // We always maintain offsets.len() == self.len().
    offsets: Vec<usize>,
    // We use `PhantomData<fn() -> T>` instead of `PhantomData<T>` to bypass
    // strict dropcheck (dropck) rules. `PackedVec` owns raw bytes, not actual
    // instances of `T`, so dropping `PackedVec` does not drop any `T`s.
    // If we used `PhantomData<T>`, the compiler would incorrectly assume we
    // are dropping `T`s, which can cause spurious lifetime errors (e.g., preventing
    // the collection from being dropped if a lifetime in `T` has expired).
    // `fn() -> T` preserves covariance over `T` and auto-traits like Send/Sync
    // while confirming to the compiler that we do not own a `T` that needs dropping.
    _phantom: std::marker::PhantomData<fn() -> T>,
}

impl<T: ?Sized + PackedItem> PackedVec<T> {
    /// Creates a new empty `PackedVec`.
    pub fn new() -> Self {
        Self { data: Vec::new(), offsets: Vec::new(), _phantom: std::marker::PhantomData }
    }

    /// Creates a new `PackedVec` with the specified capacities.
    ///
    /// The `element_capacity` argument specifies the number of slices that can be
    /// stored without reallocating the offsets vector. The `buffer_capacity`
    /// argument specifies the cumulative length of slices that can be stored
    /// without reallocating the data vector.
    pub fn with_capacity(element_capacity: usize, buffer_capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(buffer_capacity),
            offsets: Vec::with_capacity(element_capacity),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Clears the vector, removing all elements.
    pub fn clear(&mut self) {
        self.data.clear();
        self.offsets.clear();
    }

    /// Creates a new `PackedVec` from a slice of element slices, pre-allocating
    /// the required capacity.
    pub fn from_slice<U: AsRef<T>>(slices: &[U]) -> Self {
        let element_capacity = slices.len();
        let buffer_capacity = slices.iter().map(|s| s.as_ref().as_bytes().len()).sum();
        let mut vec = Self::with_capacity(element_capacity, buffer_capacity);
        vec.extend(slices);
        vec
    }

    /// Reserves capacity for at least `additional` more elements to be inserted.
    pub fn reserve(&mut self, additional: usize) {
        self.offsets.reserve(additional);
    }

    /// Shrinks the capacity of the vector as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
        self.offsets.shrink_to_fit();
    }

    /// Appends an item to the back of the collection.
    pub fn push(&mut self, slice: &T) {
        self.data.extend_from_slice(slice.as_bytes());
        self.offsets.push(self.data.len());
    }

    /// Returns the slice at the given index, or `None` if out of bounds.
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len() {
            return None;
        }
        // SAFETY: We checked `index < self.len()`.
        Some(unsafe { self.get_unchecked(index) })
    }

    /// Returns a reference to the slice at the given index without bounds checking.
    ///
    /// # Safety
    ///
    /// The index must be less than `self.len()`.
    pub unsafe fn get_unchecked(&self, index: usize) -> &T {
        let start = if index == 0 {
            0
        } else {
            // SAFETY: Since `index > 0`, `index - 1` is in bounds of `self.offsets`.
            unsafe { *self.offsets.get_unchecked(index - 1) }
        };

        // SAFETY: The caller guarantees `index < self.len()`. Since `self.len()` is
        // exactly `self.offsets.len()`, the index is guaranteed to be in-bounds.
        let end = unsafe { *self.offsets.get_unchecked(index) };

        // SAFETY: Because T: Unaligned, no padding was ever inserted. The bytes
        // of the item are stored contiguously between `start` and `end`.
        let bytes = unsafe { self.data.get_unchecked(start..end) };

        // SAFETY: `PackedItem::from_bytes` requires the input bytes to have
        // been created by `IntoBytes::as_bytes()`. This property is maintained
        // by the bytes packed into the vector by `push()` using valid
        // instances of `T`.
        unsafe { T::from_bytes(bytes) }
    }

    /// Returns the number of slices in the vector.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// Returns the cumulative length of all slices in the vector in bytes.
    pub fn buffer_len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if the vector contains no slices.
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Binary searches this sorted vector for a given element.
    ///
    /// If the value is found then [`Result::Ok`] is returned, containing the index
    /// of the matching element. If there are multiple matches, then any one of the
    /// matches could be returned.
    ///
    /// If the value is not found then [`Result::Err`] is returned, containing the
    /// index where a matching element could be inserted while maintaining sorted
    /// order.
    pub fn binary_search(&self, x: &T) -> Result<usize, usize>
    where
        T: Ord,
    {
        self.binary_search_by(|p| p.cmp(x))
    }

    /// Binary searches this sorted vector for a given element.
    ///
    /// If the value is found then [`Result::Ok`] is returned, containing the index
    /// of the matching element. If there are multiple matches, then any one of the
    /// matches could be returned.
    ///
    /// If the value is not found then [`Result::Err`] is returned, containing the
    /// index where a matching element could be inserted while maintaining sorted
    /// order.
    pub fn binary_search_by<'a, F>(&'a self, mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&'a T) -> std::cmp::Ordering,
    {
        // We want to leverage the Vec::binary_search, since it is better
        // optimized than a simple binary search algorithm. However, we can't
        // just pass a closure that operates on `&T` because `binary_search`
        // operates on `offsets` side table, and not our string table. We can
        // get the behavior we want by:
        //
        // 1. Take the `&usize` from the binary search step.
        // 2. Determine the start and end pointers of `self.data` using the
        //    start of the vector for the first element, and the `&usize` for
        //    the rest.
        // 3. Get the value at the given index.
        // 4. Apply the predicate to the value.
        self.offsets.binary_search_by(|end| {
            // SAFETY: `end` is a reference to an element in `offsets`.
            // We use pointer arithmetic to find the index of this element.
            let end_ptr = end as *const usize;
            let index = unsafe { end_ptr.offset_from_unsigned(self.offsets.as_ptr()) };

            // SAFETY: `offsets.binary_search_by` iterates over `offsets`, so `index` is valid.
            let slice = unsafe { self.get_unchecked(index) };
            f(slice)
        })
    }

    /// Returns the first element of the vector, or `None` if it is empty.
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }

    /// Returns the last element of the vector, or `None` if it is empty.
    pub fn last(&self) -> Option<&T> {
        let len = self.len();
        if len == 0 {
            None
        } else {
            // SAFETY: We checked `len > 0`, so `len - 1` is a valid index.
            Some(unsafe { self.get_unchecked(len - 1) })
        }
    }

    /// Returns an iterator over the slices.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter { vec: self, range: 0..self.len() }
    }

    /// Returns a draining iterator that removes all elements and yields them.
    pub fn drain(&mut self) -> Drain<'_, T> {
        let len = self.len();
        Drain { vec: self, cursor: 0, len }
    }

    /// Returns an iterator over a sub-range of slices in the vector.
    ///
    /// The bounds must be `usize` bounds (representing indices).
    /// If bounds are outside the vector length or inverted, they will be clipped.
    pub fn range<R: std::ops::RangeBounds<usize>>(&self, range: R) -> Iter<'_, T> {
        let range = crate::compute_range_indices(self.len(), range, |&idx| Ok(idx));
        Iter { vec: self, range }
    }
}

impl<T: ?Sized + PackedItem> Default for PackedVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ?Sized + PackedItem> Clone for PackedVec<T> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            offsets: self.offsets.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T: ?Sized + PackedItem> PartialEq for PackedVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data && self.offsets == other.offsets
    }
}

impl<T: ?Sized + PackedItem> Eq for PackedVec<T> {}

impl<T: ?Sized + PackedItem> std::hash::Hash for PackedVec<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.data.hash(state);
        self.offsets.hash(state);
    }
}

impl<T: ?Sized + PackedItem> std::fmt::Debug for PackedVec<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries::<&T, _>(self.iter()).finish()
    }
}

impl<'a, T: ?Sized + PackedItem> IntoIterator for &'a PackedVec<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        Iter::<T> { vec: self, range: 0..self.len() }
    }
}

/// An iterator over the slices in a [`PackedVec`].
pub struct Iter<'a, T: ?Sized + PackedItem> {
    vec: &'a PackedVec<T>,
    range: std::ops::Range<usize>,
}

impl<'a, T: ?Sized + PackedItem> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        let i = self.range.next()?;
        self.vec.get(i)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

impl<'a, T: ?Sized + PackedItem> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let i = self.range.next_back()?;
        self.vec.get(i)
    }
}

impl<'a, T: ?Sized + PackedItem> ExactSizeIterator for Iter<'a, T> {}

/// A draining lending iterator over the slices in a [`PackedVec`].
pub struct Drain<'a, T: ?Sized + PackedItem> {
    vec: &'a mut PackedVec<T>,
    cursor: usize,
    len: usize,
}

impl<'a, T: ?Sized + PackedItem> Drain<'a, T> {
    /// Returns the next element in the draining iterator.
    pub fn next(&mut self) -> Option<&T> {
        if self.cursor >= self.len {
            return None;
        }
        let i = self.cursor;
        self.cursor += 1;
        self.vec.get(i)
    }

    /// Returns the next element from the back in the draining iterator.
    pub fn next_back(&mut self) -> Option<&T> {
        if self.cursor >= self.len {
            return None;
        }
        self.len -= 1;
        self.vec.get(self.len)
    }

    /// Returns the number of elements remaining in the draining iterator.
    pub fn len(&self) -> usize {
        self.len - self.cursor
    }
}

impl<'a, T: ?Sized + PackedItem> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        self.vec.clear();
    }
}

impl<T: ?Sized + PackedItem> Index<usize> for PackedVec<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("index out of bounds")
    }
}

impl<U: AsRef<T>, T: ?Sized + PackedItem> Extend<U> for PackedVec<T> {
    fn extend<I: IntoIterator<Item = U>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.offsets.reserve(lower);
        for item in iter {
            self.push(item.as_ref());
        }
    }
}

impl<U: AsRef<T>, T: ?Sized + PackedItem> FromIterator<U> for PackedVec<T> {
    fn from_iter<I: IntoIterator<Item = U>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut vec = Self::with_capacity(lower, 0);
        vec.extend(iter);
        vec
    }
}

impl<U, T: ?Sized + PackedItem, const N: usize> From<[U; N]> for PackedVec<T>
where
    U: AsRef<T>,
{
    fn from(arr: [U; N]) -> Self {
        Self::from_iter(arr)
    }
}

impl<T: ?Sized + PackedItem> serde::Serialize for PackedVec<T>
where
    T: serde::Serialize,
{
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for item in self.iter() {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}

impl<'de, T: ?Sized + PackedItem + 'de> serde::Deserialize<'de> for PackedVec<T>
where
    T: ToOwned,
    Cow<'de, T>: serde::Deserialize<'de>,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SeqVisitor<T: ?Sized + PackedItem>(std::marker::PhantomData<fn() -> T>);

        impl<'de, T: ?Sized + PackedItem + 'de> serde::de::Visitor<'de> for SeqVisitor<T>
        where
            T: ToOwned,
            Cow<'de, T>: serde::Deserialize<'de>,
        {
            type Value = PackedVec<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a sequence of items")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut vec = PackedVec::with_capacity(seq.size_hint().unwrap_or(0), 0);
                while let Some(elem) = seq.next_element::<Cow<'de, T>>()? {
                    vec.push(elem.as_ref());
                }
                Ok(vec)
            }
        }

        deserializer.deserialize_seq(SeqVisitor(std::marker::PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_capacity() {
        let pv: PackedVec<[u8]> = PackedVec::with_capacity(10, 20);
        assert!(pv.offsets.capacity() >= 10);
        assert!(pv.data.capacity() >= 20);
        assert_eq!(pv.len(), 0);
    }

    #[test]
    fn test_from_slice() {
        let slices: &[&[u8]] = &[&[1, 2], &[3], &[4, 5, 6]];
        let pv = PackedVec::from_slice(slices);
        assert_eq!(pv.len(), 3);
        assert_eq!(pv.get(0), Some(&[1, 2][..]));
        assert_eq!(pv.get(1), Some(&[3][..]));
        assert_eq!(pv.get(2), Some(&[4, 5, 6][..]));
        assert!(pv.offsets.capacity() >= 3);
        assert!(pv.data.capacity() >= 6);
    }

    #[test]
    fn test_packed_vec() {
        let mut pv: PackedVec<[u8]> = PackedVec::new();
        pv.push(&[1u8, 2][..]);
        pv.push(&[][..]);
        pv.push(&[3, 4, 5][..]);

        assert_eq!(pv.len(), 3);
        assert!(!pv.is_empty());

        assert_eq!(pv.get(0), Some(&[1u8, 2][..]));
        assert_eq!(pv.get(1), Some(&[] as &[u8]));
        assert_eq!(pv.get(2), Some(&[3u8, 4, 5][..]));
        assert_eq!(pv.get(3), None);

        assert_eq!(pv.get(0).unwrap(), &[1u8, 2][..]);
        assert_eq!(pv.get(1).unwrap(), &[] as &[u8]);

        let collected: Vec<_> = pv.iter().collect();
        assert_eq!(collected, vec![&[1u8, 2][..], &[] as &[u8], &[3u8, 4, 5][..]]);
    }

    #[test]
    fn test_first_last() {
        let mut pv: PackedVec<[u8]> = PackedVec::new();
        assert_eq!(pv.first(), None);
        assert_eq!(pv.last(), None);

        pv.push(&[1][..]);
        assert_eq!(pv.first(), Some(&[1][..]));
        assert_eq!(pv.last(), Some(&[1][..]));

        pv.push(&[2, 3][..]);
        assert_eq!(pv.first(), Some(&[1][..]));
        assert_eq!(pv.last(), Some(&[2, 3][..]));
    }

    #[test]
    fn test_extend_from_iterator() {
        let pv: PackedVec<[u8]> = vec![vec![1], vec![2, 3]].iter().map(|v| v.as_slice()).collect();
        assert_eq!(pv.len(), 2);
        let slice: &[u8] = pv.get(1).unwrap();
        assert_eq!(slice, &[2, 3][..]);
    }

    #[test]
    fn test_drain() {
        let mut pv: PackedVec<[u8]> = PackedVec::new();
        pv.push(&[1][..]);
        pv.push(&[2, 3][..]);
        pv.push(&[4, 5, 6][..]);

        let mut drain = pv.drain();
        assert_eq!(drain.len(), 3);

        // Test next()
        assert_eq!(drain.next(), Some(&[1][..]));
        assert_eq!(drain.len(), 2);

        // Test next_back()
        assert_eq!(drain.next_back(), Some(&[4, 5, 6][..]));
        assert_eq!(drain.len(), 1);

        // Test next() again
        assert_eq!(drain.next(), Some(&[2, 3][..]));
        assert_eq!(drain.len(), 0);

        // Test None
        assert_eq!(drain.next(), None);
        assert_eq!(drain.next_back(), None);
    }

    #[test]
    fn test_drain_drop_clears() {
        let mut pv: PackedVec<[u8]> = PackedVec::new();
        pv.push(&[1][..]);
        pv.push(&[2, 3][..]);

        {
            let mut drain = pv.drain();
            assert_eq!(drain.next(), Some(&[1][..]));
            // Drop happens here
        }

        assert!(pv.is_empty());
        assert_eq!(pv.len(), 0);

        // Make sure both vectors were cleared, not just one.
        assert!(pv.offsets.is_empty());
        assert!(pv.data.is_empty());
    }

    #[test]
    fn test_binary_search_empty() {
        let pv: PackedVec<[u8]> = PackedVec::new();
        assert_eq!(pv.binary_search_by(|x| x.cmp(&[1u8][..])), Err(0));
    }

    #[test]
    fn test_binary_search() {
        let mut pv: PackedVec<[u8]> = PackedVec::new();
        pv.push(&[1]);
        pv.push(&[3]);
        pv.push(&[5]);

        assert_eq!(pv.binary_search_by(|x| x.cmp(&[1u8][..])), Ok(0));
        assert_eq!(pv.binary_search_by(|x| x.cmp(&[3u8][..])), Ok(1));
        assert_eq!(pv.binary_search_by(|x| x.cmp(&[5u8][..])), Ok(2));
        assert_eq!(pv.binary_search_by(|x| x.cmp(&[2u8][..])), Err(1));
        assert_eq!(pv.binary_search_by(|x| x.cmp(&[0u8][..])), Err(0));
        assert_eq!(pv.binary_search_by(|x| x.cmp(&[6u8][..])), Err(3));
    }

    #[test]
    fn test_packed_str_vec() {
        let mut pv: PackedVec<str> = PackedVec::new();
        pv.push("hello");
        pv.push("");
        pv.push("world!!");

        assert_eq!(pv.len(), 3);
        assert!(!pv.is_empty());

        assert_eq!(pv.get(0), Some("hello"));
        assert_eq!(pv.get(1), Some(""));
        assert_eq!(pv.get(2), Some("world!!"));
        assert_eq!(pv.get(3), None);

        assert_eq!(pv.get(0).unwrap(), "hello");
        assert_eq!(pv.get(1).unwrap(), "");

        let collected: Vec<_> = pv.iter().collect();
        assert_eq!(collected, vec!["hello", "", "world!!"]);
    }

    #[test]
    fn test_serde() {
        let mut pv: PackedVec<str> = PackedVec::new();
        pv.push("hello");
        pv.push("world");
        let serialized = serde_json::to_string(&pv).unwrap();
        let deserialized: PackedVec<str> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(pv, deserialized);
    }
}
