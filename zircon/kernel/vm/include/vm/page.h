// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_H_

#include <lib/zircon-internal/macros.h>
#include <stdint.h>
#include <sys/types.h>
#include <zircon/compiler.h>

#include <fbl/intrusive_double_list.h>
#include <kernel/percpu.h>
#include <ktl/atomic.h>
#include <ktl/optional.h>
#include <ktl/type_traits.h>
#include <vm/page_state.h>
#include <vm/phys/arena.h>

class FreeLoanedPagesHolder;

// core per page structure allocated at pmm arena creation time
struct vm_page {
  using NodeState =
      fbl::DoublyLinkedListNodeState<vm_page*, fbl::NodeOptions::AllowRemoveFromContainer>;
  NodeState queue_node;

  // read-only after being set up
  paddr_t paddr_priv = {};  // use paddr() accessor

  // offset 0x18

  union {
    struct {
      // Pages in the free state have no extra state right now. This branch of the union is declared
      // in order to mark it as the default constructed one to match the default initial state.
    } __PACKED free = {};
    struct {
      // This is a back pointer to the vm object this page is currently contained in.  It is
      // implicitly valid when the page is in a VmCowPages (which is a superset of intervals
      // during which the page is in a page queue), and nullptr (or logically nullptr) otherwise.
      // This should not be modified (except under the page queue lock) whilst a page is in a
      // VmCowPages.
      // Field should be modified by the setters and getters to allow for future encoding changes.
      void* object_priv;

      void* get_object() const { return object_priv; }
      void set_object(void* obj) { object_priv = obj; }
      // offset 0x20

      // When object_or_event_priv is pointing to a VmCowPages, this is the offset in the VmCowPages
      // that contains this page.
      //
      // Else this field is 0.
      //
      // Field should be modified by the setters and getters to allow for future encoding changes.
      uint64_t page_offset_priv;

      uint64_t get_page_offset() const { return page_offset_priv; }

      void set_page_offset(uint64_t page_offset) { page_offset_priv = page_offset; }

      // offset 0x28

      // Identifies how many objects can access this page when it doesn't have a unique owner. A
      // page with a unique owner may still have multiple objects able to access it, just the count
      // is not tracked.
      uint32_t share_count;

      // offset 0x2c

      // Identifies which queue this page is in.
      uint8_t page_queue_priv;

      ktl::atomic_ref<uint8_t> get_page_queue_ref() {
        return ktl::atomic_ref<uint8_t>(page_queue_priv);
      }

      // TODO(https://fxbug.dev/355287217): Remove const_cast when libcxx `atomic_ref<T>` is fixed.
      ktl::atomic_ref<uint8_t> get_page_queue_ref() const {
        return ktl::atomic_ref<uint8_t>(*const_cast<uint8_t*>(&page_queue_priv));
      }

      // offset 0x2d

#define VM_PAGE_OBJECT_PIN_COUNT_BITS 5
#define VM_PAGE_OBJECT_MAX_PIN_COUNT ((1ul << VM_PAGE_OBJECT_PIN_COUNT_BITS) - 1)
      uint8_t pin_count : VM_PAGE_OBJECT_PIN_COUNT_BITS;

      // Hint for whether the page is always needed and should not be considered for reclamation
      // under memory pressure (unless the kernel decides to override hints for some reason).
      uint8_t always_need : 1;

#define VM_PAGE_OBJECT_DIRTY_STATE_BITS 2
#define VM_PAGE_OBJECT_MAX_DIRTY_STATES ((1u << VM_PAGE_OBJECT_DIRTY_STATE_BITS))
#define VM_PAGE_OBJECT_DIRTY_STATES_MASK (VM_PAGE_OBJECT_MAX_DIRTY_STATES - 1)
      // Tracks state used to determine whether the page is dirty and its contents need to written
      // back to the page source at some point, and when it has been cleaned. Used for pages backed
      // by a user pager. The three states supported are Clean, Dirty, and AwaitingClean (more
      // details in VmCowPages::DirtyState).
      uint8_t dirty_state : VM_PAGE_OBJECT_DIRTY_STATE_BITS;

      // This struct has no type name and exists inside an unpacked parent and so it really doesn't
      // need to have any padding. By making it packed we allow the next outer variables, to use
      // space we would have otherwise wasted in padding, without breaking alignment rules.
    } __PACKED object;  // attached to a vm object
    struct {
      // Tracks user-provided metadata for the item in each of the possible buckets.
      union {
        struct {
          uint32_t left_metadata;
          uint32_t mid_metadata;
          uint32_t right_metadata;

