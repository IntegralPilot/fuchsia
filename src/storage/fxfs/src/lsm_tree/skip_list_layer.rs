// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// There are a great many optimisations that could be considered to improve performance and maybe
// memory usage.

use crate::drop_event::DropEvent;
use crate::log::*;
use crate::lsm_tree::merge::{self, MergeFn};
use crate::lsm_tree::types::{
    BoxedLayerIterator, Existence, Item, ItemRef, Key, Layer, LayerIterator, LayerIteratorMut,
    LayerValue, OrdLowerBound, OrdUpperBound,
};
use crate::serialized_types::{LATEST_VERSION, Version};
use anyhow::{Error, bail};
use async_trait::async_trait;
use fuchsia_sync::{Mutex, MutexGuard};
use futures::future::BoxFuture;
use std::cmp::{Ordering, min};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicPtr, AtomicU32};

// Each skip list node contains a variable sized pointer list. The head pointers also exist in the
// form of a pointer list. Index 0 in the pointer list is the chain with the most elements i.e.
// contains every element in the list.
struct PointerList<K, V>(Box<[AtomicPtr<SkipListNode<K, V>>]>);

impl<K, V> PointerList<K, V> {
    fn new(count: usize) -> PointerList<K, V> {
        PointerList((0..count).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect())
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    // Extracts the pointer at the given index.
    fn get(&self, index: usize) -> Option<NonNull<SkipListNode<K, V>>> {
        NonNull::new(self.0[index].load(atomic::Ordering::SeqCst))
    }

    // Sets the pointer at the given index.
    fn set(&self, index: usize, node: Option<NonNull<SkipListNode<K, V>>>) {
        self.0[index]
            .store(node.map_or(std::ptr::null_mut(), |n| n.as_ptr()), atomic::Ordering::SeqCst);
    }
}

struct SkipListNode<K, V> {
    item: Item<K, V>,
    pointers: PointerList<K, V>,
}

pub struct SkipListLayer<K, V> {
    // These are the head pointers for the list.
    pointers: PointerList<K, V>,

    inner: Mutex<Inner<K, V>>,

    // Writes are locked using this lock.
    write_lock: Mutex<()>,

    // The number of nodes that have been allocated.  This is only used for debugging purposes.
    allocated: AtomicU32,

    close_event: Mutex<Option<Arc<DropEvent>>>,
}

// The writer needs to synchronize with the readers and this is done by keeping track of read
// counts.  We could, in theory, remove the mutex and make the read counts atomic (and thus make
// reads truly lock free) but it's simpler and easier to reason about with a mutex and what matters
// most is that we avoid using a futures::lock::Mutex for readers because that can be blocked for
// relatively long periods of time.
struct Inner<K, V> {
    // After a write, if there are nodes that need to be freed, and existing readers, the epoch
    // changes and new readers will be in a new epoch.  When all the old readers finish, the nodes
    // can be freed.
    epoch: u64,

    // The number of readers on the current epoch.
    current_count: u64,

    // A list of nodes to be freed once the read counts have reached zero.
    erase_lists: BTreeMap<u64, EpochEraseList<K, V>>,

    // The number of items in the skip-list.
    item_count: usize,
}

// After a mutation that involves erasing nodes, we must keep the nodes alive until there are no
// more readers in any of the epochs prior to the mutation.  To deal with this, we track the number
// of outstanding readers in each epoch so that when the count reaches zero, we know it is safe to
// free the nodes.
struct EpochEraseList<K, V> {
    // The number of readers still associated with this epoch.  When this reaches zero, the list can
    // be freed once all previous epochs have been freed.
    count: u64,
    // We represent the list by storing the head and tail of the list which each node chained to the
    // next.
    start: NonNull<SkipListNode<K, V>>,
    end: Option<NonNull<SkipListNode<K, V>>>,
}

// SAFETY: Required because of `erase_lists` which holds pointers.
unsafe impl<K, V> Send for Inner<K, V> {}

impl<K, V> Inner<K, V> {
    fn new() -> Self {
        Inner { epoch: 0, current_count: 0, erase_lists: BTreeMap::new(), item_count: 0 }
    }
    fn free_erase_list(
        &mut self,
        owner: &SkipListLayer<K, V>,
        start: NonNull<SkipListNode<K, V>>,
        end: Option<NonNull<SkipListNode<K, V>>>,
    ) {
        let mut node = start;
        loop {
            // SAFETY: This node has no more references.
            let next = unsafe { owner.free_node(node) };
            if next == end {
                break;
            }
            node = next.unwrap();
        }
    }
}

impl<K, V> SkipListLayer<K, V> {
    pub fn new(max_item_count: usize) -> Arc<SkipListLayer<K, V>> {
        Arc::new(SkipListLayer {
            pointers: PointerList::new((usize::BITS - max_item_count.leading_zeros()) as usize),
            inner: Mutex::new(Inner::new()),
            write_lock: Mutex::new(()),
            allocated: AtomicU32::new(0),
            close_event: Mutex::new(Some(Arc::new(DropEvent::new()))),
        })
    }

