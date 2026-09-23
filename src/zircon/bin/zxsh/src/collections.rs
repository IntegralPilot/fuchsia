// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::serialization::{Deserialize, Serialize};
use crate::sort::quick_sort;

/// A flat map implemented using a vector of key-value pairs.
///
/// This is efficient for small numbers of elements, which is typical for shell environments
/// (e.g., environment variables, aliases).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlatMap<K, V>(Vec<(K, V)>);

impl<K, V> FlatMap<K, V> {
    /// Creates a new empty `FlatMap`.
    pub fn new() -> Self {
        FlatMap(Vec::new())
    }

    /// Returns a reference to the value corresponding to the key.
    pub fn get<'a, Q>(&'a self, key: &Q) -> Option<&'a V>
    where
        K: std::borrow::Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        self.0.iter().find(|(k, _)| k.borrow() == key).map(|(_, v)| v)
    }

    /// Returns a mutable reference to the value corresponding to the key.
    pub fn get_mut<'a, Q>(&'a mut self, key: &Q) -> Option<&'a mut V>
    where
        K: std::borrow::Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        self.0.iter_mut().find(|(k, _)| k.borrow() == key).map(|(_, v)| v)
    }

    /// Inserts a key-value pair into the map.
    ///
    /// If the map did have this key present, the value is updated.
    pub fn insert(&mut self, key: K, val: V)
    where
        K: PartialEq,
    {
        if let Some(pos) = self.0.iter().position(|(k, _)| k == &key) {
            self.0[pos].1 = val;
        } else {
            self.0.push((key, val));
        }
    }

    /// Removes a key from the map, returning the value at the key if the key was previously in the
    /// map.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        if let Some(pos) = self.0.iter().position(|(k, _)| k.borrow() == key) {
            Some(self.0.remove(pos).1)
        } else {
            None
        }
    }

    /// Returns `true` if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        self.0.iter().any(|(k, _)| k.borrow() == key)
    }

    /// An iterator visiting all key-value pairs in insertion order.
    pub fn iter(&self) -> std::slice::Iter<'_, (K, V)> {
        self.0.iter()
    }

    /// Returns `true` if the map contains no elements.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Clears the map, removing all key-value pairs.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Returns a vector of references to key-value pairs sorted by key.
    pub fn sorted_entries(&self) -> Vec<(&K, &V)>
    where
        K: Ord,
    {
        let mut entries = Vec::with_capacity(self.0.len());
        for (k, v) in &self.0 {
            entries.push((k, v));
        }
        quick_sort(&mut entries, &|a, b| a.0.cmp(b.0));
        entries
    }
}

impl<K: Serialize, V: Serialize> Serialize for FlatMap<K, V> {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        (self.0.len() as u32).serialize_into(buf);
        for (k, v) in &self.0 {
            k.serialize_into(buf);
            v.serialize_into(buf);
        }
    }
}

impl<K: Deserialize, V: Deserialize> Deserialize for FlatMap<K, V> {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let len = u32::deserialize(bytes, offset)? as usize;
        let mut m = Vec::with_capacity(len);
        for _ in 0..len {
            let k = K::deserialize(bytes, offset)?;
            let v = V::deserialize(bytes, offset)?;
            m.push((k, v));
        }
        Ok(FlatMap(m))
    }
}

/// A flat set implemented using a vector.
///
/// Efficient for small numbers of elements.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlatSet<T>(Vec<T>);

impl<T> FlatSet<T> {
    /// Creates a new empty `FlatSet`.
    pub fn new() -> Self {
        FlatSet(Vec::new())
    }

    /// Adds a value to the set.
    pub fn insert(&mut self, val: T)
    where
        T: PartialEq,
    {
        if !self.0.contains(&val) {
            self.0.push(val);
        }
    }

    /// Removes a value from the set. Returns whether the value was present in the set.
    pub fn remove<Q>(&mut self, val: &Q) -> bool
    where
        T: std::borrow::Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        if let Some(pos) = self.0.iter().position(|item| item.borrow() == val) {
            self.0.remove(pos);
            true
        } else {
            false
        }
    }

    /// Returns `true` if the set contains a value.
    pub fn contains<Q>(&self, val: &Q) -> bool
    where
        T: std::borrow::Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        self.0.iter().any(|item| item.borrow() == val)
    }

    /// An iterator visiting all elements in arbitrary order.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns the elements sorted.
    pub fn sorted(&self) -> Vec<&T>
    where
        T: Ord,
    {
        let mut elements = Vec::with_capacity(self.0.len());
        for item in &self.0 {
            elements.push(item);
        }
        quick_sort(&mut elements, &|a, b| a.cmp(b));
        elements
    }
}

impl<T: Serialize> Serialize for FlatSet<T> {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        (self.0.len() as u32).serialize_into(buf);
        for item in &self.0 {
            item.serialize_into(buf);
        }
    }
}

impl<T: Deserialize> Deserialize for FlatSet<T> {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let len = u32::deserialize(bytes, offset)? as usize;
        let mut s = Vec::with_capacity(len);
        for _ in 0..len {
            s.push(T::deserialize(bytes, offset)?);
        }
        Ok(FlatSet(s))
    }
}

impl<K, V> From<Vec<(K, V)>> for FlatMap<K, V> {
    fn from(v: Vec<(K, V)>) -> Self {
        FlatMap(v)
    }
}

impl<T> From<Vec<T>> for FlatSet<T> {
    fn from(v: Vec<T>) -> Self {
        FlatSet(v)
    }
}
