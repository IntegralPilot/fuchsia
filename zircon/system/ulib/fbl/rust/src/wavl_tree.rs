// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ptr_traits::{ManagedPtr, PtrTraits};
use crate::sentinel::{is_sentinel_ptr, make_sentinel, make_sentinel_null, valid_sentinel_ptr};
use crate::size_tracker::{NonTrackingSize, SizeTracker};
use crate::tag::DefaultObjectTag;
use core::borrow::Borrow;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::pin::Pin;
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};

/// Trait defining an observer for a `WavlTree`.
///
/// Observers are used by the test framework to record the number of insert,
/// erase, rank-promote, rank-demote, and rotation operations performed during
/// usage. The default implementation does nothing and is optimized away.
///
/// Observers may also be used to maintain additional application-specific per-node
/// invariants. For example, maintaining subtree min/max values is useful for multikey
/// partition searching.
///
/// Note: Records of promotions and demotions are used by tests to demonstrate
/// that the computational complexity of insert/erase rebalancing is amortized
/// constant. Promotions and demotions which are side effects of the rotation
/// phase of rebalancing are considered to be part of the cost of rotation and
/// are not tallied in the overall promote/demote accounting.
pub trait WavlTreeObserver {
    /// The type pointed to by the tree pointers.
    type Target;

    /// Invoked on the newly inserted node before rebalancing.
    fn record_insert(&self, _node: *mut Self::Target) {}

    /// Invoked on the node to be inserted and each ancestor node while traversing
    /// the tree to find the initial insertion point.
    fn record_insert_traverse(&self, _node: *mut Self::Target, _ancestor: *mut Self::Target) {}

    /// Invoked on the node to be inserted and the colliding node with the same
    /// key, during an insert-or-find operation. This method is mutually exclusive
    /// with `record_insert_replace`, only one or the other is invoked during an
    /// insert operation.
    fn record_insert_collision(&self, _node: *mut Self::Target, _collision: *mut Self::Target) {}

    /// Invoked on an existing node and its replacement, before swapping the
    /// replacement into the tree, during an insert-or-replace operation. This
    /// method is mutually exclusive with `record_insert_collision`, only one or the
    /// other is invoked during an insert operation.
    fn record_insert_replace(&self, _node: *mut Self::Target, _replacement: *mut Self::Target) {}

    /// Invoked after each promotion during post-insert rebalancing.
    fn record_insert_promote(&self) {}

    /// Invoked after a single rotation during post-insert rebalancing.
    fn record_insert_rotation(&self) {}

    /// Invoked after a double rotation during post-insert rebalancing.
    fn record_insert_double_rotation(&self) {}

    /// Invoked on the pivot node, its parent, children, and sibling before a
    /// rotation, just before updating the pointers in the relevant nodes. The
    /// chirality of the children and sibling is relative to the direction of
    /// rotation. The direction of rotation can be determined by comparing these
    /// arguments with the values returned by the left and right child properties
    /// of the pivot or parent arguments.
    ///
    /// The following diagrams the relationship of the nodes in a left rotation:
    ///
    /// ```text
    ///             pivot                          parent                             |
    ///            /     \                         /    \                             |
    ///        parent  rl_child  <-----------  sibling  pivot                         |
    ///        /    \                                   /   \                         |
    ///   sibling  lr_child                       lr_child  rl_child                  |
    /// ```
    ///
    /// In a right rotation, all of the relationships are reflected.
    fn record_rotation(
        &self,
        _pivot: *mut Self::Target,
        _lr_child: *mut Self::Target,
        _rl_child: *mut Self::Target,
        _parent: *mut Self::Target,
        _sibling: *mut Self::Target,
    ) {
    }

    /// Invoked on the node to be erased and the node in the tree where the
    /// augmented invariants become invalid, leading up to the root. Called just
    /// after updating the pointers in the relevant nodes, but before rebalancing.
    ///
    /// The following diagrams the relationship of the erased and invalidated
    /// nodes:
    ///
    /// ```text
    ///        root                                                                   |
    ///       /    \                                                                  |
    ///      A      B    <---- Invalidated starting here on up to the root            |
    ///     / \    / \                                                                |
    ///    C   D  E   F  <---- Erased node                                            |
    /// ```
    ///
    /// When the node to be erased has two children, it is first swapped with the
    /// leftmost child of the righthand subtree. In this case the invalidated node
    /// is the parent of the original leftmost child of the righthand subtree, as
    /// this is the deepest node to change after erasure.
    ///
    /// ```text
    ///        root                       root                                        |
    ///       /    \                     /    \                                       |
    ///      A      B                   A      B                                      |
    ///     / \    / \                 / \    / \                                     |
    ///    C   D  E   F  <--+         C   D  E   H    <---- Invalidated starting here |
    ///              / \    | Swap              / \                                   |
    ///             G   H <-+                  G   F  <---- Erased node               |
    /// ```
    fn record_erase(&self, _node: *mut Self::Target, _invalidated: *mut Self::Target) {}

    /// Invoked after each demotion during post-erase rebalancing.
    fn record_erase_demote(&self) {}

    /// Invoked after each single rotation during post-erase rebalancing.
    fn record_erase_rotation(&self) {}

    /// Invoked after each double rotation during post-erase rebalancing.
    fn record_erase_double_rotation(&self) {}

    /// Invoked during testing to verify WAVL tree rank rules for a given node.
    fn verify_rank_rule(
        &self,
        _node: *mut Self::Target,
        _left_most: *mut Self::Target,
        _right_most: *mut Self::Target,
        _sentinel: *mut Self::Target,
    ) {
    }

    /// Invoked during testing to verify tree balance properties given the tree size and depth.
    fn verify_balance(&self, _size: usize, _depth: usize) {}
}

pub struct DefaultWavlTreeObserver<T>(PhantomData<T>);
impl<T> Default for DefaultWavlTreeObserver<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}
impl<T> WavlTreeObserver for DefaultWavlTreeObserver<T> {
    type Target = T;
}

/// Traits to implement to use `WavlTreeAugmentedInvariantObserver`.
pub trait WavlTreeAugmentedInvariantObserverTraits {
    /// The type of the node.
    type Target;
    /// The type of the invariant value.
    type Value: Copy;

    /// Returns the invariant value for the given node.
    fn get_node_value(node: &Self::Target) -> Self::Value;

    /// Returns the invariant value for the given node's subtree.
    fn get_subtree_value(node: &Self::Target) -> Self::Value;

    /// Combines subtree and node invariant values to produce a new subtree invariant value.
    ///
    /// Values are combined in in-order (left-to-right) sequence:
    /// `left_subtree_value` is combined with `node_value`, and the result is combined
    /// with `right_subtree_value`.
    ///
    /// The operation must be associative: `(a ∘ b) ∘ c == a ∘ (b ∘ c)`.
    /// Non-commutative operations (such as string concatenation or sequence tracking)
    /// are supported because child and node values are strictly combined in
    /// in-order sequence across insertions, erasures, and rotations.
    fn combine_values(a: Self::Value, b: Self::Value) -> Self::Value;

    /// Sets the node's subtree invariant value.
    fn set_subtree_value(node: &mut Self::Target, val: Self::Value);

    /// Resets the node's subtree invariant value.
    fn reset_subtree_value(node: &mut Self::Target);
}

/// A WAVL tree observer that maintains augmented invariants.
pub struct WavlTreeAugmentedInvariantObserver<
    Tag,
    Traits,
    const ALLOW_FIND_COLLISION: bool = true,
    const ALLOW_REPLACE_COLLISION: bool = true,
> {
    _phantom: PhantomData<(Tag, Traits)>,
}

impl<Tag, Traits, const ALLOW_FIND_COLLISION: bool, const ALLOW_REPLACE_COLLISION: bool> Default
    for WavlTreeAugmentedInvariantObserver<
        Tag,
        Traits,
        ALLOW_FIND_COLLISION,
        ALLOW_REPLACE_COLLISION,
    >
{
    fn default() -> Self {
        Self { _phantom: PhantomData }
    }
}

impl<Tag, Traits, const ALLOW_FIND_COLLISION: bool, const ALLOW_REPLACE_COLLISION: bool>
    WavlTreeAugmentedInvariantObserver<Tag, Traits, ALLOW_FIND_COLLISION, ALLOW_REPLACE_COLLISION>
where
    Traits: WavlTreeAugmentedInvariantObserverTraits,
    Traits::Target: WavlTreeContainable<Traits::Target, Tag>,
{
    unsafe fn recompute_until_root(&self, mut current: *mut Traits::Target) {
        while valid_sentinel_ptr(current) {
            let current_ref = unsafe { &*current };
            let node_value = Traits::get_node_value(current_ref);
            let parent = current_ref.get_node().get_parent();
            unsafe {
                self.update_subtree_value(node_value, current);
            }
            current = parent;
        }
    }

    unsafe fn update_subtree_value(&self, mut value: Traits::Value, node: *mut Traits::Target) {
        let node_ref = unsafe { &*node };
        let ns = node_ref.get_node();

        let left = ns.get_left();
        if valid_sentinel_ptr(left) {
            let left_ref = unsafe { &*left };
            let left_val = Traits::get_subtree_value(left_ref);
            value = Traits::combine_values(left_val, value);
        }

        let right = ns.get_right();
        if valid_sentinel_ptr(right) {
            let right_ref = unsafe { &*right };
            let right_val = Traits::get_subtree_value(right_ref);
            value = Traits::combine_values(value, right_val);
        }

        let node_mut = unsafe { &mut *node };
        Traits::set_subtree_value(node_mut, value);
    }
}

#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl<Tag, Traits, const ALLOW_FIND_COLLISION: bool, const ALLOW_REPLACE_COLLISION: bool>
    WavlTreeObserver
    for WavlTreeAugmentedInvariantObserver<
        Tag,
        Traits,
        ALLOW_FIND_COLLISION,
        ALLOW_REPLACE_COLLISION,
    >
where
    Traits: WavlTreeAugmentedInvariantObserverTraits,
    Traits::Target: WavlTreeContainable<Traits::Target, Tag>,
{
    type Target = Traits::Target;

    fn record_insert(&self, node: *mut Self::Target) {
        let node_ref = unsafe { &*node };
        let parent = node_ref.get_node().get_parent();
        let val = Traits::get_node_value(node_ref);
        let node_mut = unsafe { &mut *node };
        Traits::set_subtree_value(node_mut, val);
        if valid_sentinel_ptr(parent) {
            unsafe { self.recompute_until_root(parent) };
        }
    }

    fn record_insert_traverse(&self, _node: *mut Self::Target, _ancestor: *mut Self::Target) {}

    fn record_insert_collision(&self, _node: *mut Self::Target, _collision: *mut Self::Target) {
        debug_assert!(ALLOW_FIND_COLLISION);
    }

    fn record_insert_replace(&self, node: *mut Self::Target, replacement: *mut Self::Target) {
        debug_assert!(ALLOW_REPLACE_COLLISION);
        let replacement_ref = unsafe { &*replacement };

        let parent = unsafe {
            self.update_subtree_value(Traits::get_node_value(replacement_ref), node);
            let ns = (*node).get_node();
            ns.get_parent()
        };
        if valid_sentinel_ptr(parent) {
            unsafe {
                self.recompute_until_root(parent);
            }
        }

        let replacement_mut = unsafe { &mut *replacement };
        let node_mut = unsafe { &mut *node };
        Traits::set_subtree_value(replacement_mut, Traits::get_subtree_value(node_mut));
        Traits::reset_subtree_value(node_mut);
    }

    fn record_rotation(
        &self,
        pivot: *mut Self::Target,
        lr_child: *mut Self::Target,
        _rl_child: *mut Self::Target,
        parent: *mut Self::Target,
        sibling: *mut Self::Target,
    ) {
        let parent_ref = unsafe { &*parent };
        let parent_subtree_val = Traits::get_subtree_value(parent_ref);
        let parent_ns = parent_ref.get_node();

        // Determine rotation direction before links are updated.
        // If pivot is parent's right child, it is a left rotation; otherwise a right rotation.
        let is_left_rotation = parent_ns.get_right() == pivot;
        let (left_child, right_child) = if is_left_rotation {
            // Left rotation: sibling remains left child, lr_child becomes right child
            (sibling, lr_child)
        } else {
            // Right rotation: lr_child becomes left child, sibling remains right child
            (lr_child, sibling)
        };

        let mut parent_value = Traits::get_node_value(parent_ref);

        if valid_sentinel_ptr(left_child) {
            let left_ref = unsafe { &*left_child };
            let left_val = Traits::get_subtree_value(left_ref);
            parent_value = Traits::combine_values(left_val, parent_value);
        }

        if valid_sentinel_ptr(right_child) {
            let right_ref = unsafe { &*right_child };
            let right_val = Traits::get_subtree_value(right_ref);
            parent_value = Traits::combine_values(parent_value, right_val);
        }

        let pivot_mut = unsafe { &mut *pivot };
        Traits::set_subtree_value(pivot_mut, parent_subtree_val);

        let parent_mut = unsafe { &mut *parent };
        Traits::set_subtree_value(parent_mut, parent_value);
    }

    fn record_erase(&self, node: *mut Self::Target, invalidated: *mut Self::Target) {
        if valid_sentinel_ptr(invalidated) {
            unsafe { self.recompute_until_root(invalidated) };
        }
        let node_mut = unsafe { &mut *node };
        Traits::reset_subtree_value(node_mut);
    }
}

/// Trait abstracting WAVL rank operations.
pub trait WavlTreeRank: Copy {
    /// The default rank value for a new node.
    const DEFAULT: Self;
    /// Returns the rank parity (true if odd, false if even).
    fn rank_parity(rank: Self) -> bool;
    /// Promotes the rank by 1.
    fn promote_rank(rank: &mut Self);
    /// Promotes the rank by 2.
    fn double_promote_rank(rank: &mut Self);
    /// Demotes the rank by 1.
    fn demote_rank(rank: &mut Self);
    /// Demotes the rank by 2.
    fn double_demote_rank(rank: &mut Self);
}

impl WavlTreeRank for bool {
    const DEFAULT: Self = false;
    fn rank_parity(rank: Self) -> bool {
        rank
    }
    fn promote_rank(rank: &mut Self) {
        *rank = !*rank;
    }
    fn double_promote_rank(_rank: &mut Self) {} // no-op
    fn demote_rank(rank: &mut Self) {
        *rank = !*rank;
    }
    fn double_demote_rank(_rank: &mut Self) {} // no-op
}

impl WavlTreeRank for i32 {
    const DEFAULT: Self = 0;
    fn rank_parity(rank: Self) -> bool {
        (rank & 1) != 0
    }
    fn promote_rank(rank: &mut Self) {
        *rank += 1;
    }
    fn double_promote_rank(rank: &mut Self) {
        *rank += 2;
    }
    fn demote_rank(rank: &mut Self) {
        *rank -= 1;
    }
    fn double_demote_rank(rank: &mut Self) {
        *rank -= 2;
    }
}

/// A node in a Weak AVL (WAVL) Tree.
#[repr(C)]
pub struct WavlTreeNode<T, R: WavlTreeRank = bool> {
    /// The parent element in the tree.
    pub parent: UnsafeCell<*mut T>,
    /// The left child element in the tree.
    pub left: UnsafeCell<*mut T>,
    /// The right child element in the tree.
    pub right: UnsafeCell<*mut T>,
    /// The integer rank of this node.
    pub rank: UnsafeCell<R>,
}

impl<T, R: WavlTreeRank> WavlTreeNode<T, R> {
    /// Creates a new, unlinked node.
    pub const fn new() -> Self {
        Self {
            parent: UnsafeCell::new(core::ptr::null_mut()),
            left: UnsafeCell::new(core::ptr::null_mut()),
            right: UnsafeCell::new(core::ptr::null_mut()),
            rank: UnsafeCell::new(R::DEFAULT),
        }
    }

    /// Returns true if the node is currently in a tree.
    pub fn in_container(&self) -> bool {
        // SAFETY: Accessing parent pointer from UnsafeCell is safe because WavlTree coordinates
        // exclusive mutations on containment states, ensuring no data races.
        !unsafe { *self.parent.get() }.is_null()
    }

    fn get_parent(&self) -> *mut T {
        // SAFETY: Accessing parent pointer from UnsafeCell is safe because it is only read sequentially
        // or under logical exclusive containment borrow.
        unsafe { *self.parent.get() }
    }

    fn set_parent(&self, parent: *mut T) {
        // SAFETY: Mutating parent pointer in UnsafeCell is safe because the parent container holds
        // exclusive mutable borrow of the containing tree structure.
        unsafe {
            *self.parent.get() = parent;
        }
    }

    fn get_left(&self) -> *mut T {
        // SAFETY: Accessing left pointer from UnsafeCell is safe because it is only read sequentially
        // or under logical exclusive containment borrow.
        unsafe { *self.left.get() }
    }

    fn set_left(&self, left: *mut T) {
        // SAFETY: Mutating left pointer in UnsafeCell is safe because the parent container holds
        // exclusive mutable borrow of the containing tree structure.
        unsafe {
            *self.left.get() = left;
        }
    }

    fn get_right(&self) -> *mut T {
        // SAFETY: Accessing right pointer from UnsafeCell is safe because it is only read sequentially
        // or under logical exclusive containment borrow.
        unsafe { *self.right.get() }
    }

    fn set_right(&self, right: *mut T) {
        // SAFETY: Mutating right pointer in UnsafeCell is safe because the parent container holds
        // exclusive mutable borrow of the containing tree structure.
        unsafe {
            *self.right.get() = right;
        }
    }

    fn rank_parity(&self) -> bool {
        // SAFETY: Reading rank from UnsafeCell is safe because it is only read sequentially or
        // under logical exclusive container borrow.
        unsafe { R::rank_parity(*self.rank.get()) }
    }

    /// Returns the rank value of this node.
    pub fn rank(&self) -> R {
        // SAFETY: Reading rank from UnsafeCell is safe because it is only read sequentially or
        // under logical exclusive container borrow.
        unsafe { *self.rank.get() }
    }

    fn promote_rank(&self) {
        // SAFETY: Mutating rank in UnsafeCell is safe because the parent container holds
        // exclusive mutable borrow of the containing tree structure.
        unsafe {
            R::promote_rank(&mut *self.rank.get());
        }
    }

    fn double_promote_rank(&self) {
        // SAFETY: Mutating rank in UnsafeCell is safe because the parent container holds
        // exclusive mutable borrow of the containing tree structure.
        unsafe {
            R::double_promote_rank(&mut *self.rank.get());
        }
    }

