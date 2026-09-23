// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(unsafe_op_in_unsafe_fn)]

use crate::rcu_array::RcuArray;
use crate::rcu_intrusive_list::{
    Link, RcuIntrusiveList, RcuIntrusiveListCursor, RcuListAdapter, rcu_list_adapter,
};
use crate::rcu_list::RcuList;
use fuchsia_rcu::{RcuDroppable, RcuReadScope};
use std::borrow::Borrow;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The initial capacity of the hash map.
const INITIAL_CAPACITY: usize = 16;

/// An entry in the hash table.
#[derive(Debug, RcuDroppable)]
struct Entry<K, V> {
    /// The key for this entry.
    key: K,

    /// The value for this entry.
    value: V,

    /// The link to the next node in the collision chain for this bucket.
    collision_chain: Link,

    /// The link to the next node in the insertion chain for this bucket.
    insertion_chain: Link,
}

impl<K, V> Entry<K, V> {
    /// Create a new hash table entry.
    fn new(key: K, value: V) -> Self {
        Self {
            key,
            value,
            collision_chain: Default::default(),
            insertion_chain: Default::default(),
        }
    }
}

/// An RcuListAdapter for the collision chain.
#[derive(Debug, RcuDroppable)]
struct CollisionAdapter;

impl<K, V> RcuListAdapter<Entry<K, V>> for CollisionAdapter {
    rcu_list_adapter!(Entry<K, V>, collision_chain);
}

/// An RcuListAdapter for the insertion chain.
#[derive(Debug, RcuDroppable)]
struct InsertionAdapter;

impl<K, V> RcuListAdapter<Entry<K, V>> for InsertionAdapter {
    rcu_list_adapter!(Entry<K, V>, insertion_chain);
}

/// The result of inserting an entry into the map.
pub enum InsertionResult<V> {
    /// The entry was inserted.
    ///
    /// The number of entries in the map is returned.
    Inserted(usize),

    /// The entry was updated.
    ///
    /// The old value is returned.
    Updated(V),
}

/// The bucket in the hash table.
///
/// Each bucket is a linked list to hold the collision chain.
type Bucket<K, V> = RcuList<Entry<K, V>, CollisionAdapter>;

/// A hash map that uses read-copy-update (RCU) to manage concurrent accesses.
///
/// By default, this map uses `rapidhash::RapidBuildHasher`, which provides high performance.
/// However, if this map holds keys which may be attacker-controlled, consider using
/// `std::collections::hash_map::RandomState` instead.
#[derive(RcuDroppable)]
pub struct RcuRawHashMap<K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// The table of buckets.
    table: RcuArray<Bucket<K, V>>,

    /// The number of entries in the map.
    num_entries: AtomicUsize,

    /// The entries in this map in the order they were inserted.
    insertion_chain: RcuIntrusiveList<Entry<K, V>, InsertionAdapter>,

    /// The build hasher.
    hash_builder: S,
}

impl<K, V> Default for RcuRawHashMap<K, V, rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
{
    fn default() -> Self {
        Self::with_capacity_and_hasher(0, rapidhash::RapidBuildHasher::default())
    }
}

impl<K, V> RcuRawHashMap<K, V, rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
{
    /// Creates a new hash map with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_hasher(capacity, rapidhash::RapidBuildHasher::default())
    }
}

