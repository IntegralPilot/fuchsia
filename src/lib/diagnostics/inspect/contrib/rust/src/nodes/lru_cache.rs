// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::Node;
use fuchsia_inspect_derive::Unit;
use lru_cache::LruCache;
use std::hash::Hash;

use crate::nodes::NodeTimeExt;

/// A Inspect node that holds an ordered, bounded set of data. When a new unique
/// item needs to be inserted, the least-recently-used item is evicted.
pub struct LruCacheNode<T: Unit + Eq + Hash> {
    node: Node,
    items: LruCache<T, CacheItem<T>>,
}

impl<T: Unit + Eq + Hash> LruCacheNode<T> {
    pub fn new(node: Node, capacity: usize) -> Self {
        Self { node, items: LruCache::new(std::cmp::max(capacity, 1)) }
    }

    /// Insert |item| into `LruCacheNode`.
    ///
    /// If |item| already exists in the cache, its entry is retrieved and the entry's index
    /// is returned.
    /// If |item| has not already existed in the cache, the item is recorded with the current
    /// timestamp and its index in the cache is returned. If the cache is already full,
    /// recording the new item would evict the least-recently-used item.
    pub fn insert(&mut self, item: T) -> usize {
        match self.items.get_mut(&item) {
            Some(entry) => entry.index,
            None => {
                let index = if self.items.len() < self.items.capacity() {
                    self.items.len()
                } else {
                    self.items.remove_lru().map(|entry| entry.1.index).unwrap_or(0)
                };
                let child = self.node.create_child(index.to_string());
                NodeTimeExt::<zx::BootTimeline>::record_time(&child, "@time");
                let data = item.inspect_create(&child, "data");
                self.items.insert(item, CacheItem { index, _node: child, _data: data });
                index
            }
        }
    }
}
struct CacheItem<T: Unit> {
    index: usize,
    _node: Node,
    _data: <T as Unit>::Data,
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyNumericProperty, assert_data_tree};
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn test_insert() {
        let inspector = Inspector::default();
        let cache_node = inspector.root().create_child("cache");
        let mut cache_node = LruCacheNode::new(cache_node, 3);
        // Insert unique items
        assert_eq!(cache_node.insert(111), 0);
        assert_eq!(cache_node.insert(222), 1);
        assert_eq!(cache_node.insert(333), 2);
        assert_data_tree!(inspector, root: {
            cache: {
                "0": { "@time": AnyNumericProperty, "data": 111i64},
                "1": { "@time": AnyNumericProperty, "data": 222i64},
                "2": { "@time": AnyNumericProperty, "data": 333i64},
            }
        });

        // Insert item that already exists does not change the Inspect data
        assert_eq!(cache_node.insert(222), 1);
        assert_eq!(cache_node.insert(111), 0);
        assert_eq!(cache_node.insert(333), 2);
        assert_data_tree!(inspector, root: {
            cache: {
                "0": { "@time": AnyNumericProperty, "data": 111i64},
                "1": { "@time": AnyNumericProperty, "data": 222i64},
                "2": { "@time": AnyNumericProperty, "data": 333i64},
            }
        });

        // Now that the node is full, inserting new item would replace the least recently used
        assert_eq!(cache_node.insert(444), 1);
        assert_data_tree!(inspector, root: {
            cache: {
                "0": { "@time": AnyNumericProperty, "data": 111i64},
                "1": { "@time": AnyNumericProperty, "data": 444i64},
                "2": { "@time": AnyNumericProperty, "data": 333i64},
            }
        });

        // Value that had been evicted is considered to be a new value if they are inserted again
        assert_eq!(cache_node.insert(222), 0);
        assert_data_tree!(inspector, root: {
            cache: {
                "0": { "@time": AnyNumericProperty, "data": 222i64},
                "1": { "@time": AnyNumericProperty, "data": 444i64},
                "2": { "@time": AnyNumericProperty, "data": 333i64},
            }
        });
    }

    #[derive(PartialEq, Eq, Hash, Unit)]
    struct Item {
        num: u64,
        string: String,
    }

    #[fuchsia::test]
    async fn test_insert_custom_struct() {
        let inspector = Inspector::default();
        let cache_node = inspector.root().create_child("cache");
        let mut cache_node = LruCacheNode::new(cache_node, 3);
        assert_eq!(cache_node.insert(Item { num: 1337u64, string: "42".to_string() }), 0);
        assert_eq!(cache_node.insert(Item { num: 1337u64, string: "43".to_string() }), 1);
        assert_data_tree!(inspector, root: {
            cache: {
                "0": {
                    "@time": AnyNumericProperty,
                    "data": { num: 1337u64, string: "42".to_string() }
                },
                "1": {
                    "@time": AnyNumericProperty,
                    "data": { num: 1337u64, string: "43".to_string() }
                },
            }
        });
    }
}
