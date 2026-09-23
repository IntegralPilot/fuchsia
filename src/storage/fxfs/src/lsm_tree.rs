// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # End-Key Indexing for File Extents
//!
//! Fxfs indexes file extents by their **end offset** because it avoids expensive reverse
//! iteration (`Prev()`) during lookups in the LSM tree.
//!
//! If we indexed by start offset, a `Seek(>= X)` would return the extent starting *after* X,
//! requiring a `Prev()` to find the extent actually containing X.
//!
//! By indexing by end offset, a `Seek(>= X + 1)` returns the first extent ending *after* X.
//! Since extents do not overlap, this is the only extent that could possibly contain X,
//! eliminating the need for backtracking.

mod bloom_filter;
pub mod cache;
pub mod merge;
pub mod persistent_layer;
pub mod skip_list_layer;
pub mod types;

#[cfg(any(test, fuzz))]
pub mod testing;

use crate::drop_event::DropEvent;
use crate::log::*;
use crate::metrics::DurationMeasureScope;
use crate::object_handle::{ReadObjectHandle, WriteBytes};
use crate::serialized_types::{LATEST_VERSION, Version};

use anyhow::Error;
use cache::{ObjectCache, ObjectCacheResult};

use fuchsia_inspect::HistogramProperty;
use fuchsia_sync::RwLock;
use persistent_layer::{PersistentLayer, PersistentLayerWriter};
use skip_list_layer::SkipListLayer;
use std::fmt;
use std::ops::Bound;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use storage_units::BlockSize;
use types::{
    Existence, Item, ItemRef, Key, Layer, LayerIterator, LayerKey, LayerWriter, MaybeContainsKey,
    MergeType, MergeableKey, OrdLowerBound, Value,
};

pub use merge::Query;

const SKIP_LIST_LAYER_ITEMS: usize = 512;

// For serialization.
pub use persistent_layer::{
    LayerHeader as PersistentLayerHeader, LayerHeaderV39 as PersistentLayerHeaderV39,
    LayerInfo as PersistentLayerInfo, LayerInfoV39 as PersistentLayerInfoV39,
};

pub async fn layers_from_handles<K: Key, V: Value>(
    handles: impl IntoIterator<Item = impl ReadObjectHandle + 'static>,
) -> Result<Vec<Arc<dyn Layer<K, V>>>, Error> {
    let mut layers = Vec::new();
    for handle in handles {
        layers.push(PersistentLayer::open(handle).await? as Arc<dyn Layer<K, V>>);
    }
    Ok(layers)
}

#[derive(Eq, PartialEq, Debug)]
pub enum Operation {
    Insert,
    ReplaceOrInsert,
    MergeInto,
}

pub type MutationCallback<K, V> = Option<Box<dyn Fn(Operation, &Item<K, V>) + Send + Sync>>;

struct Inner<K, V> {
    mutable_layer: Arc<SkipListLayer<K, V>>,
    layers: Vec<Arc<dyn Layer<K, V>>>,
    mutation_callback: MutationCallback<K, V>,
}

pub const LOG2_HISTOGRAM_BUCKETS: usize = 32;

/// Metrics related to LSM tree churn, layer depths, and compaction performance.
pub struct CompactionCounters {
    /// Total number of compaction events that merged layers together.
    pub compactions: u64,
    /// Total bytes written during compaction, useful for measuring write amplification.
    pub compaction_bytes_written: u64,
    /// Total duration spent compacting layers.
    pub compaction_time_ns: u64,
    /// Number of mutable layers sealed. Useful for measuring churn.
    pub total_layers_added: u64,
    /// The maximum depth of the LSM tree observed. An indicator of worst-case read amplification.
    pub max_layer_count: u64,
    /// Log2 histogram of the sizes of newly compacted layers, used to verify compaction heuristics.
    pub layer_size_histogram: [u64; LOG2_HISTOGRAM_BUCKETS],
}

impl Default for CompactionCounters {
    fn default() -> Self {
        Self {
            compactions: 0,
            compaction_bytes_written: 0,
            compaction_time_ns: 0,
            total_layers_added: 0,
            max_layer_count: 0,
            layer_size_histogram: [0; LOG2_HISTOGRAM_BUCKETS],
        }
    }
}

/// Global counters and metrics for an LSM tree's lifetime.
pub struct TreeCounters {
    /// Number of individual key-lookup attempts (reads) through the tree.
    pub num_seeks: AtomicUsize,
    /// Tracks the number of layer files we might have looked at across all seeks.
    /// Used alongside `layer_files_skipped` to compute the effectiveness of bloom filters.
    pub layer_files_total: AtomicUsize,
    /// Tracks how many layer files we skipped searching thanks to the bloom filter rejecting them.
    pub layer_files_skipped: AtomicUsize,
    /// Tracks the number of queries that had an excessive number of fuzzy hash partitions.
    pub excessive_hash_partitions: AtomicUsize,
    /// Embedded counters for mutable metrics that require locking.
    pub compaction: Mutex<CompactionCounters>,
}

impl Default for TreeCounters {
    fn default() -> Self {
        Self {
            num_seeks: AtomicUsize::new(0),
            layer_files_total: AtomicUsize::new(0),
            layer_files_skipped: AtomicUsize::new(0),
            excessive_hash_partitions: AtomicUsize::new(0),
            compaction: Mutex::new(CompactionCounters::default()),
        }
    }
}

/// Writes the items yielded by the iterator into the supplied object.
#[fxfs_trace::trace]
pub async fn compact_with_iterator<K: Key, V: Value, W: WriteBytes + Send>(
    mut iterator: impl LayerIterator<K, V>,
    num_items: usize,
    writer: W,
    block_size: BlockSize,
    mut yielder: Option<impl Yielder>,
) -> Result<u64, Error> {
    let mut writer = PersistentLayerWriter::<W, K, V>::new(writer, num_items, block_size).await?;
    while let Some(item_ref) = iterator.get() {
        debug!(item_ref:?; "compact: writing");
        writer.write(item_ref).await?;
        iterator.advance().await?;
        if let Some(y) = yielder.as_mut() {
            y.yield_now().await;
        }
    }
    writer.complete().await
}

/// LSMTree manages a tree of layers to provide a key/value store.  Each layer contains deltas on
/// the preceding layer.  The top layer is an in-memory mutable layer.  Layers can be compacted to
/// form a new combined layer.
pub struct LSMTree<K, V> {
    data: RwLock<Inner<K, V>>,
    merge_fn: merge::MergeFn<K, V>,
    cache: Option<Box<dyn ObjectCache<K, V>>>,
    counters: Arc<TreeCounters>,
}

