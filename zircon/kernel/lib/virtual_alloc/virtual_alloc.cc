// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/virtual_alloc.h"

#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <lib/zircon-internal/align.h>

#include <fbl/algorithm.h>
#ifdef _KERNEL
#include <trace.h>

#include <vm/arch_vm_aspace.h>
#include <vm/pmm.h>
#include <vm/vm_aspace.h>
#else
#include <sys/mman.h>
#include <unistd.h>
// Host systems may not have fixed page size definitions at user space and so we declare a page size
// here and will check it at run time.
#define kPageShift HOST_PAGE_SIZE_SHIFT
#define kPageSize (1ul << kPageShift)
#define LTRACEF(...) \
  do {               \
  } while (0)
#endif

#include <algorithm>

#define LOCAL_TRACE 0

VirtualAlloc::VirtualAlloc(vm_page_state allocated_page_state)
    : allocated_page_state_(allocated_page_state) {
#ifndef _KERNEL
  // Check that the system page size is what we assume the page size to be. This is necessary as
  // mprotect etc require page aligned ranges.
  ZX_ASSERT(sysconf(_SC_PAGE_SIZE) == kPageSize);
  // allocated_page_state_ is only used in kernel builds so add a synthetic
  // reference to prevent compilation warnings in host builds.
  (void)allocated_page_state_;
#endif
}

VirtualAlloc::~VirtualAlloc() { Destroy(); }

zx_status_t VirtualAlloc::Init(vaddr_t base, size_t size, size_t alloc_guard, size_t align_log2) {
  canary_.Assert();

  if (alloc_base_ != 0) {
    // This has already been initialized.
    return ZX_ERR_BAD_STATE;
  }

  if (align_log2 < kPageShift) {
    return ZX_ERR_INVALID_ARGS;
  }
  align_log2_ = align_log2;

  const size_t vaddr_align = 1ul << align_log2_;

  if (size == 0 || !ZX_IS_ALIGNED(size, vaddr_align) || !ZX_IS_ALIGNED(base, vaddr_align) ||
      base + size < base) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Work how how many pages we need for the bitmap.
  const size_t total_pages = size / kPageSize;
  constexpr size_t kBitsPerPage = kPageSize * CHAR_BIT;
  const size_t bitmap_pages = fbl::round_up(total_pages, kBitsPerPage) / kBitsPerPage;

  // Validate that there will be anything left after allocating the bitmap for an actual allocation.
  // A single allocation needs padding on both sides of it. This ignores alignment problems caused
  // by the bitmap, and so it's still possible for non page size alignments that if this check
  // passes that no allocations are possible, but this is not meant to be an exhaustive guard.
  if (bitmap_pages + alloc_guard * 2 >= total_pages) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Allocate and map the bitmap pages into the start of the range we were given.
  zx_status_t status = AllocMapPages(base, bitmap_pages);
  if (status != ZX_OK) {
    return status;
  }
  bitmap_.StorageUnsafe()->Init(reinterpret_cast<void *>(base), bitmap_pages * kPageSize);

  // Initialize the bitmap, reserving its own pages.
  alloc_base_ = base;
  bitmap_.Reset(total_pages);
  bitmap_.Set(0, bitmap_pages);

  // Set our first search to happen after the bitmap.
  next_search_start_ = bitmap_pages;

  alloc_guard_ = alloc_guard;
  return ZX_OK;
}

