// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An ordered hash map (index map) backed by generic storage.
//!
//! [`HashMap`] maintains insertion order while providing $O(1)$ average time lookups,
//! insertions, and removals. It is suitable for `#![no_std]` and embedded environments
//! where heap allocation may be unavailable or restricted.
//!
//! # Architecture
//!
//! The map uses two contiguous vectors:
//! - A dense vector of [`StoredEntry<K, V>`] preserving the sequence of insertions.
//! - A sparse bucket array storing indices into the dense entries vector, resolved
//!   using open-address linear probing with backward-shift deletion.
//!
//! # Storage Backings
//!
//! By abstracting over [`StorageFamily`], the map can be:
//! - Stack-allocated with a fixed maximum capacity via [`StackHashMap`] (backed by [`ArrayStorage`]).
//! - Heap-allocated and dynamically growable via [`StdHashMap`] (when `std` feature is enabled).

use core::borrow::Borrow;
use core::fmt;
use core::hash::{BuildHasher, Hash, Hasher};
use core::ops::{Index, IndexMut};

use crate::map::hashers::DefaultHasher;
use crate::map::modular_math::ModularIndex;
use crate::storage::{ArrayStorage, StorageFamily};
use crate::vec::Vec;

pub mod hashers;
mod modular_math;

/// A key-value entry stored in the dense list of a [`HashMap`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEntry<K, V> {
    /// The key of the entry.
    key: K,
    /// The value of the entry.
    value: V,
}

/// Sentinel value indicating an empty bucket in the bucket array.
const EMPTY: usize = usize::MAX;

/// A hash map implemented as an index map (preserving insertion order) backed by a pair of
/// vectors generic over a [`StorageFamily`] and a hashing algorithm.
///
/// Lookups use linear probing in a bucket array of indices into an ordered entries vector.
pub struct HashMap<K, V, A: StorageFamily, S = DefaultHasher> {
    buckets: Vec<usize, A>,
    entries: Vec<StoredEntry<K, V>, A>,
    hasher: S,
}

/// An index map (ordered hash map) backed by a [`StorageFamily`].
pub type IndexMap<K, V, A, S = DefaultHasher> = HashMap<K, V, A, S>;

/// A stack-allocated hash map / index map with fixed inline capacity `SIZE`.
pub type StackHashMap<K, V, const SIZE: usize, S = DefaultHasher> =
    HashMap<K, V, ArrayStorage<SIZE>, S>;

/// A stack-allocated index map with fixed inline capacity `SIZE`.
pub type StackIndexMap<K, V, const SIZE: usize, S = DefaultHasher> = StackHashMap<K, V, SIZE, S>;

impl<K, V, A: StorageFamily, S: Default> Default for HashMap<K, V, A, S>
where
    A::Storage<usize>: Default,
    A::Storage<StoredEntry<K, V>>: Default,
{
    fn default() -> Self {
        Self {
            buckets: Default::default(),
            entries: Default::default(),
            hasher: Default::default(),
        }
    }
}

impl<K, V, A: StorageFamily, S: Default> HashMap<K, V, A, S> {
    /// Creates an empty `HashMap` with the default hasher.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Creates a new `HashMap` using explicit allocators for buckets and entries.
    pub fn new_in(
        buckets_allocator: A::Storage<usize>,
        entries_allocator: A::Storage<StoredEntry<K, V>>,
    ) -> Self {
        Self {
            buckets: Vec::new_in(buckets_allocator),
            entries: Vec::new_in(entries_allocator),
            hasher: S::default(),
        }
    }
}

impl<K, V, A: StorageFamily, S> HashMap<K, V, A, S> {
    /// Creates an empty `HashMap` with the specified hasher.
    pub fn with_hasher(hasher: S) -> Self
    where
        A::Storage<usize>: Default,
        A::Storage<StoredEntry<K, V>>: Default,
    {
        Self { buckets: Default::default(), entries: Default::default(), hasher }
    }

    /// Creates a new `HashMap` using the specified hasher and allocators.
    pub fn with_hasher_in(
        hasher: S,
        buckets_allocator: A::Storage<usize>,
        entries_allocator: A::Storage<StoredEntry<K, V>>,
    ) -> Self {
        Self {
            buckets: Vec::new_in(buckets_allocator),
            entries: Vec::new_in(entries_allocator),
            hasher,
        }
    }

