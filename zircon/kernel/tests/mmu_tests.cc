// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <bits.h>
#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <lib/unittest/unittest.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/aspace.h>
#include <ktl/iterator.h>
#include <vm/arch_vm_aspace.h>
#include <vm/pmm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object_paged.h>

#include <ktl/enforce.h>

#define PGTABLE_L2_SHIFT (kPageShift + kPageTableLevelShift)
#define PGTABLE_L1_SHIFT (PGTABLE_L2_SHIFT + kPageTableLevelShift)

// Most mmu tests want a 'sufficiently large' aspace to play in, these constants define an aspace
// that is large without having a discontinuity over the sign extended canonical addresses.
constexpr vaddr_t kAspaceBase = USER_ASPACE_BASE;
constexpr size_t kAspaceSize = USER_ASPACE_SIZE;

using ArchUnmapOptions = ArchVmAspaceInterface::ArchUnmapOptions;

static bool test_large_unaligned_region() {
  BEGIN_TEST;
  ArchVmAspace aspace(kAspaceBase, kAspaceSize, 0);
  zx_status_t err = aspace.Init();
  EXPECT_EQ(err, ZX_OK, "init aspace");

  const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

  // We want our region to be misaligned by at least a page, and for
  // it to straddle the PDP.
  vaddr_t va = (1UL << PGTABLE_L1_SHIFT) - (1UL << PGTABLE_L2_SHIFT) + 2 * kPageSize;
  // Make sure alloc_size is less than 1 PD page, to exercise the
  // non-terminal code path.
  static const size_t alloc_size = (1UL << PGTABLE_L2_SHIFT) - kPageSize;

  // Map a single page to force the lower PDP of the target region
  // to be created
  err = aspace.MapContiguous(va - 3 * kPageSize, 0, 1, arch_rw_flags);
  EXPECT_EQ(err, ZX_OK, "map single page");

  // Map the last page of the region
  err = aspace.MapContiguous(va + alloc_size - kPageSize, 0, 1, arch_rw_flags);
  EXPECT_EQ(err, ZX_OK, "map last page");

  paddr_t pa;
  arch_mmu_flags_t mmu_flags;
  err = aspace.Query(va + alloc_size - kPageSize, &pa, &mmu_flags);
  EXPECT_EQ(err, ZX_OK, "last entry is mapped");

  // Attempt to unmap the target region (analogous to unmapping a demand
  // paged region that has only had its last page touched)
  err = aspace.Unmap(va, alloc_size / kPageSize, ArchUnmapOptions::Enlarge);
  EXPECT_EQ(err, ZX_OK, "unmap unallocated region");

  err = aspace.Query(va + alloc_size - kPageSize, &pa, &mmu_flags);
  EXPECT_EQ(err, ZX_ERR_NOT_FOUND, "last entry is not mapped anymore");

  // Unmap the single page from earlier
  err = aspace.Unmap(va - 3 * kPageSize, 1, ArchUnmapOptions::Enlarge);
  EXPECT_EQ(err, ZX_OK, "unmap single page");

  err = aspace.Destroy();
  EXPECT_EQ(err, ZX_OK, "destroy aspace");

  END_TEST;
}