    fn demote_rank(&self) {
        // SAFETY: Mutating rank in UnsafeCell is safe because the parent container holds
        // exclusive mutable borrow of the containing tree structure.
        unsafe {
            R::demote_rank(&mut *self.rank.get());
        }
    }

    fn double_demote_rank(&self) {
        // SAFETY: Mutating rank in UnsafeCell is safe because the parent container holds
        // exclusive mutable borrow of the containing tree structure.
        unsafe {
            R::double_demote_rank(&mut *self.rank.get());
        }
    }

    /// Returns true if the node state invariants are currently valid.
    pub fn is_valid(&self) -> bool {
        let parent = self.get_parent();
        let left = self.get_left();
        let right = self.get_right();
        !parent.is_null() || (parent.is_null() && left.is_null() && right.is_null())
    }
}

impl<T, R: WavlTreeRank> core::fmt::Debug for WavlTreeNode<T, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WavlTreeNode").field("in_container", &self.in_container()).finish()
    }
}

impl<T, R: WavlTreeRank> Default for WavlTreeNode<T, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, R: WavlTreeRank> Drop for WavlTreeNode<T, R> {
    fn drop(&mut self) {
        debug_assert!(!self.in_container(), "Object destroyed while still in container");
    }
}

/// Trait that types must implement to be contained in a `WavlTree`.
pub trait WavlTreeContainable<T, Tag = DefaultObjectTag> {
    /// The rank type used by this node.
    type Rank: WavlTreeRank;
    /// Returns a reference to the tree node.
    fn get_node(&self) -> &WavlTreeNode<T, Self::Rank>;
}

/// Trait that types must implement to expose a key for `WavlTree` sorting and lookup.
pub trait WavlTreeKeyable<K: ?Sized> {
    /// The type of key yielded by `get_key`, borrowing `K`.
    ///
    /// Implementations may return an owned/by-value key (e.g. `(u64, usize)` or `i32`)
    /// or a borrowed/by-reference key (e.g. `&'a KeyStruct`).
    type Key<'a>: Borrow<K>
    where
        Self: 'a;

    /// Returns the key of this object.
    fn get_key(&self) -> Self::Key<'_>;
}

#[allow(dead_code)]
trait LrTraits {
    type Inverse: LrTraits;

    fn lr_child<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut T;
    fn rl_child<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut T;

    fn lr_child_ptr<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut *mut T;
    fn rl_child_ptr<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut *mut T;

    fn lr_most<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>) -> *mut P::Target
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>;

    fn rl_most<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>) -> *mut P::Target
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>;

    unsafe fn set_lr_most<K, P, Tag, S, O>(
        tree: &mut WavlTree<K, P, Tag, S, O>,
        val: *mut P::Target,
    ) where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>;

    unsafe fn set_rl_most<K, P, Tag, S, O>(
        tree: &mut WavlTree<K, P, Tag, S, O>,
        val: *mut P::Target,
    ) where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>;
}

struct ForwardTraits;
struct ReverseTraits;

impl LrTraits for ForwardTraits {
    type Inverse = ReverseTraits;

    fn lr_child<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut T {
        ns.get_left()
    }
    fn rl_child<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut T {
        ns.get_right()
    }

    fn lr_child_ptr<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut *mut T {
        ns.left.get()
    }
    fn rl_child_ptr<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut *mut T {
        ns.right.get()
    }

    fn lr_most<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>) -> *mut P::Target
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.left_most
    }

    fn rl_most<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>) -> *mut P::Target
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.right_most
    }

    unsafe fn set_lr_most<K, P, Tag, S, O>(
        tree: &mut WavlTree<K, P, Tag, S, O>,
        val: *mut P::Target,
    ) where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.left_most = val;
    }

    unsafe fn set_rl_most<K, P, Tag, S, O>(
        tree: &mut WavlTree<K, P, Tag, S, O>,
        val: *mut P::Target,
    ) where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.right_most = val;
    }
}

impl LrTraits for ReverseTraits {
    type Inverse = ForwardTraits;

    fn lr_child<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut T {
        ns.get_right()
    }
    fn rl_child<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut T {
        ns.get_left()
    }

    fn lr_child_ptr<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut *mut T {
        ns.right.get()
    }
    fn rl_child_ptr<T, R: WavlTreeRank>(ns: &WavlTreeNode<T, R>) -> *mut *mut T {
        ns.left.get()
    }

    fn lr_most<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>) -> *mut P::Target
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.right_most
    }

    fn rl_most<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>) -> *mut P::Target
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.left_most
    }

    unsafe fn set_lr_most<K, P, Tag, S, O>(
        tree: &mut WavlTree<K, P, Tag, S, O>,
        val: *mut P::Target,
    ) where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.right_most = val;
    }

    unsafe fn set_rl_most<K, P, Tag, S, O>(
        tree: &mut WavlTree<K, P, Tag, S, O>,
        val: *mut P::Target,
    ) where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        tree.left_most = val;
    }
}

/// A Weak AVL (WAVL) Tree associative container.
///
/// Implementation Notes:
///
/// WAVLTree<> is an implementation of a "Weak AVL" tree; a self
/// balancing binary search tree whose rebalancing algorithm was
/// originally described in
///
/// Bernhard Haeupler, Siddhartha Sen, and Robert E. Tarjan. 2015.
/// Rank-Balanced Trees. ACM Trans. Algorithms 11, 4, Article 30 (June 2015), 26 pages.
/// DOI=http://dx.doi.org/10.1145/2689412
///
/// See also
/// https://en.wikipedia.org/wiki/WAVL_tree
/// http://sidsen.azurewebsites.net/papers/rb-trees-talg.pdf
///
/// WAVLTree<>s, like HashTables, are associative containers and support all of
/// the same key-centric operations (such as find() and insert_or_find()) that
/// HashTables support.
///
/// Additionally, WAVLTree's are internally ordered by key (unlike HashTables
/// which are un-ordered).  Iteration forwards or backwards runs in amortized
/// constant time, but in O(log) time in an individual worst case.  Forward
/// iteration will enumerate the elements in monotonically increasing order (as
/// defined by the KeyTraits::LessThan operation).
///
/// Two additional operations are supported because of the ordered nature of a
/// WAVLTree:
/// upper_bound(key)        : Returns a cursor positioned at the first element (E) in the tree such that E.key > key.
/// lower_bound(key)        : Returns a cursor positioned at the first element (E) in the tree such that E.key >= key.
///
/// The worst depth of a WAVL tree depends on whether or not the tree has ever
/// been subject to erase operations.
///
/// ++ If the tree has seen only insert operations, the worst case depth of the
///    tree is log_phi(N), where phi is the golden ratio.  This is the same bound
///    as that of an AVL tree.
/// ++ If the tree has seen erase operations in addition to insert operations,
///    the worst case depth of the tree is 2*log_2(N).  This is the same bound as
///    a Red-Black tree.
///
/// Insertion runs in O(log) time; finding the location takes O(log) time while
/// post-insert rebalancing runs in amortized constant time.
///
/// Erase-by-key runs in O(log) time; finding the node to erase takes O(log) time
/// while post-erase rebalancing runs in amortized constant time.
///
/// Because of the intrusive nature of the container, direct-erase operations
/// (AKA, erase operations where the reference to the element to be erased is
/// already known) run in amortized constant time.
type TargetRank<P, Tag> =
    <<P as PtrTraits>::Target as WavlTreeContainable<<P as PtrTraits>::Target, Tag>>::Rank;

#[repr(C)]
#[pin_data(PinnedDrop)]
pub struct WavlTree<
    K,
    P,
    Tag = DefaultObjectTag,
    S = NonTrackingSize,
    O = DefaultWavlTreeObserver<<P as PtrTraits>::Target>,
> where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    root: *mut P::Target,
    left_most: *mut P::Target,
    right_most: *mut P::Target,
    size: S,
    observer: O,
    #[pin]
    _pin: core::marker::PhantomPinned,
    _phantom: core::marker::PhantomData<(K, P, Tag)>,
}

