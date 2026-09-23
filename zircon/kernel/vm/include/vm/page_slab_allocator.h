// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_SLAB_ALLOCATOR_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_SLAB_ALLOCATOR_H_

#include <lib/page/size.h>
#include <pow2.h>

#include <ktl/algorithm.h>
#include <ktl/iterator.h>
#include <ktl/span.h>
#include <vm/page.h>
#include <vm/physmap.h>
#include <vm/pmm.h>

// Simple slab allocator that uses a vm_page_t as its slab to perform allocations out of. This makes
// the allocator only suitable for small allocations, preferably ones that divide evenly into a
// page.
//
// All per-slab metadata is stored in the vm_page_t itself and, by default, the only dependency of
// the allocator is the PMM to allocate and free pages from. The heap is not needed for any other
// metadata allocations.
//
// Free regions are tracked in a two level list with each slab having an internal list of free
// regions, and the allocator itself having a list of slabs that have at least one free region.
//
// Slabs that become fully empty are able to be returned to the PMM, although allocations cannot be
// moved between slabs, so fragmentation can still occur.
//
// The allocator can be specialized to control the actual allocation and freeing of the slabs
// themselves.
//
// This class it not thread safe.
template <size_t AllocSize>
class PageSlabAllocator {
 public:
  PageSlabAllocator() = default;
  ~PageSlabAllocator() {
    ASSERT(full_slabs_.is_empty());
    ASSERT(available_slabs_.is_empty());
  }

  // Allocates an area of uninitialized memory of AllocSize and returns a pointer to it, or nullptr
  // on error.
  void* allocate_bytes() { return reinterpret_cast<void*>(Allocate()); }

  // Allocates an area of uninitialized memory capable of holding a single object of type |T|. This
  // is largely a convenience wrapper around |allocate_bytes| that validates T is compatible with
  // the size and alignment of the allocations.
  // Returns a nullptr on error.
  template <typename T>
  T* allocate_object() {
    // T must fit inside the size of the allocation.
    static_assert(sizeof(T) <= AllocSize);
    // Allocations must have at least equivalent alignment.
    static_assert(alignof(T) <= kEntryAlign);
    return reinterpret_cast<T*>(allocate_bytes());
  }

  // Deallocates the storage referenced by |ptr|, which must be a pointer obtained by an earlier
  // call to |allocate_object| or |allocate_bytes|.
  void deallocate_bytes(void* ptr) { Free(ptr); }

  // Typed convenience wrapper around |deallocate_bytes|. Assumes the object at |ptr| has already
  // been destructed.
  template <typename T>
  void deallocate_object(T* ptr) {
    Free(ptr);
  }

  size_t AllocatedSlabs() const { return allocated_slabs_; }

  // Helper to return the number of slabs this allocator would allocate to store the specified
  // number of allocations.
  static constexpr uint32_t SlabsRequired(uint32_t num_allocs) {
    return fbl::round_up(num_allocs, kAllocsPerSlab) / kAllocsPerSlab;
  }

 protected:
  union Entry {
    Entry() : next(0) {}
    ~Entry() {}
    uint32_t next;
    char storage[AllocSize];
  };
  static_assert(sizeof(Entry) == AllocSize);
  static constexpr uint32_t kEntryAlign = 1u << ktl::countr_zero(sizeof(Entry));
  // Allocations needs to fit in a page.
  static_assert(AllocSize < kPageSize);
  static constexpr uint32_t kAllocsPerSlab = kPageSize / AllocSize;
  static constexpr uint32_t kEndOfList = UINT32_MAX;
  ktl::pair<vm_page_t*, uint32_t> AllocToSlab(const void* ptr) const {
    const uint32_t offset = reinterpret_cast<uintptr_t>(ptr) % kPageSize;
    vm_page_t* page = PaddrToPage(physmap_to_paddr(ptr));
    ASSERT(page && page->state() == vm_page_state::SLAB);
    DEBUG_ASSERT(page->queue_node.InContainer());
    DEBUG_ASSERT(offset % AllocSize == 0);
    return {page, offset / AllocSize};
  }
  static Entry* GetEntry(vm_page_t* slab, uint32_t index) {
    ASSERT(slab->state() == vm_page_state::SLAB);
    DEBUG_ASSERT(index < kAllocsPerSlab);
    return reinterpret_cast<Entry*>(paddr_to_physmap(slab->paddr())) + index;
  }
  void DebugFreeAllSlabs() {
    for (auto& p : full_slabs_) {
      profile_track_free(p.slab.profile_cookie, kPageSize);
    }
    for (auto& p : available_slabs_) {
      profile_track_free(p.slab.profile_cookie, kPageSize);
    }
    Pmm::Node().FreeList(&full_slabs_);
    Pmm::Node().FreeList(&available_slabs_);
  }
  virtual vm_page_t* AllocSlab() {
    vm_page_t* page = Pmm::Node().AllocPage(0).value_or(nullptr);
    if (page) {
      page->set_state(vm_page_state::SLAB);
      // There is enough space to store a cookie per allocation in the slab, so amortize it and
      // record a per slab cookie. On average this should have every different call site using this
      // allocator to get proportional blame.
      page->slab.profile_cookie = profile_track_alloc(kPageSize);
    }
    return page;
  }
  virtual void FreeSlab(vm_page_t* slab) {
    DEBUG_ASSERT(slab->state() == vm_page_state::SLAB);
    profile_track_free(slab->slab.profile_cookie, kPageSize);
    Pmm::Node().FreePage(slab);
  }
  virtual vm_page_t* PaddrToPage(paddr_t paddr) const { return Pmm::Node().PaddrToPage(paddr); }