    /// Returns the number of entries currently in the map.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the map contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.len() == 0
    }

    /// Returns the current entry capacity of the map, representing the minimum
    /// capacity between entry storage and bucket slots.
    pub fn capacity(&self) -> usize {
        self.entries.capacity().min(self.buckets.len())
    }

    /// Reserves capacity for at least `additional` more elements to be inserted into the map.
    ///
    /// Sizing is synchronized across internal collections: it attempts ideal power-of-two growth,
    /// backs off to the exact required capacity upon allocation constraints, adopts all available
    /// capacity for statically-sized storages, and rehashes once if bucket count changed.
    ///
    /// # Errors
    /// Returns `Err(AllocError)` if the map cannot grow its underlying storage to accommodate the
    /// requested capacity.
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), crate::AllocError>
    where
        K: Hash,
        S: BuildHasher,
    {
        let needed_capacity = self.len().checked_add(additional).ok_or(crate::AllocError)?;
        if needed_capacity <= self.capacity() {
            return Ok(());
        }

        let old_buckets_len = self.buckets.len();

        // 1. Calculate ideal capacity (doubling current capacity, or initial power of 2 >= 4 for dynamic collections).
        let current_cap = self.capacity();
        let ideal_capacity = if current_cap == 0 {
            needed_capacity.max(4).checked_next_power_of_two().unwrap_or(needed_capacity)
        } else {
            current_cap
                .checked_mul(2)
                .unwrap_or(usize::MAX)
                .max(needed_capacity)
                .checked_next_power_of_two()
                .unwrap_or(needed_capacity)
        };

        // 2. Grow entries storage: try ideal capacity first, then back off to needed_capacity.
        let needed_entries_additional = needed_capacity.saturating_sub(self.entries.len());
        let ideal_entries_additional = ideal_capacity.saturating_sub(self.entries.len());
        if self.entries.try_reserve(ideal_entries_additional).is_err() {
            self.entries.try_reserve(needed_entries_additional)?;
        }

        // 3. Resize buckets storage to match entries capacity, backing off to needed_capacity if constrained.
        let target_buckets = self.entries.capacity();
        if self.buckets.try_resize(target_buckets, EMPTY).is_err() {
            self.buckets.try_resize(needed_capacity, EMPTY)?;
        }

        // 4. Rehash existing entries if bucket count changed.
        if self.buckets.len() != old_buckets_len && !self.entries.is_empty() {
            self.rehash();
        }

        Ok(())
    }

    /// Reserves capacity for at least `additional` more elements to be inserted into the map.
    ///
    /// # Panics
    /// Panics if the map cannot grow its underlying storage.
    pub fn reserve(&mut self, additional: usize)
    where
        K: Hash,
        S: BuildHasher,
    {
        self.try_reserve(additional).expect("failed to reserve map capacity");
    }

    /// Returns a reference to the underlying hasher.
    pub fn hasher(&self) -> &S {
        &self.hasher
    }

    /// Clears the map, removing all entries and resetting all buckets to empty.
    pub fn clear(&mut self) {
        self.entries.clear();
        for bucket in self.buckets.iter_mut() {
            *bucket = EMPTY;
        }
    }

    /// Computes the 64-bit hash of the given key using the configured [`BuildHasher`].
    fn hash<Q: ?Sized>(&self, key: &Q) -> u64
    where
        Q: Hash,
        S: BuildHasher,
    {
        let mut hasher = self.hasher.build_hasher();
        key.hash(&mut hasher);
        hasher.finish()
    }

    /// Finds the index in `self.entries` corresponding to `key` by computing its hash and probing buckets.
    fn find_entry_index<Q: ?Sized>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        let hash = self.hash(key);
        self.find_entry_index_with_hash(key, hash)
    }

    /// Finds the index in `self.entries` corresponding to `key` given its precomputed `hash`.
    fn find_entry_index_with_hash<Q: ?Sized>(&self, key: &Q, hash: u64) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Eq,
    {
        let number_of_buckets = self.buckets.len();
        if number_of_buckets == 0 {
            return None;
        }
        let mut bucket_index = ModularIndex::new(hash as usize, number_of_buckets);
        for _ in 0..number_of_buckets {
            let entry_index = self.buckets[bucket_index.get()];
            if entry_index == EMPTY {
                return None;
            }
            let entry = &self.entries[entry_index];
            if entry.key.borrow() == key {
                return Some(entry_index);
            }
            bucket_index = bucket_index.increment();
        }
        None
    }

    /// Finds the bucket slot index currently holding `entry_index` with the given `hash`.
    fn find_bucket_index_of(&self, hash: u64, entry_index: usize) -> Option<usize> {
        let number_of_buckets = self.buckets.len();
        if number_of_buckets == 0 {
            return None;
        }
        let mut bucket_index = ModularIndex::new(hash as usize, number_of_buckets);
        for _ in 0..number_of_buckets {
            if self.buckets[bucket_index.get()] == entry_index {
                return Some(bucket_index.get());
            }
            if self.buckets[bucket_index.get()] == EMPTY {
                break;
            }
            bucket_index = bucket_index.increment();
        }
        None
    }

    /// Removes the bucket entry at `initial_hole` and backward-shifts subsequent colliding entries
    /// to preserve linear probing invariants.
    fn remove_bucket_at(&mut self, initial_hole: usize)
    where
        K: Hash,
        S: BuildHasher,
    {
        let number_of_buckets = self.buckets.len();
        if number_of_buckets == 0 {
            return;
        }
        let mut hole = initial_hole;
        let mut scan = ModularIndex::new(initial_hole + 1, number_of_buckets);
        while scan.get() != initial_hole && self.buckets[scan.get()] != EMPTY {
            let entry_index = self.buckets[scan.get()];
            let hash = self.hash(&self.entries[entry_index].key);
            let natural_bucket = (hash as usize) % number_of_buckets;
            let distance_to_scan =
                (scan.get() + (number_of_buckets - natural_bucket)) % number_of_buckets;
            let distance_to_hole =
                (hole + (number_of_buckets - natural_bucket)) % number_of_buckets;
            if distance_to_hole < distance_to_scan {
                self.buckets[hole] = entry_index;
                hole = scan.get();
            }
            scan = scan.increment();
        }
        self.buckets[hole] = EMPTY;
    }

    /// Clears and reconstructs the bucket table from the current entries.
    fn rehash(&mut self)
    where
        K: Hash,
        S: BuildHasher,
    {
        for bucket in self.buckets.iter_mut() {
            *bucket = EMPTY;
        }
        let number_of_buckets = self.buckets.len();
        if number_of_buckets == 0 {
            return;
        }
        for (index, entry) in self.entries.iter().enumerate() {
            let hash = self.hash(&entry.key);
            let mut probe = ModularIndex::new(hash as usize, number_of_buckets);
            loop {
                if self.buckets[probe.get()] == EMPTY {
                    self.buckets[probe.get()] = index;
                    break;
                }
                probe = probe.increment();
            }
        }
    }

    /// Removes the entry at `index` by swapping it with the last entry, updating bucket references.
    fn swap_remove_entry_index(&mut self, index: usize) -> (K, V)
    where
        K: Hash,
        S: BuildHasher,
    {
        let hash = self.hash(&self.entries[index].key);
        if let Some(bucket_index) = self.find_bucket_index_of(hash, index) {
            self.remove_bucket_at(bucket_index);
        }
        let last_index = self.entries.len() - 1;
        if index != last_index {
            let last_hash = self.hash(&self.entries[last_index].key);
            if let Some(last_bucket_index) = self.find_bucket_index_of(last_hash, last_index) {
                self.buckets[last_bucket_index] = index;
            }
            (&mut *self.entries).swap(index, last_index);
        }
        let entry = self.entries.pop().expect("entry must exist");
        (entry.key, entry.value)
    }

    /// Removes the entry at `index` by shifting subsequent entries left, updating bucket references.
    fn shift_remove_entry_index(&mut self, index: usize) -> (K, V)
    where
        K: Hash,
        S: BuildHasher,
    {
        let hash = self.hash(&self.entries[index].key);
        if let Some(bucket_index) = self.find_bucket_index_of(hash, index) {
            self.remove_bucket_at(bucket_index);
        }
        let entry = self.entries.remove(index);
        for bucket in self.buckets.iter_mut() {
            if *bucket != EMPTY && *bucket > index {
                *bucket -= 1;
            }
        }
        (entry.key, entry.value)
    }

    /// Returns `true` if the map contains an entry for the specified key.
    pub fn contains_key<Q: ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        self.find_entry_index(key).is_some()
    }

    /// Returns a shared reference to the value corresponding to the key.
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        let index = self.find_entry_index(key)?;
        Some(&self.entries[index].value)
    }

    /// Returns a mutable reference to the value corresponding to the key.
    pub fn get_mut<Q: ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        let index = self.find_entry_index(key)?;
        Some(&mut self.entries[index].value)
    }

    /// Returns shared references to the key and value corresponding to the key.
    pub fn get_key_value<Q: ?Sized>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        let index = self.find_entry_index(key)?;
        let entry = &self.entries[index];
        Some((&entry.key, &entry.value))
    }

    /// Returns shared references to the key and value at the specified index in insertion order.
    ///
    /// Returns `None` if `index >= self.len()`.
    pub fn get_index(&self, index: usize) -> Option<(&K, &V)> {
        let entry = self.entries.get(index)?;
        Some((&entry.key, &entry.value))
    }

    /// Returns a shared reference to the key and a mutable reference to the value at the specified
    /// index in insertion order.
    ///
    /// Returns `None` if `index >= self.len()`.
    pub fn get_index_mut(&mut self, index: usize) -> Option<(&K, &mut V)> {
        let entry = self.entries.get_mut(index)?;
        Some((&entry.key, &mut entry.value))
    }

    /// Gets the given key's corresponding entry in the map for in-place manipulation.
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V, A, S>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        let hash = self.hash(&key);
        if let Some(index) = self.find_entry_index_with_hash(&key, hash) {
            Entry::Occupied(OccupiedEntry { map: self, index })
        } else {
            Entry::Vacant(VacantEntry { map: self, key, hash })
        }
    }

    /// Attempts to insert a key-value pair into the map.
    ///
    /// If the map did not have this key present, `Ok(None)` is returned.
    /// If the map did have this key present, the value is updated and `Ok(Some(old_value))` is returned.
    ///
    /// # Errors
    /// Returns `Err((key, value))` if the table is full and cannot grow its underlying storage.
    pub fn try_insert(&mut self, key: K, value: V) -> Result<Option<V>, (K, V)>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self.entry(key) {
            Entry::Occupied(mut occupied) => Ok(Some(occupied.insert(value))),
            Entry::Vacant(vacant) => match vacant.try_insert(value) {
                Ok(_) => Ok(None),
                Err(err) => Err(err),
            },
        }
    }

    /// Inserts a key-value pair into the map.
    ///
    /// # Panics
    /// Panics if the map is full and cannot grow its underlying storage.
    pub fn insert(&mut self, key: K, value: V) -> Option<V>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self.try_insert(key, value) {
            Ok(old_value) => old_value,
            Err(_) => panic!("HashMap is full and cannot grow"),
        }
    }

    /// Removes the key-value pair at the specified index by swapping it with the last entry,
    /// returning the removed pair.
    ///
    /// This alters the relative order of remaining entries but runs in `O(1)` average time.
    /// Returns `None` if `index >= self.len()`.
    pub fn swap_remove_index(&mut self, index: usize) -> Option<(K, V)>
    where
        K: Hash,
        S: BuildHasher,
    {
        if index >= self.entries.len() { None } else { Some(self.swap_remove_entry_index(index)) }
    }

    /// Removes the key-value pair at the specified index, shifting all subsequent entries
    /// to preserve insertion order, returning the removed pair.
    ///
    /// This runs in `O(n)` time.
    /// Returns `None` if `index >= self.len()`.
    pub fn shift_remove_index(&mut self, index: usize) -> Option<(K, V)>
    where
        K: Hash,
        S: BuildHasher,
    {
        if index >= self.entries.len() { None } else { Some(self.shift_remove_entry_index(index)) }
    }

    /// Removes a key-value pair from the map in `O(1)` average time by swapping the removed
    /// entry with the last entry.
    ///
    /// Returns `None` if the key is not present.
    pub fn swap_remove_entry<Q: ?Sized>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q> + Hash,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        let index = self.find_entry_index(key)?;
        Some(self.swap_remove_entry_index(index))
    }

    /// Removes a key-value pair from the map, returning the pair if present.
    ///
    /// This is an alias for [`HashMap::swap_remove_entry`].
    pub fn remove_entry<Q: ?Sized>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q> + Hash,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        self.swap_remove_entry(key)
    }

    /// Removes a key-value pair from the map, preserving the relative insertion order of remaining entries.
    ///
    /// Operates in `O(n)` time. Returns `None` if the key is not present.
    pub fn shift_remove_entry<Q: ?Sized>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q> + Hash,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        let index = self.find_entry_index(key)?;
        Some(self.shift_remove_entry_index(index))
    }

    /// Removes a key from the map in `O(1)` average time by swapping the removed entry with the
    /// last entry in the underlying entries vector.
    ///
    /// Note that this may alter the relative insertion order of remaining entries.
    pub fn swap_remove<Q: ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q> + Hash,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        self.swap_remove_entry(key).map(|(_, value)| value)
    }

    /// Removes a key from the map, returning its value if present.
    ///
    /// This is an alias for [`HashMap::swap_remove`], which operates in `O(1)` average time.
    /// If you need to preserve the insertion order of remaining entries, use [`HashMap::shift_remove`].
    pub fn remove<Q: ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q> + Hash,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        self.swap_remove(key)
    }

    /// Removes a key from the map, preserving the relative insertion order of remaining entries.
    ///
    /// This operates in `O(n)` time since entries following the removed entry must be shifted.
    pub fn shift_remove<Q: ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q> + Hash,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        self.shift_remove_entry(key).map(|(_, value)| value)
    }

    /// Retains only the entries specified by the predicate.
    ///
    /// In other words, remove all entries `(k, v)` for which `f(&k, &mut v)` returns `false`.
    /// Preserves the insertion order of remaining entries.
    ///
    /// Operates in `O(n)` time by compacting the entries vector in-place and rebuilding
    /// the bucket table.
    pub fn retain<F>(&mut self, mut predicate: F)
    where
        F: FnMut(&K, &mut V) -> bool,
        K: Hash,
        S: BuildHasher,
    {
        let len = self.entries.len();
        let mut write_index = 0;
        for read_index in 0..len {
            let keep = {
                let entry = &mut self.entries[read_index];
                predicate(&entry.key, &mut entry.value)
            };
            if keep {
                if read_index != write_index {
                    (&mut *self.entries).swap(read_index, write_index);
                }
                write_index += 1;
            }
        }
        if write_index != len {
            self.entries.truncate(write_index);
            self.rehash();
        }
    }

    /// Returns an iterator over `(&K, &V)` pairs in insertion order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter { inner: self.entries.iter() }
    }

    /// Returns an iterator over `(&K, &mut V)` pairs in insertion order.
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut { inner: self.entries.iter_mut() }
    }

    /// Returns an iterator over references to keys in insertion order.
    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys { inner: self.entries.iter() }
    }

    /// Returns an iterator over references to values in insertion order.
    pub fn values(&self) -> Values<'_, K, V> {
        Values { inner: self.entries.iter() }
    }

    /// Returns an iterator over mutable references to values in insertion order.
    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        ValuesMut { inner: self.entries.iter_mut() }
    }
}

