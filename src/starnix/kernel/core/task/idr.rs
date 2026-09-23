// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An RCU-protected, lock-free integer ID radix tree (IDR).
//!
//! Maps 32-bit integer IDs to objects (`Arc<T>`), optimized for workloads with heavily
//! contended concurrent reads and serialized mutations (such as PID tables, task registries,
//! and descriptor tables).
//!
//! # Concurrency Model
//!
//! - **Readers (`lookup`, `iter`)**: Completely lock-free and wait-free. Traversal operates
//!   under an [`RcuReadScope`], ensuring memory reclamation safety without blocking or
//!   interfering with writers.
//! - **Writers (`alloc`, `reserve_id`, `remove`)**: Mutations are serialized
//!   via an [`IdrGuard`] acquired with [`Idr::lock`], while atomic pointer updates and memory
//!   barriers allow concurrent readers to proceed in parallel without interruption.
//!
//! # Key Operations
//!
//! - [`Idr::lock`]: Acquires the writer lock, returning an [`IdrGuard`] for mutating operations.
//! - [`Idr::lookup`]: Retrieves the object for a given ID without locking.
//! - [`Idr::iter`]: Iterates over all active `(u32, &Arc<T>)` entries under an RCU scope.
//! - [`Idr::max`]: Returns the maximal value allowed for allocation in this `Idr`.
//! - [`Idr::set_max`]: Sets the maximal value allowed for allocation in this `Idr`.
//! - [`IdrGuard::alloc`]: Allocates an ID using the configured allocation policy (linear from 0
//!   or cyclic from cursor).
//! - [`IdrGuard::reserve_id`]: Marks a specific ID as occupied without inserting an element.
//! - [`IdrGuard::remove`]: Removes an item and restores slot availability.
//!
//! # Structural Architecture
//!
//! The tree is a 64-ary radix tree (consuming 6 bits per layer, up to 6 layers for 32-bit IDs):
//! - Intermediate nodes (`layer > 0`) route down to child nodes.
//! - Leaf nodes (`layer == 0`) hold concrete `Arc<T>` entries.
//! - Each node maintains an atomic `free_bitmap` tracking capacity across its 64 sub-slots,
//!   enabling $O(1)$ child selection and efficient subtree skipping during allocation.
//! - The tree dynamically grows upwards in layers as ID requirements expand.

use fuchsia_rcu::{RcuDroppable, RcuDroppableArc, RcuOptionBox};
use smallvec::SmallVec;
use starnix_rcu::RcuReadScope;
use starnix_sync::{Mutex, MutexGuard};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// The number of bits consumed per layer of the tree structure.
/// Chosen as 6 because 2^6 = 64, mapping perfectly to a 64-bit word (`u64`)
/// used for atomic lock-free bitmaps per node.
const BITS_PER_LEVEL: u32 = 6;
/// The maximum number of children per node, directly derived from the bits per level.
const NODE_CAPACITY: usize = 1 << BITS_PER_LEVEL;
/// The bitmask used to extract the current layer's routing portion from a target ID.
const LEVEL_MASK: u32 = (1 << BITS_PER_LEVEL) - 1;
/// The theoretical maximum depth required to completely map a 32-bit integer space.
const MAX_DEPTH: u32 = (32 + BITS_PER_LEVEL - 1) / BITS_PER_LEVEL;

/// The allocation mode used by an [`Idr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdrAllocMode {
    /// Allocates the lowest available ID starting from 0.
    #[default]
    Linear,
    /// Allocates IDs sequentially starting from the previous cursor position,
    /// wrapping around when hitting upper bounds.
    Cyclic {
        /// The minimum ID to allocate after wrapping around (or 0 if `None`).
        min_after_wrap: Option<u32>,
    },
}

/// A concurrent, lock-free radix tree mapped structure primarily employed to map 32-bit
/// IDs to objects. Optimized for massively contended reads.
pub struct Idr<T: RcuDroppable + Send + Sync + 'static> {
    /// Serializes all mutating writes (allocations and removals) modifying the
    /// tree structure. The protected `u32` value tracks the starting cursor for
    /// cyclic allocations.
    writer_lock: Mutex<u32>,
    /// The top-level entry point descending into the tree. Atomically replaced
    /// whenever the structure grows upwards.
    root: RcuDroppableArc<IdrNode<T>>,
    /// The allocation mode determining whether IDs are allocated linearly or cyclically.
    alloc_mode: IdrAllocMode,
    /// The maximal value allowed for allocation in this `Idr`.
    max: AtomicU32,
}

impl<T: RcuDroppable + Send + Sync + 'static> Default for Idr<T> {
    fn default() -> Self {
        Self::new(IdrAllocMode::default())
    }
}