          // Used by the VmTriPageStorage allocator to record the size of the item in each of the
          // possible buckets. See it for more details.
          uint16_t left_compress_size;
          uint16_t mid_compress_size;
          uint16_t right_compress_size;
        } __PACKED;
        struct {
          // Used by the VmSlotPageStorage allocator to record free block information in the page.
          // See it for more details.
          uint64_t free_block_mask;
        } __PACKED;
      } __PACKED;
    } __PACKED zram;
    struct {
      // Optionally used by mmu code to count the number of valid mappings in this page if it is a
      // page table.
      uint num_mappings;
    } __PACKED mmu;
    struct {
      // Loaned pages maintain an optional backlink while in the alloc state to their holder object.
      FreeLoanedPagesHolder* owner;
    } __PACKED alloc;
    struct {
      uint32_t id;
      uint32_t free_slot;
      uint32_t peak_allocated;
      uint32_t allocated;
      uint32_t profile_cookie;
    } __PACKED slab;
  };
  using object_t = decltype(object);

  // offset 0x2e

  // logically private; use |state()| and |set_state()|
  vm_page_state state_priv = vm_page_state::FREE;

  // offset 0x2f

  // logically private, use loaned getters and setters below.
  static constexpr uint8_t kLoanedStateIsLoaned = 1;
  static constexpr uint8_t kLoanedStateIsLoanCancelled = 2;
  // The loaned state is packed into a single byte here to reduce memory usage, but due to the
  // allowable access patterns this means the getters and setters must perform atomic loads and
  // stores to prevent UB.
  uint8_t loaned_state_priv = {};

  // helper routines

  // Returns whether this page is in the FREE state. When in the FREE state the page is assumed to
  // be owned by the relevant PmmNode, and hence unless its lock is held this query must be assumed
  // to be racy.
  bool is_free() const { return state() == vm_page_state::FREE; }

  // Returns whether this page is in the FREE_LOANED state. Similar to the FREE state the page is
  // assumed to be owned by the relevant PmmNode, however this distinguishes whether the page is
  // part of the general purpose free list, versus the more narrowly usable set of loaned pages.
  bool is_free_loaned() const { return state() == vm_page_state::FREE_LOANED; }

  // If true, this page is "loaned" in the sense of being loaned from a contiguous VMO (via
  // decommit) to Zircon.  If the original contiguous VMO is deleted, this page will no longer be
  // loaned.  A loaned page cannot be pinned.  Instead a different physical page (non-loaned) is
  // used for the pin.  A loaned page can be (re-)committed back into its original contiguous VMO,
  // which causes the data in the loaned page to be moved into a different physical page (which
  // itself can be non-loaned or loaned).  A loaned page cannot be used to allocate a new contiguous
  // VMO.
  // Maybe queried by anyone who either owns the page, or has sufficient knowledge that the loaned
  // state cannot be being altered in parallel.
  bool is_loaned() const {
    // TODO(https://fxbug.dev/355287217): Remove const_cast when libcxx `atomic_ref<T>` is fixed.
    return !!(ktl::atomic_ref<uint8_t>(*const_cast<uint8_t*>(&loaned_state_priv))
                  .load(ktl::memory_order_relaxed) &
              kLoanedStateIsLoaned);
  }
  // If true, the original contiguous VMO wants the page back.  Such pages won't be re-used until
  // the page is no longer loaned, either via commit of the page back into the contiguous VMO that
  // loaned the page, or via deletion of the contiguous VMO that loaned the page.  Such pages are
  // not in the free_loaned_list_ in pmm, which is how re-use is prevented.
  // Should only be called by the PmmNode under its lock.
  bool is_loan_cancelled() const {
    return !!(ktl::atomic_ref<uint8_t>(*const_cast<uint8_t*>(&loaned_state_priv))
                  .load(ktl::memory_order_relaxed) &
              kLoanedStateIsLoanCancelled);
  }
  // Manipulation of 'loaned' should only be done by the PmmNode under the loaned pages lock whilst
  // it is the owner of the page.
  void set_is_loaned() {
    ktl::atomic_ref<uint8_t>(loaned_state_priv)
        .fetch_or(kLoanedStateIsLoaned, ktl::memory_order_relaxed);
  }
  void clear_is_loaned() {
    ktl::atomic_ref<uint8_t>(loaned_state_priv)
        .fetch_and(~kLoanedStateIsLoaned, ktl::memory_order_relaxed);
  }

  // Manipulation of 'loan_cancelled' should only be done by the PmmNode under its lock, but may be
  // done when the PmmNode is not the owner of the page.
  void set_is_loan_cancelled() {
    ktl::atomic_ref<uint8_t>(loaned_state_priv)
        .fetch_or(kLoanedStateIsLoanCancelled, ktl::memory_order_relaxed);
  }
  void clear_is_loan_cancelled() {
    ktl::atomic_ref<uint8_t>(loaned_state_priv)
        .fetch_and(~kLoanedStateIsLoanCancelled, ktl::memory_order_relaxed);
  }

  void dump() const;

  // return the physical address
  // future plan to store in a compressed form
  paddr_t paddr() const { return paddr_priv; }

