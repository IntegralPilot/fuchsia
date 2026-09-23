// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::file::FxFile;
use fuchsia_sync::Mutex;
use futures::future::poll_fn;
use fxfs::object_store::ObjectDescriptor;
use fxfs_macros::ToWeakNode;
use std::any::TypeId;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Weak};
use std::task::{Poll, Waker};
use vfs::common::IntoAny;

/// FxNode is a node in the filesystem hierarchy (either a file or directory).
pub trait FxNode: IntoAny + ToWeakNode + Send + Sync + 'static {
    fn object_id(&self) -> u64;
    fn parent(&self) -> Option<Arc<FxDirectory>>;
    fn set_parent(&self, parent: Arc<FxDirectory>);
    fn open_count_add_one(&self);

    /// Atomically check if this brought the count to zero while the node is
    /// marked for purge. If so, this *must* queue the node for tombstone the node.
    fn open_count_sub_one(self: Arc<Self>);
    fn object_descriptor(&self) -> ObjectDescriptor;

    /// Marks the object to be purged. Queues the node for tombstone if open count is zero.
    fn mark_to_be_purged(self: Arc<Self>) {
        panic!("Unexpected call to mark_to_be_purged");
    }

    /// Called when the filesystem is shutting down. Implementations should break any strong
    /// reference cycles that would prevent the node from being dropped.
    fn terminate(&self) {}
}

struct PlaceholderInner {
    object_id: u64,
    waker_sequence: u64,
    wakers: Vec<Waker>,
}

#[derive(ToWeakNode)]
struct Placeholder(Mutex<PlaceholderInner>);

impl FxNode for Placeholder {
    fn object_id(&self) -> u64 {
        self.0.lock().object_id
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

impl fmt::Debug for dyn FxNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FxNode")
            .field("id", &self.object_id())
            .field("descriptor", &self.object_descriptor())
            .finish()
    }
}

/// PlaceholderOwner is a reserved slot in the node cache.
pub struct PlaceholderOwner<'a> {
    inner: Arc<Placeholder>,
    committed: bool,
    cache: &'a NodeCache,
}

impl PlaceholderOwner<'_> {
    /// Commits a node to the cache, replacing the placeholder and unblocking any waiting callers.
    pub fn commit(mut self, node: &Arc<dyn FxNode>) {
        let this_object_id = self.inner.object_id();
        assert_eq!(node.object_id(), this_object_id);
        self.committed = true;
        self.cache.commit(node);
    }
}

impl Drop for PlaceholderOwner<'_> {
    fn drop(&mut self) {
        let wakers = {
            let mut cache_guard = (!self.committed).then(|| self.cache.0.lock());
            let mut p = self.inner.0.lock();
            if let Some(cache) = &mut cache_guard {
                // If the placeholder is dropped before it was committed, remove the cache entry
                // so that another caller blocked in NodeCache::get_or_reserve can take the slot.
                cache.map.remove(&p.object_id);
            }
            std::mem::take(&mut p.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

/// See NodeCache::get_or_reserve.
pub enum GetResult<'a> {
    Placeholder(PlaceholderOwner<'a>),
    Node(Arc<dyn FxNode>),
}

impl<'a> GetResult<'a> {
    pub fn placeholder(self) -> Option<PlaceholderOwner<'a>> {
        match self {
            GetResult::Placeholder(placeholder) => Some(placeholder),
            _ => None,
        }
    }
}

/// WeakNode permits `upgrade_and_downcast_node` below which only tries to upgrade if we know the
/// downcast will work.  This makes for a simpler and more performant implementation of `FileIter`
/// which would otherwise have to jump through some hoops to drop the reference count whilst not
/// holding locks if the downcast fails.
pub struct WeakNode {
    vtable: &'static WeakNodeVTable,
    node: *const (),
}

impl WeakNode {
    // This is unsafe because the caller must make sure `node` comes from `Weak<T>::into_raw` and
    // provide correct implementations for the functions in `vtable`.
    pub(crate) unsafe fn new(vtable: &'static WeakNodeVTable, node: *const ()) -> Self {
        Self { vtable, node }
    }

    unsafe fn is_type<T: 'static>(&self) -> bool {
        unsafe { (self.vtable.type_id)() == TypeId::of::<T>() }
    }
}

impl Drop for WeakNode {
    fn drop(&mut self) {
        // SAFETY: See the implementation of `ToWeakNode` in `macros.rs`.  `node` should be a
        // pointer that came from `Weak<T>::into_raw`.  The `drop` function needs to use
        // `Weak::from_raw`.
        unsafe {
            (self.vtable.drop)(self.node);
        }
    }
}

// SAFETY: `node` comes from `Weak<T>::into_raw` which is safe to send across threads.
unsafe impl Send for WeakNode {}

pub(crate) struct WeakNodeVTable {
    // This should convert the pointer back to `Weak<T>` to drop it.
    drop: unsafe fn(*const ()),

    // Returns the `TypeId::of::<T>` for `Weak<T>`.
    type_id: unsafe fn() -> TypeId,

    // Tries to upgrade the `Weak<T>` and returns `Arc<dyn FxNode>`.
    upgrade: unsafe fn(*const ()) -> Option<Arc<dyn FxNode>>,
}

impl WeakNodeVTable {
    pub(crate) const fn new(
        drop: unsafe fn(*const ()),
        type_id: unsafe fn() -> TypeId,
        upgrade: unsafe fn(*const ()) -> Option<Arc<dyn FxNode>>,
    ) -> Self {
        Self { drop, type_id, upgrade }
    }
}

/// Used to convert nodes into `WeakNode` which is stored in the cache.  This should be implemented
/// using the `ToWeakNode` derive macro.
pub trait ToWeakNode {
    fn to_weak_node(self: Arc<Self>) -> WeakNode;
}

/// Upgrades and downcasts as a single step.  This won't do the upgrade if the downcast will fail,
/// which avoids issues with dropping the reference whilst locks are held.
fn upgrade_and_downcast_node<T: 'static>(weak_node: &WeakNode) -> Option<Arc<T>> {
    // SAFETY: We check `T` matches before converting the pointer (which should be the result of
    // `Weak<T>::into_raw`), back to `Weak<T>`.
    unsafe {
        if weak_node.is_type::<T>() {
            ManuallyDrop::new(Weak::from_raw(weak_node.node as *const T)).upgrade()
        } else {
            None
        }
    }
}