impl<T: RcuDroppable + Send + Sync + 'static> Idr<T> {
    /// Creates a new `Idr` radix tree with the specified allocation mode.
    pub fn new(alloc_mode: IdrAllocMode) -> Self {
        Self {
            writer_lock: Mutex::new(0),
            root: RcuDroppableArc::new(Arc::new(IdrNode::new(0))),
            alloc_mode,
            max: AtomicU32::new(u32::MAX),
        }
    }

    /// Creates a new cyclic `Idr` radix tree with an optional minimum ID after wrapping.
    pub fn new_cyclic(min_after_wrap: Option<u32>) -> Self {
        Self::new(IdrAllocMode::Cyclic { min_after_wrap })
    }

    /// Returns the maximal value allowed for allocation in this `Idr`.
    pub fn max(&self) -> u32 {
        self.max.load(Ordering::Relaxed)
    }

    /// Sets the maximal value allowed for allocation in this `Idr`.
    pub fn set_max(&self, max: u32) {
        self.max.store(max, Ordering::Relaxed);
    }

    /// Acquires the writer lock, returning an [`IdrGuard`] that provides mutating operations.
    pub fn lock(&self) -> IdrGuard<'_, T> {
        IdrGuard { idr: self, cursor: self.writer_lock.lock() }
    }

    /// RCU protected lock-free lookup for readers
    pub fn lookup(&self, id: u32, scope: &RcuReadScope) -> Option<Arc<T>> {
        let mut current_node = self.root.as_ref(scope);

        let capacity = current_node.capacity();
        if (id as u64) >= capacity {
            return None;
        }

        loop {
            let index = current_node.index_for_id(id);

            let entry = current_node.children[index].as_ref(scope);
            match entry {
                Some(IdrEntry::Leaf(arc)) => return Some(arc.clone()),
                Some(IdrEntry::Node(next)) => {
                    current_node = &**next;
                }
                None => return None,
            }
        }
    }

    /// Returns a lock-free RCU iterator over all entries
    pub fn iter<'a>(&'a self, scope: &'a RcuReadScope) -> IdrIterator<'a, T> {
        let mut stack = SmallVec::new();
        let root = self.root.as_ref(scope);
        stack.push((root, 0, 0));
        IdrIterator { scope, stack }
    }

    /// Iteratively searches for the lowest available free ID >= `start_id` and <= `max_id`.
    /// Populates `path` with the `(node, child_index)` sequence from root to leaf.
    fn find_free_slot<'a>(
        &self,
        start_id: u32,
        max_id: u32,
        path: &mut SmallVec<[(&'a IdrNode<T>, usize); MAX_DEPTH as usize]>,
        scope: &'a RcuReadScope,
    ) -> Option<u32> {
        if start_id > max_id {
            return None;
        }

        // Traversal stack storing the path from root and unexplored sibling candidates per
        // layer. Enables iterative backtracking without recursion or heap allocations.
        struct StackEntry<'a, T: RcuDroppable + Send + Sync + 'static> {
            node: &'a IdrNode<T>,
            // Unexplored candidate slots at this level with open capacity.
            free_bits: u64,
            // True if slot selection at this level is restricted to indices >= start_id.
            min_constrained: bool,
            // True if slot selection at this level is restricted to indices <= max_id.
            max_constrained: bool,
            // The child slot index selected when descending to the next layer.
            chosen_index: usize,
        }
        let root = self.root.as_ref(scope);

        let mut stack = SmallVec::<[StackEntry<'a, T>; MAX_DEPTH as usize]>::new();

        // When constrained by start_id, mask out slots below the cursor index.
        let mut initial_free_bits = root.free_bitmap.load(Ordering::Relaxed);
        let min_constrained = start_id > 0;
        if min_constrained {
            let cursor_index = root.index_for_id(start_id);
            initial_free_bits &= !((1u64 << cursor_index) - 1);
        }

        // When constrained by max_id, mask out slots above the max index.
        let max_constrained = (max_id as u64) < root.capacity() - 1;
        if max_constrained {
            let max_index = root.index_for_id(max_id);
            let max_mask = if max_index >= 63 { !0 } else { (1u64 << (max_index + 1)) - 1 };
            initial_free_bits &= max_mask;
        }

        // No candidate slots in [start_id, max_id] at the root level.
        if initial_free_bits == 0 {
            return None;
        }

        stack.push(StackEntry {
            node: root,
            free_bits: initial_free_bits,
            min_constrained,
            max_constrained,
            chosen_index: 0,
        });

        while let Some(top) = stack.last_mut() {
            // Backtrack to the parent layer when all candidate slots in this node are exhausted.
            if top.free_bits == 0 {
                stack.pop();
                continue;
            }

            let index = top.free_bits.trailing_zeros() as usize;
            top.free_bits &= !(1u64 << index);
            top.chosen_index = index;

            let node = top.node;

            if node.layer == 0 {
                let id = stack
                    .iter()
                    .fold(0u32, |acc, entry| acc | entry.node.id_for_index(entry.chosen_index));
                path.extend(stack.into_iter().map(|entry| (entry.node, entry.chosen_index)));
                return Some(id);
            }

            // Deeper layers stay constrained only if descending into the exact boundary slot.
            let is_min_constrained = top.min_constrained && (index == node.index_for_id(start_id));
            let is_max_constrained = top.max_constrained && (index == node.index_for_id(max_id));

            let child = node.get_or_create_child(index, scope);
            let mut child_free_bits = child.free_bitmap.load(Ordering::Relaxed);
            if is_min_constrained {
                let child_cursor = child.index_for_id(start_id);
                child_free_bits &= !((1u64 << child_cursor) - 1);
            }
            if is_max_constrained {
                let child_max = child.index_for_id(max_id);
                let max_mask = if child_max >= 63 { !0 } else { (1u64 << (child_max + 1)) - 1 };
                child_free_bits &= max_mask;
            }

            // Skip child subtree if constraints left no open slots.
            if child_free_bits == 0 {
                continue;
            }

            stack.push(StackEntry {
                node: child,
                free_bits: child_free_bits,
                min_constrained: is_min_constrained,
                max_constrained: is_max_constrained,
                chosen_index: 0,
            });
        }

        None
    }

    /// When a node transitions to having 0 free bits, mark it full in its parent
    /// The caller MUST hold the `writer_lock`.
    fn propagate_fullness(&self, path: &[(&IdrNode<T>, usize)]) {
        for i in (0..path.len() - 1).rev() {
            let (parent, parent_index) = &path[i];
            if !parent.mark_allocated(*parent_index) {
                // Parent still has other free slots. Stop propagating.
                break;
            }
        }
    }

    /// When a node transitions from having 0 free bits to > 0, mark it free in its parent
    /// The caller MUST hold the `writer_lock`.
    fn propagate_availability(&self, path: &[(&IdrNode<T>, usize)]) {
        for i in (0..path.len() - 1).rev() {
            let (parent, parent_index) = &path[i];

            if !parent.mark_freed(*parent_index) {
                // Parent already had other free slots. Stop propagating.
                break;
            }
        }
    }

    /// Grows the tree by adding a new layer on top of the root.
    /// The caller MUST hold the `writer_lock`.
    fn grow_tree_by_one_layer(&self, root_arc: &Arc<IdrNode<T>>) -> Option<Arc<IdrNode<T>>> {
        let next_layer = root_arc.layer + 1;
        if next_layer >= MAX_DEPTH {
            return None;
        }

        let new_root = Arc::new(IdrNode::new(next_layer));
        if root_arc.free_bitmap.load(Ordering::Relaxed) == 0 {
            new_root.free_bitmap.fetch_and(!1, Ordering::Relaxed);
        }
        new_root.mark_present(0);
        new_root.children[0].update(Some(IdrEntry::Node(root_arc.clone())));
        self.root.update(new_root.clone());
        Some(new_root)
    }
}

/// An RAII guard representing exclusive writer access to an [`Idr`].
///
/// Holding this guard serializes mutations (allocations, reservations, removals)
/// to the radix tree while allowing concurrent lock-free reads.
pub struct IdrGuard<'a, T: RcuDroppable + Send + Sync + 'static> {
    idr: &'a Idr<T>,
    cursor: MutexGuard<'a, u32>,
}

impl<'a, T: RcuDroppable + Send + Sync + 'static> std::ops::Deref for IdrGuard<'a, T> {
    type Target = Idr<T>;

    fn deref(&self) -> &Self::Target {
        self.idr
    }
}