impl<K, V, A: StorageFamily, S: Clone> Clone for HashMap<K, V, A, S>
where
    K: Clone + Hash + Eq,
    V: Clone,
    A::Storage<usize>: Default,
    A::Storage<StoredEntry<K, V>>: Default,
    S: BuildHasher,
{
    fn clone(&self) -> Self {
        let mut map = Self::with_hasher(self.hasher.clone());
        for (key, value) in self.iter() {
            let _ = map.try_insert(key.clone(), value.clone());
        }
        map
    }
}

impl<K, V, A, S> PartialEq for HashMap<K, V, A, S>
where
    K: Hash + Eq,
    V: PartialEq,
    A: StorageFamily,
    S: BuildHasher,
{
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        for (key, value) in self.iter() {
            match other.get(key) {
                Some(other_value) if *value == *other_value => {}
                _ => return false,
            }
        }
        true
    }
}

impl<K, V, A, S> Eq for HashMap<K, V, A, S>
where
    K: Hash + Eq,
    V: Eq,
    A: StorageFamily,
    S: BuildHasher,
{
}

impl<K: fmt::Debug, V: fmt::Debug, A: StorageFamily, S> fmt::Debug for HashMap<K, V, A, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V, Q: ?Sized, A: StorageFamily, S> Index<&Q> for HashMap<K, V, A, S>
where
    K: Borrow<Q>,
    Q: Hash + Eq,
    S: BuildHasher,
{
    type Output = V;

    fn index(&self, index: &Q) -> &Self::Output {
        self.get(index).expect("no entry found for key")
    }
}

impl<K, V, Q: ?Sized, A: StorageFamily, S> IndexMut<&Q> for HashMap<K, V, A, S>
where
    K: Borrow<Q>,
    Q: Hash + Eq,
    S: BuildHasher,
{
    fn index_mut(&mut self, index: &Q) -> &mut Self::Output {
        self.get_mut(index).expect("no entry found for key")
    }
}

/// A view into a single entry in a map, which may either be vacant or occupied.
pub enum Entry<'a, K, V, A: StorageFamily, S> {
    /// An occupied entry.
    Occupied(OccupiedEntry<'a, K, V, A, S>),
    /// A vacant entry.
    Vacant(VacantEntry<'a, K, V, A, S>),
}

/// A view into an occupied entry in a [`HashMap`].
pub struct OccupiedEntry<'a, K, V, A: StorageFamily, S> {
    map: &'a mut HashMap<K, V, A, S>,
    index: usize,
}

/// A view into a vacant entry in a [`HashMap`].
pub struct VacantEntry<'a, K, V, A: StorageFamily, S> {
    map: &'a mut HashMap<K, V, A, S>,
    key: K,
    hash: u64,
}

impl<'a, K: fmt::Debug, V: fmt::Debug, A: StorageFamily, S> fmt::Debug for Entry<'a, K, V, A, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Entry::Occupied(occupied) => f.debug_tuple("Entry").field(occupied).finish(),
            Entry::Vacant(vacant) => f.debug_tuple("Entry").field(vacant).finish(),
        }
    }
}

impl<'a, K: fmt::Debug, V: fmt::Debug, A: StorageFamily, S> fmt::Debug
    for OccupiedEntry<'a, K, V, A, S>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OccupiedEntry").field("key", self.key()).field("value", self.get()).finish()
    }
}

impl<'a, K: fmt::Debug, V, A: StorageFamily, S> fmt::Debug for VacantEntry<'a, K, V, A, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VacantEntry").field(self.key()).finish()
    }
}