/// Upgrades to `Arc<dyn FxNode>`.
fn upgrade_node(weak_node: &WeakNode) -> Option<Arc<dyn FxNode>> {
    // SAFETY: Safe if `WeakNode::new` is called correctly and the `upgrade` function is implemented
    // correctly.  See the implementation in `macros.rs`.
    unsafe { (weak_node.vtable.upgrade)(weak_node.node) }
}

struct NodeCacheInner {
    map: BTreeMap<u64, WeakNode>,
    next_waker_sequence: u64,
}

/// NodeCache is an in-memory cache of weak node references.
pub struct NodeCache(Mutex<NodeCacheInner>);

/// Iterates over all files in the cache (skipping directories).
pub struct FileIter<'a> {
    cache: &'a NodeCache,
    object_id: Option<u64>,
}

impl<'a> Iterator for FileIter<'a> {
    type Item = Arc<FxFile>;
    fn next(&mut self) -> Option<Self::Item> {
        let cache = self.cache.0.lock();
        let range = match self.object_id {
            None => cache.map.range(0..),
            Some(oid) => cache.map.range(oid + 1..),
        };
        for (object_id, node) in range {
            if let Some(file) = upgrade_and_downcast_node(node) {
                self.object_id = Some(*object_id);
                return Some(file);
            }
        }
        None
    }
}

impl NodeCache {
    pub fn new() -> Self {
        Self(Mutex::new(NodeCacheInner { map: BTreeMap::new(), next_waker_sequence: 0 }))
    }

