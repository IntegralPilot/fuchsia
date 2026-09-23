// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_H_

#include <assert.h>
#include <lib/btree.h>
#include <lib/crypto/prng.h>
#include <lib/fit/function.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/types.h>

#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_wavl_tree.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <ffl/saturating_arithmetic.h>
#include <ktl/limits.h>
#include <ktl/optional.h>
#include <vm/vm_address_region_observer.h>
#include <vm/vm_aspace.h>
#include <vm/vm_cow_pages.h>
#include <vm/vm_object.h>
#include <vm/vm_page_list.h>

// Creation flags for VmAddressRegion and VmMappings

// When randomly allocating subregions, reduce sprawl by placing allocations
// near each other.
#define VMAR_FLAG_COMPACT (1 << 0)
// Request that the new region be at the specified offset in its parent region.
#define VMAR_FLAG_SPECIFIC (1 << 1)
// Like VMAR_FLAG_SPECIFIC, but permits overwriting existing mappings.  This
// flag will not overwrite through a subregion.
#define VMAR_FLAG_SPECIFIC_OVERWRITE (1 << 2)
// Allow VmMappings to be created inside the new region with the SPECIFIC or
// OFFSET_IS_UPPER_LIMIT flag.
#define VMAR_FLAG_CAN_MAP_SPECIFIC (1 << 3)
// When on a VmAddressRegion, allow VmMappings to be created inside the region
// with read permissions.  When on a VmMapping, controls whether or not the
// mapping can gain this permission.
#define VMAR_FLAG_CAN_MAP_READ (1 << 4)
// When on a VmAddressRegion, allow VmMappings to be created inside the region
// with write permissions.  When on a VmMapping, controls whether or not the
// mapping can gain this permission.
#define VMAR_FLAG_CAN_MAP_WRITE (1 << 5)
// When on a VmAddressRegion, allow VmMappings to be created inside the region
// with execute permissions.  When on a VmMapping, controls whether or not the
// mapping can gain this permission.
#define VMAR_FLAG_CAN_MAP_EXECUTE (1 << 6)
// Require that VMO backing the mapping is non-resizable.
#define VMAR_FLAG_REQUIRE_NON_RESIZABLE (1 << 7)
// Allow VMO backings that could result in faults.
#define VMAR_FLAG_ALLOW_FAULTS (1 << 8)
// Treat the offset as an upper limit when allocating a VMO or child VMAR.
#define VMAR_FLAG_OFFSET_IS_UPPER_LIMIT (1 << 9)
// Opt this VMAR out of certain debugging checks. This allows for kernel mappings that have a more
// dynamic management strategy, that the regular checks would otherwise spuriously trip on.
#define VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING (1 << 10)
// Memory accesses past the stream size rounded up to the page boundary will fault.
#define VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE (1 << 11)

#define VMAR_CAN_RWX_FLAGS \
  (VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE)

enum class VmAddressRegionOpChildren : bool {
  Yes,
  No,
};

// forward declarations
class VmAddressRegion;
class VmMapping;
class VmEnumerator;
enum class VmAddressRegionEnumeratorType : bool;
template <VmAddressRegionEnumeratorType, typename>
class VmAddressRegionEnumerator;

class MultiPageRequest;

// A VmAddressRegion represents a contiguous region of the virtual address
// space.  It is partitioned by non-overlapping children of the following types:
// 1) child VmAddressRegion
// 2) child VmMapping (leafs that map VmObjects into the address space)
// 3) gaps (logical, not actually objects).
//
// VmAddressRegionOrMapping represents a tagged union of the two types.
//
// A VmAddressRegion/VmMapping may be in one of two states: ALIVE or DEAD.  If
// it is ALIVE, then the VmAddressRegion is a description of the virtual memory
// mappings of the address range it represents in its parent VmAspace.  If it is
// DEAD, then the VmAddressRegion is invalid and has no meaning.
//
// All VmAddressRegion and VmMapping state is protected by the aspace lock.
class VmAddressRegionOrMapping : public fbl::RefCounted<VmAddressRegionOrMapping> {
 public:
  // If a VMO-mapping, unmap all pages and remove dependency on vm object it has a ref to.
  // Otherwise recursively destroy child VMARs and transition to the DEAD state.
  //
  // Returns ZX_OK on success, ZX_ERR_BAD_STATE if already dead, and other
  // values on error (typically unmap failure).
  virtual zx_status_t Destroy();

  // accessors
  vaddr_t base() const { return base_; }
  size_t size() const { return size_; }
  uint32_t flags() const { return flags_; }
  const fbl::RefPtr<VmAspace>& aspace() const { return aspace_; }

  // Subtype information and safe down-casting
  bool is_mapping() const { return is_mapping_; }
  fbl::RefPtr<VmAddressRegion> as_vm_address_region();
  fbl::RefPtr<VmMapping> as_vm_mapping();
  VmAddressRegion* as_vm_address_region_ptr();
  const VmAddressRegion* as_vm_address_region_ptr() const;
  VmMapping* as_vm_mapping_ptr();
  const VmMapping* as_vm_mapping_ptr() const;
  static fbl::RefPtr<VmAddressRegion> downcast_as_vm_address_region(
      fbl::RefPtr<VmAddressRegionOrMapping>* region_or_map);
  static fbl::RefPtr<VmMapping> downcast_as_vm_mapping(
      fbl::RefPtr<VmAddressRegionOrMapping>* region_or_map);

  // Dump debug info
  virtual void DumpLocked(uint depth, bool verbose) const TA_REQ(lock()) = 0;

  // Expose our backing lock for annotation purposes.
  Lock<CriticalMutex>* lock() const TA_RET_CAP(aspace_->lock()) { return aspace_->lock(); }
  Lock<CriticalMutex>& lock_ref() const TA_RET_CAP(aspace_->lock()) { return aspace_->lock_ref(); }
  Lock<CriticalMutex>* region_lock() const TA_RET_CAP(aspace_->region_lock_) {
    return aspace_->region_lock();
  }
  Lock<CriticalMutex>& region_lock_ref() const TA_RET_CAP(aspace_->region_lock_) {
    return aspace_->region_lock_ref();
  }

  bool is_in_range(vaddr_t base, size_t size) const {
    const size_t offset = base - base_;
    return base >= base_ && offset < size_ && size_ - offset >= size;
  }

  // Memory priorities that can be applied to VMARs and mappings to propagate to VMOs and page
  // tables.
  enum class MemoryPriority : bool {
    // Default overcommit priority where reclamation is allowed.
    DEFAULT,
    // High priority prevents all reclamation.
    HIGH,
  };