impl<K, P, Tag, S, O> WavlTree<K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    /// Creates a new, empty tree with a custom observer.
    pub fn new_with_observer(observer: O) -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(&this in Self {
            root: core::ptr::null_mut(),
            left_most: make_sentinel(this.as_ptr()),
            right_most: make_sentinel(this.as_ptr()),
            size: S::INIT,
            observer,
            _pin: core::marker::PhantomPinned,
            _phantom: core::marker::PhantomData,
        })
    }

    /// Creates a new, empty tree.
    pub fn new() -> impl PinInit<Self, core::convert::Infallible>
    where
        O: Default,
    {
        Self::new_with_observer(O::default())
    }

    fn get_sentinel(&self) -> *mut P::Target {
        make_sentinel(self as *const Self as *mut Self)
    }

    /// Returns a reference to the node state of `ptr`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid, aligned, and dereferenceable pointer
    /// to an initialized `P::Target` object that is alive for the lifetime `'a`.
    unsafe fn get_node_ref<'a>(
        ptr: *mut P::Target,
    ) -> &'a WavlTreeNode<P::Target, TargetRank<P, Tag>> {
        // SAFETY: The caller guarantees that `ptr` is valid, aligned, and dereferenceable.
        unsafe { &(*ptr) }.get_node()
    }

    /// Returns true if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.root.is_null()
    }

    /// Returns a reference to the first (smallest) element of the tree, or `None` if it is empty.
    pub fn front(&self) -> Option<&P::Target> {
        // SAFETY: `self.left_most` is a valid pointer to a node in the tree or the sentinel.
        // If `self.is_empty()` is false, it is guaranteed to be a valid, dereferenceable
        // pointer to a node.
        if self.is_empty() { None } else { unsafe { Some(&*self.left_most) } }
    }

    /// Returns a reference to the last (largest) element of the tree, or `None` if it is empty.
    pub fn back(&self) -> Option<&P::Target> {
        // SAFETY: `self.right_most` is a valid pointer to a node in the tree or the sentinel.
        // If `self.is_empty()` is false, it is guaranteed to be a valid, dereferenceable
        // pointer to a node.
        if self.is_empty() { None } else { unsafe { Some(&*self.right_most) } }
    }

    /// Returns a mutable pointer to the pointer linking `node` into the tree.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `node` is a valid, aligned, and dereferenceable pointer
    /// to a node that is currently contained within this tree instance.
    unsafe fn get_link_ptr_to_node(&mut self, node: *mut P::Target) -> *mut *mut P::Target {
        debug_assert!(valid_sentinel_ptr(node));

        // SAFETY: The caller guarantees that `node` is a valid pointer to a node currently in the tree.
        let ns = unsafe { Self::get_node_ref(node) };
        let parent = ns.get_parent();
        if is_sentinel_ptr(parent) {
            debug_assert_eq!(parent, self.get_sentinel());
            debug_assert_eq!(self.root, node);
            &mut self.root as *mut _
        } else {
            debug_assert!(!parent.is_null());
            // SAFETY: `parent` is not a sentinel and is not null, so it must be a valid,
            // dereferenceable pointer to a node in the tree.
            let parent_ns = unsafe { Self::get_node_ref(parent) };
            if parent_ns.get_left() == node {
                parent_ns.left.get()
            } else {
                debug_assert_eq!(parent_ns.get_right(), node);
                parent_ns.right.get()
            }
        }
    }

    /// Performs a tree rotation in the direction specified by `LR`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `node` and `parent` are valid, aligned, and dereferenceable
    /// pointers to nodes currently contained within this tree instance, and that `node` is
    /// a child of `parent` in the direction specified by `LR::Inverse`.
    unsafe fn rotate_lr<LR: LrTraits>(&mut self, node: *mut P::Target, parent: *mut P::Target) {
        // SAFETY: The caller guarantees that `node` and `parent` are valid pointers to nodes
        // currently in the tree. The structural modifications (rotations) correctly permute the
        // parent/child links while maintaining BST structural validity.
        unsafe {
            debug_assert!(valid_sentinel_ptr(node));
            debug_assert!(valid_sentinel_ptr(parent));

            let x = node;
            let z = parent;

            let x_ns = Self::get_node_ref(x);
            let z_ns = Self::get_node_ref(z);

            debug_assert_eq!(LR::rl_child(z_ns), x);

            let x_link = LR::rl_child_ptr(z_ns);
            let y_link = LR::lr_child_ptr(x_ns);
            let z_link = self.get_link_ptr_to_node(z);

            let g = z_ns.get_parent();
            let y = *y_link;

            debug_assert!(!is_sentinel_ptr(y));

            // Permute the downstream links.
            self.observer.record_rotation(x, y, LR::rl_child(x_ns), z, LR::lr_child(z_ns));
            let tmp = *x_link;
            *x_link = *y_link;
            *y_link = *z_link;
            *z_link = tmp;

            // Update parent pointers.
            x_ns.set_parent(g);
            z_ns.set_parent(x);
            if !y.is_null() {
                Self::get_node_ref(y).set_parent(z);
            }
        }
    }

    /// Performs post-insertion balancing fixups.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `node` and `parent` are valid, aligned, and dereferenceable
    /// pointers to nodes currently contained within this tree instance, and that `node` is
    /// a child of `parent` in the direction specified by `LR`.
    unsafe fn post_insert_fixup_lr<LR: LrTraits>(
        &mut self,
        node: *mut P::Target,
        parent: *mut P::Target,
    ) {
        type RL<LR> = <LR as LrTraits>::Inverse;

        // SAFETY: The caller guarantees `node` and `parent` are valid pointers to nodes currently
        // in the tree. The subsequent operations (including rotations and rank updates) correctly
        // rebalance the tree according to WAVL insertion balance algorithms.
        unsafe {
            debug_assert!(valid_sentinel_ptr(node));
            debug_assert!(valid_sentinel_ptr(parent));

            let node_ns = Self::get_node_ref(node);
            let parent_ns = Self::get_node_ref(parent);

            debug_assert_eq!(LR::lr_child(parent_ns), node);

            let rl_child = LR::rl_child(node_ns);
            let rl_child_ns = if valid_sentinel_ptr(rl_child) {
                Some(Self::get_node_ref(rl_child))
            } else {
                None
            };

            if rl_child_ns.is_none()
                || (rl_child_ns.unwrap().rank_parity() == node_ns.rank_parity())
            {
                // Case #1: single rotation.
                self.rotate_lr::<RL<LR>>(node, parent);
                parent_ns.demote_rank();
                self.observer.record_insert_rotation();
            } else {
                // Case #2: double rotation.
                let rl_child_ns = rl_child_ns.unwrap();
                self.rotate_lr::<LR>(rl_child, node);
                self.rotate_lr::<RL<LR>>(rl_child, parent);

                rl_child_ns.promote_rank();
                node_ns.demote_rank();
                parent_ns.demote_rank();
                self.observer.record_insert_double_rotation();
            }
        }
    }

    /// Rebalances the tree after an element is inserted.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `node` is a valid, aligned, and dereferenceable pointer
    /// to a node that has just been inserted into this tree instance, and that the tree
    /// is structurally valid except for potential balance violations at `node` and its ancestors.
    unsafe fn balance_post_insert(&mut self, mut node: *mut P::Target) {
        // SAFETY: The caller guarantees `node` is a valid pointer to a node in the tree.
        // The loop climbs the tree, updating ranks and performing rotations as needed, maintaining
        // tree integrity.
        unsafe {
            let mut node_ns = Self::get_node_ref(node);
            debug_assert!(valid_sentinel_ptr(node_ns.get_parent()));

            let mut parent = node_ns.get_parent();
            let mut parent_ns = Self::get_node_ref(parent);

            if valid_sentinel_ptr(parent_ns.get_left()) && valid_sentinel_ptr(parent_ns.get_right())
            {
                return;
            }

            let mut node_parity;
            let mut parent_parity;
            let mut sibling_parity;
            let mut is_left_child;

            loop {
                // Promote.
                parent_ns.promote_rank();
                self.observer.record_insert_promote();

                // Climb.
                node = parent;
                node_ns = Self::get_node_ref(node);
                parent = node_ns.get_parent();

                if !valid_sentinel_ptr(parent) {
                    return;
                }

                parent_ns = Self::get_node_ref(parent);
                is_left_child = parent_ns.get_left() == node;
                if is_left_child {
                    sibling_parity = if valid_sentinel_ptr(parent_ns.get_right()) {
                        Self::get_node_ref(parent_ns.get_right()).rank_parity()
                    } else {
                        true
                    };
                } else {
                    debug_assert_eq!(parent_ns.get_right(), node);
                    sibling_parity = if valid_sentinel_ptr(parent_ns.get_left()) {
                        Self::get_node_ref(parent_ns.get_left()).rank_parity()
                    } else {
                        true
                    };
                }

                node_parity = node_ns.rank_parity();
                parent_parity = parent_ns.rank_parity();

                if !((!node_parity && !parent_parity && sibling_parity)
                    || (node_parity && parent_parity && !sibling_parity))
                {
                    break;
                }
            }

            if (node_parity != parent_parity) || (node_parity != sibling_parity) {
                return;
            }

            if is_left_child {
                self.post_insert_fixup_lr::<ForwardTraits>(node, parent);
            } else {
                self.post_insert_fixup_lr::<ReverseTraits>(node, parent);
            }
        }
    }

    /// Performs balance adjustments for a "2-2 leaf" node after erasure.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `node` is a valid, aligned, and dereferenceable pointer
    /// to a node currently contained within this tree instance, and that the tree is structurally
    /// valid except for potential balance violations after an erasure at `node`.
    unsafe fn balance_post_erase_fix_22_leaf(&mut self, node: *mut P::Target) {
        // SAFETY: The caller guarantees `node` is a valid pointer to a node in the tree.
        // The function safely demotes the rank and propagates rebalancing up the tree.
        unsafe {
            debug_assert!(valid_sentinel_ptr(node));

            let ns = Self::get_node_ref(node);
            if !ns.rank_parity()
                || valid_sentinel_ptr(ns.get_left())
                || valid_sentinel_ptr(ns.get_right())
            {
                return;
            }

            ns.demote_rank();
            self.observer.record_erase_demote();

            let parent = ns.get_parent();
            debug_assert!(!parent.is_null());
            if is_sentinel_ptr(parent) {
                return;
            }

            let parent_ns = Self::get_node_ref(parent);
            let is_left_child = parent_ns.get_left() == node;
            debug_assert!(is_left_child || parent_ns.get_right() == node);

            if is_left_child {
                self.balance_post_erase_fix_lr_3_child::<ForwardTraits>(parent);
            } else {
                self.balance_post_erase_fix_lr_3_child::<ReverseTraits>(parent);
            }
        }
    }

    /// Rebalances the tree after an element is erased, resolving violations of the 3-child rule.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `node` is a valid, aligned, and dereferenceable pointer
    /// to a node currently contained within this tree instance, and that `node` is the parent
    /// of a subtree that has just seen an erasure and violates the 3-child balance rule.
    unsafe fn balance_post_erase_fix_lr_3_child<LR: LrTraits>(&mut self, node: *mut P::Target) {
        type RL<LR> = <LR as LrTraits>::Inverse;
        // SAFETY: The caller guarantees `node` is a valid pointer to a node in the tree.
        // The loop walks up the tree performing rank adjustments and triggers rotations as required
        // by the WAVL erase balancing algorithms.
        unsafe {
            debug_assert!(valid_sentinel_ptr(node));

            let mut z = node;
            let mut z_ns = Self::get_node_ref(z);
            let mut x = LR::lr_child(z_ns);

            if valid_sentinel_ptr(x) != z_ns.rank_parity() {
                return;
            }

            let mut x_is_lr_child = true;
            let mut y = LR::rl_child(z_ns);

            loop {
                debug_assert!(valid_sentinel_ptr(y));

                let y_ns = Self::get_node_ref(y);
                let y_is_2_child = y_ns.rank_parity() == z_ns.rank_parity();

                if !y_is_2_child {
                    let y_is_22_node;
                    if y_ns.rank_parity() {
                        y_is_22_node = (!valid_sentinel_ptr(y_ns.get_left())
                            || Self::get_node_ref(y_ns.get_left()).rank_parity())
                            && (!valid_sentinel_ptr(y_ns.get_right())
                                || Self::get_node_ref(y_ns.get_right()).rank_parity());
                    } else {
                        y_is_22_node = valid_sentinel_ptr(y_ns.get_left())
                            && valid_sentinel_ptr(y_ns.get_right())
                            && !Self::get_node_ref(y_ns.get_left()).rank_parity()
                            && !Self::get_node_ref(y_ns.get_right()).rank_parity();
                    }

                    if !y_is_22_node {
                        break;
                    }
                }

                z_ns.demote_rank();
                self.observer.record_erase_demote();
                if !y_is_2_child {
                    y_ns.demote_rank();
                    self.observer.record_erase_demote();
                }

                if !valid_sentinel_ptr(z_ns.get_parent()) {
                    return;
                }

                let x_rank_parity = z_ns.rank_parity();
                x = z;
                z = z_ns.get_parent();
                z_ns = Self::get_node_ref(z);

                if z_ns.rank_parity() == x_rank_parity {
                    return;
                }

                x_is_lr_child = LR::lr_child(z_ns) == x;
                y = if x_is_lr_child { LR::rl_child(z_ns) } else { LR::lr_child(z_ns) };
            }

            if x_is_lr_child {
                self.balance_post_erase_do_rotations::<LR>(y, z);
            } else {
                self.balance_post_erase_do_rotations::<RL<LR>>(y, z);
            }
        }
    }

    /// Performs necessary rotations during post-erase rebalancing.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `y` and `z` are valid, aligned, and dereferenceable
    /// pointers to nodes currently contained within this tree instance, and that `y` is the
    /// right child of `z` in the direction specified by `LR`.
    unsafe fn balance_post_erase_do_rotations<LR: LrTraits>(
        &mut self,
        y: *mut P::Target,
        z: *mut P::Target,
    ) {
        type RL<LR> = <LR as LrTraits>::Inverse;
        // SAFETY: The caller guarantees `y` and `z` are valid pointers to nodes in the tree.
        // The rotations correctly rebalance the tree at `z` and update ranks accordingly.
        unsafe {
            debug_assert!(valid_sentinel_ptr(y));
            debug_assert!(valid_sentinel_ptr(z));

            let y_ns = Self::get_node_ref(y);
            let z_ns = Self::get_node_ref(z);

            let w = LR::rl_child(y_ns);
            let w_rank_parity =
                if valid_sentinel_ptr(w) { Self::get_node_ref(w).rank_parity() } else { true };

            if y_ns.rank_parity() != w_rank_parity {
                self.rotate_lr::<LR>(y, z);
                y_ns.promote_rank();

                if !valid_sentinel_ptr(z_ns.get_left()) && !valid_sentinel_ptr(z_ns.get_right()) {
                    z_ns.double_demote_rank();
                } else {
                    z_ns.demote_rank();
                }
                self.observer.record_erase_rotation();
            } else {
                let v = LR::lr_child(y_ns);
                debug_assert!(valid_sentinel_ptr(v));
                let v_ns = Self::get_node_ref(v);
                debug_assert_ne!(v_ns.rank_parity(), y_ns.rank_parity());

                self.rotate_lr::<RL<LR>>(v, y);
                self.rotate_lr::<LR>(v, z);

                v_ns.double_promote_rank();
                y_ns.demote_rank();
                z_ns.double_demote_rank();
                self.observer.record_erase_double_rotation();
            }
        }
    }

    /// Promotes the single child of `node` (in direction `LR`) to take `node`'s place in the tree.
    ///
    /// # Safety
    ///
    /// - `owner` must be a valid, aligned, dereferenceable pointer to a `*mut P::Target`.
    /// - `*owner` must be `null_mut()`.
    /// - `node` must be a valid, aligned, dereferenceable pointer to a node currently in the tree.
    /// - `node` must have exactly one child in the direction specified by `LR` (which must be
    ///   a valid non-null, non-sentinel node), and must NOT have a valid child in the opposite direction.
    unsafe fn promote_lr_child<LR: LrTraits>(
        &mut self,
        owner: *mut *mut P::Target,
        node: *mut P::Target,
    ) {
        // SAFETY: The caller must guarantee that `owner` is a valid, aligned, dereferenceable pointer
        // to `*mut P::Target` containing `null_mut()`, that `node` is a valid node in the tree,
        // and that the node has exactly one child in the `LR` direction to promote.
        unsafe {
            debug_assert!((*owner).is_null());
            debug_assert!(valid_sentinel_ptr(node));

            let ns = Self::get_node_ref(node);
            let lr_child_ptr = LR::lr_child_ptr(ns);
            let rl_child_ptr = LR::rl_child_ptr(ns);

            debug_assert!(valid_sentinel_ptr(*lr_child_ptr) && !valid_sentinel_ptr(*rl_child_ptr));

            *owner = *lr_child_ptr;
            *lr_child_ptr = core::ptr::null_mut();
            Self::get_node_ref(*owner).set_parent(ns.get_parent());

            let rl_most = LR::rl_most(self);
            debug_assert_eq!(rl_most == node, is_sentinel_ptr(*rl_child_ptr));

            if is_sentinel_ptr(*rl_child_ptr) {
                let mut replacement = *owner;
                let mut next_rl_child_ptr;

                loop {
                    let replacement_ns = Self::get_node_ref(replacement);
                    next_rl_child_ptr = LR::rl_child_ptr(replacement_ns);

                    debug_assert!(!is_sentinel_ptr(*next_rl_child_ptr));
                    if (*next_rl_child_ptr).is_null() {
                        break;
                    }
                    replacement = *next_rl_child_ptr;
                }

                LR::set_rl_most(self, replacement);
                *next_rl_child_ptr = self.get_sentinel();
                *rl_child_ptr = core::ptr::null_mut();
            }

            ns.set_parent(core::ptr::null_mut());
            debug_assert!(ns.get_left().is_null());
            debug_assert!(ns.get_right().is_null());
        }
    }

    /// Physically swaps the position of `node1` (pointed to by `ptr_ref1`) with `node2`
    /// (pointed to by `ptr_ref2`) in the tree's pointer structure.
    ///
    /// E.g. `node2` must be the leftmost descendant of the right child of `node1`.
    ///
    /// Returns the pointer to the slot originally containing `node2` (which now contains `node1`).
    ///
    /// # Safety
    ///
    /// - `ptr_ref1` must be a valid, aligned, dereferenceable pointer to `*mut P::Target`
    ///   which contains a valid, aligned, dereferenceable pointer to `node1`.
    /// - `ptr_ref2` must be a valid, aligned, dereferenceable pointer to `*mut P::Target`
    ///   which contains a valid, aligned, dereferenceable pointer to `node2`.
    /// - `node2` must be a descendant of `node1`'s right subtree.
    /// - Both `node1` and `node2` must reside within the same tree.
    unsafe fn swap_with_right_descendant(
        &mut self,
        ptr_ref1: *mut *mut P::Target,
        ptr_ref2: *mut *mut P::Target,
    ) -> *mut *mut P::Target {
        // SAFETY: The caller must guarantee that `ptr_ref1` and `ptr_ref2` are valid, aligned
        // pointers pointing to valid nodes in the tree, and that `node2` is a descendant in the
        // right subtree of `node1`. This method performs structural pointer manipulation to swap
        // the nodes physically, preserving local tree structure.
        unsafe {
            let node1 = *ptr_ref1;
            let node2 = *ptr_ref2;

            let ns1 = Self::get_node_ref(node1);
            let ns2 = Self::get_node_ref(node2);

            if ns1.get_right().is_null() {
                panic!("node1 right is NULL inside swap");
            }

            let ns1_lp = if valid_sentinel_ptr(ns1.get_left()) {
                Self::get_node_ref(ns1.get_left()).parent.get()
            } else {
                core::ptr::null_mut()
            };

            let ns2_lp = if valid_sentinel_ptr(ns2.get_left()) {
                Self::get_node_ref(ns2.get_left()).parent.get()
            } else {
                core::ptr::null_mut()
            };

            let ns2_rp = if valid_sentinel_ptr(ns2.get_right()) {
                Self::get_node_ref(ns2.get_right()).parent.get()
            } else {
                core::ptr::null_mut()
            };

            let r1 = ns1.get_right();
            if !valid_sentinel_ptr(r1) {
                if r1.is_null() {
                    panic!("ns1.get_right() is NULL");
                } else if is_sentinel_ptr(r1) {
                    panic!("ns1.get_right() is SENTINEL");
                } else {
                    panic!("ns1.get_right() is OTHER INVALID");
                }
            }
            let ns1_rp = Self::get_node_ref(ns1.get_right()).parent.get();

            if node1 == self.left_most {
                self.left_most = node2;
            }
            if node2 == self.right_most {
                self.right_most = node1;
            }

            // Swap parent.
            let parent_tmp = ns1.get_parent();
            ns1.set_parent(ns2.get_parent());
            ns2.set_parent(parent_tmp);

            // Swap left.
            let left_tmp = ns1.get_left();
            ns1.set_left(ns2.get_left());
            ns2.set_left(left_tmp);

            // Swap right.
            let right_tmp = ns1.get_right();
            ns1.set_right(ns2.get_right());
            ns2.set_right(right_tmp);

            // Swap rank.
            let rank_tmp = *ns1.rank.get();
            *ns1.rank.get() = *ns2.rank.get();
            *ns2.rank.get() = rank_tmp;

            if !ns1_lp.is_null() {
                *ns1_lp = node2;
            }
            if !ns2_lp.is_null() {
                *ns2_lp = node1;
            }
            if !ns2_rp.is_null() {
                *ns2_rp = node1;
            }

            if ptr_ref2 != ns1.right.get() {
                #[allow(clippy::swap_ptr_to_ref)]
                core::mem::swap(&mut *ptr_ref1, &mut *ptr_ref2);
                *ns1_rp = node2;
                ptr_ref2
            } else {
                debug_assert_eq!(*ns1.parent.get(), node1);
                debug_assert_eq!(*ns2.right.get(), node2);
                #[allow(clippy::swap_ptr_to_ref)]
                core::mem::swap(&mut *ptr_ref1, &mut *ns2.right.get());
                *ns1.parent.get() = node2;
                ns2.right.get()
            }
        }
    }

    /// Inserts a new node `ptr` into the WAVL tree.
    ///
    /// If a node with an identical key already exists, does not insert it, stores the colliding node's
    /// pointer in `collision`, and returns the original `ptr` as `Err(ptr)`.
    ///
    /// # Safety
    ///
    /// - `ptr` must wrap a valid, properly aligned, dereferenceable node.
    /// - The node must NOT be currently contained in this or any other intrusive container.
    /// - `collision` must be a valid, aligned, dereferenceable pointer to `*mut P::Target`.
    unsafe fn internal_insert(&mut self, ptr: P, collision: &mut *mut P::Target) -> Result<(), P> {
        // SAFETY: The caller guarantees that `ptr` represents a valid, unlinked node, and that
        // `collision` is a valid slot. Dereferencing pointers and mutating the parent/child links
        // preserves tree structure.
        unsafe {
            let raw = P::into_raw(ptr);
            debug_assert!(!raw.is_null());

            let ns = Self::get_node_ref(raw);
            debug_assert!(ns.is_valid() && !ns.in_container());

            *ns.rank.get() = <TargetRank<P, Tag>>::DEFAULT;

            if self.root.is_null() {
                ns.set_parent(self.get_sentinel());
                ns.set_left(self.get_sentinel());
                ns.set_right(self.get_sentinel());

                debug_assert!(is_sentinel_ptr(self.left_most) && is_sentinel_ptr(self.right_most));
                self.left_most = raw;
                self.right_most = raw;

                self.root = raw;
                self.size.increment();
                self.observer.record_insert(raw);
                return Ok(());
            }

            let key = (*raw).get_key();
            let key_borrow = key.borrow();
            let mut is_left_most = true;
            let mut is_right_most = true;
            let mut parent = self.root;
            let mut owner: *mut *mut P::Target;

            loop {
                let parent_key = (*parent).get_key();
                let parent_key_borrow = parent_key.borrow();
                self.observer.record_insert_traverse(raw, parent);

                if key_borrow == parent_key_borrow {
                    *collision = parent;
                    self.observer.record_insert_collision(raw, parent);
                    return Err(P::from_raw(raw));
                }

                let parent_ns = Self::get_node_ref(parent);

                if key_borrow < parent_key_borrow {
                    owner = parent_ns.left.get();
                    is_right_most = false;
                } else {
                    owner = parent_ns.right.get();
                    is_left_most = false;
                }

                if !valid_sentinel_ptr(*owner) {
                    break;
                }

                parent = *owner;
            }

            debug_assert!(!is_left_most || !is_right_most);

            if is_right_most {
                debug_assert!(is_sentinel_ptr(*owner));
                ns.set_right(self.get_sentinel());
                self.right_most = raw;
            } else if is_left_most {
                debug_assert!(is_sentinel_ptr(*owner));
                ns.set_left(self.get_sentinel());
                self.left_most = raw;
            }

            debug_assert!(!valid_sentinel_ptr(*owner));
            ns.set_parent(parent);

            *owner = raw;
            self.size.increment();
            self.observer.record_insert(raw);

            self.balance_post_insert(*owner);
            Ok(())
        }
    }

    /// Removes the node `ptr` from the WAVL tree, rebalancing if necessary.
    ///
    /// Returns the node wrapped in `P` if it was successfully erased and returned.
    ///
    /// # Safety
    ///
    /// - `ptr` must be a valid, properly aligned, dereferenceable raw pointer to a node.
    /// - If the node is not null or sentinel, it MUST be currently contained within this tree.
    unsafe fn internal_erase(&mut self, ptr: *mut P::Target) -> Option<P> {
        // SAFETY: The caller guarantees that `ptr` points to a valid node belonging to this tree.
        // Removing it involves swapping it out structurally, repairing BST/WAVL invariants,
        // and reclaiming ownership via `P::from_raw`.
        unsafe {
            if !valid_sentinel_ptr(ptr) {
                return None;
            }

            let ns = Self::get_node_ref(ptr);
            let mut owner = self.get_link_ptr_to_node(ptr);
            debug_assert_eq!(*owner, ptr);

            if valid_sentinel_ptr(ns.get_left()) && valid_sentinel_ptr(ns.get_right()) {
                let mut new_owner = ns.right.get();
                let mut new_ns = Self::get_node_ref(ns.get_right());

                while !new_ns.get_left().is_null() {
                    debug_assert!(!is_sentinel_ptr(new_ns.get_left()));
                    new_owner = new_ns.left.get();
                    new_ns = Self::get_node_ref(*new_owner);
                }

                owner = self.swap_with_right_descendant(owner, new_owner);
                debug_assert_eq!(*owner, ptr);
            }

            let parent = ns.get_parent();
            let was_one_child;
            let was_left_child;

            debug_assert!(!parent.is_null());
            if !is_sentinel_ptr(parent) {
                let parent_ns = Self::get_node_ref(parent);
                was_one_child = ns.rank_parity() != parent_ns.rank_parity();
                was_left_child = parent_ns.left.get() == owner;
            } else {
                was_one_child = false;
                was_left_child = false;
            }

            *owner = core::ptr::null_mut();

            let target = ptr;
            if valid_sentinel_ptr(ns.get_left()) {
                self.promote_lr_child::<ForwardTraits>(owner, target);
            } else if valid_sentinel_ptr(ns.get_right()) {
                self.promote_lr_child::<ReverseTraits>(owner, target);
            } else {
                debug_assert_eq!(is_sentinel_ptr(ns.get_left()), self.left_most == target);
                debug_assert_eq!(is_sentinel_ptr(ns.get_right()), self.right_most == target);

                if is_sentinel_ptr(ns.get_left()) {
                    if is_sentinel_ptr(ns.get_right()) {
                        if S::IS_TRACKING {
                            debug_assert_eq!(self.size.get(), 1);
                        }
                        debug_assert!(is_sentinel_ptr(ns.get_parent()));
                        self.left_most = self.get_sentinel();
                        self.right_most = self.get_sentinel();
                        ns.set_left(core::ptr::null_mut());
                        ns.set_right(core::ptr::null_mut());
                    } else {
                        debug_assert!(valid_sentinel_ptr(ns.get_parent()));
                        debug_assert!(ns.get_right().is_null());
                        self.left_most = ns.get_parent();
                        *owner = ns.get_left();
                        ns.set_left(core::ptr::null_mut());
                    }
                } else if is_sentinel_ptr(ns.get_right()) {
                    debug_assert!(valid_sentinel_ptr(ns.get_parent()));
                    debug_assert!(ns.get_left().is_null());
                    self.right_most = ns.get_parent();
                    *owner = ns.get_right();
                    ns.set_right(core::ptr::null_mut());
                }

                ns.set_parent(core::ptr::null_mut());
            }

            debug_assert!(ns.is_valid() && !ns.in_container());
            self.observer.record_erase(target, parent);

            self.size.decrement();

            if !is_sentinel_ptr(parent) {
                if was_one_child {
                    self.balance_post_erase_fix_22_leaf(parent);
                } else {
                    if was_left_child {
                        self.balance_post_erase_fix_lr_3_child::<ForwardTraits>(parent);
                    } else {
                        self.balance_post_erase_fix_lr_3_child::<ReverseTraits>(parent);
                    }
                }
            }

            Some(P::from_raw(target))
        }
    }

    /// Replaces `old_node` with `new_node` in the tree's pointer structure.
    ///
    /// Returns the `old_node` wrapped in `P`.
    ///
    /// # Safety
    ///
    /// - `old_node` must be a valid, aligned, dereferenceable pointer to a node currently
    ///   contained within this tree.
    /// - `new_node` must be a valid node not currently contained in this or any other tree.
    /// - The key of `new_node` must exactly match the key of `old_node` to preserve the
    ///   Binary Search Tree (BST) ordering invariant.
    unsafe fn internal_swap(&mut self, old_node: *mut P::Target, new_node: P) -> Option<P> {
        // SAFETY: The caller must guarantee that `old_node` is in this tree, that `new_node`
        // is unlinked, and that their keys match. This method updates all parent and child
        // links to point to the new node, and reclaims ownership of `old_node`.
        unsafe {
            debug_assert!(!old_node.is_null());
            let new_raw = P::into_raw(new_node);
            debug_assert!(!new_raw.is_null());
            debug_assert!((*old_node).get_key().borrow() == (*new_raw).get_key().borrow());

            let old_ns = Self::get_node_ref(old_node);
            let new_ns = Self::get_node_ref(new_raw);

            debug_assert!(old_ns.in_container());
            debug_assert!(!new_ns.in_container());
            self.observer.record_insert_replace(old_node, new_raw);

            if valid_sentinel_ptr(old_ns.get_left()) {
                Self::get_node_ref(old_ns.get_left()).set_parent(new_raw);
            } else {
                if is_sentinel_ptr(old_ns.get_left()) {
                    debug_assert_eq!(self.left_most, old_node);
                    self.left_most = new_raw;
                }
            }
            new_ns.set_left(old_ns.get_left());
            old_ns.set_left(core::ptr::null_mut());

            if valid_sentinel_ptr(old_ns.get_right()) {
                Self::get_node_ref(old_ns.get_right()).set_parent(new_raw);
            } else {
                if is_sentinel_ptr(old_ns.get_right()) {
                    debug_assert_eq!(self.right_most, old_node);
                    self.right_most = new_raw;
                }
            }
            new_ns.set_right(old_ns.get_right());
            old_ns.set_right(core::ptr::null_mut());

            *new_ns.rank.get() = *old_ns.rank.get();

            *self.get_link_ptr_to_node(old_node) = new_raw;
            new_ns.set_parent(old_ns.get_parent());
            old_ns.set_parent(core::ptr::null_mut());

            Some(P::from_raw(old_node))
        }
    }

    /// Advances the node pointer `node` in-place to the next node in-order
    /// (according to the direction specified by `LR`).
    ///
    /// # Safety
    ///
    /// - `node` must be a valid, aligned, dereferenceable pointer to a raw pointer `*node`.
    /// - `*node` must point to a valid node currently contained within this tree.
    unsafe fn advance<LR: LrTraits>(node: &mut *mut P::Target) {
        // SAFETY: The caller must ensure that `*node` is a valid pointer to a node in this tree.
        // Traveling through parent/child links is safe as long as the tree structure is valid
        // and the node belongs to the tree.
        unsafe {
            debug_assert!(valid_sentinel_ptr(*node));

            let mut ns = Self::get_node_ref(*node);
            let rl_child = LR::rl_child(ns);
            if !rl_child.is_null() {
                *node = rl_child;

                if is_sentinel_ptr(*node) {
                    return;
                }

                let mut lr_child = LR::lr_child(Self::get_node_ref(*node));
                while !lr_child.is_null() {
                    debug_assert!(!is_sentinel_ptr(lr_child));
                    *node = lr_child;
                    lr_child = LR::lr_child(Self::get_node_ref(*node));
                }
                return;
            }

            let mut done;
            ns = Self::get_node_ref(*node);
            loop {
                debug_assert!(valid_sentinel_ptr(ns.get_parent()));

                let parent_ns = Self::get_node_ref(ns.get_parent());
                done = LR::lr_child(parent_ns) == *node;

                debug_assert!(done || LR::rl_child(parent_ns) == *node);

                *node = ns.get_parent();
                ns = parent_ns;

                if done {
                    break;
                }
            }
        }
    }

    /// Inserts an element into the tree.
    ///
    /// For raw pointers, use [`insert_raw`] instead.
    pub fn insert(&mut self, ptr: P)
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this tree.
        unsafe { self.insert_raw(ptr) }
    }

    /// Inserts an element into the tree.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a `T` and that the object outlives
    /// the reference from the tree.
    pub unsafe fn insert_raw(&mut self, ptr: P) {
        let mut collision = core::ptr::null_mut();
        // SAFETY: The caller guarantees `ptr` is valid and outlives the tree registration.
        let _ = unsafe { self.internal_insert(ptr, &mut collision) };
    }

    /// Inserts the object pointed to by `ptr` if it is not already in the tree,
    /// or finds the object that `ptr` collided with instead.
    ///
    /// For raw pointers, use [`insert_or_find_raw`] instead.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if there was no collision and the item was successfully inserted.
    ///   In this case, the tree takes ownership of `ptr`.
    /// * `Err((ptr, cursor))` if there was a collision. In this case, the
    ///   passed pointer `ptr` is returned back to the caller (not consumed), along with
    ///   a `CursorMut` positioned at the colliding node already in the tree.
    pub fn insert_or_find<'a>(
        &'a mut self,
        ptr: P,
    ) -> Result<(), (P, CursorMut<'a, K, P, Tag, S, O>)>
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this tree.
        unsafe { self.insert_or_find_raw(ptr) }
    }

    /// Inserts the object pointed to by `ptr` if it is not already in the tree,
    /// or finds the object that `ptr` collided with instead.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a `T` and that the object outlives
    /// the reference from the tree.
    pub unsafe fn insert_or_find_raw<'a>(
        &'a mut self,
        ptr: P,
    ) -> Result<(), (P, CursorMut<'a, K, P, Tag, S, O>)> {
        let mut collision = core::ptr::null_mut();
        // SAFETY: The caller guarantees `ptr` is valid and outlives the tree registration.
        // If a collision occurs, `collision` is guaranteed to be a valid pointer to the colliding
        // node in this tree, which allows us to safely construct a `CursorMut` pointing to it.
        unsafe {
            match self.internal_insert(ptr, &mut collision) {
                Ok(()) => Ok(()),
                Err(ptr) => Err((ptr, CursorMut { tree: self, current: collision })),
            }
        }
    }

    /// Finds the element in the tree with the same key as `*ptr` and replaces
    /// it with `ptr`, returning the element which was replaced.
    ///
    /// If no element in the tree shares a key with `*ptr`, simply adds `ptr` to
    /// the tree and returns `None`.
    ///
    /// In both cases, the input pointer `ptr` is consumed.
    ///
    /// For raw pointers, use [`insert_or_replace_raw`] instead.
    ///
    /// # Returns
    ///
    /// `Some(replaced)` containing the previous element if a collision occurred,
    /// or `None` if the element was newly inserted.
    pub fn insert_or_replace(&mut self, ptr: P) -> Option<P>
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this tree.
        unsafe { self.insert_or_replace_raw(ptr) }
    }

    /// Finds the element in the tree with the same key as `*ptr` and replaces
    /// it with `ptr`, returning the element which was replaced.
    ///
    /// If no element in the tree shares a key with `*ptr`, simply adds `ptr` to
    /// the tree and returns `None`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a `T` and that the object outlives
    /// the reference from the tree.
    pub unsafe fn insert_or_replace_raw(&mut self, ptr: P) -> Option<P> {
        let mut collision = core::ptr::null_mut();
        // SAFETY: The caller guarantees `ptr` is valid and outlives the tree registration.
        // If a collision occurs, `collision` points to a valid node in the tree sharing the same key,
        // making it safe to swap `collision` with `ptr` via `internal_swap`.
        unsafe {
            match self.internal_insert(ptr, &mut collision) {
                Ok(()) => None,
                Err(ptr) => self.internal_swap(collision, ptr),
            }
        }
    }

    /// Removes and returns the first (smallest) element of the tree, or `None` if it is empty.
    pub fn pop_front(&mut self) -> Option<P> {
        if self.is_empty() {
            None
        } else {
            // SAFETY: If `self.is_empty()` is false, `self.left_most` is guaranteed to be a valid
            // pointer to a node currently contained in this tree instance.
            unsafe { self.internal_erase(self.left_most) }
        }
    }

    /// Removes and returns the last (largest) element of the tree, or `None` if it is empty.
    pub fn pop_back(&mut self) -> Option<P> {
        if self.is_empty() {
            None
        } else {
            // SAFETY: If `self.is_empty()` is false, `self.right_most` is guaranteed to be a valid
            // pointer to a node currently contained in this tree instance.
            unsafe { self.internal_erase(self.right_most) }
        }
    }

    /// Removes all elements from the tree.
    pub fn clear(&mut self) {
        while !self.is_empty() {
            self.pop_front();
        }
    }

    /// Swaps the contents of this tree with another tree.
    ///
    /// This runs in O(1) time.
    pub fn swap(&mut self, other: &mut Self) {
        // Swap all fields except _pin and _phantom.
        core::mem::swap(&mut self.root, &mut other.root);
        core::mem::swap(&mut self.left_most, &mut other.left_most);
        core::mem::swap(&mut self.right_most, &mut other.right_most);
        core::mem::swap(&mut self.size, &mut other.size);
        core::mem::swap(&mut self.observer, &mut other.observer);

        // Now repair the sentinel pointers which are self-referential.
        self.fix_sentinels_after_swap(other);
    }

    fn fix_sentinels_after_swap(&mut self, other: &mut Self) {
        let self_sentinel = self.get_sentinel();
        let other_sentinel = other.get_sentinel();

        // For `self` (which currently contains `other`'s old nodes):
        // The old sentinel in these nodes is `other_sentinel`. We update them to `self_sentinel`.
        if self.root.is_null() {
            self.left_most = self_sentinel;
            self.right_most = self_sentinel;
        } else {
            // SAFETY: Sentinels are verified to be valid and correspond to the correct node locations.
            unsafe {
                let root_ns = Self::get_node_ref(self.root);
                debug_assert_eq!(root_ns.get_parent(), other_sentinel);
                root_ns.set_parent(self_sentinel);

                let left_ns = Self::get_node_ref(self.left_most);
                debug_assert_eq!(left_ns.get_left(), other_sentinel);
                left_ns.set_left(self_sentinel);

                let right_ns = Self::get_node_ref(self.right_most);
                debug_assert_eq!(right_ns.get_right(), other_sentinel);
                right_ns.set_right(self_sentinel);
            }
        }

        // For `other` (which currently contains `self`'s old nodes):
        // The old sentinel in these nodes is `self_sentinel`. We update them to `other_sentinel`.
        if other.root.is_null() {
            other.left_most = other_sentinel;
            other.right_most = other_sentinel;
        } else {
            // SAFETY: Sentinels are verified to be valid and correspond to the correct node locations.
            unsafe {
                let root_ns = Self::get_node_ref(other.root);
                debug_assert_eq!(root_ns.get_parent(), self_sentinel);
                root_ns.set_parent(other_sentinel);

                let left_ns = Self::get_node_ref(other.left_most);
                debug_assert_eq!(left_ns.get_left(), self_sentinel);
                left_ns.set_left(other_sentinel);

                let right_ns = Self::get_node_ref(other.right_most);
                debug_assert_eq!(right_ns.get_right(), self_sentinel);
                right_ns.set_right(other_sentinel);
            }
        }
    }

    /// Traverses the tree to find the node with the given key.
    ///
    /// Returns a raw pointer to the matching node, or a sentinel pointer if not found.
    ///
    /// # Safety
    ///
    /// The returned raw pointer is only valid as long as the tree structure is not modified
    /// and no elements are deleted.
    unsafe fn find_raw(&self, key: &K) -> *mut P::Target {
        // SAFETY: Accessing node keys and traversing child pointers is safe since we only
        // traverse nodes contained inside this tree and ensure they are valid via `valid_sentinel_ptr`.
        unsafe {
            let mut node = self.root;
            while valid_sentinel_ptr(node) {
                let node_key = (*node).get_key();
                let b = node_key.borrow();
                if key == b {
                    return node;
                }
                let ns = Self::get_node_ref(node);
                node = if key < b { ns.get_left() } else { ns.get_right() };
            }
            self.get_sentinel()
        }
    }

    /// Traverses the tree to find either the lower bound or upper bound node pointer.
    ///
    /// # Safety
    ///
    /// The returned raw pointer is only valid as long as the tree structure is not modified
    /// and no elements are deleted.
    unsafe fn bound_raw(&self, key: &K, strictly_greater: bool) -> *mut P::Target {
        // SAFETY: Accessing node keys and traversing child pointers is safe since we only
        // traverse nodes contained inside this tree and ensure they are valid via `valid_sentinel_ptr`.
        unsafe {
            let mut node = self.root;
            let mut found = self.get_sentinel();

            while valid_sentinel_ptr(node) {
                let node_key = (*node).get_key();
                let b = node_key.borrow();
                let is_eligible = if strictly_greater { b > key } else { b >= key };
                if is_eligible {
                    found = node;
                    node = Self::get_node_ref(node).get_left();
                } else {
                    node = Self::get_node_ref(node).get_right();
                }
            }
            found
        }
    }

    /// Finds an element in the tree by key.
    pub fn find(&self, key: &K) -> Option<&P::Target> {
        // SAFETY: find_raw returns either a sentinel pointer or a valid node in the tree.
        // If it is valid, returning a reference is safe for the lifetime of the borrow of `self`.
        unsafe {
            let node = self.find_raw(key);
            if valid_sentinel_ptr(node) { Some(&*node) } else { None }
        }
    }

    /// Finds an element in the tree by key and returns a cursor positioned at it.
    ///
    /// If the key is not found, the returned cursor is positioned at the sentinel
    /// (i.e. `cursor.get()` will return `None`).
    pub fn find_cursor(&mut self, key: &K) -> CursorMut<'_, K, P, Tag, S, O> {
        // SAFETY: find_raw returns a valid node pointer or sentinel pointer belonging to this tree.
        let node = unsafe { self.find_raw(key) };
        CursorMut { tree: self, current: node }
    }

    /// Returns a cursor positioned at the lower bound of the key (the first element
    /// in the tree whose key is greater than or equal to `key`).
    ///
    /// If no such element exists (e.g. all elements in the tree are smaller than `key`),
    /// the returned cursor is positioned at the sentinel.
    pub fn lower_bound(&mut self, key: &K) -> CursorMut<'_, K, P, Tag, S, O> {
        // SAFETY: bound_raw returns a valid node pointer or sentinel pointer in the tree.
        let node = unsafe { self.bound_raw(key, false) };
        CursorMut { tree: self, current: node }
    }

    /// Returns a cursor positioned at the upper bound of the key (the first element
    /// in the tree whose key is strictly greater than `key`).
    ///
    /// If no such element exists (e.g. all elements in the tree are smaller than or
    /// equal to `key`), the returned cursor is positioned at the sentinel.
    pub fn upper_bound(&mut self, key: &K) -> CursorMut<'_, K, P, Tag, S, O> {
        // SAFETY: bound_raw returns a valid node pointer or sentinel pointer in the tree.
        let node = unsafe { self.bound_raw(key, true) };
        CursorMut { tree: self, current: node }
    }

    /// Erases an element by key.
    pub fn erase(&mut self, key: &K) -> Option<P> {
        let mut cursor = self.find_cursor(key);
        cursor.erase()
    }

    /// Erases an element by raw pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `obj` is a valid pointer to an object that is currently
    /// contained within this tree instance.
    pub unsafe fn erase_raw(&mut self, obj: *mut P::Target) -> Option<P> {
        // SAFETY: The caller guarantees that `obj` is currently contained in this WavlTree.
        // `internal_erase` is safe to execute on a contained pointer.
        unsafe {
            let node = (*obj).get_node();
            if !node.in_container() {
                return None;
            }
            self.internal_erase(obj)
        }
    }

    /// Returns a cursor positioned at the front (smallest element) of the tree.
    pub fn cursor_mut(&mut self) -> CursorMut<'_, K, P, Tag, S, O> {
        let left_most = self.left_most;
        CursorMut { tree: self, current: left_most }
    }

    /// Returns a read-only cursor positioned at the given element.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `obj` is a member of this tree.
    /// It is undefined behavior to use the returned cursor if `obj` is not in the tree,
    /// or if it is in a different tree.
    pub unsafe fn cursor_at(&self, obj: &P::Target) -> Cursor<'_, K, P, Tag, S, O> {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        Cursor { tree: self, current: obj as *const P::Target as *mut P::Target }
    }

    /// Returns a mutable cursor positioned at the given element.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `obj` is a member of this tree.
    /// It is undefined behavior to use the returned cursor if `obj` is not in the tree,
    /// or if it is in a different tree.
    pub unsafe fn cursor_mut_at(&mut self, obj: &P::Target) -> CursorMut<'_, K, P, Tag, S, O> {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        CursorMut { tree: self, current: obj as *const P::Target as *mut P::Target }
    }

    /// Returns an iterator over the elements of the tree.
    pub fn iter(&self) -> Iterator<'_, K, P, Tag, S, O> {
        Iterator::new(self)
    }

    /// Returns a unidirectional forward iterator over the elements of the tree.
    pub fn forward_iter(&self) -> ForwardIterator<'_, K, P, Tag, S, O> {
        ForwardIterator::new(self.left_most)
    }

    /// Returns a unidirectional reverse iterator over the elements of the tree.
    pub fn reverse_iter(&self) -> ReverseIterator<'_, K, P, Tag, S, O> {
        ReverseIterator::new(self.right_most)
    }

    /// Returns a read-only cursor positioned at the root of the tree.
    pub fn root_cursor(&self) -> Cursor<'_, K, P, Tag, S, O> {
        Cursor { tree: self, current: self.root }
    }

    /// Returns a read-only cursor positioned at the first (smallest) element.
    pub fn front_cursor(&self) -> Cursor<'_, K, P, Tag, S, O> {
        Cursor { tree: self, current: self.left_most }
    }

    /// Returns a read-only cursor positioned at the last (largest) element.
    pub fn back_cursor(&self) -> Cursor<'_, K, P, Tag, S, O> {
        Cursor { tree: self, current: self.right_most }
    }

    /// Returns the number of elements in the tree.
    pub fn len(&self) -> usize {
        self.size.get()
    }
}