impl<'a, K, V, A: StorageFamily, S> Entry<'a, K, V, A, S> {
    /// Ensures a value is in the entry by inserting the default if empty, and returns
    /// a mutable reference to the value in the entry.
    ///
    /// # Panics
    /// Panics if the map is full and cannot grow its underlying storage.
    pub fn or_insert(self, default: V) -> &'a mut V
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => vacant.insert(default),
        }
    }

    /// Attempts to ensure a value is in the entry by inserting the default if empty,
    /// returning a mutable reference to the value in the entry.
    ///
    /// # Errors
    /// Returns `Err((key, default))` if the map is full and cannot grow its underlying storage.
    pub fn try_or_insert(self, default: V) -> Result<&'a mut V, (K, V)>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self {
            Entry::Occupied(occupied) => Ok(occupied.into_mut()),
            Entry::Vacant(vacant) => vacant.try_insert(default),
        }
    }

    /// Ensures a value is in the entry by inserting the result of the default function if empty,
    /// and returns a mutable reference to the value in the entry.
    ///
    /// # Panics
    /// Panics if the map is full and cannot grow its underlying storage.
    pub fn or_insert_with<F: FnOnce() -> V>(self, default: F) -> &'a mut V
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => vacant.insert(default()),
        }
    }

    /// Attempts to ensure a value is in the entry by inserting the result of the default function if empty.
    ///
    /// # Errors
    /// Returns `Err(key)` if the map is full and cannot grow its underlying storage.
    pub fn try_or_insert_with<F: FnOnce() -> V>(self, default: F) -> Result<&'a mut V, K>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self {
            Entry::Occupied(occupied) => Ok(occupied.into_mut()),
            Entry::Vacant(vacant) => match vacant.try_insert(default()) {
                Ok(value) => Ok(value),
                Err((key, _)) => Err(key),
            },
        }
    }

    /// Ensures a value is in the entry by inserting the result of the default function
    /// (which receives a reference to the key) if empty.
    ///
    /// # Panics
    /// Panics if the map is full and cannot grow its underlying storage.
    pub fn or_insert_with_key<F: FnOnce(&K) -> V>(self, default: F) -> &'a mut V
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => {
                let value = default(vacant.key());
                vacant.insert(value)
            }
        }
    }

    /// Returns a reference to this entry's key.
    pub fn key(&self) -> &K {
        match self {
            Entry::Occupied(occupied) => occupied.key(),
            Entry::Vacant(vacant) => vacant.key(),
        }
    }

    /// Provides in-place mutable access to an occupied entry before any potential inserts into the map.
    pub fn and_modify<F: FnOnce(&mut V)>(mut self, function: F) -> Self {
        if let Entry::Occupied(occupied) = &mut self {
            function(occupied.get_mut());
        }
        self
    }

    /// Ensures a value is in the entry by inserting the default value if empty.
    ///
    /// # Panics
    /// Panics if the map is full and cannot grow its underlying storage.
    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
        K: Hash + Eq,
        S: BuildHasher,
    {
        self.or_insert_with(Default::default)
    }
}

impl<'a, K, V, A: StorageFamily, S> OccupiedEntry<'a, K, V, A, S> {
    /// Gets a reference to the key in the entry.
    pub fn key(&self) -> &K {
        &self.map.entries[self.index].key
    }

    /// Gets a reference to the value in the entry.
    pub fn get(&self) -> &V {
        &self.map.entries[self.index].value
    }

    /// Gets a mutable reference to the value in the entry.
    pub fn get_mut(&mut self) -> &mut V {
        &mut self.map.entries[self.index].value
    }

    /// Converts the entry into a mutable reference to the value in the entry
    /// with a lifetime bound to the map itself.
    pub fn into_mut(self) -> &'a mut V {
        &mut self.map.entries[self.index].value
    }

    /// Sets the value of the entry, and returns the entry's old value.
    pub fn insert(&mut self, value: V) -> V {
        core::mem::replace(self.get_mut(), value)
    }

    /// Takes the value out of the entry, and returns it.
    /// Operates in `O(1)` average time by swapping with the last element in `entries`.
    pub fn remove(self) -> V
    where
        K: Hash,
        S: BuildHasher,
    {
        self.remove_entry().1
    }

    /// Takes the key and value out of the entry, and returns them.
    /// Operates in `O(1)` average time by swapping with the last element in `entries`.
    pub fn remove_entry(self) -> (K, V)
    where
        K: Hash,
        S: BuildHasher,
    {
        self.map.swap_remove_entry_index(self.index)
    }

    /// Takes the value out of the entry, and returns it.
    /// Operates in `O(n)` time, preserving the insertion order of remaining entries.
    pub fn shift_remove(self) -> V
    where
        K: Hash,
        S: BuildHasher,
    {
        self.shift_remove_entry().1
    }

    /// Takes the key and value out of the entry, and returns them.
    /// Operates in `O(n)` time, preserving the insertion order of remaining entries.
    pub fn shift_remove_entry(self) -> (K, V)
    where
        K: Hash,
        S: BuildHasher,
    {
        self.map.shift_remove_entry_index(self.index)
    }
}

impl<'a, K, V, A: StorageFamily, S> VacantEntry<'a, K, V, A, S> {
    /// Gets a reference to the key that would be used when inserting a value through the `VacantEntry`.
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Takes ownership of the key.
    pub fn into_key(self) -> K {
        self.key
    }

    /// Sets the value of the entry with the VacantEntry's key,
    /// and returns a mutable reference to it.
    ///
    /// # Panics
    /// Panics if the map is full and cannot grow its underlying storage.
    pub fn insert(self, value: V) -> &'a mut V
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        match self.try_insert(value) {
            Ok(val) => val,
            Err(_) => panic!("HashMap is full and cannot grow"),
        }
    }

    /// Attempts to set the value of the entry with the VacantEntry's key,
    /// returning a mutable reference to it.
    ///
    /// # Errors
    /// Returns `Err((key, value))` if the map is full and cannot grow its underlying storage.
    pub fn try_insert(self, value: V) -> Result<&'a mut V, (K, V)>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        if self.map.try_reserve(1).is_err() {
            return Err((self.key, value));
        }

        let entry = StoredEntry { key: self.key, value };
        self.map.entries.try_push(entry).unwrap_or_else(|_| panic!("Reserved above for 1 element"));

        let new_index = self.map.entries.len() - 1;
        let number_of_buckets = self.map.buckets.len();
        let mut bucket_index = ModularIndex::new(self.hash as usize, number_of_buckets);
        loop {
            if self.map.buckets[bucket_index.get()] == EMPTY {
                self.map.buckets[bucket_index.get()] = new_index;
                break;
            }
            bucket_index = bucket_index.increment();
        }

        Ok(&mut self.map.entries[new_index].value)
    }
}

/// An iterator yielding shared references to key-value pairs in a [`HashMap`] in insertion order.
pub struct Iter<'a, K, V> {
    inner: core::slice::Iter<'a, StoredEntry<K, V>>,
}

impl<'a, K, V> Clone for Iter<'a, K, V> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for Iter<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.inner.next()?;
        Some((&entry.key, &entry.value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for Iter<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let entry = self.inner.next_back()?;
        Some((&entry.key, &entry.value))
    }
}

impl<'a, K, V> ExactSizeIterator for Iter<'a, K, V> {}
impl<'a, K, V> core::iter::FusedIterator for Iter<'a, K, V> {}

/// An iterator yielding references to keys in a [`HashMap`] in insertion order.
pub struct Keys<'a, K, V> {
    inner: core::slice::Iter<'a, StoredEntry<K, V>>,
}

impl<'a, K, V> Clone for Keys<'a, K, V> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<'a, K: fmt::Debug, V> fmt::Debug for Keys<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;

    fn next(&mut self) -> Option<Self::Item> {
        Some(&self.inner.next()?.key)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for Keys<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        Some(&self.inner.next_back()?.key)
    }
}

impl<'a, K, V> ExactSizeIterator for Keys<'a, K, V> {}
impl<'a, K, V> core::iter::FusedIterator for Keys<'a, K, V> {}

/// An iterator yielding references to values in a [`HashMap`] in insertion order.
pub struct Values<'a, K, V> {
    inner: core::slice::Iter<'a, StoredEntry<K, V>>,
}

impl<'a, K, V> Clone for Values<'a, K, V> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<'a, K, V: fmt::Debug> fmt::Debug for Values<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<Self::Item> {
        Some(&self.inner.next()?.value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for Values<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        Some(&self.inner.next_back()?.value)
    }
}

