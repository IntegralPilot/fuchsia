// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <algorithm>
#include <map>
#include <vector>

#include <dev/arm_smmu/device_aspace.h>
#include <dev/arm_smmu/page_cache.h>
#include <dev/arm_smmu/vmsav8_64.h>
#include <vm/pinned_vm_object.h>
#include <zxtest/zxtest.h>

namespace arm_smmu {

using namespace vmsav8_64;

namespace {

class MockVmObject : public VmObject {
 public:
  using InitList = std::initializer_list<std::map<size_t, paddr_t>::value_type>;

  MockVmObject(InitList init_list) : mappings_{init_list} {
    SetLookupHook([this](size_t offset, size_t size, paddr_t* out_paddr) -> zx_status_t {
      if (size != 4096) {  // 4k page size
        return ZX_ERR_INVALID_ARGS;
      }

      auto it = mappings_.find(offset);
      if (it != mappings_.end()) {
        *out_paddr = it->second;
        return ZX_OK;
      }

      return ZX_ERR_NOT_FOUND;
    });
  }

 private:
  std::map<size_t, paddr_t> mappings_;
};

}  // namespace

class DeviceAspaceTest : public zxtest::Test {
 protected:
  struct TrackedMapping {
    fbl::RefPtr<MockVmObject> vmo;
    DeviceAspace::Allocation alloc;
    uint32_t perms;
  };

  // 48 bit address space, starting at 1 MB
  static constexpr uint64_t kDefaultASpaceStart = uint64_t{1} << 20;
  static constexpr uint64_t kDefaultASpaceEnd = uint64_t{1} << 48;
  static constexpr uint64_t kDefaultASpaceLen = kDefaultASpaceEnd - kDefaultASpaceStart;

  // Don't cache any pages.  This makes it easier to assert that we have release
  // the proper number of pages as we go through the tests.
  static constexpr uint32_t kMaxCachePages = 0;

  static constexpr uint32_t kPageSize = 4096;
  static constexpr uint32_t kPageMask = kPageSize - 1;

  static void TlbInvalThunk(void* thiz, uint64_t base, uint64_t size) {
    reinterpret_cast<DeviceAspaceTest*>(thiz)->TlbInval(base, size);
  }

  void SetUp() override {
    PmmMock::Get().ClearAllocationLimit();
    zx::result<ktl::unique_ptr<DeviceAspace>> aspace_res =
        DeviceAspace::Create(kDefaultASpaceStart, kDefaultASpaceLen, kMaxCachePages);
    ASSERT_EQ(ZX_OK, aspace_res.status_value());
    aspace_ = ktl::move(aspace_res.value());
  }

  void TearDown() override {
    EXPECT_EQ(0u, tracked_mappings_.size());
    tracked_mappings_.clear();
    invalidations_.clear();

    const size_t pages_before = PmmMock::Get().GetAllocatedPageCount();
    aspace_->FreeTranslationTables(tlb_inval_op_);
    const size_t pages_after = PmmMock::Get().GetAllocatedPageCount();

    ASSERT_EQ(1u, invalidations_.size());
    EXPECT_EQ(0u, invalidations_[0].base);
    EXPECT_EQ(0u, invalidations_[0].size);

    const size_t pages_freed = pages_before - pages_after;
    EXPECT_EQ(pages_freed, invalidations_[0].cache_entries_during_inval);
    EXPECT_EQ(0u, aspace_->page_cache().cache_entries());
    aspace_.reset();
  }