impl<'a, T: RcuDroppable + Send + Sync + 'static> IdrGuard<'a, T> {
    /// Allocates the next available ID by calling a factory providing the newly
    /// acquired ID.
    ///
    /// Depending on the `Idr` configuration, allocation will be either linear starting
    /// from 0, or cyclic starting from the previous cursor position.
    pub fn alloc<F>(&mut self, factory: F) -> Option<(u32, Arc<T>)>
    where
        F: FnOnce(u32) -> Arc<T>,
    {
        let max_id = self.idr.max.load(Ordering::Relaxed);

        let (is_cyclic, wrap_min) = match self.idr.alloc_mode {
            IdrAllocMode::Linear => (false, 0),
            IdrAllocMode::Cyclic { min_after_wrap } => (true, min_after_wrap.unwrap_or(0)),
        };

        let (start_id, wrapped) = if is_cyclic {
            if *self.cursor > max_id { (wrap_min, true) } else { (*self.cursor, false) }
        } else {
            (0, false)
        };

        if start_id > max_id {
            return None;
        }

        let mut root_arc = self.idr.root.to_arc();

        // Ensure the tree is large enough:
        // 1. If the root free bitmap is 0, the entire current tree capacity is exhausted,
        //    requiring a new root layer on top if capacity <= max_id.
        // 2. If `start_id` exceeds current tree capacity (e.g. after wrapping or initial placement),
        //    grow the tree until capacity covers `start_id` or until MAX_DEPTH is reached.
        while (start_id as u64) >= root_arc.capacity()
            || (root_arc.free_bitmap.load(Ordering::Relaxed) == 0
                && root_arc.capacity() <= (max_id as u64))
        {
            if let Some(new_root) = self.idr.grow_tree_by_one_layer(&root_arc) {
                root_arc = new_root;
            } else {
                // Tree reached maximum allowable depth and cannot grow further.
                return None;
            }
        }

        let scope = RcuReadScope::new();
        let mut path = SmallVec::<[(&IdrNode<T>, usize); MAX_DEPTH as usize]>::new();
        // First attempt: search for a free slot >= `start_id` and <= `max_id`.
        let id_opt = self.idr.find_free_slot(start_id, max_id, &mut path, &scope).or_else(|| {
            // If cyclic allocation failed to find a slot between `start_id`
            // and `max_id`, wrap around to search from `wrap_min` up to `max_id`.
            if is_cyclic && !wrapped && start_id > wrap_min && wrap_min <= max_id {
                path.clear();
                self.idr.find_free_slot(wrap_min, max_id, &mut path, &scope)
            } else {
                None
            }
        });

        let id = id_opt?;
        let (leaf_node, leaf_index) = path.last().expect("path should not be empty");
        let leaf_index = *leaf_index;
        let item = factory(id);

        // Install the newly constructed leaf item at the target slot.
        leaf_node.children[leaf_index].update(Some(IdrEntry::Leaf(item.clone())));
        leaf_node.mark_present(leaf_index);

        // Mark the leaf slot as allocated in its free bitmap. If this clears the final free bit
        // in the leaf node, propagate the full state up the ancestor chain via `propagate_fullness`.
        if leaf_node.mark_allocated(leaf_index) {
            self.idr.propagate_fullness(&path);
        }

        // For cyclic allocations, advance the cursor to `id + 1`, wrapping to `wrap_min` when
        // exceeding `max_id` or on u32 overflow.
        if is_cyclic {
            let next_cursor = id.wrapping_add(1);
            *self.cursor = if next_cursor > max_id || (next_cursor == 0 && wrap_min > 0) {
                wrap_min
            } else {
                next_cursor
            };
        }
        Some((id, item))
    }

    /// Marks a specific ID as unavailable so the allocator will never return it.
    /// Does not populate the tree with an item.
    pub fn reserve_id(&mut self, id: u32) {
        let mut root_arc = self.idr.root.to_arc();

        loop {
            let capacity = root_arc.capacity();
            if (id as u64) < capacity {
                break;
            }
            if let Some(new_root) = self.idr.grow_tree_by_one_layer(&root_arc) {
                root_arc = new_root;
            } else {
                return; // Exceeded max depth
            }
        }

        let scope = RcuReadScope::new();
        let mut current_node = root_arc.as_ref();
        let mut path = SmallVec::<[(&IdrNode<T>, usize); MAX_DEPTH as usize]>::new();

        loop {
            let index = current_node.index_for_id(id);

            path.push((current_node, index));

            // Reached a leaf node. Claim the slot.
            if current_node.layer == 0 {
                // If it was previously free, and this clears the last free bit, propagate fullness
                if current_node.mark_allocated(index) {
                    self.idr.propagate_fullness(&path);
                }
                return;
            }

            // Descend to the next layer.
            current_node = current_node.get_or_create_child(index, &scope);
        }
    }

    /// Removes an item by ID.
    pub fn remove(&mut self, id: u32) {
        let scope = RcuReadScope::new();

        let mut current_node = self.idr.root.as_ref(&scope);

        if (id as u64) >= current_node.capacity() {
            return;
        }

        let mut path = SmallVec::<[(&IdrNode<T>, usize); MAX_DEPTH as usize]>::new();

        loop {
            let index = current_node.index_for_id(id);

            path.push((current_node, index));

            let Some(child) = current_node.children[index].as_ref(&scope) else {
                // Nothing to remove
                return;
            };

            if current_node.layer == 0 {
                current_node.children[index].update(None);
                current_node.mark_absent(index);
                if current_node.mark_freed(index) {
                    self.idr.propagate_availability(&path);
                }
                return;
            } else {
                current_node = match child {
                    IdrEntry::Node(n) => &**n,
                    _ => unreachable!("Tree corruption: expected a Node entry here"),
                };
            }
        }
    }

    /// Returns the current cursor position for cyclic allocations.
    #[cfg(test)]
    fn cursor(&self) -> u32 {
        *self.cursor
    }

    /// Sets the cursor position for cyclic allocations.
    #[cfg(test)]
    fn set_cursor(&mut self, cursor: u32) {
        *self.cursor = cursor;
    }
}