 private:
  bool AddSlab() {
    // Allocate a new slab.
    vm_page_t* slab = AllocSlab();
    if (!slab) {
      return false;
    }
    slab->slab.free_slot = kEndOfList;
    slab->slab.peak_allocated = 0;
    slab->slab.allocated = 0;
    // Insert it into the available slabs.
    available_slabs_.push_front(slab);
    allocated_slabs_++;
    return true;
  }
  Entry* Allocate() {
    // See if there are any slabs available.
    if (available_slabs_.is_empty()) {
      if (!AddSlab()) {
        return nullptr;
      }
    }
    vm_page_t* page = &available_slabs_.front();
    Entry* entry;
    if (page->slab.free_slot == kEndOfList) {
      DEBUG_ASSERT(page->slab.peak_allocated < kAllocsPerSlab);
      entry = GetEntry(page, page->slab.peak_allocated);
      page->slab.peak_allocated++;
    } else {
      entry = GetEntry(page, page->slab.free_slot);
      page->slab.free_slot = entry->next;
    }
    page->slab.allocated++;
    if (page->slab.free_slot == kEndOfList && page->slab.peak_allocated == kAllocsPerSlab) {
      DEBUG_ASSERT(page->slab.allocated == kAllocsPerSlab);
      available_slabs_.erase(*page);
      full_slabs_.push_front(page);
    } else {
      DEBUG_ASSERT(page->slab.allocated < kAllocsPerSlab);
    }
    return entry;
  }
  void Free(void* ptr) {
    // Lookup the slab this was allocated in.
    auto [slab, index] = AllocToSlab(ptr);

    // This will only catch the most egregious kinds of double-frees, but is better than nothing.
    DEBUG_ASSERT(slab->slab.free_slot != index);

    DEBUG_ASSERT(slab->slab.allocated > 0);
    if (slab->slab.allocated == 1) {
      // Slab has become empty, can free it.
      available_slabs_.erase(*slab);
      allocated_slabs_--;
      FreeSlab(slab);
      return;
    }

    if (slab->slab.allocated == kAllocsPerSlab) {
      // Slab is going from full to having space available, move to the correct list. We place at
      // the back of the list to encourage allocations, which happen on the head, to fill up a page
      // instead of constantly bouncing allocations into different, partially full, pages.
      full_slabs_.erase(*slab);
      available_slabs_.push_back(slab);
    }
    slab->slab.allocated--;

    // Update the free list for this slab.
    Entry* entry = GetEntry(slab, index);
    entry->next = slab->slab.free_slot;
    slab->slab.free_slot = index;
  }

  VmPageDoublyLinkedList full_slabs_;
  VmPageDoublyLinkedList available_slabs_;

  // Track the total number of allocated slabs (i.e. pages), both full and available.
  size_t allocated_slabs_ = 0;
};