  // Helper method to map a PinnedVmObject and track the allocation.
  // Returns the allocated base virtual address, or an error status on failure.
  zx::result<uint64_t> MapAndTrack(fbl::RefPtr<MockVmObject> vmo, const PinnedVmObject& pinned_vmo,
                                   uint32_t perms,
                                   ktl::optional<uint64_t> location = ktl::nullopt) {
    invalidations_.clear();
    zx::result<DeviceAspace::Allocation> alloc_res =
        aspace_->Map(pinned_vmo, perms, tlb_inval_op_, location);
    if (!alloc_res.is_ok()) {
      return zx::error(alloc_res.status_value());
    }

    DeviceAspace::Allocation alloc = ktl::move(alloc_res.value());
    const uint64_t base = alloc->base;
    const uint64_t size = alloc->size;

    EXPECT_EQ(alloc->size, pinned_vmo.size() - pinned_vmo.offset());
    EXPECT_GE(alloc->base, aspace_->aspace_start_);
    EXPECT_LT(alloc->base, aspace_->aspace_start_ + aspace_->aspace_len_);

    if (invalidations_.size() != 1u) {
      EXPECT_EQ(1u, invalidations_.size());
      return zx::error(ZX_ERR_INTERNAL);
    }
    EXPECT_EQ(base, invalidations_[0].base);
    EXPECT_EQ(size, invalidations_[0].size);

    auto [it, inserted] =
        tracked_mappings_.emplace(base, TrackedMapping{vmo, ktl::move(alloc), perms});
    EXPECT_TRUE(inserted);

    return zx::ok(base);
  }

  // Helper method to unmap a tracked mapping by its base virtual address
  // and run verification.
  void UnmapAndUntrack(uint64_t base) {
    auto it = tracked_mappings_.find(base);
    ASSERT_TRUE(it != tracked_mappings_.end());

    DeviceAspace::Allocation alloc = ktl::move(it->second.alloc);
    const uint64_t size = alloc->size;
    tracked_mappings_.erase(it);

    invalidations_.clear();
    const size_t pages_before = PmmMock::Get().GetAllocatedPageCount();
    aspace_->Unmap(ktl::move(alloc), tlb_inval_op_);
    const size_t pages_after = PmmMock::Get().GetAllocatedPageCount();

    ASSERT_EQ(1u, invalidations_.size());
    EXPECT_EQ(base, invalidations_[0].base);
    EXPECT_EQ(size, invalidations_[0].size);

    const size_t pages_freed = pages_before - pages_after;
    EXPECT_EQ(pages_freed, invalidations_[0].cache_entries_during_inval);
    EXPECT_EQ(0u, aspace_->page_cache().cache_entries());
  }

  // Helper to kick off the recursive verification of the page table against the
  // region allocator's state. This checks that every mapped and unmapped region
  // is correctly accounted for in the page table structure.
  void VerifyAllocatedRegions() {
    if (!aspace_ || !aspace_->root_tt_page_) {
      EXPECT_EQ(0u, PmmMock::Get().GetAllocatedPageCount());
      return;
    }
    const uint64_t* pt_root =
        static_cast<const uint64_t*>(paddr_to_physmap(aspace_->root_tt_page_->paddr()));
    size_t table_pages = VerifyAllocatedRegionsRecursive(pt_root, 0, 0);
    EXPECT_EQ(table_pages, PmmMock::Get().GetAllocatedPageCount());
  }

  // A callback we inject into Map and Unmap operations which the operation will
  // call in order to "invalidate our TLBs" at the proper points in the
  // map/unmap operations.
  void TlbInval(uint64_t base, uint64_t size) {
    invalidations_.push_back({
        .base = base,
        .size = size,
        .cache_entries_during_inval = aspace_->page_cache().cache_entries(),
    });
  }