/// A lock-free iterator traversing the allocated elements inside the radix tree
/// under an automated RCU read scope, operating without acquiring any thread locks.
pub struct IdrIterator<'a, T: RcuDroppable + Send + Sync + 'static> {
    /// The ambient read scope keeping the traversed tree nodes pinned in memory.
    scope: &'a RcuReadScope,
    /// The traversal stack keeping track of the current path, the next index, and
    /// the accumulated ID prefix.
    stack: SmallVec<[(&'a IdrNode<T>, usize, u32); MAX_DEPTH as usize]>,
}

impl<'a, T: RcuDroppable + Send + Sync + 'static> Iterator for IdrIterator<'a, T> {
    type Item = (u32, &'a Arc<T>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((node, index, id_base)) = self.stack.pop() {
            let presence = node.presence_bitmap.load(Ordering::Relaxed);

            let mask = if index >= NODE_CAPACITY { 0 } else { !((1u64 << index) - 1) };
            let remaining = presence & mask;

            if remaining == 0 {
                continue;
            }

            let next_bit = remaining.trailing_zeros() as usize;

            self.stack.push((node, next_bit + 1, id_base));

            let entry_opt = node.children[next_bit].as_ref(self.scope);
            if let Some(entry) = entry_opt {
                let child_id = id_base | node.id_for_index(next_bit);

                match entry {
                    IdrEntry::Node(child_arc) => {
                        self.stack.push((child_arc.as_ref(), 0, child_id));
                    }
                    IdrEntry::Leaf(arc) => {
                        return Some((child_id, arc));
                    }
                }
            }
        }
        None
    }
}

/// Represents a single slot within an `IdrNode`'s capability array.
#[derive(Debug)]
enum IdrEntry<T: RcuDroppable + Send + Sync + 'static> {
    /// An intermediate branch pointing to the next layer down the tree structure.
    Node(Arc<IdrNode<T>>),
    /// A concrete element residing at the bottom layer.
    Leaf(Arc<T>),
}

// SAFETY: All variants contain only types that are `RcuDroppable` (`Arc<IdrNode<T>>` and `Arc<T>`).
// A manual implementation is necessary because deriving `RcuDroppable` on both types triggers a
// recursive evaluation overflow (Rust issue #26925) due to mutual recursion with `IdrNode`.
unsafe impl<T: RcuDroppable + Send + Sync + 'static> RcuDroppable for IdrEntry<T> {}