static bool test_large_unaligned_region_without_map() {
  BEGIN_TEST;

  {
    ArchVmAspace aspace(kAspaceBase, kAspaceSize, 0);
    zx_status_t err = aspace.Init();
    EXPECT_EQ(err, ZX_OK, "init aspace");

    const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

    // We want our region to be misaligned by a page, and for it to
    // straddle the PDP
    vaddr_t va = (1UL << PGTABLE_L1_SHIFT) - (1UL << PGTABLE_L2_SHIFT) + kPageSize;
    // Make sure alloc_size is bigger than 1 PD page, to exercise the
    // non-terminal code path.
    static const size_t alloc_size = 3UL << PGTABLE_L2_SHIFT;

    // Map a single page to force the lower PDP of the target region
    // to be created
    err = aspace.MapContiguous(va - 2 * kPageSize, 0, 1, arch_rw_flags);
    EXPECT_EQ(err, ZX_OK, "map single page");

    // Attempt to unmap the target region (analogous to unmapping a demand
    // paged region that has not been touched)
    err = aspace.Unmap(va, alloc_size / kPageSize, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap unallocated region");

    // Unmap the single page from earlier
    err = aspace.Unmap(va - 2 * kPageSize, 1, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap single page");

    err = aspace.Destroy();
    EXPECT_EQ(err, ZX_OK, "destroy aspace");
  }

  END_TEST;
}

static bool test_large_region_protect() {
  BEGIN_TEST;

  static const vaddr_t va = 1UL << PGTABLE_L1_SHIFT;
  // Force a large page.
  static const size_t alloc_size = 1UL << PGTABLE_L2_SHIFT;
  static const vaddr_t alloc_end = va + alloc_size;

  vaddr_t target_vaddrs[] = {
      va,
      va + kPageSize,
      va + 2 * kPageSize,
      alloc_end - 3 * kPageSize,
      alloc_end - 2 * kPageSize,
      alloc_end - kPageSize,
  };

  for (unsigned i = 0; i < ktl::size(target_vaddrs); i++) {
    ArchVmAspace aspace(kAspaceBase, kAspaceSize, 0);
    zx_status_t err = aspace.Init();
    EXPECT_EQ(err, ZX_OK, "init aspace");

    const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

    err = aspace.MapContiguous(va, 0, alloc_size / kPageSize, arch_rw_flags);
    EXPECT_EQ(err, ZX_OK, "map large page");

    err = aspace.Protect(target_vaddrs[i], 1, ARCH_MMU_FLAG_PERM_READ, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "protect single page");

    for (unsigned j = 0; j < ktl::size(target_vaddrs); j++) {
      arch_mmu_flags_t mmu_flags = 0;
      paddr_t pa;
      EXPECT_EQ(ZX_OK, aspace.Query(target_vaddrs[j], &pa, &mmu_flags));
      EXPECT_EQ(target_vaddrs[j] - va, pa);

      EXPECT_EQ(i == j ? ARCH_MMU_FLAG_PERM_READ : arch_rw_flags, mmu_flags);
    }

    err = aspace.Unmap(va, alloc_size / kPageSize, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap large page");
    err = aspace.Destroy();
    EXPECT_EQ(err, ZX_OK, "destroy aspace");
  }

  END_TEST;
}

// Since toggle_page_alloc_fn needs global state to operate, define a lock to ensure we are only
// running a single instance of these tests at a time.
DECLARE_SINGLETON_MUTEX(TogglePageAllocLock);
static bool fail_page_allocs = false;
static zx_status_t toggle_page_alloc_fn(uint alloc_flags, vm_page** p, paddr_t* pa) {
  if (fail_page_allocs) {
    return ZX_ERR_NO_MEMORY;
  }
  return pmm_alloc_page(alloc_flags, p, pa);
}

static bool test_large_region_unmap() {
  BEGIN_TEST;

  Guard<Mutex> guard{TogglePageAllocLock::Get()};

  static const vaddr_t va = 1UL << PGTABLE_L1_SHIFT;
  // Force a large page.
  static const size_t alloc_size = 1UL << PGTABLE_L2_SHIFT;
  static const vaddr_t alloc_end = va + alloc_size;

  vaddr_t target_vaddrs[] = {
      va,
      va + kPageSize,
      va + 2 * kPageSize,
      alloc_end - 3 * kPageSize,
      alloc_end - 2 * kPageSize,
      alloc_end - kPageSize,
  };

  for (unsigned i = 0; i < ktl::size(target_vaddrs); i++) {
    fail_page_allocs = false;
    ArchVmAspace aspace(kAspaceBase, kAspaceSize, 0, toggle_page_alloc_fn);
    zx_status_t err = aspace.Init();
    EXPECT_EQ(err, ZX_OK, "init aspace");

    const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

    // Use MapContiguous to create a mapping that should get backed by a large page.
    err = aspace.MapContiguous(va, 0, alloc_size / kPageSize, arch_rw_flags);
    EXPECT_EQ(err, ZX_OK, "map large page");

    // Unmap a single small page out of the larger page.
    err = aspace.Unmap(target_vaddrs[i], 1, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap single page");

    // Ensure the single page was unmapped, but the rest of the large page is still present.
    for (unsigned j = 0; j < ktl::size(target_vaddrs); j++) {
      arch_mmu_flags_t mmu_flags = 0;
      paddr_t pa;
      zx_status_t result = aspace.Query(target_vaddrs[j], &pa, &mmu_flags);
      EXPECT_EQ(i == j ? ZX_ERR_NOT_FOUND : ZX_OK, result, "query page");
    }

    err = aspace.Unmap(va, alloc_size / kPageSize, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap remaining pages");

    // Map in the large page again.
    err = aspace.MapContiguous(va, 0, alloc_size / kPageSize, arch_rw_flags);
    EXPECT_EQ(err, ZX_OK, "map large page");

    // Simulate OOM by failing allocations.
    fail_page_allocs = true;
    // Attempt to unmap a single small page, but allow over unmapping.
    err = aspace.Unmap(target_vaddrs[i], 1, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap single page");

    // The entire large page should have ended up unmapped.
    for (unsigned j = 0; j < ktl::size(target_vaddrs); j++) {
      arch_mmu_flags_t mmu_flags = 0;
      paddr_t pa;
      zx_status_t result = aspace.Query(target_vaddrs[j], &pa, &mmu_flags);
      EXPECT_EQ(ZX_ERR_NOT_FOUND, result, "query page");
    }

    err = aspace.Unmap(va, alloc_size / kPageSize, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap remaining pages");

    // Map in the large page again.
    fail_page_allocs = false;
    err = aspace.MapContiguous(va, 0, alloc_size / kPageSize, arch_rw_flags);
    EXPECT_EQ(err, ZX_OK, "map large page");

    // Simulate OOM by failing allocations.
    fail_page_allocs = true;
    // Attempt to unmap a single small page, but disallow over unmapping.
    // err = aspace.Unmap(target_vaddrs[i], 1, ArchUnmapOptions::None);
    // EXPECT_EQ(err, ZX_ERR_NO_MEMORY, "unmap single page");

    // All mappings should still be present.
    // The entire large page should have ended up unmapped.
    for (unsigned j = 0; j < ktl::size(target_vaddrs); j++) {
      arch_mmu_flags_t mmu_flags = 0;
      paddr_t pa;
      zx_status_t result = aspace.Query(target_vaddrs[j], &pa, &mmu_flags);
      EXPECT_EQ(ZX_OK, result, "query page");
    }

    err = aspace.Unmap(va, alloc_size / kPageSize, ArchUnmapOptions::Enlarge);
    EXPECT_EQ(err, ZX_OK, "unmap remaining pages");
    err = aspace.Destroy();
    EXPECT_EQ(err, ZX_OK, "destroy aspace");
  }

  END_TEST;
}

static VmPageDoublyLinkedList node;
static zx_status_t test_page_alloc_fn(uint unused, vm_page** p, paddr_t* pa) {
  if (node.is_empty()) {
    return ZX_ERR_NO_MEMORY;
  }
  vm_page_t* page = node.pop_front();
  if (p) {
    *p = page;
  }
  if (pa) {
    *pa = page->paddr();
  }
  return ZX_OK;
}

static bool test_mapping_oom() {
  BEGIN_TEST;

  constexpr uint64_t kMappingPageCount = 8;
  constexpr uint64_t kMappingSize = kMappingPageCount * kPageSize;
  constexpr vaddr_t kMappingStart = (1UL << PGTABLE_L1_SHIFT) - kMappingSize / 2;

  // Allocate the pages which will be mapped into the test aspace.
  vm_page_t* mapping_pages[kMappingPageCount] = {};
  paddr_t mapping_paddrs[kMappingPageCount] = {};

  auto undo = fit::defer([&]() {
    for (vm_page_t* mapping_page : mapping_pages) {
      if (mapping_page) {
        pmm_free_page(mapping_page);
      }
    }
  });

  for (unsigned i = 0; i < kMappingPageCount; i++) {
    ASSERT_EQ(pmm_alloc_page(0, mapping_pages + i, mapping_paddrs + i), ZX_OK);
  }

  // Try to create the mapping with a limited number of pages available to
  // the aspace. Start with only 1 available and continue until the map operation
  // succeeds without running out of memory.
  bool map_success = false;
  uint64_t avail_mmu_pages = 1;
  while (!map_success) {
    for (unsigned i = 0; i < avail_mmu_pages; i++) {
      vm_page_t* page;
      ASSERT_EQ(pmm_alloc_page(0, &page), ZX_OK, "alloc fail");
      node.push_front(page);
    }

    ArchVmAspace aspace(kAspaceBase, kAspaceSize, 0, test_page_alloc_fn);
    zx_status_t err = aspace.Init();
    ASSERT_EQ(err, ZX_OK, "init aspace");

    const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

    err = aspace.Map(kMappingStart, mapping_paddrs, kMappingPageCount, arch_rw_flags,
                     ArchVmAspace::ExistingEntryAction::Error);
    if (err == ZX_OK) {
      map_success = true;
      EXPECT_EQ(aspace.Unmap(kMappingStart, kMappingPageCount, ArchUnmapOptions::Enlarge), ZX_OK);
    } else {
      EXPECT_EQ(err, ZX_ERR_NO_MEMORY);
      avail_mmu_pages++;
      // validate that all of the pages were consumed
      EXPECT_TRUE(node.is_empty());
    }

    // Destroying the aspace verifies that everything was cleaned up
    // when the mapping failed part way through.
    err = aspace.Destroy();
    ASSERT_EQ(err, ZX_OK, "destroy aspace");
    ASSERT_TRUE(node.is_empty());
  }

  END_TEST;
}

static bool test_skip_existing_mapping() {
  BEGIN_TEST;

  constexpr vaddr_t kMapBase = kAspaceBase;
  constexpr paddr_t kPhysBase = 0;
  constexpr size_t kNumPages = 8;
  constexpr size_t kMidPage = kNumPages / 2;

  ArchVmAspace aspace(kAspaceBase, kAspaceSize, 0);
  zx_status_t err = aspace.Init();
  EXPECT_EQ(err, ZX_OK);

  const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

  paddr_t page_addresses[kNumPages];
  for (size_t i = 0; i < kNumPages; i++) {
    page_addresses[i] = kPhysBase + (kPageSize * i);
  }

  // Map in the middle page by itself first, using the final settings.
  err = aspace.Map(kMapBase + kMidPage * kPageSize, &page_addresses[kMidPage], 1, arch_rw_flags,
                   ArchVmAspace::ExistingEntryAction::Error);
  EXPECT_EQ(err, ZX_OK);

  // Now map in all the pages.
  err = aspace.Map(kMapBase, page_addresses, kNumPages, arch_rw_flags,
                   ArchVmAspace::ExistingEntryAction::Skip);
  EXPECT_EQ(err, ZX_OK);

  // Validate all the pages.
  for (size_t i = 0; i < kNumPages; i++) {
    paddr_t paddr;
    arch_mmu_flags_t mmu_flags;
    err = aspace.Query(kMapBase + i * kPageSize, &paddr, &mmu_flags);
    EXPECT_EQ(err, ZX_OK);
    EXPECT_EQ(paddr, page_addresses[i]);
    EXPECT_EQ(mmu_flags, arch_rw_flags);
  }
  err = aspace.Unmap(kMapBase, kNumPages, ArchUnmapOptions::Enlarge);
  EXPECT_EQ(err, ZX_OK);

  // Now try mapping in the midle page with different permissions.
  err = aspace.Map(kMapBase + kMidPage * kPageSize, &page_addresses[kMidPage], 1,
                   ARCH_MMU_FLAG_PERM_READ, ArchVmAspace::ExistingEntryAction::Error);
  EXPECT_EQ(err, ZX_OK);
  err = aspace.Map(kMapBase, page_addresses, kNumPages, arch_rw_flags,
                   ArchVmAspace::ExistingEntryAction::Skip);
  EXPECT_EQ(err, ZX_OK);
  for (size_t i = 0; i < kNumPages; i++) {
    paddr_t paddr;
    arch_mmu_flags_t mmu_flags;
    err = aspace.Query(kMapBase + i * kPageSize, &paddr, &mmu_flags);
    EXPECT_EQ(err, ZX_OK);
    EXPECT_EQ(paddr, page_addresses[i]);
    if (i == kMidPage) {
      EXPECT_EQ(mmu_flags, ARCH_MMU_FLAG_PERM_READ);
    } else {
      EXPECT_EQ(mmu_flags, arch_rw_flags);
    }
  }
  err = aspace.Unmap(kMapBase, kNumPages, ArchUnmapOptions::Enlarge);
  EXPECT_EQ(err, ZX_OK);

  // Now map the middle page using a completely different physical address.
  paddr_t other_paddr = kPageSize * 42;
  err = aspace.Map(kMapBase + kMidPage * kPageSize, &other_paddr, 1, ARCH_MMU_FLAG_PERM_READ,
                   ArchVmAspace::ExistingEntryAction::Error);
  EXPECT_EQ(err, ZX_OK);
  err = aspace.Map(kMapBase, page_addresses, kNumPages, arch_rw_flags,
                   ArchVmAspace::ExistingEntryAction::Skip);
  EXPECT_EQ(err, ZX_OK);
  for (size_t i = 0; i < kNumPages; i++) {
    paddr_t paddr;
    arch_mmu_flags_t mmu_flags;
    err = aspace.Query(kMapBase + i * kPageSize, &paddr, &mmu_flags);
    EXPECT_EQ(err, ZX_OK);
    if (i == kMidPage) {
      EXPECT_EQ(mmu_flags, ARCH_MMU_FLAG_PERM_READ);
      EXPECT_EQ(paddr, other_paddr);
    } else {
      EXPECT_EQ(mmu_flags, arch_rw_flags);
      EXPECT_EQ(paddr, page_addresses[i]);
    }
  }
  err = aspace.Unmap(kMapBase, kNumPages, ArchUnmapOptions::Enlarge);
  EXPECT_EQ(err, ZX_OK);

  err = aspace.Destroy();
  EXPECT_EQ(err, ZX_OK);

  END_TEST;
}

// Attempts to validate that unmapping part of a large page will not cause parallel threads
// accessing other parts of the page to fault. This test is only probabilistic and is heavily timing
// and micro architectural dependent, but could serve as a canary.
static bool test_large_region_atomic() {
  BEGIN_TEST;

  if (VmAspace::kernel_aspace()->arch_aspace().UnmapOnlyEnlargeOnOom()) {
    // Force a large page.
    static constexpr size_t alloc_size = 1UL << PGTABLE_L2_SHIFT;

    static constexpr size_t target_offsets[] = {
        0,
        kPageSize,
        2 * kPageSize,
        alloc_size - 3 * kPageSize,
        alloc_size - 2 * kPageSize,
        alloc_size - kPageSize,
    };

    for (unsigned i = 0; i < ktl::size(target_offsets); i++) {
      // Allocate a large page in the current kernel aspace. Need to allocate in the current aspace
      // and not a test aspace so that we can directly access the mappings.
      auto kaspace = VmAspace::kernel_aspace();
      fbl::RefPtr<VmAddressRegion> vmar = kaspace->RootVmar();
      fbl::RefPtr<VmObjectPaged> vmo;

      zx_status_t status =
          VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kAlwaysPinned, alloc_size, &vmo);
      ASSERT_OK(status);

      const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

      auto mapping_result = vmar->CreateVmMapping(
          0, alloc_size, 0,
          VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING,
          vmo, 0, arch_rw_flags, "test");
      ASSERT_OK(mapping_result.status_value());

      status = mapping_result->mapping->MapRange(0, alloc_size, false);
      ASSERT_OK(status);

      const vaddr_t va = mapping_result->base;

      auto cleanup_mapping = fit::defer([&] { mapping_result->mapping->Destroy(); });

      // Spin up a thread to start touching pages in the mapping.
      struct State {
        vaddr_t va;
        uint current_offset;
        ktl::atomic<bool> running;
      } state = {va, i, true};
      auto thread_body = [](void* arg) -> int {
        State* state = static_cast<State*>(arg);

        while (state->running) {
          for (unsigned i = 0; i < ktl::size(target_offsets); i++) {
            if (state->current_offset == i) {
              continue;
            }
            volatile uint64_t* addr = reinterpret_cast<uint64_t*>(state->va + target_offsets[i]);
            // Force read from the address
            __asm__ volatile("" ::"r"(*addr));
          }
        }
        return 0;
      };

      Thread* thread = Thread::Create("test-thread", thread_body, &state, DEFAULT_PRIORITY);
      ASSERT_NONNULL(thread);
      thread->Resume();

      auto cleanup_thread = fit::defer([&]() {
        state.running = false;
        thread->Join(nullptr, ZX_TIME_INFINITE);
      });

      // Wait a moment to let the other thread start touching.
      Thread::Current::SleepRelative(ZX_MSEC(50));

      // Unmap a single page.
      status = kaspace->arch_aspace().Unmap(va + target_offsets[i], 1, ArchUnmapOptions::None);
      EXPECT_EQ(status, ZX_OK, "unmap single page");

      // If the other thread didn't cause a kernel panic by having a page fault, then success.
    }
  }

  END_TEST;
}

// Test that RangeChangeUpdateLocked ignores pinned pages.
static bool test_unmap_ignore_pinned() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test aspace");
  auto cleanup_aspace = fit::defer([&]() { ASSERT(ZX_OK == aspace->Destroy()); });

  ArchVmAspace& arch_aspace = aspace->arch_aspace();
  const arch_mmu_flags_t arch_rw_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

  fbl::RefPtr<VmAddressRegion> vmar = aspace->RootVmar();

  {
    // Ensure an unpinned range gets completely unmapped.
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 4 * kPageSize, &vmo));
    ASSERT_OK(vmo->CommitRange(0, 4 * kPageSize));

    auto mapping_result =
        vmar->CreateVmMapping(0, 4 * kPageSize, 0, VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
                              vmo, 0, arch_rw_flags, "test");
    ASSERT_OK(mapping_result.status_value());
    ASSERT_OK(mapping_result->mapping->MapRange(0, 4 * kPageSize, false));
    auto cleanup_mapping = fit::defer([&] { ASSERT(ZX_OK == mapping_result->mapping->Destroy()); });

    const vaddr_t va = mapping_result->base;
    zx_paddr_t paddr;
    arch_mmu_flags_t mmu_flags;

    for (size_t i = 0; i < 4; i++) {
      EXPECT_OK(arch_aspace.Query(va + i * kPageSize, &paddr, &mmu_flags));
    }

    fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
    VmCowPages::DeferredOps deferred(cow.get());
    Guard<CriticalMutex> guard{cow->lock()};
    cow->RangeChangeUpdateLocked(VmCowRange(0, 4 * kPageSize), VmObject::RangeChangeOp::Unmap,
                                 &deferred);

    for (size_t i = 0; i < 4; i++) {
      EXPECT_EQ(ZX_ERR_NOT_FOUND, arch_aspace.Query(va + i * kPageSize, &paddr, &mmu_flags));
    }
  }

  {
    // Ensure a fully pinned range doesn't get unmapped.
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 4 * kPageSize, &vmo));
    ASSERT_OK(vmo->CommitRangePinned(0, 4 * kPageSize, false));
    auto unpin = fit::defer([&]() { vmo->Unpin(0, 4 * kPageSize); });

    auto mapping_result =
        vmar->CreateVmMapping(0, 4 * kPageSize, 0, VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
                              vmo, 0, arch_rw_flags, "test");
    ASSERT_OK(mapping_result.status_value());
    ASSERT_OK(mapping_result->mapping->MapRange(0, 4 * kPageSize, false));
    auto cleanup_mapping = fit::defer([&] { ASSERT(ZX_OK == mapping_result->mapping->Destroy()); });

    const vaddr_t va = mapping_result->base;
    zx_paddr_t paddr;
    arch_mmu_flags_t mmu_flags;

    fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
    VmCowPages::DeferredOps deferred(cow.get());
    Guard<CriticalMutex> guard{cow->lock()};
    cow->RangeChangeUpdateLocked(VmCowRange(0, 4 * kPageSize), VmObject::RangeChangeOp::Unmap,
                                 &deferred);

    for (size_t i = 0; i < 4; i++) {
      EXPECT_OK(arch_aspace.Query(va + i * kPageSize, &paddr, &mmu_flags));
    }
  }

  {
    // Unmap a greater range than exists in the VMO or in the mapping.
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 4 * kPageSize, &vmo));
    ASSERT_OK(vmo->CommitRange(0, 4 * kPageSize));
    ASSERT_OK(vmo->CommitRangePinned(kPageSize, 2 * kPageSize, false));
    auto unpin = fit::defer([&]() { vmo->Unpin(kPageSize, 2 * kPageSize); });

    auto mapping_result =
        vmar->CreateVmMapping(0, 4 * kPageSize, 0, VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
                              vmo, 0, arch_rw_flags, "test");
    ASSERT_OK(mapping_result.status_value());
    ASSERT_OK(mapping_result->mapping->MapRange(0, 4 * kPageSize, false));
    auto cleanup_mapping = fit::defer([&] { ASSERT(ZX_OK == mapping_result->mapping->Destroy()); });

    const vaddr_t va = mapping_result->base;
    zx_paddr_t paddr;
    arch_mmu_flags_t mmu_flags;

    fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
    VmCowPages::DeferredOps deferred(cow.get());
    Guard<CriticalMutex> guard{cow->lock()};
    // Note: Large len here. This is okay.
    cow->RangeChangeUpdateLocked(VmCowRange(0, 100 * kPageSize), VmObject::RangeChangeOp::Unmap,
                                 &deferred);

    EXPECT_EQ(ZX_ERR_NOT_FOUND, arch_aspace.Query(va + 0 * kPageSize, &paddr, &mmu_flags));
    EXPECT_OK(arch_aspace.Query(va + 1 * kPageSize, &paddr, &mmu_flags));
    EXPECT_OK(arch_aspace.Query(va + 2 * kPageSize, &paddr, &mmu_flags));
    EXPECT_EQ(ZX_ERR_NOT_FOUND, arch_aspace.Query(va + 3 * kPageSize, &paddr, &mmu_flags));
  }

  {
    // Alternating pinned pages.
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 20 * kPageSize, &vmo));
    ASSERT_OK(vmo->CommitRange(0, 20 * kPageSize));

    for (uint64_t i = 0; i < 20; i += 2) {
      ASSERT_OK(vmo->CommitRangePinned(i * kPageSize, kPageSize, false));
    }
    auto unpin = fit::defer([&]() {
      for (uint64_t i = 0; i < 20; i += 2) {
        vmo->Unpin(i * kPageSize, kPageSize);
      }
    });

    auto mapping_result = vmar->CreateVmMapping(0, 20 * kPageSize, 0,
                                                VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
                                                vmo, 0, arch_rw_flags, "test");
    ASSERT_OK(mapping_result.status_value());
    ASSERT_OK(mapping_result->mapping->MapRange(0, 20 * kPageSize, false));
    auto cleanup_mapping = fit::defer([&] { ASSERT(ZX_OK == mapping_result->mapping->Destroy()); });

    const vaddr_t va = mapping_result->base;
    zx_paddr_t paddr;
    arch_mmu_flags_t mmu_flags;

    fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
    VmCowPages::DeferredOps deferred(cow.get());
    Guard<CriticalMutex> guard{cow->lock()};
    cow->RangeChangeUpdateLocked(VmCowRange(0, 20 * kPageSize), VmObject::RangeChangeOp::Unmap,
                                 &deferred);

    for (uint64_t i = 0; i < 20; i++) {
      if (i % 2 == 0) {
        EXPECT_OK(arch_aspace.Query(va + i * kPageSize, &paddr, &mmu_flags));
      } else {
        EXPECT_EQ(ZX_ERR_NOT_FOUND, arch_aspace.Query(va + i * kPageSize, &paddr, &mmu_flags));
      }
    }
  }

  END_TEST;
}

UNITTEST_START_TESTCASE(mmu_tests)
UNITTEST("create large unaligned region and ensure it can be unmapped", test_large_unaligned_region)
UNITTEST("create large unaligned region without mapping and ensure it can be unmapped",
         test_large_unaligned_region_without_map)
UNITTEST("creating large vm region, and change permissions", test_large_region_protect)
UNITTEST("trigger oom failures when creating a mapping", test_mapping_oom)
UNITTEST("skip existing entry when mapping multiple pages", test_skip_existing_mapping)
UNITTEST("create large vm region and unmap single pages", test_large_region_unmap)
UNITTEST("splitting a large page is atomic", test_large_region_atomic)
UNITTEST("test that unmap ignores pinned pages", test_unmap_ignore_pinned)
UNITTEST_END_TESTCASE(mmu_tests, "mmu", "mmu tests")