    pub fn len(&self) -> usize {
        self.inner.lock().item_count
    }

    fn alloc_node(&self, item: Item<K, V>, pointer_count: usize) -> Box<SkipListNode<K, V>> {
        self.allocated.fetch_add(1, atomic::Ordering::Relaxed);
        Box::new(SkipListNode { item, pointers: PointerList::new(pointer_count) })
    }

    // Frees and then returns the next node in the chain.
    //
    // # Safety
    //
    // The node must have no other references.
    unsafe fn free_node(
        &self,
        node: NonNull<SkipListNode<K, V>>,
    ) -> Option<NonNull<SkipListNode<K, V>>> {
        self.allocated.fetch_sub(1, atomic::Ordering::Relaxed);
        unsafe { Box::from_raw(node.as_ptr()).pointers.get(0) }
    }
}

impl<K: Eq + Key + OrdLowerBound, V: LayerValue> SkipListLayer<K, V> {
    // Erases the given item. Does nothing if the item doesn't exist.
    pub fn erase(&self, key: &K)
    where
        K: std::cmp::Eq,
    {
        let mut iter = SkipListLayerIterMut::new(self, Bound::Included(key));
        if let Some(ItemRef { key: k, .. }) = iter.get() {
            if k == key {
                iter.erase();
            } else {
                warn!("Attempt to erase key not present!");
            }
        }
        iter.commit();
    }

    /// Inserts the given item.
    pub fn insert(&self, item: Item<K, V>) -> Result<(), Error> {
        let mut iter = SkipListLayerIterMut::new(self, Bound::Included(&item.key));
        if let Some(found_item) = iter.get() {
            if found_item.key == &item.key {
                bail!("Attempted to insert an existing key");
            }
        }
        iter.insert(item);
        Ok(())
    }

    /// Replaces or inserts the given item.
    pub fn replace_or_insert(&self, item: Item<K, V>) {
        let mut iter = SkipListLayerIterMut::new(self, Bound::Included(&item.key));
        if let Some(found_item) = iter.get() {
            if found_item.key == &item.key {
                iter.erase();
            }
        }
        iter.insert(item);
    }

    /// Merges the item into the layer.
    pub fn merge_into(&self, item: Item<K, V>, lower_bound: &K, merge_fn: MergeFn<K, V>) {
        merge::merge_into(
            Box::new(SkipListLayerIterMut::new(self, Bound::Included(lower_bound))),
            item,
            merge_fn,
        )
        .unwrap();
    }
}

impl<K: OrdUpperBound, V> SkipListLayer<K, V> {
    /// Searches for a key. Bound::Excluded is not supported. Bound::Unbounded positions the
    /// iterator on the first item in the layer.
    ///
    /// This is an inherent version of the `Layer::seek` trait method which avoids returning a
    /// boxed future and boxed iterator.
    pub fn seek<'a>(&'a self, bound: Bound<&K>) -> SkipListLayerIter<'a, K, V> {
        SkipListLayerIter::new(self, bound)
    }
}

// We have to manually manage memory.
impl<K, V> Drop for SkipListLayer<K, V> {
    fn drop(&mut self) {
        let mut next = self.pointers.get(0);
        while let Some(node) = next {
            // SAFETY: The node has no more references.
            next = unsafe { self.free_node(node) };
        }
        assert_eq!(self.allocated.load(atomic::Ordering::Relaxed), 0);
    }
}

#[async_trait]
impl<K: Key, V: LayerValue> Layer<K, V> for SkipListLayer<K, V> {
    async fn seek<'a>(
        &'a self,
        bound: std::ops::Bound<&K>,
    ) -> Result<BoxedLayerIterator<'a, K, V>, Error> {
        Ok(Box::new(SkipListLayer::seek(self, bound)))
    }

    fn lock(&self) -> Option<Arc<DropEvent>> {
        self.close_event.lock().clone()
    }

    fn len(&self) -> usize {
        self.inner.lock().item_count
    }

    async fn close(&self) {
        let listener = self.close_event.lock().take().expect("close already called").listen();
        listener.await;
    }

    fn get_version(&self) -> Version {
        // The SkipListLayer is stored in RAM and written to disk as a SimplePersistentLayer
        // Hence, the SkipListLayer is always at the latest version
        return LATEST_VERSION;
    }

    fn record_inspect_data(self: Arc<Self>, node: &fuchsia_inspect::Node) {
        node.record_bool("persistent", false);
        node.record_uint("num_items", self.inner.lock().item_count as u64);
    }

    async fn key_exists(&self, key: &K) -> Result<Existence, Error> {
        let iter = SkipListLayer::seek(self, Bound::Included(key));
        Ok(iter.get().map_or(Existence::Missing, |i| {
            if i.key.cmp_upper_bound(key).is_eq() { Existence::Exists } else { Existence::Missing }
        }))
    }
}