impl<K, V, S> RcuRawHashMap<K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// Creates a new hash map with the given capacity and hasher.
    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        let mut table = Vec::new();
        table.resize_with((capacity + 1) / 2, Default::default);
        Self {
            table: RcuArray::from(table),
            num_entries: AtomicUsize::new(0),
            insertion_chain: Default::default(),
            hash_builder,
        }
    }

    /// Creates a new hash map with the given hasher.
    pub fn with_hasher(hash_builder: S) -> Self {
        Self::with_capacity_and_hasher(0, hash_builder)
    }

    /// Returns the hash of the key as a u64.
    fn hash_key<Q>(&self, key: &Q) -> u64
    where
        Q: ?Sized + Hash,
    {
        let mut hasher = self.hash_builder.build_hasher();
        key.hash(&mut hasher);
        hasher.finish()
    }

    /// Returns the bucket for the given key in the given table.
    fn get_bucket<'a, Q>(&self, table: &'a [Bucket<K, V>], key: &Q) -> &'a Bucket<K, V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash,
    {
        let hash = self.hash_key(key);
        let index = hash as usize % table.len();
        &table[index]
    }

    /// Returns a reference to the bucket for the given key.
    fn read_bucket<'a, Q>(&self, scope: &'a RcuReadScope, key: &Q) -> Option<&'a Bucket<K, V>>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash,
    {
        let table = self.table.as_slice(scope);
        if table.is_empty() {
            return None;
        }
        Some(self.get_bucket(table, key))
    }

    /// Returns a reference to the value corresponding to the key.
    ///
    /// Another thread running concurrently might see a different value for the object.
    pub fn get<'a, Q>(&self, scope: &'a RcuReadScope, key: &Q) -> Option<&'a V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let bucket = self.read_bucket(scope, key)?;
        bucket.iter(scope).find(|entry| entry.key.borrow() == key).map(|entry| &entry.value)
    }

    /// Returns the number of entries in the map.
    ///
    /// The length can change concurrently with this call.
    pub fn len(&self) -> usize {
        self.num_entries.load(Ordering::Relaxed)
    }

    /// Inserts a key-value pair into the map.
    ///
    /// If the map did not have this key present, `None` is returned.
    ///
    /// If the map did have this key present, the value is updated, and the old
    /// value is returned.
    ///
    /// Concurrent readers might not see the inserted value until the RCU state machine has made
    /// sufficient progress to ensure that no concurrent readers are holding read guards.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn insert(&self, scope: &RcuReadScope, key: K, value: V) -> InsertionResult<V> {
        let mut table = self.table.as_slice(scope);
        if self.needs_to_grow(table) {
            // SAFETY: Our caller is required to use external synchronization to exclude concurrent
            // writers.
            table = unsafe { self.grow(&scope, table) };
        }
        let bucket = self.get_bucket(table, &key);
        let mut cursor = bucket.cursor(&scope);
        while let Some(entry) = cursor.current() {
            if entry.key == key {
                let old_value = entry.value.clone();
                // SAFETY: We have exclusive access to the bucket because we have exclusive access
                // to the table.
                unsafe {
                    let removed_entry = cursor.remove();
                    self.insertion_chain.remove(&scope, removed_entry);
                    let entry = bucket.push_front(&scope, Entry::new(key, value));
                    self.insertion_chain.push_back(&scope, entry);
                };
                return InsertionResult::Updated(old_value);
            }
            cursor.advance();
        }

        // SAFETY: We have exclusive access to the bucket because we have exclusive access to the
        // table.
        unsafe {
            let entry = bucket.push_front(&scope, Entry::new(key, value));
            self.insertion_chain.push_back(&scope, entry);
        }
        let count = self.num_entries.fetch_add(1, Ordering::Relaxed);
        InsertionResult::Inserted(count + 1)
    }

    /// Removes a key from the map, returning the value at the key if the key
    /// was previously in the map.
    ///
    /// Concurrent readers might see the removed value until the RCU state machine has made
    /// sufficient progress to ensure that no concurrent readers are holding read guards.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let scope = RcuReadScope::new();
        let bucket = self.read_bucket(&scope, key)?;
        let mut cursor = bucket.cursor(&scope);
        while let Some(entry) = cursor.current() {
            if entry.key.borrow() == key {
                let old_value = entry.value.clone();
                // SAFETY: We have exclusive access to the bucket because we have exclusive access
                // to the table.
                unsafe {
                    let removed_entry = cursor.remove();
                    self.insertion_chain.remove(&scope, removed_entry);
                };
                self.num_entries.fetch_sub(1, Ordering::Relaxed);
                return Some(old_value);
            }
            cursor.advance();
        }
        None
    }

    /// Whether the given table needs to grow to reduce the number of collisions.
    fn needs_to_grow(&self, table: &[Bucket<K, V>]) -> bool {
        table.is_empty() || self.num_entries.load(Ordering::Relaxed) > table.len() * 2
    }

    /// Grows the table to reduce the number of collisions.
    ///
    /// Returns a reference to the new table. Callers should be sure to update the table reference
    /// they are using to the returned value.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    #[must_use]
    unsafe fn grow<'a>(
        &self,
        scope: &'a RcuReadScope,
        old_table: &[Bucket<K, V>],
    ) -> &'a [Bucket<K, V>] {
        let new_size = if old_table.is_empty() { INITIAL_CAPACITY } else { old_table.len() * 2 };
        let mut new_table = Vec::new();
        let new_insertion_chain = RcuIntrusiveList::default();
        new_table.resize_with(new_size, Default::default);

        for entry in self.insertion_chain.iter(scope) {
            let bucket = self.get_bucket(&new_table, &entry.key);
            let key = entry.key.clone();
            let value = entry.value.clone();
            // SAFETY: We have exclusive access to new_table_vec because we just created it.
            unsafe {
                let entry = bucket.push_front(&scope, Entry::new(key, value));
                new_insertion_chain.push_back(&scope, entry);
            };
        }

        self.table.update(new_table);
        // SAFETY: Our caller promises to exclude concurrent writers.
        unsafe {
            self.insertion_chain.update(&scope, new_insertion_chain);
        }
        self.table.as_slice(scope)
    }

    /// Returns a cursor that can be used to traverse and modify the map.
    ///
    /// The cursor iterates through the map in insertion order.
    pub fn cursor<'a>(&'a self, scope: &'a RcuReadScope) -> RcuRawHashMapCursor<'a, K, V, S> {
        RcuRawHashMapCursor { inner: self.insertion_chain.cursor(scope), map: self }
    }

    /// Returns an iterator over the keys in the map.
    pub fn keys<'a>(&'a self, scope: &'a RcuReadScope) -> impl Iterator<Item = &'a K> {
        self.insertion_chain.iter(scope).map(|entry| &entry.key)
    }
}