    /// Gets a node in the cache, or reserves a placeholder in the cache to fill.
    ///
    /// Only the first caller will receive a placeholder result; all callers after that will block
    /// until the placeholder is filled (or the placeholder is dropped, at which point the next
    /// caller would get a placeholder). Callers that receive a placeholder should later commit a
    /// node with NodeCache::commit.
    pub async fn get_or_reserve<'a>(&'a self, object_id: u64) -> GetResult<'a> {
        let mut waker_sequence = 0;
        let mut waker_index = 0;
        poll_fn(|cx| {
            let mut this = self.0.lock();
            if let Some(node) = this.map.get(&object_id) {
                if let Some(node) = upgrade_node(node) {
                    if let Ok(placeholder) = node.clone().into_any().downcast::<Placeholder>() {
                        let mut inner = placeholder.0.lock();
                        if inner.waker_sequence == waker_sequence {
                            inner.wakers[waker_index] = cx.waker().clone();
                        } else {
                            waker_index = inner.wakers.len();
                            waker_sequence = inner.waker_sequence;
                            inner.wakers.push(cx.waker().clone());
                        }
                        return Poll::Pending;
                    } else {
                        return Poll::Ready(GetResult::Node(node));
                    }
                }
            }
            this.next_waker_sequence += 1;
            let inner = Arc::new(Placeholder(Mutex::new(PlaceholderInner {
                object_id,
                waker_sequence: this.next_waker_sequence,
                wakers: vec![],
            })));
            this.map.insert(object_id, inner.clone().to_weak_node());
            Poll::Ready(GetResult::Placeholder(PlaceholderOwner {
                inner,
                committed: false,
                cache: self,
            }))
        })
        .await
    }

    /// Removes a node from the cache. Calling this on a placeholder is an error; instead, the
    /// placeholder should simply be dropped.
    pub fn remove(&self, node: &dyn FxNode) {
        let mut this = self.0.lock();
        if let Entry::Occupied(o) = this.map.entry(node.object_id()) {
            // If this method is called when a node is being dropped, then upgrade will fail and
            // it's possible the cache has been populated with another node, so to avoid that race,
            // we must check that the node in the cache is the node we want to remove.
            //
            // Note this ugly cast in place of `std::ptr::eq(o.get().as_ptr(), node)` here is
            // to ensure we don't compare vtable pointers, which are not strictly guaranteed to be
            // the same across casts done in different code generation units at compilation time.
            if o.get().node == node as *const dyn FxNode as *const () {
                o.remove();
            }
        }
    }

    /// Returns the given node if present in the cache. This call should be handled with care. If it
    /// is being used to check if there is no live node version, then there should be precautions
    /// taken to ensure one will not be created in a race with this call or any of the resulting
    /// actions.
    pub fn get(&self, object_id: u64) -> Option<Arc<dyn FxNode>> {
        let l = self.0.lock();
        let weak_node = l.map.get(&object_id)?;
        // Don't return placeholders as valid nodes. Panicking here, as this is a result of a race.
        // If it happens we're likely breaking an assumption, but should not be permanent for the
        // filesystem. Better to remount.
        assert!(
            unsafe { !weak_node.is_type::<Placeholder>() },
            "Returning a placeholder indicates a race with open"
        );
        upgrade_node(weak_node)
    }

    /// Returns true if an entry exists in the cache for the given `object_id`.
    ///
    /// Note that this returns true if the node is currently open, if it is being loaded (i.e. has
    /// a `Placeholder`), or if there is a weak entry with zero strong references because another
    /// thread is in the middle of dropping the node and removing it from the cache.
    pub fn contains_key(&self, object_id: u64) -> bool {
        self.0.lock().map.contains_key(&object_id)
    }

    /// Returns an iterator over all files in the cache.
    pub fn files(&self) -> FileIter<'_> {
        FileIter { cache: self, object_id: None }
    }

    pub fn terminate(&self) {
        let _drop_list = {
            let this = self.0.lock();
            let mut drop_list = Vec::with_capacity(this.map.len());
            for (_, node) in &this.map {
                if let Some(node) = upgrade_node(&node) {
                    node.terminate();
                    // We must drop later when we're not holding the lock.
                    drop_list.push(node);
                }
            }
            drop_list
        };
    }

    fn commit(&self, node: &Arc<dyn FxNode>) {
        let mut this = self.0.lock();
        this.map.insert(node.object_id(), node.clone().to_weak_node());
    }
}

// Wraps a node with an open count.
pub struct OpenedNode<N: FxNode + ?Sized>(pub Arc<N>);

impl<N: FxNode + ?Sized> OpenedNode<N> {
    pub fn new(node: Arc<N>) -> Self {
        node.open_count_add_one();
        OpenedNode(node)
    }

    /// Downcasts to something that implements FxNode.
    pub fn downcast<T: FxNode>(self) -> Result<OpenedNode<T>, Self> {
        if self.is::<T>() {
            Ok(OpenedNode(
                self.take().into_any().downcast::<T>().unwrap_or_else(|_| unreachable!()),
            ))
        } else {
            Err(self)
        }
    }

    /// Takes the wrapped node.  The caller takes responsibility for dropping the open count.
    pub fn take(self) -> Arc<N> {
        let this = std::mem::ManuallyDrop::new(self);
        unsafe { std::ptr::read(&this.0) }
    }

    /// Returns true if this is an instance of T.
    pub fn is<T: 'static>(&self) -> bool {
        self.0.as_ref().type_id() == TypeId::of::<T>()
    }

    /// Makes another OpenedNode, increasing the open count on the node.
    pub fn dup(&self) -> Self {
        self.0.open_count_add_one();
        OpenedNode(self.0.clone())
    }
}

impl<N: FxNode + ?Sized> Drop for OpenedNode<N> {
    fn drop(&mut self) {
        self.clone().open_count_sub_one();
    }
}