  // Returns true if the instance is alive and reporting information that
  // reflects the address space layout. |aspace()->lock()| must be held.
  bool IsAliveLocked() const TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    canary_.Assert();
    return state_ == LifeCycleState::ALIVE;
  }

 private:
  fbl::Canary<fbl::magic("VMRM")> canary_;
  const bool is_mapping_;

 protected:
  // friend VmAddressRegion so it can access DestroyLocked
  friend VmAddressRegion;
  template <VmAddressRegionEnumeratorType, typename>
  friend class VmAddressRegionEnumerator;

  // destructor, should only be invoked from RefPtr
  virtual ~VmAddressRegionOrMapping();
  friend fbl::RefPtr<VmAddressRegionOrMapping>;

  enum class LifeCycleState : uint8_t {
    // Initial state: if NOT_READY, then do not invoke Destroy() in the
    // destructor
    NOT_READY,
    // Usual state: information is representative of the address space layout
    ALIVE,
    // Object is invalid
    DEAD
  };

  LifeCycleState state_locked() const TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS { return state_; }
  LifeCycleState state_locked_region() const TA_REQ(region_lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return state_;
  }

  VmAddressRegionOrMapping(vaddr_t base, size_t size, uint32_t flags, VmAspace* aspace,
                           VmAddressRegion* parent, bool is_mapping);

  // Check if the given *arch_mmu_flags* are allowed under this
  // regions *flags_*
  bool is_valid_mapping_flags(arch_mmu_flags_t arch_mmu_flags) {
    // Work out what flags we must support for these arch_mmu_flags
    uint32_t needed = 0;
    if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_READ) {
      needed |= VMAR_FLAG_CAN_MAP_READ;
    }
    if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) {
      needed |= VMAR_FLAG_CAN_MAP_WRITE;
    }
    if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) {
      needed |= VMAR_FLAG_CAN_MAP_EXECUTE;
    }
    // Mask out the actual relevant mappings flags we have.
    const uint32_t actual =
        flags_ & (VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE);
    // Validate that every |needed| occurs in |actual|
    return (needed & actual) == needed;
  }

  virtual zx_status_t DestroyLocked() TA_REQ(lock()) TA_REQ(region_lock()) = 0;

  // Performs any actions necessary to apply a high memory priority over the given range.
  // This method is always safe to call as it will internally check the memory priority status and
  // skip if necessary, so the caller does not need to worry about races with a different memory
  // priority being applied.
  // As this may need to acquire the lock even to check the memory priority, if the caller knows
  // they have not caused this to become high priority (i.e. they have called
  // SetMemoryPriorityLocked with MemoryPriority::DEFAULT), then calling this should be skipped for
  // performance.
  // Memory that needs to be committed for a high memory priority are user pager backed pages and
  // any compressed or loaned pages. Anonymous pages and copy-on-write pages do not allocated /
  // committed.
  // This method has no return value as it is entirely best effort and no part of its operation is
  // needed for correctness.
  virtual void CommitHighMemoryPriority() TA_EXCL(lock()) = 0;

  // Transition from NOT_READY to READY, and add references to self to related
  // structures.
  // On error no state is changed and no references are added.
  virtual zx_status_t Activate() TA_REQ(region_lock()) TA_REQ(lock()) = 0;

  // current state of the VMAR.  If LifeCycleState::DEAD, then all other
  // fields are invalid.
  LifeCycleState state_ TA_GUARDED(region_lock()) TA_GUARDED(lock()) = LifeCycleState::ALIVE;

  // Priority of the VMAR. This starts at DEFAULT and must be reset back to default as part of the
  // destroy path to ensure any propagation is undone correctly.
  MemoryPriority memory_priority_ TA_GUARDED(lock()) = MemoryPriority::DEFAULT;

  // flags from VMAR creation time
  const uint32_t flags_;

  // address/size within the container address space
  const vaddr_t base_;
  const size_t size_;

  // pointer back to our member address space.  The aspace's lock is used
  // to serialize all modifications.
  const fbl::RefPtr<VmAspace> aspace_;

  // pointer back to our parent region (nullptr if root or destroyed)
  VmAddressRegion* parent_ TA_GUARDED(lock());
};

// A list of regions ordered by virtual address. Templated to allow for test code to avoid needing
// to instantiate 'real' VmAddressRegionOrMapping instances.
// TODO(https://fxbug.dev/503042881): The RegionList API is quite object reference focused, instead
// of iterator focused, and this leads to a lot of theoretically redundant 'find' operations on the
// btree. These APIs, and the VMAR logic using them, should be re-designed to be more iterator
// based.
template <typename T = VmAddressRegionOrMapping>
class RegionList final {
 public:
  using PtrType = fbl::RefPtr<T>;
  using ChildList = btree::BTree<vaddr_t, PtrType, VmAddressRegionObserver>;

  RegionList() = default;

  // Remove *region* from the list, returns the removed region.
  void RemoveRegion(T* region) {
    auto it = regions_.find(region->base());
    ASSERT(it.IsValid());
    regions_.erase(it);
  }

  // Request the region to the left or right of the given region.
  ChildList::iterator LeftOf(T* region) {
    auto it = regions_.find(region->base());
    DEBUG_ASSERT(it.IsValid());
    it--;
    return it;
  }
  ChildList::iterator RightOf(T* region) {
    auto it = regions_.find(region->base());
    DEBUG_ASSERT(it.IsValid());
    it++;
    return it;
  }

  // Insert *region* to the region list. On failure the region list is unmodified.
  zx_status_t InsertRegion(fbl::RefPtr<T> region) {
    const vaddr_t base = region->base();
    return regions_.insert(base, ktl::move(region)).IsValid() ? ZX_OK : ZX_ERR_NO_MEMORY;
  }

  // Replaces the target region with a new region at the same address. Unlike insertion this cannot
  // fail.
  void ReplaceRegion(T* prev, fbl::RefPtr<T> region) {
    ASSERT(prev->base() == region->base());
    auto it = regions_.find(prev->base());
    ASSERT(it.IsValid());
    regions_.update(it, ktl::move(region));
  }

  // Use a static template to allow for returning a const and non-const pointer depending on the
  // constness of self.
  template <typename S>
  static ktl::conditional_t<ktl::is_const_v<S>, const T, T>* FindRegion(S* self, vaddr_t addr) {
    // Find the first region with a base greater than *addr*.  If a region
    // exists for *addr*, it will be immediately before it.
    auto itr = --self->regions_.upper_bound(addr);
    if (!itr.IsValid()) {
      return nullptr;
    }
    auto region = (*itr).second;
    // Subregion size should never be zero unless during unmapping which should never overlap with
    // this operation.
    DEBUG_ASSERT(region->size() > 0);
    vaddr_t region_end;
    bool overflowed = add_overflow(region->base(), region->size() - 1, &region_end);
    ASSERT(!overflowed);
    if (region->base() > addr || addr > region_end) {
      return nullptr;
    }

    return region;
  }

  // Find the region that covers addr, returns nullptr if not found.
  const T* FindRegion(vaddr_t addr) const { return FindRegion(this, addr); }
  T* FindRegion(vaddr_t addr) { return FindRegion(this, addr); }

  template <typename S>
  static ktl::conditional_t<ktl::is_const_v<S>, typename ChildList::const_iterator,
                            typename ChildList::iterator>
  IncludeOrHigher(S* self, vaddr_t base) {
    // Find the first region with a base greater than *base*.  If a region
    // exists for *base*, it will be immediately before it.
    auto itr = self->regions_.upper_bound(base);
    itr--;
    if (!itr.IsValid()) {
      itr = self->regions_.begin();
    } else {
      const T* region = (*itr).second;
      if (base >= region->base() && base - region->base() >= region->size()) {
        // If *base* isn't in this region, ignore it.
        ++itr;
      }
    }
    return itr;
  }

  // Find the region that contains |base|, or if that doesn't exist, the first region that contains
  // an address greater than |base|.
  ChildList::iterator IncludeOrHigher(vaddr_t base) { return IncludeOrHigher(this, base); }
  ChildList::const_iterator IncludeOrHigher(vaddr_t base) const {
    return IncludeOrHigher(this, base);
  }

  ChildList::iterator UpperBound(vaddr_t base) { return regions_.upper_bound(base); }
  ChildList::const_iterator UpperBound(vaddr_t base) const { return regions_.upper_bound(base); }

  // Check whether it would be valid to create a child in the range [base, base+size).
  bool IsRangeAvailable(vaddr_t base, size_t size) const {
    DEBUG_ASSERT(size > 0);

    // Find the first region with base > *base*.  Since subregions_ has no
    // overlapping elements, we just need to check this one and the prior
    // child.

    auto prev = regions_.upper_bound(base);
    auto next = prev--;

    if (prev.IsValid()) {
      const T* p = (*prev).second;
      vaddr_t prev_last_byte;
      if (add_overflow(p->base(), p->size() - 1, &prev_last_byte)) {
        return false;
      }
      if (prev_last_byte >= base) {
        return false;
      }
    }

    if (next.IsValid()) {
      const T* n = (*next).second;
      vaddr_t last_byte;
      if (add_overflow(base, size - 1, &last_byte)) {
        return false;
      }
      if (n->base() <= last_byte) {
        return false;
      }
    }
    return true;
  }