#[fxfs_trace::trace]
impl<'tree, K: MergeableKey, V: Value> LSMTree<K, V> {
    /// Creates a new empty tree.
    pub fn new(merge_fn: merge::MergeFn<K, V>, cache: Option<Box<dyn ObjectCache<K, V>>>) -> Self {
        let counters = TreeCounters::default();
        counters.compaction.lock().unwrap().max_layer_count = 1;
        LSMTree {
            data: RwLock::new(Inner {
                mutable_layer: Self::new_mutable_layer(),
                layers: Vec::new(),
                mutation_callback: None,
            }),
            merge_fn,
            cache,
            counters: Arc::new(counters),
        }
    }

    /// Opens an existing tree from the provided handles to the layer objects.
    pub async fn open(
        merge_fn: merge::MergeFn<K, V>,
        handles: impl IntoIterator<Item = impl ReadObjectHandle + 'static>,
        cache: Option<Box<dyn ObjectCache<K, V>>>,
    ) -> Result<Self, Error> {
        let layers = layers_from_handles(handles).await?;
        let max_layer_count = layers.len() as u64 + 1;
        let counters = TreeCounters::default();
        counters.compaction.lock().unwrap().max_layer_count = max_layer_count;
        Ok(LSMTree {
            data: RwLock::new(Inner {
                mutable_layer: Self::new_mutable_layer(),
                layers,
                mutation_callback: None,
            }),
            merge_fn,
            cache,
            counters: Arc::new(counters),
        })
    }

    /// Replaces the immutable layers.
    pub fn set_layers(&self, layers: Vec<Arc<dyn Layer<K, V>>>) {
        let mut data = self.data.write();
        data.layers = layers;
        let layer_count = data.layers.len() + 1;
        let mut counters = self.counters.compaction.lock().unwrap();
        counters.max_layer_count = std::cmp::max(counters.max_layer_count, layer_count as u64);
    }

    /// Appends to the given layers at the end i.e. they should be base layers.  This is supposed
    /// to be used after replay when we are opening a tree and we have discovered the base layers.
    pub async fn append_layers(
        &self,
        handles: impl IntoIterator<Item = impl ReadObjectHandle + 'static>,
    ) -> Result<(), Error> {
        let mut layers = layers_from_handles(handles).await?;
        let mut data = self.data.write();
        data.layers.append(&mut layers);
        let layer_count = data.layers.len() + 1;
        let mut counters = self.counters.compaction.lock().unwrap();
        counters.max_layer_count = std::cmp::max(counters.max_layer_count, layer_count as u64);
        Ok(())
    }

    /// Resets the immutable layers.
    pub fn reset_immutable_layers(&self) {
        self.data.write().layers = Vec::new();
    }

    /// Seals the current mutable layer and creates a new one.
    pub fn seal(&self) {
        // We need to be sure there are no mutations currently in-progress.  This is currently
        // guaranteed by ensuring that all mutations take a read lock on `data`.
        let mut data = self.data.write();
        let layer = std::mem::replace(&mut data.mutable_layer, Self::new_mutable_layer());
        data.layers.insert(0, layer);
        let layer_count = data.layers.len() + 1;
        let mut counters = self.counters.compaction.lock().unwrap();
        counters.max_layer_count = std::cmp::max(counters.max_layer_count, layer_count as u64);
        counters.total_layers_added += 1;
    }

    /// Resets the tree to an empty state.
    pub fn reset(&self) {
        let mut data = self.data.write();
        data.layers = Vec::new();
        data.mutable_layer = Self::new_mutable_layer();
    }

    /// Clears all in-memory caches (object cache and persistent layer chunk caches) for this tree.
    pub fn clear_cache(&self) {
        if let Some(cache) = &self.cache {
            cache.clear();
        }
        for layer in self.immutable_layer_set().layers {
            layer.clear_cached_data();
        }
    }

    pub fn cache_len(&self) -> usize {
        self.cache.as_ref().map_or(0, |c| c.len())
    }

    pub fn report_compaction_metrics(
        &self,
        bytes_written: u64,
        duration: std::time::Duration,
        layer_count: usize,
    ) {
        let mut counters = self.counters.compaction.lock().unwrap();
        counters.compactions += 1;
        counters.compaction_bytes_written += bytes_written;
        counters.compaction_time_ns += duration.as_nanos() as u64;

        let bucket = if bytes_written == 0 {
            0
        } else {
            std::cmp::min(LOG2_HISTOGRAM_BUCKETS - 1, 63 - bytes_written.leading_zeros() as usize)
        };
        counters.layer_size_histogram[bucket] += 1;

        crate::metrics::lsm_tree_metrics().compaction_layer_stack_depth.insert(layer_count as u64);
    }

    pub fn compaction_bytes_written(&self) -> u64 {
        self.counters.compaction.lock().unwrap().compaction_bytes_written
    }

    /// Returns an empty layer-set for this tree.
    pub fn empty_layer_set(&self) -> LayerSet<K, V> {
        LayerSet { layers: Vec::new(), merge_fn: self.merge_fn, counters: self.counters.clone() }
    }

    /// Adds all the layers (including the mutable layer) to `layer_set`.
    pub fn add_all_layers_to_layer_set(&self, layer_set: &mut LayerSet<K, V>) {
        let data = self.data.read();
        layer_set.layers.reserve_exact(data.layers.len() + 1);
        layer_set
            .layers
            .push(LockedLayer::from(data.mutable_layer.clone() as Arc<dyn Layer<K, V>>));
        for layer in &data.layers {
            layer_set.layers.push(layer.clone().into());
        }
    }

    /// Returns a clone of the current set of layers (including the mutable layer), after which one
    /// can get an iterator.
    pub fn layer_set(&self) -> LayerSet<K, V> {
        let mut layer_set = self.empty_layer_set();
        self.add_all_layers_to_layer_set(&mut layer_set);
        layer_set
    }

    /// Returns the current set of immutable layers after which one can get an iterator (for e.g.
    /// compacting).  Since these layers are immutable, getting an iterator should not block
    /// anything else.
    pub fn immutable_layer_set(&self) -> LayerSet<K, V> {
        let data = self.data.read();
        let mut layers = Vec::with_capacity(data.layers.len());
        for layer in &data.layers {
            layers.push(layer.clone().into());
        }
        LayerSet { layers, merge_fn: self.merge_fn, counters: self.counters.clone() }
    }

    /// Inserts an item into the mutable layer.
    /// Returns error if item already exists.
    pub fn insert(&self, item: Item<K, V>) -> Result<(), Error> {
        let _measure = DurationMeasureScope::new(&crate::metrics::lsm_tree_metrics().insert);

        let item_dup = if self.is_cacheable(&item.key) { Some(item.clone()) } else { None };

        {
            // `seal` below relies on us holding a read lock whilst we do the mutation.
            let data = self.data.read();
            if let Some(mutation_callback) = data.mutation_callback.as_ref() {
                mutation_callback(Operation::Insert, &item);
            }
            data.mutable_layer.insert(item)?;
        }

        if let Some(item) = item_dup {
            let val = if item.value == V::DELETED_MARKER { None } else { Some(item.value) };
            self.invalidate_cache(&item.key, val);
        }
        Ok(())
    }

    /// Replaces or inserts an item into the mutable layer.
    pub fn replace_or_insert(&self, item: Item<K, V>) {
        let _measure =
            DurationMeasureScope::new(&crate::metrics::lsm_tree_metrics().replace_or_insert);

        let item_dup = if self.is_cacheable(&item.key) { Some(item.clone()) } else { None };

        {
            // `seal` below relies on us holding a read lock whilst we do the mutation.
            let data = self.data.read();
            if let Some(mutation_callback) = data.mutation_callback.as_ref() {
                mutation_callback(Operation::ReplaceOrInsert, &item);
            }
            data.mutable_layer.replace_or_insert(item);
        }

        if let Some(item) = item_dup {
            let val = if item.value == V::DELETED_MARKER { None } else { Some(item.value) };
            self.invalidate_cache(&item.key, val);
        }
    }

    /// Merges the given item into the mutable layer.
    pub fn merge_into(&self, item: Item<K, V>, lower_bound: &K) {
        let _measure = DurationMeasureScope::new(&crate::metrics::lsm_tree_metrics().merge_into);

        let key = if self.is_cacheable(&item.key) { Some(item.key.clone()) } else { None };
        {
            // `seal` below relies on us holding a read lock whilst we do the mutation.
            let data = self.data.read();
            if let Some(mutation_callback) = data.mutation_callback.as_ref() {
                mutation_callback(Operation::MergeInto, &item);
            }
            data.mutable_layer.merge_into(item, lower_bound, self.merge_fn);
        }

        if let Some(key) = key {
            self.invalidate_cache(&key, None);
        }
    }

    /// Returns true if `key` is cacheable by the underlying cache.
    ///
    /// Returns false if no cache is configured or if the key is not cacheable.
    /// This allows tree mutation operations (`insert`, `replace_or_insert`, `merge_into`) to skip
    /// cloning keys and values when cache invalidation would be a no-op.
    #[inline(always)]
    fn is_cacheable(&self, key: &K) -> bool {
        if let Some(cache) = &self.cache { cache.is_cacheable(key) } else { false }
    }

    /// Invalidates the cache entry for `key` if a cache is configured.
    ///
    /// If `value` is Some, the cache entry may be updated with the new value. If `value` is None,
    /// the entry is removed from the cache. Does nothing if no cache is configured.
    #[inline(always)]
    fn invalidate_cache(&self, key: &K, value: Option<V>) {
        if let Some(cache) = &self.cache {
            cache.invalidate(key, value);
        }
    }

    /// Searches for an exact match for the given key, applying `f` to the found [`ItemRef`].
    /// If the item does not exist or has a value equal to [`Value::DELETED_MARKER`], returns
    /// `Ok(None)`.
    ///
    /// Range keys (`FuzzyHash::is_range_key`) are not supported.
    pub async fn find_map<R, F>(&self, search_key: &K, f: F) -> Result<Option<R>, Error>
    where
        K: Eq,
        F: FnOnce(ItemRef<'_, K, V>) -> R,
    {
        let _measure = DurationMeasureScope::new(&crate::metrics::lsm_tree_metrics().find);
        // It is important that the cache lookup is done prior to fetching the layer set as the
        // placeholder returned acts as a sort of lock for the validity of the item that may be
        // inserted later via that placeholder.
        let mut token = if let Some(cache) = &self.cache {
            match cache.lookup_or_reserve(search_key) {
                ObjectCacheResult::Value(value) => {
                    if *value == V::DELETED_MARKER {
                        return Ok(None);
                    } else {
                        return Ok(Some(f(ItemRef { key: search_key, value: &value })));
                    }
                }
                ObjectCacheResult::Placeholder(token) => Some(token),
                ObjectCacheResult::NoCache => None,
            }
        } else {
            None
        };
        let layer_set = self.layer_set();
        let result = layer_set
            .find_map(search_key, |item_ref| {
                if let Some(token) = token.take() {
                    token.complete(Some(item_ref.value));
                }
                f(item_ref)
            })
            .await?;
        Ok(result)
    }

    /// Searches for an exact match for the given key. If the item does not exist or has a value
    /// equal to [`Value::DELETED_MARKER`], returns `Ok(None)`.
    ///
    /// Range keys (`FuzzyHash::is_range_key`) are not supported.
    pub fn find(
        &self,
        search_key: &K,
    ) -> impl Future<Output = Result<Option<Item<K, V>>, Error>> + Send
    where
        K: Eq,
    {
        self.find_map(search_key, |item| Item { key: item.key.clone(), value: item.value.clone() })
    }

    /// Searches for an exact match for the given key, returning only the value. If the item does
    /// not exist or has a value equal to [`Value::DELETED_MARKER`], returns `Ok(None)`.
    ///
    /// Range keys (`FuzzyHash::is_range_key`) are not supported.
    pub fn find_value(
        &self,
        search_key: &K,
    ) -> impl Future<Output = Result<Option<V>, Error>> + Send
    where
        K: Eq,
    {
        self.find_map(search_key, |item| item.value.clone())
    }

    /// Returns true if an exact match exists for the given key and is not deleted.
    ///
    /// Range keys (`FuzzyHash::is_range_key`) are not supported.
    pub async fn exists(&self, search_key: &K) -> Result<bool, Error>
    where
        K: Eq,
    {
        Ok(self.find_map(search_key, |_| ()).await?.is_some())
    }

    pub fn mutable_layer(&self) -> Arc<SkipListLayer<K, V>> {
        self.data.read().mutable_layer.clone()
    }

    /// Sets a mutation callback which is a callback that is triggered whenever any mutations are
    /// applied to the tree.  This might be useful for tests that want to record the precise
    /// sequence of mutations that are applied to the tree.
    pub fn set_mutation_callback(&self, mutation_callback: MutationCallback<K, V>) {
        self.data.write().mutation_callback = mutation_callback;
    }

    /// Returns the earliest version used by a layer in the tree.
    pub fn get_earliest_version(&self) -> Version {
        let mut earliest_version = LATEST_VERSION;
        for layer in self.layer_set().layers {
            let layer_version = layer.get_version();
            if layer_version < earliest_version {
                earliest_version = layer_version;
            }
        }
        return earliest_version;
    }

    /// Returns a new mutable layer.
    pub fn new_mutable_layer() -> Arc<SkipListLayer<K, V>> {
        SkipListLayer::new(SKIP_LIST_LAYER_ITEMS)
    }

    /// Replaces the mutable layer.
    pub fn set_mutable_layer(&self, layer: Arc<SkipListLayer<K, V>>) {
        self.data.write().mutable_layer = layer;
    }

    /// Records inspect data for the LSM tree into `node`.  Called lazily when inspect is queried.
    pub fn record_inspect_data(&self, root: &fuchsia_inspect::Node) {
        let layer_set = self.layer_set();
        root.record_child("layers", move |node| {
            let mut index = 0;
            for layer in layer_set.layers {
                node.record_child(format!("{index}"), move |node| {
                    layer.1.record_inspect_data(node)
                });
                index += 1;
            }
        });
        {
            let counters = self.counters.compaction.lock().unwrap();
            root.record_uint("num_seeks", self.counters.num_seeks.load(Ordering::Relaxed) as u64);
            root.record_uint("bloom_filter_success_percent", {
                let layer_files_total = self.counters.layer_files_total.load(Ordering::Relaxed);
                let layer_files_skipped = self.counters.layer_files_skipped.load(Ordering::Relaxed);
                if layer_files_total == 0 {
                    0
                } else {
                    (layer_files_skipped * 100).div_ceil(layer_files_total) as u64
                }
            });
            root.record_uint(
                "excessive_hash_partitions",
                self.counters.excessive_hash_partitions.load(Ordering::Relaxed) as u64,
            );
            root.record_uint("compactions", counters.compactions);
            root.record_uint("compaction_bytes_written", counters.compaction_bytes_written);
            root.record_uint("compaction_time_ns", counters.compaction_time_ns);
            root.record_uint("total_layers_added", counters.total_layers_added);
            root.record_uint("max_layer_count", counters.max_layer_count);

            let layer_sizes = root.create_uint_exponential_histogram(
                "layer_size_histogram_log2",
                fuchsia_inspect::ExponentialHistogramParams {
                    floor: 1,
                    initial_step: 1,
                    step_multiplier: 2,
                    buckets: LOG2_HISTOGRAM_BUCKETS,
                },
            );
            for (i, count) in counters.layer_size_histogram.iter().enumerate() {
                layer_sizes.insert_multiple(1u64 << i, *count as usize);
            }
            root.record(layer_sizes);
        }
    }
}