impl<N: FxNode + ?Sized> std::ops::Deref for OpenedNode<N> {
    type Target = Arc<N>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use crate::fuchsia::directory::FxDirectory;
    use crate::fuchsia::node::{FxNode, GetResult, NodeCache};
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::future::join_all;
    use fxfs::object_store::ObjectDescriptor;
    use fxfs_macros::ToWeakNode;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[derive(ToWeakNode)]
    struct FakeNode(u64, Arc<NodeCache>);
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
            ObjectDescriptor::Directory
        }
    }
    impl Drop for FakeNode {
        fn drop(&mut self) {
            self.1.remove(self);
        }
    }

    #[fuchsia::test]
    async fn test_drop_placeholder() {
        let cache = Arc::new(NodeCache::new());
        let object_id = 0u64;
        match cache.get_or_reserve(object_id).await {
            GetResult::Node(_) => panic!("Unexpected node"),
            GetResult::Placeholder(_) => {}
        };
        match cache.get_or_reserve(object_id).await {
            GetResult::Node(_) => panic!("Unexpected node"),
            GetResult::Placeholder(_) => {}
        };
    }

    #[fuchsia::test]
    async fn test_simple() {
        let cache = Arc::new(NodeCache::new());
        let object_id = {
            let node = Arc::new(FakeNode(0, cache.clone()));
            match cache.get_or_reserve(node.object_id()).await {
                GetResult::Node(_) => panic!("Unexpected node"),
                GetResult::Placeholder(p) => {
                    p.commit(&(node.clone() as Arc<dyn FxNode>));
                }
            };
            match cache.get_or_reserve(node.object_id()).await {
                GetResult::Node(n) => assert_eq!(n.object_id(), node.object_id()),
                GetResult::Placeholder(_) => panic!("No node found"),
            };
            node.object_id()
        };
        match cache.get_or_reserve(object_id).await {
            GetResult::Node(_) => panic!("Unexpected node"),
            GetResult::Placeholder(_) => {}
        };
    }

    #[fuchsia::test(threads = 10)]
    async fn test_subsequent_callers_block() {
        let cache = Arc::new(NodeCache::new());
        let object_id = 0u64;
        let writes_to_cache = Arc::new(AtomicU64::new(0));
        let reads_from_cache = Arc::new(AtomicU64::new(0));
        let node = Arc::new(FakeNode(object_id, cache.clone()));
        join_all((0..10).map(|_| {
            let node = node.clone();
            let cache = cache.clone();
            let object_id = object_id.clone();
            let writes_to_cache = writes_to_cache.clone();
            let reads_from_cache = reads_from_cache.clone();
            async move {
                match cache.get_or_reserve(object_id).await {
                    GetResult::Node(node) => {
                        reads_from_cache.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(node.object_id(), object_id);
                    }
                    GetResult::Placeholder(p) => {
                        writes_to_cache.fetch_add(1, Ordering::SeqCst);
                        // Add a delay to simulate doing some work (e.g. loading from disk).
                        fasync::Timer::new(Duration::from_millis(100)).await;
                        p.commit(&(node as Arc<dyn FxNode>));
                    }
                }
            }
        }))
        .await;
        assert_eq!(writes_to_cache.load(Ordering::SeqCst), 1);
        assert_eq!(reads_from_cache.load(Ordering::SeqCst), 9);
    }

    #[fuchsia::test(threads = 10)]
    async fn test_multiple_nodes() {
        const NUM_OBJECTS: usize = 5;
        const TASKS_PER_OBJECT: usize = 4;

        let cache = Arc::new(NodeCache::new());
        let writes = Arc::new(Mutex::new(vec![0u64; NUM_OBJECTS]));
        let reads = Arc::new(Mutex::new(vec![0u64; NUM_OBJECTS]));
        let nodes: Vec<_> = (0..NUM_OBJECTS as u64)
            .map(|object_id| Arc::new(FakeNode(object_id, cache.clone())))
            .collect();

        join_all((0..TASKS_PER_OBJECT).flat_map(|_| {
            nodes.iter().cloned().map(|node| {
                let cache = cache.clone();
                let writes = writes.clone();
                let reads = reads.clone();
                async move {
                    match cache.get_or_reserve(node.object_id()).await {
                        GetResult::Node(result) => {
                            assert_eq!(node.object_id(), result.object_id());
                            reads.lock()[node.object_id() as usize] += 1;
                        }
                        GetResult::Placeholder(p) => {
                            writes.lock()[node.object_id() as usize] += 1;
                            // Add a delay to simulate doing some work (e.g. loading from disk).
                            fasync::Timer::new(Duration::from_millis(100)).await;
                            p.commit(&(node as Arc<dyn FxNode>));
                        }
                    }
                }
            })
        }))
        .await;
        assert_eq!(*writes.lock(), vec![1u64; NUM_OBJECTS]);
        assert_eq!(*reads.lock(), vec![TASKS_PER_OBJECT as u64 - 1; NUM_OBJECTS]);
    }
}