// -- SkipListLayerIter --

pub struct SkipListLayerIter<'a, K, V> {
    skip_list: &'a SkipListLayer<K, V>,

    // The epoch for this reader.
    epoch: u64,

    // The current node.
    node: Option<NonNull<SkipListNode<K, V>>>,
}

// SAFETY: We need this for `node` which is safe to pass across threads.
unsafe impl<K, V> Send for SkipListLayerIter<'_, K, V> {}
unsafe impl<K, V> Sync for SkipListLayerIter<'_, K, V> {}

impl<'a, K: OrdUpperBound, V> SkipListLayerIter<'a, K, V> {
    fn new(skip_list: &'a SkipListLayer<K, V>, bound: Bound<&K>) -> Self {
        let epoch = {
            let mut inner = skip_list.inner.lock();
            inner.current_count += 1;
            inner.epoch
        };
        let (included, key) = match bound {
            Bound::Unbounded => {
                return SkipListLayerIter { skip_list, epoch, node: skip_list.pointers.get(0) };
            }
            Bound::Included(key) => (true, key),
            Bound::Excluded(key) => (false, key),
        };
        let mut last_pointers = &skip_list.pointers;

        // Some care needs to be taken here because new elements can be inserted atomically, so it
        // is important that the node we return in the iterator is the same node that we performed
        // the last comparison on.
        let mut node = None;
        for index in (0..skip_list.pointers.len()).rev() {
            // Keep iterating along this level until we encounter a key that's >= our search key.
            loop {
                node = last_pointers.get(index);
                if let Some(node) = node {
                    // SAFETY: `node` should be valid; we took a reference to the epoch above.
                    let node = unsafe { node.as_ref() };
                    match &node.item.key.cmp_upper_bound(key) {
                        Ordering::Equal if included => break,
                        Ordering::Greater => break,
                        _ => {}
                    }
                    last_pointers = &node.pointers;
                } else {
                    break;
                }
            }
        }
        SkipListLayerIter { skip_list, epoch, node }
    }
}

impl<K, V> Drop for SkipListLayerIter<'_, K, V> {
    fn drop(&mut self) {
        let mut inner = self.skip_list.inner.lock();
        if self.epoch == inner.epoch {
            inner.current_count -= 1;
        } else {
            if let Some(erase_list) = inner.erase_lists.get_mut(&self.epoch) {
                erase_list.count -= 1;
                if erase_list.count == 0 {
                    while let Some(entry) = inner.erase_lists.first_entry() {
                        if entry.get().count == 0 {
                            let EpochEraseList { start, end, .. } = entry.remove_entry().1;
                            inner.free_erase_list(self.skip_list, start, end);
                        } else {
                            break;
                        }
                    }
                }
            }
        }
    }
}

impl<K: Key, V: LayerValue> LayerIterator<K, V> for SkipListLayerIter<'_, K, V> {
    async fn advance(&mut self) -> Result<(), Error> {
        let _ = self.advance_dyn()?;
        Ok(())
    }

    fn advance_dyn<'a>(&'a mut self) -> Result<Option<BoxFuture<'a, Result<(), Error>>>, Error> {
        match self.node {
            None => {}
            Some(node) => {
                self.node = {
                    // SAFETY: `node` should be valid; we took a reference to the epoch in `new`.
                    unsafe { node.as_ref() }.pointers.get(0)
                }
            }
        }
        Ok(None)
    }

    fn get(&self) -> Option<ItemRef<'_, K, V>> {
        // SAFETY: `node` should be valid; we took a reference to the epoch in `new`.
        self.node.map(|node| unsafe { node.as_ref() }.item.as_item_ref())
    }
}

type PointerListRefArray<'a, K, V> = Box<[&'a PointerList<K, V>]>;

// -- SkipListLayerIterMut --

