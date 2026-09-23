// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::log::*;
use crate::lsm_tree;
use crate::lsm_tree::types::{
    BoxedItem, Item, ItemRef, Key, Layer, LayerIterator, LayerIteratorMut, LayerKey,
    MaybeContainsKey, MergeType, OrdLowerBound, Value,
};
use anyhow::Error;
use futures::future::BoxFuture;
use futures::try_join;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt::{Debug, Write};
use std::ops::{Bound, Deref, DerefMut};
use std::sync::{Arc, atomic};

#[derive(Debug, Eq, PartialEq)]
pub enum ItemOp<K, V> {
    /// Keeps the item to be presented to the merger subsequently with a new merge pair.
    Keep,

    /// Discards the item and moves on to the next item in the respective layer.
    Discard,

    /// Replaces the item with something new which will be presented to the merger subsequently with
    /// a new pair.
    Replace(BoxedItem<K, V>),
}

#[derive(Debug, Eq, PartialEq)]
pub enum MergeResult<K, V> {
    /// Emits the left item unchanged. Keeps the right item. This is the common case. Once an item
    /// has been emitted, it will never be seen again by the merge function.
    EmitLeft,

    /// All other merge results are covered by the following. Take care when replacing items
    /// that you replace the correct item. The merger will never merge two items together from
    /// the same layer. Consider the following scenario:
    ///
    ///        +-----------+              +-----------+
    /// 0:     |    A      |              |    C      |
    ///        +-----------+--------------+-----------+
    /// 1:                 |      B       |
    ///                    +--------------+
    ///
    /// Let's say that all three items can be merged together. The merge function will first be
    /// presented with items A and B, at which point it has the option of replacing the left item
    /// (i.e. A, in layer 0) or the right item (i.e. B in layer 1). However, if you replace the left
    /// item, the merge function will not then be given the opportunity to merge it with C, so the
    /// correct thing to do in this case is to replace the right item B in layer 1, and discard the
    /// left item. A rule you can use is that you should avoid replacing an item with another item
    /// whose upper bound exceeds that of the item you are replacing.
    ///
    /// There are some combinations that might lead to infinite loops (e.g. None, Keep, Keep) and
    /// should obviously be avoided.
    Other { emit: Option<BoxedItem<K, V>>, left: ItemOp<K, V>, right: ItemOp<K, V> },
}

/// Users must provide a merge function which will take pairs of items, left and right, and return a
/// merge result. The left item's key will either be less than the right item's key, or if they are
/// the same, then the left item will be in a lower layer index (lower layer indexes indicate more
/// recent entries). The last remaining item is always emitted.
pub type MergeFn<K, V> =
    fn(&MergeLayerIterator<'_, K, V>, &MergeLayerIterator<'_, K, V>) -> MergeResult<K, V>;

pub enum MergeItem<K, V> {
    None,
    Item(BoxedItem<K, V>),
    Iter,
}

enum RawIterator<'a, K, V> {
    None,
    Const(Box<dyn LayerIterator<K, V> + 'a>),
    Mut(Box<dyn LayerIteratorMut<K, V> + 'a>),
}

// SAFETY: LayerIteratorMut is not Sync or Send, but it's only used by merge_into, which doesn't
// send across threads (and is sync, not async).
unsafe impl<K, V> Send for RawIterator<'_, K, V> {}

// An iterator that keeps track of where we are for each of the layers. We push these onto a
// min-heap.
pub struct MergeLayerIterator<'a, K, V> {
    layer: Option<&'a dyn Layer<K, V>>,

    // The underlying iterator.
    iter: RawIterator<'a, K, V>,

    // The index of the layer this is for.
    pub layer_index: u16,

    // The item we are currently pointing at.
    item: MergeItem<K, V>,
}

impl<'a, K, V> MergeLayerIterator<'a, K, V> {
    pub fn key(&self) -> &K {
        self.item().key
    }

    pub fn value(&self) -> &V {
        self.item().value
    }

    fn new(layer_index: u16, layer: &'a dyn Layer<K, V>) -> Self {
        MergeLayerIterator {
            layer: Some(layer),
            iter: RawIterator::None,
            layer_index,
            item: MergeItem::None,
        }
    }

    fn new_with_item(layer_index: u16, item: MergeItem<K, V>) -> Self {
        MergeLayerIterator { layer: None, iter: RawIterator::None, layer_index, item }
    }

    fn item(&self) -> ItemRef<'_, K, V> {
        match &self.item {
            MergeItem::None => panic!("No item!"),
            MergeItem::Item(item) => item.as_item_ref(),
            MergeItem::Iter => self.get().unwrap(),
        }
    }

    fn get(&self) -> Option<ItemRef<'_, K, V>> {
        match &self.iter {
            RawIterator::None => panic!("No iterator!"),
            RawIterator::Const(iter) => iter.get(),
            RawIterator::Mut(iter) => iter.get(),
        }
    }

    fn set_item_from_iter(&mut self) {
        self.item = if match &self.iter {
            RawIterator::None => unreachable!(),
            RawIterator::Const(iter) => iter.get(),
            RawIterator::Mut(iter) => iter.get(),
        }
        .is_some()
        {
            MergeItem::Iter
        } else {
            MergeItem::None
        };
    }

    fn take_item(&mut self) -> Option<BoxedItem<K, V>> {
        if matches!(self.item, MergeItem::Item(_)) {
            if let MergeItem::Item(item) = std::mem::replace(&mut self.item, MergeItem::None) {
                return Some(item);
            }
        }
        None
    }

    async fn advance(&mut self) -> Result<(), Error> {
        if let MergeItem::Iter = self.item {
            if let RawIterator::Const(iter) = &mut self.iter {
                iter.advance().await?;
            } else {
                // This will never get called in the RawIterator::Mut case.
                unreachable!();
            }
        }
        self.set_item_from_iter();
        Ok(())
    }

    fn replace(&mut self, item: BoxedItem<K, V>) {
        self.item = MergeItem::Item(item);
    }

    fn is_some(&self) -> bool {
        !matches!(self.item, MergeItem::None)
    }

    // This function exists so that we can advance multiple iterators concurrently using, say,
    // try_join!.
    async fn maybe_advance(&mut self, op: &ItemOp<K, V>) -> Result<(), Error> {
        if let ItemOp::Keep = op { Ok(()) } else { self.advance().await }
    }
}

// -- Ord and friends --
impl<K: OrdLowerBound, V> Ord for MergeLayerIterator<'_, K, V> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering because we want min-heap not max-heap.
        other.key().cmp_lower_bound(self.key()).then(other.layer_index.cmp(&self.layer_index))
    }
}
impl<K: OrdLowerBound, V> PartialOrd for MergeLayerIterator<'_, K, V> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<K: OrdLowerBound, V> PartialEq for MergeLayerIterator<'_, K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl<K: OrdLowerBound, V> Eq for MergeLayerIterator<'_, K, V> {}

// As we merge items, the current item can be an item that has been replaced (and later emitted) by
// the merge function, or an item referenced by an iterator, or nothing.
enum CurrentItem<'a, 'b, K, V> {
    None,
    Item(BoxedItem<K, V>),
    Iterator(&'a mut MergeLayerIterator<'b, K, V>),
}

impl<'a, 'b, K, V> CurrentItem<'a, 'b, K, V> {
    fn take_iterator(&mut self) -> Option<&'a mut MergeLayerIterator<'b, K, V>> {
        if matches!(self, CurrentItem::Iterator(_)) {
            if let CurrentItem::Iterator(iter) = std::mem::replace(self, CurrentItem::None) {
                return Some(iter);
            }
        }
        None
    }
}

impl<'a, K, V> From<&'a CurrentItem<'_, '_, K, V>> for Option<ItemRef<'a, K, V>> {
    fn from(iter: &'a CurrentItem<'_, '_, K, V>) -> Option<ItemRef<'a, K, V>> {
        match iter {
            CurrentItem::None => None,
            CurrentItem::Iterator(iterator) => Some(iterator.item()),
            CurrentItem::Item(item) => Some(item.as_item_ref()),
        }
    }
}

