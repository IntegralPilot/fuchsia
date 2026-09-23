// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::lsm_tree::types::{LayerIterator, MergeableKey, Value};
use crate::lsm_tree::{LSMTree, LockedLayer, Query, Yielder, compact_with_iterator};
use crate::object_handle::WriteBytes;
use crate::object_store::journal;
use anyhow::{Context, Error};
use std::future::{Future, ready};

pub trait MajorCompactable<K: 'static, V: 'static> {
    /// Returns an iterator that wraps another iterator that is appropriate for major compactions.
    /// Such an iterator should elide items that don't need to be written if a major compaction is
    /// taking place.
    fn major_iter(
        iter: impl LayerIterator<K, V>,
    ) -> impl Future<Output = Result<impl LayerIterator<K, V>, Error>> + Send {
        ready(Ok(iter))
    }
}

type Layers<K, V> = Vec<LockedLayer<K, V>>;

/// Picks an appropriate set of layers to flush from the tree and writes them to writer.  After
/// successfully writing a new layer, it returns the layers that should be kept and the old layers
/// that should be purged.  `yielder` will be used to periodically yield.
pub async fn flush<'a, K: MergeableKey, V: Value>(
    tree: &'a LSMTree<K, V>,
    writer: impl WriteBytes + Send,
    yielder: Option<impl Yielder>,
    full_compaction: bool,
) -> Result<(Layers<K, V>, Layers<K, V>), Error>
where
    LSMTree<K, V>: MajorCompactable<K, V>,
{
    let mut layer_set = tree.immutable_layer_set();
    let mut total_size = 0;
    let mut layer_count = 0;
    let mut split_index = if full_compaction {
        layer_set.layers.len()
    } else {
        layer_set
            .layers
            .iter()
            .position(|layer| {
                match layer.handle() {
                    None => {}
                    Some(handle) => {
                        let size = handle.get_size();
                        // Stop adding more layers when the total size of all the immutable layers
                        // so far is less than 3/4 of the layer we are looking at.
                        if total_size > 0 && total_size * 4 < size * 3 {
                            return true;
                        }
                        total_size += size;
                        layer_count += 1;
                    }
                }
                false
            })
            .unwrap_or(layer_set.layers.len())
    };

    // If there's only one immutable layer to merge with and it's big, don't merge with it.
    // Instead, we'll create a new layer and eventually that will grow to be big enough to merge
    // with the existing layers.  The threshold here cannot be so big such that merging with a
    // layer that big ends up using so much journal space that we have to immediately flush
    // again as that has the potential to end up in an infinite loop, and so the number chosen
    // here is related to the journal's RECLAIM_SIZE.
    if layer_count == 1
        && total_size > journal::DEFAULT_RECLAIM_SIZE
        && layer_set.layers[split_index - 1].handle().is_some()
    {
        split_index -= 1;
    }

    let layers_to_keep = layer_set.layers.split_off(split_index);

    {
        let start_time = std::time::Instant::now();
        let merged_layer_count = layer_set.layers.len();
        let block_size = writer.block_size();
        let total_len = layer_set.sum_len();
        let mut merger = layer_set.merger();
        let iter = merger.query(Query::FullScan).await?;
        let bytes_written = if layers_to_keep.is_empty() {
            let major_iter = LSMTree::<K, V>::major_iter(iter).await?;
            compact_with_iterator(major_iter, total_len, writer, block_size, yielder).await
        } else {
            compact_with_iterator(iter, total_len, writer, block_size, yielder).await
        }
        .context("ObjectStore::flush")?;

        tree.report_compaction_metrics(bytes_written, start_time.elapsed(), merged_layer_count);
    }

    Ok((layers_to_keep, layer_set.layers))
}

/// Returns the amount of space that should be reserved to allow for compactions when the size of
/// all layers is `layer_size`.
pub fn reservation_amount_from_layer_size(layer_size: u64) -> u64 {
    // Conservatively allow for 50% overheads (which covers the amount of metadata space required
    // for compactions).
    layer_size * 3 / 2
}