 private:
  // Walk the page table tree recursively and verify that the translation tables
  // are in sync with the RegionAllocator's state.
  //
  // Specifically:
  // 1. Any valid leaf entry must be fully contained within the set of regions
  //    marked as "Allocated".
  // 2. Any valid leaf entry must map to the expected physical address of the
  //    corresponding tracked MockVmObject and have the correct access permissions.
  // 3. Any invalid entry (or portion of an invalid entry) that falls within the
  //    bounds of the active address space must be fully contained within the
  //    set of regions marked as "Available" (free/unmapped space).
  // 4. There cannot be any Block entries (only Table or Page entries).
  // 5. Invalid entries must be strictly zero, not just missing the valid bit.
  //
  // Returns the total number of translation table pages encountered, including the
  // current table.
  size_t VerifyAllocatedRegionsRecursive(const uint64_t* table, uint64_t vaddr, int level) {
    size_t table_pages = 1;

    const uint64_t aspace_start = aspace_->aspace_start_;
    const uint64_t aspace_end = aspace_start + aspace_->aspace_len_;
    for (uint32_t i = 0; i < kEntriesPerPage; ++i) {
      const uint64_t e = table[i];
      const uint64_t entry_vaddr = vaddr | (static_cast<uint64_t>(i) << (39 - 9 * level));
      const uint64_t entry_size = uint64_t{1} << (39 - 9 * level);
      const uint64_t entry_end = entry_vaddr + entry_size;

      if (IsValidEntry(e)) {
        EXPECT_FALSE(IsBlockEntry(e, level));

        if (level < 3 && IsTableEntry(e)) {
          const paddr_t next_pa = GetTableEntryPAddr(e);
          const uint64_t* next_table = static_cast<const uint64_t*>(paddr_to_physmap(next_pa));
          table_pages += VerifyAllocatedRegionsRecursive(next_table, entry_vaddr, level + 1);
        } else {
          {
            const zx::result<bool> res = aspace_->avail_regions_.TestRegionContainedBy(
                {.base = entry_vaddr, .size = entry_size},
                RegionAllocator::TestRegionSet::Allocated);
            EXPECT_OK(res.status_value());
            EXPECT_TRUE(res.is_ok() && res.value());
          }

          bool found_mapping = false;
          // Find the first mapping whose base address is > entry_vaddr.
          // If such a mapping exists, the mapping which *might* contain
          // entry_vaddr must be the one immediately preceding it.
          // If it == begin(), it means either the map is empty, or all mappings
          // start after entry_vaddr, meaning this vaddr maps to nothing tracked.
          // If it != begin(), we can safely decrement it to find the candidate mapping
          // and check if its [base, base + size) range actually covers entry_vaddr.
          auto it = tracked_mappings_.upper_bound(entry_vaddr);
          if (it != tracked_mappings_.begin()) {
            --it;
            const auto& mapping = it->second;
            if (entry_vaddr >= mapping.alloc->base &&
                entry_vaddr < mapping.alloc->base + mapping.alloc->size) {
              found_mapping = true;

              paddr_t expected_pa = 0;
              zx_status_t status = mapping.vmo->LookupContiguous(entry_vaddr - mapping.alloc->base,
                                                                 kPageSize, &expected_pa);
              EXPECT_EQ(ZX_OK, status);
              EXPECT_EQ(expected_pa, e & kValidAddrMask);

              uint64_t expected_perms = GetPageEntryPerms(mapping.perms);
              constexpr uint64_t kPermBits = MakePerms(1, 1, 3);
              EXPECT_EQ(expected_perms, e & kPermBits);
            }
          }
          EXPECT_TRUE(found_mapping);
        }
      } else {
        EXPECT_EQ(e, 0u);
        const uint64_t check_start = std::max(entry_vaddr, aspace_start);
        const uint64_t check_end = std::min(entry_end, aspace_end);
        if (check_start < check_end) {
          const zx::result<bool> res = aspace_->avail_regions_.TestRegionContainedBy(
              {.base = check_start, .size = check_end - check_start},
              RegionAllocator::TestRegionSet::Available);
          EXPECT_OK(res.status_value());
          EXPECT_TRUE(res.is_ok() && res.value());
        }
      }
    }

    return table_pages;
  }

 protected:
  struct InvalidationRecord {
    uint64_t base;
    uint64_t size;
    size_t cache_entries_during_inval;
  };
  std::vector<InvalidationRecord> invalidations_;