// This works by building an insertion chain.  When that chain is committed, it is done atomically
// so that readers are not interrupted.  When the existing readers are finished, it is then safe to
// release memory for any nodes that might have been erased.  In the case that we are only erasing
// elements, there will be no insertion chain, in which case we just atomically remove the elements
// from the chain.
pub struct SkipListLayerIterMut<'a, K: Key, V: LayerValue> {
    skip_list: &'a SkipListLayer<K, V>,

    // Since this is a mutable iterator, we need to keep pointers to all the nodes that precede the
    // current position at every level, so that we can update them when inserting or erasing
    // elements.
    prev_pointers: PointerListRefArray<'a, K, V>,

    // When we first insert or erase an element, we take a copy of prev_pointers so that
    // we know which pointers need to be updated when we commit.
    insertion_point: Option<PointerListRefArray<'a, K, V>>,

    // These are the nodes that we should point to when we commit.
    insertion_nodes: PointerList<K, V>,

    // Only one write can proceed at a time.  We only need a place to keep the mutex guard, which is
    // why Rust thinks this is unused.
    #[allow(dead_code)]
    write_guard: MutexGuard<'a, ()>,

    // The change in item count as a result of this mutation.
    item_delta: isize,
}

impl<'a, K: Key, V: LayerValue> SkipListLayerIterMut<'a, K, V> {
    pub fn new(skip_list: &'a SkipListLayer<K, V>, bound: std::ops::Bound<&K>) -> Self {
        let write_guard = skip_list.write_lock.lock();
        let len = skip_list.pointers.len();

        // Start by setting all the previous pointers to the head.
        //
        // To understand how the previous pointers work, imagine the list looks something like the
        // following:
        //
        // 2  |--->|
        // 1  |--->|--|------->|
        // 0  |--->|--|--|--|->|
        //  HEAD   A  B  C  D  E  F
        //
        // Now imagine that the iterator is pointing at element D. In that case, the previous
        // pointers will point at C for index 0, B for index 1 and A for index 2. With that
        // information, it will be possible to insert an element immediately prior to D and
        // correctly update as many pointers as required (remember a new element will be given a
        // random number of levels).
        let mut prev_pointers = vec![&skip_list.pointers; len].into_boxed_slice();
        match bound {
            Bound::Unbounded => {}
            Bound::Included(key) => {
                let pointers = &mut prev_pointers;
                for index in (0..len).rev() {
                    while let Some(node) = pointers[index].get(index) {
                        // Keep iterating along this level until we encounter a key that's >= our
                        // search key.

                        // SAFETY: `node` should be valid; a write guard was taken above so nodes
                        // in the current epoch cannot be erased.
                        let node = unsafe { node.as_ref() };

                        match node.item.key.cmp_upper_bound(key) {
                            Ordering::Equal | Ordering::Greater => break,
                            Ordering::Less => {}
                        }
                        pointers[index] = &node.pointers;
                    }
                    if index > 0 {
                        pointers[index - 1] = pointers[index];
                    }
                }
            }
            Bound::Excluded(_) => panic!("Excluded bounds not supported"),
        }
        SkipListLayerIterMut {
            skip_list,
            prev_pointers,
            insertion_point: None,
            insertion_nodes: PointerList::new(len),
            write_guard,
            item_delta: 0,
        }
    }
}

impl<K: Key, V: LayerValue> Drop for SkipListLayerIterMut<'_, K, V> {
    fn drop(&mut self) {
        self.commit();
    }
}