  // Returns the base address of an available spot in the address range that satisfies the given
  // entropy, alignment, size, and upper limit requirements. If no spot is found that satisfies the
  // given entropy (i.e. target_index), the number of candidate spots encountered is returned.
  //
  // See vm/vm_address_region_subtree_state.h for an explanation of the augmented state used by this
  // method to perform efficient tree traversal.
  struct FindSpotAtIndexFailed {
    size_t candidate_spot_count;
  };
  fit::result<FindSpotAtIndexFailed, vaddr_t> FindSpotAtIndex(vaddr_t target_index,
                                                              uint8_t align_pow2, size_t size,
                                                              vaddr_t parent_base,
                                                              size_t parent_size,
                                                              vaddr_t upper_limit) const {
    // Returns the number of addresses that satisfy the size and alignment in the given range,
    // accounting for ranges that overlap the upper limit.
    const auto spots_in_range = [align_pow2, size, upper_limit](vaddr_t aligned_base,
                                                                size_t aligned_size) -> size_t {
      DEBUG_ASSERT(aligned_base < upper_limit);

      const size_t range_limit = ffl::SaturateAddAs<size_t>(aligned_base, aligned_size);
      const size_t clamped_range_size =
          range_limit < upper_limit ? aligned_size : aligned_size - (range_limit - upper_limit);

      if (clamped_range_size >= size) {
        return ((clamped_range_size - size) >> align_pow2) + 1;
      }
      return 0;
    };

    // Returns the given range with the base aligned and the size adjusted to maintain the same end
    // address. If the aligned base address is greater than the end address, the returned size is
    // zero.
    struct AlignedRange {
      vaddr_t base;
      size_t size;
    };
    const auto align_range = [align_pow2](vaddr_t range_base, size_t range_size) -> AlignedRange {
      const vaddr_t aligned_base = ROUNDUP(range_base, 1UL << align_pow2);
      const size_t base_delta = aligned_base - range_base;
      const size_t aligned_size = ffl::SaturateSubtractAs<size_t>(range_size, base_delta);
      return {.base = aligned_base, .size = aligned_size};
    };

    // Track the number of candidate spots encountered.
    size_t candidate_spot_count = 0;

    // As we iterate through regions we remember the end of the last allocated region as the start
    // of a potential gap.
    vaddr_t next_gap_start = parent_base;
    // Because an allocation can end at the very top of the 64-bit address space the calculation of
    // the logical start of the next gap (end + 1) could overflow. To assist with debug validation
    // we track this overflow explicitly.
    bool next_gap_overflow = false;
    // When we do find something record it here. Use UINT64_MAX, which can never be a valid base
    // address, to track the difference between the walk terminating early with success, or
    // completing with success (and hence not yet having a spot).
    vaddr_t spot = UINT64_MAX;

    // Helper to process a gap and count our spots. Returns true if a spot was found.
    auto record_gap = [&](vaddr_t gap_base, size_t gap_size) -> bool {
      const AlignedRange aligned_gap = align_range(gap_base, gap_size);
      if (aligned_gap.base >= upper_limit) {
        return false;
      }
      const size_t spot_count = spots_in_range(aligned_gap.base, aligned_gap.size);
      candidate_spot_count += spot_count;
      if (target_index < spot_count) {
        spot = aligned_gap.base + (target_index << align_pow2);
        return true;
      }
      target_index -= spot_count;
      return false;
    };

    // Lambda passed to the walker for handling an intermediate btree node.
    auto examine_subtree = [&](VmAddressRegionObserver::State state) -> zx_status_t {
      if (next_gap_start < state.min_addr()) {
        if (record_gap(next_gap_start, state.min_addr() - next_gap_start)) {
          return ZX_ERR_STOP;
        }
      }
      if (state.min_addr() >= upper_limit) {
        return ZX_ERR_OUT_OF_RANGE;
      }
      if (auto max_gap = state.max_gap(); max_gap && *max_gap < size) {
        // max_addr is inclusive, but already_visited is exclusive, so we add 1. Should this
        // overflow then that means we have reached the end of the possible address space and will
        // be handled when we check for trailing gaps at the end.
        next_gap_overflow = add_overflow(state.max_addr, 1, &next_gap_start);
        // This subtree has no gaps that would fit, so skip the entire subtree with ZX_ERR_NEXT.
        return ZX_ERR_NEXT;
      }
      // Set already_visited to the start of the subtree as we are about to descend into it and we
      // previously processed any gap up to min_addr.
      next_gap_start = state.min_addr();
      return ZX_OK;
    };

    // Lambda passed to the walker for handling leaf btree nodes.
    auto examine_leaf = [&](VmAddressRegionObserver::State state, auto first,
                            auto last) -> zx_status_t {
      if (next_gap_start < state.min_addr()) {
        if (record_gap(next_gap_start, state.min_addr() - next_gap_start)) {
          return ZX_ERR_STOP;
        }
      }
      if (state.min_addr() >= upper_limit) {
        return ZX_ERR_OUT_OF_RANGE;
      }
      // No matter what happens, we will have processed till max_addr + 1. See examine_subtree for
      // why explanation of +1 and overflow.
      next_gap_overflow = add_overflow(state.max_addr, 1, &next_gap_start);
      if (auto max_gap = state.max_gap(); max_gap && *max_gap < size) {
        // No gaps in this leaf node what would fit, can skip the iteration and go to the next node.
        return ZX_ERR_NEXT;
      }
      auto prev = first;
      // The provided iterators are inclusive, so increment last to simplify our loop.
      last++;
      for (first++; first != last; first++) {
        vaddr_t gap_start = (*prev).second->base() + (*prev).second->size();
        vaddr_t gap_end = (*first).second->base();
        if (gap_start < gap_end) {
          if (record_gap(gap_start, gap_end - gap_start)) {
            // Location found, can cease walking.
            return ZX_ERR_STOP;
          }
        }
        prev = first;
      }
      // Continue to the next node.
      return ZX_ERR_NEXT;
    };

    // Walk the btree examining the augmented state to optimally skip irrelevant subtrees.
    zx_status_t status = regions_.walk(examine_subtree, examine_leaf);
    if (status != ZX_OK && status != ZX_ERR_OUT_OF_RANGE) {
      return fit::error(FindSpotAtIndexFailed{candidate_spot_count});
    }
    // Check if we already found a spot or if we need to consider any trailing gap.
    if (spot != UINT64_MAX) {
      return fit::success{spot};
    }
    if (unlikely(next_gap_overflow)) {
      vaddr_t parent_top;
      // The next gap should only overflow if the parent region is the end of the 64-bit address
      // space. There should also, therefore, not actually be any gap.
      ASSERT(add_overflow(parent_base, parent_size, &parent_top));
      ASSERT(parent_top == next_gap_start);
    } else {
      // Any potential remaining gap has not wrapped, but the parent could still be at the end of
      // address space, so operate on its max_byte and not its top to avoid overflow.
      const vaddr_t parent_max_byte = parent_base + (parent_size - 1);
      if (next_gap_start <= parent_max_byte) {
        const size_t remaining_size = parent_max_byte - next_gap_start + 1;
        if (record_gap(next_gap_start, remaining_size)) {
          return fit::success{spot};
        }
      }
    }
    return fit::error(FindSpotAtIndexFailed{candidate_spot_count});
  }