// Common base for a specialized PageSlabAllocator that can assign a numerical ID to each
// allocation, in lieu of the pointer, and convert between. This base implements the conversion
// functions under the assumption that the derived type is assigning each allocated slab a unique
// ID, such that id*kAllocsPerSlab fits within a uint32_t.
//
// The size of the ID is fixed at 32-bits, and not variable, since a 16-bit limit is unlikely to be
// useful in practice, and a 64-bit limit has no value since the pointer is already a 64-bit ID.
template <size_t AllocSize>
class BaseIdSlabAllocator : public PageSlabAllocator<AllocSize> {
 public:
  static constexpr uint32_t kAllocsPerSlab = PageSlabAllocator<AllocSize>::kAllocsPerSlab;
  BaseIdSlabAllocator() = default;
  ~BaseIdSlabAllocator() = default;

  uint32_t AllocToId(const void* ptr) const {
    auto [slab, slab_index] = PageSlabAllocator<AllocSize>::AllocToSlab(ptr);
    uint32_t id = (slab->slab.id * kAllocsPerSlab) + slab_index;
    return id;
  }
  void* IdToAlloc(uint32_t id) const {
    uint32_t slab_id = id / kAllocsPerSlab;
    uint32_t slab_index = id % kAllocsPerSlab;
    vm_page_t* slab = IdToSlab(slab_id);
    const uintptr_t base =
        reinterpret_cast<uintptr_t>(PageSlabAllocator<AllocSize>::GetEntry(slab, 0));
    const uintptr_t alloc = base + slab_index * AllocSize;
    return reinterpret_cast<void*>(alloc);
  }

 private:
  virtual vm_page_t* IdToSlab(uint32_t slab_id) const = 0;
};

// An implementation of the BaseIdSlabAllocator that supports a fixed amount of slabs, as determined
// by MaxAllocs, and statically pre-allocates all the metadata for those slabs. The metadata is 8
// bytes per slab.
//
// This allocator guarantees that all IDs are within [0, MaxAllocs). To achieve this it is a
// restriction that the maximum number of requested allocations be a multiple of the number of
// allocations per slab.
//
// Due to the static overhead per potential slab required this object can become quite large as
// MaxAllocs grows. Users should check the resulting size of the object, based on the provided
// allocation size and MaxAllocs, to ensure it is within acceptable limits.
//
// Most likely this allocator is not what you are looking for, and you want the IdSlabAllocator.
template <size_t AllocSize, size_t MaxAllocs>
class FixedIdSlabAllocator final : public BaseIdSlabAllocator<AllocSize> {
 public:
  static_assert(MaxAllocs % BaseIdSlabAllocator<AllocSize>::kAllocsPerSlab == 0);
  FixedIdSlabAllocator() = default;
  ~FixedIdSlabAllocator() {
    ASSERT(ktl::all_of(slabs_.begin(), slabs_.end(), [](vm_page_t* p) { return p == nullptr; }));
  }

 private:
  vm_page_t* AllocSlab() override {
    // Find an unused slot/id.
    auto it = ktl::find(slabs_.begin(), slabs_.end(), nullptr);
    if (it == slabs_.end()) {
      return nullptr;
    }
    vm_page_t* slab = BaseIdSlabAllocator<AllocSize>::AllocSlab();
    if (!slab) {
      return nullptr;
    }
    const uint32_t id = static_cast<uint32_t>(ktl::distance(slabs_.begin(), it));
    slab->slab.id = id;
    *it = slab;
    return slab;
  }
  void FreeSlab(vm_page_t* slab) override {
    DEBUG_ASSERT(slab->state() == vm_page_state::SLAB);
    DEBUG_ASSERT(slabs_.at(slab->slab.id) == slab);
    slabs_.at(slab->slab.id) = nullptr;
    BaseIdSlabAllocator<AllocSize>::FreeSlab(slab);
  }
  vm_page_t* IdToSlab(uint32_t slab_id) const override {
    vm_page_t* slab = slabs_.at(slab_id);
    DEBUG_ASSERT(slab);
    return slab;
  }
  ktl::array<vm_page_t*, BaseIdSlabAllocator<AllocSize>::SlabsRequired(MaxAllocs)> slabs_ = {
      nullptr};
};

