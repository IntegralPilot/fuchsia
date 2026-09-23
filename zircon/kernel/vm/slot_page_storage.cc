// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page/size.h>

#include <ktl/memory.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/slot_page_storage.h>

// The following helper methods are used for manipulating the free_block_mask. Slots are tracked
// where a bit being set indicates the block is not allocated, and being clear means it is
// allocated.
namespace {
constexpr size_t kNumSlots = VmSlotPageStorage::kNumSlots;
constexpr size_t kSlotSize = VmSlotPageStorage::kSlotSize;

// Given a zero indexed starting slot, and a non-zero length, returns a bitmask with those slots
// set to true.
constexpr uint64_t SlotMask(uint8_t base, uint8_t len) {
  DEBUG_ASSERT(base < kNumSlots && len > 0 && base + len <= kNumSlots);
  // Need to handle len == kNumSlots separately to avoid an undefined shift.
  if (len == kNumSlots) {
    return UINT64_MAX;
  }
  return ((1ul << len) - 1) << base;
}

// Given a free block mask, finds the longest contiguous run of free bits, and returns the length
// of that run.
constexpr uint8_t ContigFree(uint64_t free_mask) {
  // Handle the free_mask being all 1's separately to avoid an undefined shift in the loop.
  if (free_mask == UINT64_MAX) {
    return kNumSlots;
  }
  uint8_t max_free = 0;
  // Remove every run of 0's and 1's until the mask is fully processed, remembering the longest
  // run seen.
  while (free_mask) {
    free_mask >>= ktl::countr_zero(free_mask);
    const uint8_t ones = static_cast<uint8_t>(ktl::countr_one(free_mask));
    max_free = ktl::max(max_free, ones);
    free_mask >>= ones;
  }
  return max_free;
}

// Returns the offset in free_mask that has a contiguous run of free (i.e. set bits) slots of at
// least |len|. It is an error to call this if there is not such a run available.
constexpr uint8_t ContigBase(uint64_t free_mask, uint8_t len) {
  // Record how many shifts we have made, which will become the offset we return.
  uint8_t shifts = 0;
  const uint64_t target_mask = SlotMask(0, len);
  // TODO(adanis): Consider searching for the smallest contiguous run, instead of just the first
  // one, to minimize fragmentation.
  // Keep cycling past any runs of 0's, and insufficiently long runs of 1's, until we find a run
  // that fits the target mask.
  while ((free_mask & target_mask) != target_mask) {
    DEBUG_ASSERT(free_mask);
    const uint8_t ones = static_cast<uint8_t>(ktl::countr_one(free_mask));
    free_mask >>= ones;
    const uint8_t zeroes = static_cast<uint8_t>(ktl::countr_zero(free_mask));
    free_mask >>= zeroes;
    shifts += static_cast<uint8_t>(ones + zeroes);
  }
  return shifts;
}

// Given an item of byte size |len|, returns the number of slots required to store it.
constexpr uint8_t SlotsNeeded(uint64_t len) {
  DEBUG_ASSERT(len > 0 && len < kPageSize);
  return static_cast<uint8_t>(((len - 1) / kSlotSize) + 1);
}

constexpr uint16_t LastSlotBytes(uint64_t len) {
  DEBUG_ASSERT(len > 0);
  return ((len - 1) % kSlotSize) + 1;
}
}  // namespace

VmSlotPageStorage::VmSlotPageStorage() {}

VmSlotPageStorage::~VmSlotPageStorage() {
  ktl::ranges::for_each(max_contig_remain_, [](auto& list) { ASSERT(list.is_empty()); });
}

uint32_t VmSlotPageStorage::GetMetadata(CompressedRef ref) {
  canary_.Assert();
  Guard<CriticalMutex> guard{&lock_};
  return RefToAllocLocked(ref)->metadata;
}

void VmSlotPageStorage::SetMetadata(CompressedRef ref, uint32_t metadata) {
  canary_.Assert();
  Guard<CriticalMutex> guard{&lock_};
  RefToAllocLocked(ref)->metadata = metadata;
}

ktl::tuple<const void*, uint32_t, size_t> VmSlotPageStorage::CompressedData(
    CompressedRef ref) const {
  canary_.Assert();
  Guard<CriticalMutex> guard{&lock_};
  const Allocation* data = RefToAllocLocked(ref);
  return {data->data(), data->metadata, data->byte_size()};
}

