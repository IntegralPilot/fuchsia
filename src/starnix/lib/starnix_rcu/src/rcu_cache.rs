// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_rcu::{RcuDroppable, RcuReadScope};
use fuchsia_rcu_collections::rcu_raw_hash_map::{InsertionResult, RcuRawHashMap};
use starnix_sync::{Mutex, MutexGuard};
use std::hash::Hash;

pub enum RcuCacheInsertionResult<V> {
    /// The entry was inserted.
    Inserted,

    /// The entry was updated.
    ///
    /// The old value is returned.
    Updated(V),

    /// The entry was inserted and caused another entry to be evicted.
    ///
    /// The evicted value is returned.
    Evicted(V),
}

/// A cache that uses RCU to provide thread-safe access to a hash map.
///
/// This is similar to `RcuHashMap`, but it also evicts items when the cache
/// exceeds a specified capacity.
///
/// Entries are evicted in a FIFO manner.
///
/// By default, this map uses `rapidhash::RapidBuildHasher`, which provides high performance.
/// However, if this map holds keys which may be attacker-controlled, consider using
/// `std::collections::hash_map::RandomState` instead.
#[derive(Debug, RcuDroppable)]
pub struct RcuCache<K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: std::hash::BuildHasher + Send + Sync + 'static,
{
    /// The maximum number of entries in the cache.
    capacity: usize,

    /// The underlying hash map.
    map: RcuRawHashMap<K, V, S>,

    /// A mutex to provide synchronization for writing to the map.
    mutex: Mutex<()>,
}

impl<K, V> RcuCache<K, V, rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
{
    /// Creates a new `RcuCache` with the specified capacity.
    pub fn new(capacity: usize) -> Self {
        Self { capacity, map: RcuRawHashMap::with_capacity(capacity + 1), mutex: Mutex::new(()) }
    }
}

impl<K, V, S> RcuCache<K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: std::hash::BuildHasher + Send + Sync + 'static,
{
    /// Creates a new `RcuCache` with the specified capacity and hasher.
    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        Self {
            capacity,
            map: RcuRawHashMap::with_capacity_and_hasher(capacity + 1, hash_builder),
            mutex: Mutex::new(()),
        }
    }
}

impl<K, V, S> RcuCache<K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: std::hash::BuildHasher + Send + Sync + 'static,
{
    /// Returns the capacity with which this instance was initialized.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the number of entries in the cache.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns a reference to the value associated with the key.
    pub fn get<'a>(&self, scope: &'a RcuReadScope, key: &K) -> Option<&'a V> {
        self.map.get(scope, key)
    }

    pub fn lock(&self) -> RcuCacheGuard<'_, K, V, S> {
        let guard = self.mutex.lock();
        RcuCacheGuard { cache: self, _guard: guard }
    }

    /// Removes all entries from the cache.
    pub fn clear(&self) {
        let _guard = self.mutex.lock();
        let scope = RcuReadScope::new();
        let mut cursor = self.map.cursor(&scope);
        loop {
            // SAFETY: We have exclusive access to the map because we have exclusive access to the
            // mutex.
            let removed = unsafe { cursor.remove() };
            if removed.is_none() {
                break;
            }
        }
    }
}

pub struct RcuCacheGuard<'a, K, V, S = rapidhash::RapidBuildHasher>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: std::hash::BuildHasher + Send + Sync + 'static,
{
    cache: &'a RcuCache<K, V, S>,
    _guard: MutexGuard<'a, ()>,
}

impl<'a, K, V, S> RcuCacheGuard<'a, K, V, S>
where
    K: Eq + Hash + Clone + RcuDroppable + Sync,
    V: Clone + RcuDroppable + Sync,
    S: std::hash::BuildHasher + Send + Sync + 'static,
{
    pub fn get<'rcu>(&self, scope: &'rcu RcuReadScope, key: &K) -> Option<&'rcu V> {
        self.cache.map.get(scope, key)
    }

    /// Inserts a key-value pair into the cache.
    ///
    /// If the cache exceeds its capacity, entries are evicted in a FIFO manner.
    pub fn insert(&self, scope: &RcuReadScope, key: K, value: V) -> RcuCacheInsertionResult<V> {
        // SAFETY: We have exclusive access to the map because we have exclusive access to the mutex.
        match unsafe { self.cache.map.insert(scope, key, value) } {
            InsertionResult::Inserted(count) => {
                if count > self.cache.capacity {
                    // The mutex should prevent any other modifications to the map while the insert
                    // operation is in progress.
                    assert!(count == self.cache.capacity + 1);
                    let mut cursor = self.cache.map.cursor(&scope);
                    // SAFETY: We have exclusive access to the map because we have exclusive access
                    // to the mutex.
                    if let Some(old_value) = unsafe { cursor.remove() } {
                        RcuCacheInsertionResult::Evicted(old_value)
                    } else {
                        unreachable!("cache is full but no entries to evict")
                    }
                } else {
                    RcuCacheInsertionResult::Inserted
                }
            }
            InsertionResult::Updated(old_value) => RcuCacheInsertionResult::Updated(old_value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_rcu::rcu_run_callbacks;

    #[test]
    fn test_rcu_cache_fifo_eviction() {
        let capacity = 3;
        let cache = RcuCache::new(capacity);
        let guard = cache.lock();
        let scope = RcuReadScope::new();

        // Insert items up to capacity
        guard.insert(&scope, 1, 10);
        guard.insert(&scope, 2, 20);
        guard.insert(&scope, 3, 30);

        assert_eq!(guard.get(&scope, &1), Some(&10));
        assert_eq!(guard.get(&scope, &2), Some(&20));
        assert_eq!(guard.get(&scope, &3), Some(&30));

        // Insert an item beyond capacity, should evict 1
        guard.insert(&scope, 4, 40);

        assert_eq!(cache.get(&scope, &1), None);
        assert_eq!(cache.get(&scope, &2), Some(&20));
        assert_eq!(cache.get(&scope, &3), Some(&30));
        assert_eq!(cache.get(&scope, &4), Some(&40));

        // Insert another item, should evict 2
        guard.insert(&scope, 5, 50);

        assert_eq!(cache.get(&scope, &1), None);
        assert_eq!(cache.get(&scope, &2), None);
        assert_eq!(cache.get(&scope, &3), Some(&30));
        assert_eq!(cache.get(&scope, &4), Some(&40));
        assert_eq!(cache.get(&scope, &5), Some(&50));

        // Update an existing item, should not evict and not change order for eviction
        guard.insert(&scope, 3, 300);

        assert_eq!(cache.get(&scope, &1), None);
        assert_eq!(cache.get(&scope, &2), None);
        assert_eq!(cache.get(&scope, &3), Some(&300));
        assert_eq!(cache.get(&scope, &4), Some(&40));
        assert_eq!(cache.get(&scope, &5), Some(&50));

        // Insert another item, should evict 4 (because 3 was updated, not re-inserted)
        guard.insert(&scope, 6, 60);

        assert_eq!(cache.get(&scope, &1), None);
        assert_eq!(cache.get(&scope, &2), None);
        assert_eq!(cache.get(&scope, &3), Some(&300));
        assert_eq!(cache.get(&scope, &4), None);
        assert_eq!(cache.get(&scope, &5), Some(&50));
        assert_eq!(cache.get(&scope, &6), Some(&60));

        std::mem::drop(guard);
        std::mem::drop(scope);
        rcu_run_callbacks();
    }
}
