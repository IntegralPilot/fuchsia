// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef FBL_WAVL_TREE_AUGMENTED_INVARIANT_OBSERVER_H_
#define FBL_WAVL_TREE_AUGMENTED_INVARIANT_OBSERVER_H_

#include <zircon/assert.h>

#include <fbl/macros.h>

namespace fbl {

// WAVLTreeAugmentedInvariantObserver
//
// Definition of a WAVLTreeObserver which helps to automate the process of maintaining augmented
// invariants inside of a WAVL tree.
//
// Traits implementations should contain the following definitions:
//
// struct Traits {
//  // Returns the invariant value for the given node.
//  static SubtreeValueType GetNodeValue(const Object& node) { ... }
//
//  // Returns the invariant value for the given node's subtree.
//  static SubtreeValueType GetSubtreeValue(const Object& node) { ... }
//
//  // Combines subtree and node invariant values to produce a new subtree invarant value.
//  static SubtreeValueType CombineValues(SubtreeValueType a, SubtreeValueType b) { ... }
//
//  // Sets the node's subtree invariant value.
//  static void SetSubtreeValue(Object& node, SubtreeValueType val) { ... }
//
//  // Resets the node's subtree invariant value.
//  static void ResetSubtreeValue(Object& target) { ... }
// };
//
// In addition to the traits which define the value to maintain, WAVLTreeAugmentedInvariantObserver
// has two more boolean template parameters which can be used to control behavior:
//
// AllowInsertOrFindCollision
// AllowInsertOrReplaceCollision
//
// By default, both of these values are true. If a collision happens during either an insert_or_find
// or an insert_or_replace operation, the subtree invariant will be maintained.  On the other hand,
// if a user knows that they will never encounter collisions as a result of one or the other (or
// both) of these operations, they may set the appropriate Allow template argument to false, causing
// the WAVLTreeAugmentedInvariantObserver to fail a ZX_DEBUG_ASSERT in the case that associated
// collision is ever encountered during operation.
//
template <typename Traits, bool AllowInsertOrFindCollision = true,
          bool AllowInsertOrReplaceCollision = true>
struct WAVLTreeAugmentedInvariantObserver {
 private:
  DECLARE_HAS_MEMBER_FN(has_on_insert_collision, OnInsertCollision);
  DECLARE_HAS_MEMBER_FN(has_on_insert_replace, OnInsertReplace);

 public:
  // Initializes the subtree value of the node when it is first inserted as a leaf.
  template <typename Iter>
  static void RecordInsert(Iter node) {
    Traits::SetSubtreeValue(*node, Traits::GetNodeValue(*node));
  }

  // Updates the subtree value of an ancestor node as it is traversed during insertion.
  template <typename T, typename Iter>
  static void RecordInsertTraverse(T* node, Iter ancestor) {
    const auto node_value = Traits::GetNodeValue(*node);
    const auto ancestor_subtree_value = Traits::GetSubtreeValue(*ancestor);
    const auto combined_value = Traits::CombineValues(ancestor_subtree_value, node_value);
    Traits::SetSubtreeValue(*ancestor, combined_value);
  }

  // Fixes invalid invariants in nodes traversed along the path to the colliding node when a node is
  // not inserted into the tree due to the collision.
  template <typename T, typename Iter>
  static void RecordInsertCollision(T* node, Iter collision) {
    ZX_DEBUG_ASSERT(AllowInsertOrFindCollision);
    RecomputeUntilRoot(collision);
  }

  // The node is still in the tree and is about to be replaced by replacement. Temporarily update
  // the value of node to the value of the replacement, then propagate the value up the tree to the
  // root and transfer the computed invariant value for for the node's subtree over to the
  // replacement.

