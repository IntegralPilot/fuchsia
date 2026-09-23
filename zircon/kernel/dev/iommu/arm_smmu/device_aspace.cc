// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/cache.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <dev/arm_smmu/device_aspace.h>
#include <dev/arm_smmu/page_cache.h>
#include <dev/arm_smmu/translation_table_helper.h>
#include <dev/arm_smmu/vmsav8_64.h>
#include <dev/iommu/common.h>
#include <fbl/alloc_checker.h>
#include <vm/physmap.h>

namespace arm_smmu {

// Bring in the constants and helpers we need to work with VMSAv8-64 translation
// tables.
using namespace vmsav8_64;

DeviceAspace::~DeviceAspace() {
  // By the time that we destruct, our translation tables must have already been freed.
  ZX_DEBUG_ASSERT(root_tt_page_ == nullptr);
  ZX_DEBUG_ASSERT(page_cache_.cache_entries() == 0);
  ZX_DEBUG_ASSERT(page_cache_.in_flight_pages() == 0);
}

zx::result<ktl::unique_ptr<DeviceAspace>> DeviceAspace::Create(uint64_t aspace_start,
                                                               uint64_t aspace_len,
                                                               uint32_t max_cache_pages) {
  // Address spaces must have a non-zero length, and may not wrap.
  if (!aspace_len || ((aspace_start + aspace_len) < aspace_start)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::AllocChecker ac;
  ktl::unique_ptr<DeviceAspace> aspace{new (&ac)
                                           DeviceAspace(aspace_start, aspace_len, max_cache_pages)};
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // Allocate a page from the PMM to serve as the root translation table page.
  // If anything goes wrong after this, we will drop our DeviceAspace pointer,
  // and the page we allocated will be cleaned up during the object's
  // destruction.
  if (zx::result<vm_page_t*> maybe_root = aspace->page_cache_.GetPage(); !maybe_root.is_ok()) {
    return maybe_root.take_error();
  } else {
    aspace->root_tt_page_ = maybe_root.value();
  }

  // Attempt to add the region representing our valid address space to our
  // region allocator.
  const ralloc_region_t initial_region(aspace_start, aspace_len);
  if (const zx_status_t status = aspace->avail_regions_.AddRegion(initial_region);
      status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(ktl::move(aspace));
}

paddr_t DeviceAspace::GetRootPaddr() const {
  ZX_DEBUG_ASSERT(root_tt_page_ != nullptr);
  return root_tt_page_->paddr();
}

zx::result<DeviceAspace::Allocation> DeviceAspace::Map(const PinnedVmObject& pinned_vmo,
                                                       uint32_t perms, TlbInvalOp& tlb_inval_op,
                                                       ktl::optional<uint64_t> location) {
  // All requests have to be:
  //
  // 1) page aligned.
  // 2) integer multiple of our page size.
  // 3) for at least one page.
  //
  uint64_t offset = pinned_vmo.offset();
  const uint64_t size = pinned_vmo.size();
  if (((offset & kPageMask) != 0) || ((size & kPageMask) != 0) || (size == 0)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (location.has_value() && ((*location & kPageMask) != 0)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // I'm not sure what a legit maximum allocation size is, but 2^32 pages is too
  // much.
  if ((size >> kPageShift) > ktl::numeric_limits<uint32_t>::max()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // First, attempt to reserve a region in our address space bookkeeping.  If
  // our user has asked for a specific location, try to allocate the region
  // there.  Otherwise, just ask for any region which can fit our mapping.
  Allocation alloc;
  if (location.has_value()) {
    // If the base + size overflows the address space, then this is an invalid
    // request.
    const ralloc_region_t req = {.base = *location, .size = size};
    uint64_t exclusive_end;
    if (add_overflow(req.base, req.size, &exclusive_end)) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    // If the proposed region lies outside the aspace valid address range in any
    // way, return INVALID_ARGS instead of ALREADY_EXISTS, to make it clear that
    // the request was malformed, not that it collided with an existing
    // allocation.
    ZX_DEBUG_ASSERT(exclusive_end > 0);
    if ((req.base < first_valid_address()) || ((exclusive_end - 1) > last_valid_address())) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    if (const zx_status_t status = avail_regions_.GetRegion(req, alloc); status != ZX_OK) {
      // When the region allocator library cannot find a place for our
      // allocation, it will return ZX_ERR_NOT_FOUND.  If this happens when our
      // user is managing their own address space, return ZX_ERR_ALREADY_EXISTS
      // instead, to more clearly indicate a collision.
      return (status == ZX_ERR_NOT_FOUND) ? zx::error(ZX_ERR_ALREADY_EXISTS) : zx::error(status);
    }
  } else if (const zx_status_t status = avail_regions_.GetRegion(size, kPageSize, alloc);
             status != ZX_OK) {
    return zx::error(status);
  }

  // Now use our TranslationTableHelper helper to populate our translation
  // tables. If something goes wrong during this process, hand our allocation
  // off to Unmap to undo the changes we have made before we get out.
  auto cleanup = fit::defer([&]() { this->Unmap(ktl::move(alloc), tlb_inval_op); });

  const uint32_t page_total = static_cast<uint32_t>(size >> kPageShift);
  for (uint32_t i = 0; i < page_total; ++i) {
    // Start by looking up the physical address at our current offset.
    uint64_t phys_tgt{0};
    if (const zx_status_t res = pinned_vmo.vmo()->LookupContiguous(offset, kPageSize, &phys_tgt);
        res != ZX_OK) {
      return zx::error(res);
    }

    // If this is the first page, we need to initialize our helper with the
    // starting device virtual address.  Otherwise, we need to advance it.
    if (const zx::result<> res =
            (i == 0) ? tt_helper_.InitializeForMap(alloc->base) : tt_helper_.Advance();
        !res.is_ok()) {
      return zx::error(res.error_value());
    }

    // Success.  Create our new entry and advance our offset by a page.
    tt_helper_.AssignPageEntry(MakePageEntry(phys_tgt, perms));
    offset += kPageSize;
  }

  // We're done and success is now guaranteed.  Finish the operation, flushing
  // the pages for any levels which still have dirty entries from CPU cache to
  // physical memory, and invalidating the TLBs for our region in the process.
  tt_helper_.FinishOperation();
  tlb_inval_op.Invalidate(alloc->base, alloc->size);
  cleanup.cancel();
  return zx::ok(ktl::move(alloc));
}

void DeviceAspace::Unmap(Allocation alloc, TlbInvalOp& tlb_inval_op) {
  // Start by caching our allocation's device virtual address and size.  Then
  // return the allocation to the region allocator.
  //
  // Every time we remove a translation table entry from one of the translation
  // levels, we will mark the level as being dirty.  Every time we finish with
  // a page at a given level, either because we advance to the next page or
  // because we are at the end of the operation, we can test the region map to
  // see if the virtual address range covered by the page level is available for
  // allocation.  If it is, we know that the translation table page is no longer
  // in use by any other allocation, and we can put it on the list to return to
  // the PMM after TLB invalidation
  ZX_DEBUG_ASSERT(tlb_inval_op.is_valid());
  ZX_DEBUG_ASSERT((alloc->base & kPageMask) == 0);
  ZX_DEBUG_ASSERT((alloc->size & kPageMask) == 0);
  const uint64_t base = alloc->base;
  const uint64_t size = alloc->size;
  uint64_t addr = base;
  alloc.reset();

  // We don't permit mappings larger than 2^32 pages (and even that would be
  // pretty extreme).
  ZX_DEBUG_ASSERT((size >> kPageShift) < ktl::numeric_limits<uint32_t>::max());

  [[maybe_unused]] const zx::result<> init_res = tt_helper_.InitializeForUnmap(addr);
  ZX_DEBUG_ASSERT(init_res.is_ok());  // Init for an unmap can never fail.

  ZX_DEBUG_ASSERT(base + size > base);
  const uint64_t last = base + size;
  while (addr < last) {
    // If we have pages for all four translation levels, then go ahead and
    // invalidate the entry for this page, then advance the indices.
    //
    // Otherwise, we should be finished.  Why?  Because when we perform any map
    // operation, there are only two options.
    //
    // 1) We succeeded and we had pages in the translation table which cover the
    //    entire region from [map.start, map.end).
    // 2) We failed, and we only have pages covering [map.start, X) where
    //    X < map.end.
    //
    // In the case of #1 (a successful mapping), we should never hit a point in
    // the translation table where we are missing a page.  In the case of #2 (we
    // are cleaning up after a failed mapping), as soon as we hit either a
    // missing page, or an non-valid Page entry, we have to be done.
    //
    if (tt_helper_.CurrentPageEntryValid()) {
      tt_helper_.AssignPageEntry(0);
      [[maybe_unused]] const zx::result<> advance_res = tt_helper_.Advance();
      ZX_DEBUG_ASSERT(advance_res.is_ok());  // Advance during an unmap can never fail.
      addr += kPageSize;
    } else {
      break;
    }
  }

  // Finish the operation (flushing the CPU cache where needed), then invalidate
  // the TLBs for the region we just unmapped.  After this, there should be no
  // entries (either in physical memory or in TLB cache) which refer to any of
  // the pages we may have returned to the page cache.  We can now trim the page
  // cache and be done.
  tt_helper_.FinishOperation();
  tlb_inval_op.Invalidate(base, size);
  page_cache_.Trim(max_cache_pages_);
}

void DeviceAspace::FreeTranslationTablesHelper(vm_page_t* table_page, uint32_t level) {
  // We should never be passed a `nullptr` value for the table we are attempting
  // to free, and never recurse past the Table/Block descriptors level of
  // things (levels [0-2]).
  ZX_DEBUG_ASSERT(table_page);
  ZX_DEBUG_ASSERT(level < 3);

  // Our page is a table of 512 64-bit entries.  Set up an alias to easily
  // access the entries.
  uint64_t* table = static_cast<uint64_t*>(paddr_to_physmap(table_page->paddr()));

  // Check all of the entries in this page.  If any of them are valid, recurse
  // into the page if we have not hit the leaf-node level of things, and then
  // (either way) return the page to the PMM.
  for (uint32_t i = 0; i < kEntriesPerPage; ++i) {
    uint64_t e = table[i];
    if (IsValidEntry(e)) {
      ZX_DEBUG_ASSERT(IsTableEntry(e));

      const paddr_t next_pa = GetTableEntryPAddr(e);
      uint64_t* next_va = static_cast<uint64_t*>(paddr_to_physmap(next_pa));
      vm_page_t* next_vm_page = paddr_to_vm_page(next_pa);

      // Descend if needed.
      if (level < 2) {
        FreeTranslationTablesHelper(next_vm_page, level + 1);
      } else {
        // This is a level 2 entry pointing to a level 3 page.  We are not going
        // to walk every entry in the level 3 page, so explicitly zero out the
        // level 3 page before proceeding.
        ZX_DEBUG_ASSERT(level == 2);
        memset(next_va, 0, kPageSize);
      }

      // Zero out the entry;
      table[i] = 0;

      // Flush the (now zero) page, and add the page to our page cache.  We will
      // free the entire page cache all at once when we are done.  Note: the
      // fact that we are flushing all of this from leaf -> root is just an
      // artifact of our recursive depth first traversal, it is not important
      // for correctness.  Also note, this is rooted in the fact that we will never return
      // translation table pages to the PMM until after we have finished and
      // invalidated the TLBs.  Even if a TLB entry, or stale PTE entry
      // references a page, that page will be in the cache, not the PMM pool.
      arch::CleanDataCacheRange(reinterpret_cast<uintptr_t>(next_va), kPageSize);
      page_cache_.ReturnPage(next_vm_page);
    } else {
      // If an entry is not valid, we should have stored all zeros there.
      ZX_DEBUG_ASSERT_MSG(e == 0, "Bad Invalid Entry 0x%08lx (table %p, level %u, index %u)", e,
                          table, level, i);
    }
  }

  // If this is the top level, then the page we were given should be the root
  // page.  Flush it out of the CPU cache and add it back to the page cache,
  // then zero out the bookkeeping.
  if (level == 0) {
    ZX_DEBUG_ASSERT(root_tt_page_ == table_page);
    arch::CleanDataCacheRange(reinterpret_cast<uintptr_t>(table), kPageSize);
    page_cache_.ReturnPage(table_page);
    root_tt_page_ = nullptr;
  }
}

void DeviceAspace::FreeTranslationTables(TlbInvalOp& tlb_inval_op) {
  // Recursively zero-out all of the PTE levels.
  ZX_DEBUG_ASSERT(root_tt_page_ != nullptr);
  FreeTranslationTablesHelper(root_tt_page_, 0);

  // Then invalidate the TLBs and finally return all of the pages from the cache
  // to the PMM.
  tlb_inval_op.InvalidateAll();
  page_cache_.Trim(0);
}

}  // namespace arm_smmu
