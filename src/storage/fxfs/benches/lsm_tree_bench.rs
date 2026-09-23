// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::Criterion;
use futures::executor::block_on;

use fxfs::lsm_tree::merge::{MergeLayerIterator, MergeResult};
use fxfs::lsm_tree::types::{Item, LayerIterator, LayerKey, MergeableKey, Value};
use fxfs::lsm_tree::{LSMTree, Query, compact_with_iterator, layers_from_handles};
use fxfs::object_handle::ObjectHandle;
use fxfs::object_store::Extent;
use fxfs::object_store::allocator::{AllocatorKey, AllocatorValue};
use fxfs::object_store::journal::CompactionYielder;
use fxfs::object_store::object_record::{
    AttributeId, AttributeKey, ObjectKey, ObjectKeyData, ObjectValue,
};
use fxfs::testing::fake_object::{FakeObject, FakeObjectHandle};
use fxfs::testing::writer::Writer;
use std::sync::Arc;

/// Extent size in bytes used for generating and querying extent-based records.
const EXTENT_SIZE: u64 = 1024;

/// Number of extents to scan in range query benchmarks.
const RANGE_QUERY_EXTENTS: u64 = 10;

/// Merge function for LSM tree layers that resolves key collisions by emitting the left item.
fn emit_left_merge_fn<K: MergeableKey, V: Value>(
    _left: &MergeLayerIterator<'_, K, V>,
    _right: &MergeLayerIterator<'_, K, V>,
) -> MergeResult<K, V> {
    MergeResult::EmitLeft
}

/// Helper to construct a sealed LSM tree with `depth` persistent layers and `size` total items.
fn create_tree_generic<K, V, F>(depth: u64, size: u64, mut populate_layer: F) -> LSMTree<K, V>
where
    K: MergeableKey,
    V: Value,
    F: FnMut(&LSMTree<K, V>, u64, u64),
{
    let items_per_layer = size / depth;
    let mut handles = Vec::new();

    for layer_idx in 0..depth {
        let layer_tree = LSMTree::new(emit_left_merge_fn, None);
        populate_layer(&layer_tree, layer_idx, items_per_layer);
        layer_tree.seal();

        let object = Arc::new(FakeObject::new());
        let handle = FakeObjectHandle::new(object.clone());

        block_on(async {
            let layer_set = layer_tree.layer_set();
            let mut merger = layer_set.merger();
            let iter = merger.query(Query::FullScan).await.unwrap();
            compact_with_iterator(
                iter,
                items_per_layer as usize,
                Writer::new(&handle).await,
                handle.block_size(),
                None::<CompactionYielder<'static>>,
            )
            .await
            .unwrap();
        });

        handles.push(FakeObjectHandle::new(object));
    }

    let layers = block_on(async { layers_from_handles(handles).await.unwrap() });

    let tree = LSMTree::new(emit_left_merge_fn, None);
    tree.set_layers(layers);
    tree
}

/// Populates an LSM tree with standard object records (`ObjectKey::object(key_id)`).
fn create_tree(depth: u64, size: u64) -> LSMTree<ObjectKey, ObjectValue> {
    create_tree_generic(depth, size, |tree, layer_idx, items_per_layer| {
        for i in 0..items_per_layer {
            let key_id = i * depth + layer_idx;
            tree.insert(Item::new(ObjectKey::object(key_id), ObjectValue::Some)).unwrap();
        }
    })
}

/// Populates an LSM tree with long child keys to benchmark performance on large keys.
fn create_long_tree(depth: u64, size: u64) -> LSMTree<ObjectKey, ObjectValue> {
    create_tree_generic(depth, size, |tree, layer_idx, items_per_layer| {
        for i in 0..items_per_layer {
            let key_id = i * depth + layer_idx;
            let name = "a".repeat(300);
            let key = ObjectKey { object_id: key_id, data: ObjectKeyData::Child { name } };
            tree.insert(Item::new(key, ObjectValue::Some)).unwrap();
        }
    })
}

/// Populates an LSM tree with attribute extent records interleaved across layers.
fn create_extent_tree(depth: u64, size: u64) -> LSMTree<ObjectKey, ObjectValue> {
    create_tree_generic(depth, size, |tree, layer_idx, items_per_layer| {
        let mut offset = layer_idx * EXTENT_SIZE;
        for _ in 0..items_per_layer {
            let key = ObjectKey {
                object_id: 1,
                data: ObjectKeyData::Attribute(
                    AttributeId::DATA,
                    AttributeKey::Extent(Extent(offset..offset + EXTENT_SIZE)),
                ),
            };
            tree.insert(Item::new(key, ObjectValue::Some)).unwrap();
            offset += depth * EXTENT_SIZE;
        }
    })
}

/// Populates an LSM tree with allocator extent records interleaved across layers.
fn create_allocator_tree(depth: u64, size: u64) -> LSMTree<AllocatorKey, AllocatorValue> {
    create_tree_generic(depth, size, |tree, layer_idx, items_per_layer| {
        let mut offset = layer_idx * EXTENT_SIZE;
        for _ in 0..items_per_layer {
            let key = AllocatorKey { device_range: Extent(offset..offset + EXTENT_SIZE) };
            let value = AllocatorValue::Abs { count: 1, owner_object_id: 1 };
            tree.insert(Item::new(key, value)).unwrap();
            offset += depth * EXTENT_SIZE;
        }
    })
}

/// Advances an iterator until all items have been consumed.
async fn drain_iterator<K: LayerKey, V: Value>(mut iter: impl LayerIterator<K, V>) {
    while iter.get().is_some() {
        iter.advance().await.unwrap();
    }
}