  // Get the allocation spot that is free and large enough for the aligned size.
  zx_status_t GetAllocSpot(vaddr_t* alloc_spot, uint8_t align_pow2, uint8_t entropy, size_t size,
                           vaddr_t parent_base, size_t parent_size, crypto::Prng* prng,
                           vaddr_t upper_limit = ktl::numeric_limits<vaddr_t>::max()) const {
    DEBUG_ASSERT(entropy < sizeof(size_t) * 8);

    // The number of addresses to consider based on the configured entropy.
    const size_t max_candidate_spaces = 1ul << entropy;

    // We first pick an index in [0, max_candidate_spaces] and hope to find a spot there. If the
    // number of available spots is less than the selected index, the attempt fails, returning the
    // actual number of candidate spots found, and we try again in this smaller range.
    //
    // This is mathematically equivalent to randomly picking a spot within [0, candidate_spot_count]
    // when selected_index <= candidate_spot_count.
    //
    // Prove as following:
    // Define M = candidate_spot_count
    // Define N = max_candidate_spaces (M < N, otherwise we can randomly allocate any spot from
    // [0, max_candidate_spaces], thus allocate a specific slot has (1 / N) probability).
    // Define slot X0 where X0 belongs to [1, M].
    // Define event A: randomly pick a slot X in [1, N], N = X0.
    // Define event B: randomly pick a slot X in [1, N], N belongs to [1, M].
    // Define event C: randomly pick a slot X in [1, N], N = X0 when N belongs to [1, M].
    // P(C) = P(A | B)
    // Since when A happens, B definitely happens, so P(AB) = P(A)
    // P(C) = P(A) / P(B) = (1 / N) / (M / N) = (1 / M)
    // which is equal to the probability of picking a specific spot in [0, M].
    vaddr_t selected_index = prng != nullptr ? prng->RandInt(max_candidate_spaces) : 0;

    fit::result allocation_result =
        FindSpotAtIndex(selected_index, align_pow2, size, parent_base, parent_size, upper_limit);
    if (allocation_result.is_error()) {
      const size_t candidate_spot_count = allocation_result.error_value().candidate_spot_count;
      if (candidate_spot_count == 0) {
        return ZX_ERR_NO_RESOURCES;
      }

      // If the number of available spaces is smaller than the selected index, pick again from the
      // available range.
      DEBUG_ASSERT(candidate_spot_count < max_candidate_spaces);
      DEBUG_ASSERT(prng);
      selected_index = prng->RandInt(candidate_spot_count);
      allocation_result =
          FindSpotAtIndex(selected_index, align_pow2, size, parent_base, parent_size, upper_limit);
    }

    DEBUG_ASSERT(allocation_result.is_ok());
    *alloc_spot = allocation_result.value();
    ASSERT_MSG(IS_ROUNDED(*alloc_spot, 1UL << align_pow2), "size=%zu align_pow2=%u alloc_spot=%zx",
               size, align_pow2, *alloc_spot);
    return ZX_OK;
  }

  // Returns whether the region list is empty.
  bool IsEmpty() const { return regions_.is_empty(); }

  // Returns the first element of the list.
  T& front() {
    DEBUG_ASSERT(!IsEmpty());
    return *(*regions_.begin()).second;
  }

  ChildList::iterator begin() { return regions_.begin(); }
  ChildList::const_iterator begin() const { return regions_.begin(); }

  ChildList::iterator end() { return regions_.end(); }
  ChildList::const_iterator end() const { return regions_.end(); }

  size_t size_slow() const { return regions_.calculate_utilization_slow().stored_values; }

 private:
  // list of memory regions, indexed by base address.
  ChildList regions_;
};

// A representation of a contiguous range of virtual address space
class VmAddressRegion final : public VmAddressRegionOrMapping {
 public:
  // Creates a root region.  This will span the entire aspace
  static zx_status_t CreateRootLocked(VmAspace& aspace, uint32_t vmar_flags,
                                      fbl::RefPtr<VmAddressRegion>* out)
      TA_REQ(aspace.region_lock()) TA_REQ(aspace.lock());
  // Creates a subregion of this region
  zx_status_t CreateSubVmar(size_t offset, size_t size, uint8_t align_pow2, uint32_t vmar_flags,
                            const char* name, fbl::RefPtr<VmAddressRegion>* out);
  // Creates a VmMapping within this region. To avoid leaks, this should be paired with a call to
  // VmMapping::Destroy if desired; dropping `MapResult::mapping` will *not* destroy the mapping.
  struct MapResult {
    // This will never be null
    fbl::RefPtr<VmMapping> mapping;
    // Represents the virtual address of |mapping| at the time of creation, which is equivalent to
    // |mapping->base_locking()|.
    vaddr_t base;
  };
  zx::result<MapResult> CreateVmMapping(size_t mapping_offset, size_t size, uint8_t align_pow2,
                                        uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo,
                                        uint64_t vmo_offset, arch_mmu_flags_t arch_mmu_flags,
                                        const char* name);

  // Finds the child region that contains the given addr.  If addr is in a gap,
  // returns nullptr.  This is a non-recursive search.
  fbl::RefPtr<VmAddressRegionOrMapping> FindRegion(vaddr_t addr);
  fbl::RefPtr<VmAddressRegionOrMapping> FindRegionLocked(vaddr_t addr) TA_REQ(lock());

  // Applies the given memory priority to this VMAR, which may or may not result in a change. Up to
  // the derived type to know how to apply and update the |memory_priority_| field.
  zx_status_t SetMemoryPriorityLocked(MemoryPriority priority) TA_REQ(lock());

  enum class RangeOpType {
    Commit,
    Decommit,
    MapRange,
    Zero,
    DontNeed,
    AlwaysNeed,
    Prefetch,
  };

  // Apply |op| to VMO mappings in the specified range of pages.
  zx_status_t RangeOp(RangeOpType op, vaddr_t base, size_t len,
                      VmAddressRegionOpChildren op_children, user_inout_ptr<void> buffer,
                      size_t buffer_size);

  // Unmap a subset of the region of memory in the containing address space,
  // returning it to this region to allocate.  If a subregion is entirely in
  // the range, and op_children is Yes, that subregion is destroyed. If a subregion is partially in
  // the range, Unmap() will fail.
  zx_status_t Unmap(vaddr_t base, size_t size, VmAddressRegionOpChildren op_children);

  // Change protections on a subset of the region of memory in the containing
  // address space. If the requested range overlaps with a subregion and op_children is No,
  // Protect() will fail, otherwise the mapping permissions in the sub-region may only be reduced.
  zx_status_t Protect(vaddr_t base, size_t size, arch_mmu_flags_t new_arch_mmu_flags,
                      VmAddressRegionOpChildren op_children);

  // Reserve a memory region within this VMAR. This region is already mapped in the page table with
  // |arch_mmu_flags|. VMAR should create a VmMapping for this region even though no physical pages
  // need to be allocated for this region.
  zx_status_t ReserveSpace(const char* name, size_t base, size_t size,
                           arch_mmu_flags_t arch_mmu_flags);

  const char* name() const { return name_; }
  bool has_parent() const;

  void DumpLocked(uint depth, bool verbose) const TA_REQ(region_lock()) TA_REQ(lock()) override;

  // Recursively traverses the regions for a given virtual address and returns a raw pointer to a
  // mapping if one is found. The returned pointer is only valid as long as the aspace lock remains
  // held.
  VmMapping* FindMappingLocked(vaddr_t va) TA_REQ(lock());

  // Apply a memory priority to this VMAR and all of its subregions.
  zx_status_t SetMemoryPriority(MemoryPriority priority);

  // Recursively compute the amount of attributed memory within this region
  using AttributionCounts = VmObject::AttributionCounts;
  AttributionCounts GetAttributedMemory() const;

  // Constructors are public as LazyInit cannot use them otherwise, even if friended, but
  // otherwise should be considered private and Create...() should be used instead.
  VmAddressRegion(VmAspace& aspace, vaddr_t base, size_t size, uint32_t vmar_flags);
  VmAddressRegion(VmAddressRegion& parent, vaddr_t base, size_t size, uint32_t vmar_flags,
                  const char* name);

  bool is_in_range(vaddr_t base, size_t size) const {
    const size_t offset = base - base_;
    return base >= base_ && offset < size_ && size_ - offset >= size;
  }