  vm_page_state state() const {
    // TODO(https://fxbug.dev/355287217): Remove const_cast when libcxx `atomic_ref<T>` is fixed.
    return ktl::atomic_ref<vm_page_state>(*const_cast<vm_page_state*>(&state_priv))
        .load(ktl::memory_order_relaxed);
  }

  void set_state(vm_page_state new_state) {
    const vm_page_state old_state = state();
    ktl::atomic_ref<vm_page_state>(state_priv).store(new_state, ktl::memory_order_relaxed);

    // See comment at percpu::vm_page_counts
    auto& p = percpu::GetCurrent();
    p.vm_page_counts.by_state[VmPageStateIndex(old_state)] -= 1;
    p.vm_page_counts.by_state[VmPageStateIndex(new_state)] += 1;
  }

  // Return the approximate number of pages in state |state|.
  //
  // When called concurrently with |set_state|, the count may be off by a small amount.
  static uint64_t get_count(vm_page_state state);

  // Add |n| to the count of pages in state |state|.
  //
  // Should be used when first constructing pages.
  static void add_to_initial_count(vm_page_state state, uint64_t n);

 private:
  static constexpr uintptr_t kObjectOrStackOwnerIsStackOwnerFlag = 0x1;
  static constexpr uintptr_t kObjectOrStackOwnerHasWaiter = 0x2;
  static constexpr uintptr_t kObjectOrStackOwnerFlags = 0x3;
};

// Provide a type alias using modern syntax to avoid clang-tidy warnings.
using vm_page_t = vm_page;

struct vm_page_node_traits {
  static vm_page_t::NodeState& node_state(vm_page& obj) { return obj.queue_node; }
};

using VmPageDoublyLinkedList = fbl::DoublyLinkedList<vm_page_t*, fbl::DefaultObjectTag,
                                                     fbl::SizeOrder::N, vm_page_node_traits>;

// assert expected offsets (the offsets in comments above) and natural alignments
static_assert(offsetof(vm_page_t, queue_node) == 0x0);
static_assert(offsetof(vm_page_t, queue_node) % alignof(decltype(vm_page_t::queue_node)) == 0);
static_assert(offsetof(vm_page_t, queue_node) % alignof(vm_page_t::NodeState) == 0);

static_assert(offsetof(vm_page_t, paddr_priv) == 0x10);
static_assert(offsetof(vm_page_t, paddr_priv) % alignof(decltype(vm_page_t::paddr_priv)) == 0);
static_assert(offsetof(vm_page_t, paddr_priv) % alignof(paddr_t) == 0);

static_assert(offsetof(vm_page_t, object.object_priv) == 0x18);
static_assert(offsetof(vm_page_t, object.object_priv) %
                  alignof(decltype(vm_page_t::object_t::object_priv)) ==
              0);
static_assert(offsetof(vm_page_t, object.object_priv) % alignof(uintptr_t) == 0);

static_assert(offsetof(vm_page_t, object.page_offset_priv) == 0x20);
static_assert(offsetof(vm_page_t, object.page_offset_priv) %
                  alignof(decltype(vm_page_t::object_t::page_offset_priv)) ==
              0);
static_assert(offsetof(vm_page_t, object.page_offset_priv) % alignof(uint64_t) == 0);

static_assert(offsetof(vm_page_t, object.share_count) == 0x28);
static_assert(offsetof(vm_page_t, object.share_count) %
                  alignof(decltype(vm_page_t::object_t::share_count)) ==
              0);
static_assert(offsetof(vm_page_t, object.share_count) % alignof(uint32_t) == 0);

static_assert(offsetof(vm_page_t, object.page_queue_priv) == 0x2c);
static_assert(offsetof(vm_page_t, object.page_queue_priv) %
                  alignof(decltype(vm_page_t::object_t::page_queue_priv)) ==
              0);
static_assert(offsetof(vm_page_t, object.page_queue_priv) % alignof(uint8_t) == 0);

static_assert(offsetof(vm_page_t, state_priv) == 0x2e);
static_assert(offsetof(vm_page_t, state_priv) % alignof(decltype(vm_page_t::state_priv)) == 0);
static_assert(offsetof(vm_page_t, state_priv) % alignof(vm_page_state) == 0);

static_assert(offsetof(vm_page_t, loaned_state_priv) == 0x2f);
static_assert(offsetof(vm_page_t, loaned_state_priv) %
                  alignof(decltype(vm_page_t::loaned_state_priv)) ==
              0);
static_assert(offsetof(vm_page_t, loaned_state_priv) % alignof(vm_page_state) == 0);

// Assert that the page structure isn't growing uncontrollably, and that its
// size and alignment are kept in sync with the arena selection algorithm.
static_assert(sizeof(vm_page_t) == kArenaPageBookkeepingSize);
static_assert(alignof(vm_page_t) == kArenaPageBookkeepingAlignment);

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_H_