/// Registers and runs the full suite of LSM tree benchmarks across depths and sizes.
fn bench_lsm_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("fuchsia.fxfs.lsm_tree");

    // We fix the tree size to 10,000 items (2,500 items/layer at depth 4, spanning ~125 blocks
    // of 512 bytes). This is large enough to exercise multi-block seek tables and layer indexing
    // without making full scans slow.
    //
    // The benchmarked dimension is `depth` ([1, 4]), which tests the LSM merger's performance
    // when searching and merging across multiple layers vs. a single layer.
    const ITEMS: u64 = 10_000;
    for depth in [1, 4] {
        // --- Object Record Benchmarks ---
        let tree = Arc::new(create_tree(depth, ITEMS));

        let tree_hit = tree.clone();
        group.bench_function(&format!("find_hit_depth_{}", depth), move |b| {
            let key = ObjectKey::object(ITEMS / 2);
            b.iter(|| {
                block_on(async {
                    let _ = tree_hit.find(&key).await.unwrap();
                });
            })
        });

        let tree_scan = tree.clone();
        group.bench_function(&format!("full_scan_depth_{}", depth), move |b| {
            let layer_set = tree_scan.layer_set();
            let mut merger = layer_set.merger();
            b.iter(|| {
                block_on(async {
                    drain_iterator(merger.query(Query::FullScan).await.unwrap()).await;
                });
            })
        });

        // --- Long Key Benchmarks ---
        let long_tree = Arc::new(create_long_tree(depth, ITEMS));

        let tree_long_hit = long_tree.clone();
        group.bench_function(&format!("find_long_hit_depth_{}", depth), move |b| {
            let key = ObjectKey {
                object_id: ITEMS / 2,
                data: ObjectKeyData::Child { name: "a".repeat(300) },
            };
            b.iter(|| {
                block_on(async {
                    let _ = tree_long_hit.find(&key).await.unwrap();
                });
            })
        });

        let tree_long_scan = long_tree.clone();
        group.bench_function(&format!("full_long_scan_depth_{}", depth), move |b| {
            let long_layer_set = tree_long_scan.layer_set();
            let mut merger_long_scan = long_layer_set.merger();
            b.iter(|| {
                block_on(async {
                    drain_iterator(merger_long_scan.query(Query::FullScan).await.unwrap()).await;
                });
            })
        });

        // --- Extent Record Benchmarks ---
        let extent_tree = Arc::new(create_extent_tree(depth, ITEMS));

        let tree_extent_scan = extent_tree.clone();
        group.bench_function(&format!("extent_scan_depth_{}", depth), move |b| {
            let extent_layer_set = tree_extent_scan.layer_set();
            let mut merger_extent_scan = extent_layer_set.merger();
            b.iter(|| {
                block_on(async {
                    drain_iterator(merger_extent_scan.query(Query::FullScan).await.unwrap()).await;
                });
            })
        });

        let tree_extent_range = extent_tree.clone();
        group.bench_function(&format!("extent_range_depth_{}", depth), move |b| {
            let extent_layer_set_range = tree_extent_range.layer_set();
            let mut merger_extent_range = extent_layer_set_range.merger();
            let query_start = (ITEMS / 2) * EXTENT_SIZE;
            let query_end = query_start + RANGE_QUERY_EXTENTS * EXTENT_SIZE;
            let key = ObjectKey {
                object_id: 1,
                data: ObjectKeyData::Attribute(
                    AttributeId::DATA,
                    AttributeKey::Extent(Extent(query_start..query_start + EXTENT_SIZE)),
                ),
            };
            b.iter(|| {
                block_on(async {
                    let mut iter =
                        merger_extent_range.query(Query::LimitedRange(&key)).await.unwrap();
                    while let Some(item) = iter.get() {
                        if let ObjectKeyData::Attribute(_, AttributeKey::Extent(extent_key)) =
                            &item.key.data
                        {
                            if extent_key.start >= query_end {
                                break;
                            }
                        }
                        iter.advance().await.unwrap();
                    }
                });
            })
        });

        // --- Allocator Record Benchmarks ---
        let allocator_tree = Arc::new(create_allocator_tree(depth, ITEMS));

        let tree_allocator_scan = allocator_tree.clone();
        group.bench_function(&format!("allocator_scan_depth_{}", depth), move |b| {
            let allocator_layer_set = tree_allocator_scan.layer_set();
            let mut merger_allocator_scan = allocator_layer_set.merger();
            b.iter(|| {
                block_on(async {
                    drain_iterator(merger_allocator_scan.query(Query::FullScan).await.unwrap())
                        .await;
                });
            })
        });

        let tree_allocator_range = allocator_tree.clone();
        group.bench_function(&format!("allocator_range_depth_{}", depth), move |b| {
            let allocator_layer_set_range = tree_allocator_range.layer_set();
            let mut merger_allocator_range = allocator_layer_set_range.merger();
            let query_start = (ITEMS / 2) * EXTENT_SIZE;
            let query_end = query_start + RANGE_QUERY_EXTENTS * EXTENT_SIZE;
            let key = AllocatorKey { device_range: Extent(query_start..query_start + EXTENT_SIZE) };
            let search_key = key.search_key().unwrap();
            b.iter(|| {
                block_on(async {
                    let mut iter =
                        merger_allocator_range.query(Query::FullRange(&search_key)).await.unwrap();
                    while let Some(item) = iter.get() {
                        if item.key.device_range.start >= query_end {
                            break;
                        }
                        iter.advance().await.unwrap();
                    }
                });
            })
        });
    }

    group.finish();
}

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(std::time::Duration::from_millis(1))
        .measurement_time(std::time::Duration::from_millis(100))
        .sample_size(10);
    bench_lsm_tree(&mut c);
}