  // Traverses this vmar (and any sub-vmars) starting at this node, in depth-first pre-order. See
  // VmEnumerator for more details. If this vmar is not alive (in the LifeCycleState sense) or
  // otherwise not enumerable this returns ZX_ERR_BAD_STATE, otherwise the result of enumeration is
  // returned.
  zx_status_t EnumerateChildren(VmEnumerator* ve) TA_EXCL(lock());

 protected:
  friend class VmAspace;
  friend lazy_init::Access;
  friend void vm_init_preheap();

  // constructor for use in creating the kernel aspace singleton
  explicit VmAddressRegion(VmAspace& kernel_aspace);

  void CommitHighMemoryPriority() override TA_EXCL(lock());

  friend class VmMapping;
  template <VmAddressRegionEnumeratorType, typename>
  friend class VmAddressRegionEnumerator;

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(VmAddressRegion);

  fbl::Canary<fbl::magic("VMAR")> canary_;

  zx_status_t DestroyLocked() TA_REQ(lock()) TA_REQ(region_lock()) override;

  zx_status_t Activate() TA_REQ(region_lock()) TA_REQ(lock()) override;

  // Helpers to share code between CreateSubVmar and CreateVmMapping
  zx_status_t CreateSubVmarInternal(size_t offset, size_t size, uint8_t align_pow2,
                                    uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo,
                                    uint64_t vmo_offset, arch_mmu_flags_t arch_mmu_flags,
                                    const char* name, vaddr_t* base_out,
                                    fbl::RefPtr<VmAddressRegionOrMapping>* out);
  zx_status_t CreateSubVmarInner(size_t offset, size_t size, uint8_t align_pow2,
                                 uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo,
                                 uint64_t vmo_offset, arch_mmu_flags_t arch_mmu_flags,
                                 const char* name, vaddr_t* base_out,
                                 fbl::RefPtr<VmAddressRegionOrMapping>* out);

  // Create a new VmMapping within this region, overwriting any existing
  // mappings that are in the way.  If the range crosses a subregion, the call
  // fails.
  zx_status_t OverwriteVmMappingLocked(vaddr_t base, size_t size, uint32_t vmar_flags,
                                       fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset,
                                       arch_mmu_flags_t arch_mmu_flags,
                                       fbl::RefPtr<VmAddressRegionOrMapping>* out)
      TA_REQ(region_lock()) TA_REQ(lock());

  // Implementation for Unmap() and OverwriteVmMapping() that does not hold
  // the aspace lock. If |can_destroy_regions| is true, then this may destroy
  // VMARs that it completely covers.
  zx_status_t UnmapInternalLocked(vaddr_t base, size_t size, bool can_destroy_regions)
      TA_REQ(region_lock()) TA_REQ(lock());

  // If the allocation between the given children can be met this returns a virtual address of the
  // base address of that allocation, otherwise a nullopt is returned.
  ktl::optional<vaddr_t> CheckGapLockedRegion(const VmAddressRegionOrMapping* prev,
                                              const VmAddressRegionOrMapping* next,
                                              vaddr_t search_base, vaddr_t align,
                                              size_t region_size, size_t min_gap,
                                              arch_mmu_flags_t arch_mmu_flags)
      TA_REQ(region_lock());

  // search for a spot to allocate for a region of a given size
  zx_status_t AllocSpotLockedRegion(size_t size, uint8_t align_pow2,
                                    arch_mmu_flags_t arch_mmu_flags, vaddr_t* spot,
                                    vaddr_t upper_limit = ktl::numeric_limits<vaddr_t>::max())
      TA_REQ(region_lock());

  const RegionList<VmAddressRegionOrMapping>& subregions_locked() const
      TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return subregions_;
  }

  const RegionList<VmAddressRegionOrMapping>& subregions_locked_region() const
      TA_REQ(region_lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return subregions_;
  }

  RegionList<VmAddressRegionOrMapping> subregions_ TA_GUARDED(lock()) TA_GUARDED(region_lock());

  const char name_[ZX_MAX_NAME_LEN] = {};
};

extern "C" {
void cpp_vm_mapping_free(VmMapping* mapping);
zx_status_t cpp_vm_mapping_force_writable(VmMapping* mapping, VmMapping** out_mapping);
}