// TODO(https://fxbug.dev/482462174): switch back to #[derive(Debug)]
impl<K, V, S> std::fmt::Debug for RcuRawHashMap<K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync + std::fmt::Debug,
    V: Clone + RcuDroppable + Sync + std::fmt::Debug,
    S: std::hash::BuildHasher + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RcuRawHashMap")
            .field("table", &self.table)
            .field("num_entries", &self.num_entries)
            .field("insertion_chain", &self.insertion_chain)
            .field("hash_builder", &std::any::type_name::<S>())
            .finish_non_exhaustive()
    }
}

/// A cursor for traversing and modifying an `RcuRawHashMap`.
///
/// See `RcuRawHashMap::cursor` for more information.
pub struct RcuRawHashMapCursor<'a, K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    inner: RcuIntrusiveListCursor<'a, Entry<K, V>, InsertionAdapter>,
    map: &'a RcuRawHashMap<K, V, S>,
}

impl<'a, K, V, S> RcuRawHashMapCursor<'a, K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// Returns the element at the current cursor position.
    pub fn current(&self) -> Option<(&'a K, &'a V)> {
        self.inner.current().map(|entry| (&entry.key, &entry.value))
    }

    /// Advances the cursor to the next element in the list.
    pub fn advance(&mut self) {
        self.inner.advance()
    }

    /// Removes the element at the current cursor position.
    ///
    /// After calling `remove`, the cursor will be positioned at the next element in the list.
    ///
    /// Concurrent readers may continue to see this entry in the list until the RCU state machine
    /// has made sufficient progress to ensure that no concurrent readers are holding read guards.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn remove(&mut self) -> Option<V> {
        if let Some((key, _)) = self.current() {
            self.advance();
            // SAFETY: The caller promises to exclude concurrent writers.
            unsafe { self.map.remove(key) }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_rcu::rcu_run_callbacks;

    #[test]
    fn test_rcu_hash_map_custom_hasher() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::BuildHasherDefault;
        let hasher = BuildHasherDefault::<DefaultHasher>::default();
        let map = RcuRawHashMap::with_capacity_and_hasher(10, hasher);
        let scope = RcuReadScope::new();
        unsafe {
            map.insert(&scope, 1, 10);
        }
        assert_eq!(map.get(&scope, &1), Some(&10));
    }

    #[test]
    fn test_rcu_hash_map_insert_and_get() {
        let map = RcuRawHashMap::default();
        let scope = RcuReadScope::new();
        unsafe {
            map.insert(&scope, 1, 10);
            map.insert(&scope, 2, 20);
        }

        assert_eq!(map.get(&scope, &1), Some(&10));
        assert_eq!(map.get(&scope, &2), Some(&20));
        assert_eq!(map.get(&scope, &3), None);

        std::mem::drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_remove() {
        let map = RcuRawHashMap::default();
        let scope = RcuReadScope::new();
        unsafe {
            map.insert(&scope, 1, 10);
            map.insert(&scope, 2, 20);
        }

        assert_eq!(map.get(&scope, &1), Some(&10));

        unsafe {
            assert_eq!(map.remove(&1), Some(10));
        }

        assert_eq!(map.get(&scope, &1), None);
        assert_eq!(map.get(&scope, &2), Some(&20));

        std::mem::drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_insert_update() {
        let map = RcuRawHashMap::default();
        let scope = RcuReadScope::new();
        unsafe {
            map.insert(&scope, 1, 10);
        }

        assert_eq!(map.get(&scope, &1), Some(&10));

        let result = unsafe { map.insert(&scope, 1, 100) };
        assert!(matches!(result, InsertionResult::Updated(10)));

        assert_eq!(map.get(&scope, &1), Some(&100));

        std::mem::drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_cursor() {
        let map = RcuRawHashMap::default();
        let scope = RcuReadScope::new();
        unsafe {
            map.insert(&scope, 1, 10);
            map.insert(&scope, 2, 20);
            map.insert(&scope, 3, 30);
        }

        let mut cursor = map.cursor(&scope);

        assert_eq!(cursor.current(), Some((&1, &10)));
        cursor.advance();
        assert_eq!(cursor.current(), Some((&2, &20)));

        unsafe {
            cursor.remove();
        }

        assert_eq!(cursor.current(), Some((&3, &30)));
        assert_eq!(map.get(&scope, &2), None);

        cursor.advance();
        assert_eq!(cursor.current(), None);

        std::mem::drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_grow_maintains_order() {
        let map = RcuRawHashMap::default();
        let scope = RcuReadScope::new();
        let num_elements = INITIAL_CAPACITY * 3;
        let mut expected_order = Vec::new();

        for i in 0..num_elements {
            unsafe {
                map.insert(&scope, i, i * 10);
            }
            expected_order.push((i, i * 10));
        }

        let mut cursor = map.cursor(&scope);
        let mut actual_order = Vec::new();

        while let Some((key, value)) = cursor.current() {
            actual_order.push((*key, *value));
            cursor.advance();
        }

        assert_eq!(actual_order, expected_order);

        std::mem::drop(scope);
        rcu_run_callbacks();
    }
    #[test]
    fn test_rcu_hash_map_grow_overwrites_maintain_order() {
        let map = RcuRawHashMap::default();
        let scope = RcuReadScope::new();
        let num_elements = INITIAL_CAPACITY * 3;
        let mut expected_order = Vec::new();

        for i in 0..num_elements {
            unsafe {
                map.insert(&scope, i, i * 10);
            }
            expected_order.push((i, i * 10));
        }

        // Overwrite some existing entries and add new ones
        unsafe {
            map.insert(&scope, 5, 500);
            map.insert(&scope, INITIAL_CAPACITY * 3, (INITIAL_CAPACITY * 3) * 10); // New entry
        }
        expected_order.retain(|(k, _)| *k != 5);
        expected_order.push((5, 500));
        expected_order.push((INITIAL_CAPACITY * 3, (INITIAL_CAPACITY * 3) * 10));

        let mut cursor = map.cursor(&scope);
        let mut actual_order = Vec::new();

        while let Some((key, value)) = cursor.current() {
            actual_order.push((*key, *value));
            cursor.advance();
        }

        assert_eq!(actual_order, expected_order);

        std::mem::drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_grow() {
        let map = RcuRawHashMap::default();
        let scope = RcuReadScope::new();
        for i in 0..(INITIAL_CAPACITY * 3) {
            unsafe {
                map.insert(&scope, i, i * 10);
            }
        }

        for i in 0..(INITIAL_CAPACITY * 3) {
            assert_eq!(map.get(&scope, &i), Some(&(i * 10)));
        }

        std::mem::drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_capacity_zero() {
        let map = RcuRawHashMap::with_capacity(0);
        let scope = RcuReadScope::new();

        assert_eq!(map.get(&scope, &1), None);

        unsafe {
            map.insert(&scope, 1, 10);
        }
        assert_eq!(map.get(&scope, &1), Some(&10));

        unsafe {
            assert_eq!(map.remove(&1), Some(10));
        }
        assert_eq!(map.get(&scope, &1), None);

        std::mem::drop(scope);
        rcu_run_callbacks();
    }
}