/// This is an RAII wrapper for a layer which holds a lock on the layer (via the Layer::lock
/// method).
pub struct LockedLayer<K, V>(Arc<DropEvent>, Arc<dyn Layer<K, V>>);

impl<K, V> LockedLayer<K, V> {
    pub async fn close_layer(self) {
        let layer = self.1;
        std::mem::drop(self.0);
        layer.close().await;
    }
}

impl<K, V> From<Arc<dyn Layer<K, V>>> for LockedLayer<K, V> {
    fn from(layer: Arc<dyn Layer<K, V>>) -> Self {
        let event = layer.lock().unwrap();
        Self(event, layer)
    }
}

impl<K, V> std::ops::Deref for LockedLayer<K, V> {
    type Target = Arc<dyn Layer<K, V>>;

    fn deref(&self) -> &Self::Target {
        &self.1
    }
}

impl<K, V> AsRef<dyn Layer<K, V>> for LockedLayer<K, V> {
    fn as_ref(&self) -> &(dyn Layer<K, V> + 'static) {
        self.1.as_ref()
    }
}

/// A LayerSet provides a snapshot of the layers at a particular point in time, and allows you to
/// get an iterator.  Iterators borrow the layers so something needs to hold reference count.
pub struct LayerSet<K, V> {
    pub layers: Vec<LockedLayer<K, V>>,
    merge_fn: merge::MergeFn<K, V>,
    counters: Arc<TreeCounters>,
}