// A representation of the mapping of a VMO into the address space
class VmMapping final : public VmAddressRegionOrMapping {
 public:
  // Accessors for VMO-mapping state
  // These can be read under either lock (both locks being held for writing), so we provide two
  // different accessors, one for each lock.
  arch_mmu_flags_t arch_mmu_flags_locked(vaddr_t offset) const
      TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return FlagsRangeAtAddrLocked(offset).mmu_flags;
  }
  arch_mmu_flags_t arch_mmu_flags_locked_object(vaddr_t offset) const
      TA_REQ(object_->lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return FlagsRangeAtAddrLocked(offset).mmu_flags;
  }
  struct FlagsRange {
    arch_mmu_flags_t mmu_flags;
    uint64_t region_top;
  };
  FlagsRange arch_mmu_flags_range_locked(vaddr_t offset) const
      TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return FlagsRangeAtAddrLocked(offset);
  }
  uint64_t object_offset() const { return object_offset_; }

  Lock<CriticalMutex>* object_lock() const TA_RET_CAP(object_->lock()) TA_REQ(lock()) {
    return object_->lock();
  }
  Lock<CriticalMutex>& object_lock_ref() const TA_RET_CAP(object_->lock()) TA_REQ(lock()) {
    return object_->lock_ref();
  }

  // Intended to be used from VmEnumerator callbacks where the aspace_->lock() will be held.
  fbl::RefPtr<VmObject> vmo_locked() const TA_REQ(lock()) { return object_; }
  fbl::RefPtr<VmObject> vmo() const TA_EXCL(lock());

  // Convenience wrapper for vmo()->DecommitRange() with the necessary
  // offset modification and locking.
  zx_status_t DecommitRange(size_t offset, size_t len) TA_EXCL(lock());

  // Map in pages from the underlying vm object, optionally committing pages as it goes.
  // |ignore_existing| controls whether existing hardware mappings in the specified range should be
  // ignored or treated as an error. |ignore_existing| should only be set to true for user mappings
  // where populating mappings may already be racy with multiple threads, and where we are already
  // tolerant of mappings being arbitrarily created and destroyed.
  zx_status_t MapRange(size_t offset, size_t len, bool commit, bool ignore_existing = false)
      TA_EXCL(lock());

  using AttributionCounts = VmObject::AttributionCounts;
  AttributionCounts GetAttributedMemory() const;

  // Unlocked convenience wrapper of UnmapLocked for testing.
  zx_status_t DebugUnmap(vaddr_t base, size_t size) TA_EXCL(lock()) {
    Guard<CriticalMutex> region_guard{region_lock()};
    Guard<CriticalMutex> guard{lock()};
    return UnmapLocked(base, size);
  }

  // Unlocked convenience wrapper of ProtectLocked for testing.
  zx_status_t DebugProtect(vaddr_t base, size_t size, arch_mmu_flags_t new_arch_mmu_flags)
      TA_EXCL(lock()) {
    Guard<CriticalMutex> guard{lock()};
    return ProtectLocked(base, size, new_arch_mmu_flags);
  }

  void DumpLocked(uint depth, bool verbose) const TA_REQ(lock()) override;

  // Page fault in an address within the mapping. The requested address must be paged aligned. If
  // |additional_pages| is non-zero, then up to that many additional pages may be resolved using the
  // same |pf_flags|. It is not an error for the |additional_pages| to span beyond the mapping or
  // underlying VMO, although the range will get truncated internally. As such only the page
  // containing va is required to be resolved, and this method may return ZX_OK if any number,
  // including zero, of the additional pages are resolved.
  // |object| is required to be the value of object_ with the requirement that if the aspace lock()
  // is not held over the call to this function, then the caller is required to ensure that |object|
  // will remain alive for the duration of the call.
  // As the |additional_pages| are resolved with the same |pf_flags| they may trigger copy-on-write
  // or other allocations in the underlying VMO.
  // If this returns ZX_ERR_SHOULD_WAIT, then the caller should wait on |page_request|
  // and try again. In addition to a status this returns how many pages got mapped in.
  // This may return ZX_ERR_UNAVAILABLE if the aspace lock() is not held and means that the mapping
  // was destroyed before the page fault could be handled.
  // If ZX_OK is returned then the number of pages mapped in is guaranteed to be >0.
  // If |additional_pages| was non-zero, then the maximum number of pages that will be mapped is
  // |additional_pages + 1|. Otherwise the maximum number of pages that will be mapped is
  // kPageFaultMaxOptimisticPages.
  ktl::pair<zx_status_t, uint32_t> PageFault(vaddr_t va, uint pf_flags, size_t additional_pages,
                                             VmObject* object, MultiPageRequest* page_request);

  // Convenience wrapper around PageFault that can be called with the aspace lock held and will
  // never return ZX_ERR_UNAVAILABLE.
  ktl::pair<zx_status_t, uint32_t> PageFaultLocked(vaddr_t va, uint pf_flags,
                                                   size_t additional_pages,
                                                   MultiPageRequest* page_request) TA_REQ(lock());

  // Apis intended for use by VmObject

  // |assert_object_lock| exists to satisfy clang capability analysis since there are circumstances
  // when the object_->lock() is actually being held, but it was not acquired by dereferencing
  // object_. In this scenario we need to explain to the analysis that the lock held is actually the
  // same as object_->lock(), and even though we otherwise have no intention of using object_, the
  // only way to do this is to notionally dereferencing object_ to compare the lock.
  // Since this is asserting that the lock is held, and not just returning a reference to the lock,
  // this method is logically correct since object_ itself is only modified if object_->lock() is
  // held.
  void assert_object_lock() const TA_ASSERT(object_->lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    AssertHeld(object_->lock_ref());
  }

  enum UnmapOptions : uint8_t {
    kNone = 0u,
    OnlyHasZeroPages = (1u << 0),
    Harvest = (1u << 1),
  };

  // Unmap any pages that map the passed in vmo range from the arch aspace.
  // May not intersect with this range.
  // If |only_has_zero_pages| is true then the caller is asserting that it knows that any mappings
  // in the region will only be for the shared zero page.
  void AspaceUnmapLockedObject(uint64_t offset, uint64_t len, UnmapOptions options) const
      TA_REQ(object_->lock());

  // Removes any writeable mappings for the passed in vmo range from the arch aspace.
  // May fall back to unmapping pages from the arch aspace if necessary.
  void AspaceRemoveWriteLockedObject(uint64_t offset, uint64_t len) const TA_REQ(object_->lock());

  // Checks if this is a kernel mapping within the given VMO range, which would be an error to be
  // unpinning.
  void AspaceDebugUnpinLockedObject(uint64_t offset, uint64_t len) const TA_REQ(object_->lock());

  // Marks this mapping as being a candidate for merging, and will immediately attempt to merge with
  // any neighboring mappings. Making a mapping mergeable essentially indicates that you will no
  // longer use this specific VmMapping instance to refer to the referenced region, and will access
  // the region via the parent vmar in the future, and so the region merely needs to remain valid
  // through some VmMapping.
  // For this the function requires you to hand in your last remaining refptr to the mapping.
  static void MarkMergeable(fbl::RefPtr<VmMapping> mapping);

  // Enumerates any different protection ranges that exist inside this mapping. The virtual range
  // specified by range_base and range_size must be within this mappings base_ and size_. The
  // provided callback is called in virtual address order for each protection type. ZX_ERR_NEXT
  // and ZX_ERR_STOP can be used to control iteration, with any other status becoming the return
  // value of this method.
  template <typename F>
  zx_status_t EnumerateProtectionRangesLocked(vaddr_t base, size_t size, F func) const
      TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    DEBUG_ASSERT(is_in_range(base, size));
    // If the mapping is no longer alive, then return early since there's nothing to enumerate.
    if (!IsAliveLocked()) {
      return ZX_OK;
    }

    const vaddr_t end = base + size;

    // Find the first transition strictly after 'base'.
    auto it = rest_protection_ranges_.upper_bound(base);

    // |it| now represents the end point of the first range, so look backwards to determine the
    // start flags.
    auto prev = it;
    prev--;
    arch_mmu_flags_t flags = prev ? (*prev).second : first_region_arch_mmu_flags_;
    vaddr_t range_start = base;

    while (true) {
      // The current range ends at either 'end' or the next transition point, whichever is earlier.
      const vaddr_t range_end = it ? ktl::min((*it).first, end) : end;
      DEBUG_ASSERT(range_start < range_end);

      zx_status_t result = func(range_start, range_end - range_start, flags);
      if (result != ZX_ERR_NEXT) {
        if (result == ZX_ERR_STOP) {
          return ZX_OK;
        }
        return result;
      }

      // If we've reached the end of the requested range, or there are no more transitions, stop.
      // Placing this check here allows the range generation above to also generate the 'trailing'
      // range for the last transition point without needing an extra invocation of |func|, which
      // helps inlining.
      if (!it || (*it).first >= end) {
        break;
      }

      // Move to the next transition point.
      range_start = range_end;
      flags = (*it).second;
      it++;
    }
    return ZX_OK;
  }

  template <typename F>
  zx_status_t EnumerateProtectionRangesLockedObject(vaddr_t base, size_t size, F func) const
      TA_REQ(object_->lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return EnumerateProtectionRangesLocked(base, size, func);
  }

  FlagsRange FlagsRangeAtAddrLocked(vaddr_t va) const TA_REQ(lock()) TA_REQ(object_->lock()) {
    if (rest_protection_ranges_.is_empty()) {
      return FlagsRange{first_region_arch_mmu_flags_, base_ + size_};
    }
    // Find the first transition strictly after 'va'.
    auto it = rest_protection_ranges_.upper_bound(va);
    // |it| represents the end of the range that includes |va|
    vaddr_t top = it ? (*it).first : base_ + size_;
    // Now go backwards to find the start of the range that includes |va|, which tells us the flags.
    it--;
    arch_mmu_flags_t flags = it ? (*it).second : first_region_arch_mmu_flags_;
    return FlagsRange{flags, top};
  }
  FlagsRange FlagsRangeAtAddrLockedObject(vaddr_t va) const
      TA_REQ(object_->lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return FlagsRangeAtAddrLocked(va);
  }

  // The maximum number of pages that a page fault can optimistically extend the fault to include.
  // This is defined and exposed here for the purposes of unittests.
  static constexpr uint64_t kPageFaultMaxOptimisticPages = 16;

  // TODO(https://fxbug.dev/42106188): Informs the mapping that a write is going to be performed to
  // the backing VMO, even if the VMO is not writable. This gives the mapping an opportunity to
  // create a private clone of the VMO if necessary and use that to back a new mapping instead,
  // providing a way to 'safely' perform the write. On success a RefPtr is returned either to the
  // current mapping, or to a new mapping if one was created. If a new mapping was created then this
  // mapping is no longer valid.
  zx::result<fbl::RefPtr<VmMapping>> ForceWritable();

 protected:
  ~VmMapping() override;
  friend fbl::RefPtr<VmMapping>;
  friend void ::cpp_vm_mapping_free(VmMapping*);

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(VmMapping);

  fbl::Canary<fbl::magic("VMAP")> canary_;

  enum class Mergeable : bool { YES = true, NO = false };

  // allow VmAddressRegion to manipulate VmMapping internals for construction
  // and bookkeeping
  friend class VmAddressRegion;

  // private constructors, use VmAddressRegion::Create...() instead
  VmMapping(VmAddressRegion& parent, bool private_clone, vaddr_t base, size_t size,
            uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset,
            arch_mmu_flags_t arch_mmu_flags, Mergeable mergeable);
  VmMapping(VmAddressRegion& parent, bool private_clone, vaddr_t base, size_t size,
            uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset,
            arch_mmu_flags_t first_mmu_flags, btree::BTree<vaddr_t, arch_mmu_flags_t>&& ranges,
            Mergeable mergeable);

  zx_status_t DestroyLocked() TA_REQ(region_lock()) TA_REQ(lock()) override;

  // Internal fully locked version of Destroy. Has controls to both skip the arch aspace unmapping
  // as well as removal from the parent subregions list. These controls facilitate the fine grained
  // control needed when splitting, merging and replacing mappings.
  // If unmap is |No| then this method is defined to never fail. |remove_region| does not impact
  // success or failure of the operation.
  enum class DestroyUnmap : bool {
    No,
    Yes,
  };
  enum class DestroyRemoveFromParent : bool {
    No,
    Yes,
  };
  zx_status_t DestroyLockedObject(DestroyUnmap unmap, DestroyRemoveFromParent remove_region)
      TA_REQ(region_lock()) TA_REQ(lock()) TA_REQ(object_->lock());

  // Internal helper for performing a page fault after the object_ lock is acquired. Additionally
  // object_ is passed in as the downcast specific type in object to allow the helper to assume
  // that object->lock() is held in all paths, without needing to do its own AssertHeld's, after
  // runtime casting, throughout.
  template <typename T>
  ktl::pair<zx_status_t, uint32_t> PageFaultLockedObject(vaddr_t va, uint pf_flags,
                                                         size_t additional_pages, T* object,
                                                         VmCowPages::DeferredOps* deferred,
                                                         MultiPageRequest* page_request)
      TA_REQ(object->lock()) TA_REQ(object_->lock());

  // Unmap a subset of the region of memory in the containing address space,
  // returning it to the parent region to allocate.  If all of the memory is unmapped,
  // Destroy()s this mapping.  If a subrange of the mapping is specified, the
  // mapping may be split.
  zx_status_t UnmapLocked(vaddr_t base, size_t size) TA_REQ(region_lock()) TA_REQ(lock());

  // Change access permissions for this mapping.  It is an error to specify a
  // caching mode in the flags.  This will persist the caching mode the
  // mapping was created with.  If a subrange of the mapping is specified, the
  // mapping may be split.
  zx_status_t ProtectLocked(vaddr_t base, size_t size, arch_mmu_flags_t new_arch_mmu_flags)
      TA_REQ(lock());

  // Helper for protect and unmap.
  static zx_status_t ProtectOrUnmap(const fbl::RefPtr<VmAspace>& aspace, vaddr_t base, size_t size,
                                    arch_mmu_flags_t new_arch_mmu_flags);

  // Copies protection ranges for the given sub-range into |out_first_flags| and |out_ranges|.
  // The sub-range [base, base + size) must be within [base_, base_ + size_).
  zx_status_t CopyProtectionRangesLocked(vaddr_t base, size_t size,
                                         arch_mmu_flags_t* out_first_flags,
                                         btree::BTree<vaddr_t, arch_mmu_flags_t>* out_ranges) const
      TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS;

  // Merges a copy of the protection ranges of |right| into this mapping.
  // Assumes |right| is immediately to the right of this mapping.
  zx_status_t MergeProtectionRangesLocked(const VmMapping& right) TA_REQ(lock())
      TA_REQ(object_->lock()) TA_REQ(right.lock()) TA_REQ(right.object_lock());

  // Removes any protection range transitions within the range [base, end).
  void ClearProtectionRangeTransitionsLocked(vaddr_t base, vaddr_t end) TA_REQ(lock())
      TA_REQ(object_->lock());

  // Removes all protection range transitions at or below |split_addr|, returning the flags
  // that are active at |split_addr|.
  arch_mmu_flags_t RemoveAfterSplitLocked(vaddr_t split_addr) TA_REQ(lock())
      TA_REQ(object_->lock());

  AttributionCounts GetAttributedMemoryLocked(Guard<CriticalMutex>& guard) const TA_REQ(lock());

  // If MemoryPriority::HIGH, then disable dynamic reclamation within this region. If
  // MemoryPriority::DEFAULT, move towards allowing reclamation.
  //
  // When called with SplitOnUnmap=true, we set the memory
  // priority assuming the object has not been |Activate|'d yet.
  template <bool SplitOnUnmap = false>
  void SetMemoryPriorityLocked(VmAddressRegion::MemoryPriority priority) TA_REQ(lock())
      TA_EXCL(object_->lock());

  // Move towards allowing dynamic reclamation in the region. You may call this method with the
  // object_ lock, in contrast to SetMemoryPriorityLocked.
  //
  // When called with SetMemoryPriorityLockedObject</*SplitOnUnmap=*/true>, we set the memory
  // priority assuming the object has not been |Activate|'d yet.
  template <bool SplitOnUnmap = false>
  void SetMemoryPriorityDefaultLockedObject() TA_REQ(lock()) TA_REQ(object_->lock());

  // Marks the address region as high priority. Only call this method if the mapped
  // VmObject is already high priority (for example if you have already called
  // SetMemoryPriorityLocked on a child).
  //
  // You may call this method with the object_ lock, in contrast to SetMemoryPriorityLocked.
  //
  // When called with SetMemoryPriorityLockedObject</*SplitOnUnmap=*/true>, we set the memory
  // priority assuming the object has not been |Activate|'d yet.
  template <bool SplitOnUnmap = false>
  void SetMemoryPriorityHighAlreadyPositiveLockedObject() TA_REQ(lock()) TA_REQ(object_->lock());

  void CommitHighMemoryPriority() override TA_EXCL(lock());

  zx_status_t Activate() TA_REQ(region_lock()) TA_REQ(lock()) override;

  // Fully locked version of Activate that can additionally control whether the region is installed
  // into the parent subregion list and vmo mapping list or not. This control exists to facilitate
  // the fine grained control needed for splitting, merging and replacing of mappings as when set to
  // |No| this method is defined as never failing.
  enum class ActivateInsertRegions : bool {
    No,
    Yes,
  };
  zx_status_t ActivateLocked(ActivateInsertRegions insert_region) TA_REQ(region_lock())
      TA_REQ(lock()) TA_REQ(object_->lock());

  // Wrapper for ActivateLocked that does not insert into the parent region, and therefore cannot
  // fail.
  void ActivateNoInsertLocked() TA_REQ(region_lock()) TA_REQ(lock()) TA_REQ(object_->lock()) {
    [[maybe_unused]] zx_status_t status = ActivateLocked(ActivateInsertRegions::No);
    ASSERT(status == ZX_OK);
  }

  // Takes a range relative to the vmo object_ and converts it into a virtual address range relative
  // to aspace_. Returns true if a non zero sized intersection was found, false otherwise. If false
  // is returned |base| and |virtual_len| hold undefined contents.
  bool ObjectRangeToVaddrRange(uint64_t offset, uint64_t len, vaddr_t* base,
                               uint64_t* virtual_len) const TA_REQ(object_->lock());

  // Attempts to merge this mapping with any neighbors. It is the responsibility of the caller to
  // ensure a refptr to this is being held, as on return |this| may be in the dead state and have
  // removed itself from the hierarchy, dropping a refptr.
  void TryMergeNeighborsLocked() TA_REQ(region_lock()) TA_REQ(lock());

  // Attempts to merge this and the given mapping into a new one. This only succeeds if the
  // candidate is placed just after |this|, both in the aspace and the vmo. See implementation for
  // the full requirements for merging to succeed. The candidate and this must be held as a RefPtr
  // by the caller so that this function does not trigger any VmMapping destructor by dropping the
  // last reference when removing from the parent vmar. If merging is successfully the newly created
  // and installed mapping is returned, otherwise a nullptr is returned.
  fbl::RefPtr<VmMapping> TryMergeRightNeighborLocked(VmMapping* right_candidate)
      TA_REQ(region_lock()) TA_REQ(lock());

  // For a VmMapping |state_| is only modified either with the object_ lock held, or if there is no
  // |object_|. Therefore it is safe to read state if just the object lock is held.
  LifeCycleState get_state_locked_object() const
      TA_REQ(object_->lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return state_;
  }

  // Returns the minimum of the requested map length, the size of the VMO or, if
  // FAULT_BEYOND_STREAM_SIZE is set, the  page containing the stream size. MapRange can be trimmed
  // to these lengths as it should not be considered an error to call MapRange past the VMO size in
  // a resizable VMO or past the page containing the stream size in a FAULT_BEYOND_STREAM_SIZE VMO.
  uint64_t TrimmedObjectRangeLocked(uint64_t offset, uint64_t len) const TA_REQ(lock())
      TA_REQ(object_->lock());

  const btree::BTree<vaddr_t, arch_mmu_flags_t>& rest_protection_ranges_locked() const
      TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return rest_protection_ranges_;
  }

  arch_mmu_flags_t first_region_arch_mmu_flags_locked() const
      TA_REQ(lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    return first_region_arch_mmu_flags_;
  }

  // Whether this mapping may be merged with other adjacent mappings. A mergeable mapping is just a
  // region that can be represented by any VmMapping object, not specifically this one.
  Mergeable mergeable_ TA_GUARDED(lock()) = Mergeable::NO;

  // TODO(https://fxbug.dev/42106188): Tracks whether this mapping has been transitioned into a
  // private clone to allow for writes to safely be done without modifying a VMO that the mapping
  // does not have permission to.
  const bool private_clone_ = false;

  // Tracks whether the object_ has been reset (i.e. is null) or not. object_ is always non-null at
  // construction and only ever performs a single transition, which is to the null value, prior to
  // destruction. The object_.reset, and hence the modification of this value, happens under both
  // the aspace lock() and the object_->lock(). See PageFault for usage of this.
  RelaxedAtomic<bool> object_reset_ = false;

  // The protection flags of this mapping are tracked as a series of contiguous
  // virtual address ranges. Since the base address of the mapping is known (`base_`),
  // we only need to store the flags for the first range, and then the start address
  // and flags for any subsequent ranges where the protections change.
  //
  // `first_region_arch_mmu_flags_` holds the MMU flags for the interval starting
  // exactly at `base_`. If `protection_ranges_` is empty, these flags apply to the
  // entire mapping: [base_, base_ + size_).
  //
  // `protection_ranges_` is a B-Tree that records any transitions in protection.
  // Each entry consists of a `vaddr_t` key and an `arch_mmu_flags_t` value.
  // A node (addr, flags) means that the range starting at `addr` has the protections
  // `flags`, up until the address of the next node in the tree, or `base_ + size_`
  // if it is the last node.
  //
  // This can be read with either lock held, but requires both locks to write it.
  arch_mmu_flags_t first_region_arch_mmu_flags_ TA_GUARDED(lock()) TA_GUARDED(object_->lock());
  btree::BTree<vaddr_t, arch_mmu_flags_t> rest_protection_ranges_ TA_GUARDED(lock())
      TA_GUARDED(object_->lock());

  // pointer and region of the object we are mapping.
  // The object_ cannot be marked const, as it gets reset(), but logically it is a constant value
  // from construction until it gets reset, and then it stays null until the mapping is destructed.
  fbl::RefPtr<VmObject> object_ TA_GUARDED(lock());
  const uint64_t object_offset_ = 0;

  class CurrentlyFaulting;
  // Pointer to a CurrentlyFaulting object if the mapping is presently handling a page fault. This
  // is protected specifically by the object lock so that AspaceUnmapLockedObject can inspect it.
  CurrentlyFaulting* currently_faulting_ TA_GUARDED(object_->lock()) = nullptr;
};

