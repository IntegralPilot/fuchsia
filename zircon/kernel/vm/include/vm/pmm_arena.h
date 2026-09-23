// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_ARENA_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_ARENA_H_

#include <lib/zx/result.h>
#include <trace.h>
#include <zircon/types.h>

#include <fbl/macros.h>
#include <ktl/array.h>
#include <vm/page.h>
#include <vm/phys/arena.h>

#define PMM_ARENA_FLAG_LO_MEM \
  (0x1)  // this arena is contained within architecturally-defined 'low memory'

typedef struct pmm_arena_info {
  char name[16];

  uint flags;

  paddr_t base;
  size_t size;
} pmm_arena_info_t;

using PmmStateCount = ktl::array<size_t, static_cast<size_t>(vm_page_state::COUNT_)>;
void PrintPageStateCounts(const PmmStateCount& state_count);

class PmmNode;

class PmmArena {
 public:
  constexpr PmmArena() = default;
  ~PmmArena() = default;

  DISALLOW_COPY_ASSIGN_AND_MOVE(PmmArena);

  // initialize the arena and allocate memory for internal data structures
  void Init(const PmmArenaSelection& selected, PmmNode* node);

  void InitForTest(const pmm_arena_info_t& info, vm_page_t* page_array);

  // accessors
  const pmm_arena_info_t& info() const { return info_; }
  const char* name() const { return info_.name; }
  paddr_t base() const { return info_.base; }
  size_t size() const { return info_.size; }
  paddr_t end() const { return base() + size(); }
  unsigned int flags() const { return info_.flags; }

  // Counts the number of pages in every state. For each page in the arena,
  // increments the corresponding vm_page_state::*-indexed entry of
  // |state_count|. Does not zero out the entries first.
  void CountStates(PmmStateCount* state_count) const;

  vm_page_t* get_page(size_t index) { return &page_array_[index]; }
  size_t get_index(const vm_page_t* page) { return page - page_array_; }

  // find a free run of contiguous pages
  vm_page_t* FindFreeContiguous(size_t count, uint8_t alignment_log2);

  // return a pointer to a specific page
  vm_page_t* FindSpecific(paddr_t pa);

  // helpers
  bool page_belongs_to_arena(const vm_page* page) const {
    return (page->paddr() >= base() && page->paddr() < (base() + size()));
  }

  bool address_in_arena(paddr_t address) const {
    return (address >= info_.base && address <= info_.base + info_.size - 1);
  }

  void Dump(bool dump_pages, bool dump_free_ranges, PmmStateCount* counts) const;

 private:
  pmm_arena_info_t info_ = {};
  vm_page_t* page_array_ = nullptr;
  // Marked maybe_unused since the logic of using this is in Rust, but we need to keep the
  // definition of data structures equivalent.
  [[maybe_unused]] uint64_t search_hint_ = 0;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_ARENA_H_
