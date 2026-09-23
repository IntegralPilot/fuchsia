// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::node::FxNode;
use fuchsia_sync::Mutex;
use fxfs::object_handle::INVALID_OBJECT_ID;
use linked_hash_map::LinkedHashMap;
use rustc_hash::FxHasher;
use std::borrow::Borrow;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::Arc;

enum CacheHolder {
    Node(Arc<dyn FxNode>),
    Timer,
}

struct DirentCacheInner {
    lru: LinkedHashMap<DirentCacheKey, CacheHolder, BuildHasherDefault<FxHasher>>,
    limit: usize,
    timer_in_queue: bool,
}

impl DirentCacheInner {
    fn insert_internal(
        &mut self,
        key: DirentCacheKey,
        item: CacheHolder,
    ) -> Option<Arc<dyn FxNode>> {
        self.lru.insert(key, item);
        if self.lru.len() > self.limit {
            if let CacheHolder::Node(node) = self.lru.pop_front().unwrap().1 {
                // Drop outside the lock.
                return Some(node);
            } else {
                self.timer_in_queue = false;
            }
        }
        None
    }
}

/// A cache for directory entry lookup to return the node directly. Uses an LRU to keep things alive
/// and may be periodically cleaned up with calls to `recycle_stale_files` dropping items that
/// haven't been refreshed since the previous call to it.
pub struct DirentCache {
    inner: Mutex<DirentCacheInner>,
}

/// Cache for directory entry object ids.
#[fxfs_trace::trace]
impl DirentCache {
    /// The provided `limit` is the initial max size of the cache.
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Mutex::new(DirentCacheInner {
                lru: create_linked_hash_map(),
                limit,
                timer_in_queue: false,
            }),
        }
    }

    /// Fetch the limit for the cache.
    pub fn limit(&self) -> usize {
        self.inner.lock().limit
    }

    /// Returns the number of elements in the cache.
    pub fn len(&self) -> usize {
        self.inner.lock().lru.len()
    }

    /// Lookup directory entry by (object_id, &filename, is_casefold).
    pub fn lookup(&self, key: &(u64, &str, bool)) -> Option<Arc<dyn FxNode>> {
        assert_ne!(key.0, INVALID_OBJECT_ID, "Looked up dirent key reserved for timer.");
        if let CacheHolder::Node(node) =
            self.inner.lock().lru.get_refresh(key as &dyn DirentCacheKeyRef)?
        {
            return Some(node.clone());
        }
        None
    }

    /// Insert an object id for a directory entry.
    pub(crate) fn insert(&self, key: DirentCacheKey, node: Arc<dyn FxNode>) {
        assert_ne!(key.dir_id, INVALID_OBJECT_ID, "Looked up dirent key reserved for timer.");
        let _dropped = self.inner.lock().insert_internal(key, CacheHolder::Node(node));
    }

    /// Remove an entry from the cache. Key is (object_id, &filename, is_casefold).
    pub fn remove(&self, key: &(u64, &str, bool)) {
        let _dropped_item = self.inner.lock().lru.remove(key as &dyn DirentCacheKeyRef);
    }

    /// Remove all items from the cache.
    pub fn clear(&self) {
        let _dropped = {
            let mut this = self.inner.lock();
            this.timer_in_queue = false;
            std::mem::replace(&mut this.lru, create_linked_hash_map())
        };
    }

    /// Set a new limit for the cache size.
    pub fn set_limit(&self, limit: usize) {
        #[allow(clippy::collection_is_never_read)]
        let mut dropped_items;
        {
            let mut this = self.inner.lock();

            this.limit = limit;
            if this.lru.len() <= limit {
                if this.lru.capacity() > limit {
                    this.lru.shrink_to_fit();
                }
                return;
            }
            dropped_items = Vec::with_capacity(this.lru.len() - limit);
            while this.lru.len() > limit {
                match this.lru.pop_front().unwrap().1 {
                    CacheHolder::Node(node) => dropped_items.push(node),
                    CacheHolder::Timer => this.timer_in_queue = false,
                }
            }
            this.lru.shrink_to_fit();
        }
    }

    /// Drop entries that haven't been refreshed since the last call to this method.
    #[trace]
    pub fn recycle_stale_files(&self) {
        // Drop outside the lock.
        #[allow(clippy::collection_is_never_read)]
        let mut dropped_items = Vec::new();
        {
            let mut this = self.inner.lock();
            if this.timer_in_queue {
                while let CacheHolder::Node(node) = this.lru.pop_front().unwrap().1 {
                    dropped_items.push(node);
                }
                this.timer_in_queue = false;
            }

            if this.lru.len() > 0 {
                this.timer_in_queue = true;
                if let Some(node) =
                    this.insert_internal(DirentCacheKey::default(), CacheHolder::Timer)
                {
                    dropped_items.push(node);
                }
            }
        }
    }
}