impl<'a, K, V> ExactSizeIterator for Values<'a, K, V> {}
impl<'a, K, V> core::iter::FusedIterator for Values<'a, K, V> {}

/// An iterator yielding mutable references to key-value pairs in a [`HashMap`] in insertion order.
pub struct IterMut<'a, K, V> {
    inner: core::slice::IterMut<'a, StoredEntry<K, V>>,
}

impl<'a, K, V> fmt::Debug for IterMut<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IterMut").field("remaining", &self.inner.len()).finish()
    }
}

impl<'a, K, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.inner.next()?;
        Some((&entry.key, &mut entry.value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for IterMut<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let entry = self.inner.next_back()?;
        Some((&entry.key, &mut entry.value))
    }
}

impl<'a, K, V> ExactSizeIterator for IterMut<'a, K, V> {}
impl<'a, K, V> core::iter::FusedIterator for IterMut<'a, K, V> {}

/// An iterator yielding mutable references to values in a [`HashMap`] in insertion order.
pub struct ValuesMut<'a, K, V> {
    inner: core::slice::IterMut<'a, StoredEntry<K, V>>,
}

impl<'a, K, V> fmt::Debug for ValuesMut<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValuesMut").field("remaining", &self.inner.len()).finish()
    }
}

impl<'a, K, V> Iterator for ValuesMut<'a, K, V> {
    type Item = &'a mut V;

    fn next(&mut self) -> Option<Self::Item> {
        Some(&mut self.inner.next()?.value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for ValuesMut<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        Some(&mut self.inner.next_back()?.value)
    }
}

impl<'a, K, V> ExactSizeIterator for ValuesMut<'a, K, V> {}
impl<'a, K, V> core::iter::FusedIterator for ValuesMut<'a, K, V> {}

/// An owning iterator yielding key-value pairs of a [`HashMap`] in insertion order.
///
/// Iteration executes in $O(1)$ time per element and $O(N)$ total time, with safe RAII cleanup
/// of unconsumed entries on drop.
pub struct IntoIter<K, V, A: StorageFamily> {
    entries: Vec<StoredEntry<K, V>, A>,
    start_index: usize,
    end_index: usize,
}

impl<K, V, A: StorageFamily> IntoIter<K, V, A> {
    fn new<S>(map: HashMap<K, V, A, S>) -> Self {
        let mut entries = map.entries;
        let end_index = entries.len();
        // SAFETY:
        // Setting `len` to 0 prevents `Vec::drop` from attempting to drop elements in `entries`.
        // `IntoIter` takes over exclusive ownership and manages destruction of elements
        // in `start_index..end_index` through `IntoIter::drop` and `next`/`next_back`.
        unsafe {
            entries.set_len(0);
        }
        Self { entries, start_index: 0, end_index }
    }
}

impl<K, V, A: StorageFamily> Iterator for IntoIter<K, V, A> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        if self.start_index >= self.end_index {
            None
        } else {
            let index = self.start_index;
            self.start_index += 1;
            // SAFETY:
            // - `index` is within `0..self.end_index`, which was initialized upon `IntoIter` creation.
            // - Each index is read at most once because `self.start_index` is incremented.
            // - `self.entries.as_ptr().add(index)` points to an initialized `StoredEntry<K, V>`.
            let entry = unsafe { core::ptr::read(self.entries.as_ptr().add(index)) };
            Some((entry.key, entry.value))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let length = self.end_index - self.start_index;
        (length, Some(length))
    }
}

impl<K, V, A: StorageFamily> DoubleEndedIterator for IntoIter<K, V, A> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.start_index >= self.end_index {
            None
        } else {
            self.end_index -= 1;
            let index = self.end_index;
            // SAFETY:
            // - `index` is within `self.start_index..self.end_index`, pointing to an initialized `StoredEntry<K, V>`.
            // - Each index is read at most once because `self.end_index` is decremented prior to reading.
            let entry = unsafe { core::ptr::read(self.entries.as_ptr().add(index)) };
            Some((entry.key, entry.value))
        }
    }
}

impl<K, V, A: StorageFamily> ExactSizeIterator for IntoIter<K, V, A> {}
impl<K, V, A: StorageFamily> core::iter::FusedIterator for IntoIter<K, V, A> {}

impl<K, V, A: StorageFamily> Drop for IntoIter<K, V, A> {
    fn drop(&mut self) {
        for index in self.start_index..self.end_index {
            // SAFETY:
            // - Elements from `self.start_index..self.end_index` were initialized and have not been read
            //   via `next()` or `next_back()`.
            // - Dropping them in place frees resources without double-dropping.
            unsafe {
                core::ptr::drop_in_place(self.entries.as_mut_ptr().add(index));
            }
        }
    }
}

impl<K, V, A: StorageFamily> fmt::Debug for IntoIter<K, V, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntoIter").field("remaining", &(self.end_index - self.start_index)).finish()
    }
}

impl<'a, K, V, A: StorageFamily, S> IntoIterator for &'a HashMap<K, V, A, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K, V, A: StorageFamily, S> IntoIterator for &'a mut HashMap<K, V, A, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<K, V, A: StorageFamily, S> IntoIterator for HashMap<K, V, A, S> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V, A>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter::new(self)
    }
}

/// A heap-allocated hash map backed by the global allocator.
#[cfg(feature = "std")]
pub type StdHashMap<K, V, S = DefaultHasher> = HashMap<K, V, crate::storage::Global, S>;

