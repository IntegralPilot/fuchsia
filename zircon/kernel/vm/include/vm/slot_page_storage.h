// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_SLOT_PAGE_STORAGE_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_SLOT_PAGE_STORAGE_H_

#include <lib/page/size.h>

#include <arch/kernel_aspace.h>
#include <fbl/canary.h>
#include <vm/compression.h>
#include <vm/page_slab_allocator.h>

// Slot-page storage is an allocator optimized around the expectations, and distributions of,
// compressed pages. At the high level it breaks a page into 64 equal slots that can be used to
// store data.
//
// The overall operation is closest to a buddy allocator where a page is broken into 64 slots, and
// kept in a list indexed by the largest contiguous run of free slots. This largest contiguous run
// represents the largest allocation supported by the page.
//
// As free space in the page is measured only in slots, any allocation is rounded up to the next
// slot size multiple, causing wastage due to internal fragmentation. Performing an allocation is
// done by finding a page that has at least as many contiguous free slots as the size of the
// allocation, allocating the run of slots, and then re-calculating the potentially changed
// contiguous run and placing in the correct list.
//
// There are three relevant data structures for this system:
//  1. vm_page_t free_block_mask: This is a bitmap, stored directly in the vm_page_t, that tracks
//     exactly which slots are free and which slots are allocated.
//  2. Contiguous free slot list: For every possible slot length [0, 64) there is a linked list of
//     pages for which that is their largest run of contiguous free slots. This is just a cache of
//     what can always be calculated from the free_block_mask, but this serves to provide a way to
//     find a valid page to allocate from in O(1) time, instead of scanning all pages.
//  3. `Data` allocation: This is the metadata about the specific allocation being tracked and
//     points to the vm_page_t that contains the allocation, the slot range being used, and the
//     true (unrounded) length of the data. This metadata is separately allocated using a slab
//     allocator, instead of the heap, that so that a reference to it that fits in a CompressedRef
//     can be returned to the user.
//
// Compressed pages, by number, tend to have a reasonably even distribution over the different size
// buckets. However, when weighted by cumulative size, there is therefore more data to store at
// larger compressed sizes. The use of slots allows for a balance of being able to store these
// large items, and then packing in multiple smaller items around it, as well as being able to
// store large amount of small items in a single page if need be.
//
// The use of 64 slots, and the resulting 64-byte slot size for 4k pages, is a trade-off in the
// resulting fragmentation due to rounding, and the efficiency of being able to track slots in a
// bitmap that fits in a single machine word.

class VmSlotPageStorage final : public VmCompressedStorage {
 public:
  VmSlotPageStorage();
  ~VmSlotPageStorage() final;

  void Free(CompressedRef ref) final;
  std::pair<ktl::optional<CompressedRef>, vm_page_t*> Store(vm_page_t* page, size_t len) final;
  ktl::tuple<const void*, uint32_t, size_t> CompressedData(CompressedRef ref) const final;

  uint32_t GetMetadata(CompressedRef ref) final;
  void SetMetadata(CompressedRef ref, uint32_t metadata) final;

  void Dump() const final;
  MemoryUsage GetMemoryUsage() const final;

  // Query specific breakdown of the memory usage used internally by the storage system. Intended to
  // be used by tests.
  struct InternalMemoryUsage {
    size_t data_bytes;
    size_t metadata_bytes;
  };
  InternalMemoryUsage GetInternalMemoryUsage() const;

  // We want to use a single word to store the bitmap for slot usage, so the number of available
  // slots is therefore just the number of bits in a word.
  // From this can calculate the size of each slot and how many bits are needed to represent an
  // index into these two.
  static constexpr size_t kNumSlots = sizeof(uint64_t) * 8;
  static constexpr size_t kSlotSize = kPageSize / kNumSlots;
  static constexpr size_t kNumSlotBits = log2_floor(kNumSlots);
  static constexpr size_t kSlotSizeBits = log2_floor(kSlotSize);

 private:
  // Used to track a single allocation of the underlying storage. References to this are what is
  // returned in the `CompressedRef`, and these are allocated out of a slab allocator.
  struct Allocation {
    // The original (non-rounded) size in bytes of the data being stored.
    uint64_t byte_size() const {
      DEBUG_ASSERT(num_slots > 0);
      DEBUG_ASSERT(last_slot_bytes > 0 && last_slot_bytes <= kSlotSize);
      return (static_cast<uint64_t>(num_slots - 1) * kSlotSize) +
             static_cast<uint64_t>(last_slot_bytes);
    }
    // Base address of the raw data.
    void* data() const {
      return reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(paddr_to_physmap(page->paddr())) +
                                     (static_cast<uint64_t>(slot_start) * kSlotSize));
    }
    // Reference to the page being used by this allocation.
    vm_page_t* page = nullptr;
    // Offset of the start slot in the page, zero indexed.
    uint8_t slot_start = 0;
    // Number of slots referenced by this data. By definition zero sized data may not be stored,
    // and so this must always be > 0.
    uint8_t num_slots = 0;
    // As we're using 8-bit values, ensure that a slot index or offset will not overflow.
    static_assert(kNumSlotBits <= 8);
    // Amount of data stored in the last slot. This is between [1, kSlotSize)
    uint16_t last_slot_bytes = 0;
    static_assert(kSlotSizeBits <= 16);
    // Metadata that we are obligated to track for the user for each allocation.
    uint32_t metadata = 0;
  };
  static_assert(sizeof(Allocation) == 16);

  CompressedRef AllocToRefLocked(const Allocation* alloc) const TA_REQ(lock_) {
    static_assert(ktl::bit_width(kMaxStorageItems) + CompressedRef::kAlignBits <= 32);
    return CompressedRef(allocator_.AllocToId(alloc) << CompressedRef::kAlignBits);
  }
  Allocation* RefToAllocLocked(CompressedRef ref) const TA_REQ(lock_) {
    return reinterpret_cast<Allocation*>(
        allocator_.IdToAlloc(ref.value() >> CompressedRef::kAlignBits));
  }
  InternalMemoryUsage GetInternalMemoryUsageLocked() const TA_REQ(lock_);

  fbl::Canary<fbl::magic("SPS_")> canary_;

  mutable DECLARE_CRITICAL_MUTEX(VmSlotPageStorage) lock_;

  // Maintain a list of pages for every possible max contiguous free slots in those pages. The 0
  // entry, i.e. pages that have no free slots, represent completely full pages and are tracked for
  // consistency
  ktl::array<VmPageDoublyLinkedList, kNumSlots> max_contig_remain_ TA_GUARDED(lock_);

  // Every `Data` represents a page that has been compressed. Being able to store 2^24 `Data`
  // items means tracking 2^24 pages that have been compressed, which if uncompressed would
  // represent 64GiB of data.
  // Due to the implementation of the IdPageSlabAllocator, every doubling of the number of items
  // doubles the amount of fixed tracking that is needed. At 2^24 items this requires approx. 1KiB
  // of data, which is a very small fraction of the amount of compressed data we are able to track.
  // Still, this should not be sized arbitrarily high, as the doubling of size starts to become
  // meaningful, and is potentially wasting memory if not storing that many items.
  static constexpr size_t kMaxStorageItems = 1ul << 24;
  IdSlabAllocator<sizeof(Allocation), kMaxStorageItems> allocator_ TA_GUARDED(lock_);

  // Informational counters not required for operation, but used to provide statistics.
  size_t stored_items_ TA_GUARDED(lock_) = 0;
  size_t total_compressed_item_size_ TA_GUARDED(lock_) = 0;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_SLOT_PAGE_STORAGE_H_