// Slab allocator that can convert each allocation to/from a numerical ID, with the added guarantee
// that all IDs fall within [0, MaxAllocs). This implies that only up to MaxAllocs can be
// allocated at one time. To achieve this it is a restriction that the maximum number of requested
// allocations be a common multiple of the number of allocations per slab and per ID slab.
//
// The properties of this allocator are otherwise the same as the PageSlabAllocator, with the only
// dependency being direct PMM allocations, and slabs able to be cleaned up if they become fully
// free.
//
// This slab allocator uses the FixedIdSlabAllocator internally to allocate slab IDs. This works as
// follows:
// 1. Whenever this slab allocator allocates a slab, the `vm_page_t*` pointing to that slab is
//    stored in the FixedIdSlabAllocator.
// 2. By construction, each slab in the FixedIdSlabAllocator has an ID that we can convert into a
//    slab. Thus, using the AllocToId function allows us to convert the `vm_page_t**` into a stable
//    ID that we can assign as the slab's ID in this allocator.
// We do this to minimize the amount of metadata needed to store the IDs. Notice that using a
// FixedIdSlabAllocator directly would increase the allocation size by 8 bytes for every slab we
// allocate. By introducing this level of indirection, we are able to add a slab to the
// FixedIdSlabAllocator only when we have (8 / kPageSize) slabs allocated in this allocator. We can
// fit (sizeof(T) / kPageSize) allocations in each slab in this allocator, so this means that the
// size of the underlying FixedIdSlabAllocator will grow at a rate of
// (8 * sizeof(T)) / (kPageSize * kPageSize) bytes per allocation added.
//
// The rate of growth of the static metadata can be further reduced by adding additional levels of
// indirection for the slab id allocation by overriding the SLAB template argument.
template <size_t AllocSize, size_t MaxAllocs,
          typename SLAB = FixedIdSlabAllocator<
              sizeof(vm_page_t*), BaseIdSlabAllocator<AllocSize>::SlabsRequired(MaxAllocs)>>
class IdSlabAllocator final : public BaseIdSlabAllocator<AllocSize> {
 public:
  static_assert(MaxAllocs % BaseIdSlabAllocator<AllocSize>::kAllocsPerSlab == 0);
  IdSlabAllocator() = default;
  ~IdSlabAllocator() = default;

  // Total memory usage, in bytes, including any unused portions of slabs, not including the size of
  // |this|.
  size_t MemoryUsage() const {
    return (slab_id_allocator_.AllocatedSlabs() +
            BaseIdSlabAllocator<AllocSize>::AllocatedSlabs()) *
           kPageSize;
  }

 private:
  vm_page_t* AllocSlab() override {
    // Allocate a new slot to store the slab from the id allocator.
    vm_page_t** slab_ref = slab_id_allocator_.template allocate_object<vm_page_t*>();
    if (!slab_ref) {
      return nullptr;
    }
    vm_page_t* slab = BaseIdSlabAllocator<AllocSize>::AllocSlab();
    if (!slab) {
      slab_id_allocator_.deallocate_object(slab_ref);
      return nullptr;
    }
    *slab_ref = slab;
    // The object id of our slot is our slab id, allowing us to go from id->vm_page_t* later.
    const uint32_t slab_id = slab_id_allocator_.AllocToId(slab_ref);
    slab->slab.id = slab_id;
    return slab;
  }
  void FreeSlab(vm_page_t* slab) override {
    // Return the slot used to store slab to the slab id allocator.
    vm_page_t** slab_ref =
        reinterpret_cast<vm_page_t**>(slab_id_allocator_.IdToAlloc(slab->slab.id));
    DEBUG_ASSERT(*slab_ref == slab);
    *slab_ref = nullptr;
    slab_id_allocator_.deallocate_object(slab_ref);
    BaseIdSlabAllocator<AllocSize>::FreeSlab(slab);
  }
  vm_page_t* IdToSlab(uint32_t slab_id) const override {
    return *reinterpret_cast<vm_page_t**>(slab_id_allocator_.IdToAlloc(slab_id));
  }

  // The slab id allocator both allocates our unique slab ids, and provides the storage / mapping
  // from slab_id->vm_page_t*. Importantly it guarantees that its IDs are from [0..kMaxSlabs), which
  // we need to then guarantee that our final IDs are from [0..MaxAllocs).
  SLAB slab_id_allocator_;
};

// An example of overriding the SLAB argument to provide an additional level of indirection for the
// ID allocation, limiting the growth of metadata by another multiple of the kPageSize.
template <size_t AllocSize, size_t MaxAllocs>
using TwoLevelIdSlabAllocator = IdSlabAllocator<
    AllocSize, MaxAllocs,
    IdSlabAllocator<sizeof(vm_page_t*), BaseIdSlabAllocator<AllocSize>::SlabsRequired(MaxAllocs)>>;

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_SLAB_ALLOCATOR_H_