/// A heap-allocated index map backed by the global allocator.
#[cfg(feature = "std")]
pub type StdIndexMap<K, V, S = DefaultHasher> = IndexMap<K, V, crate::storage::Global, S>;

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn test_stack_hash_map_zero_capacity() {
        let mut map = StackHashMap::<i32, &'static str, 0>::new();
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
        assert_eq!(map.capacity(), 0);
        assert_eq!(map.get(&1), None);
        assert!(!map.contains_key(&1));
        assert_eq!(map.get_index(0), None);
        assert_eq!(map.swap_remove(&1), None);
        assert_eq!(map.shift_remove(&1), None);
        assert_eq!(map.swap_remove_index(0), None);
        assert_eq!(map.shift_remove_index(0), None);

        // Insertion must fail gracefully
        assert_eq!(map.try_insert(1, "one"), Err((1, "one")));
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn test_stack_hash_map_one_capacity() {
        let mut map = StackHashMap::<i32, &'static str, 1>::new();
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());

        assert_eq!(map.try_insert(1, "one"), Ok(None));
        assert_eq!(map.len(), 1);
        assert_eq!(map.capacity(), 1);
        assert_eq!(map.get(&1), Some(&"one"));
        assert_eq!(map.get_index(0), Some((&1, &"one")));
        assert_eq!(map.get_index(1), None);

        // Overwrite existing key
        assert_eq!(map.try_insert(1, "ONE"), Ok(Some("one")));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&1), Some(&"ONE"));

        // Second item fails because capacity is 1
        assert_eq!(map.try_insert(2, "two"), Err((2, "two")));
        assert_eq!(map.len(), 1);

        // Removal allows re-inserting
        assert_eq!(map.remove(&1), Some("ONE"));
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
        assert_eq!(map.get(&1), None);

        assert_eq!(map.try_insert(2, "two"), Ok(None));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&2), Some(&"two"));
    }

    #[test]
    fn test_stack_hash_map_non_power_of_two_capacities() {
        // Test N = 3
        let mut map3 = StackHashMap::<i32, i32, 3>::new();
        for i in 0..3 {
            assert_eq!(map3.try_insert(i, i * 10), Ok(None));
        }
        assert_eq!(map3.len(), 3);
        assert_eq!(map3.try_insert(99, 990), Err((99, 990)));
        for i in 0..3 {
            assert_eq!(map3.get(&i), Some(&(i * 10)));
        }

        // Test N = 5
        let mut map5 = StackHashMap::<i32, i32, 5>::new();
        for i in 0..5 {
            assert_eq!(map5.try_insert(i, i * 10), Ok(None));
        }
        assert_eq!(map5.len(), 5);
        assert_eq!(map5.try_insert(99, 990), Err((99, 990)));
        for i in 0..5 {
            assert_eq!(map5.get(&i), Some(&(i * 10)));
        }

        // Test N = 7
        let mut map7 = StackHashMap::<i32, i32, 7>::new();
        for i in 0..7 {
            assert_eq!(map7.try_insert(i, i * 10), Ok(None));
        }
        assert_eq!(map7.len(), 7);
        assert_eq!(map7.try_insert(99, 990), Err((99, 990)));
        for i in 0..7 {
            assert_eq!(map7.get(&i), Some(&(i * 10)));
        }
    }

    #[test]
    fn test_stack_hash_map_basic() {
        let mut map = StackHashMap::<i32, &'static str, 4>::new();
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
        assert_eq!(map.capacity(), 0);

        assert_eq!(map.try_insert(1, "one"), Ok(None));
        assert_eq!(map.len(), 1);
        assert_eq!(map.capacity(), 4);
        assert_eq!(map.get(&1), Some(&"one"));
        assert_eq!(map.get(&5), None);
        assert!(map.contains_key(&1));
        assert!(!map.contains_key(&5));

        // Overwriting existing key
        assert_eq!(map.try_insert(1, "ONE"), Ok(Some("one")));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&1), Some(&"ONE"));

        assert_eq!(map.try_insert(2, "two"), Ok(None));
        assert_eq!(map.try_insert(3, "three"), Ok(None));
        assert_eq!(map.try_insert(4, "four"), Ok(None));
        assert_eq!(map.len(), 4);

        // Fifth item should fail on a capacity 4 stack map
        assert_eq!(map.try_insert(5, "five"), Err((5, "five")));
        assert_eq!(map.len(), 4);

        // Removing frees up capacity
        assert_eq!(map.remove(&2), Some("two"));
        assert_eq!(map.len(), 3);
        assert_eq!(map.try_insert(5, "five"), Ok(None));
        assert_eq!(map.len(), 4);
        assert_eq!(map.get(&5), Some(&"five"));
    }

    #[test]
    fn test_stack_hash_map_buckets_fully_allocated_on_first_insert() {
        let mut map = StackHashMap::<i32, i32, 64>::new();
        assert_eq!(map.buckets.len(), 0);
        map.insert(1, 100);
        // Resizing buckets to full available capacity on first allocation avoids intermediate rehashes
        assert_eq!(map.buckets.len(), 64);
        assert_eq!(map.buckets.capacity(), 64);
    }

    #[test]
    fn test_hash_map_reserve() {
        // Test reserve on StackHashMap
        let mut stack_map = StackHashMap::<i32, i32, 4>::new();
        assert_eq!(stack_map.capacity(), 0);
        stack_map.reserve(2);
        assert_eq!(stack_map.capacity(), 4);
        assert!(stack_map.try_reserve(4).is_ok()); // Already fits 4
        assert!(stack_map.try_reserve(5).is_err()); // Exceeds capacity 4

        // Test reserve on StdHashMap
        let mut std_map = StdHashMap::<i32, i32>::new();
        assert_eq!(std_map.capacity(), 0);
        std_map.reserve(10);
        assert!(std_map.capacity() >= 10);
        for i in 0..10 {
            std_map.insert(i, i * 10);
        }
        for i in 0..10 {
            assert_eq!(std_map.get(&i), Some(&(i * 10)));
        }
    }

    #[test]
    fn test_hash_map_capacity_minimums() {
        // Map with capacity 4 entries and 4 buckets
        let mut map = StackHashMap::<i32, i32, 4>::new();
        assert_eq!(map.capacity(), 0);
        map.insert(1, 10);
        assert_eq!(map.capacity(), 4);

        // Zero-capacity map always reports 0 capacity and fails reservation
        let mut zero_map = StackHashMap::<i32, i32, 0>::new();
        assert_eq!(zero_map.capacity(), 0);
        assert!(zero_map.try_reserve(1).is_err());
    }

    #[test]
    fn test_stack_hash_map_order_and_iter() {
        let mut map = StackHashMap::<&'static str, i32, 4>::new();
        map.insert("first", 10);
        map.insert("second", 20);
        map.insert("third", 30);

        let keys: Vec<_, ArrayStorage<4>> = map.keys().copied().collect();
        assert_eq!(keys.as_slice(), &["first", "second", "third"]);

        let vals: Vec<_, ArrayStorage<4>> = map.values().copied().collect();
        assert_eq!(vals.as_slice(), &[10, 20, 30]);

        for (k, v) in map.iter_mut() {
            if *k == "second" {
                *v = 200;
            }
        }
        assert_eq!(map.get("second"), Some(&200));

        // Test DoubleEndedIterator
        let rev_keys: Vec<_, ArrayStorage<4>> = map.keys().copied().rev().collect();
        assert_eq!(rev_keys.as_slice(), &["third", "second", "first"]);

        let rev_vals: Vec<_, ArrayStorage<4>> = map.values().copied().rev().collect();
        assert_eq!(rev_vals.as_slice(), &[30, 200, 10]);
    }

    #[test]
    fn test_stack_hash_map_get_index() {
        let mut map = StackHashMap::<i32, &'static str, 4>::new();
        map.insert(10, "A");
        map.insert(20, "B");
        map.insert(30, "C");

        assert_eq!(map.get_index(0), Some((&10, &"A")));
        assert_eq!(map.get_index(1), Some((&20, &"B")));
        assert_eq!(map.get_index(2), Some((&30, &"C")));
        assert_eq!(map.get_index(3), None);

        if let Some((&k, v)) = map.get_index_mut(1) {
            assert_eq!(k, 20);
            *v = "B_MODIFIED";
        }
        assert_eq!(map.get(&20), Some(&"B_MODIFIED"));
    }

    #[test]
    fn test_stack_hash_map_swap_remove() {
        let mut map = StackHashMap::<i32, &'static str, 4>::new();
        map.insert(1, "A");
        map.insert(2, "B");
        map.insert(3, "C");
        map.insert(4, "D");

        assert_eq!(map.swap_remove(&2), Some("B"));
        assert_eq!(map.len(), 3);
        // In swap_remove, the last element D moves into index 1
        let keys: Vec<_, ArrayStorage<4>> = map.keys().copied().collect();
        assert_eq!(keys.as_slice(), &[1, 4, 3]);

        assert_eq!(map.get(&1), Some(&"A"));
        assert_eq!(map.get(&2), None);
        assert_eq!(map.get(&3), Some(&"C"));
        assert_eq!(map.get(&4), Some(&"D"));

        // swap_remove_index
        assert_eq!(map.swap_remove_index(0), Some((1, "A")));
        assert_eq!(map.len(), 2);
        assert_eq!(map.swap_remove_index(5), None);
    }

    #[test]
    fn test_stack_hash_map_shift_remove() {
        let mut map = StackHashMap::<i32, &'static str, 4>::new();
        map.insert(1, "A");
        map.insert(2, "B");
        map.insert(3, "C");
        map.insert(4, "D");

        assert_eq!(map.shift_remove(&2), Some("B"));
        assert_eq!(map.len(), 3);
        // In shift_remove, relative order of remaining elements is preserved
        let keys: Vec<_, ArrayStorage<4>> = map.keys().copied().collect();
        assert_eq!(keys.as_slice(), &[1, 3, 4]);

        assert_eq!(map.get(&1), Some(&"A"));
        assert_eq!(map.get(&2), None);
        assert_eq!(map.get(&3), Some(&"C"));
        assert_eq!(map.get(&4), Some(&"D"));

        // shift_remove_index
        assert_eq!(map.shift_remove_index(1), Some((3, "C")));
        assert_eq!(map.len(), 2);
        let keys_after: Vec<_, ArrayStorage<4>> = map.keys().copied().collect();
        assert_eq!(keys_after.as_slice(), &[1, 4]);
        assert_eq!(map.shift_remove_index(5), None);
    }

    #[test]
    fn test_stack_hash_map_index_traits() {
        let mut map = StackHashMap::<i32, &'static str, 4>::new();
        map.insert(10, "foo");
        assert_eq!(map[&10], "foo");
        map[&10] = "bar";
        assert_eq!(map[&10], "bar");
    }

    #[test]
    fn test_stack_hash_map_into_iterator() {
        let mut map = StackHashMap::<i32, i32, 4>::new();
        map.insert(1, 100);
        map.insert(2, 200);
        map.insert(3, 300);

        let mut out = Vec::<(i32, i32), ArrayStorage<4>>::new();
        for pair in map {
            out.try_push(pair).unwrap();
        }
        assert_eq!(out.as_slice(), &[(1, 100), (2, 200), (3, 300)]);
    }

    #[test]
    fn test_stack_hash_map_into_iterator_double_ended() {
        let mut map = StackHashMap::<i32, i32, 4>::new();
        map.insert(1, 100);
        map.insert(2, 200);
        map.insert(3, 300);

        let mut iter = map.into_iter();
        assert_eq!(iter.next(), Some((1, 100)));
        assert_eq!(iter.next_back(), Some((3, 300)));
        assert_eq!(iter.next(), Some((2, 200)));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }

    #[test]
    fn test_stack_hash_map_retain() {
        let mut map = StackHashMap::<i32, &'static str, 4>::new();
        map.insert(1, "a");
        map.insert(2, "b");
        map.insert(3, "c");
        map.insert(4, "d");

        map.retain(|&k, _| k % 2 == 0);
        assert_eq!(map.len(), 2);
        let keys: Vec<_, ArrayStorage<4>> = map.keys().copied().collect();
        assert_eq!(keys.as_slice(), &[2, 4]);
        assert_eq!(map.get(&1), None);
        assert_eq!(map.get(&2), Some(&"b"));
        assert_eq!(map.get(&3), None);
        assert_eq!(map.get(&4), Some(&"d"));

        // Mutating value during retain
        map.retain(|&k, v| {
            if k == 2 {
                *v = "b_modified";
                true
            } else {
                false
            }
        });
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&2), Some(&"b_modified"));
        assert_eq!(map.get(&4), None);

        // Retain keeping nothing
        map.retain(|_, _| false);
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
        assert_eq!(map.get(&2), None);

        // Can insert again after retaining nothing
        assert_eq!(map.try_insert(10, "ten"), Ok(None));
        assert_eq!(map.get(&10), Some(&"ten"));
    }

    #[test]
    fn test_stack_hash_map_entry_api() {
        let mut map = StackHashMap::<i32, &'static str, 4>::new();

        // Vacant or_insert
        map.entry(1).or_insert("one");
        assert_eq!(map.get(&1), Some(&"one"));

        // Occupied or_insert
        map.entry(1).or_insert("ONE");
        assert_eq!(map.get(&1), Some(&"one")); // Not overwritten

        // and_modify + or_insert
        map.entry(1).and_modify(|v| *v = "uno").or_insert("default");
        assert_eq!(map.get(&1), Some(&"uno"));

        // or_insert_with
        map.entry(2).or_insert_with(|| "two");
        assert_eq!(map.get(&2), Some(&"two"));

        // or_insert_with_key
        map.entry(3).or_insert_with_key(|&k| if k == 3 { "three" } else { "other" });
        assert_eq!(map.get(&3), Some(&"three"));

        // or_default
        let mut int_map = StackHashMap::<i32, i32, 4>::new();
        assert_eq!(*int_map.entry(42).or_default(), 0);
        assert_eq!(int_map.get(&42), Some(&0));

        // try_or_insert
        assert_eq!(map.entry(4).try_or_insert("four"), Ok(&mut "four"));
        assert_eq!(map.len(), 4);

        // Map is now at full capacity (4). Trying to insert a 5th via try_or_insert should fail.
        assert_eq!(map.entry(5).try_or_insert("five"), Err((5, "five")));
        assert_eq!(map.entry(6).try_or_insert_with(|| "six"), Err(6));
        assert_eq!(map.len(), 4);

        // OccupiedEntry operations
        if let Entry::Occupied(mut occupied) = map.entry(1) {
            assert_eq!(occupied.key(), &1);
            assert_eq!(occupied.get(), &"uno");
            assert_eq!(occupied.insert("ONE"), "uno");
            assert_eq!(occupied.get(), &"ONE");
        }
        if let Entry::Occupied(occupied) = map.entry(1) {
            let (k, v) = occupied.remove_entry();
            assert_eq!((k, v), (1, "ONE"));
        }
        assert_eq!(map.len(), 3);
    }

    #[derive(Clone, Default)]
    struct CollidingHasher;

    impl Hasher for CollidingHasher {
        fn finish(&self) -> u64 {
            0
        }
        fn write(&mut self, _bytes: &[u8]) {}
    }

    #[derive(Clone, Copy, Default)]
    struct CollidingBuildHasher;

    impl BuildHasher for CollidingBuildHasher {
        type Hasher = CollidingHasher;
        fn build_hasher(&self) -> Self::Hasher {
            CollidingHasher
        }
    }

    #[test]
    fn test_colliding_hasher_linear_probing_and_backward_shift_deletion() {
        let mut map =
            StackHashMap::<i32, i32, 5, CollidingBuildHasher>::with_hasher(CollidingBuildHasher);

        // Insert 5 colliding elements filling the table 100%
        map.insert(10, 100);
        map.insert(20, 200);
        map.insert(30, 300);
        map.insert(40, 400);
        map.insert(50, 500);
        assert_eq!(map.len(), 5);

        // Verify all 5 are found despite complete collision
        for &(k, v) in &[(10, 100), (20, 200), (30, 300), (40, 400), (50, 500)] {
            assert_eq!(map.get(&k), Some(&v));
        }

        // Shift-remove middle element (30)
        assert_eq!(map.shift_remove(&30), Some(300));
        assert_eq!(map.len(), 4);
        assert_eq!(map.get(&30), None);
        // Verify remaining 4 elements are still accessible
        assert_eq!(map.get(&10), Some(&100));
        assert_eq!(map.get(&20), Some(&200));
        assert_eq!(map.get(&40), Some(&400));
        assert_eq!(map.get(&50), Some(&500));

        // Shift-remove head element (10)
        assert_eq!(map.shift_remove(&10), Some(100));
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(&10), None);
        assert_eq!(map.get(&20), Some(&200));
        assert_eq!(map.get(&40), Some(&400));
        assert_eq!(map.get(&50), Some(&500));

        // Swap-remove last element
        assert_eq!(map.swap_remove(&50), Some(500));
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&50), None);
        assert_eq!(map.get(&20), Some(&200));
        assert_eq!(map.get(&40), Some(&400));

        // Insert new elements back into table
        assert_eq!(map.try_insert(60, 600), Ok(None));
        assert_eq!(map.get(&60), Some(&600));
    }

    #[test]
    fn test_raii_drop_safety() {
        let key_drops = Rc::new(Cell::new(0));
        let val_drops = Rc::new(Cell::new(0));

        #[derive(Debug, PartialEq, Eq, Hash, Clone)]
        struct DropKey(i32, Rc<Cell<usize>>);
        impl Drop for DropKey {
            fn drop(&mut self) {
                self.1.set(self.1.get() + 1);
            }
        }

        #[derive(Debug, PartialEq, Eq, Clone)]
        struct DropVal(i32, Rc<Cell<usize>>);
        impl Drop for DropVal {
            fn drop(&mut self) {
                self.1.set(self.1.get() + 1);
            }
        }

        // Test dropping map drops all keys and values
        {
            let mut map = StackHashMap::<DropKey, DropVal, 4>::new();
            map.insert(DropKey(1, key_drops.clone()), DropVal(10, val_drops.clone()));
            map.insert(DropKey(2, key_drops.clone()), DropVal(20, val_drops.clone()));
            map.insert(DropKey(3, key_drops.clone()), DropVal(30, val_drops.clone()));
            assert_eq!(key_drops.get(), 0);
            assert_eq!(val_drops.get(), 0);
        }
        assert_eq!(key_drops.get(), 3);
        assert_eq!(val_drops.get(), 3);

        // Test IntoIter partial consumption drops unconsumed items exactly once
        key_drops.set(0);
        val_drops.set(0);
        {
            let mut map = StackHashMap::<DropKey, DropVal, 4>::new();
            map.insert(DropKey(1, key_drops.clone()), DropVal(10, val_drops.clone()));
            map.insert(DropKey(2, key_drops.clone()), DropVal(20, val_drops.clone()));
            map.insert(DropKey(3, key_drops.clone()), DropVal(30, val_drops.clone()));
            map.insert(DropKey(4, key_drops.clone()), DropVal(40, val_drops.clone()));

            let mut iter = map.into_iter();
            let first = iter.next().unwrap();
            assert_eq!(first.0.0, 1);
            assert_eq!(first.1.0, 10);
            drop(first);
            assert_eq!(key_drops.get(), 1);
            assert_eq!(val_drops.get(), 1);

            // Drop remaining iterator without consuming items 2, 3, 4
            drop(iter);
        }
        assert_eq!(key_drops.get(), 4);
        assert_eq!(val_drops.get(), 4);

        // Test overwrite drops old value
        val_drops.set(0);
        {
            let mut map = StackHashMap::<i32, DropVal, 4>::new();
            map.insert(1, DropVal(10, val_drops.clone()));
            assert_eq!(val_drops.get(), 0);
            let old = map.insert(1, DropVal(20, val_drops.clone()));
            assert_eq!(val_drops.get(), 0);
            drop(old);
            assert_eq!(val_drops.get(), 1);
        }
        assert_eq!(val_drops.get(), 2);

        // Test retain drops removed items and does not drop kept items
        key_drops.set(0);
        val_drops.set(0);
        {
            let mut map = StackHashMap::<DropKey, DropVal, 4>::new();
            map.insert(DropKey(1, key_drops.clone()), DropVal(10, val_drops.clone()));
            map.insert(DropKey(2, key_drops.clone()), DropVal(20, val_drops.clone()));
            map.insert(DropKey(3, key_drops.clone()), DropVal(30, val_drops.clone()));
            map.insert(DropKey(4, key_drops.clone()), DropVal(40, val_drops.clone()));

            assert_eq!(key_drops.get(), 0);
            assert_eq!(val_drops.get(), 0);

            // Retain only even keys (2, 4), dropping keys 1 and 3
            map.retain(|k, _| k.0 % 2 == 0);
            assert_eq!(key_drops.get(), 2);
            assert_eq!(val_drops.get(), 2);
            assert_eq!(map.len(), 2);
        }
        // Dropping map drops remaining 2 items (2, 4)
        assert_eq!(key_drops.get(), 4);
        assert_eq!(val_drops.get(), 4);
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        #[derive(Debug, Clone)]
        enum MapOp<K, V> {
            Insert(K, V),
            Remove(K),
            SwapRemove(K),
            ShiftRemove(K),
            Get(K),
            GetIndex(usize),
            SwapRemoveIndex(usize),
            ShiftRemoveIndex(usize),
            EntryOrInsert(K, V),
            EntryAndModifyOrInsert(K, V, V),
            Retain(i32),
            Clear,
        }

        proptest! {
            #[test]
            fn test_map_differential(
                ops in prop::collection::vec(
                    prop_oneof![
                        (0..30i32, any::<i32>()).prop_map(|(k, v)| MapOp::Insert(k, v)),
                        (0..30i32).prop_map(MapOp::Remove),
                        (0..30i32).prop_map(MapOp::SwapRemove),
                        (0..30i32).prop_map(MapOp::ShiftRemove),
                        (0..30i32).prop_map(MapOp::Get),
                        (0..40usize).prop_map(MapOp::GetIndex),
                        (0..40usize).prop_map(MapOp::SwapRemoveIndex),
                        (0..40usize).prop_map(MapOp::ShiftRemoveIndex),
                        (0..30i32, any::<i32>()).prop_map(|(k, v)| MapOp::EntryOrInsert(k, v)),
                        (0..30i32, any::<i32>(), any::<i32>()).prop_map(|(k, mod_val, def_val)| {
                            MapOp::EntryAndModifyOrInsert(k, mod_val, def_val)
                        }),
                        (1..10i32).prop_map(MapOp::Retain),
                        Just(MapOp::Clear),
                    ],
                    0..150
                )
            ) {
                let mut custom_map = StdHashMap::<i32, i32>::new();
                let mut std_map = std::collections::HashMap::<i32, i32>::new();

                for op in ops {
                    match op {
                        MapOp::Insert(k, v) => {
                            assert_eq!(custom_map.insert(k, v), std_map.insert(k, v));
                        }
                        MapOp::Remove(k) => {
                            assert_eq!(custom_map.remove(&k), std_map.remove(&k));
                        }
                        MapOp::SwapRemove(k) => {
                            assert_eq!(custom_map.swap_remove(&k), std_map.remove(&k));
                        }
                        MapOp::ShiftRemove(k) => {
                            assert_eq!(custom_map.shift_remove(&k), std_map.remove(&k));
                        }
                        MapOp::Get(k) => {
                            assert_eq!(custom_map.get(&k), std_map.get(&k));
                            assert_eq!(custom_map.contains_key(&k), std_map.contains_key(&k));
                        }
                        MapOp::GetIndex(idx) => {
                            if idx < custom_map.len() {
                                let (k, v) = custom_map.get_index(idx).unwrap();
                                assert_eq!(std_map.get(k), Some(v));
                            } else {
                                assert_eq!(custom_map.get_index(idx), None);
                            }
                        }
                        MapOp::SwapRemoveIndex(idx) => {
                            if idx < custom_map.len() {
                                let (k, v) = custom_map.swap_remove_index(idx).unwrap();
                                assert_eq!(std_map.remove(&k), Some(v));
                            } else {
                                assert_eq!(custom_map.swap_remove_index(idx), None);
                            }
                        }
                        MapOp::ShiftRemoveIndex(idx) => {
                            if idx < custom_map.len() {
                                let (k, v) = custom_map.shift_remove_index(idx).unwrap();
                                assert_eq!(std_map.remove(&k), Some(v));
                            } else {
                                assert_eq!(custom_map.shift_remove_index(idx), None);
                            }
                        }
                        MapOp::EntryOrInsert(k, v) => {
                            let custom_res = *custom_map.entry(k).or_insert(v);
                            let std_res = *std_map.entry(k).or_insert(v);
                            assert_eq!(custom_res, std_res);
                        }
                        MapOp::EntryAndModifyOrInsert(k, mod_val, def_val) => {
                            let custom_res = *custom_map
                                .entry(k)
                                .and_modify(|val| *val = mod_val)
                                .or_insert(def_val);
                            let std_res = *std_map
                                .entry(k)
                                .and_modify(|val| *val = mod_val)
                                .or_insert(def_val);
                            assert_eq!(custom_res, std_res);
                        }
                        MapOp::Retain(mod_val) => {
                            custom_map.retain(|k, _| k % mod_val != 0);
                            std_map.retain(|k, _| k % mod_val != 0);
                        }
                        MapOp::Clear => {
                            custom_map.clear();
                            std_map.clear();
                        }
                    }

                    assert_eq!(custom_map.len(), std_map.len());
                    assert_eq!(custom_map.is_empty(), std_map.is_empty());

                    for (k, v) in std_map.iter() {
                        assert_eq!(custom_map.get(k), Some(v));
                    }
                    for (k, v) in custom_map.iter() {
                        assert_eq!(std_map.get(k), Some(v));
                    }

                    let mut seen = std::collections::HashSet::new();
                    for (k, _) in custom_map.iter() {
                        assert!(seen.insert(*k), "Duplicate key in custom_map: {}", k);
                    }

                    assert_eq!(custom_map.clone(), custom_map);
                }
            }
        }
    }
}