/// Merger is the main entry point to merging.
pub struct Merger<'a, K, V> {
    // A buffer containing all the MergeLayerIterator objects.
    iterators: Vec<MergeLayerIterator<'a, K, V>>,

    // The function to be used for merging items.
    merge_fn: MergeFn<K, V>,

    // If true, additional logging is enabled.
    trace: bool,

    // Tracks statistics related to the LSM tree.
    counters: Arc<lsm_tree::TreeCounters>,
}

/// Query describes the goal of a search in the LSM tree.  The caller specifies this to guide the
/// merger on which layer files it must consult.
/// Layers might be skipped during a query for a few reasons:
///   * Existence filters might allow layer files to be skipped.
///   * For bounded queries (i.e. any except FullScan), if the search key has
///     MergeType::OptimizedMerge, we can use that to omit older layers once we find a match.
#[derive(Debug, Clone)]
pub enum Query<'a, K: Key + LayerKey + OrdLowerBound> {
    /// Point queries look for a specific key in the LSM tree.  In this case, the existence filters
    /// for each layer file can be used to decide if the layer file needs to be consulted.
    /// Note that it is an error to use `Point` for range-like keys.  Either `LimitedRange` or
    /// `FullRange` must be used instead.
    Point(&'a K),

    /// LimitedRange queries allow for iteration over a range of keys.  The returned iterator will
    /// not necessarily terminate at the end of the range; the caller should stop iterating when the
    /// iterator passes the end of the range: the records returned cannot be relied upon as
    /// accurate.  For a LimitedRange query, the existence filters for each layer file can be used,
    /// but we have to check for all possible keys in the range we wish to search.  Obviously, that
    /// means that the range should be, well, limited.  Fuzzy hashes permit this for extent-like
    /// keys, but these queries are not the right choice for things like searching a directory.
    LimitedRange(&'a K),

    /// FullRange queries position the iterator to a starting key, and scans forward to the first
    /// key of a different type.  In this case, the existence filters are not used.  The key should
    /// be a search key (see `LayerKey::search_key`) (which, for extent based keys, would normally
    /// be `start..start + 1`).  This kind of query will still use optimized merges if the keys
    /// found support it.
    FullRange(&'a K),

    /// FullScan queries are intended to yield every record in the tree.  In this case, the
    /// existence filters are not used.
    FullScan,
}

impl<'a, K: Key + LayerKey + OrdLowerBound> Query<'a, K> {
    fn check_layer<V>(&self, layer: &dyn Layer<K, V>) -> MaybeContainsKey {
        match self {
            Self::Point(key) => layer.maybe_contains_key(key),
            Self::LimitedRange(key) => layer.maybe_contains_key(key),
            Self::FullRange(_) => MaybeContainsKey::Maybe,
            Self::FullScan => MaybeContainsKey::Maybe,
        }
    }
}

#[fxfs_trace::trace]
impl<'a, K: Key + LayerKey + OrdLowerBound, V: Value> Merger<'a, K, V> {
    pub(super) fn new<I: Iterator<Item = &'a dyn Layer<K, V>>>(
        layers: I,
        merge_fn: MergeFn<K, V>,
        counters: Arc<lsm_tree::TreeCounters>,
    ) -> Merger<'a, K, V> {
        Merger {
            iterators: layers
                .enumerate()
                .map(|(index, layer)| MergeLayerIterator::new(index as u16, layer))
                .collect(),
            merge_fn: merge_fn,
            trace: false,
            counters,
        }
    }

    /// Executes `query`, positioning the iterator to the first matching item.  See `Query` for
    /// details.
    #[trace]
    pub async fn query(
        &mut self,
        query: Query<'_, K>,
    ) -> Result<MergerIterator<'_, 'a, K, V>, Error> {
        if let Query::Point(key) = query {
            // NB: It is almost certainly an error to provide a range-like key to a Point query,
            // hence this debug assertion.  This will most likely result in the wrong result, since
            // the starting bound would be the original range key and therefore we might miss
            // records that start earlier but overlap with the key.
            // For example, if there is a layer which contains 0..100 and 100..200, and we search
            // for 50..150, we should get both extents, which requires a different starting bound
            // than 50..150 (particularly it would require the `K::search_key`).
            debug_assert!(!key.is_range_key())
        };
        let len = self.iterators.len();
        let mut excessive_partitions = false;
        let pending_iterators = {
            fxfs_trace::duration!("Merger::filter_layer_files", "len" => len);
            self.iterators
                .iter_mut()
                .rev()
                .filter(|l| match query.check_layer(l.layer.unwrap()) {
                    MaybeContainsKey::False => false,
                    MaybeContainsKey::Maybe => true,
                    MaybeContainsKey::RangeKeyTooLarge => {
                        excessive_partitions = true;
                        true
                    }
                })
                .collect::<Vec<&mut MergeLayerIterator<'a, K, V>>>()
        };
        let layer_count = pending_iterators.len();
        {
            self.counters.num_seeks.fetch_add(1, atomic::Ordering::Relaxed);
            self.counters.layer_files_total.fetch_add(len, atomic::Ordering::Relaxed);
            self.counters
                .layer_files_skipped
                .fetch_add(len - layer_count, atomic::Ordering::Relaxed);
            if excessive_partitions {
                self.counters.excessive_hash_partitions.fetch_add(1, atomic::Ordering::Relaxed);
            }
        }
        log::debug!(query:?; "Consulting {}/{} layers", layer_count, len);
        let mut merger_iter = MergerIterator {
            merge_fn: self.merge_fn,
            pending_iterators,
            heap: BinaryHeap::with_capacity(layer_count),
            item: CurrentItem::None,
            trace: self.trace,
            history: String::new(),
        };
        let owned_key;
        let search_key = match query {
            Query::Point(key) => Bound::Included(key),
            Query::LimitedRange(key) => match key.search_key() {
                Some(k) => {
                    owned_key = k;
                    Bound::Included(&owned_key)
                }
                None => Bound::Included(key),
            },
            Query::FullRange(key) => {
                assert!(key.is_search_key());
                Bound::Included(key)
            }
            Query::FullScan => Bound::Unbounded,
        };
        merger_iter.seek(search_key).await?;
        Ok(merger_iter)
    }

    pub fn set_trace(&mut self, v: bool) {
        self.trace = v;
    }
}

/// This is an iterator that will allow iteration over merged layers.  The primary interface is via
/// the LayerIterator trait.
pub struct MergerIterator<'a, 'b, K, V> {
    merge_fn: MergeFn<K, V>,

    // Iterators that we have not yet pushed onto the heap.
    pending_iterators: Vec<&'a mut MergeLayerIterator<'b, K, V>>,

    // A heap with the merge iterators.
    heap: BinaryHeap<&'a mut MergeLayerIterator<'b, K, V>>,

    // The current item.
    item: CurrentItem<'a, 'b, K, V>,

    // If true, logs regarding merger behaviour are appended to history.
    trace: bool,

    // Holds trace information if trace is true.
    history: String,
}

impl<'a, 'b, K: Key + LayerKey + OrdLowerBound, V: Value> MergerIterator<'a, 'b, K, V> {
    pub fn pending_iterators_len(&self) -> usize {
        self.pending_iterators.len()
    }

    /// Positions the iterator using `search_key`.
    async fn seek(&mut self, search_key: Bound<&K>) -> Result<(), Error> {
        self.push_iterators(search_key).await?;
        self.advance_impl(search_key).await
    }

