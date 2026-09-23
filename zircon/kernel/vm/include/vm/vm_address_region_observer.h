// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_OBSERVER_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_OBSERVER_H_

#include <assert.h>
#include <lib/page/size.h>
#include <stdint.h>
#include <zircon/types.h>

#include <ktl/algorithm.h>
#include <ktl/optional.h>

// # VmAddressRegion Augmented B-Tree Support
//
// VmAddressRegion maintains an ordered set of non-overlapping subregions in a RegionList,
// which is implemented as a B-Tree. To efficiently find gaps for new allocations,
// the tree is augmented with subtree metadata:
// 1. min_addr: The lowest address in the subtree.
// 2. max_addr: The highest (inclusive) address in the subtree.
// 3. max_gap: The largest unallocated gap between any two adjacent regions in the subtree.
//
// ## Calculation Logic
//
// The augmentation state is maintained such that each entry in the B-Tree represents the aggregated
// state of the subtree rooted at that entry.
//
// ### Leaf Nodes
// For a leaf node containing a sequence of regions [R0, R1, ..., Rn]:
// - min_addr = R0.base
// - max_addr = Rn.end - 1
// - max_gap  = max(R1.base - R0.end, R2.base - R1.end, ..., Rn.base - R(n-1).end)
//
// ### Intermediate Nodes
// For an intermediate node with entries [E0, E1, ..., Em], where each entry represents a child
// subtree:
// - min_addr = E0.min_addr
// - max_addr = Em.max_addr
// - max_gap  = max(
//     E0.max_gap, E1.max_gap, ..., Em.max_gap,         // Gaps within child subtrees
//     E1.min_addr - E0.max_addr - 1,                   // Gaps between child subtrees
//     E2.min_addr - E1.max_addr - 1,
//     ...
//   )
//
// ## Example B-Tree Node Diagram
//
// The following diagram illustrates an augmented B-Tree node with four entries (E0, E1, E2, E3).
// Each entry is a subtree with its own min_addr, max_addr, and max_gap values.
//
//                                Node State
//           +-------------------------------------------------------+
//           | min_addr: 0, max_addr: 59, max_gap: 15                |
//           +-------------------------------------------------------+
//                 |                  |                    |
//    _____________|        __________|__________          |____________
//    |                     |                   |                      |
//    V                     V                    V                     V
// +-------------+       +-------------+      +-------------+       +-------------+
// | Subtree E0  | (Gap) | Subtree E1  | (Gap)| Subtree E2  | (Gap) | Subtree E3  |
// | [0,  9]     |  10   | [20, 29]    |  5   | [35, 39]    |  15   | [55, 59]    |
// | max_gap: 5  |       | max_gap: 2  |      | max_gap: 8  |       | max_gap: 3  |
// +-------------+       +-------------+      +-------------+       +-------------+
//
// Calculation:
// - min_addr = E0.min (0)
// - max_addr = E3.max (59)
// - max_gap  = max(E0.max_gap(5), Gap(E0,E1)(10), E1.max_gap(2), Gap(E1,E2)(5),
//                  E2.max_gap(8), Gap(E2,E3)(15), E3.max_gap(3))
//            = max(5, 10, 2, 5, 8, 15, 3) = 15
//

struct VmAddressRegionObserver {
  struct State {
    // We store the min_addr, max_addr and max_gap in 16 bytes.
    // Since addresses and sizes are always multiples of kPageSize (and thus
    // page-aligned), we can pack the max_gap (measured in pages) into the
    // lower bits of the first uint64_t, which otherwise stores the page-aligned
    // min_addr.
    uint64_t max_gap_pages : kPageShift;
    uint64_t min_addr_page : (64 - kPageShift);
    // max_addr is the inclusive top byte of the range, and is therefore not page aligned.
    uint64_t max_addr;

    // Due to the limited bits from the page alignment there is a maximum size of gap we can store.
    // The largest gap, kMaxGapPages, therefore becomes a sentinel value representing infinity. A
    // consequence is that when performing a search a subtree with a gap of kMaxGapPages must always
    // be descended into. Pragmatically the main optimization of tracking max gaps is for when
    // entropy is at or near 0, and we are attempting to skip runs of adjacent mappings, i.e. where
    // max_gap is zero.
    static constexpr uint64_t kMaxGapPages = (1UL << kPageShift) - 1;

    vaddr_t min_addr() const { return static_cast<vaddr_t>(min_addr_page) << kPageShift; }
    void set_min_addr(vaddr_t addr) {
      DEBUG_ASSERT(IsPageRounded(addr));
// GCC is unable to understand that a value shifted by a certain number of bits will always
// fit into a bitfield reduced by that many bits, so just disable the warning for it.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wconversion"
      min_addr_page = addr >> kPageShift;
#pragma GCC diagnostic pop
    }

    ktl::optional<size_t> max_gap() const {
      if (max_gap_pages != kMaxGapPages) {
        return static_cast<size_t>(max_gap_pages) << kPageShift;
      }
      return ktl::nullopt;
    }
    void set_max_gap(size_t gap) {
// GCC is unable to understand that we will never store a value larger than kMaxGapPages,
// which is a constexpr that fits in the number of bits in max_gap_pages by definition and so
// we disable the relevant warning.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wconversion"
      max_gap_pages = ktl::min(gap >> kPageShift, kMaxGapPages);
#pragma GCC diagnostic pop
    }

    bool operator==(const State& other) const {
      return max_gap_pages == other.max_gap_pages && min_addr_page == other.min_addr_page &&
             max_addr == other.max_addr;
    }
    bool operator!=(const State& other) const { return !(*this == other); }
  };

  using AugmentedState = State;

  // Implementation of BTree Observer::Calculate. Find the min_addr, max_addr and max_gap for the
  // provided iterator range.
  template <typename iterator>
  static State Calculate(iterator node_start, iterator node_end) {
    State state = {};
    uint64_t max_gap = 0;

    state.set_min_addr((*node_start).second->base());

    auto it = node_start;
    auto prev = it;
    auto endd = node_end;
    endd++;
    for (it++; it != endd; it++) {
      vaddr_t prev_top = (*prev).second->base() + ((*prev).second->size() - 1);
      // Regions can temporarily overlap, so only consider if there is actually a gap.
      if (prev_top < (*it).second->base()) {
        uint64_t gap = ((*it).second->base() - prev_top) - 1;
        max_gap = ktl::max(max_gap, gap);
      }
      prev = it;
    }

    state.max_addr = (*node_end).second->base() + ((*node_end).second->size() - 1);
    state.set_max_gap(max_gap);
    return state;
  }

  // Implementation of BTree Observer::Fold. Determines the min_addr, max_addr and max_gap of two
  // adjacent subtrees based on their provided State.
  static State Fold(State left, State right) {
    ASSERT(left.min_addr() <= right.min_addr());
    ASSERT(left.max_addr <= right.max_addr);

    State state = {};
    state.set_min_addr(left.min_addr());
    state.max_addr = right.max_addr;

    uint64_t max_gap = ktl::max(left.max_gap_pages, right.max_gap_pages) << kPageShift;
    uint64_t inter_gap = 0;
    // Regions can temporarily overlap, so only consider if there is actually a gap.
    if (left.max_addr < right.min_addr()) {
      inter_gap = (right.min_addr() - left.max_addr) - 1;
    }
    max_gap = ktl::max(max_gap, inter_gap);

    state.set_max_gap(max_gap);
    return state;
  }
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_OBSERVER_H_
