// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_MAPPING_OBSERVER_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_MAPPING_OBSERVER_H_

#include <assert.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stddef.h>
#include <sys/types.h>

#include <fbl/intrusive_wavl_tree.h>
#include <ktl/algorithm.h>
#include <ktl/declval.h>
#include <ktl/type_traits.h>
#include <lockdep/guard.h>

//
// # VmMapping Augmented Binary Search Tree Support
//
// The following types provide the state and tree maintenance hooks to implement an augmented binary
// search tree for VmMappings. The augmentation maintains information about the largest mapping end
// address in the subregion, allowing for efficiently finding all mappings that overlap with a given
// range.
//
// ## General Approach
//
// VmObject maintains an ordered set of possibly overlapping mappings sorted by base offset, with
// the object heap address used as a secondary sorting key for mappings that share the base offset.
// The mappings, characterized by base offset and size, are instances of a VmMapping. Many different
// mappings might reference the same offset.
//
// The set of mappings is stored in a BTree and the approach described here takes advantage of the
// augmented tree subtree information that can be stored as part of the tree nodes to improve the
// time complexity of finding the set of mappings that might contain a range of offsets.
//
// ### Base Representation
//
// The following diagram is a linear representation of the offsets covering addresses 0 to 20
// with six mappings (small numbers are used for simplicity) across two lines due to overlap. The
// boxes represent mappings labeled <first offset>,<last offset>.
//
//   +-------+       +---------------------------+                +-----------------------+
//   | [0,1] |       |          [4,10]           |                |        [15,20]        |
//   +-------+       +---------------------------+                +-----------------------+
//               +-----------+---------------+--------------------------------+
//               |   [3,5]   |     [6,8]     |              [9,17]            |
//               +-----------+---------------+--------------------------------+
//     0   1   2   3   4   5   6   7   8   9   10  11  12  13  14  15  16  17  18  19  20
//
// The following diagram illustrates the same mappings in a btree representation, sorted by
// <first offset>. The actual structure would vary depending on insertion order of leaf node size.
//
//                                            +-----------+
//                                            | [ ] 9 [ ] |
//                                            +--^-----^--+
//                                               |     |
//                      +------------------------+     +---------------------+
//                      |                                                    |
//      +-------+-------+----------+-----+                          +--------+---------+
//      | [0,1] | [3,5] | [4,10] | [6,8] |                          | [9,17] | [15,20] |
//      +-------+-------+----------+-----+                          +--------+---------+
//
// This structure supports efficient searches for mappings that begin at a particular offset in
// O(log n) time. However, finding all mappings that cover a particular offset requires, in the
// worst case, a full tree walk, since no information about the size of the mappings is encoded in
// the tree. Therefore as long as the base offset is below the search offset, any mapping somewhere
// in the subtree could extend into the search offset.
//
// ### Augmented Representation
//
// The augmented representation builds on the base by storing and maintaining the small and largest
// offset of subtree of each node. This allows for skipping subtrees that cannot have a mapping that
// might contain the search offset.
//
// The following diagram illustrates the augmented BTree representation for the same allocated
// regions as the previous illustration.
//
//                                         +-----------------+
//                                         | min: 0, max: 20 |
//                                         | [ ] 9 [ ]       |
//                                         +--^-----^--------+
//                                            |     |
//                      +---------------------+     +---------------------+
//                      |                                                 |
//      +-------+-------+----------+-----+                       +--------+---------+
//      | min: 0, max: 11 (exclusive)    |                       | min: 9, max: 21  |
//      | [0,1] | [3,5] | [4,10] | [6,8] |                       | [9,17] | [15,20] |
//      +-------+-------+----------+-----+                       +--------+---------+
//
// The new row in each node is the minimum start offset and the exclusive end offset (inclusive) for
// that nodes subtree.

struct VmMappingObserver {
  struct State {
    // Inclusive smallest object offset.
    uint64_t min_offset;
    // Exclusive largest object offset.
    uint64_t max_offset;

    bool operator==(const State& other) const {
      return min_offset == other.min_offset && max_offset == other.max_offset;
    }
    bool operator!=(const State& other) const { return !(*this == other); }
  };

  using AugmentedState = State;

  // Implementation of BTree Observer::Calculate. Find the min_offset and max_offset for the
  // provided iterator range.
  template <typename iterator>
  static State Calculate(iterator node_start, iterator node_end) {
    uint64_t min_offset = (*node_start).second->object_offset();
    uint64_t max_last = (*node_end).second->object_offset() + (*node_end).second->size();
    for (; node_start != node_end; node_start++) {
      max_last =
          ktl::max(max_last, (*node_start).second->object_offset() + (*node_start).second->size());
    }
    return State{.min_offset = min_offset, .max_offset = max_last};
  }

  // Implementation of BTree Observer::Fold. Determines the min_offset and max_offset of two
  // adjacent subtrees based on their provided State.
  static State Fold(State left, State right) {
    DEBUG_ASSERT(left.min_offset <= right.min_offset);
    return State{.min_offset = left.min_offset,
                 .max_offset = ktl::max(left.max_offset, right.max_offset)};
  }
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_MAPPING_OBSERVER_H_