#[derive(Debug, RcuDroppable)]
struct IdrNode<T: RcuDroppable + Send + Sync + 'static> {
    /// The structural height of this node in the tree. Leaf nodes holding concrete
    /// elements sit at layer 0. Intermediate branches exist at layers > 0.
    layer: u32,

    /// Tracks allocation capacity across the 64 sub-slots. A `1` bit indicates the
    /// corresponding slot (or its sub-branch) still has open IDs available. A `0`
    /// bit signifies the branch or leaf is at 100% capacity (or reserved).
    free_bitmap: AtomicU64,

    /// Tracks structural instantiation of the 64 sub-slots. A `1` bit indicates an
    /// intermediate branch node has physically been allocated into memory. Used strictly
    /// by the lock-free iterator to gracefully skip over uninstantiated memory gaps.
    presence_bitmap: AtomicU64,

    /// The contiguous memory array branching off this node, storing inner `IdrNode`
    /// branches (when layer > 0) or underlying `Arc<T>` leaf items (when layer == 0).
    children: [RcuOptionBox<IdrEntry<T>>; NODE_CAPACITY],
}

impl<T: RcuDroppable + Send + Sync + 'static> Default for IdrNode<T> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<T: RcuDroppable + Send + Sync + 'static> IdrNode<T> {
    fn new(layer: u32) -> Self {
        let children = std::array::from_fn(|_| RcuOptionBox::new(None));
        Self {
            layer,
            free_bitmap: AtomicU64::new(!0), // all 1s means all free
            presence_bitmap: AtomicU64::new(0),
            children,
        }
    }

    /// Computes the total capacity underneath this specific node.
    /// Layer 0 nodes possess a capacity of exactly 64. Each higher layer multiplies it by 64.
    #[inline]
    fn capacity(&self) -> u64 {
        1u64 << (BITS_PER_LEVEL * (self.layer + 1))
    }

    /// Compute the index of the child of this node that contains `id`.
    #[inline]
    fn index_for_id(&self, id: u32) -> usize {
        ((id >> (self.layer * BITS_PER_LEVEL)) & LEVEL_MASK) as usize
    }

    /// Compute the contribution to the final value of the `index` child of this node.
    /// Utilized by the iterator to reconstruct numeric IDs.
    #[inline]
    fn id_for_index(&self, index: usize) -> u32 {
        (index as u32) << (self.layer * BITS_PER_LEVEL)
    }

    /// Clears the `index` free bit.
    /// Returns true if this transition caused the node to become completely full (0).
    #[inline]
    fn mark_allocated(&self, index: usize) -> bool {
        let old_free = self.free_bitmap.fetch_and(!(1 << index), Ordering::Relaxed);
        old_free == (1 << index)
    }

    /// Adds the `index` free bit.
    /// Returns true if this transition caused the node to transition from completely
    /// full (0) to having capacity.
    #[inline]
    fn mark_freed(&self, index: usize) -> bool {
        let old_free = self.free_bitmap.fetch_or(1 << index, Ordering::Relaxed);
        old_free == 0
    }

    /// Marks the `index` slot as populated with a branch or leaf.
    #[inline]
    fn mark_present(&self, index: usize) {
        self.presence_bitmap.fetch_or(1 << index, Ordering::Relaxed);
    }

    /// Marks the `index` slot as physically empty/removed.
    #[inline]
    fn mark_absent(&self, index: usize) {
        self.presence_bitmap.fetch_and(!(1 << index), Ordering::Relaxed);
    }