zx::result<size_t> VirtualAlloc::BitmapAllocRange(size_t num_pages, size_t start, size_t end) {
  ZX_DEBUG_ASSERT(end >= start);
  ZX_DEBUG_ASSERT(num_pages > 0);
  const size_t align_pages = 1ul << (align_log2_ - kPageShift);
  // Want to find a run of num_pages + padding on either end. By over-searching we can ensure there
  // is always alloc_guard_ unused pages / unset-bits between each allocation.
  const size_t find_pages = num_pages + alloc_guard_ * 2;

  // Helper to finalize an allocated range once found. This just allows for structuring the code in
  // a slightly more readable way for the two different kinds of searches.
  auto complete_alloc = [this, num_pages](size_t start_index) {
    // Increase our start to skip the padding we want to leave.
    start_index += alloc_guard_;
    // Record the end of this allocation as our next search start. We set the end to not include the
    // padding so that the padding at the end of this allocation becomes the padding at the start of
    // the next one.
    next_search_start_ = start_index + num_pages;
    // Set the bits for the 'inner' allocation, leaving the padding we found unset.
    bitmap_.Set(start_index, start_index + num_pages);
    return zx::ok(start_index);
  };

  // If requested less pages than the alignment then do not bother finding an aligned range, just
  // find anything. The assumption here is that the block of pages we map in later will not be large
  // enough to benefit from any alignment, so might as well avoid fragmentation and do a more
  // efficient search.
  if (num_pages >= align_pages && align_pages > 1) {
    size_t current_start = start;
    while (true) {
      // Construct a candidate range from the start. This candidate needs to be chosen such that
      // after skipping the allocation padding it is aligned.
      size_t candidate = fbl::round_up(current_start, align_pages);
      if (candidate - current_start < alloc_guard_) {
        // Add on sufficient alignment multiples that we will be able to subtract alloc_guard_
        // without ending up below start.
        candidate += fbl::round_up(alloc_guard_, align_pages);
      }
      candidate -= alloc_guard_;
      // If the range from the candidate would exceed our search range, then no aligned range.
      if (candidate + find_pages > end) {
        break;
      }
      // Scan from the candidate and see if all the bits are clear.
      size_t set_bit = 0;
      if (bitmap_.Scan(candidate, candidate + find_pages, false, &set_bit)) {
        return complete_alloc(candidate);
      }
      // Search from the bit that was set to find the next unset bit. This will become our next
      // starting search location.
      size_t next_start = 0;
      if (bitmap_.Scan(set_bit, end, true, &next_start)) {
        // If all the bits are set then no aligned range.
        break;
      }
      ZX_DEBUG_ASSERT(next_start > current_start);
      current_start = next_start;
    }
  }
  // See if there's an unaligned range that will satisfy.
  size_t alloc_start = 0;
  zx_status_t status = bitmap_.Find(false, start, end, find_pages, &alloc_start);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return complete_alloc(alloc_start);
}

zx::result<size_t> VirtualAlloc::BitmapAlloc(size_t num_pages) {
  // First search from our saved recommended starting location.
  zx::result<size_t> result = BitmapAllocRange(num_pages, next_search_start_, bitmap_.size());
  if (result.is_error()) {
    // Try again from the beginning (skipping the bitmap itself). Still search to the end just in
    // case the original search start was in the middle of a free run.
    return BitmapAllocRange(num_pages, BitmapPages(), bitmap_.size());
  }
  return result;
}