fn create_linked_hash_map()
-> LinkedHashMap<DirentCacheKey, CacheHolder, BuildHasherDefault<FxHasher>> {
    LinkedHashMap::with_hasher(BuildHasherDefault::<FxHasher>::default())
}

trait DirentCacheKeyRef {
    fn directory_object_id(&self) -> u64;
    fn name(&self) -> NameRef<'_>;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Name {
    CaseSensitive(String),
    CaseInsensitive(fxfs_unicode::CasefoldString),
}

#[derive(Copy, Clone, Debug)]
enum NameRef<'a> {
    CaseSensitive(&'a str),
    CaseInsensitive(&'a str),
}

impl Hash for NameRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            NameRef::CaseSensitive(s) => {
                0.hash(state);
                s.hash(state);
            }
            NameRef::CaseInsensitive(s) => {
                1.hash(state);
                fxfs_unicode::CasefoldStr::new(s).hash(state);
            }
        }
    }
}

impl PartialEq for NameRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (NameRef::CaseSensitive(a), NameRef::CaseSensitive(b)) => a == b,
            (NameRef::CaseInsensitive(a), NameRef::CaseInsensitive(b)) => {
                fxfs_unicode::casefold_cmp(a, b).is_eq()
            }
            _ => false,
        }
    }
}

impl Eq for NameRef<'_> {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DirentCacheKey {
    pub(crate) dir_id: u64,
    pub(crate) name: Name,
}

impl DirentCacheKey {
    pub(crate) fn new(dir_id: u64, name: String, is_casefold: bool) -> Self {
        let name = if is_casefold {
            Name::CaseInsensitive(fxfs_unicode::CasefoldString::new(name))
        } else {
            Name::CaseSensitive(name)
        };
        Self { dir_id, name }
    }
}

impl Default for DirentCacheKey {
    fn default() -> Self {
        Self { dir_id: INVALID_OBJECT_ID, name: Name::CaseSensitive(String::new()) }
    }
}

impl DirentCacheKeyRef for DirentCacheKey {
    fn directory_object_id(&self) -> u64 {
        self.dir_id
    }
    fn name(&self) -> NameRef<'_> {
        match &self.name {
            Name::CaseSensitive(s) => NameRef::CaseSensitive(s),
            Name::CaseInsensitive(s) => NameRef::CaseInsensitive(s.as_str()),
        }
    }
}

impl<'a> Borrow<dyn DirentCacheKeyRef + 'a> for DirentCacheKey {
    fn borrow(&self) -> &(dyn DirentCacheKeyRef + 'a) {
        self
    }
}

impl Hash for dyn DirentCacheKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.directory_object_id().hash(state);
        self.name().hash(state);
    }
}

impl PartialEq for dyn DirentCacheKeyRef + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.directory_object_id() == other.directory_object_id() && self.name() == other.name()
    }
}

impl Eq for dyn DirentCacheKeyRef + '_ {}

impl DirentCacheKeyRef for (u64, &str, bool) {
    fn directory_object_id(&self) -> u64 {
        self.0
    }
    fn name(&self) -> NameRef<'_> {
        if self.2 { NameRef::CaseInsensitive(self.1) } else { NameRef::CaseSensitive(self.1) }
    }
}

