// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_rcu::{RcuDroppable, RcuReadScope};
use fuchsia_rcu_collections::rcu_raw_hash_map::{InsertionResult, RcuRawHashMap};
use starnix_sync::Mutex;
use std::borrow::Borrow;
use std::hash::{BuildHasher, Hash};

/// A concurrent hash map that uses RCU for read synchronization and a mutex for write synchronization.
///
/// This map allows concurrent readers to access entries without blocking, while writers are
/// synchronized via a `Mutex`.
///
/// By default, this map uses `rapidhash::RapidBuildHasher`, which provides high performance.
/// However, if this map holds keys which may be attacker-controlled, consider using
/// `std::collections::hash_map::RandomState` instead.
#[derive(RcuDroppable)]
pub struct RcuHashMap<K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    map: RcuRawHashMap<K, V, S>,
    mutex: Mutex<()>,
}

impl<K, V> Default for RcuHashMap<K, V, rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
{
    fn default() -> Self {
        Self { map: Default::default(), mutex: Mutex::new(()) }
    }
}

impl<K, V, S> RcuHashMap<K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// Creates a new hash map with the given capacity and hasher.
    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        Self {
            map: RcuRawHashMap::with_capacity_and_hasher(capacity, hash_builder),
            mutex: Mutex::new(()),
        }
    }

    /// Creates a new hash map with the given hasher.
    pub fn with_hasher(hash_builder: S) -> Self {
        Self { map: RcuRawHashMap::with_hasher(hash_builder), mutex: Mutex::new(()) }
    }

    /// Returns a reference to the value associated with the given key, if it exists.
    ///
    /// The returned reference is bound to the lifetime of the `RcuReadScope`.
    pub fn get<'a, Q>(&self, scope: &'a RcuReadScope, key: &Q) -> Option<&'a V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.get(scope, key)
    }

    /// Locks the map for exclusive access, returning a guard that allows mutation.
    pub fn lock(&self) -> RcuHashMapGuard<'_, K, V, S> {
        RcuHashMapGuard { map: &self.map, _guard: self.mutex.lock() }
    }

    /// Inserts a key-value pair into the map, returning the old value if the key was already present.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        self.lock().insert(key, value)
    }

    /// Removes a key from the map, returning the value if the key was present.
    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.lock().remove(key)
    }

    /// Returns an iterator over the map's entries.
    pub fn iter<'a>(&'a self, scope: &'a RcuReadScope) -> impl Iterator<Item = (&'a K, &'a V)> {
        let mut cursor = self.map.cursor(scope);
        std::iter::from_fn(move || {
            let current = cursor.current();
            if current.is_some() {
                cursor.advance();
            }
            current
        })
    }

    /// Returns an iterator over the map's keys.
    pub fn keys<'a>(&'a self, scope: &'a RcuReadScope) -> impl Iterator<Item = &'a K> {
        self.iter(scope).map(|(k, _)| k)
    }

    /// Returns the number of entries in the map.
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

// TODO(https://fxbug.dev/482462174): switch back to #[derive(Debug)]
impl<K, V, S> std::fmt::Debug for RcuHashMap<K, V, S>
where
    K: Eq + Hash + std::fmt::Debug + Clone + RcuDroppable + Sync,
    V: std::fmt::Debug + Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RcuHashMap").field("map", &self.map).finish()
    }
}

/// A guard that provides exclusive access to the `RcuHashMap`.
pub struct RcuHashMapGuard<'a, K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    map: &'a RcuRawHashMap<K, V, S>,
    _guard: starnix_sync::MutexGuard<'a, ()>,
}