#[pinned_drop]
impl<K, P, Tag, S, O> PinnedDrop for WavlTree<K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn drop(self: Pin<&mut Self>) {
        if P::IS_MANAGED {
            let me = unsafe { self.get_unchecked_mut() };
            me.clear();
        } else {
            debug_assert!(self.is_empty(), "Tree must be empty on destruction");
            if S::IS_TRACKING {
                debug_assert_eq!(self.size.get(), 0, "Size must be zero on destruction");
            }
        }
    }
}

/// A read-only cursor positioned in a `WavlTree`.
pub struct Cursor<
    'a,
    K,
    P,
    Tag = DefaultObjectTag,
    S = NonTrackingSize,
    O = DefaultWavlTreeObserver<<P as PtrTraits>::Target>,
> where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    tree: &'a WavlTree<K, P, Tag, S, O>,
    current: *mut P::Target,
}

impl<'a, K, P, Tag, S, O> Clone for Cursor<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, K, P, Tag, S, O> Copy for Cursor<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
}

impl<'a, K, P, Tag, S, O> PartialEq for Cursor<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn eq(&self, other: &Self) -> bool {
        self.current == other.current
    }
}

impl<'a, K, P, Tag, S, O> core::fmt::Debug for Cursor<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Cursor").field("current", &self.current).finish()
    }
}