// Interface for walking a VmAspace-rooted VmAddressRegion/VmMapping tree.
// Override this class and pass an instance to VmAddressRegion::EnumerateChildren().
// VmAddressRegion::EnumerateChildren() will call the On* methods in depth-first pre-order.
// ZX_ERR_NEXT and ZX_ERR_STOP can be used to control iteration, with any other status becoming the
// return value of this method. The root VmAspace's lock is held during the traversal and passed in
// to the callbacks as |guard|. A callback is permitted to temporarily drop the lock, using
// |CallUnlocked|, although doing so invalidates the pointers and to use them without the lock held,
// of after it is reacquired, they should first be turned into a RefPtr, with the caveat that they
// might now refer to a dead, aka unmapped, object.
class VmEnumerator {
 public:
  // |depth| will be 0 for the root VmAddressRegion.
  virtual zx_status_t OnVmAddressRegion(VmAddressRegion* vmar, uint depth,
                                        Guard<CriticalMutex>& guard) TA_REQ(vmar->lock()) {
    return ZX_ERR_NEXT;
  }

  // |vmar| is the parent of |map|.
  virtual zx_status_t OnVmMapping(VmMapping* map, VmAddressRegion* vmar, uint depth,
                                  Guard<CriticalMutex>& guard) TA_REQ(map->lock())
      TA_REQ(vmar->lock()) {
    return ZX_ERR_NEXT;
  }