std::pair<ktl::optional<VmCompressedStorage::CompressedRef>, vm_page_t*> VmSlotPageStorage::Store(
    vm_page_t* page, size_t len) {
  DEBUG_ASSERT(page);
  DEBUG_ASSERT(!page->queue_node.InContainer());

  canary_.Assert();
  Guard<CriticalMutex> guard{&lock_};

  // Require an |Allocation| for our metadata.
  Allocation* data = allocator_.allocate_object<Allocation>();
  if (!data) {
    return {ktl::nullopt, page};
  }
  ktl::construct_at(data);

  // Allocation cannot fail from here, so update our stats.
  total_compressed_item_size_ += len;
  stored_items_++;

  const uint8_t slots = SlotsNeeded(len);
  data->num_slots = slots;
  data->last_slot_bytes = LastSlotBytes(len);

  // Search for an existing page to add to first, preferring to break up the smallest contiguous
  // slot run possible.
  auto target_list = ktl::find_if_not(max_contig_remain_.begin() + slots, max_contig_remain_.end(),
                                      [](auto& list) { return list.is_empty(); });
  if (target_list == max_contig_remain_.end()) {
    // No existing page available, convert the provided |page| into a zram page.
    DEBUG_ASSERT(!page->queue_node.InContainer());
    page->set_state(vm_page_state::ZRAM);
    // |page| had the data starting at offset 0, so but bottom slots are initially allocated,
    // meaning the remainder are free.
    const uint8_t contig_free = kNumSlots - slots;
    page->zram.free_block_mask = SlotMask(slots, contig_free);
    data->slot_start = 0;
    data->page = page;
    max_contig_remain_[contig_free].push_front(page);
    // We have consumed |page|, so do not return it to the caller.
    return {AllocToRefLocked(data), nullptr};
  }

  // Found a non-empty list, remove a page from it under the assumption that its max contiguous
  // slot run will change and so it's going to move lists anyway.
  vm_page_t* target_page = target_list->pop_front();
  // Find that part of the free mask to allocate from, and remove those slots from it.
  const uint8_t base = ContigBase(target_page->zram.free_block_mask, slots);
  const uint64_t alloc_mask = SlotMask(base, slots);
  target_page->zram.free_block_mask &= ~alloc_mask;
  data->slot_start = base;
  data->page = target_page;
  // Copy the actual data.
  memcpy(data->data(), paddr_to_physmap(page->paddr()), len);
  // Re-insert the page into the, possibly changed, target list.
  const uint8_t contig_free = ContigFree(target_page->zram.free_block_mask);
  max_contig_remain_[contig_free].push_front(target_page);
  // We copied out of |page| so return it to the caller.
  return {AllocToRefLocked(data), page};
}

void VmSlotPageStorage::Free(CompressedRef ref) {
  canary_.Assert();
  vm_page_t* page = nullptr;
  {
    Guard<CriticalMutex> guard{&lock_};

    // Lookup the metadata for this allocation.
    Allocation* data = RefToAllocLocked(ref);
    DEBUG_ASSERT(data->page->queue_node.InContainer());
    page = data->page;

    // Update stats tracking.
    total_compressed_item_size_ -= data->byte_size();
    stored_items_--;

    // Save the old contig free slot count so we know which list the page was in.
    const uint64_t old_contig_free = ContigFree(page->zram.free_block_mask);

    // Add the slots for this allocation back to the free mask of the page and then can release the
    // |Allocation|.
    uint64_t free_mask = SlotMask(data->slot_start, data->num_slots);
    page->zram.free_block_mask |= free_mask;
    ktl::destroy_at(data);
    allocator_.deallocate_bytes(data);

    // Remove the page from its current list.
    max_contig_remain_[old_contig_free].erase(*page);
    const uint64_t contig_free = ContigFree(page->zram.free_block_mask);
    if (contig_free != kNumSlots) {
      max_contig_remain_[contig_free].push_front(page);
      // Page still in use, do not let it get freed.
      page = nullptr;
    }
  }
  // If we ended up with a completely free page, give it back to the PMM.
  if (page) {
    Pmm::Node().FreePage(page);
  }
}

VmSlotPageStorage::InternalMemoryUsage VmSlotPageStorage::GetInternalMemoryUsage() const {
  canary_.Assert();
  Guard<CriticalMutex> guard{&lock_};
  return GetInternalMemoryUsageLocked();
}

VmSlotPageStorage::InternalMemoryUsage VmSlotPageStorage::GetInternalMemoryUsageLocked() const {
  uint64_t data_bytes = 0;
  for (const auto& list : max_contig_remain_) {
    data_bytes += list.size_slow() * kPageSize;
  }
  const uint64_t metadata_bytes = allocator_.MemoryUsage();
  return InternalMemoryUsage{.data_bytes = data_bytes, .metadata_bytes = metadata_bytes};
}

VmCompressedStorage::MemoryUsage VmSlotPageStorage::GetMemoryUsage() const {
  canary_.Assert();
  Guard<CriticalMutex> guard{&lock_};
  InternalMemoryUsage usage = GetInternalMemoryUsageLocked();
  return MemoryUsage{.uncompressed_content_bytes = stored_items_ * kPageSize,
                     .compressed_storage_bytes = usage.data_bytes + usage.metadata_bytes,
                     .compressed_storage_used_bytes = total_compressed_item_size_};
}

void VmSlotPageStorage::Dump() const {
  canary_.Assert();

  MemoryUsage usage = GetMemoryUsage();
  printf("Storing %lu bytes compressed to %lu bytes using %lu bytes of storage\n",
         usage.uncompressed_content_bytes, usage.compressed_storage_used_bytes,
         usage.compressed_storage_bytes);
}