impl<K: Key, V: LayerValue> LayerIteratorMut<K, V> for SkipListLayerIterMut<'_, K, V> {
    fn advance(&mut self) {
        if self.insertion_point.is_some() {
            if let Some(item) = self.get() {
                // Copy the current item into the insertion chain.
                let copy = item.cloned();
                self.insert(copy);
                self.erase();
            }
        } else {
            let pointers = &mut self.prev_pointers;
            if let Some(next) = pointers[0].get(0) {
                // SAFETY: `node` should be valid; a write guard was taken above so nodes
                // in the current epoch cannot be erased.
                let next = unsafe { next.as_ref() };
                for i in 0..next.pointers.len() {
                    pointers[i] = &next.pointers;
                }
            }
        }
    }

    fn get(&self) -> Option<ItemRef<'_, K, V>> {
        // SAFETY: `node` should be valid; a write guard was taken above so nodes in the current
        // epoch cannot be erased.
        self.prev_pointers[0].get(0).map(|node| unsafe { node.as_ref() }.item.as_item_ref())
    }

    fn insert(&mut self, item: Item<K, V>) {
        use rand::RngExt as _;
        let mut rng = rand::rng();
        let max_pointers = self.skip_list.pointers.len();
        // This chooses a random number of pointers such that each level has half the number of
        // pointers of the previous one.
        let pointer_count = min(1 + rng.random::<u32>().trailing_zeros() as usize, max_pointers);
        let node = Box::leak(self.skip_list.alloc_node(item, pointer_count));
        if self.insertion_point.is_none() {
            self.insertion_point = Some(self.prev_pointers.clone());
        }
        let node_ptr = node.into();
        for i in 0..pointer_count {
            let pointers = self.prev_pointers[i];
            node.pointers.set(i, pointers.get(i));
            if self.insertion_nodes.get(i).is_none() {
                // If there's no insertion node at this level, record this node as the node to
                // switch in when we commit.
                self.insertion_nodes.set(i, Some(node_ptr));
            } else {
                // There's already an insertion node at this level which means that it's part of the
                // insertion chain, so we can just update the pointers now.
                pointers.set(i, Some(node_ptr));
            }
            // The iterator should point at the node following the new node i.e. the existing node.
            self.prev_pointers[i] = &node.pointers;
        }
        self.item_delta += 1;
    }

    fn erase(&mut self) {
        let pointers = &mut self.prev_pointers;
        if let Some(next) = pointers[0].get(0) {
            // SAFETY: `next` should be valid; a write guard was taken above so nodes in the current
            // epoch cannot be erased.
            let next = unsafe { next.as_ref() };
            if self.insertion_point.is_none() {
                self.insertion_point = Some(pointers.clone());
            }
            if self.insertion_nodes.get(0).is_none() {
                // If there's no insertion node, then just update the iterator position to point to
                // the next node, and then when we commit, it'll get erased.
                pointers[0] = &next.pointers;
            } else {
                // There's an insertion node, so the current element must be part of the insertion
                // chain and so we can update the pointers immediately.  There will be another node
                // that isn't part of the insertion chain that will still point at this node, but it
                // will disappear when we commit.
                pointers[0].set(0, next.pointers.get(0));
            }
            // Fix up all the pointers except the bottom one. Readers will still find this node,
            // just not as efficiently.
            for i in 1..next.pointers.len() {
                pointers[i].set(i, next.pointers.get(i));
            }
        }
        self.item_delta -= 1;
    }

    // Commits the changes.  Note that this doesn't wait for readers to finish; any barrier that be
    // required should be handled by the caller.
    fn commit(&mut self) {
        // Splice the changes into the list.
        let prev_pointers = match self.insertion_point.take() {
            Some(prev_pointers) => prev_pointers,
            None => return,
        };

        // Keep track of the first node that we might need to erase later.
        let maybe_erase = prev_pointers[0].get(0);

        // If there are no insertion nodes, then it means that we're only erasing nodes.
        if self.insertion_nodes.get(0).is_none() {
            // Erase all elements between the insertion point and the current element. The
            // pointers for levels > 0 should already have been done, so it's only level 0 we
            // need to worry about.
            prev_pointers[0].set(0, self.prev_pointers[0].get(0));
        } else {
            // Switch the pointers over so that the insertion chain is spliced in.  This is safe
            // so long as the bottom pointer is done first because that guarantees the new nodes
            // will be found, just maybe not as efficiently.
            for i in 0..self.insertion_nodes.len() {
                if let Some(node) = self.insertion_nodes.get(i) {
                    prev_pointers[i].set(i, Some(node));
                }
            }
        }

        // Switch the epoch so that we can track when existing readers have finished.
        let mut inner = self.skip_list.inner.lock();
        inner.item_count = inner.item_count.checked_add_signed(self.item_delta).unwrap();
        if let Some(start) = maybe_erase {
            let end = self.prev_pointers[0].get(0);
            if maybe_erase != end {
                if inner.current_count > 0 || !inner.erase_lists.is_empty() {
                    let count = std::mem::take(&mut inner.current_count);
                    let epoch = inner.epoch;
                    inner.erase_lists.insert(epoch, EpochEraseList { count, start, end });
                    inner.epoch = inner.epoch.wrapping_add(1);
                } else {
                    inner.free_erase_list(self.skip_list, start, end);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SkipListLayer, SkipListLayerIterMut};
    use crate::lsm_tree::merge::ItemOp::{Discard, Replace};
    use crate::lsm_tree::merge::{MergeLayerIterator, MergeResult};
    use crate::lsm_tree::skip_list_layer::SkipListLayerIter;
    use crate::lsm_tree::types::{
        DefaultOrdLowerBound, DefaultOrdUpperBound, Existence, FuzzyHash, Item, ItemRef, Layer,
        LayerIterator, LayerIteratorMut, SortByU64,
    };
    use crate::serialized_types::{
        LATEST_VERSION, Version, Versioned, VersionedLatest, versioned_type,
    };
    use assert_matches::assert_matches;
    use fprint::TypeFingerprint;
    use fuchsia_async as fasync;
    use futures::future::join_all;
    use futures::{FutureExt as _, join};
    use fxfs_macros::{FuzzyHash, SerializeKey};
    use std::hash::Hash;
    use std::ops::Bound;
    use std::time::{Duration, Instant};

    #[derive(
        Clone,
        Eq,
        Debug,
        Hash,
        FuzzyHash,
        PartialEq,
        PartialOrd,
        Ord,
        serde::Serialize,
        serde::Deserialize,
        TypeFingerprint,
        Versioned,
        SerializeKey,
    )]
    struct TestKey(u64);

    versioned_type! { 1.. => TestKey }

    impl SortByU64 for TestKey {
        fn get_leading_u64(&self) -> u64 {
            self.0
        }
    }

    impl DefaultOrdLowerBound for TestKey {}
    impl DefaultOrdUpperBound for TestKey {}

    #[fuchsia::test]
    async fn test_key_exists() {
        let skip_list = SkipListLayer::new(100);
        skip_list.insert(Item::new(TestKey(1), 1)).expect("insert error");
        skip_list.insert(Item::new(TestKey(3), 3)).expect("insert error");

        assert_eq!(
            skip_list.key_exists(&TestKey(0)).await.expect("key_exists failed"),
            Existence::Missing
        );
        assert_eq!(
            skip_list.key_exists(&TestKey(1)).await.expect("key_exists failed"),
            Existence::Exists
        );
        assert_eq!(
            skip_list.key_exists(&TestKey(2)).await.expect("key_exists failed"),
            Existence::Missing
        );
        assert_eq!(
            skip_list.key_exists(&TestKey(3)).await.expect("key_exists failed"),
            Existence::Exists
        );
        assert_eq!(
            skip_list.key_exists(&TestKey(4)).await.expect("key_exists failed"),
            Existence::Missing
        );
    }

    #[fuchsia::test]
    async fn test_iteration() {
        // Insert two items and make sure we can iterate back in the correct order.
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[1].clone()).expect("insert error");
        skip_list.insert(items[0].clone()).expect("insert error");
        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_seek_exact() {
        // Seek for an exact match.
        let skip_list = SkipListLayer::new(100);
        for i in (0..100).rev() {
            skip_list.insert(Item::new(TestKey(i), i)).expect("insert error");
        }
        let mut iter = skip_list.seek(Bound::Included(&TestKey(57)));
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(57), &57));

        // And check the next item is correct.
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(58), &58));
    }

    #[fuchsia::test]
    async fn test_seek_lower_bound() {
        // Seek for a non-exact match.
        let skip_list = SkipListLayer::new(100);
        for i in (0..100).rev() {
            skip_list.insert(Item::new(TestKey(i * 3), i * 3)).expect("insert error");
        }
        let mut expected_index = 57 * 3;
        let mut iter = skip_list.seek(Bound::Included(&TestKey(expected_index - 1)));
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(expected_index), &expected_index));

        // And check the next item is correct.
        expected_index += 3;
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(expected_index), &expected_index));
    }

    #[fuchsia::test]
    async fn test_replace_or_insert_replaces() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[1].clone()).expect("insert error");
        skip_list.insert(items[0].clone()).expect("insert error");
        let replacement_value = 3;
        skip_list.replace_or_insert(Item::new(items[1].key.clone(), replacement_value));

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &replacement_value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_replace_or_insert_inserts() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2), Item::new(TestKey(3), 3)];
        skip_list.insert(items[2].clone()).expect("insert error");
        skip_list.insert(items[0].clone()).expect("insert error");
        skip_list.replace_or_insert(items[1].clone());

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
    async fn test_erase() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[1].clone()).expect("insert error");
        skip_list.insert(items[0].clone()).expect("insert error");

        assert_eq!(skip_list.len(), 2);

        skip_list.erase(&items[1].key);

        assert_eq!(skip_list.len(), 1);

        {
            let mut iter = skip_list.seek(Bound::Unbounded);
            let ItemRef { key, value, .. } = iter.get().expect("missing item");
            assert_eq!((key, value), (&items[0].key, &items[0].value));
            iter.advance().await.unwrap();
            assert!(iter.get().is_none());
        }

        skip_list.erase(&items[0].key);

        assert_eq!(skip_list.len(), 0);

        {
            let iter = skip_list.seek(Bound::Unbounded);
            assert!(iter.get().is_none());
        }
    }

    // This test ends up being flaky on CQ. It is left here as it might be useful in case
    // significant changes are made.
    #[fuchsia::test]
    #[ignore]
    async fn test_seek_is_log_n_complexity() {
        // Keep doubling up the number of items until it takes about 500ms to search and then go
        // back and measure something that should, in theory, take about half that time.
        let mut n = 100;
        let mut loops = 0;
        const TARGET_TIME: Duration = Duration::from_millis(500);
        let time = loop {
            let skip_list = SkipListLayer::new(n as usize);
            for i in 0..n {
                skip_list.insert(Item::new(TestKey(i), i)).expect("insert error");
            }
            let start = Instant::now();
            for i in 0..n {
                skip_list.seek(Bound::Included(&TestKey(i)));
            }
            let elapsed = Instant::now() - start;
            if elapsed > TARGET_TIME {
                break elapsed;
            }
            n *= 2;
            loops += 1;
        };

        let seek_count = n;
        n >>= loops / 2; // This should, in theory, result in 50% seek time.
        let skip_list = SkipListLayer::new(n as usize);
        for i in 0..n {
            skip_list.insert(Item::new(TestKey(i), i)).expect("insert error");
        }
        let start = Instant::now();
        for i in 0..seek_count {
            skip_list.seek(Bound::Included(&TestKey(i)));
        }
        let elapsed = Instant::now() - start;

        eprintln!(
            "{} items: {}ms, {} items: {}ms",
            seek_count,
            time.as_millis(),
            n,
            elapsed.as_millis()
        );

        // Experimental results show that typically we do a bit better than log(n), but here we just
        // check that the time we just measured is above 25% of the time we first measured, the
        // theory suggests it should be around 50%.
        assert!(elapsed * 4 > time);
    }

    #[fuchsia::test]
    async fn test_large_number_of_items() {
        let item_count = 1000;
        let skip_list = SkipListLayer::new(1000);
        for i in 1..item_count {
            skip_list.insert(Item::new(TestKey(i), 1)).expect("insert error");
        }
        let mut iter = skip_list.seek(Bound::Included(&TestKey(item_count - 10)));
        for i in item_count - 10..item_count {
            assert_eq!(iter.get().expect("missing item").key, &TestKey(i));
            iter.advance().await.unwrap();
        }
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_multiple_readers_allowed() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[1].clone()).expect("insert error");
        skip_list.insert(items[0].clone()).expect("insert error");

        // Create the first iterator and check the first item.
        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));

        // Create a second iterator and check the first item.
        let iter2 = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter2.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));

        // Now go back to the first iterator and check the second item.
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
    }

    fn merge(
        left: &'_ MergeLayerIterator<'_, TestKey, i32>,
        right: &'_ MergeLayerIterator<'_, TestKey, i32>,
    ) -> MergeResult<TestKey, i32> {
        MergeResult::Other {
            emit: None,
            left: Replace(Item::new((*left.key()).clone(), *left.value() + *right.value()).boxed()),
            right: Discard,
        }
    }

    #[fuchsia::test]
    async fn test_merge_into() {
        let skip_list = SkipListLayer::new(100);
        skip_list.insert(Item::new(TestKey(1), 1)).expect("insert error");

        skip_list.merge_into(Item::new(TestKey(2), 2), &TestKey(1), merge);

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&TestKey(1), &3));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_two_inserts() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        {
            let mut iter = SkipListLayerIterMut::new(&skip_list, std::ops::Bound::Unbounded);
            iter.insert(items[0].clone());
            iter.insert(items[1].clone());
        }

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
    }

    #[fuchsia::test]
    async fn test_erase_after_insert() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[1].clone()).expect("insert error");
        {
            let mut iter = SkipListLayerIterMut::new(&skip_list, std::ops::Bound::Unbounded);
            iter.insert(items[0].clone());
            iter.erase();
        }

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_insert_after_erase() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[1].clone()).expect("insert error");
        {
            let mut iter = SkipListLayerIterMut::new(&skip_list, std::ops::Bound::Unbounded);
            iter.erase();
            iter.insert(items[0].clone());
        }

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_insert_erase_insert() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2), Item::new(TestKey(3), 3)];
        skip_list.insert(items[0].clone()).expect("insert error");
        {
            let mut iter = SkipListLayerIterMut::new(&skip_list, std::ops::Bound::Unbounded);
            iter.insert(items[1].clone());
            iter.erase();
            iter.insert(items[2].clone());
        }

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[2].key, &items[2].value));
    }

    #[fuchsia::test]
    async fn test_two_erase_erases() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2), Item::new(TestKey(3), 3)];
        skip_list.insert(items[0].clone()).expect("insert error");
        skip_list.insert(items[1].clone()).expect("insert error");
        skip_list.insert(items[2].clone()).expect("insert error");
        {
            let mut iter = SkipListLayerIterMut::new(&skip_list, std::ops::Bound::Unbounded);
            iter.erase();
            iter.erase();
        }

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[2].key, &items[2].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_readers_not_blocked_by_writers() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[1].clone()).expect("insert error");

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));

        let mut iter2 = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter2.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));

        join!(async { skip_list.insert(items[0].clone()).expect("insert error") }, async {
            loop {
                let iter = skip_list.seek(Bound::Unbounded);
                let ItemRef { key, .. } = iter.get().expect("missing item");
                if key == &items[0].key {
                    break;
                }
            }
            iter.advance().await.unwrap();
            assert!(iter.get().is_none());
            std::mem::drop(iter);
            iter2.advance().await.unwrap();
            assert!(iter2.get().is_none());
            std::mem::drop(iter2);
        });
    }

    #[fuchsia::test(threads = 20)]
    async fn test_many_readers_and_writers() {
        let skip_list = SkipListLayer::new(100);
        join_all(
            (0..10)
                .map(|i| {
                    let skip_list_clone = skip_list.clone();
                    fasync::Task::spawn(async move {
                        for j in 0..10 {
                            skip_list_clone
                                .insert(Item::new(TestKey(i * 100 + j), i))
                                .expect("insert error");
                        }
                    })
                })
                .chain((0..10).map(|_| {
                    let skip_list_clone = skip_list.clone();
                    fasync::Task::spawn(async move {
                        for _ in 0..300 {
                            let mut iter = skip_list_clone.seek(Bound::Unbounded);
                            let mut last_item: Option<TestKey> = None;
                            while let Some(item) = iter.get() {
                                if let Some(last) = last_item {
                                    assert!(item.key > &last);
                                }
                                last_item = Some(item.key.clone());
                                iter.advance().await.expect("advance failed");
                            }
                        }
                    })
                })),
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_insert_advance_erase() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2), Item::new(TestKey(3), 3)];
        skip_list.insert(items[1].clone()).expect("insert error");
        skip_list.insert(items[2].clone()).expect("insert error");

        assert_eq!(skip_list.len(), 2);

        {
            let mut iter = SkipListLayerIterMut::new(&skip_list, std::ops::Bound::Unbounded);
            iter.insert(items[0].clone());
            iter.advance();
            iter.erase();
        }

        assert_eq!(skip_list.len(), 2);

        let mut iter = skip_list.seek(Bound::Unbounded);
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[0].key, &items[0].value));
        iter.advance().await.unwrap();
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_seek_excluded() {
        let skip_list = SkipListLayer::new(100);
        let items = [Item::new(TestKey(1), 1), Item::new(TestKey(2), 2)];
        skip_list.insert(items[0].clone()).expect("insert error");
        skip_list.insert(items[1].clone()).expect("insert error");
        let iter = skip_list.seek(Bound::Excluded(&items[0].key));
        let ItemRef { key, value, .. } = iter.get().expect("missing item");
        assert_eq!((key, value), (&items[1].key, &items[1].value));
    }

    #[fuchsia::test]
    fn test_insert_race() {
        for _ in 0..1000 {
            let skip_list = SkipListLayer::new(100);
            skip_list.insert(Item::new(TestKey(2), 2)).expect("insert error");

            let skip_list_clone = skip_list.clone();
            let thread1 = std::thread::spawn(move || {
                skip_list_clone.insert(Item::new(TestKey(1), 1)).expect("insert error")
            });
            let thread2 = std::thread::spawn(move || {
                let iter = SkipListLayerIter::new(&skip_list, Bound::Included(&TestKey(2)));
                match iter.get() {
                    Some(ItemRef { key: TestKey(2), .. }) => {}
                    result => assert!(false, "{:?}", result),
                }
            });
            thread1.join().unwrap();
            thread2.join().unwrap();
        }
    }

    #[fuchsia::test]
    fn test_replace_or_insert_multi_thread() {
        let skip_list = SkipListLayer::new(100);
        skip_list.insert(Item::new(TestKey(1), 1)).expect("insert error");
        skip_list.insert(Item::new(TestKey(2), 2)).expect("insert error");
        skip_list.insert(Item::new(TestKey(3), 3)).expect("insert error");
        skip_list.insert(Item::new(TestKey(4), 4)).expect("insert error");

        // Set up a number of threads that are repeatedly replacing the '3' key.
        let mut threads = Vec::new();
        for i in 0..200 {
            let skip_list_clone = skip_list.clone();
            threads.push(std::thread::spawn(move || {
                skip_list_clone.replace_or_insert(Item::new(TestKey(3), i));
            }));
        }

        // Have one thread repeatedly checking the list.
        let _checker_thread = std::thread::spawn(move || {
            loop {
                let mut iter = SkipListLayerIter::new(&skip_list, Bound::Included(&TestKey(2)));
                assert_matches!(iter.get(), Some(ItemRef { key: TestKey(2), .. }));
                iter.advance().now_or_never().unwrap().unwrap();
                assert_matches!(iter.get(), Some(ItemRef { key: TestKey(3), .. }));
                iter.advance().now_or_never().unwrap().unwrap();
                assert_matches!(iter.get(), Some(ItemRef { key: TestKey(4), .. }));
            }
        });

        for thread in threads {
            thread.join().unwrap();
        }
    }
}