    /// Descends into the specific child branch index. If the branch is currently
    /// entirely empty and uninstantiated, it constructs the next layer.
    /// Must never be called on layer 0.
    fn get_or_create_child<'a>(&'a self, index: usize, scope: &'a RcuReadScope) -> &'a IdrNode<T> {
        debug_assert!(self.layer > 0);
        if let Some(IdrEntry::Node(n)) = self.children[index].as_ref(scope) {
            return &**n;
        }

        let new_node = Arc::new(IdrNode::new(self.layer - 1));
        self.children[index].update(Some(IdrEntry::Node(new_node)));
        self.mark_present(index);
        match self.children[index].as_ref(scope).unwrap() {
            IdrEntry::Node(n) => &**n,
            _ => unreachable!("Tree corruption: expected a Node entry here"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(RcuDroppable)]
    struct MockItem {
        value: u32,
    }

    #[fuchsia::test]
    fn test_basic_alloc_lookup() {
        let idr = Idr::default();
        assert_eq!(idr.alloc_mode, IdrAllocMode::Linear);

        let (id1, _item1) = idr.lock().alloc(|id| Arc::new(MockItem { value: id * 10 })).unwrap();
        assert_eq!(id1, 0);

        let (id2, _item2) = idr.lock().alloc(|id| Arc::new(MockItem { value: id * 10 })).unwrap();
        assert_eq!(id2, 1);

        let scope = RcuReadScope::new();
        let lookup1 = idr.lookup(0, &scope).unwrap();
        assert_eq!(lookup1.value, 0);

        let lookup2 = idr.lookup(1, &scope).unwrap();
        assert_eq!(lookup2.value, 10);

        idr.lock().remove(0);
        assert!(idr.lookup(0, &scope).is_none());

        // Second item remains completely untouched and safely addressable
        let lookup_still_there = idr.lookup(1, &scope).unwrap();
        assert_eq!(lookup_still_there.value, 10);
    }

    #[fuchsia::test]
    fn test_tree_growth() {
        let idr = Idr::default();
        let mut _items = Vec::new();

        // NODE_CAPACITY is natively 64. Allocating 150 structurally guarantees forcing the tree to
        // grow at least once.
        for i in 0..150 {
            let (id, item) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
            _items.push(item);
            assert_eq!(id, i);
        }

        let scope = RcuReadScope::new();
        for i in 0..150 {
            let item = idr.lookup(i, &scope).unwrap();
            assert_eq!(item.value, i);
        }
    }

    #[fuchsia::test]
    fn test_alloc_cyclic() {
        let idr = Idr::new_cyclic(None);
        let mut _items = Vec::new();

        for i in 0..30 {
            let (id, item) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
            _items.push(item);
            assert_eq!(id, i);
        }

        // Manually bump the cursor.
        idr.lock().set_cursor(100);

        // Allocation formally continues from new cursor.
        let (id, item1) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        _items.push(item1);
        assert_eq!(id, 100);

        let (id, item2) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        _items.push(item2);
        assert_eq!(id, 101);

        let scope = RcuReadScope::new();
        assert!(idr.lookup(30, &scope).is_none());
        assert_eq!(idr.lookup(100, &scope).unwrap().value, 100);
    }

    #[fuchsia::test]
    fn test_alloc_cyclic_start_exceeds_capacity() {
        let idr = Idr::new_cyclic(None);
        let mut _items = Vec::new();

        // Immediately bump start_id above the initial layer 0 capacity (64)
        idr.lock().set_cursor(256);

        let (id, item) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        _items.push(item);

        // It should have assigned exactly 256, successfully growing the tree.
        assert_eq!(id, 256);
    }

    #[fuchsia::test]
    fn test_reserve_id() {
        let idr = Idr::new_cyclic(None);
        let mut _items = Vec::new();

        // Reserve an ID within initial layer
        idr.lock().reserve_id(10);

        let (id1, item1) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        _items.push(item1);
        assert_eq!(id1, 0);

        // Reserve an ID requiring tree growth
        idr.lock().reserve_id(200);

        // Fill up to 10
        let mut allocated_10 = false;
        for _ in 1..15 {
            let (id, item) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
            _items.push(item);
            if id == 10 {
                allocated_10 = true;
            }
        }
        assert!(!allocated_10, "ID 10 was allocated despite being reserved");

        // Verify ID 200 is skipped when allocating near it
        idr.lock().set_cursor(199);

        let mut allocated_200 = false;
        for _ in 0..5 {
            let (id, item) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
            _items.push(item);
            if id == 200 {
                allocated_200 = true;
            }
        }
        assert!(!allocated_200, "ID 200 was allocated despite being reserved");

        let scope = RcuReadScope::new();
        // Lookup of reserved IDs should return None
        assert!(idr.lookup(10, &scope).is_none());
        assert!(idr.lookup(200, &scope).is_none());

        // Remove should not panic or corrupt it
        idr.lock().remove(10);
        idr.lock().remove(200);
        assert!(idr.lookup(10, &scope).is_none());
    }

    #[fuchsia::test]
    fn test_iter_and_remove() {
        let idr = Idr::default();
        let mut _items = Vec::new();

        // Assigned cleanly to 0
        _items.push(idr.lock().alloc(|_| Arc::new(MockItem { value: 10 })).unwrap().1);
        // Assigned cleanly to 1
        _items.push(idr.lock().alloc(|_| Arc::new(MockItem { value: 20 })).unwrap().1);
        // Assigned cleanly to 2
        _items.push(idr.lock().alloc(|_| Arc::new(MockItem { value: 30 })).unwrap().1);

        // Eliminate middle ID freeing slot 1.
        idr.lock().remove(1);

        let scope = RcuReadScope::new();
        let mut iter = idr.iter(&scope);

        let next_a = iter.next();
        let (id_a, item_a) = next_a.unwrap();
        assert_eq!(id_a, 0);
        assert_eq!(item_a.value, 10);

        let (id_b, item_b) = iter.next().unwrap();
        assert_eq!(id_b, 2);
        assert_eq!(item_b.value, 30);

        assert!(iter.next().is_none());
    }

    #[fuchsia::test]
    fn test_iter_minimal() {
        let idr = Idr::default();
        let mut _items = Vec::new();
        _items.push(idr.lock().alloc(|value| Arc::new(MockItem { value })).unwrap().1);
        let scope = RcuReadScope::new();
        let mut iter = idr.iter(&scope);
        let next_a = iter.next();
        assert!(next_a.is_some(), "iter.next() returned None!");
    }

    #[fuchsia::test]
    fn test_alloc_cyclic_with_gaps() {
        let idr = Idr::new_cyclic(None);
        let mut _items = Vec::new();

        // Allocate 100 items (0..99) across layer 0 and layer 1.
        for i in 0..100 {
            let (id, item) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
            assert_eq!(id, i);
            _items.push(item);
        }

        // Create holes in child 0 (0..63) below 50, but keep 50..63 allocated.
        for id in 40..50 {
            idr.lock().remove(id);
        }

        // Advance cursor to 50.
        idr.lock().set_cursor(50);

        // Cyclic allocation should find next free slot >= 50, which is slot 100 in child 1,
        // rather than prematurely wrapping to 40 in child 0.
        let (id_100, item_100) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        assert_eq!(id_100, 100);
        _items.push(item_100);

        // Subsequent allocations should continue forward monotonically across child boundaries (101..130).
        for expected in 101..130 {
            let (id, item) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
            assert_eq!(id, expected);
            _items.push(item);
        }
    }

    #[fuchsia::test]
    fn test_concurrent_readers_and_writers() {
        let idr = Arc::new(Idr::new_cyclic(None));
        let running = Arc::new(AtomicBool::new(true));

        // Pre-populate some items
        for _i in 0..50 {
            idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        }

        let mut reader_handles = Vec::new();
        for _ in 0..4 {
            let idr_clone = Arc::clone(&idr);
            let running_clone = Arc::clone(&running);
            reader_handles.push(std::thread::spawn(move || {
                while running_clone.load(Ordering::Relaxed) {
                    let scope = RcuReadScope::new();
                    // Random lookups
                    for id in 0..100 {
                        if let Some(item) = idr_clone.lookup(id, &scope) {
                            assert_eq!(item.value, id);
                        }
                    }

                    // Iterator traversal
                    let iter = idr_clone.iter(&scope);
                    for (id, item) in iter {
                        assert_eq!(item.value, id);
                    }
                }
            }));
        }

        let running_clone = Arc::clone(&running);
        let rcu_advancer = std::thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                fuchsia_rcu::rcu_synchronize();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });

        // Writer thread performs allocations and deletions
        let idr_clone = Arc::clone(&idr);
        let writer_handle = std::thread::spawn(move || {
            for _ in 0..200 {
                let (id, _) =
                    idr_clone.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
                if id > 50 && id % 3 == 0 {
                    idr_clone.lock().remove(id);
                }
            }
        });

        writer_handle.join().unwrap();
        running.store(false, Ordering::Relaxed);
        rcu_advancer.join().unwrap();

        for handle in reader_handles {
            handle.join().unwrap();
        }
    }