#[cfg(test)]
mod tests {
    use super::DirentCacheKey;
    use crate::fuchsia::directory::FxDirectory;
    use crate::fuchsia::dirent_cache::DirentCache;
    use crate::fuchsia::node::FxNode;
    use fxfs::object_store::ObjectDescriptor;
    use fxfs_macros::ToWeakNode;
    use std::sync::Arc;

    #[derive(ToWeakNode)]
    struct FakeNode(u64);
    impl FxNode for FakeNode {
        fn object_id(&self) -> u64 {
            self.0
        }
        fn parent(&self) -> Option<Arc<FxDirectory>> {
            unreachable!();
        }
        fn set_parent(&self, _parent: Arc<FxDirectory>) {
            unreachable!();
        }
        fn open_count_add_one(&self) {}
        fn open_count_sub_one(self: Arc<Self>) {}

        fn object_descriptor(&self) -> ObjectDescriptor {
            ObjectDescriptor::File
        }
    }

    #[fuchsia::test]
    fn test_simple_lru() {
        let cache = DirentCache::new(5);
        for i in 1..6 {
            cache.insert(DirentCacheKey::new(1, i.to_string(), false), Arc::new(FakeNode(i)));
        }

        // Refresh entry 2. Puts it at the top of the used list.
        assert!(cache.lookup(&(1, "2", false)).is_some());

        // Add 2 more items. This will expire 1 and 3 since 2 was refreshed.
        for i in 6..8 {
            cache.insert(DirentCacheKey::new(1, i.to_string(), false), Arc::new(FakeNode(i)));
        }

        // 2 is still there, but 1 and 3 aren't.
        assert!(cache.lookup(&(1, "1", false)).is_none());
        assert!(cache.lookup(&(1, "2", false)).is_some());
        assert!(cache.lookup(&(1, "3", false)).is_none());

        // Remove 2 and now it's gone.
        cache.remove(&(1, "2", false));
        assert!(cache.lookup(&(1, "2", false)).is_none());

        // All remaining items are still there.
        for i in 4..8 {
            assert!(cache.lookup(&(1, &i.to_string(), false)).is_some(), "Missing item {}", i);
        }

        // Add one more, as there's space from the removal and everything is still there.
        cache.insert(DirentCacheKey::new(1, "8".to_string(), false), Arc::new(FakeNode(8)));
        for i in 4..9 {
            assert!(cache.lookup(&(1, &i.to_string(), false)).is_some(), "Missing item {}", i);
        }
    }

    #[fuchsia::test]
    fn test_change_limit() {
        let cache = DirentCache::new(10);

        for i in 1..16 {
            cache.insert(DirentCacheKey::new(1, i.to_string(), false), Arc::new(FakeNode(i)));
        }

        // Only the last ten should be there.
        for i in 1..6 {
            assert!(
                cache.lookup(&(1, &i.to_string(), false)).is_none(),
                "Shouldn't have item {}",
                i
            );
        }
        for i in 6..16 {
            assert!(cache.lookup(&(1, &i.to_string(), false)).is_some(), "Missing item {}", i);
        }

        // Lower the limit and see that only the last five are left.
        cache.set_limit(5);
        for i in 1..11 {
            assert!(
                cache.lookup(&(1, &i.to_string(), false)).is_none(),
                "Shouldn't have item {}",
                i
            );
        }
        for i in 11..16 {
            assert!(cache.lookup(&(1, &i.to_string(), false)).is_some(), "Missing item {}", i);
        }
    }

    #[fuchsia::test]
    fn test_cache_clear() {
        let cache = DirentCache::new(10);

        for i in 1..6 {
            cache.insert(DirentCacheKey::new(1, i.to_string(), false), Arc::new(FakeNode(i)));
        }

        // All entries should be present.
        for i in 1..6 {
            assert!(cache.lookup(&(1, &i.to_string(), false)).is_some(), "Missing item {}", i);
        }

        // Clear, then none should be present.
        cache.clear();
        for i in 1..6 {
            assert!(
                cache.lookup(&(1, &i.to_string(), false)).is_none(),
                "Shouldn't have item {}",
                i
            );
        }
    }

