// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::object_record::{ObjectKey, ObjectKeyData, ObjectValue};
use crate::lsm_tree::cache::{ObjectCache, ObjectCachePlaceholder, ObjectCacheResult};
use fuchsia_sync::{Mutex, MutexGuard};
use linked_hash_map::{Entry, LinkedHashMap};
use std::hash::BuildHasherDefault;
use std::sync::atomic::{AtomicU64, Ordering};

fn filter(key: &ObjectKey) -> bool {
    match key.data {
        // Attribute and keys could also be added here to some immediate benefit, but would be
        // somewhat redundant with a node cache planned to be added after.
        ObjectKeyData::Object => true,
        ObjectKeyData::ExtendedAttribute { .. } => true,
        ObjectKeyData::Keys => true,
        _ => false,
    }
}

// Limiting to ~100KiB of space usage. 56 bytes of linear overhead per item plus the overhead of
// the structure. This is just used directly for now, we can parameterize it in the type if this is
// ever desired to vary.
const ITEM_LIMIT: usize = 1535;

struct Placeholder<'a> {
    cache: &'a TreeCache,
    key: Option<ObjectKey>,
    placeholder_id: u64,
}

impl Placeholder<'_> {
    fn replace_entry(&mut self, value: Option<CacheValue>) {
        let key = self.key.take().expect("This method should only be called once");
        let mut inner = self.cache.inner.lock();
        // The value is present...
        if let Entry::Occupied(mut entry) = inner.entry(key) {
            // And the same placeholder as the token has...
            let is_current = match entry.get() {
                CacheValue::Placeholder(placeholder_id) => &self.placeholder_id == placeholder_id,
                _ => false,
            };
            if is_current {
                match value {
                    Some(v) => *(entry.get_mut()) = v,
                    None => {
                        entry.remove();
                    }
                }
            }
        }
    }
}

impl Drop for Placeholder<'_> {
    fn drop(&mut self) {
        if self.key.is_some() {
            self.replace_entry(None);
        }
    }
}

impl<'a> ObjectCachePlaceholder<ObjectValue> for Placeholder<'a> {
    fn complete(mut self: Box<Self>, value: Option<&ObjectValue>) {
        let entry_value = match value {
            value @ Some(ObjectValue::Object { .. }) => value.cloned(),
            value @ Some(ObjectValue::ExtendedAttribute(_)) => value.cloned(),
            value @ Some(ObjectValue::Keys(_)) => value.cloned(),
            _ => None,
        }
        .map(|v| CacheValue::Value(v));
        self.replace_entry(entry_value);
    }
}

enum CacheValue {
    Placeholder(u64),
    Value(ObjectValue),
}

/// Supports caching for Objects directly for now. Speeds up stat calls.
pub struct TreeCache {
    inner: Mutex<LinkedHashMap<ObjectKey, CacheValue, BuildHasherDefault<rustc_hash::FxHasher>>>,
    placeholder_counter: AtomicU64,
}

impl TreeCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LinkedHashMap::with_capacity_and_hasher(
                ITEM_LIMIT + 1,
                BuildHasherDefault::<rustc_hash::FxHasher>::default(),
            )),
            placeholder_counter: AtomicU64::new(0),
        }
    }
}