impl<'a, K, V, S> RcuHashMapGuard<'a, K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// Returns a copy (clone) of the value associated with the given key, if it exists.
    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let scope = RcuReadScope::new();
        self.map.get(&scope, key).cloned()
    }

    /// Inserts a key-value pair into the map.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let scope = RcuReadScope::new();
        // SAFETY: We have exclusive access to the map because we have exclusive access to the mutex.
        match unsafe { self.map.insert(&scope, key, value) } {
            InsertionResult::Inserted(_) => None,
            InsertionResult::Updated(old_value) => Some(old_value),
        }
    }

    /// Removes a key from the map.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        // SAFETY: We have exclusive access to the map because we have exclusive access to the mutex.
        unsafe { self.map.remove(key) }
    }

    /// Removes all values from the map and returns them.
    pub fn drain<'b>(&'b mut self) -> impl Iterator<Item = (K, V)> + 'b {
        let scope = RcuReadScope::new();
        // We collect the keys first because we cannot iterate and modify the map at the same time.
        #[allow(clippy::needless_collect)]
        let keys: Vec<_> = self.map.keys(&scope).map(Clone::clone).collect();
        keys.into_iter().filter_map(move |k| self.remove(&k).map(|v| (k, v)))
    }

    /// Returns true if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.get(key).is_some()
    }

    /// Gets the given key's corresponding entry in the map for in-place manipulation.
    pub fn entry<'b>(&'b mut self, key: K) -> Entry<'b, 'a, K, V, S> {
        if self.get(&key).is_some() {
            Entry::Occupied(OccupiedEntry { guard: self, key })
        } else {
            Entry::Vacant(VacantEntry { guard: self, key })
        }
    }
}

/// A view into a single entry in the map, which may either be vacant or occupied.
pub enum Entry<'b, 'a, K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// An occupied entry.
    Occupied(OccupiedEntry<'b, 'a, K, V, S>),
    /// A vacant entry.
    Vacant(VacantEntry<'b, 'a, K, V, S>),
}

impl<'b, 'a, K, V, S> Entry<'b, 'a, K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// Ensures a value is in the entry by inserting the result of the default function if empty,
    /// and returns an occupied entry.
    pub fn or_insert_with<F: FnOnce() -> V>(self, default: F) -> OccupiedEntry<'b, 'a, K, V, S> {
        match self {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(entry) => entry.insert_entry(default()),
        }
    }
}

/// A view into an occupied entry in a `RcuHashMap`.
pub struct OccupiedEntry<'b, 'a, K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    guard: &'b mut RcuHashMapGuard<'a, K, V, S>,
    key: K,
}

impl<K, V, S> OccupiedEntry<'_, '_, K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// Gets a copy (clone) of the value in the entry.
    pub fn get(&self) -> V {
        self.guard.get(&self.key).unwrap()
    }

    /// Sets the value of the entry, returning the old value.
    pub fn insert(&mut self, value: V) -> V {
        self.guard.insert(self.key.clone(), value).unwrap()
    }

    /// Removes the entry from the map, returning the value.
    pub fn remove(self) -> V {
        self.guard.remove(&self.key).unwrap()
    }
}

/// A view into a vacant entry in a `RcuHashMap`.
pub struct VacantEntry<'b, 'a, K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    guard: &'b mut RcuHashMapGuard<'a, K, V, S>,
    key: K,
}

impl<'b, 'a, K, V, S> VacantEntry<'b, 'a, K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: BuildHasher + Send + Sync + 'static,
{
    /// Sets the value of the entry with the VacantEntry's key.
    pub fn insert(self, value: V) {
        self.guard.insert(self.key, value);
    }

    /// Sets the value of the entry with the VacantEntry's key, and returns an occupied entry.
    pub fn insert_entry(self, value: V) -> OccupiedEntry<'b, 'a, K, V, S> {
        self.guard.insert(self.key.clone(), value);
        OccupiedEntry { guard: self.guard, key: self.key }
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
        let map = RcuHashMap::with_capacity_and_hasher(10, hasher);
        let mut guard = map.lock();
        guard.insert(1, 10);
        assert_eq!(guard.get(&1), Some(10));
    }