    #[fuchsia::test]
    fn test_timeout() {
        let cache = DirentCache::new(20);

        cache.recycle_stale_files();

        // Put in 10 items.
        for i in 1..11 {
            cache.insert(DirentCacheKey::new(1, i.to_string(), false), Arc::new(FakeNode(i)));
        }

        cache.recycle_stale_files();

        // Refresh only the odd numbered entries.
        for i in (1..11).step_by(2) {
            assert!(cache.lookup(&(1, &i.to_string(), false)).is_some(), "Missing item {}", i);
        }

        cache.recycle_stale_files();

        // Only the refreshed dd numbered nodes should be left.
        for i in 1..11 {
            match cache.lookup(&(1, &i.to_string(), false)) {
                Some(_) => assert_eq!(i % 2, 1, "Even number {} found.", i),
                None => assert_eq!(i % 2, 0, "Odd number {} missing.", i),
            }
        }
    }

    #[fuchsia::test]
    fn test_set_limit_changes_capacity_when_above_limit() {
        const CACHE_SIZE: usize = 1024;
        let cache = DirentCache::new(CACHE_SIZE);
        for i in 0..CACHE_SIZE {
            cache
                .insert(DirentCacheKey::new(1, i.to_string(), false), Arc::new(FakeNode(i as u64)));
        }
        assert!(cache.inner.lock().lru.capacity() >= CACHE_SIZE);
        cache.set_limit(8);
        assert!(cache.inner.lock().lru.capacity() < CACHE_SIZE);
    }

    #[fuchsia::test]
    fn test_set_limit_changes_capacity_when_below_limit() {
        const CACHE_SIZE: usize = 1024;
        let cache = DirentCache::new(CACHE_SIZE);
        for i in 0..CACHE_SIZE {
            cache
                .insert(DirentCacheKey::new(1, i.to_string(), false), Arc::new(FakeNode(i as u64)));
        }
        assert!(cache.inner.lock().lru.capacity() >= CACHE_SIZE);
        cache.recycle_stale_files();
        cache.recycle_stale_files(); // Remove everything from the cache.
        assert_eq!(cache.inner.lock().lru.len(), 0);
        cache.set_limit(8);
        assert!(cache.inner.lock().lru.capacity() < CACHE_SIZE);
    }

    #[fuchsia::test]
    fn test_casefold_lookup() {
        let cache = DirentCache::new(5);
        cache.insert(DirentCacheKey::new(1, "Foo".to_string(), true), Arc::new(FakeNode(1)));

        // Lookup with exact match should succeed.
        assert!(cache.lookup(&(1, "Foo", true)).is_some());
        // Lookup with different casing should succeed.
        assert!(cache.lookup(&(1, "foo", true)).is_some());
        assert!(cache.lookup(&(1, "FOO", true)).is_some());

        // Lookup with case_fold = false should fail.
        assert!(cache.lookup(&(1, "Foo", false)).is_none());
        assert!(cache.lookup(&(1, "foo", false)).is_none());
    }

    #[fuchsia::test]
    fn test_casefold_remove() {
        let cache = DirentCache::new(5);
        cache.insert(DirentCacheKey::new(1, "Foo".to_string(), true), Arc::new(FakeNode(1)));

        // Remove with different casing should succeed.
        cache.remove(&(1, "foo", true));
        assert!(cache.lookup(&(1, "Foo", true)).is_none());
    }

    #[fuchsia::test]
    fn test_casefold_overwrite() {
        let cache = DirentCache::new(5);
        cache.insert(DirentCacheKey::new(1, "Foo".to_string(), true), Arc::new(FakeNode(1)));
        // Inserting "foo" should overwrite "Foo" since they are case-equivalents.
        cache.insert(DirentCacheKey::new(1, "foo".to_string(), true), Arc::new(FakeNode(2)));

        assert_eq!(cache.len(), 1);
        let node = cache.lookup(&(1, "Foo", true)).expect("Missing item");
        assert_eq!(node.object_id(), 2);
    }
}