    #[fuchsia::test]
    fn test_u32_max_wrap_around() {
        let idr = Idr::<MockItem>::new_cyclic(None);
        idr.lock().set_cursor(u32::MAX);

        // First allocation is at u32::MAX
        let (id1, _item1) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        assert_eq!(id1, u32::MAX);

        // Next allocation correctly wraps around to 0
        let (id2, _item2) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        assert_eq!(id2, 0);

        // Validating they are safely addressable
        let scope = RcuReadScope::new();
        assert_eq!(idr.lookup(u32::MAX, &scope).unwrap().value, u32::MAX);
        assert_eq!(idr.lookup(0, &scope).unwrap().value, 0);
    }

    #[fuchsia::test]
    fn test_overflow_bug_at_u32_max() {
        let idr = Idr::<MockItem>::new_cyclic(None);

        // Reserve 0 so that if the allocator wraps around safely,
        // it assigns 1 (acting as a dual check).
        idr.lock().reserve_id(0);

        // Reserving `u32::MAX` is required to trigger this bug. If `u32::MAX` is free, an
        // allocation starting at `u32::MAX` simply takes it, and the cursor wraps cleanly to `0`.
        // By making it occupied, `find_free_slot` descends to the bottom of
        // subtree 3, discovers there is no space left at or above the cursor,
        // and backtracks all the way up to layer 5.
        // If layer 5 is not properly constrained by the maximal value, this backtracking
        // causes the allocator to erroneously spill over into the invalid index 4.
        idr.lock().reserve_id(u32::MAX);

        idr.lock().set_cursor(u32::MAX);

        let (id, _) = idr.lock().alloc(|id| Arc::new(MockItem { value: id })).unwrap();
        assert_eq!(id, 1);

        let scope = RcuReadScope::new();
        assert!(idr.lookup(1, &scope).is_some());
    }

    #[fuchsia::test]
    fn test_lock_held_across_operations() {
        let idr = Idr::<MockItem>::default();

        // Acquire the lock outside the class and perform multiple operations while holding it.
        let mut guard = idr.lock();
        assert_eq!(guard.cursor(), 0);

        guard.reserve_id(0);
        let (id1, item1) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id1, 1);
        assert_eq!(item1.value, 1);