    #[test]
    fn test_rcu_hash_map_insert_and_get() {
        let map = RcuHashMap::<i32, i32>::default();
        let mut guard = map.lock();
        let scope = RcuReadScope::new();

        guard.insert(1, 10);
        guard.insert(2, 20);

        assert_eq!(guard.get(&1), Some(10));
        assert_eq!(guard.get(&2), Some(20));
        assert_eq!(guard.get(&3), None);

        // Verify we can read without the lock too
        drop(guard);
        assert_eq!(map.get(&scope, &1), Some(&10));
        assert_eq!(map.get(&scope, &2), Some(&20));

        drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_update() {
        let map = RcuHashMap::<i32, i32>::default();
        let mut guard = map.lock();
        let scope = RcuReadScope::new();

        guard.insert(1, 10);
        assert_eq!(guard.get(&1), Some(10));

        guard.insert(1, 20);
        assert_eq!(guard.get(&1), Some(20));

        drop(guard);
        assert_eq!(map.get(&scope, &1), Some(&20));

        drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_remove() {
        let map = RcuHashMap::<i32, i32>::default();
        let mut guard = map.lock();
        let scope = RcuReadScope::new();

        guard.insert(1, 10);
        assert_eq!(guard.get(&1), Some(10));

        guard.remove(&1);
        assert_eq!(guard.get(&1), None);

        drop(guard);
        assert_eq!(map.get(&scope, &1), None);

        drop(scope);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_entry_api() {
        let map = RcuHashMap::<i32, i32>::default();
        let mut guard = map.lock();

        // Vacant entry
        match guard.entry(1) {
            Entry::Vacant(e) => e.insert(10),
            Entry::Occupied(_) => panic!("Should be vacant"),
        }
        assert_eq!(guard.get(&1), Some(10));

        // Occupied entry
        match guard.entry(1) {
            Entry::Occupied(mut e) => {
                assert_eq!(e.get(), 10);
                e.insert(20);
            }
            Entry::Vacant(_) => panic!("Should be occupied"),
        }
        assert_eq!(guard.get(&1), Some(20));

        drop(guard);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_hash_map_iter() {
        let map = RcuHashMap::<i32, i32>::default();
        let scope = RcuReadScope::new();
        map.insert(1, 10);
        map.insert(2, 20);
        map.insert(3, 30);

        let mut items: Vec<_> = map.iter(&scope).collect();
        items.sort_by_key(|(k, _)| **k);
        assert_eq!(items, vec![(&1, &10), (&2, &20), (&3, &30)]);
    }

    #[test]
    fn test_rcu_hash_map_keys() {
        let map = RcuHashMap::<i32, i32>::default();
        let scope = RcuReadScope::new();
        map.insert(1, 10);
        map.insert(2, 20);
        map.insert(3, 30);

        let mut keys: Vec<_> = map.keys(&scope).collect();
        keys.sort();
        assert_eq!(keys, vec![&1, &2, &3]);
    }

    #[test]
    fn test_rcu_hash_map_len() {
        let map = RcuHashMap::<i32, i32>::default();
        map.insert(1, 10);
        map.insert(2, 20);
        map.insert(3, 30);

        assert_eq!(map.len(), 3);
    }

    #[test]
    fn test_rcu_hash_map_or_insert_with() {
        let map = RcuHashMap::<i32, i32>::default();
        let mut guard = map.lock();

        // test or_insert_with
        guard.entry(1).or_insert_with(|| 10);
        assert!(guard.contains_key(&1));
        assert_eq!(guard.get(&1), Some(10));

        // test or_insert_with existing
        guard.entry(1).or_insert_with(|| 20);
        assert_eq!(guard.get(&1), Some(10));

        // test OccupiedEntry::remove
        match guard.entry(1) {
            Entry::Occupied(e) => {
                assert_eq!(e.remove(), 10);
            }
            Entry::Vacant(_) => panic!("Should be occupied"),
        }
        assert!(!guard.contains_key(&1));
    }

    #[test]
    fn test_rcu_hash_map_drain() {
        let map = RcuHashMap::<i32, i32>::default();
        let mut guard = map.lock();

        guard.insert(1, 10);
        guard.insert(2, 20);
        guard.insert(3, 30);

        let mut items: Vec<_> = guard.drain().collect();
        items.sort_by_key(|(k, _)| *k);
        assert_eq!(items, vec![(1, 10), (2, 20), (3, 30)]);

        assert!(!guard.contains_key(&1));
        assert!(!guard.contains_key(&2));
        assert!(!guard.contains_key(&3));
    }

    #[test]
    fn test_rcu_hash_map_capacity_zero() {
        use std::collections::hash_map::RandomState;
        let map =
            RcuHashMap::<i32, i32, RandomState>::with_capacity_and_hasher(0, RandomState::new());
        let mut guard = map.lock();

        assert_eq!(guard.get(&1), None);

        guard.insert(1, 10);
        assert_eq!(guard.get(&1), Some(10));

        guard.remove(&1);
        assert_eq!(guard.get(&1), None);
    }
}