  // Fixes invalid invariants in nodes traversed along the path to the colliding node when a node is
  // replace in the tree due to the collision.
  template <typename Iter, typename T>
  static void RecordInsertReplace(Iter node, T* replacement) {
    ZX_DEBUG_ASSERT(AllowInsertOrReplaceCollision);
    // The node is still in the tree and will be swapped out with replacement after this hook
    // returns. Set node to the value of the replacement so that the effect of the node's removal
    // can be propagated up the tree.
    UpdateSubtreeValue(Traits::GetNodeValue(*replacement), node);
    RecomputeUntilRoot(node.parent());

    // Copy the updated values to the replacement and reset the node.
    Traits::SetSubtreeValue(*replacement, Traits::GetSubtreeValue(*node));
    Traits::ResetSubtreeValue(*node);
  }

  // Rotations are used to adjust the height of nodes that are out of balance. During a rotation,
  // the pivot takes the position of the parent, and takes over storing the invariant value for the
  // subtree, as all of the nodes in the overall subtree remain the same. The original parent
  // inherits the lr_child of the pivot, potentially invalidating its new subtree and requiring an
  // update.
  //
  // The following diagrams the relationship of the nodes in a left rotation:
  //
  //           ::After::                      ::Before::                           |
  //                                                                               |
  //             pivot                          parent                             |
  //            /     \                         /    \                             |
  //        parent  rl_child  <-----------  sibling  pivot                         |
  //        /    \                                   /   \                         |
  //   sibling  lr_child                       lr_child  rl_child                  |
  //
  // In a right rotation, all of the relationships are reflected. However, this does not affect the
  // update logic.
  template <typename Iter>
  static void RecordRotation(Iter pivot, Iter lr_child, Iter rl_child, Iter parent, Iter sibling) {
    // The overall subtree value does not change as pivot node assumes the position of the parent
    // node.
    Traits::SetSubtreeValue(*pivot, Traits::GetSubtreeValue(*parent));

    // Update the parent node subtree state to reflect the new descendent nodes, sibling and
    // lr_child.
    auto parent_value = Traits::GetNodeValue(*parent);

    if (sibling) {
      const auto sibling_subtree_value = Traits::GetSubtreeValue(*sibling);
      parent_value = Traits::CombineValues(sibling_subtree_value, parent_value);
    }

    if (lr_child) {
      const auto lr_child_subtree_value = Traits::GetSubtreeValue(*lr_child);
      parent_value = Traits::CombineValues(lr_child_subtree_value, parent_value);
    }

    Traits::SetSubtreeValue(*parent, parent_value);
  }

  // Restores the subtree values of the invalidated ancestors from the point of removal up to the
  // root node.
  template <typename T, typename Iter>
  static void RecordErase(T* node, Iter invalidated) {
    RecomputeUntilRoot(invalidated);
    Traits::ResetSubtreeValue(*node);
  }

  // Promotion/Demotion/DoubleRotation count hooks are not needed to maintain subtree invariants.
  static void RecordInsertPromote() {}
  static void RecordInsertRotation() {}
  static void RecordInsertDoubleRotation() {}
  static void RecordEraseDemote() {}
  static void RecordEraseRotation() {}
  static void RecordEraseDoubleRotation() {}

 private:
  template <typename Iter>
  static void RecomputeUntilRoot(Iter current) {
    for (; current; current = current.parent()) {
      UpdateSubtreeValue(Traits::GetNodeValue(*current), current);
    }
  }

  template <typename ValueType, typename Iter>
  static void UpdateSubtreeValue(ValueType value, Iter node) {
    if (Iter left = node.left(); left) {
      const auto left_subtree_value = Traits::GetSubtreeValue(*left);
      value = Traits::CombineValues(left_subtree_value, value);
    }

    if (Iter right = node.right(); right) {
      const auto right_subtree_value = Traits::GetSubtreeValue(*right);
      value = Traits::CombineValues(right_subtree_value, value);
    }

    Traits::SetSubtreeValue(*node, value);
  }
};

}  // namespace fbl

#endif  // FBL_WAVL_TREE_AUGMENTED_INVARIANT_OBSERVER_H_