  ktl::unique_ptr<DeviceAspace> aspace_;
  std::map<uint64_t, TrackedMapping> tracked_mappings_;
  DeviceAspace::TlbInvalOp tlb_inval_op_{TlbInvalThunk, this};
};

TEST_F(DeviceAspaceTest, MapAndUnmap) {
  // We should have exactly 1 page allocated at this point (for the root translation table page);
  // VerifyAllocatedRegions will check this.
  VerifyAllocatedRegions();

  // Create Mock VmObject and PinnedVmObject
  // Set up mock mapping: virtual offset to physical address
  fbl::RefPtr<MockVmObject> vmo = fbl::MakeRefCounted<MockVmObject>(
      MockVmObject::InitList{{0x0, 0x1000}, {0x1000, 0x2000}, {0x2000, 0x3000}});

  PinnedVmObject pinned_vmo(vmo, 0, 0x3000);  // 3 pages

  // Now perform the mapping operation, then verify that the page tables match what we
  // would expect based on our region allocator and tracked mappings state.  Test all
  // combinations of the IOMMU permission flags.
  for (uint32_t perms = 0; perms <= 0x7; ++perms) {
    zx::result<uint64_t> maybe_base = MapAndTrack(vmo, pinned_vmo, perms);
    ASSERT_TRUE(maybe_base.is_ok(), "Failed for perms 0x%x", perms);
    const uint64_t base = maybe_base.value();
    VerifyAllocatedRegions();

    // Unmap and revalidate the page tables once again.
    UnmapAndUntrack(base);
    VerifyAllocatedRegions();
  }
}

TEST_F(DeviceAspaceTest, MapAtSpecificLocation) {
  VerifyAllocatedRegions();

  // Create Mock VmObject and PinnedVmObject
  fbl::RefPtr<MockVmObject> vmo =
      fbl::MakeRefCounted<MockVmObject>(MockVmObject::InitList{{0x0, 0x1000}});
  PinnedVmObject pinned_vmo(vmo, 0, 0x1000);

  // Pick a location in the middle of our address space, and make sure it is page aligned.
  const uint64_t target_location =
      ((kDefaultASpaceStart + kDefaultASpaceEnd) / 2) & ~uint64_t{kPageMask};
  zx::result<uint64_t> maybe_base =
      MapAndTrack(vmo, pinned_vmo, IOMMU_FLAG_PERM_READ | IOMMU_FLAG_PERM_WRITE, target_location);

  ASSERT_TRUE(maybe_base.is_ok(), "Failed to map at 0x%lx (status %d)", target_location,
              maybe_base.status_value());
  EXPECT_EQ(maybe_base.value(), target_location);
  VerifyAllocatedRegions();

  // Unmap and revalidate the page tables once again.
  UnmapAndUntrack(maybe_base.value());
  VerifyAllocatedRegions();
}

// Create a 2-page mapping which straddles all of the translation table levels.
//
// The first page should require an entry in the last slot of every level
// of the translation tables, while the second page should require an entry in
// the first slot of every level of the translation table.
TEST_F(DeviceAspaceTest, MapStraddlingTableLevels) {
  VerifyAllocatedRegions();

  // Create Mock VmObject and PinnedVmObject for 2 pages
  fbl::RefPtr<MockVmObject> vmo =
      fbl::MakeRefCounted<MockVmObject>(MockVmObject::InitList{{0x0, 0x1000}, {0x1000, 0x2000}});
  PinnedVmObject pinned_vmo(vmo, 0, 0x2000);

  // Pick a location which straddles all of the translation table levels.  The
  // L0/L1 boundary is at 1 << 39.
  const uint64_t target_location = (uint64_t{1} << 39) - kPageSize;
  zx::result<uint64_t> maybe_base =
      MapAndTrack(vmo, pinned_vmo, IOMMU_FLAG_PERM_READ | IOMMU_FLAG_PERM_WRITE, target_location);

  ASSERT_TRUE(maybe_base.is_ok(), "Failed to map at 0x%lx (status %d)", target_location,
              maybe_base.status_value());
  EXPECT_EQ(maybe_base.value(), target_location);
  VerifyAllocatedRegions();

  // Unmap and revalidate the page tables once again.
  UnmapAndUntrack(maybe_base.value());
  VerifyAllocatedRegions();
}

// Test mapping which fails during translation table allocation.
// Ensure that the operation returns a proper error and that the page tables
// are properly reset (no leaked partial mappings or tables).
TEST_F(DeviceAspaceTest, MapFailureInjection) {
  for (uint64_t limit = 0; limit < 4; ++limit) {
    VerifyAllocatedRegions();

    // Create a 10-page Mock VmObject
    fbl::RefPtr<MockVmObject> vmo = fbl::MakeRefCounted<MockVmObject>(MockVmObject::InitList{
        {0x0000, 0x1000},
        {0x1000, 0x2000},
        {0x2000, 0x3000},
        {0x3000, 0x4000},
        {0x4000, 0x5000},
        {0x5000, 0x6000},
        {0x6000, 0x7000},
        {0x7000, 0x8000},
        {0x8000, 0x9000},
        {0x9000, 0xA000},
    });
    PinnedVmObject pinned_vmo(vmo, 0, 10 * kPageSize);

    // Pick a location which straddles the Level 2 (2 MB) boundary.
    // Pages 0-4 will be in Level 3 Table A (Level 2 index 0).
    // Pages 5-9 will be in Level 3 Table B (Level 2 index 1).
    const uint64_t target_location = (uint64_t{1} << 21) - (5 * kPageSize);

    // We expect 4 successful allocations for translation tables after the root
    // page:
    // 1. Level 1 table
    // 2. Level 2 table
    // 3. Level 3 table A (for the first part of the mapping)
    // 4. Level 3 table B (during Advance to the 2MB boundary at Page 5)
    //
    // By looping from 0 to 3, we test failing at each of these points.
    PmmMock::Get().SetAllocationLimit(limit);
    auto cleanup_limit = fit::defer([]() { PmmMock::Get().ClearAllocationLimit(); });

    invalidations_.clear();

    const size_t pages_before = PmmMock::Get().GetAllocatedPageCount();
    zx::result<DeviceAspace::Allocation> alloc_res = aspace_->Map(
        pinned_vmo, IOMMU_FLAG_PERM_READ | IOMMU_FLAG_PERM_WRITE, tlb_inval_op_, target_location);
    EXPECT_FALSE(alloc_res.is_ok());
    EXPECT_EQ(ZX_ERR_NO_MEMORY, alloc_res.status_value());
    const size_t pages_after = PmmMock::Get().GetAllocatedPageCount();

    // Since it failed, it should have unmapped and invalidated.
    ASSERT_EQ(1u, invalidations_.size());
    EXPECT_EQ(target_location, invalidations_[0].base);
    EXPECT_EQ(pinned_vmo.size(), invalidations_[0].size);

    EXPECT_EQ(pages_before, pages_after);
    EXPECT_EQ(limit, invalidations_[0].cache_entries_during_inval);
    EXPECT_EQ(0u, aspace_->page_cache().cache_entries());

    // Confirm that nothing remained allocated in the RegionAllocator and the
    // page tables were properly cleaned up.
    EXPECT_EQ(0u, tracked_mappings_.size());
    VerifyAllocatedRegions();

    // Clean up for the next iteration.
    VerifyAllocatedRegions();
    PmmMock::Get().ClearAllocationLimit();
    cleanup_limit.cancel();
  }
}

// Test creating mappings that share various levels of the translation table.
TEST_F(DeviceAspaceTest, SharedTableLevels) {
  VerifyAllocatedRegions();

  // Create a 1-page Mock VmObject to use for all mappings.
  fbl::RefPtr<MockVmObject> vmo =
      fbl::MakeRefCounted<MockVmObject>(MockVmObject::InitList{{0x0, 0x1000}});
  PinnedVmObject pinned_vmo(vmo, 0, kPageSize);

  // Define a set of virtual addresses to map.
  // The first one is our "base".
  // The subsequent ones share L0, L1, L2, and L3 tables with the base respectively.
  constexpr uint64_t base_vaddr = uint64_t{2} << 39;
  constexpr uint64_t test_vaddrs[] = {
      base_vaddr,
      base_vaddr + (uint64_t{1} << 39),  // Different L0 index (shares L0 table/root)
      base_vaddr + (uint64_t{1} << 30),  // Different L1 index (shares L1 table)
      base_vaddr + (uint64_t{1} << 21),  // Different L2 index (shares L2 table)
      base_vaddr + (uint64_t{1} << 12),  // Different L3 index (shares L3 table)
  };

  // Map each of the addresses and verify consistency after each step.
  for (uint64_t vaddr : test_vaddrs) {
    ASSERT_TRUE(MapAndTrack(vmo, pinned_vmo, IOMMU_FLAG_PERM_READ, vaddr).is_ok());
    VerifyAllocatedRegions();
  }

  // Now unmap each of the mappings in reverse order and verify consistency at each step.
  for (size_t i = std::size(test_vaddrs); i > 0;) {
    UnmapAndUntrack(test_vaddrs[--i]);
    VerifyAllocatedRegions();
  }
}

TEST_F(DeviceAspaceTest, MapAtLocationEdgeCases) {
  VerifyAllocatedRegions();

  // Create Mock VmObject and PinnedVmObject of 2 pages (aligned size).
  fbl::RefPtr<MockVmObject> vmo =
      fbl::MakeRefCounted<MockVmObject>(MockVmObject::InitList{{0x0, 0x1000}, {0x1000, 0x2000}});
  PinnedVmObject pinned_vmo(vmo, 0, 2 * kPageSize);

  const uint64_t target_location =
      ((kDefaultASpaceStart + kDefaultASpaceEnd) / 2) & ~uint64_t{kPageMask};

  // 1. Either the location or size are not page aligned (expected: ZX_ERR_INVALID_ARGS)
  {
    // A. Location not page aligned
    const uint64_t unaligned_location = target_location + 1;
    invalidations_.clear();
    zx::result<DeviceAspace::Allocation> alloc_res =
        aspace_->Map(pinned_vmo, IOMMU_FLAG_PERM_READ, tlb_inval_op_, unaligned_location);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, alloc_res.status_value());
    EXPECT_TRUE(invalidations_.empty());

    // B. Size not page aligned
    PinnedVmObject unaligned_pinned_vmo(vmo, 0, (2 * kPageSize) - 1);
    invalidations_.clear();
    alloc_res =
        aspace_->Map(unaligned_pinned_vmo, IOMMU_FLAG_PERM_READ, tlb_inval_op_, target_location);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, alloc_res.status_value());
    EXPECT_TRUE(invalidations_.empty());
  }

  // 2. The sum of location and size overflows a 64-bit unsigned int (expected: ZX_ERR_INVALID_ARGS)
  {
    // Use a location close to uint64_t max such that location + size overflows.
    const uint64_t overflowing_location = uint64_t{0} - kPageSize;
    invalidations_.clear();
    zx::result<DeviceAspace::Allocation> alloc_res =
        aspace_->Map(pinned_vmo, IOMMU_FLAG_PERM_READ, tlb_inval_op_, overflowing_location);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, alloc_res.status_value());
    EXPECT_TRUE(invalidations_.empty());
  }

  // 3. The requested region goes outside the address space's valid address range (expected:
  // ZX_ERR_INVALID_ARGS)
  {
    // A. Region starts before first_valid_address()
    const uint64_t before_aspace_location = kDefaultASpaceStart - kPageSize;
    invalidations_.clear();
    zx::result<DeviceAspace::Allocation> alloc_res =
        aspace_->Map(pinned_vmo, IOMMU_FLAG_PERM_READ, tlb_inval_op_, before_aspace_location);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, alloc_res.status_value());
    EXPECT_TRUE(invalidations_.empty());

    // B. Region ends after last_valid_address()
    // kDefaultASpaceEnd is exclusive end of valid address range (i.e. last valid is
    // kDefaultASpaceEnd - 1)
    const uint64_t past_aspace_location = kDefaultASpaceEnd - kPageSize;
    invalidations_.clear();
    alloc_res = aspace_->Map(pinned_vmo, IOMMU_FLAG_PERM_READ, tlb_inval_op_, past_aspace_location);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, alloc_res.status_value());
    EXPECT_TRUE(invalidations_.empty());
  }

  // 4. The requested region collides with a currently active allocation (expected:
  // ZX_ERR_ALREADY_EXISTS)
  {
    // Map first region successfully
    zx::result<uint64_t> first_base =
        MapAndTrack(vmo, pinned_vmo, IOMMU_FLAG_PERM_READ, target_location);
    ASSERT_TRUE(first_base.is_ok());
    VerifyAllocatedRegions();

    // Attempt to map another region overlapping the first region
    // A. Exactly same location
    invalidations_.clear();
    zx::result<DeviceAspace::Allocation> alloc_res =
        aspace_->Map(pinned_vmo, IOMMU_FLAG_PERM_READ, tlb_inval_op_, target_location);
    EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, alloc_res.status_value());
    EXPECT_TRUE(invalidations_.empty());

    // B. Partially overlapping location (1 page offset)
    invalidations_.clear();
    alloc_res =
        aspace_->Map(pinned_vmo, IOMMU_FLAG_PERM_READ, tlb_inval_op_, target_location + kPageSize);
    EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, alloc_res.status_value());
    EXPECT_TRUE(invalidations_.empty());

    // Clean up the first allocation
    UnmapAndUntrack(first_base.value());
    VerifyAllocatedRegions();
  }
}

}  // namespace arm_smmu