impl<'a, K, P, Tag, S, O> Cursor<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    /// Returns a reference to the current element, or `None` if the cursor is at the sentinel.
    pub fn get(&self) -> Option<&'a P::Target> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is checked to be non-sentinel.
            // The lifetime `'a` is tied to the `WavlTree` borrow.
            unsafe { Some(&*self.current) }
        }
    }

    /// Returns true if the cursor is positioned at a valid element (not the sentinel).
    pub fn is_valid(&self) -> bool {
        valid_sentinel_ptr(self.current)
    }

    /// Returns a cursor positioned at the left child of the current element.
    /// If the current element is the sentinel, returns a cursor at the sentinel.
    pub fn left(&self) -> Self {
        if !self.is_valid() {
            *self
        } else {
            // SAFETY: `self.current` is checked to be non-sentinel (which implies it is a valid,
            // non-null node in the tree because `current` is never null). Accessing the tree node state is safe.
            let ns = unsafe { WavlTree::<K, P, Tag, S, O>::get_node_ref(self.current) };
            Self { tree: self.tree, current: ns.get_left() }
        }
    }

    /// Returns a cursor positioned at the right child of the current element.
    pub fn right(&self) -> Self {
        if !self.is_valid() {
            *self
        } else {
            // SAFETY: `self.current` is checked to be non-sentinel (which implies it is a valid,
            // non-null node in the tree because `current` is never null). Accessing the tree node state is safe.
            let ns = unsafe { WavlTree::<K, P, Tag, S, O>::get_node_ref(self.current) };
            Self { tree: self.tree, current: ns.get_right() }
        }
    }

    /// Returns a cursor positioned at the parent of the current element.
    pub fn parent(&self) -> Self {
        if !self.is_valid() {
            *self
        } else {
            // SAFETY: `self.current` is checked to be non-sentinel (which implies it is a valid,
            // non-null node in the tree because `current` is never null). Accessing the tree node state is safe.
            let ns = unsafe { WavlTree::<K, P, Tag, S, O>::get_node_ref(self.current) };
            let parent = ns.get_parent();
            Self { tree: self.tree, current: parent }
        }
    }

    /// Returns the raw pointer to the current element.
    /// This may be a sentinel pointer if the cursor is at the sentinel.
    pub fn as_raw_ptr(&self) -> *mut P::Target {
        self.current
    }
}

/// A cursor over elements in a `WavlTree`.
pub struct CursorMut<
    'a,
    K,
    P,
    Tag = DefaultObjectTag,
    S = NonTrackingSize,
    O = DefaultWavlTreeObserver<<P as PtrTraits>::Target>,
> where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    tree: &'a mut WavlTree<K, P, Tag, S, O>,
    current: *mut P::Target,
}

impl<'a, K, P, Tag, S, O> CursorMut<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    /// Returns a reference to the current element.
    pub fn get(&self) -> Option<&P::Target> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is checked to be non-sentinel (which implies it is a valid,
            // non-null node in the tree because `current` is never null). Since `CursorMut` mutably
            // borrows the tree, the node is guaranteed to be valid and dereferenceable for the
            // lifetime of the reference.
            unsafe { Some(&*self.current) }
        }
    }

    /// Moves the cursor to the next (larger) element.
    pub fn move_next(&mut self) {
        if valid_sentinel_ptr(self.current) {
            // SAFETY: `self.current` is verified to be a valid node in the tree. Advancing through
            // tree pointers is safe.
            unsafe {
                WavlTree::<K, P, Tag, S, O>::advance::<ForwardTraits>(&mut self.current);
            }
        }
    }

    /// Moves the cursor to the previous (smaller) element.
    pub fn move_prev(&mut self) {
        if valid_sentinel_ptr(self.current) {
            // SAFETY: `self.current` is verified to be a valid node in the tree. Advancing through
            // tree pointers is safe.
            unsafe {
                WavlTree::<K, P, Tag, S, O>::advance::<ReverseTraits>(&mut self.current);
            }
        } else if is_sentinel_ptr(self.current) {
            self.current = self.tree.right_most;
        }
    }

    /// Erases the current element and moves the cursor to the next element.
    pub fn erase(&mut self) -> Option<P> {
        if !valid_sentinel_ptr(self.current) {
            return None;
        }

        let to_erase = self.current;
        // SAFETY: `to_erase` is verified to be a valid, non-sentinel node in the tree.
        // `advance` moves the cursor to a safe position before `internal_erase` physically removes
        // `to_erase` from the tree.
        unsafe {
            WavlTree::<K, P, Tag, S, O>::advance::<ForwardTraits>(&mut self.current);
            self.tree.internal_erase(to_erase)
        }
    }
}

/// A unidirectional forward iterator over the elements of a `WavlTree`.
pub struct ForwardIterator<
    'a,
    K,
    P,
    Tag = DefaultObjectTag,
    S = NonTrackingSize,
    O = DefaultWavlTreeObserver<<P as PtrTraits>::Target>,
> where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    current: *mut P::Target,
    _phantom: core::marker::PhantomData<&'a WavlTree<K, P, Tag, S, O>>,
}

impl<'a, K, P, Tag, S, O> ForwardIterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn new(current: *mut P::Target) -> Self {
        Self { current, _phantom: core::marker::PhantomData }
    }

    /// Creates an iterator starting from a specific element.
    ///
    /// # Panics
    ///
    /// Panics if the object is not in a container.
    pub fn from_element(obj: &'a P::Target) -> Self {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        Self { current: obj as *const _ as *mut _, _phantom: core::marker::PhantomData }
    }

    fn get_current(&self) -> Option<&'a P::Target> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is checked to be non-sentinel (which implies it is a valid,
            // non-null node in the tree because `current` is never null). Since the iterator
            // holds a lifetime borrow of the `WavlTree`, and the tree remains unmodified,
            // the node is guaranteed to be valid and dereferenceable for `'a`.
            unsafe { Some(&*self.current) }
        }
    }
}

impl<'a, K, P, Tag, S, O> Clone for ForwardIterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn clone(&self) -> Self {
        Self { current: self.current, _phantom: core::marker::PhantomData }
    }
}

impl<'a, K, P, Tag, S, O> core::iter::Iterator for ForwardIterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    type Item = &'a P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.get_current()?;
        // SAFETY: `self.current` is validated as non-sentinel by `get_current`.
        // Moving through tree pointers is safe.
        unsafe {
            WavlTree::<K, P, Tag, S, O>::advance::<ForwardTraits>(&mut self.current);
        }
        Some(current)
    }
}

/// A unidirectional reverse iterator over the elements of a `WavlTree`.
pub struct ReverseIterator<
    'a,
    K,
    P,
    Tag = DefaultObjectTag,
    S = NonTrackingSize,
    O = DefaultWavlTreeObserver<<P as PtrTraits>::Target>,
> where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    current: *mut P::Target,
    _phantom: core::marker::PhantomData<&'a WavlTree<K, P, Tag, S, O>>,
}

impl<'a, K, P, Tag, S, O> ReverseIterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn new(current: *mut P::Target) -> Self {
        Self { current, _phantom: core::marker::PhantomData }
    }

    /// Creates an iterator starting from a specific element.
    ///
    /// # Panics
    ///
    /// Panics if the object is not in a container.
    pub fn from_element(obj: &'a P::Target) -> Self {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        Self { current: obj as *const _ as *mut _, _phantom: core::marker::PhantomData }
    }

    fn get_current(&self) -> Option<&'a P::Target> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is checked to be non-sentinel (which implies it is a valid,
            // non-null node in the tree because `current` is never null). Since the iterator
            // holds a lifetime borrow of the `WavlTree`, and the tree remains unmodified,
            // the node is guaranteed to be valid and dereferenceable for `'a`.
            unsafe { Some(&*self.current) }
        }
    }
}

impl<'a, K, P, Tag, S, O> Clone for ReverseIterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn clone(&self) -> Self {
        Self { current: self.current, _phantom: core::marker::PhantomData }
    }
}

impl<'a, K, P, Tag, S, O> core::iter::Iterator for ReverseIterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    type Item = &'a P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.get_current()?;
        // SAFETY: `self.current` is validated as non-sentinel by `get_current`.
        // Moving through tree pointers is safe.
        unsafe {
            WavlTree::<K, P, Tag, S, O>::advance::<ReverseTraits>(&mut self.current);
        }
        Some(current)
    }
}

/// An iterator over the elements of a `WavlTree`.
pub struct Iterator<
    'a,
    K,
    P,
    Tag = DefaultObjectTag,
    S = NonTrackingSize,
    O = DefaultWavlTreeObserver<<P as PtrTraits>::Target>,
> where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    front: ForwardIterator<'a, K, P, Tag, S, O>,
    back: ReverseIterator<'a, K, P, Tag, S, O>,
}

impl<'a, K, P, Tag, S, O> Iterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn new(tree: &'a WavlTree<K, P, Tag, S, O>) -> Self {
        if tree.is_empty() {
            Self {
                front: ForwardIterator::new(make_sentinel_null()),
                back: ReverseIterator::new(make_sentinel_null()),
            }
        } else {
            Self {
                front: ForwardIterator::new(tree.left_most),
                back: ReverseIterator::new(tree.right_most),
            }
        }
    }
}

impl<'a, K, P, Tag, S, O> Clone for Iterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn clone(&self) -> Self {
        Self { front: self.front.clone(), back: self.back.clone() }
    }
}

impl<'a, K, P, Tag, S, O> core::iter::Iterator for Iterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    type Item = &'a P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        let met = self.front.current == self.back.current;
        let item = self.front.next();
        if item.is_some() {
            if met {
                self.front.current = make_sentinel_null();
                self.back.current = make_sentinel_null();
            }
        }
        item
    }
}

impl<'a, K, P, Tag, S, O> core::iter::DoubleEndedIterator for Iterator<'a, K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        let met = self.front.current == self.back.current;
        let item = self.back.next();
        if item.is_some() {
            if met {
                self.front.current = make_sentinel_null();
                self.back.current = make_sentinel_null();
            }
        }
        item
    }
}

impl<K, T, Tag, S, O> WavlTree<K, *mut T, Tag, S, O>
where
    T: WavlTreeContainable<T, Tag> + WavlTreeKeyable<K>,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = T>,
{
    /// Unsafely removes all elements from the tree without modifying node memory.
    ///
    /// This method resets the tree's internal pointers, effectively emptying it, but does
    /// NOT modify the node state of the elements that were in the tree.
    ///
    /// # Safety
    ///
    /// Because the nodes are not modified, they will still believe they are in a container
    /// (i.e. `in_container()` will return `true` for them). If these elements are subsequently
    /// dropped, they will trigger a `debug_assert` panic (as `WavlTreeNode` asserts on drop
    /// that it is not in a container).
    ///
    /// The caller is responsible for manually clearing the node state of the elements, or
    /// ensuring they are never dropped while in this "dirty" state.
    ///
    /// Only usable with containers of unmanaged pointers. Think carefully before calling this!
    pub fn clear_unsafe(&mut self) {
        self.root = core::ptr::null_mut();
        self.left_most = self.get_sentinel();
        self.right_most = self.get_sentinel();
        self.size.set(0);
    }
}