impl<K: Key + LayerKey + OrdLowerBound, V: Value> LayerSet<K, V> {
    pub fn sum_len(&self) -> usize {
        let mut size = 0;
        for layer in &self.layers {
            size += layer.len()
        }
        size
    }

    pub fn merger(&self) -> merge::Merger<'_, K, V> {
        merge::Merger::new(
            self.layers.iter().map(|x| x.as_ref()),
            self.merge_fn,
            self.counters.clone(),
        )
    }

    /// Searches for an exact match for the given key, applying `f` to the found [`ItemRef`].
    /// If the item does not exist or has a value equal to [`Value::DELETED_MARKER`], returns
    /// `Ok(None)`.
    ///
    /// Range keys (`FuzzyHash::is_range_key`) are not supported.
    pub async fn find_map<R, F>(&self, search_key: &K, f: F) -> Result<Option<R>, Error>
    where
        K: Eq,
        F: FnOnce(ItemRef<'_, K, V>) -> R,
    {
        if search_key.merge_type() == MergeType::OptimizedMerge {
            self.counters.num_seeks.fetch_add(1, Ordering::Relaxed);
            for layer in &self.layers {
                self.counters.layer_files_total.fetch_add(1, Ordering::Relaxed);
                if layer.maybe_contains_key(search_key) == MaybeContainsKey::False {
                    self.counters.layer_files_skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let iter = layer.seek(Bound::Included(search_key)).await?;
                if let Some(item_ref) = iter.get() {
                    if item_ref.key == search_key {
                        if *item_ref.value == V::DELETED_MARKER {
                            return Ok(None);
                        } else {
                            return Ok(Some(f(item_ref)));
                        }
                    }
                }
            }
            return Ok(None);
        }

        let mut merger = self.merger();
        Ok(match merger.query(Query::Point(search_key)).await?.get() {
            Some(item_ref)
                if item_ref.key == search_key && *item_ref.value != V::DELETED_MARKER =>
            {
                Some(f(item_ref))
            }
            _ => None,
        })
    }

    /// See `Layer::key_exists`.
    pub async fn key_exists(&self, key: &K) -> Result<Existence, Error> {
        for l in &self.layers {
            match l.key_exists(key).await? {
                e @ (Existence::Exists | Existence::MaybeExists) => return Ok(e),
                _ => {}
            }
        }
        Ok(Existence::Missing)
    }
}

impl<K, V> fmt::Debug for LayerSet<K, V> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_list()
            .entries(self.layers.iter().map(|l| {
                if let Some(handle) = l.handle() {
                    format!("{}", handle.object_id())
                } else {
                    format!("{:?}", Arc::as_ptr(l))
                }
            }))
            .finish()
    }
}

/// A yielder can be used during compactions which are low priority.
pub trait Yielder: Send {
    fn yield_now(&mut self) -> impl Future<Output = ()> + Send;
}

#[cfg(test)]
mod tests {
    use super::{LSMTree, Yielder, compact_with_iterator};
    use crate::drop_event::DropEvent;
    use crate::lsm_tree::cache::{ObjectCache, ObjectCachePlaceholder, ObjectCacheResult};
    use crate::lsm_tree::merge::{ItemOp, MergeLayerIterator, MergeResult};
    use crate::lsm_tree::types::{
        BoxedLayerIterator, Existence, Item, ItemRef, Key, Layer, LayerIterator, MaybeContainsKey,
        Value,
    };
    use crate::lsm_tree::{Query, layers_from_handles};
    use crate::object_handle::ObjectHandle;
    use crate::serialized_types::{LATEST_VERSION, Version};
    use crate::testing::fake_object::{FakeObject, FakeObjectHandle};
    use crate::testing::writer::Writer;
    use anyhow::{Error, anyhow};
    use async_trait::async_trait;

    use fuchsia_sync::{Mutex, MutexGuard};

    use rand::rng;
    use rand::seq::SliceRandom as _;

    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::testing::TestKey;

    fn emit_left_merge_fn(
        _left: &MergeLayerIterator<'_, TestKey, u64>,
        _right: &MergeLayerIterator<'_, TestKey, u64>,
    ) -> MergeResult<TestKey, u64> {
        MergeResult::EmitLeft
    }

    fn emit_left_i32_merge_fn(
        _left: &MergeLayerIterator<'_, i32, u64>,
        _right: &MergeLayerIterator<'_, i32, u64>,
    ) -> MergeResult<i32, u64> {
        MergeResult::EmitLeft
    }

    fn merge_sum_i32(
        left: &MergeLayerIterator<'_, i32, u64>,
        right: &MergeLayerIterator<'_, i32, u64>,
    ) -> MergeResult<i32, u64> {
        if left.key() == right.key() {
            MergeResult::Other {
                emit: None,
                left: ItemOp::Discard,
                right: ItemOp::Replace(
                    Item::new(*left.key(), *left.value() + *right.value()).boxed(),
                ),
            }
        } else {
            MergeResult::EmitLeft
        }
    }

    impl Value for u64 {
        const DELETED_MARKER: Self = 0;
    }

    struct NoOpYielder;
    impl Yielder for NoOpYielder {
        async fn yield_now(&mut self) {}
    }

    #[fuchsia::test]
    async fn test_iteration() {
        let tree = LSMTree::new(emit_left_merge_fn, None);
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        tree.insert(items[0].clone()).expect("insert error");
        tree.insert(items[1].clone()).expect("insert error");
        let layers = tree.layer_set();
        let mut merger = layers.merger();
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.expect("advance failed");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.expect("advance failed");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_compact() {
        let tree = LSMTree::new(emit_left_merge_fn, None);
        let items = [
            Item::new(TestKey(1..1), 1),
            Item::new(TestKey(2..2), 2),
            Item::new(TestKey(3..3), 3),
            Item::new(TestKey(4..4), 4),
        ];
        tree.insert(items[0].clone()).expect("insert error");
        tree.insert(items[1].clone()).expect("insert error");
        tree.seal();
        tree.insert(items[2].clone()).expect("insert error");
        tree.insert(items[3].clone()).expect("insert error");
        tree.seal();
        let object = Arc::new(FakeObject::new());
        let handle = FakeObjectHandle::new(object.clone());
        {
            let layer_set = tree.immutable_layer_set();
            let mut merger = layer_set.merger();
            let iter = merger.query(Query::FullScan).await.expect("create merger");
            compact_with_iterator(
                iter,
                items.len(),
                Writer::new(&handle).await,
                handle.block_size(),
                Option::<NoOpYielder>::None,
            )
            .await
            .expect("compact failed");
        }
        tree.set_layers(layers_from_handles([handle]).await.expect("layers_from_handles failed"));
        let handle = FakeObjectHandle::new(object.clone());
        let tree = LSMTree::open(emit_left_merge_fn, [handle], None).await.expect("open failed");

        let layers = tree.layer_set();
        let mut merger = layers.merger();
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");
        for i in 1..5 {
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!((key, value), (&TestKey(i..i), &i));
            iter.advance().await.expect("advance failed");
        }
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_find() {
        let items = [
            Item::new(TestKey(1..1), 1),
            Item::new(TestKey(2..2), 2),
            Item::new(TestKey(3..3), 3),
            Item::new(TestKey(4..4), 4),
        ];
        let tree = LSMTree::new(emit_left_merge_fn, None);
        tree.insert(items[0].clone()).expect("insert error");
        tree.insert(items[1].clone()).expect("insert error");
        tree.seal();
        tree.insert(items[2].clone()).expect("insert error");
        tree.insert(items[3].clone()).expect("insert error");

        let item = tree.find(&items[1].key).await.expect("find failed").expect("not found");
        assert_eq!(item, items[1]);
        assert!(tree.find(&TestKey(100..100)).await.expect("find failed").is_none());

        assert_eq!(tree.find_value(&items[2].key).await.expect("find_value failed"), Some(3));
        assert_eq!(tree.find_value(&TestKey(100..100)).await.expect("find_value failed"), None);

        assert!(tree.exists(&items[0].key).await.expect("exists failed"));
        assert!(!tree.exists(&TestKey(100..100)).await.expect("exists failed"));

        assert_eq!(
            tree.find_map(&items[3].key, |item| item.value * 10).await.expect("find_map failed"),
            Some(40)
        );
        assert_eq!(
            tree.find_map(&TestKey(100..100), |item| item.value * 10)
                .await
                .expect("find_map failed"),
            None
        );
    }

    #[fuchsia::test]
    async fn test_find_no_return_deleted_values() {
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), u64::DELETED_MARKER)];
        let tree = LSMTree::new(emit_left_merge_fn, None);
        tree.insert(items[0].clone()).expect("insert error");
        tree.insert(items[1].clone()).expect("insert error");