 protected:
  VmEnumerator() = default;
  ~VmEnumerator() = default;
};

// Now that all the sub-classes are defined finish declaring some inline VmAddressRegionOrMapping
// methods.
inline fbl::RefPtr<VmAddressRegion> VmAddressRegionOrMapping::as_vm_address_region() {
  canary_.Assert();
  if (is_mapping()) {
    return nullptr;
  }
  return fbl::RefPtr<VmAddressRegion>(static_cast<VmAddressRegion*>(this));
}

inline VmAddressRegion* VmAddressRegionOrMapping::as_vm_address_region_ptr() {
  canary_.Assert();
  if (unlikely(is_mapping())) {
    return nullptr;
  }
  return static_cast<VmAddressRegion*>(this);
}

inline const VmAddressRegion* VmAddressRegionOrMapping::as_vm_address_region_ptr() const {
  canary_.Assert();
  if (unlikely(is_mapping())) {
    return nullptr;
  }
  return static_cast<const VmAddressRegion*>(this);
}

inline fbl::RefPtr<VmAddressRegion> VmAddressRegionOrMapping::downcast_as_vm_address_region(
    fbl::RefPtr<VmAddressRegionOrMapping>* region_or_map) {
  DEBUG_ASSERT(region_or_map);
  if ((*region_or_map)->is_mapping()) {
    return nullptr;
  }
  return fbl::RefPtr<VmAddressRegion>::Downcast(ktl::move(*region_or_map));
}

inline fbl::RefPtr<VmMapping> VmAddressRegionOrMapping::as_vm_mapping() {
  canary_.Assert();
  if (!is_mapping()) {
    return nullptr;
  }
  return fbl::RefPtr<VmMapping>(static_cast<VmMapping*>(this));
}

inline VmMapping* VmAddressRegionOrMapping::as_vm_mapping_ptr() {
  canary_.Assert();
  if (unlikely(!is_mapping())) {
    return nullptr;
  }
  return static_cast<VmMapping*>(this);
}

inline const VmMapping* VmAddressRegionOrMapping::as_vm_mapping_ptr() const {
  canary_.Assert();
  if (unlikely(!is_mapping())) {
    return nullptr;
  }
  return static_cast<const VmMapping*>(this);
}

inline fbl::RefPtr<VmMapping> VmAddressRegionOrMapping::downcast_as_vm_mapping(
    fbl::RefPtr<VmAddressRegionOrMapping>* region_or_map) {
  DEBUG_ASSERT(region_or_map);
  if (!(*region_or_map)->is_mapping()) {
    return nullptr;
  }
  return fbl::RefPtr<VmMapping>::Downcast(ktl::move(*region_or_map));
}

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_H_
