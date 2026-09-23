// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/cache.h>
#include <stdint.h>
#include <zircon/assert.h>

#include <dev/arm_smmu/device_aspace.h>
#include <dev/arm_smmu/page_cache.h>
#include <dev/arm_smmu/translation_table_helper.h>
#include <dev/arm_smmu/vmsav8_64.h>
#include <ktl/memory.h>
#include <region-alloc/region-alloc.h>
#include <vm/page.h>
#include <vm/physmap.h>

namespace arm_smmu {

// Bring in the constants and helpers we need to work with VMSAv8-64 translation
// tables.
using namespace vmsav8_64;

zx::result<> TranslationTableHelper::Advance() {
  // Starting from level 3 and moving up to level 0, increment each index.  If
  // we roll over an index for a given level, flush the page to the PoC and
  // clear our internal bookkeeping.  We will obtain new pages as needed after
  // we are done advancing our indexes.
  uint32_t last_finished_level = kLevels;
  for (uint32_t i = kLevels; i > 0;) {
    // We can stop as soon as we don't wrap an index.
    //
    // Note: it should be safe to advance this index right now.  During an
    // unmap operation, FinishLevel is going to test to see if the level we
    // are finishing is no longer in use, which requires reconstructing the
    // device virtual address of the entry we are flushing, which depends only
    // on the indices of the levels before this one, not on the index for this
    // level.
    --i;
    if (++levels_[i].ndx <= kAddrBitsMask) {
      break;
    }

    // We have moved on to the next page.  Flush any changes we have made out
    // to the PoC with the SMMU hardware.  Additionally, if this is an unmap
    // operation, FinishLevel will also check the region allocator to see if
    // we can return the page to the PMM.
    //
    // Finally, FinishLevel will also move our index back to 0 and reset the
    // table pointer for us, as long as we are not at the root level.
    last_finished_level = i;
    FinishLevel(i);

    // We should never be in a position where we need a new root page.
    if (i == 0) {
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  // Re-populate pages as needed.
  for (uint32_t i = last_finished_level; i < kLevels; ++i) {
    zx::result<> res = FindPageForLevel(i);
    if (!res.is_ok()) {
      return res.take_error();
    }
  }

  return zx::ok();
}

void TranslationTableHelper::AssignPageEntry(uint64_t page_entry) {
  static_assert(kLevels > 0);
  Level& L = levels_[kLevels - 1];

  ZX_DEBUG_ASSERT(L.table != nullptr);
  ZX_DEBUG_ASSERT(L.ndx <= kAddrBitsMask);
  L.table[L.ndx] = page_entry;
  L.dirty = true;
}

void TranslationTableHelper::FinishOperation() {
  ZX_DEBUG_ASSERT_MSG(op_ != Op::Invalid, "Bad State %u", static_cast<uint32_t>(op_));

  // Finish each level, flushing any dirty pages and reclaiming any now-unused
  // pages if this is an UnmapOp.
  for (uint32_t i = kLevels; i > 0;) {
    FinishLevel(--i);
  }

  op_ = Op::Invalid;
  memset(&levels_, 0, sizeof(levels_));
}

// Returns true if the current index points to a valid Page entry.
bool TranslationTableHelper::CurrentPageEntryValid() {
  const Level& last_level = levels_[kLevels - 1];

  if (last_level.table == nullptr) {
    return false;
  }

  ZX_DEBUG_ASSERT(last_level.ndx <= kAddrBitsMask);
  const uint64_t entry = last_level.table[last_level.ndx];
  const bool valid = IsValidEntry(entry);
  ZX_DEBUG_ASSERT(valid || (entry == 0));

  return valid;
}

zx::result<> TranslationTableHelper::Initialize(Op op, uint64_t address) {
  // We expect only page aligned addresses
  ZX_DEBUG_ASSERT((address & ~kValidAddrMask) == 0);

  // The requested operation must either be a map or unmap operation.
  ZX_DEBUG_ASSERT(op != Op::Invalid);
  op_ = op;

  // Reset our level state, then compute the initial indices for each level of
  // the translation table and stash our root page.
  ZX_DEBUG_ASSERT(aspace_.root_tt_page_ != nullptr);
  memset(&levels_, 0, sizeof(levels_));
  levels_[0].table = static_cast<uint64_t*>(paddr_to_physmap(aspace_.root_tt_page_->paddr()));
  for (uint32_t i = 0; i < kLevels; ++i) {
    levels_[i].ndx =
        (address >> (kPageShift + (kAddrBitsPerLevel * (kLevels - i - 1)))) & kAddrBitsMask;
  }

  // Attempt to find translation table pages for each of our levels.  We'll
  // first look in in the existing translation table structure for pages already
  // allocated.  Failing that, if this is a Map operation, we'll try to get some
  // pages from the cache.
  for (uint32_t i = 1; i < kLevels; ++i) {
    zx::result<> res = FindPageForLevel(i);
    if (!res.is_ok()) {
      return res.take_error();
    }
  }

  return zx::ok();
}

void TranslationTableHelper::FinishLevel(uint32_t level_ndx) {
  ZX_DEBUG_ASSERT(level_ndx < kLevels);
  Level& l = levels_[level_ndx];

  // Nothing to do if we have no page for this level (something which should
  // only be possible during Unmap operations).
  if (l.table == nullptr) {
    ZX_DEBUG_ASSERT(op_ == Op::Unmap);
    return;
  }

  // If the page is dirty, flush the cache.
  if (l.dirty) {
    arch::CleanDataCacheRange(reinterpret_cast<uintptr_t>(l.table), kPageSize);
    l.dirty = false;
  }

  // If this is not the root level, return the page to the page pool if this is
  // an unmap operation and we don't need it anymore, and reset the table and
  // index bookkeeping no matter what.
  if (level_ndx != 0) {
    if (op_ == Op::Unmap) {
      // Compute the base device virtual address for the level we are finishing.
      // This is made from the indices of all the levels before us.
      ralloc_region_t r = {.base = 0, .size = 0};
      for (uint32_t i = 0; i < level_ndx; ++i) {
        const uint32_t shift = kPageShift + ((kLevels - i - 1) * kAddrBitsPerLevel);
        r.base |= uint64_t{levels_[i].ndx} << shift;
      }

      // Figure out the size of this level.  The granule is 4k, and every level
      // adds kAddrBitsPerLevel.
      r.size = uint64_t{1} << (kPageShift + ((kLevels - level_ndx) * kAddrBitsPerLevel));

      // Limit the coverage of this PTE level to just the address space that our
      // DeviceAspace was created to handle.  Otherwise, really large regions
      // covered by things like level 1 might never get reclaimed as they are not
      // "available for allocation" from our region allocator.
      if (aspace_.aspace_start_ > r.base) {
        const uint64_t delta = aspace_.aspace_start_ - r.base;

        // We should never be considering whether or not we need to reclaim the
        // page for a level which does not even intersect our address space.
        // There is no reason we should have every allocated a page for that slice
        // of the address space in the first place.
        ZX_DEBUG_ASSERT(r.size > delta);
        r.base = aspace_.aspace_start_;
        r.size -= delta;
      }

      const uint64_t aspace_end = aspace_.aspace_start_ + aspace_.aspace_len_;
      const uint64_t region_end = r.base + r.size;
      if (region_end > aspace_end) {
        const uint64_t delta = region_end - aspace_end;
        ZX_DEBUG_ASSERT(r.size > delta);
        r.size -= delta;
      }

      // Finally, test to see if we should return page for this level to the
      // PMM.  If the virtual address range controlled by this level of the
      // translation tables is entirely available for allocation in the region
      // allocator, then the page is no longer needed.  We can invalidate the
      // Table Entry which points to us, and then add the page to the
      // page_cache.  We'll trim the cache as a batch operation at the end of
      // the Unmap operation, after the TLBs have been invalidated.
      zx::result<bool> test_result = aspace_.avail_regions_.TestRegionContainedBy(
          r, RegionAllocator::TestRegionSet::Available);
      ZX_DEBUG_ASSERT(test_result.is_ok());
      if (test_result.value()) {
        Level& prev_level = levels_[level_ndx - 1];
        ZX_DEBUG_ASSERT(prev_level.table != nullptr);
        ZX_DEBUG_ASSERT(prev_level.ndx <= kAddrBitsMask);

        prev_level.table[prev_level.ndx] = 0;
        prev_level.dirty = true;

        const paddr_t return_me_phys = physmap_to_paddr(l.table);
        vm_page_t* const return_me = paddr_to_vm_page(return_me_phys);

        // Return the page to the page cache.  It will linger there full of
        // zeros until the TLBs have been invalidated and it can finally be
        // safely returned to the PMM.
        ZX_DEBUG_ASSERT_MSG(return_me != nullptr, "Failed to recover vm_page %p, phys 0x%016lx",
                            l.table, return_me_phys);
        aspace_.page_cache_.ReturnPage(return_me);
      }
    }

    // We are finished with this level.  Reset the table pointer and the index.
    l.table = nullptr;
    l.ndx = 0;
    ZX_DEBUG_ASSERT(l.dirty == false);
  }
}

zx::result<> TranslationTableHelper::FindPageForLevel(uint32_t level) {
  ZX_DEBUG_ASSERT((level > 0) && (level < kLevels));
  if (levels_[level].table != nullptr) {
    return zx::ok();
  }

  // If we have not found a page for this level, start by checking the level
  // before this one to see if there is a valid Table entry there.  If there
  // is, just translate the physical address to the vm_page_t address and we
  // should be good to go.
  //
  // Note: If we are performing an Unmap operation as a result of a cleanup
  // after a failed Map operation, it is possible that we don't _have_ a
  // previous table level.  We need to handle that edge case here.
  const uint32_t prev = level - 1;
  if ((op_ == Op::Unmap) && (levels_[prev].table == nullptr)) {
    return zx::ok();
  }

  {
    const uint64_t existing_entry = levels_[prev].table[levels_[prev].ndx];
    if (IsValidEntry(existing_entry)) {
      ZX_DEBUG_ASSERT(IsTableEntry(existing_entry));
      paddr_t phys = GetTableEntryPAddr(existing_entry);
      levels_[level].table = static_cast<uint64_t*>(paddr_to_physmap(phys));
      return zx::ok();
    }
  }

  // We have not found a page for this level because we have not allocated one
  // yet.  If this is an Unmap operation, that's OK.  We are in the process of
  // removing entries and empty levels anyway.
  if (op_ == Op::Unmap) {
    return zx::ok();
  }

  // Try to grab a page, and if we succeed, create the table entry at
  // the level before this one pointing at our new page.
  zx::result<vm_page_t*> maybe_page = aspace_.page_cache_.GetPage();
  if (!maybe_page.is_ok()) {
    return maybe_page.take_error();
  }

  // Make the new entry.  Be sure to flag the previous level's page as dirty
  // so we can flush it to physical memory when we are done.
  //
  // Note: It is assumed that all pages which come out of the page cache are
  // already zeroed (meaning that all of the entries are already invalid), and
  // that it is already flushed to the PoC with the SMMU.  If any of this is an
  // invalid assumption, this is the place to fix it (by explicitly zeroing the
  // new page, and flagging it as dirty so it will be flushed at the proper
  // time).
  paddr_t page_phys = maybe_page.value()->paddr();
  levels_[prev].table[levels_[prev].ndx] = MakeTableEntry(page_phys);
  levels_[level].table = static_cast<uint64_t*>(paddr_to_physmap(maybe_page.value()->paddr()));
  levels_[prev].dirty = true;
  return zx::ok();
}

}  // namespace arm_smmu