    // Merges items from an array of layers using the provided merge function. The merge function is
    // repeatedly provided the lowest and the second lowest element, if one exists. In cases where
    // the two lowest elements compare equal, the element with the lowest layer (i.e. whichever
    // comes first in the layers array) will come first.  `search_key` is a bound for the next
    // key we expect.
    async fn advance_impl(&mut self, search_key: Bound<&K>) -> Result<(), Error> {
        loop {
            loop {
                if self.heap.is_empty() {
                    self.item = CurrentItem::None;
                    return Ok(());
                }
                let lowest = self.heap.pop().unwrap();
                let maybe_second_lowest = self.heap.pop();
                if let Some(second_lowest) = maybe_second_lowest {
                    let result = (self.merge_fn)(&lowest, &second_lowest);
                    if self.trace {
                        writeln!(
                            self.history,
                            "merge {:?}, {:?} -> {:?}",
                            lowest.item(),
                            second_lowest.item(),
                            result
                        )
                        .unwrap();
                    }
                    match result {
                        MergeResult::EmitLeft => {
                            self.heap.push(second_lowest);
                            self.item = CurrentItem::Iterator(lowest);
                            break;
                        }
                        MergeResult::Other { emit, left, right } => {
                            try_join!(
                                lowest.maybe_advance(&left),
                                second_lowest.maybe_advance(&right)
                            )?;
                            self.update_item(lowest, left);
                            self.update_item(second_lowest, right);
                            if let Some(emit) = emit {
                                self.item = CurrentItem::Item(emit);
                                break;
                            }
                        }
                    }
                } else {
                    self.item = CurrentItem::Iterator(lowest);
                    break;
                }
            }

            // If the item we're about to yield isn't within `search_key`, ignore it and
            // continue.  To see how this would happen, imagine the following scenario:
            //
            //       0          10          20            30
            //       +----------+
            //   0   |          |
            //       +----------+-----------+
            //   1   |                      |
            //       +----------------------+-------------+
            //   2   |                                    |
            //       +------------------------------------+
            //
            // If we are to seek for 0..1 and then iterate over what we find, we expect the sequence
            // 0..10, 10..20, 20..30.  After the first seek, we should only consult iterator 0.  For
            // the first advance, we will consult iterator 1 but we need to merge the 0..20 element
            // with the 0..10 element which we already emitted.  To make this work, we push the
            // 0..10 item back on the heap (see advance) and then merge, but that will yield the
            // 0..10 entry again, so here we need to skip over it, and then merge again, at which
            // point we should see the 10..20 entry as expected.
            match search_key {
                Bound::Included(key)
                    if self.get().unwrap().key.cmp_upper_bound(key) == Ordering::Less => {}
                Bound::Excluded(key)
                    if self.get().unwrap().key.cmp_upper_bound(key) != Ordering::Greater => {}
                _ => return Ok(()),
            }
            if let Some(iterator) = self.item.take_iterator() {
                iterator.advance().await?;
                if iterator.is_some() {
                    self.heap.push(iterator);
                }
            }
        }
    }

    // Returns whether more iterators are required for the given `search_key`.  See push_iterators.
    fn needs_more_iterators(&self, search_key: Bound<&K>) -> bool {
        if self.pending_iterators.is_empty() {
            return false;
        }
        if self.heap.is_empty() {
            return true;
        }
        let Bound::Included(target) = search_key else { return true };
        match target.merge_type() {
            MergeType::FullMerge => true,
            MergeType::OptimizedMerge => !self.heap.peek().unwrap().key().overlaps(target),
        }
    }

    // Pushes additional iterators onto the heap until we are confident that the top element will
    // yield what we are looking for.
    async fn push_iterators(&mut self, search_key: Bound<&K>) -> Result<(), Error> {
        while self.needs_more_iterators(search_key) {
            let iter = self.pending_iterators.pop().unwrap();
            let sub_iter = iter.layer.as_ref().unwrap().seek(search_key).await?;
            if self.trace {
                writeln!(
                    self.history,
                    "merger: search for {:?}, found {:?}",
                    search_key,
                    sub_iter.get()
                )
                .unwrap();
            }
            iter.iter = RawIterator::Const(sub_iter);
            iter.set_item_from_iter();
            if iter.is_some() {
                self.heap.push(iter);
            }
        }
        Ok(())
    }

    // Updates the merge iterator depending on |op|. If discarding, the iterator should have already
    // been advanced.
    fn update_item(&mut self, item: &'a mut MergeLayerIterator<'b, K, V>, op: ItemOp<K, V>) {
        match op {
            ItemOp::Keep => self.heap.push(item),
            ItemOp::Discard => {
                // The iterator should have already been advanced.
                if item.is_some() {
                    self.heap.push(item);
                }
            }
            ItemOp::Replace(replacement) => {
                item.replace(replacement);
                self.heap.push(item);
            }
        }
    }
}

impl<'a, K: Key + LayerKey + OrdLowerBound, V: Value> LayerIterator<K, V>
    for MergerIterator<'a, '_, K, V>
{
    async fn advance(&mut self) -> Result<(), Error> {
        let owned_key;
        let mut search_key = Bound::Unbounded;
        if !self.pending_iterators.is_empty() {
            if let Some(ItemRef { key, .. }) = self.get() {
                match key.next_key() {
                    Some(k) => {
                        assert!(k.is_search_key());
                        owned_key = k;
                        search_key = Bound::Included(&owned_key);
                    }
                    None => {
                        owned_key = key.clone();
                        search_key = Bound::Excluded(&owned_key);
                    }
                }
            }
        }

        // Advance the iterator for the current item and push it onto the heap, and also push any
        // additional iterators onto the heap (by calling push_iterators).
        if let Some(iterator) = self.item.take_iterator() {
            if self.needs_more_iterators(search_key) {
                let existing_item = iterator.item().boxed();
                iterator.advance().await?;
                if let Bound::Included(s) = search_key
                    && iterator.is_some()
                    && iterator.key().merge_type() == MergeType::OptimizedMerge
                    && s.overlaps(iterator.key())
                {
                    // In this case, the key immediately following is a good candidate, and all
                    // we need to do is merge it with existing iterators; we shouldn't need to
                    // consult with any more iterators.
                } else {
                    // We are going to need to consult more iterators so we need to go back to
                    // the previous item so that we can merge with it.  See the comment in
                    // advance_impl.
                    iterator.replace(existing_item);

                    // We must push other iterators here before pushing iterator onto the heap
                    // because we know `iterator` would end up at the top of the heap.
                    self.push_iterators(search_key).await?;
                }
            } else {
                iterator.advance().await?;
            }
            if iterator.is_some() {
                self.heap.push(iterator);
            }
        } else {
            self.push_iterators(search_key).await?;
        }

        self.advance_impl(search_key).await
    }

    fn advance_dyn<'b>(&'b mut self) -> Result<Option<BoxFuture<'b, Result<(), Error>>>, Error> {
        Ok(Some(Box::pin(self.advance())))
    }

    fn get(&self) -> Option<ItemRef<'_, K, V>> {
        (&self.item).into()
    }
}

struct MutMergeLayerIterator<'a, K, V>(MergeLayerIterator<'a, K, V>);

impl<K, V> MutMergeLayerIterator<'_, K, V> {
    fn advance(&mut self) {
        if let MergeItem::Iter = self.item {
            self.as_mut().advance();
        }
        self.set_item_from_iter();
    }
}

impl<'a, K, V> AsMut<dyn LayerIteratorMut<K, V> + 'a> for MutMergeLayerIterator<'a, K, V> {
    fn as_mut(&mut self) -> &mut (dyn LayerIteratorMut<K, V> + 'a) {
        let RawIterator::Mut(iter) = &mut self.0.iter else { unreachable!() };
        iter.as_mut()
    }
}