        let (id2, item2) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id2, 2);
        assert_eq!(item2.value, 2);

        guard.remove(1);

        // The slot for ID 1 is now available again.
        let (id3, item3) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id3, 1);
        assert_eq!(item3.value, 1);

        // Readers can still access items through Deref on the lock.
        let scope = RcuReadScope::new();
        assert_eq!(guard.lookup(1, &scope).unwrap().value, 1);
        assert_eq!(guard.lookup(2, &scope).unwrap().value, 2);

        // Allocating the next lowest available slot yields 3.
        let (id4, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id4, 3);

        guard.set_cursor(50);
        assert_eq!(guard.cursor(), 50);
    }

    #[fuchsia::test]
    fn test_alloc_cyclic_min_after_wrap() {
        let idr = Idr::<MockItem>::new_cyclic(Some(2));
        assert_eq!(idr.alloc_mode, IdrAllocMode::Cyclic { min_after_wrap: Some(2) });
        let mut guard = idr.lock();

        // Allocate slots 0, 1, 2, 3, 4.
        let (id0, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id0, 0);
        let (id1, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id1, 1);
        let (id2, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id2, 2);
        let (id3, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id3, 3);
        let (id4, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id4, 4);

        // Free 0 and 1 so slots 0 and 1 become free in the tree.
        guard.remove(0);
        guard.remove(1);

        // Reserve slots 10..64 within initial layer 0 (capacity 64).
        for i in 10..64 {
            guard.reserve_id(i);
        }

        // Advance cursor to 10.
        guard.set_cursor(10);

        // Since slots 10..64 are reserved and cursor is 10, searching >= 10 fails.
        // It wraps around. With min_after_wrap = Some(2), it must skip free slots 0 and 1,
        // and find the next free slot >= 2, which is slot 5.
        let (id5, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id5, 5);

        // Allocate remaining slots up to 9.
        for expected in 6..10 {
            let (id, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
            assert_eq!(id, expected);
        }

        // Now slots 2..64 are all occupied (allocated or reserved).
        // Slots 0 and 1 remain free.
        // Attempting another allocation with min_after_wrap = Some(2) must return None,
        // because all slots >= 2 are full.
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());

        // For an IDR configured without min_after_wrap, wrapping allows allocating slots 0 and 1.
        let idr_zero = Idr::<MockItem>::new_cyclic(None);
        let mut guard_zero = idr_zero.lock();
        let (z0, _) = guard_zero.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(z0, 0);
        let (z1, _) = guard_zero.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(z1, 1);
        guard_zero.remove(0);
        guard_zero.remove(1);
        guard_zero.set_cursor(10);
        for i in 10..64 {
            guard_zero.reserve_id(i);
        }
        let (id0_after, _) = guard_zero.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id0_after, 0);

        let (id1_after, _) = guard_zero.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id1_after, 1);
    }

    #[fuchsia::test]
    fn test_alloc_cyclic_min_after_wrap_at_u32_max() {
        let idr = Idr::<MockItem>::new_cyclic(Some(2));
        let mut guard = idr.lock();

        // Slots 0, 1, 2 are all free. Set cursor to u32::MAX.
        guard.set_cursor(u32::MAX);

        // Allocate u32::MAX.
        let (id_max, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id_max, u32::MAX);

        // Cursor should wrap to min_after_wrap (2), not 0.
        assert_eq!(guard.cursor(), 2);

        // Next allocation starts at cursor (2), skipping slots 0 and 1.
        let (id2, _) = guard.alloc(|value| Arc::new(MockItem { value })).unwrap();
        assert_eq!(id2, 2);
        assert_eq!(guard.cursor(), 3);

        let scope = RcuReadScope::new();
        assert!(guard.lookup(0, &scope).is_none());
        assert!(guard.lookup(1, &scope).is_none());
        assert!(guard.lookup(2, &scope).is_some());
        assert!(guard.lookup(u32::MAX, &scope).is_some());
    }

    #[fuchsia::test]
    fn test_linear_alloc_max() {
        let idr = Idr::<MockItem>::default();
        idr.set_max(3);
        assert_eq!(idr.max(), 3);

        let mut guard = idr.lock();
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 0);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 1);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 2);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 3);

        // All IDs <= max are allocated; subsequent allocation fails.
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());

        // Free ID 1 below max.
        guard.remove(1);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 1);
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());
    }

    #[fuchsia::test]
    fn test_update_max() {
        let idr = Idr::<MockItem>::default();
        idr.set_max(2);
        let mut guard = idr.lock();

        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 0);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 1);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 2);
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());

        // Increase maximal value to 5.
        guard.set_max(5);
        assert_eq!(guard.max(), 5);

        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 3);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 4);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 5);
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());

        // Lower maximal value to 4. Existing ID 5 remains readable.
        guard.set_max(4);
        let scope = RcuReadScope::new();
        assert!(guard.lookup(5, &scope).is_some());

        // Removing 5 does not allow reallocating it because 5 > max (4).
        guard.remove(5);
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());

        // Removing 2 allows reallocating it because 2 <= max (4).
        guard.remove(2);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 2);
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());
    }

    #[fuchsia::test]
    fn test_cyclic_alloc_max_wrapping() {
        let idr = Idr::<MockItem>::new_cyclic(Some(2));
        idr.set_max(5);
        let mut guard = idr.lock();

        // Initial sequential allocations.
        for expected in 0..=5 {
            assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, expected);
        }

        // Cursor should wrap to min_after_wrap (2).
        assert_eq!(guard.cursor(), 2);

        // Slots 2..=5 are full, so allocation returns None.
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());

        // Free slot 3. Allocation should reuse it and advance cursor to 4.
        guard.remove(3);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 3);
        assert_eq!(guard.cursor(), 4);

        // Free slots 0 and 1. Allocation returns None because wrapping stays >= 2.
        guard.remove(0);
        guard.remove(1);
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());
    }

    #[fuchsia::test]
    fn test_cyclic_cursor_above_new_max() {
        let idr = Idr::<MockItem>::new_cyclic(None);
        let mut guard = idr.lock();

        // Advance cursor to 80 and allocate.
        guard.set_cursor(80);
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 80);
        assert_eq!(guard.cursor(), 81);

        // Lower max below current cursor.
        guard.set_max(50);

        // Next allocation resets cursor to wrap_min (0) and allocates slot 0.
        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 0);
        assert_eq!(guard.cursor(), 1);
    }

    #[fuchsia::test]
    fn test_max_across_tree_layers() {
        // 70 exceeds layer 0 capacity (64), requiring the tree to grow to layer 1.
        let idr = Idr::<MockItem>::default();
        idr.set_max(70);
        let mut guard = idr.lock();

        for expected in 0..=70 {
            assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, expected);
        }

        // Exhausted up to 70.
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());

        // Update maximal value to 75.
        guard.set_max(75);
        for expected in 71..=75 {
            assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, expected);
        }
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());
    }

    #[fuchsia::test]
    fn test_max_zero() {
        let idr = Idr::<MockItem>::default();
        idr.set_max(0);
        let mut guard = idr.lock();

        assert_eq!(guard.alloc(|value| Arc::new(MockItem { value })).unwrap().0, 0);
        assert!(guard.alloc(|value| Arc::new(MockItem { value })).is_none());
    }
}