impl<K, P, Tag, S, O> core::fmt::Debug for WavlTree<K, P, Tag, S, O>
where
    P: PtrTraits,
    P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K> + core::fmt::Debug,
    K: Ord,
    S: SizeTracker,
    O: WavlTreeObserver<Target = P::Target>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intrusive_container_test_support::*;
    use crate::ref_counted::HasRefCount;
    use crate::ref_ptr::RefPtr;
    use crate::size_tracker::TrackingSize;
    use crate::unique_ptr::UniquePtr;
    use crate::{Recyclable, WavlTreeContainable};
    use core::ffi::c_void;
    use pin_init::stack_pin_init;

    trait AsTargetRef {
        type Target;
        unsafe fn as_target_ref(&self) -> &Self::Target;
    }

    impl<T> AsTargetRef for *mut T {
        type Target = T;
        unsafe fn as_target_ref(&self) -> &T {
            unsafe { &**self }
        }
    }

    impl<T: Recyclable> AsTargetRef for UniquePtr<T> {
        type Target = T;
        unsafe fn as_target_ref(&self) -> &T {
            &**self
        }
    }

    impl<T: HasRefCount + Recyclable> AsTargetRef for RefPtr<T> {
        type Target = T;
        unsafe fn as_target_ref(&self) -> &T {
            &**self
        }
    }

    #[derive(crate::WavlTreeContainable, crate::Recyclable)]
    struct TestObject {
        value: i32,
        #[wavl_node]
        node: WavlTreeNode<TestObject>,
    }

    impl TestObject {
        fn new(value: i32) -> Self {
            Self { value, node: WavlTreeNode::new() }
        }
    }

    impl WavlTreeKeyable<i32> for TestObject {
        type Key<'a> = i32;
        fn get_key(&self) -> i32 {
            self.value
        }
    }

    impl TestValue for TestObject {
        fn new(value: i32) -> Self {
            Self::new(value)
        }
    }

    ::zr::static_assert!(
        core::mem::size_of::<WavlTree<i32, *mut TestObject>>()
            == 3 * core::mem::size_of::<*mut TestObject>()
    );
    ::zr::static_assert!(
        core::mem::align_of::<WavlTree<i32, *mut TestObject>>()
            == core::mem::align_of::<*mut TestObject>()
    );

    ::zr::static_assert!(
        core::mem::size_of::<WavlTree<i32, *mut TestObject, DefaultObjectTag, TrackingSize>>()
            == 4 * core::mem::size_of::<*mut TestObject>()
    );
    ::zr::static_assert!(
        core::mem::align_of::<WavlTree<i32, *mut TestObject, DefaultObjectTag, TrackingSize>>()
            == core::mem::align_of::<*mut TestObject>()
    );

    #[derive(crate::WavlTreeContainable, crate::Recyclable)]
    struct UniqueTestObject {
        value: i32,
        #[wavl_node]
        node: WavlTreeNode<UniqueTestObject>,
    }

    impl UniqueTestObject {
        fn new(value: i32) -> Self {
            Self { value, node: WavlTreeNode::new() }
        }
    }

    impl WavlTreeKeyable<i32> for UniqueTestObject {
        type Key<'a> = i32;
        fn get_key(&self) -> i32 {
            self.value
        }
    }

    impl TestValue for UniqueTestObject {
        fn new(value: i32) -> Self {
            Self::new(value)
        }
    }

    #[fbl::ref_counted]
    #[derive(crate::WavlTreeContainable, crate::Recyclable)]
    #[repr(C)]
    pub struct RefTestObject {
        value: i32,
        #[wavl_node]
        node: WavlTreeNode<RefTestObject>,
    }

    impl WavlTreeKeyable<i32> for RefTestObject {
        type Key<'a> = i32;
        fn get_key(&self) -> i32 {
            self.value
        }
    }

    impl TestValue for RefTestObject {
        fn new_ref_counted(value: i32) -> RefPtr<Self> {
            crate::make_ref_counted!(RefTestObject { value: value, node: WavlTreeNode::new() })
                .unwrap()
        }
    }

    macro_rules! generate_tree_tests {
        ($mod_name:ident, $ptr_type:ty, $factory_type:ty, $get_val:expr, $insert:expr, $insert_or_find:expr, $insert_or_replace:expr) => {
            mod $mod_name {
                use super::*;

                #[test]
                fn test_basic_sorting() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    assert!(tree.is_empty());

                    // Insert in scrambled order
                    $insert(tree, factory.create(3));
                    $insert(tree, factory.create(1));
                    $insert(tree, factory.create(4));
                    $insert(tree, factory.create(2));

                    assert!(!tree.is_empty());

                    // Iteration should be sorted
                    let mut iter = tree.iter();
                    assert_eq!($get_val(iter.next().unwrap()), 1);
                    assert_eq!($get_val(iter.next().unwrap()), 2);
                    assert_eq!($get_val(iter.next().unwrap()), 3);
                    assert_eq!($get_val(iter.next().unwrap()), 4);
                    assert!(iter.next().is_none());

                    tree.clear();
                    assert!(tree.is_empty());
                }

                #[test]
                fn test_double_ended_iterator() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(30));
                    $insert(tree, factory.create(10));
                    $insert(tree, factory.create(20));

                    let mut iter = tree.iter();
                    assert_eq!($get_val(iter.next().unwrap()), 10);
                    assert_eq!($get_val(iter.next_back().unwrap()), 30);
                    assert_eq!($get_val(iter.next().unwrap()), 20);
                    assert!(iter.next().is_none());
                    assert!(iter.next_back().is_none());

                    tree.clear();
                }

                #[test]
                fn test_find() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(3));
                    $insert(tree, factory.create(1));
                    $insert(tree, factory.create(2));

                    assert!(tree.find(&2).is_some());
                    assert_eq!($get_val(tree.find(&2).unwrap()), 2);
                    assert!(tree.find(&4).is_none());

                    tree.clear();
                }

                #[test]
                fn test_bounds() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(10));
                    $insert(tree, factory.create(30));
                    $insert(tree, factory.create(20));

                    // lower_bound(>=)
                    assert_eq!($get_val(tree.lower_bound(&15).get().unwrap()), 20);
                    assert_eq!($get_val(tree.lower_bound(&20).get().unwrap()), 20);
                    assert!(tree.lower_bound(&35).get().is_none());

                    // upper_bound(>)
                    assert_eq!($get_val(tree.upper_bound(&15).get().unwrap()), 20);
                    assert_eq!($get_val(tree.upper_bound(&20).get().unwrap()), 30);
                    assert!(tree.upper_bound(&30).get().is_none());

                    tree.clear();
                }

                #[test]
                fn test_pops() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(10));
                    $insert(tree, factory.create(30));
                    $insert(tree, factory.create(20));

                    let popped = tree.pop_front();
                    assert!(popped.is_some());
                    let val = popped.unwrap();
                    assert_eq!($get_val(unsafe { val.as_target_ref() }), 10);

                    let popped = tree.pop_back();
                    assert!(popped.is_some());
                    let val = popped.unwrap();
                    assert_eq!($get_val(unsafe { val.as_target_ref() }), 30);

                    let popped = tree.pop_front();
                    assert!(popped.is_some());
                    let val = popped.unwrap();
                    assert_eq!($get_val(unsafe { val.as_target_ref() }), 20);

                    assert!(tree.pop_front().is_none());
                }

                #[test]
                fn test_erase_cursor() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(10));
                    $insert(tree, factory.create(30));
                    $insert(tree, factory.create(20));

                    let mut cursor = tree.find_cursor(&20);
                    let erased = cursor.erase();
                    assert!(erased.is_some());
                    let val = erased.unwrap();
                    assert_eq!($get_val(unsafe { val.as_target_ref() }), 20);

                    // Cursor should advance to next element (30)
                    assert_eq!($get_val(cursor.get().unwrap()), 30);

                    let mut iter = tree.iter();
                    assert_eq!($get_val(iter.next().unwrap()), 10);
                    assert_eq!($get_val(iter.next().unwrap()), 30);
                    assert!(iter.next().is_none());

                    tree.clear();
                }

                #[test]
                fn test_insert_or_find() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(10));

                    let new_item = factory.create(10); // Duplicate key
                    let res = $insert_or_find(tree, new_item);
                    assert!(res.is_err());
                    let (failed_ptr, collision) = res.err().unwrap();
                    assert_eq!($get_val(unsafe { failed_ptr.as_target_ref() }), 10);
                    assert_eq!($get_val(collision.get().unwrap()), 10);

                    let ok_item = factory.create(20);
                    assert!($insert_or_find(tree, ok_item).is_ok());

                    tree.clear();
                }

                #[test]
                fn test_insert_or_replace() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(10));

                    let replacement = factory.create(10);
                    let res = $insert_or_replace(tree, replacement);
                    assert!(res.is_some());
                    let old_item = res.unwrap();
                    assert_eq!($get_val(unsafe { old_item.as_target_ref() }), 10);

                    let found = tree.find(&10);
                    assert!(found.is_some());
                    assert_eq!($get_val(found.unwrap()), 10);

                    tree.clear();
                }

                #[test]
                fn test_from_element() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };

                    $insert(tree, factory.create(10));
                    $insert(tree, factory.create(20));
                    $insert(tree, factory.create(30));

                    let target_ref = tree.find(&20).unwrap();

                    let mut forward_iter: ForwardIterator<'_, i32, $ptr_type> = ForwardIterator::from_element(target_ref);
                    assert_eq!($get_val(forward_iter.next().unwrap()), 20);
                    assert_eq!($get_val(forward_iter.next().unwrap()), 30);
                    assert!(forward_iter.next().is_none());

                    let mut reverse_iter: ReverseIterator<'_, i32, $ptr_type> = ReverseIterator::from_element(target_ref);
                    assert_eq!($get_val(reverse_iter.next().unwrap()), 20);
                    assert_eq!($get_val(reverse_iter.next().unwrap()), 10);
                    assert!(reverse_iter.next().is_none());

                    tree.clear();
                }

                #[test]
                fn test_cursor_at() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };

                    $insert(tree, factory.create(10));
                    $insert(tree, factory.create(20));
                    $insert(tree, factory.create(30));

                    let target_ptr = tree.find(&20).unwrap() as *const <$ptr_type as PtrTraits>::Target;
                    // SAFETY: target_ptr is a valid pointer to an object currently in the tree.
                    // Using a raw pointer bypasses the borrow checker, allowing us to obtain an unbound reference
                    // and borrow the tree mutably afterward.
                    let target_ref = unsafe { &*target_ptr };

                    // Test read-only cursor_at
                    let cursor = unsafe { tree.cursor_at(target_ref) };
                    assert!(cursor.is_valid());
                    assert_eq!($get_val(cursor.get().unwrap()), 20);
                    assert_eq!($get_val(cursor.left().get().unwrap()), 10);
                    assert_eq!($get_val(cursor.right().get().unwrap()), 30);

                    // Test mutable cursor_mut_at
                    let mut cursor_mut = unsafe { tree.cursor_mut_at(target_ref) };
                    assert_eq!($get_val(cursor_mut.get().unwrap()), 20);

                    // Verify we can erase using the cursor returned by cursor_mut_at
                    let erased = cursor_mut.erase();
                    assert!(erased.is_some());
                    let val = erased.unwrap();
                    assert_eq!($get_val(unsafe { val.as_target_ref() }), 20);

                    // Cursor should now be at the next element (30)
                    assert_eq!($get_val(cursor_mut.get().unwrap()), 30);

                    tree.clear();
                }

                #[test]
                fn test_iterator_clone() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree = WavlTree::<i32, $ptr_type>::new());
                    let tree = unsafe { tree.get_unchecked_mut() };
                    $insert(tree, factory.create(10));
                    $insert(tree, factory.create(20));
                    $insert(tree, factory.create(30));

                    let mut iter = tree.iter();
                    assert_eq!($get_val(iter.next().unwrap()), 10);

                    let mut cloned_iter = iter.clone();

                    assert_eq!($get_val(iter.next().unwrap()), 20);
                    assert_eq!($get_val(iter.next().unwrap()), 30);
                    assert!(iter.next().is_none());

                    assert_eq!($get_val(cloned_iter.next().unwrap()), 20);
                    assert_eq!($get_val(cloned_iter.next().unwrap()), 30);
                    assert!(cloned_iter.next().is_none());

                    tree.clear();
                }

                #[test]
                fn test_swap() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let tree1 = WavlTree::<i32, $ptr_type>::new());
                    let tree1 = unsafe { tree1.get_unchecked_mut() };
                    stack_pin_init!(let tree2 = WavlTree::<i32, $ptr_type>::new());
                    let tree2 = unsafe { tree2.get_unchecked_mut() };

                    $insert(tree1, factory.create(1));
                    $insert(tree1, factory.create(3));

                    $insert(tree2, factory.create(2));
                    $insert(tree2, factory.create(4));

                    tree1.swap(tree2);

                    let mut iter1 = tree1.iter();
                    assert_eq!($get_val(iter1.next().unwrap()), 2);
                    assert_eq!($get_val(iter1.next().unwrap()), 4);
                    assert!(iter1.next().is_none());

                    let mut iter2 = tree2.iter();
                    assert_eq!($get_val(iter2.next().unwrap()), 1);
                    assert_eq!($get_val(iter2.next().unwrap()), 3);
                    assert!(iter2.next().is_none());

                    tree1.clear();
                    tree2.clear();
                }
            }
        };
    }

    generate_tree_tests!(
        raw_ptr_tests,
        *mut TestObject,
        RawFactory<TestObject>,
        |p: &TestObject| p.value,
        |tree, obj| unsafe { WavlTree::<i32, *mut TestObject>::insert_raw(tree, obj) },
        |tree, obj| unsafe { WavlTree::<i32, *mut TestObject>::insert_or_find_raw(tree, obj) },
        |tree, obj| unsafe { WavlTree::<i32, *mut TestObject>::insert_or_replace_raw(tree, obj) }
    );

    generate_tree_tests!(
        unique_ptr_tests,
        UniquePtr<UniqueTestObject>,
        UniqueFactory<UniqueTestObject>,
        |p: &UniqueTestObject| p.value,
        |tree, obj| WavlTree::<i32, UniquePtr<UniqueTestObject>>::insert(tree, obj),
        |tree, obj| WavlTree::<i32, UniquePtr<UniqueTestObject>>::insert_or_find(tree, obj),
        |tree, obj| WavlTree::<i32, UniquePtr<UniqueTestObject>>::insert_or_replace(tree, obj)
    );

    generate_tree_tests!(
        ref_ptr_tests,
        RefPtr<RefTestObject>,
        RefFactory<RefTestObject>,
        |p: &RefTestObject| p.value,
        |tree, obj| WavlTree::<i32, RefPtr<RefTestObject>>::insert(tree, obj),
        |tree, obj| WavlTree::<i32, RefPtr<RefTestObject>>::insert_or_find(tree, obj),
        |tree, obj| WavlTree::<i32, RefPtr<RefTestObject>>::insert_or_replace(tree, obj)
    );

    #[test]
    fn test_erase_by_raw_pointer() {
        stack_pin_init!(let tree = WavlTree::<i32, *mut TestObject, DefaultObjectTag, TrackingSize>::new());
        let tree = unsafe { tree.get_unchecked_mut() };
        let mut obj1 = TestObject::new(10);
        let mut obj2 = TestObject::new(20);
        let mut obj3 = TestObject::new(30);

        unsafe {
            tree.insert_raw(core::ptr::addr_of_mut!(obj1));
            tree.insert_raw(core::ptr::addr_of_mut!(obj2));
            tree.insert_raw(core::ptr::addr_of_mut!(obj3));
        }

        assert_eq!(tree.len(), 3);

        // Erase obj2 directly
        let erased = unsafe { tree.erase_raw(core::ptr::addr_of_mut!(obj2)) };
        assert!(erased.is_some());
        assert_eq!(unsafe { &*erased.unwrap() }.value, 20);
        assert_eq!(tree.len(), 2);

        let mut iter = tree.iter();
        assert_eq!(iter.next().unwrap().value, 10);
        assert_eq!(iter.next().unwrap().value, 30);
        assert!(iter.next().is_none());

        tree.clear();
    }

    #[test]
    fn test_clear_unsafe() {
        stack_pin_init!(let tree = WavlTree::<i32, *mut TestObject, DefaultObjectTag, TrackingSize>::new());
        let tree = unsafe { tree.get_unchecked_mut() };
        let mut obj1 = TestObject::new(10);
        let mut obj2 = TestObject::new(20);
        let mut obj3 = TestObject::new(30);

        unsafe {
            tree.insert_raw(core::ptr::addr_of_mut!(obj1));
            tree.insert_raw(core::ptr::addr_of_mut!(obj2));
            tree.insert_raw(core::ptr::addr_of_mut!(obj3));
        }

        assert_eq!(tree.len(), 3);
        assert!(!tree.is_empty());

        tree.clear_unsafe();

        assert_eq!(tree.len(), 0);
        assert!(tree.is_empty());

        // Clean up the nodes manually so that they can be safely dropped.
        unsafe {
            (*obj1.get_node().parent.get()) = core::ptr::null_mut();
            (*obj1.get_node().left.get()) = core::ptr::null_mut();
            (*obj1.get_node().right.get()) = core::ptr::null_mut();

            (*obj2.get_node().parent.get()) = core::ptr::null_mut();
            (*obj2.get_node().left.get()) = core::ptr::null_mut();
            (*obj2.get_node().right.get()) = core::ptr::null_mut();

            (*obj3.get_node().parent.get()) = core::ptr::null_mut();
            (*obj3.get_node().left.get()) = core::ptr::null_mut();
            (*obj3.get_node().right.get()) = core::ptr::null_mut();
        }
    }

    #[test]
    fn test_tracking_size() {
        stack_pin_init!(let tree = WavlTree::<i32, UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let tree = unsafe { tree.get_unchecked_mut() };

        assert_eq!(tree.len(), 0);
        tree.insert(UniquePtr::try_new(UniqueTestObject::new(10)).unwrap());
        assert_eq!(tree.len(), 1);
        tree.insert(UniquePtr::try_new(UniqueTestObject::new(20)).unwrap());
        assert_eq!(tree.len(), 2);
        tree.pop_front();
        assert_eq!(tree.len(), 1);
        tree.clear();
        assert_eq!(tree.len(), 0);
    }

    struct Tag2;

    #[fbl::ref_counted]
    #[derive(crate::WavlTreeContainable, crate::Recyclable)]
    #[repr(C)]
    struct MultiTreeObject {
        value: i32,
        #[wavl_node]
        node1: WavlTreeNode<MultiTreeObject>,
        #[wavl_node(tag = Tag2)]
        node2: WavlTreeNode<MultiTreeObject>,
    }

    impl WavlTreeKeyable<i32> for MultiTreeObject {
        type Key<'a> = i32;
        fn get_key(&self) -> i32 {
            self.value
        }
    }

    #[test]
    fn test_multiple_containers() {
        stack_pin_init!(let tree1 = WavlTree::<i32, RefPtr<MultiTreeObject>, DefaultObjectTag>::new());
        let tree1 = unsafe { tree1.get_unchecked_mut() };
        stack_pin_init!(let tree2 = WavlTree::<i32, RefPtr<MultiTreeObject>, Tag2>::new());
        let tree2 = unsafe { tree2.get_unchecked_mut() };

        let obj1 = fbl::make_ref_counted!(MultiTreeObject {
            value: 10,
            node1: WavlTreeNode::new(),
            node2: WavlTreeNode::new(),
        })
        .unwrap();

        let obj2 = fbl::make_ref_counted!(MultiTreeObject {
            value: 20,
            node1: WavlTreeNode::new(),
            node2: WavlTreeNode::new(),
        })
        .unwrap();

        tree1.insert(obj1.clone());
        tree1.insert(obj2.clone());

        tree2.insert(obj1.clone());
        tree2.insert(obj2.clone());

        let mut iter1 = tree1.iter();
        assert_eq!(iter1.next().unwrap().value, 10);
        assert_eq!(iter1.next().unwrap().value, 20);

        let mut iter2 = tree2.iter();
        assert_eq!(iter2.next().unwrap().value, 10);
        assert_eq!(iter2.next().unwrap().value, 20);

        tree1.clear();
        tree2.clear();
    }

    extern crate alloc;
    use alloc::boxed::Box;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct Lfsr {
        core: u64,
    }

    impl Lfsr {
        fn new(initial_core: u64) -> Self {
            Self { core: initial_core }
        }

        fn set_core(&mut self, val: u64) {
            self.core = val;
        }

        fn get_next(&mut self) -> u64 {
            let mut ret = 0u64;
            let mut flag = 1u64;
            let generator = 0xD800000000000000u64;

            for _ in 0..(core::mem::size_of::<usize>() * 8) {
                let bit = (self.core & 1) != 0;
                self.core >>= 1;
                if bit {
                    self.core ^= generator;
                    ret |= flag;
                }
                flag <<= 1;
            }

            ret
        }
    }

    struct OpCounts {
        insert_ops: AtomicUsize,
        insert_promotes: AtomicUsize,
        insert_rotations: AtomicUsize,
        insert_double_rotations: AtomicUsize,
        insert_collisions: AtomicUsize,
        insert_replacements: AtomicUsize,
        insert_traversals: AtomicUsize,
        inspected_rotations: AtomicUsize,
        erase_ops: AtomicUsize,
        erase_demotes: AtomicUsize,
        erase_rotations: AtomicUsize,
        erase_double_rotations: AtomicUsize,
    }

    impl OpCounts {
        const fn new() -> Self {
            Self {
                insert_ops: AtomicUsize::new(0),
                insert_promotes: AtomicUsize::new(0),
                insert_rotations: AtomicUsize::new(0),
                insert_double_rotations: AtomicUsize::new(0),
                insert_collisions: AtomicUsize::new(0),
                insert_replacements: AtomicUsize::new(0),
                insert_traversals: AtomicUsize::new(0),
                inspected_rotations: AtomicUsize::new(0),
                erase_ops: AtomicUsize::new(0),
                erase_demotes: AtomicUsize::new(0),
                erase_rotations: AtomicUsize::new(0),
                erase_double_rotations: AtomicUsize::new(0),
            }
        }
    }

    #[derive(crate::WavlTreeContainable)]
    #[repr(C)]
    struct BalanceTestObj {
        key: u64,
        min_subtree_key: u64,
        max_subtree_key: u64,
        erase_deck_ptr: core::cell::Cell<*mut BalanceTestObj>,
        #[wavl_node(rank = i32)]
        node: WavlTreeNode<BalanceTestObj, i32>,
    }

    impl BalanceTestObj {
        fn new(key: u64) -> Self {
            Self {
                key,
                min_subtree_key: 0,
                max_subtree_key: 0,
                erase_deck_ptr: core::cell::Cell::new(core::ptr::null_mut()),
                node: WavlTreeNode::new(),
            }
        }

        fn swap_erase_deck_ptr(a: &BalanceTestObj, b: &BalanceTestObj) {
            let tmp = a.erase_deck_ptr.get();
            a.erase_deck_ptr.set(b.erase_deck_ptr.get());
            b.erase_deck_ptr.set(tmp);
        }
    }

    impl WavlTreeKeyable<u64> for BalanceTestObj {
        type Key<'a> = u64;
        fn get_key(&self) -> u64 {
            self.key
        }
    }

    struct WavlBalanceTestObserver {
        op_counts: Arc<OpCounts>,
    }
    impl WavlTreeObserver for WavlBalanceTestObserver {
        type Target = BalanceTestObj;

        fn record_insert(&self, node: *mut BalanceTestObj) {
            self.op_counts.insert_ops.fetch_add(1, Ordering::Relaxed);
            unsafe {
                (*node).min_subtree_key = (*node).key;
                (*node).max_subtree_key = (*node).key;
            }
        }

        fn record_insert_traverse(&self, node: *mut BalanceTestObj, ancestor: *mut BalanceTestObj) {
            self.op_counts.insert_traversals.fetch_add(1, Ordering::Relaxed);
            unsafe {
                (*ancestor).min_subtree_key =
                    core::cmp::min((*ancestor).min_subtree_key, (*node).key);
                (*ancestor).max_subtree_key =
                    core::cmp::max((*ancestor).max_subtree_key, (*node).key);
            }
        }

        fn record_insert_collision(
            &self,
            _node: *mut BalanceTestObj,
            _collision: *mut BalanceTestObj,
        ) {
            self.op_counts.insert_collisions.fetch_add(1, Ordering::Relaxed);
        }

        fn record_insert_replace(
            &self,
            node: *mut BalanceTestObj,
            replacement: *mut BalanceTestObj,
        ) {
            self.op_counts.insert_replacements.fetch_add(1, Ordering::Relaxed);
            unsafe {
                (*replacement).min_subtree_key = (*node).min_subtree_key;
                (*replacement).max_subtree_key = (*node).max_subtree_key;
            }
        }

        fn record_insert_promote(&self) {
            self.op_counts.insert_promotes.fetch_add(1, Ordering::Relaxed);
        }

        fn record_insert_rotation(&self) {
            self.op_counts.insert_rotations.fetch_add(1, Ordering::Relaxed);
        }

        fn record_insert_double_rotation(&self) {
            self.op_counts.insert_double_rotations.fetch_add(1, Ordering::Relaxed);
        }

        fn record_rotation(
            &self,
            pivot: *mut BalanceTestObj,
            lr_child: *mut BalanceTestObj,
            _rl_child: *mut BalanceTestObj,
            parent: *mut BalanceTestObj,
            sibling: *mut BalanceTestObj,
        ) {
            self.op_counts.inspected_rotations.fetch_add(1, Ordering::Relaxed);
            unsafe {
                (*pivot).min_subtree_key = (*parent).min_subtree_key;
                (*pivot).max_subtree_key = (*parent).max_subtree_key;

                (*parent).min_subtree_key = (*parent).key;
                (*parent).max_subtree_key = (*parent).key;

                if valid_sentinel_ptr(sibling) {
                    (*parent).min_subtree_key =
                        core::cmp::min((*parent).min_subtree_key, (*sibling).min_subtree_key);
                    (*parent).max_subtree_key =
                        core::cmp::max((*parent).max_subtree_key, (*sibling).max_subtree_key);
                }
                if valid_sentinel_ptr(lr_child) {
                    (*parent).min_subtree_key =
                        core::cmp::min((*parent).min_subtree_key, (*lr_child).min_subtree_key);
                    (*parent).max_subtree_key =
                        core::cmp::max((*parent).max_subtree_key, (*lr_child).max_subtree_key);
                }
            }
        }

        fn record_erase(&self, _node: *mut BalanceTestObj, invalidated: *mut BalanceTestObj) {
            self.op_counts.erase_ops.fetch_add(1, Ordering::Relaxed);
            unsafe {
                let mut current = invalidated;
                while valid_sentinel_ptr(current) {
                    (*current).min_subtree_key = (*current).key;
                    (*current).max_subtree_key = (*current).key;

                    let ns = (*current).get_node();
                    let left = ns.get_left();
                    if valid_sentinel_ptr(left) {
                        (*current).min_subtree_key =
                            core::cmp::min((*current).min_subtree_key, (*left).min_subtree_key);
                        (*current).max_subtree_key =
                            core::cmp::max((*current).max_subtree_key, (*left).max_subtree_key);
                    }
                    let right = ns.get_right();
                    if valid_sentinel_ptr(right) {
                        (*current).min_subtree_key =
                            core::cmp::min((*current).min_subtree_key, (*right).min_subtree_key);
                        (*current).max_subtree_key =
                            core::cmp::max((*current).max_subtree_key, (*right).max_subtree_key);
                    }
                    current = ns.get_parent();
                }
            }
        }

        fn record_erase_demote(&self) {
            self.op_counts.erase_demotes.fetch_add(1, Ordering::Relaxed);
        }

        fn record_erase_rotation(&self) {
            self.op_counts.erase_rotations.fetch_add(1, Ordering::Relaxed);
        }

        fn record_erase_double_rotation(&self) {
            self.op_counts.erase_double_rotations.fetch_add(1, Ordering::Relaxed);
        }

        fn verify_rank_rule(
            &self,
            node: *mut BalanceTestObj,
            _left_most: *mut BalanceTestObj,
            _right_most: *mut BalanceTestObj,
            _sentinel: *mut BalanceTestObj,
        ) {
            unsafe {
                let ns = (*node).get_node();
                let rank = ns.rank();
                assert!(rank >= 0, "All ranks must be non-negative.");

                let left = ns.get_left();
                let right = ns.get_right();

                if !valid_sentinel_ptr(left) && !valid_sentinel_ptr(right) {
                    assert_eq!(rank, 0i32, "Leaf nodes must have rank 0!");
                } else {
                    if valid_sentinel_ptr(left) {
                        let left_ns = (*left).get_node();
                        let delta = rank - left_ns.rank();
                        assert!(
                            delta >= 1 && delta <= 2,
                            "Left hand rank difference not in range [1, 2]"
                        );
                    }

                    if valid_sentinel_ptr(right) {
                        let right_ns = (*right).get_node();
                        let delta = rank - right_ns.rank();
                        assert!(
                            delta >= 1 && delta <= 2,
                            "Right hand rank difference not in range [1, 2]"
                        );
                    }
                }
            }
        }

        fn verify_balance(&self, size: usize, depth: usize) {
            if size > 0 {
                let log2_n = (size as f64).log2();
                let erase_ops = self.op_counts.erase_ops.load(Ordering::Relaxed);
                let scale = if erase_ops > 0 { 2.0 } else { 1.4404200904125564 };
                let max_depth = (log2_n * scale) as usize + 1;
                assert!(
                    max_depth >= depth,
                    "Depth bound exceeded! max_depth: {}, actual depth: {}",
                    max_depth,
                    depth
                );

                let insert_rotations = self.op_counts.insert_rotations.load(Ordering::Relaxed);
                let insert_double_rotations =
                    self.op_counts.insert_double_rotations.load(Ordering::Relaxed);
                let insert_promotes = self.op_counts.insert_promotes.load(Ordering::Relaxed);
                let insert_ops = self.op_counts.insert_ops.load(Ordering::Relaxed);

                let total_insert_rotations = insert_rotations + insert_double_rotations;
                assert!(
                    insert_promotes <= (3 * insert_ops) + (2 * erase_ops),
                    "#insert promotes must be <= (3 * #inserts) + (2 * #erases)"
                );
                assert!(
                    total_insert_rotations <= insert_ops,
                    "#insert_rotations must be <= #inserts"
                );

                let erase_demotes = self.op_counts.erase_demotes.load(Ordering::Relaxed);
                let erase_rotations = self.op_counts.erase_rotations.load(Ordering::Relaxed);
                let erase_double_rotations =
                    self.op_counts.erase_double_rotations.load(Ordering::Relaxed);

                let total_erase_rotations = erase_rotations + erase_double_rotations;
                assert!(erase_demotes <= erase_ops, "#erase demotes must be <= #erases");
                assert!(total_erase_rotations <= erase_ops, "#erase_rotations must be <= #erases");

                let inspected_rotations =
                    self.op_counts.inspected_rotations.load(Ordering::Relaxed);
                let total_inspected_rotations = insert_rotations
                    + erase_rotations
                    + 2 * insert_double_rotations
                    + 2 * erase_double_rotations;
                assert_eq!(
                    total_inspected_rotations, inspected_rotations,
                    "#inspected rotations must be == #rotations"
                );
            }
        }
    }

    struct WavlTreeChecker;
    impl WavlTreeChecker {
        fn verify_parent_back_links<K, P, Tag, S, O>(cursor: Cursor<'_, K, P, Tag, S, O>)
        where
            P: PtrTraits,
            P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
            K: Ord,
            S: SizeTracker,
            O: WavlTreeObserver<Target = P::Target>,
        {
            assert!(cursor.is_valid());
            let left = cursor.left();
            if left.is_valid() {
                assert_eq!(
                    cursor.as_raw_ptr(),
                    left.parent().as_raw_ptr(),
                    "Corrupt left-side parent back-link!"
                );
            }

            let right = cursor.right();
            if right.is_valid() {
                assert_eq!(
                    cursor.as_raw_ptr(),
                    right.parent().as_raw_ptr(),
                    "Corrupt right-side parent back-link!"
                );
            }
        }

        fn sanity_check<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>)
        where
            P: PtrTraits,
            P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
            K: Ord,
            S: SizeTracker,
            O: WavlTreeObserver<Target = P::Target>,
        {
            let is_empty = tree.is_empty();
            let root = tree.root_cursor();
            let front = tree.front_cursor();
            let back = tree.back_cursor();

            let sentinel_ptr =
                if is_empty { front.as_raw_ptr() } else { front.left().as_raw_ptr() };

            if is_empty {
                assert!(!root.is_valid());
                assert!(!front.is_valid());
                assert!(!back.is_valid());
                if S::IS_TRACKING {
                    assert_eq!(tree.len(), 0);
                }
            } else {
                assert!(root.is_valid());
                assert!(front.is_valid());
                assert!(back.is_valid());
                assert!(!front.left().is_valid());
                assert!(!back.right().is_valid());
                if S::IS_TRACKING {
                    assert!(tree.len() > 0);
                }
            }

            let mut cur_depth = 0;
            let mut depth = 0;
            let mut size = 0;

            let mut cursor = root;

            while cursor.is_valid() {
                Self::verify_parent_back_links(cursor);
                cur_depth += 1;

                let left = cursor.left();
                if !left.is_valid() {
                    break;
                }
                cursor = left;
            }

            while cursor.is_valid() {
                if depth < cur_depth {
                    depth = cur_depth;
                }
                size += 1;

                Self::verify_parent_back_links(cursor);
                tree.observer.verify_rank_rule(
                    cursor.as_raw_ptr(),
                    front.as_raw_ptr(),
                    back.as_raw_ptr(),
                    sentinel_ptr,
                );

                let right = cursor.right();
                if right.is_valid() {
                    cur_depth += 1;
                    cursor = right;
                    Self::verify_parent_back_links(cursor);

                    loop {
                        let left = cursor.left();
                        if !left.is_valid() {
                            break;
                        }
                        cur_depth += 1;
                        cursor = left;
                        Self::verify_parent_back_links(cursor);
                    }
                    continue;
                }

                let mut parent = cursor.parent();
                let mut keep_going = false;
                while parent.is_valid() {
                    let is_left = parent.left() == cursor;
                    let is_right = parent.right() == cursor;

                    assert!(is_left != is_right);
                    assert!(is_left || is_right);

                    cursor = parent;
                    cur_depth -= 1;

                    if is_left {
                        keep_going = true;
                        break;
                    }

                    parent = parent.parent();
                }

                if !keep_going {
                    break;
                }
            }

            if S::IS_TRACKING {
                assert_eq!(tree.len(), size);
            }
            tree.observer.verify_balance(size, depth);
        }
    }

    fn shuffle_erase_deck(objects: &[Box<BalanceTestObj>], rng: &mut Lfsr, size: usize) {
        for i in (2..size).rev() {
            let ndx = (rng.get_next() as usize) % i;
            if ndx != i {
                BalanceTestObj::swap_erase_deck_ptr(&objects[i], &objects[ndx]);
            }
        }
    }

    fn check_augmented_invariants<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>)
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        if tree.is_empty() {
            return;
        }
        let root = tree.root_cursor().as_raw_ptr() as *mut BalanceTestObj;
        let left = tree.front_cursor().as_raw_ptr() as *mut BalanceTestObj;
        let right = tree.back_cursor().as_raw_ptr() as *mut BalanceTestObj;

        unsafe {
            assert_eq!((*root).min_subtree_key, (*left).key, "Min subtree key invariant violated!");
            assert_eq!(
                (*root).max_subtree_key,
                (*right).key,
                "Max subtree key invariant violated!"
            );
        }
    }

    fn check_iterators<K, P, Tag, S, O>(tree: &WavlTree<K, P, Tag, S, O>)
    where
        P: PtrTraits,
        P::Target: WavlTreeContainable<P::Target, Tag> + WavlTreeKeyable<K>,
        K: Ord,
        S: SizeTracker,
        O: WavlTreeObserver<Target = P::Target>,
    {
        if tree.is_empty() {
            return;
        }
        let root = tree.root_cursor();
        let left_most = tree.front_cursor();
        let right_most = tree.back_cursor();

        let mut left_cursor = root;
        let mut right_cursor = root;
        let mut i = 0;

        let limit = if S::IS_TRACKING { tree.len() } else { 10000 };

        while (left_cursor != left_most || right_cursor != right_most) && i < limit {
            assert!(left_cursor.is_valid());
            if left_cursor == left_most {
                assert!(!left_cursor.left().is_valid());
            } else {
                left_cursor = left_cursor.left();
            }

            assert!(right_cursor.is_valid());
            if right_cursor == right_most {
                assert!(!right_cursor.right().is_valid());
            } else {
                right_cursor = right_cursor.right();
            }

            i += 1;
        }

        assert_eq!(left_cursor, left_most);
        assert_eq!(right_cursor, right_most);

        let limit = i;
        left_cursor = left_most;
        right_cursor = right_most;
        i = 0;

        while (left_cursor != root || right_cursor != root) && i < limit {
            assert!(left_cursor.is_valid());
            if left_cursor == root {
                assert!(!left_cursor.parent().is_valid());
            } else {
                left_cursor = left_cursor.parent();
            }

            assert!(right_cursor.is_valid());
            if right_cursor == root {
                assert!(!right_cursor.parent().is_valid());
            } else {
                right_cursor = right_cursor.parent();
            }

            i += 1;
        }

        assert_eq!(left_cursor, root);
        assert_eq!(right_cursor, root);
    }

    #[test]
    fn test_balance_and_invariants() {
        let seeds = [0xe87e1062fc1f4f80u64, 0x03d6bffb124b4918u64, 0x8f7d83e8d10b4765u64];
        let test_size = 128;
        let replacement_count = test_size / 8;
        let mut rng = Lfsr::new(1);

        for seed_ndx in 0..seeds.len() {
            let seed = seeds[seed_ndx];
            rng.set_core(seed);

            let op_counts = Arc::new(OpCounts::new());
            let observer = WavlBalanceTestObserver { op_counts: Arc::clone(&op_counts) };

            stack_pin_init!(let tree = WavlTree::<u64, *mut BalanceTestObj, DefaultObjectTag, TrackingSize, WavlBalanceTestObserver>::new_with_observer(observer));
            let tree = unsafe { tree.get_unchecked_mut() };

            let mut objects = Vec::with_capacity(test_size);
            let mut replacements = Vec::with_capacity(replacement_count);

            match seed_ndx {
                0 => {
                    for i in 0..test_size {
                        let obj = Box::new(BalanceTestObj::new(i as u64));
                        let raw = &*obj as *const BalanceTestObj as *mut BalanceTestObj;
                        obj.erase_deck_ptr.set(raw);
                        objects.push(obj);

                        if i < replacement_count {
                            let rep = Box::new(BalanceTestObj::new(i as u64));
                            let raw = &*rep as *const BalanceTestObj as *mut BalanceTestObj;
                            rep.erase_deck_ptr.set(raw);
                            replacements.push(rep);
                        }
                    }
                }
                1 => {
                    for i in 0..test_size {
                        let obj = Box::new(BalanceTestObj::new((test_size - i) as u64));
                        let raw = &*obj as *const BalanceTestObj as *mut BalanceTestObj;
                        obj.erase_deck_ptr.set(raw);
                        objects.push(obj);

                        if i < replacement_count {
                            let rep = Box::new(BalanceTestObj::new((test_size - i) as u64));
                            let raw = &*rep as *const BalanceTestObj as *mut BalanceTestObj;
                            rep.erase_deck_ptr.set(raw);
                            replacements.push(rep);
                        }
                    }
                }
                _ => {
                    for i in 0..test_size {
                        let val = rng.get_next();
                        let obj = Box::new(BalanceTestObj::new(val));
                        let raw = &*obj as *const BalanceTestObj as *mut BalanceTestObj;
                        obj.erase_deck_ptr.set(raw);
                        objects.push(obj);

                        if i < replacement_count {
                            let rep = Box::new(BalanceTestObj::new(val));
                            let raw = &*rep as *const BalanceTestObj as *mut BalanceTestObj;
                            rep.erase_deck_ptr.set(raw);
                            replacements.push(rep);
                        }
                    }
                }
            }

            // 1. Insert all objects
            for i in 0..test_size {
                unsafe {
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                    let raw = &mut *objects[i] as *mut BalanceTestObj;
                    tree.insert_raw(raw);
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                }
            }

            check_iterators(tree);

            // 2. Collide replacements
            for i in 0..replacement_count {
                unsafe {
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                    let raw = &mut *replacements[i] as *mut BalanceTestObj;
                    assert!(tree.insert_or_find_raw(raw).is_err());
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                }
            }

            // 3. Replace original nodes with replacements
            for i in 0..replacement_count {
                unsafe {
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                    let raw = &mut *replacements[i] as *mut BalanceTestObj;
                    assert!(tree.insert_or_replace_raw(raw).is_some());
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                }
            }

            check_iterators(tree);

            // 4. Swap them back
            for i in 0..replacement_count {
                unsafe {
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                    let raw = &mut *objects[i] as *mut BalanceTestObj;
                    assert!(tree.insert_or_replace_raw(raw).is_some());
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                }
            }

            check_iterators(tree);

            // Shuffle erase deck
            shuffle_erase_deck(&objects, &mut rng, test_size);

            // 5. Erase half the elements
            for i in 0..(test_size / 2) {
                unsafe {
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                    let raw_target = objects[i].erase_deck_ptr.get();
                    let erased = tree.erase_raw(raw_target);
                    assert!(erased.is_some());
                    assert_eq!(erased.unwrap(), raw_target);
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                }
            }

            check_iterators(tree);

            // 6. Put them back
            for i in 0..(test_size / 2) {
                unsafe {
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                    let raw_target = objects[i].erase_deck_ptr.get();
                    tree.insert_raw(raw_target);
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                }
            }

            check_iterators(tree);

            // Shuffle erase deck again
            shuffle_erase_deck(&objects, &mut rng, test_size);

            // 7. Erase everything
            for i in 0..test_size {
                unsafe {
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                    let raw_target = objects[i].erase_deck_ptr.get();
                    let erased = tree.erase_raw(raw_target);
                    assert!(erased.is_some());
                    assert_eq!(erased.unwrap(), raw_target);
                    check_augmented_invariants(tree);
                    WavlTreeChecker::sanity_check(tree);
                }
            }

            check_iterators(tree);
            assert_eq!(tree.size.get(), 0);

            assert!(op_counts.insert_ops.load(Ordering::Relaxed) > 0);
            assert!(op_counts.insert_promotes.load(Ordering::Relaxed) > 0);
            assert!(op_counts.insert_rotations.load(Ordering::Relaxed) > 0);
            assert!(op_counts.insert_traversals.load(Ordering::Relaxed) > 0);
            assert!(op_counts.erase_ops.load(Ordering::Relaxed) > 0);
            assert!(op_counts.erase_demotes.load(Ordering::Relaxed) > 0);
            assert!(op_counts.erase_rotations.load(Ordering::Relaxed) > 0);
        }
    }

    // WavlTree FFI Declarations
    unsafe extern "C" {
        // UniqueTree Helpers
        fn cpp_create_unique_tree() -> *mut c_void;
        fn cpp_destroy_unique_tree(tree: *mut c_void);
        fn cpp_unique_tree_insert(tree: *mut c_void, item: *mut c_void);
        fn cpp_unique_tree_erase(tree: *mut c_void, key: i32) -> *mut c_void;
        fn cpp_unique_tree_find(tree: *mut c_void, key: i32) -> *mut c_void;
        fn cpp_unique_tree_is_empty(tree: *mut c_void) -> bool;

        // RefTree Helpers
        fn cpp_create_ref_tree() -> *mut c_void;
        fn cpp_destroy_ref_tree(tree: *mut c_void);
        fn cpp_ref_tree_insert(tree: *mut c_void, item: *mut c_void);
        fn cpp_ref_tree_erase(tree: *mut c_void, key: i32) -> *mut c_void;
        fn cpp_ref_tree_find(tree: *mut c_void, key: i32) -> *mut c_void;
        fn cpp_ref_tree_is_empty(tree: *mut c_void) -> bool;

        // SharedUniqueObject Helpers (Defined in intrusive_container_test_support.cc)
        fn cpp_create_unique_object(value: i32, destruction_flag: *mut bool) -> *mut c_void;
        fn cpp_get_unique_object_value(obj: *mut c_void) -> i32;

        // SharedRefObject Helpers (Defined in intrusive_container_test_support.cc)
        fn cpp_create_ref_object(value: i32, destruction_flag: *mut bool) -> *mut c_void;
        fn cpp_get_ref_object_value(obj: *mut c_void) -> i32;
    }

    #[test]
    fn test_interop_rust_tree_cpp_unique_objects() {
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed1 = AtomicBool::new(false);
        let destroyed2 = AtomicBool::new(false);

        unsafe {
            stack_pin_init!(let tree = WavlTree::<i32, UniquePtr<SharedUniqueObject>>::new());
            let tree = tree.get_unchecked_mut();

            let cpp_raw1 = cpp_create_unique_object(10, destroyed1.as_ptr() as *mut bool);
            let cpp_raw2 = cpp_create_unique_object(20, destroyed2.as_ptr() as *mut bool);

            let obj1 = UniquePtr::from_raw(cpp_raw1 as *mut SharedUniqueObject);
            let obj2 = UniquePtr::from_raw(cpp_raw2 as *mut SharedUniqueObject);

            tree.insert(obj1);
            tree.insert(obj2);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Find one
            let found = tree.find(&10);
            assert!(found.is_some());
            assert_eq!(found.unwrap().value, 10);

            // Erase one
            let popped = tree.erase(&20);
            assert!(popped.is_some());
            assert_eq!(popped.as_ref().unwrap().value, 20);

            // Drop popped -> should destroy in C++!
            drop(popped);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Drop tree -> should destroy remaining in C++!
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }

    #[test]
    fn test_interop_cpp_tree_rust_unique_objects() {
        use alloc::sync::Arc;
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed1 = Arc::new(AtomicBool::new(false));
        let destroyed2 = Arc::new(AtomicBool::new(false));

        unsafe {
            let cpp_tree = cpp_create_unique_tree();
            assert!(cpp_unique_tree_is_empty(cpp_tree));

            let obj1 = UniquePtr::try_new(SharedUniqueObject::new(10)).unwrap();
            let obj2 = UniquePtr::try_new(SharedUniqueObject::new(20)).unwrap();

            // Set destruction flags
            let raw1 = UniquePtr::as_ptr(&obj1) as *mut SharedUniqueObject;
            (*raw1).destruction_flag = destroyed1.as_ptr() as *mut bool;
            let raw2 = UniquePtr::as_ptr(&obj2) as *mut SharedUniqueObject;
            (*raw2).destruction_flag = destroyed2.as_ptr() as *mut bool;

            // Push to C++ tree (transfers ownership)
            cpp_unique_tree_insert(cpp_tree, UniquePtr::into_raw(obj1) as *mut c_void);
            cpp_unique_tree_insert(cpp_tree, UniquePtr::into_raw(obj2) as *mut c_void);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Find in C++
            let found = cpp_unique_tree_find(cpp_tree, 10);
            assert!(!found.is_null());
            assert_eq!(cpp_get_unique_object_value(found), 10);

            // Erase one from C++
            let popped = cpp_unique_tree_erase(cpp_tree, 20);
            assert!(!popped.is_null());
            assert_eq!(cpp_get_unique_object_value(popped), 20);

            // Convert back to Rust UniquePtr and drop -> should free in Rust!
            let popped_rust = UniquePtr::from_raw(popped as *mut SharedUniqueObject);
            drop(popped_rust);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Destroy C++ tree -> should destroy remaining in Rust!
            cpp_destroy_unique_tree(cpp_tree);
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }

    #[test]
    fn test_interop_rust_tree_cpp_ref_objects() {
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed1 = AtomicBool::new(false);
        let destroyed2 = AtomicBool::new(false);

        unsafe {
            stack_pin_init!(let tree = WavlTree::<i32, RefPtr<SharedRefObject>>::new());
            let tree = tree.get_unchecked_mut();

            let cpp_raw1 = cpp_create_ref_object(10, destroyed1.as_ptr() as *mut bool);
            let cpp_raw2 = cpp_create_ref_object(20, destroyed2.as_ptr() as *mut bool);

            let obj1 = RefPtr::from_raw(cpp_raw1 as *mut SharedRefObject);
            let obj2 = RefPtr::from_raw(cpp_raw2 as *mut SharedRefObject);

            tree.insert(obj1);
            tree.insert(obj2);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Find one
            let found = tree.find(&10);
            assert!(found.is_some());
            assert_eq!(found.unwrap().value, 10);

            // Erase one
            let popped = tree.erase(&20);
            assert!(popped.is_some());
            assert_eq!(popped.as_ref().unwrap().value, 20);

            // Drop popped -> should destroy in C++!
            drop(popped);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Drop tree -> should destroy remaining in C++!
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }

    #[test]
    fn test_interop_cpp_tree_rust_ref_objects() {
        use alloc::sync::Arc;
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed1 = Arc::new(AtomicBool::new(false));
        let destroyed2 = Arc::new(AtomicBool::new(false));

        unsafe {
            let cpp_tree = cpp_create_ref_tree();
            assert!(cpp_ref_tree_is_empty(cpp_tree));

            let obj1 = SharedRefObject::new_ref_counted(10);
            let obj2 = SharedRefObject::new_ref_counted(20);

            // Set destruction flags
            let raw1 = RefPtr::as_ptr(&obj1) as *mut SharedRefObject;
            (*raw1).destruction_flag = destroyed1.as_ptr() as *mut bool;
            let raw2 = RefPtr::as_ptr(&obj2) as *mut SharedRefObject;
            (*raw2).destruction_flag = destroyed2.as_ptr() as *mut bool;

            // Insert to C++ tree (transfers ownership)
            cpp_ref_tree_insert(
                cpp_tree,
                RefPtr::into_raw(obj1) as *mut SharedRefObject as *mut c_void,
            );
            cpp_ref_tree_insert(
                cpp_tree,
                RefPtr::into_raw(obj2) as *mut SharedRefObject as *mut c_void,
            );

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Find in C++
            let found = cpp_ref_tree_find(cpp_tree, 10);
            assert!(!found.is_null());
            assert_eq!(cpp_get_ref_object_value(found), 10);

            // Erase one from C++
            let popped = cpp_ref_tree_erase(cpp_tree, 20);
            assert!(!popped.is_null());
            assert_eq!(cpp_get_ref_object_value(popped), 20);

            // Convert back to Rust RefPtr and drop -> should free in Rust!
            let popped_rust = RefPtr::from_raw(popped as *mut SharedRefObject);
            drop(popped_rust);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Destroy C++ tree -> should destroy remaining in Rust!
            cpp_destroy_ref_tree(cpp_tree);
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }

    #[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
    struct LargeKey {
        name: [u8; 32],
    }

    #[derive(crate::WavlTreeContainable, crate::Recyclable)]
    struct TestRefKeyObject {
        key: LargeKey,
        #[wavl_node]
        node: WavlTreeNode<TestRefKeyObject>,
    }

    impl TestRefKeyObject {
        fn new(name: [u8; 32]) -> Self {
            Self { key: LargeKey { name }, node: WavlTreeNode::new() }
        }
    }

    impl WavlTreeKeyable<LargeKey> for TestRefKeyObject {
        type Key<'a> = &'a LargeKey;
        fn get_key(&self) -> &LargeKey {
            &self.key
        }
    }

    #[test]
    fn test_by_reference_key() {
        use crate::UniquePtr;

        type TestTree = WavlTree<LargeKey, UniquePtr<TestRefKeyObject>>;
        stack_pin_init!(let tree = TestTree::new());
        let tree = unsafe { tree.get_unchecked_mut() };

        let mut key1 = [0u8; 32];
        key1[0] = 10;
        let mut key2 = [0u8; 32];
        key2[0] = 20;

        let obj1 = UniquePtr::try_new(TestRefKeyObject::new(key1)).unwrap();
        let obj2 = UniquePtr::try_new(TestRefKeyObject::new(key2)).unwrap();
        tree.insert(obj1);
        tree.insert(obj2);

        let query = LargeKey { name: key1 };
        let found = tree.find(&query);
        assert!(found.is_some());
        assert_eq!(found.unwrap().key, query);

        let erased = tree.erase(&query);
        assert!(erased.is_some());
        assert_eq!(erased.unwrap().key, query);
    }

    #[derive(WavlTreeContainable, Recyclable)]
    struct TestAugmentedObject {
        value: i32,
        subtree_sum: i32,
        #[wavl_node]
        node: WavlTreeNode<TestAugmentedObject>,
    }

    impl TestAugmentedObject {
        fn new(value: i32) -> Self {
            Self { value, subtree_sum: 0, node: WavlTreeNode::new() }
        }
    }

    impl WavlTreeKeyable<i32> for TestAugmentedObject {
        type Key<'a> = i32;
        fn get_key(&self) -> i32 {
            self.value
        }
    }

    struct SubtreeSumTraits;
    impl WavlTreeAugmentedInvariantObserverTraits for SubtreeSumTraits {
        type Target = TestAugmentedObject;
        type Value = i32;

        fn get_node_value(node: &Self::Target) -> Self::Value {
            node.value
        }

        fn get_subtree_value(node: &Self::Target) -> Self::Value {
            node.subtree_sum
        }

        fn combine_values(a: Self::Value, b: Self::Value) -> Self::Value {
            a + b
        }

        fn set_subtree_value(node: &mut Self::Target, val: Self::Value) {
            node.subtree_sum = val;
        }

        fn reset_subtree_value(node: &mut Self::Target) {
            node.subtree_sum = 0;
        }
    }

    #[test]
    fn test_augmented_invariant_observer() {
        type TestObserver = WavlTreeAugmentedInvariantObserver<DefaultObjectTag, SubtreeSumTraits>;
        type TestTree = WavlTree<
            i32,
            UniquePtr<TestAugmentedObject>,
            DefaultObjectTag,
            TrackingSize,
            TestObserver,
        >;

        stack_pin_init!(let tree = TestTree::new_with_observer(TestObserver::default()));
        let tree = unsafe { tree.get_unchecked_mut() };

        // Helper to verify subtree sums recursively
        fn verify_subtree_sums(node: *mut TestAugmentedObject) -> i32 {
            if !valid_sentinel_ptr(node) {
                return 0;
            }
            let node_ref = unsafe { &*node };
            let ns = node_ref.get_node();
            let left_sum = verify_subtree_sums(ns.get_left());
            let right_sum = verify_subtree_sums(ns.get_right());
            let expected_sum = node_ref.value + left_sum + right_sum;
            assert_eq!(
                node_ref.subtree_sum, expected_sum,
                "Subtree sum mismatch at node {}",
                node_ref.value
            );
            expected_sum
        }

        // Insert some nodes
        let values = [10, 20, 5, 15, 25, 2, 7];
        for &val in &values {
            let obj = UniquePtr::try_new(TestAugmentedObject::new(val)).unwrap();
            tree.insert(obj);
            // Verify after each insert
            if !tree.root.is_null() {
                verify_subtree_sums(tree.root);
            }
        }

        // Verify final sum at root
        let expected_total_sum: i32 = values.iter().sum();
        assert_eq!(unsafe { &*tree.root }.subtree_sum, expected_total_sum);

        // Erase some nodes
        let to_erase = [15, 5, 20];
        let mut current_sum = expected_total_sum;
        for &val in &to_erase {
            let erased = tree.erase(&val);
            assert!(erased.is_some());
            assert_eq!(erased.as_ref().unwrap().value, val);
            current_sum -= val;
            if !tree.root.is_null() {
                assert_eq!(unsafe { &*tree.root }.subtree_sum, current_sum);
                verify_subtree_sums(tree.root);
            }
        }
    }

    #[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
    struct NonCommutativeVal {
        hash: u64,
        len: u32,
    }

    impl NonCommutativeVal {
        fn single(val: i32) -> Self {
            Self { hash: (val as u64) & 0xffff, len: 1 }
        }

        fn combine(left: Self, right: Self) -> Self {
            if left.len == 0 {
                return right;
            }
            if right.len == 0 {
                return left;
            }
            let mut power = 1u64;
            for _ in 0..right.len {
                power = power.wrapping_mul(31);
            }
            Self {
                hash: left.hash.wrapping_mul(power).wrapping_add(right.hash),
                len: left.len + right.len,
            }
        }
    }

    #[derive(WavlTreeContainable, Recyclable)]
    struct TestNonCommutativeObject {
        value: i32,
        subtree_val: NonCommutativeVal,
        #[wavl_node]
        node: WavlTreeNode<TestNonCommutativeObject>,
    }

    impl TestNonCommutativeObject {
        fn new(value: i32) -> Self {
            Self { value, subtree_val: NonCommutativeVal::default(), node: WavlTreeNode::new() }
        }
    }

    impl WavlTreeKeyable<i32> for TestNonCommutativeObject {
        type Key<'a> = i32;
        fn get_key(&self) -> i32 {
            self.value
        }
    }

    struct NonCommutativeTraits;
    impl WavlTreeAugmentedInvariantObserverTraits for NonCommutativeTraits {
        type Target = TestNonCommutativeObject;
        type Value = NonCommutativeVal;

        fn get_node_value(node: &Self::Target) -> Self::Value {
            NonCommutativeVal::single(node.value)
        }

        fn get_subtree_value(node: &Self::Target) -> Self::Value {
            node.subtree_val
        }

        fn combine_values(a: Self::Value, b: Self::Value) -> Self::Value {
            NonCommutativeVal::combine(a, b)
        }

        fn set_subtree_value(node: &mut Self::Target, val: Self::Value) {
            node.subtree_val = val;
        }

        fn reset_subtree_value(node: &mut Self::Target) {
            node.subtree_val = NonCommutativeVal::default();
        }
    }

    #[test]
    fn test_augmented_invariant_observer_non_commutative() {
        type TestObserver =
            WavlTreeAugmentedInvariantObserver<DefaultObjectTag, NonCommutativeTraits>;
        type TestTree = WavlTree<
            i32,
            UniquePtr<TestNonCommutativeObject>,
            DefaultObjectTag,
            TrackingSize,
            TestObserver,
        >;

        stack_pin_init!(let tree = TestTree::new_with_observer(TestObserver::default()));
        let tree = unsafe { tree.get_unchecked_mut() };

        // Helper to verify non-commutative subtree values recursively
        fn verify_subtree_values(node: *mut TestNonCommutativeObject) -> NonCommutativeVal {
            if !valid_sentinel_ptr(node) {
                return NonCommutativeVal::default();
            }
            let node_ref = unsafe { &*node };
            let ns = node_ref.get_node();
            let left_val = verify_subtree_values(ns.get_left());
            let node_val = NonCommutativeVal::single(node_ref.value);
            let right_val = verify_subtree_values(ns.get_right());
            let expected = NonCommutativeVal::combine(
                NonCommutativeVal::combine(left_val, node_val),
                right_val,
            );
            assert_eq!(
                node_ref.subtree_val, expected,
                "Subtree non-commutative value mismatch at node {}",
                node_ref.value
            );
            expected
        }

        // Helper to compute expected in-order value from a list of sorted keys
        fn expected_in_order(keys: &[i32]) -> NonCommutativeVal {
            let mut sorted: alloc::vec::Vec<i32> = keys.iter().copied().collect();
            sorted.sort();
            let mut acc = NonCommutativeVal::default();
            for &k in &sorted {
                acc = NonCommutativeVal::combine(acc, NonCommutativeVal::single(k));
            }
            acc
        }

        // Insert nodes in an order that triggers single and double rotations in both directions
        let values = [10, 20, 5, 15, 25, 2, 7, 1, 3, 6, 8, 12, 17, 22, 30];
        for (i, &val) in values.iter().enumerate() {
            let obj = UniquePtr::try_new(TestNonCommutativeObject::new(val)).unwrap();
            tree.insert(obj);
            if !tree.root.is_null() {
                verify_subtree_values(tree.root);
                let expected = expected_in_order(&values[..=i]);
                assert_eq!(unsafe { &*tree.root }.subtree_val, expected);
            }
        }

        // Erase nodes causing rebalancings/rotations
        let to_erase = [5, 20, 15, 2, 25];
        let mut remaining: alloc::vec::Vec<i32> = values.iter().copied().collect();
        for &val in &to_erase {
            let erased = tree.erase(&val);
            assert!(erased.is_some());
            assert_eq!(erased.as_ref().unwrap().value, val);
            remaining.retain(|&x| x != val);
            if !tree.root.is_null() {
                verify_subtree_values(tree.root);
                let expected = expected_in_order(&remaining);
                assert_eq!(unsafe { &*tree.root }.subtree_val, expected);
            }
        }
    }
}
