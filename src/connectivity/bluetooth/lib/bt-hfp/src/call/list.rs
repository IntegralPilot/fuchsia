// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// The index associated with a call, that is guaranteed to be unique for the lifetime of the call,
/// but will be recycled after the call is released.
pub type Idx = usize;

/// A collection designed for the specific requirements of storing Calls with an associated index.
///
/// The requirements found in HFP v1.8, Section 4.34.2, "+CLCC":
///
///   * Each call is assigned a number starting at 1.
///   * Calls hold their number until they are released.
///   * New calls take the lowest available number.
///
/// Note: "Insert" is a O(n) operation in order to simplify the implementation.
/// This data structure is best suited towards small n for this reason.
pub struct List<T> {
    inner: Vec<Option<T>>,
}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self { inner: Vec::default() }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for List<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.inner)
    }
}

impl<T> List<T> {
    /// Insert a new value into the list, returning an index that is guaranteed to be unique until
    /// the value is removed from the list.
    pub fn insert(&mut self, value: T) -> Idx {
        let index = if let Some(index) = self.inner.iter_mut().position(|v| v.is_none()) {
            self.inner[index] = Some(value);
            index
        } else {
            self.inner.push(Some(value));
            self.inner.len() - 1
        };

        Self::to_call_index(index)
    }

    /// Retrieve a value by index. Returns `None` if the index does not point to a value.
    pub fn get(&self, index: Idx) -> Option<&T> {
        let index = Self::to_internal_index(index)?;
        self.inner.get(index).map(|v| v.as_ref()).unwrap_or_default()
    }

    /// Retrieve a mutable reference to a value by index. Returns `None` if the index does not point
    /// to a value.
    pub fn get_mut(&mut self, index: Idx) -> Option<&mut T> {
        let index = Self::to_internal_index(index)?;
        self.inner.get_mut(index).map(|v| v.as_mut()).unwrap_or_default()
    }

    /// Remove a value by index. Returns `None` if the value did not point to a value.
    pub fn remove(&mut self, index: Idx) -> Option<T> {
        let index = Self::to_internal_index(index)?;
        self.inner.get_mut(index).map(|v| v.take()).unwrap_or_default()
    }

    /// Return an iterator of the calls and associated call indices.
    pub fn calls(&self) -> impl Iterator<Item = (Idx, &T)> + Clone {
        self.inner
            .iter()
            .enumerate()
            .flat_map(|(i, entry)| entry.as_ref().map(|v| (Self::to_call_index(i), v)))
    }

    /// Return an iterator of mutable references to the calls and associated call indices.
    pub fn calls_mut(&mut self) -> impl Iterator<Item = (Idx, &mut T)> {
        self.inner
            .iter_mut()
            .enumerate()
            .flat_map(|(i, entry)| entry.as_mut().map(|v| (Self::to_call_index(i), v)))
    }

    /// Convert a `Idx` to the internal index used to locate a call.
    ///
    /// The Idx for a call starts at 1 instead of 0, so the internal index must be decremented
    /// after being received by the user.
    ///
    /// Returns `None` if `index` is 0 because 0 is an invalid index.
    fn to_internal_index(index: Idx) -> Option<usize> {
        (index != 0).then(|| index - 1)
    }

    /// Convert the internal index for a call to the external `Idx`.
    /// The Idx for a call starts at 1 instead of 0, so the internal index must be incremented
    /// before being returned to the user.
    fn to_call_index(internal: usize) -> Idx {
        internal + 1
    }

    /// Returns the number of calls in the call list.
    pub fn len(&self) -> usize {
        self.inner.iter().map(Option::as_ref).filter(Option::is_some).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn call_list_insert() {
        let mut list = List::default();
        let i1 = list.insert(1);
        assert_eq!(i1, 1, "The first value must be assigned the number 1");
        let i2 = list.insert(2);
        assert_eq!(i2, 2, "The second value is assigned the next available number");
    }

    #[fuchsia::test]
    fn call_list_get() {
        let mut list = List::default();
        let i1 = list.insert(1);
        let i2 = list.insert(2);
        assert_eq!(list.get(0), None);
        assert_eq!(list.get(i1), Some(&1));
        assert_eq!(list.get(i2), Some(&2));
        assert_eq!(list.get(3), None);
    }

    #[fuchsia::test]
    fn call_list_get_mut() {
        let mut list = List::default();
        let i1 = list.insert(1);
        let i2 = list.insert(2);
        assert_eq!(list.get_mut(i1), Some(&mut 1));
        assert_eq!(list.get_mut(i2), Some(&mut 2));
        assert_eq!(list.get_mut(3), None);
    }

    #[fuchsia::test]
    fn call_list_remove() {
        let mut list = List::default();
        let i1 = list.insert(1);
        let i2 = list.insert(2);
        let removed = list.remove(i1);
        assert!(removed.is_some());
        assert_eq!(list.get(i1), None, "The value at i1 is removed");
        assert_eq!(list.get(i2), Some(&2), "The value at i2 is untouched");
        let invalid_idx = 0;
        assert!(list.remove(invalid_idx).is_none());
    }

    #[fuchsia::test]
    fn call_list_remove_and_insert_behaves() {
        let mut list = List::default();
        let i1 = list.insert(1);
        let i2 = list.insert(2);
        let i3 = list.insert(3);
        let i4 = list.insert(4);
        assert!(list.remove(i2).is_some());
        assert!(list.remove(i1).is_some());
        assert!(list.remove(i3).is_some());
        let i5 = list.insert(5);
        assert_eq!(
            i5, i1,
            "i5 is the lowest possible index (i1) even though i1 was not the first or last index removed"
        );
        assert_eq!(list.get(i5), Some(&5), "The value at i5 is correct");
        let i6 = list.insert(6);
        let i7 = list.insert(7);
        assert_eq!(i6, i2, "i6 takes the next available index (i2)");
        assert_eq!(i7, i3, "i7 takes the next available index (i3)");
        let i8_ = list.insert(8);
        assert_ne!(i8_, i4, "i4 is reserved, so i8_ must take a new index");
        assert_eq!(
            i8_, 5,
            "i8_ takes an index of 5 since it is the last of the 5 values to be inserted"
        );
    }

    #[fuchsia::test]
    fn call_list_iter_returns_all_valid_values() {
        let mut list = List::default();
        let i1 = list.insert(1);
        let i2 = list.insert(2);
        let i3 = list.insert(3);
        let i4 = list.insert(4);
        assert!(list.remove(i2).is_some());
        let actual: Vec<_> = list.calls().collect();
        let expected = vec![(i1, &1), (i3, &3), (i4, &4)];
        assert_eq!(actual, expected);
    }
}