impl<'a, K, V> Deref for MutMergeLayerIterator<'a, K, V> {
    type Target = MergeLayerIterator<'a, K, V>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V> DerefMut for MutMergeLayerIterator<'_, K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Merges the given item into a mutable layer.
pub(super) fn merge_into<K: Debug + OrdLowerBound, V: Debug>(
    mut_iter: Box<dyn LayerIteratorMut<K, V> + '_>,
    item: Item<K, V>,
    merge_fn: MergeFn<K, V>,
) -> Result<(), Error> {
    let merge_item = if mut_iter.get().is_some() { MergeItem::Iter } else { MergeItem::None };
    let mut mut_merge_iter = MutMergeLayerIterator(MergeLayerIterator {
        layer: None,
        iter: RawIterator::Mut(mut_iter),
        layer_index: 1,
        item: merge_item,
    });
    let mut item_merge_iter = MergeLayerIterator::new_with_item(0, MergeItem::Item(item.boxed()));
    while mut_merge_iter.is_some() && item_merge_iter.is_some() {
        if mut_merge_iter.0 > item_merge_iter {
            // In this branch the mutable layer is left and the item we're merging-in is right.
            let merge_result = merge_fn(&mut_merge_iter, &item_merge_iter);
            debug!(
                lhs:? = mut_merge_iter.key(),
                rhs:? = item_merge_iter.key(),
                result:? = merge_result;
                "(1) merge");
            match merge_result {
                MergeResult::EmitLeft => {
                    if let Some(item) = mut_merge_iter.take_item() {
                        mut_merge_iter.as_mut().insert(*item);
                        mut_merge_iter.set_item_from_iter();
                    } else {
                        mut_merge_iter.advance();
                    }
                }
                MergeResult::Other { emit, left, right } => {
                    if let Some(emit) = emit {
                        mut_merge_iter.as_mut().insert(*emit);
                    }
                    match left {
                        ItemOp::Keep => {}
                        ItemOp::Discard => {
                            if matches!(mut_merge_iter.item, MergeItem::Iter) {
                                mut_merge_iter.as_mut().erase();
                            }
                            mut_merge_iter.set_item_from_iter();
                        }
                        ItemOp::Replace(item) => {
                            if let MergeItem::Iter = mut_merge_iter.item {
                                mut_merge_iter.as_mut().erase();
                            }
                            mut_merge_iter.item = MergeItem::Item(item)
                        }
                    }
                    match right {
                        ItemOp::Keep => {}
                        ItemOp::Discard => item_merge_iter.item = MergeItem::None,
                        ItemOp::Replace(item) => item_merge_iter.item = MergeItem::Item(item),
                    }
                }
            }
        } else {
            // In this branch, the item we're merging-in is left and the mutable layer is right.
            let merge_result = merge_fn(&item_merge_iter, &mut_merge_iter);
            debug!(
                lhs:? = mut_merge_iter.key(),
                rhs:? = item_merge_iter.key(),
                result:? = merge_result;
                "(2) merge");
            match merge_result {
                MergeResult::EmitLeft => break, // Item is inserted outside the loop
                MergeResult::Other { emit, left, right } => {
                    if let Some(emit) = emit {
                        mut_merge_iter.as_mut().insert(*emit);
                    }
                    match left {
                        ItemOp::Keep => {}
                        ItemOp::Discard => item_merge_iter.item = MergeItem::None,
                        ItemOp::Replace(item) => item_merge_iter.item = MergeItem::Item(item),
                    }
                    match right {
                        ItemOp::Keep => {}
                        ItemOp::Discard => {
                            if matches!(mut_merge_iter.item, MergeItem::Iter) {
                                mut_merge_iter.as_mut().erase();
                            }
                            mut_merge_iter.set_item_from_iter();
                        }
                        ItemOp::Replace(item) => {
                            if let MergeItem::Iter = mut_merge_iter.item {
                                mut_merge_iter.as_mut().erase();
                            }
                            mut_merge_iter.item = MergeItem::Item(item)
                        }
                    }
                }
            }
        }
    } // while ...

    // The only way we could get here with both items is via the break above, so we know the correct
    // order required here.
    if let MergeItem::Item(item) = item_merge_iter.item {
        mut_merge_iter.as_mut().insert(*item);
    }
    if let Some(item) = mut_merge_iter.take_item() {
        mut_merge_iter.as_mut().insert(*item);
    }
    if let RawIterator::Mut(mut iter) = mut_merge_iter.0.iter {
        iter.commit();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ItemOp::{Discard, Keep, Replace};
    use super::{MergeLayerIterator, MergeResult, Merger};
    use crate::lsm_tree::persistent_layer::{PersistentLayer, PersistentLayerWriter};
    use crate::lsm_tree::skip_list_layer::SkipListLayer;
    use crate::lsm_tree::types::{
        FuzzyHash, Item, ItemRef, Key, Layer, LayerIterator, LayerKey, LayerWriter,
        MaybeContainsKey, MergeType, OrdLowerBound, OrdUpperBound, SortByU64,
    };
    use crate::lsm_tree::{self, Query, Value};
    use crate::object_store::{self, AttributeId, ObjectKey, ObjectValue, VOLUME_DATA_KEY_ID};
    use crate::serialized_types::{
        LATEST_VERSION, Version, Versioned, VersionedLatest, versioned_type,
    };
    use crate::testing::fake_object::{FakeObject, FakeObjectHandle};
    use crate::testing::writer::Writer;
    use fprint::TypeFingerprint;
    use fxfs_macros::{FuzzyHash, SerializeKey};
    use rand::RngExt as _;
    use std::hash::Hash;
    use std::ops::{Bound, Range};
    use std::sync::Arc;
    use storage_units::BlockSize;

    use crate::lsm_tree::testing::TestKey;

    impl Value for i32 {
        const DELETED_MARKER: Self = 0;
    }

    fn layer_ref_iter<K: Key, V: Value>(
        layers: &[Arc<SkipListLayer<K, V>>],
    ) -> impl Iterator<Item = &dyn Layer<K, V>> {
        layers.iter().map(|x| x.as_ref() as &dyn Layer<K, V>)
    }

    fn dyn_layer_ref_iter<K: Key, V: Value>(
        layers: &[Arc<dyn Layer<K, V>>],
    ) -> impl Iterator<Item = &dyn Layer<K, V>> {
        layers.iter().map(|x| x.as_ref())
    }

    fn counters() -> Arc<lsm_tree::TreeCounters> {
        Arc::new(lsm_tree::TreeCounters::default())
    }

    #[fuchsia::test]
    async fn test_emit_left() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::EmitLeft,
            counters(),
        );
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_other_emit() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other {
                emit: Some(Item::new(TestKey(3..3), 3).boxed()),
                left: Discard,
                right: Discard,
            },
            counters(),
        );
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");

        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(3..3), &3));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_replace_left() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other {
                emit: None,
                left: Replace(Item::new(TestKey(3..3), 3).boxed()),
                right: Discard,
            },
            counters(),
        );
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");

        // The merger should replace the left item and then after discarding the right item, it
        // should emit the replacement.
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(3..3), &3));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_replace_right() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other {
                emit: None,
                left: Discard,
                right: Replace(Item::new(TestKey(3..3), 3).boxed()),
            },
            counters(),
        );
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");

        // The merger should replace the right item and then after discarding the left item, it
        // should emit the replacement.
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(3..3), &3));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_left_less_than_right() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |left, right| {
                assert_eq!((left.key(), left.value()), (&TestKey(1..1), &1));
                assert_eq!((right.key(), right.value()), (&TestKey(2..2), &2));
                MergeResult::EmitLeft
            },
            counters(),
        );
        merger.query(Query::FullScan).await.expect("seek failed");
    }

    #[fuchsia::test]
    async fn test_left_equals_right() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let item = Item::new(TestKey(1..1), 1);
        skip_lists[0].insert(item.clone()).expect("insert error");
        skip_lists[1].insert(item.clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |left, right| {
                assert_eq!((left.key(), left.value()), (&TestKey(1..1), &1));
                assert_eq!((right.key(), right.value()), (&TestKey(1..1), &1));
                assert_eq!(left.layer_index, 0);
                assert_eq!(right.layer_index, 1);
                MergeResult::EmitLeft
            },
            counters(),
        );
        merger.query(Query::FullScan).await.expect("seek failed");
    }

    #[fuchsia::test]
    async fn test_keep() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |left, right| {
                if left.key() == &TestKey(1..1) {
                    MergeResult::Other {
                        emit: None,
                        left: Replace(Item::new(TestKey(3..3), 3).boxed()),
                        right: Keep,
                    }
                } else {
                    assert_eq!(left.key(), &TestKey(2..2));
                    assert_eq!(right.key(), &TestKey(3..3));
                    MergeResult::Other { emit: None, left: Discard, right: Keep }
                }
            },
            counters(),
        );
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");

        // The merger should first replace left and then it should call the merger again with 2 & 3
        // and end up just keeping 3.
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(3..3), &3));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_10_layers() {
        let skip_lists: Vec<_> = (0..10).map(|_| SkipListLayer::new(100)).collect();
        let mut rng = rand::rng();
        for i in 0..100 {
            skip_lists[rng.random_range(0..10) as usize]
                .insert(Item::new(TestKey(i..i), i))
                .expect("insert error");
        }
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::EmitLeft,
            counters(),
        );
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");

        for i in 0..100 {
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!((key, value), (&TestKey(i..i), &i));
            iter.advance().await.unwrap();
        }
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_uses_cmp_lower_bound() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..10), 1), Item::new(TestKey(2..3), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::EmitLeft,
            counters(),
        );
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");

        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_into_emit_left() {
        let skip_list = SkipListLayer::new(100);
        let items =
            [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2), Item::new(TestKey(3..3), 3)];
        skip_list.insert(items[0].clone()).expect("insert error");
        skip_list.insert(items[2].clone()).expect("insert error");
        skip_list
            .merge_into(items[1].clone(), &items[0].key, |_left, _right| MergeResult::EmitLeft);

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[2].key, &items[2].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_into_emit_last_after_replacing() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_list.insert(items[0].clone()).expect("insert error");

        skip_list.merge_into(items[1].clone(), &items[0].key, |left, right| {
            if left.key() == &TestKey(1..1) {
                assert_eq!(right.key(), &TestKey(2..2));
                MergeResult::Other {
                    emit: None,
                    left: Replace(Item::new(TestKey(3..3), 3).boxed()),
                    right: Keep,
                }
            } else {
                assert_eq!(left.key(), &TestKey(2..2));
                assert_eq!(right.key(), &TestKey(3..3));
                MergeResult::EmitLeft
            }
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(3..3), &3));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_into_emit_left_after_replacing() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(3..3), 3)];
        skip_list.insert(items[0].clone()).expect("insert error");

        skip_list.merge_into(items[1].clone(), &items[0].key, |left, right| {
            if left.key() == &TestKey(1..1) {
                assert_eq!(right.key(), &TestKey(3..3));
                MergeResult::Other {
                    emit: None,
                    left: Replace(Item::new(TestKey(2..2), 2).boxed()),
                    right: Keep,
                }
            } else {
                assert_eq!(left.key(), &TestKey(2..2));
                assert_eq!(right.key(), &TestKey(3..3));
                MergeResult::EmitLeft
            }
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(2..2), &2));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    // This tests emitting in both branches of merge_into, and most of the discard paths.
    #[fuchsia::test]
    async fn test_merge_into_emit_other_and_discard() {
        let skip_list = SkipListLayer::new(100);
        let items =
            [Item::new(TestKey(1..1), 1), Item::new(TestKey(3..3), 3), Item::new(TestKey(5..5), 3)];
        skip_list.insert(items[0].clone()).expect("insert error");
        skip_list.insert(items[2].clone()).expect("insert error");

        skip_list.merge_into(items[1].clone(), &items[0].key, |left, right| {
            if left.key() == &TestKey(1..1) {
                // This tests the top branch in merge_into.
                assert_eq!(right.key(), &TestKey(3..3));
                MergeResult::Other {
                    emit: Some(Item::new(TestKey(2..2), 2).boxed()),
                    left: Discard,
                    right: Keep,
                }
            } else {
                // This tests the bottom branch in merge_into.
                assert_eq!(left.key(), &TestKey(3..3));
                assert_eq!(right.key(), &TestKey(5..5));
                MergeResult::Other {
                    emit: Some(Item::new(TestKey(4..4), 4).boxed()),
                    left: Discard,
                    right: Discard,
                }
            }
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(2..2), &2));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(4..4), &4));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    // This tests replacing the item and discarding the right item (the one remaining untested
    // discard path) in the top branch in merge_into.
    #[fuchsia::test]
    async fn test_merge_into_replace_and_discard() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(3..3), 3)];
        skip_list.insert(items[0].clone()).expect("insert error");

        skip_list.merge_into(items[1].clone(), &items[0].key, |_left, _right| MergeResult::Other {
            emit: Some(Item::new(TestKey(2..2), 2).boxed()),
            left: Replace(Item::new(TestKey(4..4), 4).boxed()),
            right: Discard,
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(2..2), &2));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(4..4), &4));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    // This tests replacing the right item in the top branch of merge_into and the left item in the
    // bottom branch of merge_into.
    #[fuchsia::test]
    async fn test_merge_into_replace_merge_item() {
        let skip_list = SkipListLayer::new(100);
        let items =
            [Item::new(TestKey(1..1), 1), Item::new(TestKey(3..3), 3), Item::new(TestKey(5..5), 5)];
        skip_list.insert(items[0].clone()).expect("insert error");
        skip_list.insert(items[2].clone()).expect("insert error");

        skip_list.merge_into(items[1].clone(), &items[0].key, |_left, right| {
            if right.key() == &TestKey(3..3) {
                MergeResult::Other {
                    emit: None,
                    left: Discard,
                    right: Replace(Item::new(TestKey(2..2), 2).boxed()),
                }
            } else {
                assert_eq!(right.key(), &TestKey(5..5));
                MergeResult::Other {
                    emit: None,
                    left: Replace(Item::new(TestKey(4..4), 4).boxed()),
                    right: Discard,
                }
            }
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(4..4), &4));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    // This tests replacing the right item in the bottom branch of merge_into.
    #[fuchsia::test]
    async fn test_merge_into_replace_existing() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(3..3), 3)];
        skip_list.insert(items[1].clone()).expect("insert error");

        skip_list.merge_into(items[0].clone(), &items[0].key, |_left, right| {
            if right.key() == &TestKey(3..3) {
                MergeResult::Other {
                    emit: None,
                    left: Keep,
                    right: Replace(Item::new(TestKey(2..2), 2).boxed()),
                }
            } else {
                MergeResult::EmitLeft
            }
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(2..2), &2));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_into_discard_last() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_list.insert(items[0].clone()).expect("insert error");

        skip_list.merge_into(items[1].clone(), &items[0].key, |_left, _right| MergeResult::Other {
            emit: None,
            left: Discard,
            right: Keep,
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_into_empty() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1..1), 1)];

        skip_list.merge_into(items[0].clone(), &items[0].key, |_left, _right| {
            panic!("Unexpected merge!");
        });

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_seek_uses_minimum_number_of_iterators() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..2), 1), Item::new(TestKey(1..2), 2)];
        skip_lists[0].insert(items[0].clone()).expect("insert error");
        skip_lists[1].insert(items[1].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other { emit: None, left: Discard, right: Keep },
            counters(),
        );
        let iter = merger
            .query(Query::FullRange(&items[0].key.search_key().unwrap()))
            .await
            .expect("seek failed");

        // Seek should only search in the first skip list, so no merge should take place, and we'll
        // know if it has because we'll see a different value (2 rather than 1).
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
    }

    // Checks that merging the given layers produces |expected| sequence of items starting from
    // |start|.
    async fn test_advance<K: Eq + Key + LayerKey + OrdLowerBound>(
        layers: &[&[(K, i32)]],
        query: Query<'_, K>,
        expected: &[(K, i32)],
    ) {
        let mut skip_lists = Vec::new();
        for &layer in layers {
            let skip_list = SkipListLayer::new(100);
            for (k, v) in layer {
                skip_list.insert(Item::new(k.clone(), *v)).expect("insert error");
            }
            skip_lists.push(skip_list);
        }
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::EmitLeft,
            counters(),
        );
        let mut iter = merger.query(query).await.expect("seek failed");
        for (k, v) in expected {
            let ItemRef { key, value, .. } = iter.get().expect("get failed");
            assert_eq!((key, value), (k, v));
            iter.advance().await.expect("advance failed");
        }
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_seek_skips_replaced_items() {
        // The 1..2 and the 2..3 items are overwritten and merging them should be skipped.
        test_advance(
            &[
                &[(TestKey(1..2), 1), (TestKey(2..3), 2), (TestKey(4..5), 3)],
                &[(TestKey(1..2), 4), (TestKey(2..3), 5), (TestKey(3..4), 6)],
            ],
            Query::FullRange(&TestKey(1..2).search_key().unwrap()),
            &[(TestKey(1..2), 1), (TestKey(2..3), 2), (TestKey(3..4), 6), (TestKey(4..5), 3)],
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_advance_skips_replaced_items_at_end() {
        // Like the last test, the 1..2 item is overwritten and seeking for it should skip the merge
        // but this time, the keys are at the end.
        test_advance(
            &[&[(TestKey(1..2), 1)], &[(TestKey(1..2), 2)]],
            Query::FullRange(&TestKey(1..2).search_key().unwrap()),
            &[(TestKey(1..2), 1)],
        )
        .await;
    }

    #[derive(
        Clone,
        Eq,
        Hash,
        FuzzyHash,
        PartialEq,
        Debug,
        serde::Serialize,
        serde::Deserialize,
        TypeFingerprint,
        Versioned,
        SerializeKey,
    )]
    struct TestKeyWithFullMerge(Range<u64>);

    versioned_type! { 1.. => TestKeyWithFullMerge }

    impl LayerKey for TestKeyWithFullMerge {
        fn merge_type(&self) -> MergeType {
            MergeType::FullMerge
        }

        fn search_key(&self) -> Option<Self> {
            Some(Self(self.0.start..self.0.start + 1))
        }

        fn is_search_key(&self) -> bool {
            self.0.end == self.0.start + 1
        }

        fn overlaps(&self, other: &Self) -> bool {
            self.0.start < other.0.end && self.0.end > other.0.start
        }
    }

    impl SortByU64 for TestKeyWithFullMerge {
        fn get_leading_u64(&self) -> u64 {
            self.0.end
        }
    }

    impl OrdUpperBound for TestKeyWithFullMerge {
        fn cmp_upper_bound(&self, other: &TestKeyWithFullMerge) -> std::cmp::Ordering {
            self.0.end.cmp(&other.0.end).then(other.0.start.cmp(&self.0.start))
        }
    }

    impl OrdLowerBound for TestKeyWithFullMerge {
        fn cmp_lower_bound(&self, other: &Self) -> std::cmp::Ordering {
            self.0.start.cmp(&other.0.start)
        }
    }

    // Same set up as `test_seek_skips_replaced_items` except here nothing should skip. Any seek
    // should just place the iterator at a particular spot in the merged layers.
    #[fuchsia::test]
    async fn test_full_merge_consistent_advance_ordering() {
        let layer_set = [
            [
                (TestKeyWithFullMerge(1..2), 1i32),
                (TestKeyWithFullMerge(2..3), 2i32),
                (TestKeyWithFullMerge(4..5), 3i32),
            ]
            .as_slice(),
            [
                (TestKeyWithFullMerge(1..2), 4i32),
                (TestKeyWithFullMerge(2..3), 5i32),
                (TestKeyWithFullMerge(3..4), 6i32),
            ]
            .as_slice(),
        ];

        let full_merge_result = [
            (TestKeyWithFullMerge(1..2), 1),
            (TestKeyWithFullMerge(1..2), 4),
            (TestKeyWithFullMerge(2..3), 2),
            (TestKeyWithFullMerge(2..3), 5),
            (TestKeyWithFullMerge(3..4), 6),
            (TestKeyWithFullMerge(4..5), 3),
        ];

        test_advance(layer_set.as_slice(), Query::FullScan, &full_merge_result).await;

        test_advance(
            layer_set.as_slice(),
            Query::FullRange(&TestKeyWithFullMerge(1..2).search_key().unwrap()),
            &full_merge_result,
        )
        .await;

        test_advance(
            layer_set.as_slice(),
            Query::FullRange(&TestKeyWithFullMerge(2..3).search_key().unwrap()),
            &full_merge_result[2..],
        )
        .await;

        test_advance(
            layer_set.as_slice(),
            Query::FullRange(&TestKeyWithFullMerge(3..4).search_key().unwrap()),
            &full_merge_result[4..],
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_full_merge_always_consult_all_layers() {
        // | 1 |   |
        // |   | 2 |
        // | 3 | 4 |
        let skip_lists =
            [SkipListLayer::new(100), SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [
            Item::new(TestKeyWithFullMerge(1..2), 1),
            Item::new(TestKeyWithFullMerge(2..3), 2),
            Item::new(TestKeyWithFullMerge(1..2), 3),
            Item::new(TestKeyWithFullMerge(2..3), 4),
        ];
        skip_lists[0].insert(items[0].clone()).expect("insert error");
        skip_lists[1].insert(items[1].clone()).expect("insert error");
        skip_lists[2].insert(items[2].clone()).expect("insert error");
        skip_lists[2].insert(items[3].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |left, right| {
                // Sum matching keys.
                if left.key() == right.key() {
                    MergeResult::Other {
                        emit: None,
                        left: Discard,
                        right: Replace(
                            Item::new(left.key().clone(), left.value() + right.value()).boxed(),
                        ),
                    }
                } else {
                    MergeResult::EmitLeft
                }
            },
            counters(),
        );
        let mut iter = merger
            .query(Query::FullRange(&items[0].key.search_key().unwrap()))
            .await
            .expect("seek failed");

        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[0].key, items[0].value + items[2].value));
        iter.advance().await.expect("advance");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[1].key, items[1].value + items[3].value));
        iter.advance().await.expect("advance");
        assert!(iter.get().is_none());
    }

    #[derive(
        Clone,
        Eq,
        Hash,
        FuzzyHash,
        PartialEq,
        Debug,
        serde::Serialize,
        serde::Deserialize,
        TypeFingerprint,
        Versioned,
        SerializeKey,
    )]
    struct TestKeyWithDefaultLayerKey(Range<u64>);

    versioned_type! { 1.. => TestKeyWithDefaultLayerKey }

    // Default layer key is using `MergeType::FullMerge` and returns None for `next_key()`.
    impl LayerKey for TestKeyWithDefaultLayerKey {
        fn search_key(&self) -> Option<Self> {
            Some(Self(self.0.start..self.0.start + 1))
        }

        fn is_search_key(&self) -> bool {
            self.0.end == self.0.start + 1
        }

        fn overlaps(&self, other: &Self) -> bool {
            self.0.start < other.0.end && self.0.end > other.0.start
        }
    }

    impl SortByU64 for TestKeyWithDefaultLayerKey {
        fn get_leading_u64(&self) -> u64 {
            self.0.end
        }
    }

    impl OrdUpperBound for TestKeyWithDefaultLayerKey {
        fn cmp_upper_bound(&self, other: &TestKeyWithDefaultLayerKey) -> std::cmp::Ordering {
            self.0.end.cmp(&other.0.end).then(other.0.start.cmp(&self.0.start))
        }
    }

    impl OrdLowerBound for TestKeyWithDefaultLayerKey {
        fn cmp_lower_bound(&self, other: &Self) -> std::cmp::Ordering {
            self.0.start.cmp(&other.0.start)
        }
    }

    #[fuchsia::test]
    async fn test_no_merge_unbounded_include_all_layers() {
        test_advance(
            &[
                &[
                    (TestKeyWithDefaultLayerKey(1..2), 1),
                    (TestKeyWithDefaultLayerKey(2..3), 2),
                    (TestKeyWithDefaultLayerKey(4..5), 3),
                ],
                &[
                    (TestKeyWithDefaultLayerKey(1..2), 4),
                    (TestKeyWithDefaultLayerKey(2..3), 5),
                    (TestKeyWithDefaultLayerKey(3..4), 6),
                ],
            ],
            Query::FullScan,
            &[
                (TestKeyWithDefaultLayerKey(1..2), 1),
                (TestKeyWithDefaultLayerKey(1..2), 4),
                (TestKeyWithDefaultLayerKey(2..3), 2),
                (TestKeyWithDefaultLayerKey(2..3), 5),
                (TestKeyWithDefaultLayerKey(3..4), 6),
                (TestKeyWithDefaultLayerKey(4..5), 3),
            ],
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_no_merge_proceeds_comprehensively_after_seek() {
        test_advance(
            &[
                &[
                    (TestKeyWithDefaultLayerKey(1..2), 1),
                    (TestKeyWithDefaultLayerKey(2..3), 2),
                    (TestKeyWithDefaultLayerKey(4..5), 3),
                ],
                &[
                    (TestKeyWithDefaultLayerKey(1..2), 4),
                    (TestKeyWithDefaultLayerKey(2..3), 5),
                    (TestKeyWithDefaultLayerKey(3..4), 6),
                ],
            ],
            Query::FullRange(&TestKeyWithDefaultLayerKey(1..2).search_key().unwrap()),
            &[
                (TestKeyWithDefaultLayerKey(1..2), 1),
                (TestKeyWithDefaultLayerKey(1..2), 4),
                (TestKeyWithDefaultLayerKey(2..3), 2),
                (TestKeyWithDefaultLayerKey(2..3), 5),
                (TestKeyWithDefaultLayerKey(3..4), 6),
                (TestKeyWithDefaultLayerKey(4..5), 3),
            ],
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_no_merge_seek_finds_lower_layer() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [
            Item::new(TestKeyWithDefaultLayerKey(2..3), 1),
            Item::new(TestKeyWithDefaultLayerKey(0..1), 2),
        ];
        skip_lists[0].insert(items[0].clone()).expect("insert error");
        skip_lists[1].insert(items[1].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::EmitLeft,
            counters(),
        );
        let iter = merger
            .query(Query::FullRange(&items[1].key.search_key().unwrap()))
            .await
            .expect("seek failed");

        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
    }

    #[fuchsia::test]
    async fn test_no_merge_seek_stops_at_exact_match() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [
            Item::new(TestKeyWithDefaultLayerKey(2..3), 1),
            Item::new(TestKeyWithDefaultLayerKey(1..4), 2),
        ];
        skip_lists[0].insert(items[0].clone()).expect("insert error");
        skip_lists[1].insert(items[1].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other { emit: None, left: Discard, right: Keep },
            counters(),
        );
        let iter = merger
            .query(Query::FullRange(&items[0].key.search_key().unwrap()))
            .await
            .expect("seek failed");

        // Seek should only search in the first skip list, so no merge should take place, and we'll
        // know if it has because we'll see a different value (2 rather than 1).
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
    }

    #[fuchsia::test]
    async fn test_seek_less_than() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[0].clone()).expect("insert error");
        skip_lists[1].insert(items[1].clone()).expect("insert error");
        // Search for a key before 1..1.
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other { emit: None, left: Discard, right: Keep },
            counters(),
        );
        let iter = merger
            .query(Query::FullRange(&TestKey(0..0).search_key().unwrap()))
            .await
            .expect("seek failed");

        // This should find the 2..2 key because of our merge function.
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
    }

    #[fuchsia::test]
    async fn test_seek_to_end() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..1), 1), Item::new(TestKey(2..2), 2)];
        skip_lists[0].insert(items[0].clone()).expect("insert error");
        skip_lists[1].insert(items[1].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other { emit: None, left: Discard, right: Keep },
            counters(),
        );
        let iter = merger
            .query(Query::FullRange(&TestKey(3..3).search_key().unwrap()))
            .await
            .expect("seek failed");

        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_all_discarded() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [Item::new(TestKey(1..2), 1), Item::new(TestKey(2..3), 2)];
        skip_lists[0].insert(items[1].clone()).expect("insert error");
        skip_lists[1].insert(items[0].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |_left, _right| MergeResult::Other { emit: None, left: Discard, right: Discard },
            counters(),
        );
        let iter = merger.query(Query::FullScan).await.expect("seek failed");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_overlapping_keys() {
        let skip_lists =
            [SkipListLayer::new(100), SkipListLayer::new(100), SkipListLayer::new(100)];
        let items = [
            Item::new(TestKey(0..10), 1),
            Item::new(TestKey(0..20), 2),
            Item::new(TestKey(0..30), 3),
        ];
        skip_lists[0].insert(items[0].clone()).expect("insert error");
        skip_lists[1].insert(items[1].clone()).expect("insert error");
        skip_lists[2].insert(items[2].clone()).expect("insert error");
        let mut merger = Merger::new(
            layer_ref_iter(&skip_lists),
            |left, right| {
                if left.key().0.end <= right.key().0.start {
                    MergeResult::EmitLeft
                } else {
                    // Hardcoded rule for specific test data to test containment split.
                    if left.key() == &TestKey(0..30) && right.key() == &TestKey(10..20) {
                        MergeResult::Other {
                            emit: Some(Item::new(TestKey(0..10), 1).boxed()),
                            left: Replace(Item::new(TestKey(10..30), 1).boxed()),
                            right: Keep,
                        }
                    } else {
                        MergeResult::Other {
                            emit: None,
                            left: Keep,
                            right: Replace(
                                Item::new(TestKey(left.key().0.end..right.key().0.end), 1).boxed(),
                            ),
                        }
                    }
                }
            },
            counters(),
        );
        let mut iter = merger
            .query(Query::FullRange(&TestKey(0..1).search_key().unwrap()))
            .await
            .expect("seek failed");
        assert_eq!(iter.pending_iterators_len(), 2);
        let ItemRef { key, .. } = iter.get().expect("missing item");
        assert_eq!(key, &TestKey(0..10));
        iter.advance().await.expect("advance failed");
        assert_eq!(iter.pending_iterators_len(), 1);
        let ItemRef { key, .. } = iter.get().expect("missing item");
        assert_eq!(key, &TestKey(10..20));
        iter.advance().await.expect("advance failed");
        assert_eq!(iter.pending_iterators_len(), 0);
        let ItemRef { key, .. } = iter.get().expect("missing item");
        assert_eq!(key, &TestKey(20..30));
        iter.advance().await.expect("advance failed");
        assert_eq!(iter.pending_iterators_len(), 0);
        assert_eq!(iter.get(), None);
    }

    async fn write_layer<K: Key, V: Value>(items: Vec<Item<K, V>>) -> Arc<dyn Layer<K, V>> {
        let object = Arc::new(FakeObject::new());
        let write_handle = FakeObjectHandle::new(object.clone());
        let mut writer = PersistentLayerWriter::<_, K, V>::new(
            Writer::new(&write_handle).await,
            items.len(),
            BlockSize::SIZE_512B,
        )
        .await
        .expect("PersistentLayerWriter::new failed");
        for item in items {
            writer.write(item.as_item_ref()).await.expect("write failed");
        }
        writer.complete().await.expect("flush failed");
        PersistentLayer::open(FakeObjectHandle::new(object))
            .await
            .expect("open_persistent_layer failed")
    }

    fn merge_sum(
        left: &MergeLayerIterator<'_, i32, i32>,
        right: &MergeLayerIterator<'_, i32, i32>,
    ) -> MergeResult<i32, i32> {
        // Sum matching keys.
        if left.key() == right.key() {
            MergeResult::Other {
                emit: None,
                left: Discard,
                right: Replace(Item::new(left.key().clone(), left.value() + right.value()).boxed()),
            }
        } else {
            MergeResult::EmitLeft
        }
    }

    #[fuchsia::test]
    async fn test_merge_bloom_filters_point_query() {
        let layer_0_items = vec![Item::new(1, 1), Item::new(2, 1)];
        let layer_1_items = vec![Item::new(2, 1), Item::new(4, 1)];
        let items = [Item::new(1, 1), Item::new(2, 2), Item::new(4, 1)];
        let layers: [Arc<dyn Layer<i32, i32>>; 2] =
            [write_layer(layer_0_items).await, write_layer(layer_1_items).await];
        let mut merger = Merger::new(dyn_layer_ref_iter(&layers), merge_sum, counters());

        {
            // Key that exists in layer 0 only
            let iter = merger.query(Query::Point(&1)).await.expect("seek failed");
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!((key, *value), (&items[0].key, items[0].value));
        }
        {
            // Key that exists in layer 1 and 2
            let iter = merger.query(Query::Point(&2)).await.expect("seek failed");
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!((key, *value), (&items[1].key, items[1].value));
        }
        {
            // Key that exists in layer 1
            let iter = merger.query(Query::Point(&4)).await.expect("seek failed");
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!((key, *value), (&items[2].key, items[2].value));
        }
        {
            // Key that doesn't exist at all
            let iter = merger.query(Query::Point(&400)).await.expect("seek failed");
            assert!(iter.get().is_none());
        }
    }

    #[fuchsia::test]
    async fn test_merge_bloom_filters_limited_range() {
        // NB: This test uses ObjectKey so that we don't have to reimplement the complex merging
        // logic for range-like keys.
        let layer_0_items = vec![Item::new(
            ObjectKey::extent(0, AttributeId::TEST_ID, 0..2048),
            ObjectValue::extent(0, VOLUME_DATA_KEY_ID),
        )];
        let layer_1_items = vec![
            Item::new(
                ObjectKey::extent(0, AttributeId::TEST_ID, 1024..4096),
                ObjectValue::extent(32768, VOLUME_DATA_KEY_ID),
            ),
            Item::new(
                ObjectKey::extent(0, AttributeId::TEST_ID, 16384..17408),
                ObjectValue::extent(65536, VOLUME_DATA_KEY_ID),
            ),
        ];
        let items = [
            Item::new(
                ObjectKey::extent(0, AttributeId::TEST_ID, 0..2048),
                ObjectValue::extent(0, VOLUME_DATA_KEY_ID),
            ),
            Item::new(
                ObjectKey::extent(0, AttributeId::TEST_ID, 2048..4096),
                ObjectValue::extent(33792, VOLUME_DATA_KEY_ID),
            ),
            Item::new(
                ObjectKey::extent(0, AttributeId::TEST_ID, 16384..17408),
                ObjectValue::extent(65536, VOLUME_DATA_KEY_ID),
            ),
        ];
        let layers: [Arc<dyn Layer<ObjectKey, ObjectValue>>; 2] =
            [write_layer(layer_0_items).await, write_layer(layer_1_items).await];
        let mut merger =
            Merger::new(dyn_layer_ref_iter(&layers), object_store::merge::merge, counters());

        {
            // Range contains just keys in layer 1
            let mut iter = merger
                .query(Query::LimitedRange(&ObjectKey::extent(
                    0,
                    AttributeId::TEST_ID,
                    16384..16386,
                )))
                .await
                .expect("seek failed");
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!(key, &items[2].key);
            assert_eq!(value, &items[2].value);
            iter.advance().await.expect("advance");
            assert!(iter.get().is_none());
        }
        {
            // Range contains keys in layer 0 and 1
            let mut iter = merger
                .query(Query::LimitedRange(&ObjectKey::extent(0, AttributeId::TEST_ID, 0..4096)))
                .await
                .expect("seek failed");
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!(key, &items[0].key);
            assert_eq!(value, &items[0].value);
            iter.advance().await.expect("advance");
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!(key, &items[1].key);
            assert_eq!(value, &items[1].value);
            iter.advance().await.expect("advance");
        }
        {
            // Range contains no keys
            let mut iter = merger
                .query(Query::LimitedRange(&ObjectKey::extent(
                    0,
                    AttributeId::TEST_ID,
                    8192..12288,
                )))
                .await
                .expect("seek failed");
            let ItemRef { key, .. } = iter.get().expect("missing item");
            assert_eq!(key, &items[2].key);
            iter.advance().await.expect("advance");
            assert!(iter.get().is_none());
        }
    }

    #[fuchsia::test]
    async fn test_merge_bloom_filters_full_range() {
        let layer_0_items = vec![Item::new(1, 1), Item::new(2, 1)];
        let layer_1_items = vec![Item::new(2, 1), Item::new(4, 1)];
        let items = [Item::new(1, 1), Item::new(2, 2), Item::new(4, 1)];
        let layers: [Arc<dyn Layer<i32, i32>>; 2] =
            [write_layer(layer_0_items).await, write_layer(layer_1_items).await];
        let mut merger = Merger::new(dyn_layer_ref_iter(&layers), merge_sum, counters());

        let mut iter = merger.query(Query::FullRange(&0)).await.expect("seek failed");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[0].key, items[0].value));
        iter.advance().await.expect("advance");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[1].key, items[1].value));
        iter.advance().await.expect("advance");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[2].key, items[2].value));
        iter.advance().await.expect("advance");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_bloom_filters_full_scan() {
        let layer_0_items = vec![Item::new(1, 1), Item::new(2, 1)];
        let layer_1_items = vec![Item::new(2, 1), Item::new(4, 1)];
        let items = [Item::new(1, 1), Item::new(2, 2), Item::new(4, 1)];
        let layers: [Arc<dyn Layer<i32, i32>>; 2] =
            [write_layer(layer_0_items).await, write_layer(layer_1_items).await];
        let mut merger = Merger::new(dyn_layer_ref_iter(&layers), merge_sum, counters());

        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[0].key, items[0].value));
        iter.advance().await.expect("advance");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[1].key, items[1].value));
        iter.advance().await.expect("advance");
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, *value), (&items[2].key, items[2].value));
        iter.advance().await.expect("advance");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_optimized_merge_lazy_pull() {
        let skip_lists = [SkipListLayer::new(100), SkipListLayer::new(100)];
        let items_top = [
            Item::new(TestKey(10..20), 1),
            Item::new(TestKey(20..30), 2),
            Item::new(TestKey(30..40), 3),
        ];
        let items_bottom = [Item::new(TestKey(40..50), 4)];

        for item in &items_top {
            skip_lists[0].insert(item.clone()).expect("insert error");
        }
        for item in &items_bottom {
            skip_lists[1].insert(item.clone()).expect("insert error");
        }

        let mut merger =
            Merger::new(layer_ref_iter(&skip_lists), |_, _| MergeResult::EmitLeft, counters());
        merger.set_trace(true);

        let mut iter =
            merger.query(Query::LimitedRange(&TestKey(10..20))).await.expect("seek failed");
        assert_eq!(iter.pending_iterators_len(), 1);

        let ItemRef { key, .. } = iter.get().expect("missing item");
        assert_eq!(key, &TestKey(10..20));

        iter.advance().await.expect("advance failed");
        assert_eq!(iter.pending_iterators_len(), 1);

        let ItemRef { key, .. } = iter.get().expect("missing item");
        assert_eq!(key, &TestKey(20..30));

        iter.advance().await.expect("advance failed");
        assert_eq!(iter.pending_iterators_len(), 1);

        let ItemRef { key, .. } = iter.get().expect("missing item");
        assert_eq!(key, &TestKey(30..40));

        iter.advance().await.expect("advance failed");
        assert_eq!(iter.pending_iterators_len(), 0);

        let ItemRef { key, .. } = iter.get().expect("missing item");
        assert_eq!(key, &TestKey(40..50));

        iter.advance().await.expect("advance failed");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_excessive_hash_partitions_counter() {
        use std::sync::atomic::Ordering;

        let layer_0_items: Vec<_> = (0..100)
            .map(|i| {
                Item::new(
                    ObjectKey::extent(0, AttributeId::TEST_ID, i * 512..(i + 1) * 512),
                    ObjectValue::extent(0, VOLUME_DATA_KEY_ID),
                )
            })
            .collect();
        let layers: [Arc<dyn Layer<ObjectKey, ObjectValue>>; 1] =
            [write_layer(layer_0_items).await];
        let counters = counters();
        let mut merger =
            Merger::new(dyn_layer_ref_iter(&layers), object_store::merge::merge, counters.clone());

        // A query with <= MAX_HASH_PARTITIONS (e.g. 1 partition: 0..4096)
        merger
            .query(Query::LimitedRange(&ObjectKey::extent(0, AttributeId::TEST_ID, 0..4096)))
            .await
            .expect("seek failed");
        assert_eq!(counters.excessive_hash_partitions.load(Ordering::Relaxed), 0);

        let key_large = ObjectKey::extent(0, AttributeId::TEST_ID, 0..10485760);
        assert_eq!(layers[0].maybe_contains_key(&key_large), MaybeContainsKey::RangeKeyTooLarge);

        // A query with > MAX_HASH_PARTITIONS (e.g. 10MB range: 0..10485760 = 10 partitions)
        merger.query(Query::LimitedRange(&key_large)).await.expect("seek failed");
        assert_eq!(counters.excessive_hash_partitions.load(Ordering::Relaxed), 1);
    }
}