zx::result<vaddr_t> VirtualAlloc::AllocPages(size_t pages) {
  canary_.Assert();

  if (unlikely(alloc_base_ == 0)) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (unlikely(pages == 0)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Allocate space from the bitmap, it will set the bits and ensure padding is left around the
  // allocation.
  zx::result<size_t> alloc_result = BitmapAlloc(pages);
  if (unlikely(alloc_result.is_error())) {
    return alloc_result.take_error();
  }
  const size_t start = alloc_result.value();

  // Turn the bitmap index into a virtual address and allocate the pages there.
  const vaddr_t vstart = alloc_base_ + start * kPageSize;
  zx_status_t status = AllocMapPages(vstart, pages);
  if (status != ZX_OK) {
    // Return the range back to the bitmap.
    BitmapFree(start, pages);
    return zx::error(status);
  }
  LTRACEF("Allocated %zu pages at %p\n", pages, (void *)vstart);
  return zx::ok(vstart);
}

void VirtualAlloc::BitmapFree(size_t start, size_t num_pages) {
  ZX_ASSERT(start >= BitmapPages());
  ZX_ASSERT(start < bitmap_.size());
  ZX_ASSERT(num_pages <= bitmap_.size() - start);
  ZX_DEBUG_ASSERT(bitmap_.Scan(start, start + num_pages, true, nullptr));

  zx_status_t status = bitmap_.Clear(start, start + num_pages);
  ZX_ASSERT(status == ZX_OK);
  if (start < next_search_start_) {
    next_search_start_ = start;
    // To attempt to keep allocations compact check alloc_guard_ bits backwards, and move our
    // search start if unset. This ensures that if we alloc+free that our search_start_ gets reset
    // to the original location, otherwise it will constantly creep by alloc_guard_.
    if (next_search_start_ >= alloc_guard_) {
      size_t candidate = 0;
      if (bitmap_.ReverseScan(next_search_start_ - alloc_guard_, next_search_start_, false,
                              &candidate)) {
        LTRACEF("Reverse scan moved search from %zu all the way to %zu\n", next_search_start_,
                next_search_start_ - alloc_guard_);
        next_search_start_ -= alloc_guard_;
      } else {
        LTRACEF("Reverse scan moved search from %zu part way to %zu\n", next_search_start_,
                candidate + 1);
        next_search_start_ = candidate + 1;
      }
    }
  }
}

void VirtualAlloc::FreePages(vaddr_t vaddr, size_t pages) {
  ZX_ASSERT(alloc_base_ != 0);
  ZX_ASSERT(pages > 0);
  ZX_ASSERT(IsPageRounded(vaddr));
  canary_.Assert();

  LTRACEF("Free %zu pages at %p\n", pages, (void *)vaddr);

  // Release the bitmap range prior to unmapping to ensure any attempts to free an invalid range are
  // caught before attempting to unmap 'random' memory.
  ZX_ASSERT(vaddr >= alloc_base_);
  BitmapFree((vaddr - alloc_base_) / kPageSize, pages);
  UnmapFreePages(vaddr, pages);
}

void VirtualAlloc::UnmapFreePages(vaddr_t vaddr, size_t pages) {
#ifdef _KERNEL
  VmPageDoublyLinkedList free_list;
  LTRACEF("Unmapping %zu pages at %" PRIxPTR "\n", pages, vaddr);
  for (size_t i = 0; i < pages; i++) {
    paddr_t paddr;
    zx_status_t status =
        VmAspace::kernel_aspace()->arch_aspace().Query(vaddr + i * kPageSize, &paddr, nullptr);
    ZX_ASSERT(status == ZX_OK);
    vm_page_t *page = paddr_to_vm_page(paddr);
    ZX_ASSERT(page);
    free_list.push_back(page);
  }
  // As this is the kernel aspace we cannot tolerate enlarging the operation, as other CPUs may be
  // accessing the nearby portions and we cannot trigger faults.
  zx_status_t status = VmAspace::kernel_aspace()->arch_aspace().Unmap(
      vaddr, pages, ArchVmAspaceInterface::ArchUnmapOptions::None);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to unmap %zu pages at %" PRIxPTR "", pages, vaddr);
  pmm_free(&free_list);
#else
  int result = mprotect(reinterpret_cast<void *>(vaddr), pages * kPageSize, PROT_NONE);
  ZX_ASSERT(result == 0);
  result = madvise(reinterpret_cast<void *>(vaddr), pages * kPageSize, MADV_DONTNEED);
  ZX_ASSERT(result == 0);
#endif
}

zx_status_t VirtualAlloc::AllocMapPages(vaddr_t vaddr, size_t num_pages) {
#ifdef _KERNEL
  constexpr uint kMmuFlags =
      ARCH_MMU_FLAG_CACHED | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
  ZX_ASSERT(num_pages > 0);
  VmPageDoublyLinkedList alloc_pages;

  size_t mapped_count = 0;

  auto cleanup = fit::defer([&mapped_count, &alloc_pages, vaddr]() {
    if (mapped_count > 0) {
      zx_status_t status = VmAspace::kernel_aspace()->arch_aspace().Unmap(
          vaddr, mapped_count, ArchVmAspaceInterface::ArchUnmapOptions::None);
      ZX_ASSERT(status == ZX_OK);
    }
    if (!alloc_pages.is_empty()) {
      pmm_free(&alloc_pages);
    }
  });

  if (CanAttemptContiguousMappings()) {
    const size_t align_pages = 1ul << (align_log2_ - kPageShift);
    if (align_pages > 1) {
      while (mapped_count + align_pages <= num_pages) {
        paddr_t paddr;
        VmPageDoublyLinkedList contiguous_pages;
        // Being in this path we know that align_pages is >1, which can only happen if our
        // align_log_2 is greater than the system kPageShift. As such we need to allocate
        // multiple contiguous pages at a greater than system page size alignment, and so we must
        // use the general pmm_alloc_contiguous.
        zx_status_t status =
            pmm_alloc_contiguous(align_pages, 0, (uint8_t)align_log2_, &paddr, &contiguous_pages);
        if (status != ZX_OK) {
          // Failing to allocate a contiguous block isn't an error, as the pmm could just be
          // fragmented. Drop out of this loop and attempt to finish with the single page mappings.
          break;
        }
        for (auto &p : contiguous_pages) {
          p.set_state(allocated_page_state_);
        }
        alloc_pages.splice(alloc_pages.begin(), contiguous_pages);
        status = VmAspace::kernel_aspace()->arch_aspace().MapContiguous(
            vaddr + mapped_count * kPageSize, paddr, align_pages, kMmuFlags);
        if (status != ZX_OK) {
          return status;
        }
        mapped_count += align_pages;
      }
      if (mapped_count == num_pages) {
        cleanup.cancel();
        alloc_pages.clear();
        return ZX_OK;
      }
    }
  }

  // Allocate any remaining pages.
  VmPageDoublyLinkedList remaining_pages;
  zx_status_t status = pmm_alloc_pages(num_pages - mapped_count, 0, &remaining_pages);
  if (status != ZX_OK) {
    return status;
  }

  vm_page_t *current_page = &remaining_pages.front();
  ZX_ASSERT(current_page);

  // Place them specifically at the end of any already allocated pages. This ensures that if we
  // should iterate too far we will hit a null page and not one of our contiguous pages to ensure we
  // can never attempt to map something twice. Due to how list_node's work this does not affect the
  // current_page pointer we already retrieved.
  alloc_pages.splice(alloc_pages.end(), remaining_pages);

  while (mapped_count < num_pages) {
    constexpr size_t kBatchPages = 128;
    paddr_t paddrs[kBatchPages] __UNINITIALIZED;
    const size_t map_pages = std::min(kBatchPages, num_pages - mapped_count);
    ZX_ASSERT(map_pages > 0);
    for (size_t page = 0; page < map_pages; page++) {
      ZX_ASSERT(current_page);
      current_page->set_state(allocated_page_state_);
      paddrs[page] = current_page->paddr();
      auto iter = ++alloc_pages.make_iterator(*current_page);
      current_page = iter.IsValid() ? &*iter : nullptr;
    }

    status = VmAspace::kernel_aspace()->arch_aspace().Map(vaddr + mapped_count * kPageSize, paddrs,
                                                          map_pages, kMmuFlags,
                                                          ArchVmAspace::ExistingEntryAction::Error);
    if (status != ZX_OK) {
      return status;
    }
    mapped_count += map_pages;
  }
  // If we successfully mapped everything we should have iterated all the way to the end of the
  // pages we allocated.
  ZX_ASSERT(!current_page);

  cleanup.cancel();
  alloc_pages.clear();

  LTRACEF("Mapped %zu pages at %" PRIxPTR "\n", num_pages, vaddr);
#else
  int result =
      mprotect(reinterpret_cast<void *>(vaddr), num_pages * kPageSize, PROT_READ | PROT_WRITE);
  ZX_ASSERT(result == 0);
  memset(reinterpret_cast<void *>(vaddr), 0, num_pages * kPageSize);
#endif
  return ZX_OK;
}

void VirtualAlloc::Destroy() {
  canary_.Assert();
  // No need to destroy if not yet initialized.
  if (alloc_base_ == 0) {
    return;
  }

  const size_t bitmap_pages = BitmapPages();
  // Check that all allocated blocks were freed. Outstanding allocations indicate something is still
  // holding a reference that they will try and use later, so we have no choice but to fail.
  // There are more optimal ways to track outstanding allocations, but as destroying allocators is
  // considered an extremely uncommon operation (largely just when running tests) this O(N) scan is
  // fine. This check needs to ignore the pages for the bitmap itself, as they should still be set.
  ZX_ASSERT(bitmap_.Scan(bitmap_pages, bitmap_.size(), false, nullptr));

  // Release the pages backing the bitmap.
  UnmapFreePages(alloc_base_, bitmap_pages);
  alloc_base_ = 0;
}

bool VirtualAlloc::CanAttemptContiguousMappings() const {
  // Ideally we would like to create the chance of using large pages with aligned allocations and
  // MapContiguous, but we can only do this if we can later safely unmap without enlarging the
  // range.
  return VmAspace::kernel_aspace()->arch_aspace().UnmapOnlyEnlargeOnOom();
}

void VirtualAlloc::DebugFreeAllAllocations() {
  canary_.Assert();
  ZX_DEBUG_ASSERT(alloc_base_);

  const size_t bitmap_pages = BitmapPages();

  size_t allocated_page = bitmap_pages;
  while (!bitmap_.Scan(allocated_page, bitmap_.size(), false, &allocated_page)) {
    FreePages(alloc_base_ + allocated_page * kPageSize, 1);
  }
}

void VirtualAlloc::DebugAllocateVaddrRange(vaddr_t vaddr, size_t num_pages) {
  canary_.Assert();
  ZX_ASSERT(IsPageRounded(vaddr));
  ZX_ASSERT(num_pages > 0);
  ZX_ASSERT(vaddr >= alloc_base_ + BitmapPages() * kPageSize);

  const size_t index = (vaddr - alloc_base_) >> kPageShift;

  ZX_ASSERT(bitmap_.Scan(index, index + num_pages, false, nullptr));
  bitmap_.Set(index, index + num_pages);
  AllocMapPages(vaddr, num_pages);
}

size_t VirtualAlloc::BitmapPages() const {
  canary_.Assert();
  ZX_ASSERT(alloc_base_ != 0);

  return bitmap_.StorageUnsafe()->GetSize() / kPageSize;
}