        let item = tree.find(&items[0].key).await.expect("find failed").expect("not found");
        assert_eq!(item, items[0]);
        assert_eq!(tree.find_value(&items[0].key).await.expect("find_value failed"), Some(1));
        assert_eq!(
            tree.find_map(&items[0].key, |item| *item.value).await.expect("find_map failed"),
            Some(1)
        );
        assert!(tree.exists(&items[0].key).await.expect("exists failed"));

        assert!(tree.find(&items[1].key).await.expect("find failed").is_none());
        assert!(tree.find_value(&items[1].key).await.expect("find_value failed").is_none());
        assert!(tree.find_map(&items[1].key, |_| ()).await.expect("find_map failed").is_none());
        assert!(!tree.exists(&items[1].key).await.expect("exists failed"));
    }

    #[fuchsia::test]
    async fn test_find_full_merge() {
        let tree = LSMTree::new(emit_left_i32_merge_fn, None);
        let items = [
            Item::new(1, 10),
            Item::new(2, 20),
            Item::new(3, 30),
            Item::new(4, u64::DELETED_MARKER),
        ];
        for item in &items {
            tree.insert(item.clone()).expect("insert error");
        }

        assert_eq!(tree.find(&1).await.expect("find failed"), Some(items[0].clone()));
        assert_eq!(tree.find_value(&2).await.expect("find_value failed"), Some(20));
        assert!(tree.exists(&3).await.expect("exists failed"));
        assert_eq!(
            tree.find_map(&3, |item| *item.value * 2).await.expect("find_map failed"),
            Some(60)
        );

        // Missing key
        assert_eq!(tree.find(&99).await.expect("find failed"), None);
        assert_eq!(tree.find_value(&99).await.expect("find_value failed"), None);
        assert!(!tree.exists(&99).await.expect("exists failed"));
        assert_eq!(tree.find_map(&99, |_| ()).await.expect("find_map failed"), None);

        // Deleted key
        assert_eq!(tree.find(&4).await.expect("find failed"), None);
        assert_eq!(tree.find_value(&4).await.expect("find_value failed"), None);
        assert!(!tree.exists(&4).await.expect("exists failed"));
        assert_eq!(tree.find_map(&4, |_| ()).await.expect("find_map failed"), None);
    }

    #[fuchsia::test]
    async fn test_find_full_merge_multi_layer() {
        let tree = LSMTree::new(merge_sum_i32, None);

        // Base layer:
        tree.insert(Item::new(1, 100)).expect("insert error");
        tree.insert(Item::new(2, 200)).expect("insert error");
        tree.insert(Item::new(3, 300)).expect("insert error");

        tree.seal();

        // Top layer:
        // Key 1 has an update (+50) that should merge to 150.
        // Key 4 is new (40).
        tree.insert(Item::new(1, 50)).expect("insert error");
        tree.insert(Item::new(4, 40)).expect("insert error");

        // Key 1 should be merged across layers: 50 + 100 = 150.
        assert_eq!(tree.find_value(&1).await.expect("find_value failed"), Some(150));
        assert_eq!(tree.find(&1).await.expect("find failed"), Some(Item::new(1, 150)));
        assert!(tree.exists(&1).await.expect("exists failed"));
        assert_eq!(
            tree.find_map(&1, |item| *item.value * 2).await.expect("find_map failed"),
            Some(300)
        );

        // Key 2 only exists in the base layer: 200.
        assert_eq!(tree.find_value(&2).await.expect("find_value failed"), Some(200));
        assert!(tree.exists(&2).await.expect("exists failed"));

        // Key 3 only exists in the base layer: 300.
        assert_eq!(tree.find_value(&3).await.expect("find_value failed"), Some(300));
        assert!(tree.exists(&3).await.expect("exists failed"));

        // Key 4 only exists in the top layer: 40.
        assert_eq!(tree.find_value(&4).await.expect("find_value failed"), Some(40));
        assert!(tree.exists(&4).await.expect("exists failed"));

        // Key 5 is missing entirely.
        assert_eq!(tree.find_value(&5).await.expect("find_value failed"), None);
        assert!(!tree.exists(&5).await.expect("exists failed"));
    }

    #[fuchsia::test]
    async fn test_find_optimized_merge_multi_layer() {
        let tree = LSMTree::new(emit_left_merge_fn, None);

        // Insert items in base layer (older).
        tree.insert(Item::new(TestKey(1..1), 10)).expect("insert error");
        tree.insert(Item::new(TestKey(2..2), 20)).expect("insert error");
        tree.insert(Item::new(TestKey(3..3), 30)).expect("insert error");

        tree.seal();

        // In the newer layer:
        // - Update key 2 to 25
        // - Tombstone key 3
        // - Insert new key 4
        tree.insert(Item::new(TestKey(2..2), 25)).expect("insert error");
        tree.insert(Item::new(TestKey(3..3), u64::DELETED_MARKER)).expect("insert error");
        tree.insert(Item::new(TestKey(4..4), 40)).expect("insert error");

        // Key 1: only in older layer.
        assert_eq!(tree.find_value(&TestKey(1..1)).await.expect("find_value failed"), Some(10));
        assert!(tree.exists(&TestKey(1..1)).await.expect("exists failed"));

        // Key 2: updated in newer layer (shadows older layer).
        assert_eq!(tree.find_value(&TestKey(2..2)).await.expect("find_value failed"), Some(25));
        assert!(tree.exists(&TestKey(2..2)).await.expect("exists failed"));

        // Key 3: deleted in newer layer (shadows older layer).
        assert_eq!(tree.find(&TestKey(3..3)).await.expect("find failed"), None);
        assert_eq!(tree.find_value(&TestKey(3..3)).await.expect("find_value failed"), None);
        assert!(!tree.exists(&TestKey(3..3)).await.expect("exists failed"));
        assert_eq!(tree.find_map(&TestKey(3..3), |_| ()).await.expect("find_map failed"), None);

        // Key 4: only in newer layer.
        assert_eq!(tree.find_value(&TestKey(4..4)).await.expect("find_value failed"), Some(40));
        assert!(tree.exists(&TestKey(4..4)).await.expect("exists failed"));

        // Key 5: missing in all layers.
        assert_eq!(tree.find_value(&TestKey(5..5)).await.expect("find_value failed"), None);
        assert!(!tree.exists(&TestKey(5..5)).await.expect("exists failed"));
    }

    #[fuchsia::test]
    async fn test_find_bloom_filter_rejection() {
        struct BloomFilterMockLayer {
            drop_event: Mutex<Option<Arc<DropEvent>>>,
        }

        impl BloomFilterMockLayer {
            fn new() -> Self {
                Self { drop_event: Mutex::new(Some(Arc::new(DropEvent::new()))) }
            }
        }

        #[async_trait]
        impl<K: Key, V: Value> Layer<K, V> for BloomFilterMockLayer {
            async fn seek(
                &self,
                _bound: std::ops::Bound<&K>,
            ) -> Result<BoxedLayerIterator<'_, K, V>, Error> {
                panic!("seek should not be called on layer skipped by bloom filter");
            }

            fn lock(&self) -> Option<Arc<DropEvent>> {
                self.drop_event.lock().clone()
            }

            fn len(&self) -> usize {
                0
            }

            async fn close(&self) {}

            fn get_version(&self) -> Version {
                LATEST_VERSION
            }

            fn maybe_contains_key(&self, _key: &K) -> MaybeContainsKey {
                MaybeContainsKey::False
            }

            async fn key_exists(&self, _key: &K) -> Result<Existence, Error> {
                unimplemented!()
            }
        }

        let tree = LSMTree::new(emit_left_merge_fn, None);
        let layer: Arc<dyn Layer<TestKey, u64>> = Arc::new(BloomFilterMockLayer::new());
        tree.set_layers(vec![layer]);

        let skipped_before = tree.counters.layer_files_skipped.load(Ordering::Relaxed);
        let total_before = tree.counters.layer_files_total.load(Ordering::Relaxed);

        // Searching for any key: the bloom filter on the immutable layer reports False,
        // so seek() is never called (which would panic) and the layer is skipped.
        assert_eq!(tree.find_value(&TestKey(1..1)).await.expect("find failed"), None);
        assert!(!tree.exists(&TestKey(1..1)).await.expect("exists failed"));

        let skipped_after = tree.counters.layer_files_skipped.load(Ordering::Relaxed);
        let total_after = tree.counters.layer_files_total.load(Ordering::Relaxed);

        // 2 queries (find_value and exists); each inspects mutable layer + 1 immutable mock layer.
        // The mock layer is skipped by the bloom filter on both queries.
        assert_eq!(skipped_after - skipped_before, 2);
        assert_eq!(total_after - total_before, 4);
    }

    #[fuchsia::test]
    async fn test_empty_seal() {
        let tree = LSMTree::new(emit_left_merge_fn, None);
        tree.seal();
        let item = Item::new(TestKey(1..1), 1);
        tree.insert(item.clone()).expect("insert error");
        let object = Arc::new(FakeObject::new());
        let handle = FakeObjectHandle::new(object.clone());
        {
            let layer_set = tree.immutable_layer_set();
            let mut merger = layer_set.merger();
            let iter = merger.query(Query::FullScan).await.expect("create merger");
            compact_with_iterator(
                iter,
                0,
                Writer::new(&handle).await,
                handle.block_size(),
                Option::<NoOpYielder>::None,
            )
            .await
            .expect("compact failed");
        }
        tree.set_layers(layers_from_handles([handle]).await.expect("layers_from_handles failed"));
        let found_item = tree.find(&item.key).await.expect("find failed").expect("not found");
        assert_eq!(found_item, item);
        assert!(!tree.exists(&TestKey(2..2)).await.expect("find failed"));
    }

    #[fuchsia::test]
    async fn test_filter() {
        let items = [
            Item::new(TestKey(1..1), 1),
            Item::new(TestKey(2..2), 2),
            Item::new(TestKey(3..3), 3),
            Item::new(TestKey(4..4), 4),
        ];
        let tree = LSMTree::new(emit_left_merge_fn, None);
        tree.insert(items[0].clone()).expect("insert error");
        tree.insert(items[1].clone()).expect("insert error");
        tree.insert(items[2].clone()).expect("insert error");
        tree.insert(items[3].clone()).expect("insert error");

        let layers = tree.layer_set();
        let mut merger = layers.merger();

        // Filter out odd keys (which also guarantees we skip the first key which is an edge case).
        let mut iter = merger
            .query(Query::FullScan)
            .await
            .expect("seek failed")
            .filter(|item: ItemRef<'_, TestKey, u64>| item.key.0.start % 2 == 0)
            .await
            .expect("filter failed");

        assert_eq!(iter.get(), Some(items[1].as_item_ref()));
        iter.advance().await.expect("advance failed");
        assert_eq!(iter.get(), Some(items[3].as_item_ref()));
        iter.advance().await.expect("advance failed");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_insert_order_agnostic() {
        let items = [
            Item::new(TestKey(1..1), 1),
            Item::new(TestKey(2..2), 2),
            Item::new(TestKey(3..3), 3),
            Item::new(TestKey(4..4), 4),
            Item::new(TestKey(5..5), 5),
            Item::new(TestKey(6..6), 6),
        ];
        let a = LSMTree::new(emit_left_merge_fn, None);
        for item in &items {
            a.insert(item.clone()).expect("insert error");
        }
        let b = LSMTree::new(emit_left_merge_fn, None);
        let mut shuffled = items.clone();
        shuffled.shuffle(&mut rng());
        for item in &shuffled {
            b.insert(item.clone()).expect("insert error");
        }
        let layers = a.layer_set();
        let mut merger = layers.merger();
        let mut iter_a = merger.query(Query::FullScan).await.expect("seek failed");
        let layers = b.layer_set();
        let mut merger = layers.merger();
        let mut iter_b = merger.query(Query::FullScan).await.expect("seek failed");

        for item in items {
            assert_eq!(Some(item.as_item_ref()), iter_a.get());
            assert_eq!(Some(item.as_item_ref()), iter_b.get());
            iter_a.advance().await.expect("advance failed");
            iter_b.advance().await.expect("advance failed");
        }
        assert!(iter_a.get().is_none());
        assert!(iter_b.get().is_none());
    }

    enum AuditCacheResult<V> {
        Value(V),
        NoCache,
    }

    struct AuditCacheInner<V: Value> {
        lookups: u64,
        completions: u64,
        invalidations: u64,
        drops: u64,
        result: Option<AuditCacheResult<V>>,
        is_cacheable: bool,
    }

    impl<V: Value> AuditCacheInner<V> {
        fn stats(&self) -> (u64, u64, u64, u64) {
            (self.lookups, self.completions, self.invalidations, self.drops)
        }
    }

    struct AuditCache<V: Value> {
        inner: Arc<Mutex<AuditCacheInner<V>>>,
    }

    impl<V: Value> AuditCache<V> {
        fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(AuditCacheInner {
                    lookups: 0,
                    completions: 0,
                    invalidations: 0,
                    drops: 0,
                    result: None,
                    is_cacheable: true,
                })),
            }
        }
    }

    struct AuditPlaceholder<V: Value> {
        inner: Arc<Mutex<AuditCacheInner<V>>>,
        completed: Mutex<bool>,
    }

    impl<V: Value> ObjectCachePlaceholder<V> for AuditPlaceholder<V> {
        fn complete(self: Box<Self>, _: Option<&V>) {
            self.inner.lock().completions += 1;
            *self.completed.lock() = true;
        }
    }

    impl<V: Value> Drop for AuditPlaceholder<V> {
        fn drop(&mut self) {
            if !*self.completed.lock() {
                self.inner.lock().drops += 1;
            }
        }
    }

    impl<K: Key + std::cmp::PartialEq, V: Value> ObjectCache<K, V> for AuditCache<V> {
        fn lookup_or_reserve(&self, _key: &K) -> ObjectCacheResult<'_, V> {
            {
                let mut inner = self.inner.lock();
                inner.lookups += 1;
                match MutexGuard::try_map_or_err(inner, |inner| match &mut inner.result {
                    Some(AuditCacheResult::Value(value)) => Ok(value),
                    Some(AuditCacheResult::NoCache) => Err(false),
                    None => Err(true),
                }) {
                    Ok(guard) => {
                        return ObjectCacheResult::Value(guard);
                    }
                    Err((_, false)) => {
                        return ObjectCacheResult::NoCache;
                    }
                    Err((_, true)) => {}
                }
            }
            ObjectCacheResult::Placeholder(Box::new(AuditPlaceholder {
                inner: self.inner.clone(),
                completed: Mutex::new(false),
            }))
        }

        fn is_cacheable(&self, _key: &K) -> bool {
            self.inner.lock().is_cacheable
        }

        fn invalidate(&self, _key: &K, _value: Option<V>) {
            self.inner.lock().invalidations += 1;
        }

        fn clear(&self) {
            self.inner.lock().result = None;
        }
    }

    #[fuchsia::test]
    async fn test_cache_handling() {
        let item = Item::new(TestKey(1..1), 1);
        let cache = Box::new(AuditCache::new());
        let inner = cache.inner.clone();
        let a = LSMTree::new(emit_left_merge_fn, Some(cache));

        // Zero counters.
        assert_eq!(inner.lock().stats(), (0, 0, 0, 0));

        // Look for an item, but don't find it. So no insertion. It is dropped.
        assert!(!a.exists(&item.key).await.expect("Failed find"));
        assert_eq!(inner.lock().stats(), (1, 0, 0, 1));

        // Insert attempts to invalidate.
        let _ = a.insert(item.clone());
        assert_eq!(inner.lock().stats(), (1, 0, 1, 1));

        // Look for item, find it and insert into the cache.
        assert_eq!(
            a.find(&item.key).await.expect("Failed find").expect("Item should be found.").value,
            item.value
        );
        assert_eq!(inner.lock().stats(), (2, 1, 1, 1));

        // find_value on item2 also inserts into cache
        let item2 = Item::new(TestKey(2..2), 2);
        let _ = a.insert(item2.clone());
        assert_eq!(a.find_value(&item2.key).await.expect("Failed find_value"), Some(item2.value));
        assert_eq!(inner.lock().stats(), (3, 2, 2, 1));

        // exists on item3 also inserts into cache
        let item3 = Item::new(TestKey(3..3), 3);
        let _ = a.insert(item3.clone());
        assert!(a.exists(&item3.key).await.expect("Failed exists"));
        assert_eq!(inner.lock().stats(), (4, 3, 3, 1));

        // find_map on item4 also inserts into cache
        let item4 = Item::new(TestKey(4..4), 4);
        let _ = a.insert(item4.clone());
        assert_eq!(
            a.find_map(&item4.key, |item| item.value * 2).await.expect("Failed find_map"),
            Some(8)
        );
        assert_eq!(inner.lock().stats(), (5, 4, 4, 1));

        // Insert or replace attempts to invalidate as well.
        a.replace_or_insert(item.clone());
        assert_eq!(inner.lock().stats(), (5, 4, 5, 1));
    }

    #[fuchsia::test]
    async fn test_cache_hit() {
        let item = Item::new(TestKey(1..1), 1);
        let cache = Box::new(AuditCache::new());
        let inner = cache.inner.clone();
        let a = LSMTree::new(emit_left_merge_fn, Some(cache));

        // Zero counters.
        assert_eq!(inner.lock().stats(), (0, 0, 0, 0));

        // Insert attempts to invalidate.
        let _ = a.insert(item.clone());
        assert_eq!(inner.lock().stats(), (0, 0, 1, 0));

        // Set up the item to find in the cache.
        inner.lock().result = Some(AuditCacheResult::Value(item.value.clone()));

        // Look for item, find it in cache, so no insert.
        assert_eq!(
            a.find_value(&item.key).await.expect("Failed find").expect("Item should be found."),
            item.value
        );
        assert_eq!(inner.lock().stats(), (1, 0, 1, 0));
    }

    #[fuchsia::test]
    async fn test_cache_hit_deleted_marker() {
        let item = Item::new(TestKey(1..1), 1);
        let cache = Box::new(AuditCache::new());
        let inner = cache.inner.clone();
        let a = LSMTree::new(emit_left_merge_fn, Some(cache));

        // Zero counters.
        assert_eq!(inner.lock().stats(), (0, 0, 0, 0));

        // Insert attempts to invalidate.
        let _ = a.insert(item.clone());
        assert_eq!(inner.lock().stats(), (0, 0, 1, 0));

        // Set up the cache to return DELETED_MARKER.
        inner.lock().result = Some(AuditCacheResult::Value(u64::DELETED_MARKER));

        // find_value should hit the cache, see DELETED_MARKER, and return None.
        assert_eq!(a.find_value(&item.key).await.expect("Failed find_value"), None);
        assert_eq!(inner.lock().stats(), (1, 0, 1, 0));

        // exists should also return false on cache hit DELETED_MARKER.
        inner.lock().result = Some(AuditCacheResult::Value(u64::DELETED_MARKER));
        assert!(!a.exists(&item.key).await.expect("Failed exists"));
        assert_eq!(inner.lock().stats(), (2, 0, 1, 0));

        // find should also return None on cache hit DELETED_MARKER.
        inner.lock().result = Some(AuditCacheResult::Value(u64::DELETED_MARKER));
        assert!(a.find(&item.key).await.expect("Failed find").is_none());
        assert_eq!(inner.lock().stats(), (3, 0, 1, 0));

        // find_map should also return None on cache hit DELETED_MARKER.
        inner.lock().result = Some(AuditCacheResult::Value(u64::DELETED_MARKER));
        assert!(a.find_map(&item.key, |_| ()).await.expect("Failed find_map").is_none());
        assert_eq!(inner.lock().stats(), (4, 0, 1, 0));
    }

    #[fuchsia::test]
    async fn test_cache_says_uncacheable() {
        let item = Item::new(TestKey(1..1), 1);
        let cache = Box::new(AuditCache::new());
        let inner = cache.inner.clone();
        let a = LSMTree::new(emit_left_merge_fn, Some(cache));
        let _ = a.insert(item.clone());

        // One invalidation from the insert.
        assert_eq!(inner.lock().stats(), (0, 0, 1, 0));

        // Set up the NoCache response to find in the cache.
        inner.lock().result = Some(AuditCacheResult::NoCache);

        // Look for item, it is uncacheable, so no insert.
        assert_eq!(
            a.find_value(&item.key).await.expect("Failed find").expect("Should find item"),
            item.value
        );
        assert_eq!(inner.lock().stats(), (1, 0, 1, 0));
    }

    #[fuchsia::test]
    async fn test_uncacheable_key_skips_invalidation() {
        let item = Item::new(TestKey(1..1), 1);
        let cache = Box::new(AuditCache::new());
        cache.inner.lock().is_cacheable = false;
        let inner = cache.inner.clone();
        let a = LSMTree::new(emit_left_merge_fn, Some(cache));

        // Insert of an uncacheable key should not invalidate the cache.
        a.insert(item.clone()).expect("insert failed");
        assert_eq!(inner.lock().stats(), (0, 0, 0, 0));

        // replace_or_insert of an uncacheable key should not invalidate the cache.
        a.replace_or_insert(item.clone());
        assert_eq!(inner.lock().stats(), (0, 0, 0, 0));

        // merge_into of an uncacheable key should not invalidate the cache.
        a.merge_into(item.clone(), &TestKey(1..1));
        assert_eq!(inner.lock().stats(), (0, 0, 0, 0));
    }

    struct FailLayer {
        drop_event: Mutex<Option<Arc<DropEvent>>>,
    }

    impl FailLayer {
        fn new() -> Self {
            Self { drop_event: Mutex::new(Some(Arc::new(DropEvent::new()))) }
        }
    }

    #[async_trait]
    impl<K: Key, V: Value> Layer<K, V> for FailLayer {
        async fn seek(
            &self,
            _bound: std::ops::Bound<&K>,
        ) -> Result<BoxedLayerIterator<'_, K, V>, Error> {
            Err(anyhow!("Purposely failed seek"))
        }

        fn lock(&self) -> Option<Arc<DropEvent>> {
            self.drop_event.lock().clone()
        }

        fn len(&self) -> usize {
            0
        }

        async fn close(&self) {
            let listener = match std::mem::replace(&mut (*self.drop_event.lock()), None) {
                Some(drop_event) => drop_event.listen(),
                None => return,
            };
            listener.await;
        }

        fn get_version(&self) -> Version {
            LATEST_VERSION
        }

        async fn key_exists(&self, _key: &K) -> Result<Existence, Error> {
            unimplemented!();
        }
    }

    struct MockLayer {
        exists_result: Existence,
        drop_event: Mutex<Option<Arc<DropEvent>>>,
        cleared: std::sync::atomic::AtomicBool,
    }

    impl MockLayer {
        fn new(exists_result: Existence) -> Self {
            Self {
                exists_result,
                drop_event: Mutex::new(Some(Arc::new(DropEvent::new()))),
                cleared: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl<K: Key, V: Value> Layer<K, V> for MockLayer {
        async fn seek(
            &self,
            _bound: std::ops::Bound<&K>,
        ) -> Result<BoxedLayerIterator<'_, K, V>, Error> {
            unimplemented!()
        }

        fn clear_cached_data(&self) {
            self.cleared.store(true, Ordering::Relaxed);
        }

        fn lock(&self) -> Option<Arc<DropEvent>> {
            self.drop_event.lock().clone()
        }

        fn len(&self) -> usize {
            0
        }

        async fn close(&self) {
            let listener = match std::mem::replace(&mut (*self.drop_event.lock()), None) {
                Some(drop_event) => drop_event.listen(),
                None => return,
            };
            listener.await;
        }

        fn get_version(&self) -> Version {
            LATEST_VERSION
        }

        async fn key_exists(&self, _key: &K) -> Result<Existence, Error> {
            Ok(self.exists_result)
        }
    }

    #[fuchsia::test]
    async fn test_clear_cache() {
        let item = Item::new(TestKey(1..1), 1);
        let cache = Box::new(AuditCache::new());
        let inner = cache.inner.clone();
        let tree = LSMTree::new(emit_left_merge_fn, Some(cache));

        let layer1 = Arc::new(MockLayer::new(Existence::MaybeExists));
        let layer2 = Arc::new(MockLayer::new(Existence::MaybeExists));
        tree.set_layers(vec![
            layer1.clone() as Arc<dyn Layer<TestKey, u64>>,
            layer2.clone() as Arc<dyn Layer<TestKey, u64>>,
        ]);

        inner.lock().result = Some(AuditCacheResult::Value(item.value));
        assert_eq!(tree.find_value(&item.key).await.expect("find_value failed"), Some(item.value));
        assert!(!layer1.cleared.load(Ordering::Relaxed));
        assert!(!layer2.cleared.load(Ordering::Relaxed));

        tree.clear_cache();

        assert!(inner.lock().result.is_none());
        assert!(layer1.cleared.load(Ordering::Relaxed));
        assert!(layer2.cleared.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_layer_set_key_exists() {
        use super::LockedLayer;

        let tree = LSMTree::new(emit_left_merge_fn, None);
        let mut layer_set = tree.empty_layer_set();

        // Empty layer set should return Missing.
        assert_eq!(
            layer_set.key_exists(&TestKey(0..1)).await.expect("key_exists failed"),
            Existence::Missing
        );

        // Add a layer that returns Missing.
        layer_set.layers.push(LockedLayer::from(
            Arc::new(MockLayer::new(Existence::Missing)) as Arc<dyn Layer<TestKey, u64>>
        ));
        assert_eq!(
            layer_set.key_exists(&TestKey(0..1)).await.expect("key_exists failed"),
            Existence::Missing
        );

        // Add a layer that returns MaybeExists.
        layer_set.layers.push(LockedLayer::from(
            Arc::new(MockLayer::new(Existence::MaybeExists)) as Arc<dyn Layer<TestKey, u64>>
        ));
        assert_eq!(
            layer_set.key_exists(&TestKey(0..1)).await.expect("key_exists failed"),
            Existence::MaybeExists
        );

        // Add a layer that returns Exists.
        layer_set.layers.insert(
            0,
            LockedLayer::from(
                Arc::new(MockLayer::new(Existence::Exists)) as Arc<dyn Layer<TestKey, u64>>
            ),
        );
        assert_eq!(
            layer_set.key_exists(&TestKey(0..1)).await.expect("key_exists failed"),
            Existence::Exists
        );
    }

    #[fuchsia::test]
    async fn test_failed_lookup() {
        let cache = Box::new(AuditCache::new());
        let inner = cache.inner.clone();
        let a = LSMTree::new(emit_left_merge_fn, Some(cache));
        a.set_layers(vec![Arc::new(FailLayer::new())]);

        // Zero counters.
        assert_eq!(inner.lock().stats(), (0, 0, 0, 0));

        // Lookup should fail and drop the placeholder.
        assert!(a.find(&TestKey(1..1)).await.is_err());
        assert_eq!(inner.lock().stats(), (1, 0, 0, 1));
    }
}

#[cfg(fuzz)]
mod fuzz {
    use crate::lsm_tree::types::{Item, Value};
    use crate::serialized_types::{
        LATEST_VERSION, Version, Versioned, VersionedLatest, versioned_type,
    };
    use arbitrary::Arbitrary;

    use fuzz::fuzz;

    use super::testing::TestKey;

    impl Versioned for u64 {}
    versioned_type! { 1.. => u64 }

    impl Value for u64 {
        const DELETED_MARKER: Self = 0;
    }

    // Note: This code isn't really dead. it's used below in
    // `fuzz_lsm_tree_action`. However, the `#[fuzz]` proc macro attribute
    // obfuscates the usage enough to confuse the compiler.
    #[allow(dead_code)]
    #[derive(Arbitrary)]
    enum FuzzAction {
        Insert(Item<TestKey, u64>),
        ReplaceOrInsert(Item<TestKey, u64>),
        MergeInto(Item<TestKey, u64>, TestKey),
        Find(TestKey),
        Seal,
    }

    #[fuzz]
    fn fuzz_lsm_tree_actions(actions: Vec<FuzzAction>) {
        use super::LSMTree;
        use crate::lsm_tree::merge::{MergeLayerIterator, MergeResult};
        use futures::executor::block_on;

        fn emit_left_merge_fn(
            _left: &MergeLayerIterator<'_, TestKey, u64>,
            _right: &MergeLayerIterator<'_, TestKey, u64>,
        ) -> MergeResult<TestKey, u64> {
            MergeResult::EmitLeft
        }

        let tree = LSMTree::new(emit_left_merge_fn, None);
        for action in actions {
            match action {
                FuzzAction::Insert(item) => {
                    let _ = tree.insert(item);
                }
                FuzzAction::ReplaceOrInsert(item) => {
                    tree.replace_or_insert(item);
                }
                FuzzAction::Find(key) => {
                    block_on(tree.find(&key)).expect("find failed");
                }
                FuzzAction::MergeInto(item, bound) => tree.merge_into(item, &bound),
                FuzzAction::Seal => tree.seal(),
            };
        }
    }
}
