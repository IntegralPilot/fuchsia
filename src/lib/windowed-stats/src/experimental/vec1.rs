// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::{Deref, DerefMut};
use std::slice;

/// A contiguous growable array type that contains one or more items (never zero).
///
/// This vector type is similar to [`Vec`], but is **never** empty and guarantees that it contains
/// at least one item.
///
/// [`Vec`]: std::vec::Vec
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Vec1<T> {
    items: Vec<T>,
}

impl<T> Vec1<T> {
    /// Constructs a `Vec1` from an item.
    pub fn from_item(item: T) -> Self {
        Vec1 { items: vec![item] }
    }

    /// Constructs a `Vec1` from a head item and zero or more tail items.
    pub fn from_head_and_tail(head: T, tail: impl IntoIterator<Item = T>) -> Self {
        Vec1 { items: Some(head).into_iter().chain(tail).collect() }
    }

    /// Constructs a `Vec1` from an [`IntoIterator`] of items.
    ///
    /// # Errors
    ///
    /// Returns an error if the [`IntoIterator`] is empty.
    ///
    /// [`IntoIterator`]: std::iter::IntoIterator
    pub fn try_from_iter(items: impl IntoIterator<Item = T>) -> Result<Self, ()> {
        let mut items = items.into_iter();
        match items.next() {
            Some(head) => Ok(Vec1::from_head_and_tail(head, items)),
            _ => Err(()),
        }
    }

    // TODO(https://fxbug.dev/352335343): These iterator-like operations should instead be
    // implemented by an `Iterator1` (and arguably `Slice1`) type.
    /// Maps the items from the vector into another.
    pub fn map_into<U, F>(self, f: F) -> Vec1<U>
    where
        F: FnMut(T) -> U,
    {
        Vec1 { items: self.items.into_iter().map(f).collect() }
    }

    /// Fallibly maps the items in the vector by reference to either another vector or an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the mapping function returns an error for some item in the vector.
    pub fn try_map_ref<U, E, F>(&self, f: F) -> Result<Vec1<U>, E>
    where
        F: FnMut(&T) -> Result<U, E>,
    {
        self.items.iter().map(f).collect::<Result<Vec<_>, _>>().map(|items| Vec1 { items })
    }

    /// Fallibly maps the items in the vector by mutable reference to either another vector or an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns an error if the mapping function returns an error for some item in the vector.
    pub fn try_map_mut<U, E, F>(&mut self, f: F) -> Result<Vec1<U>, E>
    where
        F: FnMut(&mut T) -> Result<U, E>,
    {
        self.items.iter_mut().map(f).collect::<Result<Vec<_>, _>>().map(|items| Vec1 { items })
    }

    /// Gets an iterator over the items in the vector.
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.items.iter()
    }

    /// Gets an iterator over the items in the vector.
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, T> {
        self.items.iter_mut()
    }

    /// Gets a slice over the items in the vector.
    pub fn as_slice(&self) -> &[T] {
        self.items.as_slice()
    }

    /// Gets a slice over the items in the vector.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.items.as_mut_slice()
    }
}

impl<T> AsMut<[T]> for Vec1<T> {
    fn as_mut(&mut self) -> &mut [T] {
        self.items.as_mut()
    }
}

impl<T> AsRef<[T]> for Vec1<T> {
    fn as_ref(&self) -> &[T] {
        self.items.as_ref()
    }
}

impl<T> Deref for Vec1<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.items.deref()
    }
}

impl<T> DerefMut for Vec1<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.items.deref_mut()
    }
}

impl<T> From<Vec1<T>> for Vec<T> {
    fn from(one_or_more: Vec1<T>) -> Self {
        one_or_more.items
    }
}

impl<T> IntoIterator for Vec1<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<T> TryFrom<Vec<T>> for Vec1<T> {
    type Error = ();

    fn try_from(items: Vec<T>) -> Result<Self, Self::Error> {
        if items.is_empty() { Err(()) } else { Ok(Vec1 { items }) }
    }
}

macro_rules! with_literals {
    ($f:ident$(,)?) => {};
    ($f:ident, [ $($N:literal),+ $(,)? ]$(,)?) => {
        $(
            $f!($N);
        )+
    };
}
macro_rules! impl_from_array_for_vec1 {
    ($N:literal $(,)?) => {
        impl<T> From<[T; $N]> for Vec1<T> {
            fn from(items: [T; $N]) -> Self {
                Vec1 { items: items.into() }
            }
        }
    };
}
with_literals!(impl_from_array_for_vec1, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);

#[cfg(test)]
mod tests {
    use crate::experimental::vec1::Vec1;

    #[test]
    fn vec1_from_item_or_head_and_tail() {
        let xs = Vec1::from_item(0i32);
        assert_eq!(xs.as_slice(), &[0]);

        let xs = Vec1::from_head_and_tail(0i32, []);
        assert_eq!(xs.as_slice(), &[0]);

        let xs = Vec1::from_head_and_tail(0i32, [1, 2]);
        assert_eq!(xs.as_slice(), &[0, 1, 2]);
    }

    #[test]
    fn vec1_ref() {
        let mut xs = Vec1::from_item(0i32);
        assert_eq!(xs.as_slice(), &[0]);
        assert_eq!(xs.as_ref(), &[0]);
        assert_eq!(xs.as_mut_slice(), &mut [0]);
        assert_eq!(xs.as_mut(), &mut [0]);
    }

    #[test]
    fn vec1_from_array() {
        let xs = Vec1::from([0i32]);
        assert_eq!(xs.as_slice(), &[0]);

        let xs = Vec1::from([0i32, 1, 2]);
        assert_eq!(xs.as_slice(), &[0, 1, 2]);
    }

    #[test]
    fn vec1_try_from_vec() {
        let xs = Vec1::<i32>::try_from(vec![]);
        assert!(xs.is_err());

        let xs = Vec1::try_from(vec![0i32]).unwrap();
        assert_eq!(xs.as_slice(), &[0]);

        let xs = Vec1::try_from(vec![0i32, 1, 2]).unwrap();
        assert_eq!(xs.as_slice(), &[0, 1, 2]);
    }

    #[test]
    fn vec1_try_from_iter() {
        let xs = Vec1::<i32>::try_from_iter([]);
        assert!(xs.is_err());

        let xs = Vec1::try_from_iter([0i32]).unwrap();
        assert_eq!(xs.as_slice(), &[0]);

        let xs = Vec1::try_from_iter([0i32, 1, 2]).unwrap();
        assert_eq!(xs.as_slice(), &[0, 1, 2]);
    }

    #[test]
    fn vec1_iter() {
        let xs = Vec1::from([0i32, 1, 2]);
        let ys: Vec<_> = xs.iter().map(|x| x + 1).collect();
        assert_eq!(ys.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn vec1_map_into() {
        let xs = Vec1::from([0i32, 1, 2]);
        let ys = xs.map_into(|x| x + 1);
        assert_eq!(ys.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn vec1_try_map_ref() {
        let xs = Vec1::from([0i32, 1, 2]);
        let ys: Result<_, ()> = xs.try_map_ref(|x| Ok(*x + 1));
        assert_eq!(ys.unwrap().as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn vec1_try_map_mut() {
        let mut xs = Vec1::from([0i32, 1, 2]);
        let ys: Result<_, ()> = xs.try_map_mut(|x| Ok(*x + 1));
        assert_eq!(ys.unwrap().as_slice(), &[1, 2, 3]);
    }
}