impl ObjectCache<ObjectKey, ObjectValue> for TreeCache {
    fn lookup_or_reserve(&self, key: &ObjectKey) -> ObjectCacheResult<'_, ObjectValue> {
        if !filter(key) {
            return ObjectCacheResult::NoCache;
        }
        let inner = self.inner.lock();
        match MutexGuard::try_map_or_err(inner, |inner| match inner.get_refresh(key) {
            Some(CacheValue::Value(v)) => Ok(v),
            Some(CacheValue::Placeholder(_)) => Err(false),
            None => Err(true),
        }) {
            Ok(guard) => ObjectCacheResult::Value(guard),
            Err((_, false)) => ObjectCacheResult::NoCache,
            Err((mut inner, true)) => {
                let placeholder_id = self.placeholder_counter.fetch_add(1, Ordering::Relaxed);
                inner.insert(key.clone(), CacheValue::Placeholder(placeholder_id));
                if inner.len() > ITEM_LIMIT {
                    let _ = inner.pop_front();
                }
                ObjectCacheResult::Placeholder(Box::new(Placeholder {
                    cache: self,
                    key: Some(key.clone()),
                    placeholder_id,
                }))
            }
        }
    }

    fn is_cacheable(&self, key: &ObjectKey) -> bool {
        filter(key)
    }

    fn invalidate(&self, key: &ObjectKey, value: Option<ObjectValue>) {
        if !filter(key) {
            return;
        }
        let mut inner = self.inner.lock();
        if let Some(replacement) = value {
            if let Some(entry) = inner.get_mut(key) {
                *entry = CacheValue::Value(replacement);
            }
        } else {
            inner.remove(key);
        }
    }

    fn clear(&self) {
        self.inner.lock().clear();
    }

    fn len(&self) -> usize {
        self.inner.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::super::object_record::{AttributeId, ObjectKey, ObjectValue, Timestamp};
    use super::{ITEM_LIMIT, TreeCache};
    use crate::lsm_tree::cache::{ObjectCache, ObjectCacheResult};
    use crate::object_store::EncryptionKey;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    async fn test_basic_operations() {
        let cache = TreeCache::new();
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value = ObjectValue::file(1, 0, now, now, now, now, None, None);

        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&value));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find item."),
        };

        cache.invalidate(&key, None);

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder.complete(None),
            _ => panic!("Expected cache miss with placeholder returned."),
        };
    }

    #[fuchsia::test]
    async fn test_enforce_limits() {
        let cache = TreeCache::new();
        let now = Timestamp::now();

        for i in 1..(ITEM_LIMIT as u64 + 2) {
            let key = ObjectKey::object(i);
            let value = ObjectValue::file(1, 0, now, now, now, now, None, None);
            let placeholder = match cache.lookup_or_reserve(&key) {
                ObjectCacheResult::Placeholder(placeholder) => placeholder,
                _ => panic!("Expected cache miss with placeholder returned."),
            };
            placeholder.complete(Some(&value));
        }

        // Item 1 should be evicted.
        assert_matches!(
            cache.lookup_or_reserve(&ObjectKey::object(1)),
            ObjectCacheResult::Placeholder(_)
        );

        // And item 2 has been evicted by the lookup of item 1.
        for i in 3..(ITEM_LIMIT as u64 + 2) {
            let key = ObjectKey::object(i);
            assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Value(_));
        }
    }

    #[fuchsia::test]
    async fn test_invalidate_inserts() {
        let cache = TreeCache::new();
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value1 = ObjectValue::file(1, 0, now, now, now, now, None, None);
        let value2 = ObjectValue::file(2, 0, now, now, now, now, None, None);

        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&value1));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value1),
            _ => panic!("Expected to find item."),
        };

        cache.invalidate(&key, Some(value2.clone()));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value2),
            _ => panic!("Expected to find item."),
        };
    }

    #[fuchsia::test]
    async fn test_no_caching_for_filtered_item() {
        let cache = TreeCache::new();
        let key = ObjectKey::extent(1, AttributeId::TEST_ID, 1..2);

        assert!(!cache.is_cacheable(&key));
        assert!(matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::NoCache));
    }

    #[fuchsia::test]
    async fn test_is_cacheable() {
        let cache = TreeCache::new();

        // Cacheable keys: Object, ExtendedAttribute, Keys
        assert!(cache.is_cacheable(&ObjectKey::object(1)));
        assert!(cache.is_cacheable(&ObjectKey::keys(1)));
        assert!(cache.is_cacheable(&ObjectKey::extended_attribute(1, vec![1, 2, 3])));

        // Non-cacheable keys: Extent, Graveyard, etc.
        assert!(!cache.is_cacheable(&ObjectKey::extent(1, AttributeId::TEST_ID, 1..2)));
        assert!(!cache.is_cacheable(&ObjectKey::graveyard_entry(1, 2)));
        assert!(!cache.is_cacheable(&ObjectKey::graveyard_attribute_entry(
            1,
            2,
            AttributeId::TEST_ID
        )));
    }

    // Two clients looking for the same key don't interfere with each other. Prevents priority
    // inversion.
    #[fuchsia::test]
    async fn test_two_parallel_clients() {
        let cache = TreeCache::new();
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value1 = ObjectValue::file(1, 0, now, now, now, now, None, None);
        let value2 = ObjectValue::file(2, 0, now, now, now, now, None, None);

        let placeholder1 = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };

        // Another search should not get a placeholder, as one is already held.
        assert!(matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::NoCache));

        // Invalidate the current placeholder.
        cache.invalidate(&key, None);

        // Get a new placeholder
        let placeholder2 = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };

        // Complete them out of order.
        placeholder2.complete(Some(&value2));
        placeholder1.complete(Some(&value1));

        // Result should be from the second placeholder, as the first was invalidated.
        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value2),
            _ => panic!("Expected to find item."),
        };
    }

    #[fuchsia::test]
    async fn test_keys_are_cached() {
        let cache = TreeCache::new();
        let key = ObjectKey::keys(1);
        let value = ObjectValue::keys(
            vec![(2, EncryptionKey::FscryptInoLblk32File { key_identifier: [3u8; 16] })].into(),
        );

        let placeholder = match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder,
            _ => panic!("Expected cache miss with placeholder returned."),
        };
        placeholder.complete(Some(&value));

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Value(result) => assert_eq!(&*result, &value),
            _ => panic!("Expected to find item."),
        };

        cache.invalidate(&key, None);

        match cache.lookup_or_reserve(&key) {
            ObjectCacheResult::Placeholder(placeholder) => placeholder.complete(None),
            _ => panic!("Expected cache miss with placeholder returned."),
        };
    }

    #[fuchsia::test]
    async fn test_dropped_placeholder_clears_entry() {
        let cache = TreeCache::new();
        let key = ObjectKey::object(1);

        {
            let _placeholder = match cache.lookup_or_reserve(&key) {
                ObjectCacheResult::Placeholder(placeholder) => placeholder,
                _ => panic!("Expected cache miss with placeholder returned."),
            };
        }

        assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Placeholder(_));
    }

    #[fuchsia::test]
    async fn test_clear() {
        let cache = TreeCache::new();
        let key = ObjectKey::object(1);
        let now = Timestamp::now();
        let value = ObjectValue::file(1, 0, now, now, now, now, None, None);

        if let ObjectCacheResult::Placeholder(placeholder) = cache.lookup_or_reserve(&key) {
            placeholder.complete(Some(&value));
        } else {
            panic!("Expected cache miss with placeholder returned.");
        }

        assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Value(_));
        cache.clear();
        assert_matches!(cache.lookup_or_reserve(&key), ObjectCacheResult::Placeholder(_));
    }
}
