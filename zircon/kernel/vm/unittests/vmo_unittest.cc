// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/page/size.h>

#include <vm/discardable_vmo_tracker.h>
#include <vm/pinned_vm_object.h>

#include "test_helper.h"

namespace vm_unittest {

namespace {

// Helper wrapper around reclaiming a page that returns the pages to the pmm.
uint64_t reclaim(fbl::RefPtr<VmCowPages> vmo, vm_page_t* page, uint64_t offset,
                 VmCowPages::EvictionAction hint_action, VmCompressor* compressor) {
  // If we've passed a compressor, it's expected we're testing compression.
  DEBUG_ASSERT(!compressor || !vmo->can_evict());

  VmCowReclaimResult reclaimed = vmo->ReclaimPage(page, offset, hint_action, compressor);
  if (reclaimed.is_ok()) {
    return reclaimed.value().num_pages;
  }
  return 0;
}

// Simulate the reclamation thread.
uint64_t reclaim(fbl::RefPtr<VmObjectPaged> vmo, vm_page_t* page, uint64_t offset,
                 VmCowPages::EvictionAction hint_action) {
  // Move to 'DontNeed' unless the page is dirty, as dirty pages should never be in the isolate
  // queue.
  if (!pmm_page_queues()->DebugPageIsPagerBackedDirty(page)) {
    pmm_page_queues()->MoveToReclaimDontNeed(page);
  }
  return reclaim(vmo->DebugGetCowPages(), page, offset, hint_action, nullptr);
}

// Test compression.
uint64_t compress_page(fbl::RefPtr<VmObjectPaged> vmo, vm_page_t* page, uint64_t offset,
                       VmCowPages::EvictionAction hint_action, VmCompressor* compressor) {
  fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
  // Reclamation won't compress if eviction is possible.
  DEBUG_ASSERT(!cow_pages->can_evict());
  return reclaim(cow_pages, page, offset, hint_action, compressor);
}

bool evict_loaned_page(fbl::RefPtr<VmObjectPaged> vmo, vm_page_t* page, uint64_t offset) {
  zx_status_t status = vmo->DebugGetCowPages()->EvictLoanedPage(page, offset);
  return status == ZX_OK;
}

// Creates a vm object.
bool vmo_create_test() {
  BEGIN_TEST;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize, &vmo);
  ASSERT_EQ(status, ZX_OK);
  ASSERT_TRUE(vmo);
  EXPECT_FALSE(vmo->is_contiguous(), "vmo is not contig\n");
  EXPECT_FALSE(vmo->is_resizable(), "vmo is not resizable\n");
  END_TEST;
}

bool vmo_create_maximum_size() {
  BEGIN_TEST;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, VmObject::max_size(), &vmo);
  EXPECT_EQ(ZX_OK, status, "should be ok\n");

  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, VmObject::max_size() + kPageSize, &vmo);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status, "should be too large\n");
  END_TEST;
}

// Helper that tests if all pages in a vmo in the specified range pass the given predicate.
template <typename F>
bool AllPagesMatch(VmObject* vmo, F pred, uint64_t offset, uint64_t len) {
  bool pred_matches = true;
  zx_status_t status =
      vmo->Lookup(offset, len, [&pred, &pred_matches](uint64_t offset, paddr_t pa) {
        const vm_page_t* p = paddr_to_vm_page(pa);
        if (!pred(p)) {
          pred_matches = false;
          return ZX_ERR_STOP;
        }
        return ZX_ERR_NEXT;
      });
  return status == ZX_OK ? pred_matches : false;
}

bool PagesInAnyAnonymousQueue(VmObject* vmo, uint64_t offset, uint64_t len) {
  return AllPagesMatch(
      vmo, [](const vm_page_t* p) { return pmm_page_queues()->DebugPageIsAnyAnonymous(p); }, offset,
      len);
}

bool PagesInWiredQueue(VmObject* vmo, uint64_t offset, uint64_t len) {
  return AllPagesMatch(
      vmo, [](const vm_page_t* p) { return pmm_page_queues()->DebugPageIsWired(p); }, offset, len);
}

// Creates a vm object, commits memory.
bool vmo_commit_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  auto ret = vmo->CommitRange(0, alloc_size);
  ASSERT_EQ(ZX_OK, ret, "committing vm object\n");
  EXPECT_TRUE(make_private_attribution_counts(alloc_size, 0) == vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, alloc_size));
  EXPECT_TRUE(PagesInAnyAnonymousQueue(vmo.get(), 0, alloc_size));
  END_TEST;
}

bool vmo_commit_compressed_pages_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;
  // Need a working compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  // Create a VMO and commit some real pages.
  constexpr size_t kPages = 8;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPages * kPageSize, &vmo);
  ASSERT_OK(status);
  status = vmo->CommitRange(0, kPages * kPageSize);
  ASSERT_OK(status);

  // Validate these are committed.
  EXPECT_TRUE(make_private_attribution_counts(kPages * kPageSize, 0) == vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPages * kPageSize));
  EXPECT_TRUE(PagesInAnyAnonymousQueue(vmo.get(), 0, kPages * kPageSize));

  // Lookup and compress each page;
  for (size_t i = 0; i < kPages; i++) {
    // Write some data (possibly zero) to the page.
    EXPECT_OK(vmo->Write(&i, i * kPageSize, sizeof(i)));
    vm_page_t* page;
    status = vmo->GetPageBlocking(i * kPageSize, 0, &page, nullptr);
    ASSERT_OK(status);
    auto compressor = compression->AcquireCompressor();
    ASSERT_OK(compressor.get().Arm());

    uint64_t reclaimed = compress_page(vmo, page, i * kPageSize,
                                       VmCowPages::EvictionAction::FollowHint, &compressor.get());
    EXPECT_EQ(reclaimed, 1u);
  }

  // Should be no real pages, and one of the pages should have been deduped to zero and not even be
  // compressed.
  EXPECT_TRUE(make_private_attribution_counts(0, (kPages - 1) * kPageSize) ==
              vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, (kPages - 1) * kPageSize));

  // Now use commit again, this should decompress things.
  status = vmo->CommitRange(0, kPages * kPageSize);
  ASSERT_OK(status);

  EXPECT_TRUE(make_private_attribution_counts(kPages * kPageSize, 0) == vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPages * kPageSize));
  EXPECT_TRUE(PagesInAnyAnonymousQueue(vmo.get(), 0, kPages * kPageSize));

  END_TEST;
}

// Creates paged VMOs, pins them, and tries operations that should unpin.
bool vmo_pin_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = kPageSize * 16;
  for (uint32_t is_loaning_enabled = 0; is_loaning_enabled < 2; ++is_loaning_enabled) {
    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(!!is_loaning_enabled);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status;
    status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, alloc_size, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");

    status = vmo->CommitRangePinned(kPageSize, alloc_size, false);
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status, "pinning out of range\n");
    status = vmo->CommitRangePinned(kPageSize, 0, false);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, status, "pinning range of len 0\n");

    status = vmo->CommitRangePinned(kPageSize, 3 * kPageSize, false);
    EXPECT_EQ(ZX_OK, status, "pinning range\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), kPageSize, 3 * kPageSize));

    status = vmo->DecommitRange(kPageSize, 3 * kPageSize);
    EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");
    status = vmo->DecommitRange(kPageSize, kPageSize);
    EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");
    status = vmo->DecommitRange(3 * kPageSize, kPageSize);
    EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");

    vmo->Unpin(kPageSize, 3 * kPageSize);
    EXPECT_TRUE(PagesInAnyAnonymousQueue(vmo.get(), kPageSize, 3 * kPageSize));

    status = vmo->DecommitRange(kPageSize, 3 * kPageSize);
    EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");

    status = vmo->CommitRangePinned(kPageSize, 3 * kPageSize, false);
    EXPECT_EQ(ZX_OK, status, "pinning range after decommit\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), kPageSize, 3 * kPageSize));

    status = vmo->Resize(0);
    EXPECT_EQ(ZX_ERR_BAD_STATE, status, "resizing pinned range\n");

    vmo->Unpin(kPageSize, 3 * kPageSize);

    status = vmo->Resize(0);
    EXPECT_EQ(ZX_OK, status, "resizing unpinned range\n");
  }

  END_TEST;
}

// Creates contiguous VMOs, pins them, and tries operations that should unpin.
bool vmo_pin_contiguous_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = kPageSize * 16;
  for (uint32_t is_loaning_enabled = 0; is_loaning_enabled < 2; ++is_loaning_enabled) {
    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(!!is_loaning_enabled);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status;
    status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, alloc_size,
                                             /*alignment_log2=*/0, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");

    status = vmo->CommitRangePinned(kPageSize, alloc_size, false);
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status, "pinning out of range\n");
    status = vmo->CommitRangePinned(kPageSize, 0, false);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, status, "pinning range of len 0\n");

    status = vmo->CommitRangePinned(kPageSize, 3 * kPageSize, false);
    EXPECT_EQ(ZX_OK, status, "pinning range\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), kPageSize, 3 * kPageSize));

    status = vmo->DecommitRange(kPageSize, 3 * kPageSize);
    if (!is_loaning_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting pinned range\n");
    } else {
      EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");
    }
    status = vmo->DecommitRange(kPageSize, kPageSize);
    if (!is_loaning_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting pinned range\n");
    } else {
      EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");
    }
    status = vmo->DecommitRange(3 * kPageSize, kPageSize);
    if (!is_loaning_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting pinned range\n");
    } else {
      EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");
    }

    vmo->Unpin(kPageSize, 3 * kPageSize);
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), kPageSize, 3 * kPageSize));

    status = vmo->DecommitRange(kPageSize, 3 * kPageSize);
    if (!is_loaning_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting unpinned range\n");
    } else {
      EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");
    }

    status = vmo->CommitRangePinned(kPageSize, 3 * kPageSize, false);
    EXPECT_EQ(ZX_OK, status, "pinning range after decommit\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), kPageSize, 3 * kPageSize));

    vmo->Unpin(kPageSize, 3 * kPageSize);
  }

  END_TEST;
}

// Creates a page VMO and pins the same pages multiple times
bool vmo_multiple_pin_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = kPageSize * 16;
  for (uint32_t is_ppb_enabled = 0; is_ppb_enabled < 2; ++is_ppb_enabled) {
    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(is_ppb_enabled);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status;
    status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");

    status = vmo->CommitRangePinned(0, alloc_size, false);
    EXPECT_EQ(ZX_OK, status, "pinning whole range\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), 0, alloc_size));
    status = vmo->CommitRangePinned(kPageSize, 4 * kPageSize, false);
    EXPECT_EQ(ZX_OK, status, "pinning subrange\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), 0, alloc_size));

    for (unsigned int i = 1; i < VM_PAGE_OBJECT_MAX_PIN_COUNT; ++i) {
      status = vmo->CommitRangePinned(0, kPageSize, false);
      EXPECT_EQ(ZX_OK, status, "pinning first page max times\n");
    }
    status = vmo->CommitRangePinned(0, kPageSize, false);
    EXPECT_EQ(ZX_ERR_UNAVAILABLE, status, "page is pinned too much\n");

    vmo->Unpin(0, alloc_size);
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), kPageSize, 4 * kPageSize));
    EXPECT_TRUE(PagesInAnyAnonymousQueue(vmo.get(), 5 * kPageSize, alloc_size - 5 * kPageSize));
    status = vmo->DecommitRange(kPageSize, 4 * kPageSize);
    EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");
    status = vmo->DecommitRange(5 * kPageSize, alloc_size - 5 * kPageSize);
    EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");

    vmo->Unpin(kPageSize, 4 * kPageSize);
    status = vmo->DecommitRange(kPageSize, 4 * kPageSize);
    EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");

    for (unsigned int i = 2; i < VM_PAGE_OBJECT_MAX_PIN_COUNT; ++i) {
      vmo->Unpin(0, kPageSize);
    }
    status = vmo->DecommitRange(0, kPageSize);
    EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting unpinned range\n");

    vmo->Unpin(0, kPageSize);
    status = vmo->DecommitRange(0, kPageSize);
    EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");
  }

  END_TEST;
}

// Creates a contiguous VMO and pins the same pages multiple times
bool vmo_multiple_pin_contiguous_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = kPageSize * 16;
  for (uint32_t is_ppb_enabled = 0; is_ppb_enabled < 2; ++is_ppb_enabled) {
    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(is_ppb_enabled);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status;
    status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, alloc_size,
                                             /*alignment_log2=*/0, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");

    status = vmo->CommitRangePinned(0, alloc_size, false);
    EXPECT_EQ(ZX_OK, status, "pinning whole range\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), 0, alloc_size));
    status = vmo->CommitRangePinned(kPageSize, 4 * kPageSize, false);
    EXPECT_EQ(ZX_OK, status, "pinning subrange\n");
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), 0, alloc_size));

    for (unsigned int i = 1; i < VM_PAGE_OBJECT_MAX_PIN_COUNT; ++i) {
      status = vmo->CommitRangePinned(0, kPageSize, false);
      EXPECT_EQ(ZX_OK, status, "pinning first page max times\n");
    }
    status = vmo->CommitRangePinned(0, kPageSize, false);
    EXPECT_EQ(ZX_ERR_UNAVAILABLE, status, "page is pinned too much\n");

    vmo->Unpin(0, alloc_size);
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), kPageSize, 4 * kPageSize));
    EXPECT_TRUE(PagesInWiredQueue(vmo.get(), 5 * kPageSize, alloc_size - 5 * kPageSize));
    status = vmo->DecommitRange(kPageSize, 4 * kPageSize);
    if (!is_ppb_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting pinned range\n");
    } else {
      EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting pinned range\n");
    }
    status = vmo->DecommitRange(5 * kPageSize, alloc_size - 5 * kPageSize);
    if (!is_ppb_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting unpinned range\n");
    } else {
      EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");
    }

    vmo->Unpin(kPageSize, 4 * kPageSize);
    status = vmo->DecommitRange(kPageSize, 4 * kPageSize);
    if (!is_ppb_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting unpinned range\n");
    } else {
      EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");
    }

    for (unsigned int i = 2; i < VM_PAGE_OBJECT_MAX_PIN_COUNT; ++i) {
      vmo->Unpin(0, kPageSize);
    }
    status = vmo->DecommitRange(0, kPageSize);
    if (!is_ppb_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting unpinned range\n");
    } else {
      EXPECT_EQ(ZX_ERR_BAD_STATE, status, "decommitting unpinned range\n");
    }

    vmo->Unpin(0, kPageSize);
    status = vmo->DecommitRange(0, kPageSize);
    if (!is_ppb_enabled) {
      EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "decommitting unpinned range\n");
    } else {
      EXPECT_EQ(ZX_OK, status, "decommitting unpinned range\n");
    }
  }

  END_TEST;
}

// Checks that VMOs must be page aligned sizes.
bool vmo_unaligned_size_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = 15;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_ERR_INVALID_ARGS, "vmobject creation\n");

  END_TEST;
}

// Creates a vm object, checks that attribution via reference doesn't attribute pages unless we
// specifically request it
bool vmo_reference_attribution_commit_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = 8ul * kPageSize;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  fbl::RefPtr<VmObject> vmo_reference;
  status =
      vmo->CreateChildReference(Resizability::NonResizable, 0u, 0, true, nullptr, &vmo_reference);
  ASSERT_EQ(status, ZX_OK, "vmobject reference creation\n");
  ASSERT_TRUE(vmo_reference, "vmobject reference creation\n");

  auto ret = vmo->CommitRange(0, alloc_size);
  EXPECT_EQ(ZX_OK, ret, "committing vm object\n");
  EXPECT_TRUE(make_private_attribution_counts(alloc_size, 0) == vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, alloc_size));

  EXPECT_TRUE((vm::AttributionCounts{}) == vmo_reference->GetAttributedMemory(),
              "vmo_reference attribution\n");
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo_reference, alloc_size));

  EXPECT_TRUE(make_private_attribution_counts(alloc_size, 0) ==
                  vmo_reference->GetAttributedMemoryInReferenceOwner(),
              "vmo_reference explicit reference attribution\n");

  END_TEST;
}

bool vmo_create_physical_test() {
  BEGIN_TEST;

  paddr_t pa;
  vm_page_t* vm_page;
  zx_status_t status = pmm_alloc_page(0, &vm_page, &pa);
  arch_mmu_flags_t cache_policy;

  ASSERT_EQ(ZX_OK, status, "vm page allocation\n");
  ASSERT_TRUE(vm_page);

  fbl::RefPtr<VmObjectPhysical> vmo;
  status = VmObjectPhysical::Create(pa, kPageSize, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");
  cache_policy = vmo->GetMappingCachePolicy();
  EXPECT_EQ(ARCH_MMU_FLAG_UNCACHED, cache_policy, "check initial cache policy");
  EXPECT_TRUE(vmo->is_contiguous(), "check contiguous");

  vmo.reset();
  pmm_free_page(vm_page);

  END_TEST;
}

bool vmo_physical_pin_test() {
  BEGIN_TEST;

  paddr_t pa;
  vm_page_t* vm_page;
  zx_status_t status = pmm_alloc_page(0, &vm_page, &pa);
  ASSERT_EQ(ZX_OK, status);

  fbl::RefPtr<VmObjectPhysical> vmo;
  status = VmObjectPhysical::Create(pa, kPageSize, &vmo);

  // Validate we can pin the range.
  EXPECT_EQ(ZX_OK, vmo->CommitRangePinned(0, kPageSize, false));

  // Pinning out side should fail.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo->CommitRangePinned(kPageSize, kPageSize, false));

  // Unpin for physical VMOs does not currently do anything, but still call it to be API correct.
  vmo->Unpin(0, kPageSize);

  vmo.reset();
  pmm_free_page(vm_page);

  END_TEST;
}

// Creates a vm object that commits contiguous memory.
bool vmo_create_contiguous_test() {
  BEGIN_TEST;
  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, alloc_size, 0, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  EXPECT_TRUE(vmo->is_contiguous(), "vmo is contig\n");

  // Contiguous VMOs are not pinned, but they are notionally wired as they will not be automatically
  // manipulated by the kernel.
  EXPECT_TRUE(PagesInWiredQueue(vmo.get(), 0, alloc_size));

  paddr_t last_pa;
  auto lookup_func = [&last_pa](uint64_t offset, paddr_t pa) {
    if (offset != 0 && last_pa + kPageSize != pa) {
      return ZX_ERR_BAD_STATE;
    }
    last_pa = pa;
    return ZX_ERR_NEXT;
  };
  status = vmo->Lookup(0, alloc_size, lookup_func);
  paddr_t first_pa;
  paddr_t second_pa;
  EXPECT_EQ(status, ZX_OK, "vmo lookup\n");
  EXPECT_EQ(ZX_OK, vmo->LookupContiguous(0, alloc_size, &first_pa));
  EXPECT_EQ(first_pa + alloc_size - kPageSize, last_pa);
  EXPECT_EQ(ZX_OK, vmo->LookupContiguous(kPageSize, kPageSize, &second_pa));
  EXPECT_EQ(first_pa + kPageSize, second_pa);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmo->LookupContiguous(42, kPageSize, nullptr));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo->LookupContiguous(alloc_size - kPageSize, kPageSize * 2, nullptr));

  END_TEST;
}

// Make sure decommitting pages from a contiguous VMO is allowed, and that we get back the correct
// pages when committing pages back into a contiguous VMO, even if another VMO was (temporarily)
// using those pages.
bool vmo_contiguous_decommit_test() {
  BEGIN_TEST;

  bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, alloc_size, 0, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  paddr_t base_pa = static_cast<paddr_t>(-1);
  status = vmo->Lookup(0, kPageSize, [&base_pa](size_t offset, paddr_t pa) {
    ASSERT(base_pa == static_cast<paddr_t>(-1));
    ASSERT(offset == 0);
    base_pa = pa;
    return ZX_ERR_NEXT;
  });
  ASSERT_EQ(status, ZX_OK, "stash base pa works\n");
  ASSERT_TRUE(base_pa != static_cast<paddr_t>(-1));

  bool borrowed_seen = false;

  bool page_expected[alloc_size / kPageSize];
  for (bool& present : page_expected) {
    // Default to true.
    present = true;
  }
  // Make sure expected pages (and only expected pages) are present and consistent with start
  // physical address of contiguous VMO.
  auto verify_expected_pages = [vmo, base_pa, &borrowed_seen, &page_expected]() -> void {
    auto cow = vmo->DebugGetCowPages();
    bool page_seen[alloc_size / kPageSize] = {};
    auto lookup_func = [base_pa, &page_seen](size_t offset, paddr_t pa) {
      ASSERT(!page_seen[offset / kPageSize]);
      page_seen[offset / kPageSize] = true;
      if (pa - base_pa != offset) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    };
    zx_status_t status = vmo->Lookup(0, alloc_size, lookup_func);
    ASSERT_MSG(status == ZX_OK, "vmo->Lookup() failed - status: %d\n", status);
    for (uint64_t offset = 0; offset < alloc_size; offset += kPageSize) {
      uint64_t page_index = offset / kPageSize;
      ASSERT_MSG(page_expected[page_index] == page_seen[page_index],
                 "page_expected[page_index] != page_seen[page_index]\n");
      vm_page_t* page_from_cow = cow->DebugGetPage(offset);
      vm_page_t* page_from_pmm = paddr_to_vm_page(base_pa + offset);
      ASSERT(page_from_pmm);
      if (page_expected[page_index]) {
        ASSERT(page_from_cow);
        ASSERT(page_from_cow == page_from_pmm);
        ASSERT(cow->DebugIsPage(offset));
        ASSERT(!page_from_pmm->is_loaned());
      } else {
        ASSERT(!page_from_cow);
        ASSERT(cow->DebugIsEmpty(offset));
        ASSERT(page_from_pmm->is_loaned());
        if (!page_from_pmm->is_free()) {
          // It's not in cow, and it's not free, so note that we observed a borrowed page.
          borrowed_seen = true;
        }
      }
      ASSERT(!page_from_pmm->is_loan_cancelled());
    }
  };
  verify_expected_pages();
  auto track_decommit = [vmo, &page_expected, &verify_expected_pages](uint64_t start_offset,
                                                                      uint64_t size) {
    ASSERT(IsPageRounded(start_offset));
    ASSERT(IsPageRounded(size));
    uint64_t end_offset = start_offset + size;
    for (uint64_t offset = start_offset; offset < end_offset; offset += kPageSize) {
      page_expected[offset / kPageSize] = false;
    }
    verify_expected_pages();
  };
  auto track_commit = [vmo, &page_expected, &verify_expected_pages](uint64_t start_offset,
                                                                    uint64_t size) {
    ASSERT(IsPageRounded(start_offset));
    ASSERT(IsPageRounded(size));
    uint64_t end_offset = start_offset + size;
    for (uint64_t offset = start_offset; offset < end_offset; offset += kPageSize) {
      page_expected[offset / kPageSize] = true;
    }
    verify_expected_pages();
  };

  status = vmo->DecommitRange(kPageSize, 4 * kPageSize);
  ASSERT_EQ(status, ZX_OK, "decommit of contiguous VMO pages works\n");
  track_decommit(kPageSize, 4 * kPageSize);

  status = vmo->DecommitRange(0, 4 * kPageSize);
  ASSERT_EQ(status, ZX_OK,
            "decommit of contiguous VMO pages overlapping non-present pages works\n");
  track_decommit(0, 4 * kPageSize);

  status = vmo->DecommitRange(alloc_size - kPageSize, kPageSize);
  ASSERT_EQ(status, ZX_OK, "decommit at end of contiguous VMO works\n");
  track_decommit(alloc_size - kPageSize, kPageSize);

  status = vmo->DecommitRange(0, alloc_size);
  ASSERT_EQ(status, ZX_OK, "decommit all overlapping non-present pages\n");
  track_decommit(0, alloc_size);

  // Due to concurrent activity of the system, we may not be able to allocate the loaned pages into
  // a VMO we're creating here, and depending on timing, we may also not observe the pages being
  // borrowed.  However, it shouldn't take many tries, if we continue to allocate non-pinned pages
  // to a VMO repeatedly, since loaned pages are preferred for allocations that can use them.
  //
  // We pay attention to whether ASAN is enabled in order to apply a strategy that's optimized for
  // pages being put on the head (normal) or tail (ASAN) of the free list
  // (PmmNode::free_loaned_list_).

  // Reset borrowed_seen since we should be able to see borrowing _within_ the loop below, mainly
  // so we can also have the loop below do a CommitRange() to reclaim before the borrowing VMO is
  // deleted.
  borrowed_seen = false;
  zx_instant_mono_t complain_deadline = current_mono_time() + ZX_SEC(5);
  uint32_t loop_count = 0;
  while (!borrowed_seen || loop_count < 5) {
    // Not super small, in case we end up needing to do multiple iterations of the loop to see the
    // pages being borrowed, and ASAN is enabled which could require more iterations of this loop
    // if this size were smaller.  Also hopefully not big enough to fail on small-ish devices.
    constexpr uint64_t kBorrowingVmoPages = 64;
    vm_page_t* pages[kBorrowingVmoPages];
    fbl::RefPtr<VmObjectPaged> borrowing_vmo;
    status = make_committed_pager_vmo(kBorrowingVmoPages, /*trap_dirty=*/false, /*resizable=*/false,
                                      &pages[0], &borrowing_vmo);
    ASSERT_EQ(status, ZX_OK);

    // Updates borrowing_seen to true, if any pages of vmo are seen to be borrowed (maybe by
    // borrowing_vmo, or maybe by some other VMO; we don't care which here).
    verify_expected_pages();

    // We want the last iteration of the loop to have seen borrowing itself, so we're sure the else
    // case below runs (to commit) before the borrowing VMO is deleted.
    if (loop_count < 5) {
      borrowed_seen = false;
    }

    if (!borrowed_seen || loop_count < 5) {
      if constexpr (!__has_feature(address_sanitizer)) {
        // By committing and de-committing in the loop, we put the pages we're paying attention to
        // back at the head of the free_loaned_list_, so the next iteration of the loop is more
        // likely to see them being borrowed (by allocating them).
        status = vmo->CommitRange(0, 4 * kPageSize);
        ASSERT_EQ(status, ZX_OK, "temp commit back to contiguous VMO, to remove from free list\n");
        track_commit(0, 4 * kPageSize);

        status = vmo->DecommitRange(0, 4 * kPageSize);
        ASSERT_EQ(status, ZX_OK, "decommit back to free list at head of free list\n");
        track_decommit(0, 4 * kPageSize);
      } else {
        // By _not_ committing and de-committing in the loop, the pages we're allocating in a loop
        // will eventually work through the free_loaned_list_, even if a large contiguous VMO was
        // decomitted at an inconvenient time.
      }
      zx_instant_mono_t now = current_mono_time();
      if (now > complain_deadline) {
        dprintf(INFO, "!borrowed_seen is persisting longer than expected; still trying...\n");
        complain_deadline = now + ZX_SEC(5);
      }
    } else {
      // This covers the case where a page is reclaimed before being freed from the borrowing VMO.
      // And by forcing an iteration with loop_count >= 1 with the last iteration of the loop seeing
      // borrowing durign the last iteration, we cover the case where we free the pages from the
      // borrowing VMO before reclaiming.
      status = vmo->CommitRange(0, alloc_size);
      ASSERT_EQ(status, ZX_OK, "committed pages back into contiguous VMO\n");
      track_commit(0, alloc_size);
    }
    ++loop_count;
  }

  status = vmo->DecommitRange(0, alloc_size);
  ASSERT_EQ(status, ZX_OK, "decommit from contiguous VMO\n");

  status = vmo->CommitRange(0, alloc_size);
  ASSERT_EQ(status, ZX_OK, "committed pages back into contiguous VMO\n");
  track_commit(0, alloc_size);

  END_TEST;
}

bool vmo_contiguous_decommit_disabled_test() {
  BEGIN_TEST;

  bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(false);
  auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, alloc_size, 0, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  status = vmo->DecommitRange(kPageSize, 4 * kPageSize);
  ASSERT_EQ(status, ZX_ERR_NOT_SUPPORTED, "decommit fails as expected\n");
  status = vmo->DecommitRange(0, 4 * kPageSize);
  ASSERT_EQ(status, ZX_ERR_NOT_SUPPORTED, "decommit fails as expected\n");
  status = vmo->DecommitRange(alloc_size - kPageSize, kPageSize);
  ASSERT_EQ(status, ZX_ERR_NOT_SUPPORTED, "decommit fails as expected\n");

  END_TEST;
}

bool vmo_contiguous_decommit_enabled_test() {
  BEGIN_TEST;

  bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, alloc_size, 0, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  // Scope the memsetting so that the kernel mapping does not keep existing to the point that the
  // Decommits happen below. As those decommits would need to perform unmaps, and we prefer to not
  // modify kernel mappings in this way, we just remove the kernel region.
  {
    auto ka = VmAspace::kernel_aspace();
    void* ptr;
    auto ret = ka->MapObjectInternal(vmo, "test", 0, alloc_size, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
                                     kArchRwFlags);
    ASSERT_EQ(ZX_OK, ret, "mapping object");
    auto cleanup_mapping = fit::defer([&ka, ptr] {
      auto err = ka->FreeRegion((vaddr_t)ptr);
      DEBUG_ASSERT(err == ZX_OK);
    });
    uint8_t* base = reinterpret_cast<uint8_t*>(ptr);

    for (uint64_t offset = 0; offset < alloc_size; offset += kPageSize) {
      memset(&base[offset], 0x42, kPageSize);
    }
  }

  paddr_t base_pa = -1;
  status = vmo->Lookup(0, kPageSize, [&base_pa](uint64_t offset, paddr_t pa) {
    DEBUG_ASSERT(offset == 0);
    base_pa = pa;
    return ZX_ERR_NEXT;
  });
  ASSERT_EQ(status, ZX_OK);
  ASSERT_TRUE(base_pa != static_cast<paddr_t>(-1));

  status = vmo->DecommitRange(kPageSize, 4 * kPageSize);
  ASSERT_EQ(status, ZX_OK, "decommit pretends to work\n");
  status = vmo->DecommitRange(0, 4 * kPageSize);
  ASSERT_EQ(status, ZX_OK, "decommit pretends to work\n");
  status = vmo->DecommitRange(alloc_size - kPageSize, kPageSize);
  ASSERT_EQ(status, ZX_OK, "decommit pretends to work\n");

  // Make sure decommit removed pages.  Make sure pages which are present are the correct physical
  // address.
  for (uint64_t offset = 0; offset < alloc_size; offset += kPageSize) {
    bool page_absent = true;
    status = vmo->Lookup(offset, kPageSize,
                         [base_pa, offset, &page_absent](uint64_t lookup_offset, paddr_t pa) {
                           // TODO(johngro): remove this explicit unused-capture warning suppression
                           // when https://bugs.llvm.org/show_bug.cgi?id=35450 gets fixed.
                           (void)base_pa;
                           (void)offset;

                           page_absent = false;
                           DEBUG_ASSERT(offset == lookup_offset);
                           DEBUG_ASSERT(base_pa + lookup_offset == pa);
                           return ZX_ERR_NEXT;
                         });
    bool absent_expected = (offset < 5 * kPageSize) || (offset == alloc_size - kPageSize);
    ASSERT_EQ(absent_expected, page_absent);
  }

  END_TEST;
}

// Creates a vm object, maps it, precommitted.
bool vmo_precommitted_map_test() {
  BEGIN_TEST;
  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  auto ka = VmAspace::kernel_aspace();
  void* ptr;
  auto ret = ka->MapObjectInternal(vmo, "test", 0, alloc_size, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
                                   kArchRwFlags);
  ASSERT_EQ(ZX_OK, ret, "mapping object");

  // fill with known pattern and test
  if (!fill_and_test(ptr, alloc_size)) {
    all_ok = false;
  }

  auto err = ka->FreeRegion((vaddr_t)ptr);
  EXPECT_EQ(ZX_OK, err, "unmapping object");
  END_TEST;
}

// Creates a vm object, maps it, demand paged.
bool vmo_demand_paged_map_test() {
  BEGIN_TEST;

  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test aspace");
  ASSERT_NONNULL(aspace, "VmAspace::Create pointer");

  VmAspace* old_aspace = Thread::Current::active_aspace();
  auto cleanup_aspace = fit::defer([&]() {
    vmm_set_active_aspace(old_aspace);
    ASSERT(aspace->Destroy() == ZX_OK);
  });
  vmm_set_active_aspace(aspace.get());

  constexpr uint kArchFlags = kArchRwFlags | ARCH_MMU_FLAG_PERM_USER;
  auto mapping_result =
      aspace->RootVmar()->CreateVmMapping(0, alloc_size, 0, 0, vmo, 0, kArchFlags, "test");
  ASSERT_MSG(mapping_result.is_ok(), "mapping object");

  auto uptr = make_user_inout_ptr(reinterpret_cast<void*>(mapping_result->base));

  // fill with known pattern and test
  if (!fill_and_test_user(uptr, alloc_size)) {
    all_ok = false;
  }

  // cleanup_aspace destroys the whole space now.

  END_TEST;
}

// Creates a vm object, maps it, drops ref before unmapping.
bool vmo_dropped_ref_test() {
  BEGIN_TEST;
  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  auto ka = VmAspace::kernel_aspace();
  void* ptr;
  auto ret = ka->MapObjectInternal(ktl::move(vmo), "test", 0, alloc_size, &ptr, 0,
                                   VmAspace::VMM_FLAG_COMMIT, kArchRwFlags);
  ASSERT_EQ(ret, ZX_OK, "mapping object");

  EXPECT_NULL(vmo, "dropped ref to object");

  // fill with known pattern and test
  if (!fill_and_test(ptr, alloc_size)) {
    all_ok = false;
  }

  auto err = ka->FreeRegion((vaddr_t)ptr);
  EXPECT_EQ(ZX_OK, err, "unmapping object");
  END_TEST;
}

// Creates a vm object, maps it, fills it with data, unmaps,
// maps again somewhere else.
bool vmo_remap_test() {
  BEGIN_TEST;
  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  auto ka = VmAspace::kernel_aspace();
  void* ptr;
  auto ret = ka->MapObjectInternal(vmo, "test", 0, alloc_size, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
                                   kArchRwFlags);
  ASSERT_EQ(ZX_OK, ret, "mapping object");

  // fill with known pattern and test.  The initial virtual address will be used
  // to generate the seed which is used to generate the fill pattern.  Make sure
  // we save it off right now to use when we test the fill pattern later on
  // after re-mapping.
  const uintptr_t fill_seed = reinterpret_cast<uintptr_t>(ptr);
  if (!fill_and_test(ptr, alloc_size)) {
    all_ok = false;
  }

  auto err = ka->FreeRegion((vaddr_t)ptr);
  EXPECT_EQ(ZX_OK, err, "unmapping object");

  // map it again
  ret = ka->MapObjectInternal(vmo, "test", 0, alloc_size, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
                              kArchRwFlags);
  ASSERT_EQ(ret, ZX_OK, "mapping object");

  // test that the pattern is still valid.  Be sure to use the original seed we
  // saved off earlier when verifying.
  bool result = test_region(fill_seed, ptr, alloc_size);
  EXPECT_TRUE(result, "testing region for corruption");

  err = ka->FreeRegion((vaddr_t)ptr);
  EXPECT_EQ(ZX_OK, err, "unmapping object");
  END_TEST;
}

// Creates a vm object, maps it, fills it with data, maps it a second time and
// third time somwehere else.
bool vmo_double_remap_test() {
  BEGIN_TEST;
  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  auto ka = VmAspace::kernel_aspace();
  void* ptr;
  auto ret = ka->MapObjectInternal(vmo, "test0", 0, alloc_size, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
                                   kArchRwFlags);
  ASSERT_EQ(ZX_OK, ret, "mapping object");

  // fill with known pattern and test
  if (!fill_and_test(ptr, alloc_size)) {
    all_ok = false;
  }

  // map it again
  void* ptr2;
  ret = ka->MapObjectInternal(vmo, "test1", 0, alloc_size, &ptr2, 0, VmAspace::VMM_FLAG_COMMIT,
                              kArchRwFlags);
  ASSERT_EQ(ret, ZX_OK, "mapping object second time");
  EXPECT_NE(ptr, ptr2, "second mapping is different");

  // test that the pattern is still valid
  bool result = test_region((uintptr_t)ptr, ptr2, alloc_size);
  EXPECT_TRUE(result, "testing region for corruption");

  // map it a third time with an offset
  void* ptr3;
  constexpr size_t alloc_offset = kPageSize;
  ret = ka->MapObjectInternal(vmo, "test2", alloc_offset, alloc_size - alloc_offset, &ptr3, 0,
                              VmAspace::VMM_FLAG_COMMIT, kArchRwFlags);
  ASSERT_EQ(ret, ZX_OK, "mapping object third time");
  EXPECT_NE(ptr3, ptr2, "third mapping is different");
  EXPECT_NE(ptr3, ptr, "third mapping is different");

  // test that the pattern is still valid
  int mc = memcmp((uint8_t*)ptr + alloc_offset, ptr3, alloc_size - alloc_offset);
  EXPECT_EQ(0, mc, "testing region for corruption");

  ret = ka->FreeRegion((vaddr_t)ptr3);
  EXPECT_EQ(ZX_OK, ret, "unmapping object third time");

  ret = ka->FreeRegion((vaddr_t)ptr2);
  EXPECT_EQ(ZX_OK, ret, "unmapping object second time");

  ret = ka->FreeRegion((vaddr_t)ptr);
  EXPECT_EQ(ZX_OK, ret, "unmapping object");
  END_TEST;
}

bool vmo_read_write_smoke_test() {
  BEGIN_TEST;
  constexpr size_t alloc_size = kPageSize * 16;

  // create object
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  // create test buffer
  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> a;
  a.reserve(alloc_size, &ac);
  ASSERT_TRUE(ac.check());
  fill_region(99, a.data(), alloc_size);

  // write to it, make sure it seems to work with valid args
  zx_status_t err = vmo->Write(a.data(), 0, 0);
  EXPECT_EQ(ZX_OK, err, "writing to object");

  err = vmo->Write(a.data(), 0, 37);
  EXPECT_EQ(ZX_OK, err, "writing to object");

  err = vmo->Write(a.data(), 99, 37);
  EXPECT_EQ(ZX_OK, err, "writing to object");

  // can't write past end
  err = vmo->Write(a.data(), 0, alloc_size + 47);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, err, "writing to object");

  // can't write past end
  err = vmo->Write(a.data(), 31, alloc_size + 47);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, err, "writing to object");

  // should return an error because out of range
  err = vmo->Write(a.data(), alloc_size + 99, 42);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, err, "writing to object");

  // map the object
  auto ka = VmAspace::kernel_aspace();
  uint8_t* ptr;
  err = ka->MapObjectInternal(vmo, "test", 0, alloc_size, (void**)&ptr, 0,
                              VmAspace::VMM_FLAG_COMMIT, kArchRwFlags);
  ASSERT_EQ(ZX_OK, err, "mapping object");

  // write to it at odd offsets
  err = vmo->Write(a.data(), 31, 4197);
  EXPECT_EQ(ZX_OK, err, "writing to object");
  int cmpres = memcmp(ptr + 31, a.data(), 4197);
  EXPECT_EQ(0, cmpres, "reading from object");

  // write to it, filling the object completely
  err = vmo->Write(a.data(), 0, alloc_size);
  EXPECT_EQ(ZX_OK, err, "writing to object");

  // test that the data was actually written to it
  bool result = test_region(99, ptr, alloc_size);
  EXPECT_TRUE(result, "writing to object");

  // unmap it
  ka->FreeRegion((vaddr_t)ptr);

  // test that we can read from it
  fbl::Vector<uint8_t> b;
  b.reserve(alloc_size, &ac);
  ASSERT_TRUE(ac.check(), "can't allocate buffer");

  err = vmo->Read(b.data(), 0, alloc_size);
  EXPECT_EQ(ZX_OK, err, "reading from object");

  // validate the buffer is valid
  cmpres = memcmp(b.data(), a.data(), alloc_size);
  EXPECT_EQ(0, cmpres, "reading from object");

  // read from it at an offset
  err = vmo->Read(b.data(), 31, 4197);
  EXPECT_EQ(ZX_OK, err, "reading from object");
  cmpres = memcmp(b.data(), a.data() + 31, 4197);
  EXPECT_EQ(0, cmpres, "reading from object");
  END_TEST;
}

bool vmo_cache_test() {
  BEGIN_TEST;

  paddr_t pa;
  vm_page_t* vm_page;
  zx_status_t status = pmm_alloc_page(0, &vm_page, &pa);
  auto ka = VmAspace::kernel_aspace();
  arch_mmu_flags_t cache_policy = ARCH_MMU_FLAG_UNCACHED_DEVICE;
  arch_mmu_flags_t cache_policy_get;
  void* ptr;

  ASSERT_TRUE(vm_page);
  // Test that the flags set/get properly
  {
    fbl::RefPtr<VmObjectPhysical> vmo;
    status = VmObjectPhysical::Create(pa, kPageSize, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");
    cache_policy_get = vmo->GetMappingCachePolicy();
    EXPECT_NE(cache_policy, cache_policy_get, "check initial cache policy");
    EXPECT_EQ(ZX_OK, vmo->SetMappingCachePolicy(cache_policy), "try set");
    cache_policy_get = vmo->GetMappingCachePolicy();
    EXPECT_EQ(cache_policy, cache_policy_get, "compare flags");
  }

  // Test valid flags
  for (uint32_t i = 0; i <= ARCH_MMU_FLAG_CACHE_MASK; i++) {
    fbl::RefPtr<VmObjectPhysical> vmo;
    status = VmObjectPhysical::Create(pa, kPageSize, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");
    EXPECT_EQ(ZX_OK, vmo->SetMappingCachePolicy(cache_policy), "try setting valid flags");
  }

  // Test invalid flags
  for (uint32_t i = ARCH_MMU_FLAG_CACHE_MASK + 1; i < 32; i++) {
    fbl::RefPtr<VmObjectPhysical> vmo;
    status = VmObjectPhysical::Create(pa, kPageSize, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmo->SetMappingCachePolicy(static_cast<arch_mmu_flags_t>(i)),
              "try set with invalid flags");
  }

  // Test valid flags with invalid flags
  {
    fbl::RefPtr<VmObjectPhysical> vmo;
    status = VmObjectPhysical::Create(pa, kPageSize, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              vmo->SetMappingCachePolicy(static_cast<arch_mmu_flags_t>(cache_policy | 0x5)),
              "bad 0x5");
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              vmo->SetMappingCachePolicy(static_cast<arch_mmu_flags_t>(cache_policy | 0xA)),
              "bad 0xA");
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              vmo->SetMappingCachePolicy(static_cast<arch_mmu_flags_t>(cache_policy | 0x55)),
              "bad 0x55");
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              vmo->SetMappingCachePolicy(static_cast<arch_mmu_flags_t>(cache_policy | 0xAA)),
              "bad 0xAA");
  }

  // Test that changing policy while mapped is blocked
  {
    fbl::RefPtr<VmObjectPhysical> vmo;
    status = VmObjectPhysical::Create(pa, kPageSize, &vmo);
    ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
    ASSERT_TRUE(vmo, "vmobject creation\n");
    ASSERT_EQ(ZX_OK,
              ka->MapObjectInternal(vmo, "test", 0, kPageSize, (void**)&ptr, 0,
                                    VmAspace::VMM_FLAG_COMMIT, kArchRwFlags),
              "map vmo");
    EXPECT_EQ(ZX_ERR_BAD_STATE, vmo->SetMappingCachePolicy(cache_policy), "set flags while mapped");
    EXPECT_EQ(ZX_OK, ka->FreeRegion((vaddr_t)ptr), "unmap vmo");
    EXPECT_EQ(ZX_OK, vmo->SetMappingCachePolicy(cache_policy), "set flags after unmapping");
    ASSERT_EQ(ZX_OK,
              ka->MapObjectInternal(vmo, "test", 0, kPageSize, (void**)&ptr, 0,
                                    VmAspace::VMM_FLAG_COMMIT, kArchRwFlags),
              "map vmo again");
    EXPECT_EQ(ZX_OK, ka->FreeRegion((vaddr_t)ptr), "unmap vmo");
  }

  pmm_free_page(vm_page);
  END_TEST;
}

bool vmo_lookup_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t alloc_size = kPageSize * 16;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo);
  ASSERT_EQ(status, ZX_OK, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  size_t pages_seen = 0;
  auto lookup_fn = [&pages_seen](size_t offset, paddr_t pa) {
    pages_seen++;
    return ZX_ERR_NEXT;
  };
  status = vmo->Lookup(0, alloc_size, lookup_fn);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(0u, pages_seen, "lookup on uncommitted pages\n");
  pages_seen = 0;

  status = vmo->CommitRange(kPageSize, kPageSize);
  EXPECT_EQ(ZX_OK, status, "committing vm object\n");
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory(),
              "committing vm object\n");
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize), "committing vm object\n");

  // Should not see any pages in the early range.
  status = vmo->Lookup(0, kPageSize, lookup_fn);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(0u, pages_seen, "lookup on partially committed pages\n");
  pages_seen = 0;

  // Should see a committed page if looking at any range covering the committed.
  status = vmo->Lookup(0, alloc_size, lookup_fn);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(1u, pages_seen, "lookup on partially committed pages\n");
  pages_seen = 0;

  status = vmo->Lookup(kPageSize, alloc_size - kPageSize, lookup_fn);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(1u, pages_seen, "lookup on partially committed pages\n");
  pages_seen = 0;

  status = vmo->Lookup(kPageSize, kPageSize, lookup_fn);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(1u, pages_seen, "lookup on partially committed pages\n");
  pages_seen = 0;

  // Contiguous lookups of single pages should also succeed
  status = vmo->LookupContiguous(kPageSize, kPageSize, nullptr);
  EXPECT_EQ(ZX_OK, status, "contiguous lookup of single page\n");

  // Commit the rest
  status = vmo->CommitRange(0, alloc_size);
  EXPECT_EQ(ZX_OK, status, "committing vm object\n");
  EXPECT_TRUE(make_private_attribution_counts(alloc_size, 0) == vmo->GetAttributedMemory(),
              "committing vm object\n");
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, alloc_size), "committing vm object\n");

  status = vmo->Lookup(0, alloc_size, lookup_fn);
  EXPECT_EQ(ZX_OK, status, "lookup on partially committed pages\n");
  EXPECT_EQ(alloc_size / kPageSize, pages_seen, "lookup on partially committed pages\n");
  status = vmo->LookupContiguous(0, kPageSize, nullptr);
  EXPECT_EQ(ZX_OK, status, "contiguous lookup of single page\n");
  status = vmo->LookupContiguous(0, alloc_size, nullptr);
  EXPECT_NE(ZX_OK, status, "contiguous lookup of multiple pages\n");

  END_TEST;
}

bool vmo_lookup_slice_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kAllocSize = kPageSize * 16;
  constexpr size_t kCommitOffset = kPageSize * 4;
  constexpr size_t kSliceOffset = kPageSize;
  constexpr size_t kSliceSize = kAllocSize - kSliceOffset;
  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kAllocSize, &vmo));
  ASSERT_TRUE(vmo);

  // Commit a page in the vmo.
  ASSERT_OK(vmo->CommitRange(kCommitOffset, kPageSize));

  // Create a slice that is offset slightly.
  fbl::RefPtr<VmObject> slice;
  ASSERT_OK(vmo->CreateChildSlice(kSliceOffset, kSliceSize, false, &slice));
  ASSERT_TRUE(slice);

  // Query the slice and validate we see one page at the offset relative to us, not the parent it is
  // committed in.
  uint64_t offset_seen = UINT64_MAX;

  auto lookup_fn = [&offset_seen](size_t offset, paddr_t pa) {
    ASSERT(offset_seen == UINT64_MAX);
    offset_seen = offset;
    return ZX_ERR_NEXT;
  };
  EXPECT_OK(slice->Lookup(0, kSliceSize, lookup_fn));

  EXPECT_EQ(offset_seen, kCommitOffset - kSliceOffset);

  END_TEST;
}

bool vmo_lookup_clone_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t page_count = 4;
  constexpr size_t alloc_size = kPageSize * page_count;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &vmo);
  ASSERT_EQ(ZX_OK, status, "vmobject creation\n");
  ASSERT_TRUE(vmo, "vmobject creation\n");

  vmo->set_user_id(ZX_KOID_KERNEL);

  // Commit the whole original VMO and the first and last page of the clone.
  status = vmo->CommitRange(0, alloc_size);
  ASSERT_EQ(ZX_OK, status, "vmobject creation\n");

  fbl::RefPtr<VmObject> clone;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, alloc_size, false,
                            &clone);
  ASSERT_EQ(ZX_OK, status, "vmobject creation\n");
  ASSERT_TRUE(clone, "vmobject creation\n");

  clone->set_user_id(ZX_KOID_KERNEL);

  status = clone->CommitRange(0, kPageSize);
  ASSERT_EQ(ZX_OK, status, "vmobject creation\n");
  status = clone->CommitRange(alloc_size - kPageSize, kPageSize);
  ASSERT_EQ(ZX_OK, status, "vmobject creation\n");

  // Lookup the paddrs for both VMOs.
  paddr_t vmo_lookup[page_count] = {};
  paddr_t clone_lookup[page_count] = {};
  auto vmo_lookup_func = [&vmo_lookup](uint64_t offset, paddr_t pa) {
    vmo_lookup[offset / kPageSize] = pa;
    return ZX_ERR_NEXT;
  };
  auto clone_lookup_func = [&clone_lookup](uint64_t offset, paddr_t pa) {
    clone_lookup[offset / kPageSize] = pa;
    return ZX_ERR_NEXT;
  };
  status = vmo->Lookup(0, alloc_size, vmo_lookup_func);
  EXPECT_EQ(ZX_OK, status, "vmo lookup\n");
  status = clone->Lookup(0, alloc_size, clone_lookup_func);
  EXPECT_EQ(ZX_OK, status, "vmo lookup\n");

  // The original VMO is now copy-on-write so we should see none of its pages,
  // and we should only see the two pages that explicitly committed into the clone.
  for (unsigned i = 0; i < page_count; i++) {
    EXPECT_EQ(0ul, vmo_lookup[i], "Bad paddr\n");
    if (i == 0 || i == page_count - 1) {
      EXPECT_NE(0ul, clone_lookup[i], "Bad paddr\n");
    }
  }

  END_TEST;
}

bool vmo_clone_removes_write_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Create and map a VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "vmo create");

  // Use UserMemory to map the VMO, instead of mapping into the kernel aspace, so that we can freely
  // cause the mappings to modified as a consequence of the clone operation. Causing kernel mappings
  // to get modified in such a way is preferably avoided.
  ktl::unique_ptr<testing::UserMemory> mapping = testing::UserMemory::Create(vmo);
  ASSERT_NONNULL(mapping);
  status = mapping->CommitAndMap(kPageSize);
  EXPECT_OK(status);

  // Query the aspace and validate there is a writable mapping.
  paddr_t paddr_writable;
  arch_mmu_flags_t mmu_flags;
  status = mapping->aspace()->arch_aspace().Query(mapping->base(), &paddr_writable, &mmu_flags);
  EXPECT_EQ(ZX_OK, status, "query aspace");

  EXPECT_TRUE(mmu_flags & ARCH_MMU_FLAG_PERM_WRITE, "mapping is writable check");

  // Clone the VMO, which causes the parent to have to downgrade any mappings to read-only so that
  // copy-on-write can take place. Need to set a fake user id so that the COW creation code is
  // happy.
  vmo->set_user_id(42);
  fbl::RefPtr<VmObject> clone;
  status =
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, true, &clone);
  EXPECT_EQ(ZX_OK, status, "create clone");

  // Aspace should now have a read only mapping with the same underlying page.
  paddr_t paddr_readable;
  status = mapping->aspace()->arch_aspace().Query(mapping->base(), &paddr_readable, &mmu_flags);
  EXPECT_EQ(ZX_OK, status, "query aspace");
  EXPECT_FALSE(mmu_flags & ARCH_MMU_FLAG_PERM_WRITE, "mapping is read only check");
  EXPECT_EQ(paddr_writable, paddr_readable, "mapping has same page");

  END_TEST;
}

// Test that when creating or destroying clones that compressed pages, even if forked, do not need
// to get unnecessarily uncompressed.
bool vmo_clones_of_compressed_pages_test() {
  BEGIN_TEST;

  // Need a compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  AutoVmScannerDisable scanner_disable;

  // Create a VMO and make one of its pages compressed.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize, &vmo);
  ASSERT_OK(status);
  // Set ids so that attribution can work correctly.
  vmo->set_user_id(42);

  status = vmo->CommitRange(0, kPageSize);
  EXPECT_OK(status);

  // Write non-zero data to the page.
  uint64_t data = 42;
  EXPECT_OK(vmo->Write(&data, 0, sizeof(data)));

  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  vm_page_t* page = nullptr;
  status = vmo->GetPageBlocking(0, 0, &page, nullptr);
  ASSERT_OK(status);
  ASSERT_NONNULL(page);
  {
    auto compressor = compression->AcquireCompressor();
    ASSERT_OK(compressor.get().Arm());
    uint64_t reclaimed =
        compress_page(vmo, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get());
    EXPECT_EQ(reclaimed, 1u);
  }
  page = nullptr;
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  // Creating a clone should keep the page compressed.
  fbl::RefPtr<VmObject> clone;
  status =
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, true, &clone);
  ASSERT_OK(status);
  clone->set_user_id(43);
  EXPECT_TRUE((VmObject::AttributionCounts{.compressed_bytes = kPageSize,
                                           .scaled_compressed_bytes = vm::FractionalBytes(
                                               kPageSize, 2)}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_TRUE((VmObject::AttributionCounts{.compressed_bytes = kPageSize,
                                           .scaled_compressed_bytes = vm::FractionalBytes(
                                               kPageSize, 2)}) == clone->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, kPageSize));

  // Forking the page into a child will decompress in order to do the copy.
  status = clone->Write(&data, 0, sizeof(data));
  EXPECT_OK(status);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == clone->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, kPageSize));

  // Compress the parent page again by reaching into the hidden VMO parent.
  fbl::RefPtr<VmCowPages> hidden_root = vmo->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(hidden_root);
  page = hidden_root->DebugGetPage(0);
  ASSERT_NONNULL(page);
  {
    auto compressor = compression->AcquireCompressor();

    ASSERT_OK(compressor.get().Arm());
    uint64_t reclaimed =
        reclaim(hidden_root, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get());
    EXPECT_EQ(reclaimed, 1u);
  }
  page = nullptr;
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == clone->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, kPageSize));

  // Closing the child VMO should allow the now merged VMO to just have the compressed page without
  // causing it to be decompressed.
  clone.reset();
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  END_TEST;
}

// Test that CoW clones mapped into the kernel behave correctly if a 'parent' page gets compressed.
bool vmo_clone_kernel_mapped_compressed_test() {
  BEGIN_TEST;

  // Need a compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  AutoVmScannerDisable scanner_disable;

  // Create a VMO and write to its page to commit it.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize, &vmo);
  ASSERT_OK(status);

  uint64_t data = 42;
  EXPECT_OK(vmo->Write(&data, 0, sizeof(data)));

  // Now create a child of the VMO and fork the page into it.
  fbl::RefPtr<VmObject> clone;
  ASSERT_OK(
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, true, &clone));
  data = 41;
  EXPECT_OK(clone->Write(&data, 0, sizeof(data)));

  // Pin and map the clone into the kernel aspace.
  PinnedVmObject pinned_vmo;
  ASSERT_OK(PinnedVmObject::Create(clone, 0, kPageSize, true, &pinned_vmo));
  fbl::RefPtr<VmMapping> mapping;
  auto result = VmAspace::kernel_aspace()->RootVmar()->CreateVmMapping(
      0, kPageSize, 0, VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, clone, 0,
      ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE, "pin clone map");
  ASSERT_TRUE(result.is_ok());
  mapping = ktl::move(result->mapping);
  auto cleanup = fit::defer([&]() { mapping->Destroy(); });
  ASSERT_OK(mapping->MapRange(0, kPageSize, true));
  volatile uint64_t* ptr = reinterpret_cast<volatile uint64_t*>(result->base);

  // Should be able to use ptr without a fault.
  EXPECT_EQ(*ptr, 41u);

  // Compress the parent page by reaching into the hidden VMO parent.
  fbl::RefPtr<VmCowPages> hidden_root = vmo->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(hidden_root);
  vm_page_t* page = hidden_root->DebugGetPage(0);
  ASSERT_NONNULL(page);
  {
    auto compressor = compression->AcquireCompressor();
    ASSERT_OK(compressor.get().Arm());
    // Attempt to reclaim the page in the hidden parent. As the clone is a fully committed and
    // mapped VMO this should *not* cause any of its pages to be unmapped. If it did this violate
    // the requirement that kernel mappings are always pinned and mapped.
    uint64_t reclaimed =
        reclaim(hidden_root, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get());
    EXPECT_EQ(reclaimed, 1u);
    page = nullptr;
  }

  // Should still be able to touch the ptr;
  EXPECT_EQ(*ptr, 41u);

  END_TEST;
}

bool vmo_move_pages_on_access_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Our page should now be in a pager backed page queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page));

  PageRequest request;
  // If we lookup the page then it should be moved to specifically the first page queue.
  status = vmo->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr);
  EXPECT_EQ(ZX_OK, status);
  size_t queue;
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Rotate the queues and check the page moves.
  pmm_page_queues()->RotateReclaimQueues();
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(1u, queue);

  // Touching the page should move it back to the first queue.
  status = vmo->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Touching pages in a child should also move the page to the front of the queues.
  fbl::RefPtr<VmObject> child;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, kPageSize, true,
                            &child);
  ASSERT_EQ(ZX_OK, status);

  status = child->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);
  pmm_page_queues()->RotateReclaimQueues();
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(1u, queue);
  status = child->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  END_TEST;
}

bool vmo_eviction_hints_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with two pages.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* pages[2];
  zx_status_t status =
      make_committed_pager_vmo(2, /*trap_dirty=*/false, /*resizable=*/false, pages, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Newly created page should be in the first pager backed page queue.
  size_t queue;
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(0u, queue);

  // Hint that first page is not needed.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));

  // The page should now have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Hint that the page is always needed.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));

  // If the page was loaned, it will be replaced with a non-loaned page now.
  pages[0] = vmo->DebugGetPage(0);

  // The page should now have moved to the first LRU queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(0u, queue);

  // We should not be able to evict the page.
  ASSERT_LT(reclaim(vmo, pages[0], 0, VmCowPages::EvictionAction::FollowHint), 2u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Hint that the page is not needed again.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));

  // HintRange() is allowed to replace the page.
  pages[0] = vmo->DebugGetPage(0);

  // The page should now have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // We should still not be able to evict the page, the AlwaysNeed hint is sticky.
  ASSERT_LT(reclaim(vmo, pages[0], 0, VmCowPages::EvictionAction::FollowHint), 2u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Accessing the page should move it out of the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(0u, queue);

  // Verify that the page can be rotated as normal.
  pmm_page_queues()->RotateReclaimQueues();
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(1u, queue);

  // Touching the page should move it back to the first queue.
  status = vmo->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(0u, queue);

  // We should be able to evict first page when told to override the hint.
  ASSERT_GE(reclaim(vmo, pages[0], 0, VmCowPages::EvictionAction::IgnoreHint), 1u);
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemoryInRange(0, kPageSize))

  // Re-supply pages.
  supply_pager_vmo_pages(vmo.get(), 0, 2, pages);

  // Hint that second page is always needed.
  ASSERT_OK(vmo->HintRange(kPageSize, kPageSize, VmObject::EvictionHint::AlwaysNeed));
  // If the page was loaned, it will be replaced with a non-loaned page now.
  pages[1] = vmo->DebugGetPage(kPageSize);

  END_TEST;
}

bool vmo_always_need_evicts_loaned_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Depending on which loaned page we get, it may not still be loaned at the time HintRange() is
  // called, so try a few times and make sure we see non-loaned after HintRange() for all the tries.
  const uint32_t kTryCount = 30;
  for (uint32_t try_ordinal = 0; try_ordinal < kTryCount; ++try_ordinal) {
    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    // create a contiguous VMO so that we are guaranteed to have a place to borrow from
    fbl::RefPtr<VmObjectPaged> contiguous_vmo;
    ASSERT_OK(VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, kPageSize, /*alignment_log2*/ 0,
                                              &contiguous_vmo));
    ASSERT_OK(contiguous_vmo->DecommitRange(0, kPageSize));

    // we will replace the only page in vmo with a loaned page
    fbl::RefPtr<VmObjectPaged> vmo;
    vm_page_t* before_page;
    ASSERT_OK(
        make_committed_pager_vmo(1, /*trap_dirty*/ false, /*resizable*/ false, &before_page, &vmo));
    uint64_t offset = 0;
    fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
    ASSERT_OK(cow_pages->ReplacePageWithLoaned(before_page, offset));
    // The call to ReplacePageWithLoaned may loan vmo's page to a VMO that's not contiguous_vmo.
    // So, it might get called back, and the rest of the test must tolerate the vmo's page becoming
    // unloaned at any time.

    // Hint that the page is always needed.
    ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));

    // If the page was still loaned, it will be replaced with a non-loaned page now.
    vm_page_t* page = vmo->DebugGetPage(0);

    ASSERT_FALSE(page->is_loaned());
  }

  END_TEST;
}

bool vmo_eviction_hints_clone_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with two pages. We will fork a page in a clone later.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* pages[2];
  zx_status_t status =
      make_committed_pager_vmo(2, /*trap_dirty=*/false, /*resizable=*/false, pages, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Newly created pages should be in the first pager backed page queue.
  size_t queue;
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(0u, queue);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[1], &queue));
  EXPECT_EQ(0u, queue);

  // Create a clone.
  fbl::RefPtr<VmObject> clone;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, 2 * kPageSize,
                            true, &clone);
  ASSERT_EQ(ZX_OK, status);

  // Use the clone to perform a bunch of hinting operations on the first page.
  // Hint that the page is not needed.
  ASSERT_OK(clone->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));

  // The page should now have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Hint that the page is always needed.
  ASSERT_OK(clone->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));

  // If the page was loaned, it will be replaced with a non-loaned page now.
  pages[0] = vmo->DebugGetPage(0);

  // The page should now have moved to the first LRU queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(0u, queue);

  // Evicting the page should fail.
  ASSERT_LT(reclaim(vmo, pages[0], 0, VmCowPages::EvictionAction::FollowHint), 2u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Hinting should also work via a clone of a clone.
  fbl::RefPtr<VmObject> clone2;
  status = clone->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, 2 * kPageSize,
                              true, &clone2);
  ASSERT_EQ(ZX_OK, status);

  // Hint that the page is not needed.
  ASSERT_OK(clone2->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));

  // The page should now have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Hint that the page is always needed.
  ASSERT_OK(clone2->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));

  // If the page was loaned, it will be replaced with a non-loaned page now.
  pages[0] = vmo->DebugGetPage(0);

  // The page should now have moved to the first LRU queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0], &queue));
  EXPECT_EQ(0u, queue);

  // Evicting the page should fail.
  ASSERT_LT(reclaim(vmo, pages[0], 0, VmCowPages::EvictionAction::FollowHint), 2u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Re supply the second page, in case it was evicted.
  supply_pager_vmo_pages(vmo.get(), 1, 1, &pages[1]);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[1], &queue));

  // Verify that hinting still works via the parent VMO.
  // Hint that the page is not needed again.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));

  // The page should now have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Fork the page in the clone. And make sure hints no longer apply.
  uint64_t data = 0xff;
  clone->Write(&data, 0, sizeof(data));
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == clone->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, kPageSize));

  // The write will have moved the page to the first page queue, because the page is still accessed
  // in order to perform the fork. So hint using the parent again to move to the Isolate queue.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));

  // The page should now have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Hint that the page is always needed via the clone.
  ASSERT_OK(clone->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));

  // The page should still be in the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Hint that the page is always needed via the second level clone.
  ASSERT_OK(clone2->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));

  // This should move the page out of the the Isolate queue. Since we forked the page in the
  // intermediate clone *after* this clone was created, it will still refer to the original page,
  // which is the same as the page in the root.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Create another clone that sees the forked page.
  // Hinting through this clone should have no effect, since it will see the forked page.
  fbl::RefPtr<VmObject> clone3;
  status = clone->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, 2 * kPageSize,
                              true, &clone3);
  ASSERT_EQ(ZX_OK, status);

  // Move the page back to the Isolate queue first.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));

  // The page should now have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Hint through clone3.
  ASSERT_OK(clone3->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));

  // The page should still be in the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[0]));

  // Hint on the second page using clone3. This page hasn't been forked by the intermediate clone.
  // So clone3 should still be able to see the root page.
  // First verify that the page is still in queue 0.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[1], &queue));
  EXPECT_EQ(0u, queue);

  // Hint DontNeed through clone 3.
  ASSERT_OK(clone3->HintRange(kPageSize, kPageSize, VmObject::EvictionHint::DontNeed));

  // The page should have moved to the Isolate queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[1]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(pages[1]));

  END_TEST;
}

bool vmo_unloan_test() {
  BEGIN_TEST;
  // Disable the page scanner as this test would be flaky if our pages get evicted by someone else.
  AutoVmScannerDisable scanner_disable;

  bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  fbl::RefPtr<VmObjectPaged> contiguous_vmo;
  zx_status_t status =
      VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, 2 * kPageSize, 0, &contiguous_vmo);
  ASSERT_EQ(ZX_OK, status);
  status = contiguous_vmo->DecommitRange(0, 2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);

  fbl::RefPtr<VmObjectPaged> vmo;
  fbl::RefPtr<VmObjectPaged> vmo2;
  vm_page_t* page;
  vm_page_t* page2;
  status = make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page, &vmo);
  ASSERT_EQ(ZX_OK, status);
  ASSERT_OK(vmo->DebugGetCowPages()->ReplacePageWithLoaned(page, 0));
  page = vmo->DebugGetPage(0);
  ASSERT_TRUE(page->is_loaned());

  status = make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page2, &vmo2);
  ASSERT_EQ(ZX_OK, status);
  ASSERT_OK(vmo2->DebugGetCowPages()->ReplacePageWithLoaned(page2, 0));
  page2 = vmo2->DebugGetPage(0);
  ASSERT_TRUE(page2->is_loaned());

  // Shouldn't be able to evict pages from the wrong VMO.
  ASSERT_FALSE(evict_loaned_page(vmo, page2, 0));
  ASSERT_FALSE(evict_loaned_page(vmo2, page, 0));

  // Evicting a loaned page should drop the number of committed pages.
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo2->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo2, kPageSize));
  ASSERT_TRUE(evict_loaned_page(vmo2, page2, 0));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo2->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo2, 0));

  // Pinned pages should not be evictable.
  status = vmo->CommitRangePinned(0, kPageSize, false);
  EXPECT_EQ(ZX_OK, status);
  ASSERT_EQ(evict_loaned_page(vmo, page, 0), 0u);
  vmo->Unpin(0, kPageSize);

  END_TEST;
}

bool vmo_reclamation_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  constexpr size_t kNumPages = 2;
  constexpr size_t kAllocSize = kNumPages * kPageSize;

  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* pages[kNumPages];
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, pages, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Reclamation should drop the number of committed pages.
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  ASSERT_EQ(reclaim(vmo, pages[0], 0, VmCowPages::EvictionAction::FollowHint), 1u);
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_GT(vmo->ReclamationEventCount(), 0u);

  // Pinned pages should not be reclaimable.
  status = make_committed_pager_vmo(kNumPages, /*trap_dirty=*/false, /*resizable=*/false, &pages[0],
                                    &vmo);
  ASSERT_EQ(ZX_OK, status);

  status = vmo->CommitRangePinned(0, kAllocSize / 2, false);
  EXPECT_EQ(ZX_OK, status);
  ASSERT_LE(reclaim(vmo, pages[0], 0, VmCowPages::EvictionAction::FollowHint), 2u);
  vmo->Unpin(0, kAllocSize / 2);

  // Trying to reclaim from a VMO with no pages in isolate is considered an 'evict accesed' failure.
  status = make_committed_pager_vmo(kNumPages, /*trap_dirty=*/false, /*resizable=*/false, &pages[0],
                                    &vmo);
  ASSERT_EQ(ZX_OK, status);

  for (size_t i = 0; i < kNumPages; i++) {
    EXPECT_FALSE(PageQueues::IsPageReclaimable(pages[i]));
  }

  auto reclaimed = vmo->DebugGetCowPages()->ReclaimPage(
      pages[0], 0, VmCowPages::EvictionAction::FollowHint, nullptr);

  EXPECT_TRUE(reclaimed.is_error());
  EXPECT_EQ(reclaimed.error_value(), VmCowReclaimFailure::EvictAccessed);

  END_TEST;
}

// Tests memory attribution under various cloning behaviors - creation of snapshot clones and
// slices, removal of clones, committing pages in the original vmo and in the clones.
bool vmo_attribution_clones_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;
  using AttributionCounts = VmObject::AttributionCounts;

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 4 * kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);
  // Fake user id to keep the cloning code happy.
  vmo->set_user_id(0xff);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Commit the first two pages.
  status = vmo->CommitRange(0, 2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));

  // Create a clone that sees the second and third pages.
  fbl::RefPtr<VmObject> clone;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, kPageSize,
                            2 * kPageSize, true, &clone);
  ASSERT_EQ(ZX_OK, status);
  clone->set_user_id(0xfc);

  EXPECT_TRUE(vmo->GetAttributedMemory() ==
              (AttributionCounts{.uncompressed_bytes = 2ul * kPageSize,
                                 .private_uncompressed_bytes = kPageSize,
                                 .scaled_uncompressed_bytes = vm::FractionalBytes(kPageSize, 2) +
                                                              vm::FractionalBytes(kPageSize)}));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));

  EXPECT_TRUE(clone->GetAttributedMemory() ==
              (AttributionCounts{.uncompressed_bytes = kPageSize,
                                 .scaled_uncompressed_bytes = vm::FractionalBytes(kPageSize, 2)}));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, kPageSize));

  // Commit both pages in the clone.
  status = clone->CommitRange(0, 2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(clone->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, 2ul * kPageSize));

  // Commit the last page in the original vmo.
  status = vmo->CommitRange(3 * kPageSize, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(3ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 3ul * kPageSize));

  // Create a slice that sees all four pages of the original vmo.
  fbl::RefPtr<VmObject> slice;
  status = vmo->CreateChildSlice(0, 4 * kPageSize, true, &slice);
  ASSERT_EQ(ZX_OK, status);
  slice->set_user_id(0xf5);

  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(3ul * kPageSize, 0));
  EXPECT_TRUE(clone->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 3ul * kPageSize));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, 2ul * kPageSize));
  EXPECT_TRUE(slice->GetAttributedMemory() == AttributionCounts{});

  // Committing the slice's last page is a no-op (as the page is already committed).
  status = slice->CommitRange(3 * kPageSize, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(3ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 3ul * kPageSize));

  // Committing the remaining 3 pages in the slice will commit pages in the original vmo.
  status = slice->CommitRange(0, 4 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));
  EXPECT_TRUE(clone->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 4ul * kPageSize));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, 2ul * kPageSize));
  EXPECT_TRUE(slice->GetAttributedMemory() == AttributionCounts{});

  clone.reset();
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 4ul * kPageSize));
  EXPECT_TRUE(slice->GetAttributedMemory() == AttributionCounts{});

  slice.reset();
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 4ul * kPageSize));

  END_TEST;
}

// Tests that memory attribution behaves as expected under various operations performed on the vmo
// that can change its page list - committing / decommitting pages, reading / writing, zero range,
// resizing.
bool vmo_attribution_ops_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  for (uint32_t is_ppb_enabled = 0; is_ppb_enabled < 2; ++is_ppb_enabled) {
    dprintf(INFO, "is_ppb_enabled: %u\n", is_ppb_enabled);

    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(is_ppb_enabled);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status;
    status =
        VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, 4 * kPageSize, &vmo);
    ASSERT_EQ(ZX_OK, status);

    VmObject::AttributionCounts expected_attribution_counts;
    expected_attribution_counts.uncompressed_bytes = 0;
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

    status = vmo->CommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(4ul * kPageSize, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 4 * kPageSize));

    // Committing the same range again will be a no-op.
    status = vmo->CommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 4 * kPageSize));

    status = vmo->DecommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(0, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

    status = vmo->CommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(4ul * kPageSize, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 4 * kPageSize));

    status = vmo->DecommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(0, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

    fbl::AllocChecker ac;
    fbl::Vector<uint8_t> buf;
    buf.reserve(2 * kPageSize, &ac);
    ASSERT_TRUE(ac.check());

    // Read the first two pages.
    status = vmo->Read(buf.data(), 0, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    // Since these are zero pages being read, this won't commit any pages in
    // the vmo.
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

    // Write the first two pages, committing them.
    status = vmo->Write(buf.data(), 0, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(2ul * kPageSize, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2 * kPageSize));

    // Write the last two pages, committing them.
    status = vmo->Write(buf.data(), 2 * kPageSize, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(4ul * kPageSize, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 4 * kPageSize));

    status = vmo->Resize(2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(2ul * kPageSize, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2 * kPageSize));

    // Zero'ing the range will decommit pages.
    status = vmo->ZeroRange(0, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    expected_attribution_counts = make_private_attribution_counts(0, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  }

  END_TEST;
}

// Tests that memory attribution behaves as expected under various operations performed on a
// contiguous vmo that can change its page list - committing / decommitting pages, reading /
// writing, zero range, resizing.
bool vmo_attribution_ops_contiguous_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  for (uint32_t is_ppb_enabled = 0; is_ppb_enabled < 2; ++is_ppb_enabled) {
    dprintf(INFO, "is_ppb_enabled: %u\n", is_ppb_enabled);

    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(is_ppb_enabled);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status;
    status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, 4 * kPageSize,
                                             /*alignment_log2=*/0, &vmo);
    ASSERT_EQ(ZX_OK, status);

    VmObject::AttributionCounts expected_attribution_counts;
    expected_attribution_counts = make_private_attribution_counts(4ul * kPageSize, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    status = vmo->CommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    // Committing the same range again will be a no-op.
    status = vmo->CommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    status = vmo->DecommitRange(0, 4 * kPageSize);
    if (!is_ppb_enabled) {
      ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, status);
      // No change because DecommitRange() failed (as expected).
      DEBUG_ASSERT(expected_attribution_counts.uncompressed_bytes == 4ul * kPageSize);
    } else {
      ASSERT_EQ(ZX_OK, status);
      expected_attribution_counts = make_private_attribution_counts(0, 0);
    }
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    status = vmo->CommitRange(0, 4 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    if (!is_ppb_enabled) {
      // expected_attribution_counts don't change because the pages are already present.
      DEBUG_ASSERT(expected_attribution_counts.uncompressed_bytes == 4ul * kPageSize);
    } else {
      expected_attribution_counts = make_private_attribution_counts(4ul * kPageSize, 0);
    }
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    status = vmo->DecommitRange(0, 4 * kPageSize);
    if (!is_ppb_enabled) {
      ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, status);
      // and expected_attribution_counts don't change because we're zeroing not decommitting.
      DEBUG_ASSERT(expected_attribution_counts.uncompressed_bytes == 4ul * kPageSize);
    } else {
      ASSERT_EQ(ZX_OK, status);
      expected_attribution_counts = make_private_attribution_counts(0, 0);
    }
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    fbl::AllocChecker ac;
    fbl::Vector<uint8_t> buf;
    buf.reserve(2 * kPageSize, &ac);
    ASSERT_TRUE(ac.check());

    // Read the first two pages. Reading will still cause pages to get committed.
    status = vmo->Read(buf.data(), 0, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    if (!is_ppb_enabled) {
      // and expected_attribution_counts don't change because the pages are already present.
      DEBUG_ASSERT(expected_attribution_counts.uncompressed_bytes == 4ul * kPageSize);
    } else {
      expected_attribution_counts = make_private_attribution_counts(2ul * kPageSize, 0);
    }
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    // Write the last two pages, committing them.
    status = vmo->Write(buf.data(), 2 * kPageSize, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    if (!is_ppb_enabled) {
      // and expected_attribution_counts don't change because the pages are already present.
      DEBUG_ASSERT(expected_attribution_counts.uncompressed_bytes == 4ul * kPageSize);
    } else {
      expected_attribution_counts = make_private_attribution_counts(4ul * kPageSize, 0);
    }
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    // Zero'ing the range will decommit pages. In the case of contiguous VMOs, we don't decommit
    // pages (so far).
    status = vmo->ZeroRange(0, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    // Zeroing doesn't decommit pages of contiguous VMOs (nor does it commit pages).
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    status = vmo->DecommitRange(0, 2 * kPageSize);
    if (!is_ppb_enabled) {
      ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, status);
      DEBUG_ASSERT(expected_attribution_counts.uncompressed_bytes == 4ul * kPageSize);
    } else {
      ASSERT_EQ(ZX_OK, status);
      // We were able to decommit two pages.
      expected_attribution_counts = make_private_attribution_counts(2ul * kPageSize, 0);
    }
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));

    // Zero'ing a decommitted range (if is_ppb_enabled is true) should not commit any new pages.
    // Empty slots in a decommitted contiguous VMO are zero by default, as the physical page
    // provider will zero these pages on supply.
    status = vmo->ZeroRange(0, 2 * kPageSize);
    ASSERT_EQ(ZX_OK, status);
    // The attribution counts should remain unchanged.
    EXPECT_TRUE(vmo->GetAttributedMemory() == expected_attribution_counts);
    EXPECT_TRUE(
        verify_continuous_attribution_bytes(*vmo, expected_attribution_counts.total_bytes()));
  }

  END_TEST;
}

// Tests that memory attribution behaves as expected for operations specific to pager-backed
// vmo's - supplying pages, creating COW clones.
bool vmo_attribution_pager_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  constexpr size_t kNumPages = 2;
  constexpr size_t alloc_size = kNumPages * kPageSize;
  using AttributionCounts = VmObject::AttributionCounts;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      make_uncommitted_pager_vmo(kNumPages, /*trap_dirty=*/false, /*resizable=*/false, &vmo);
  ASSERT_EQ(ZX_OK, status);
  // Fake user id to keep the cloning code happy.
  vmo->set_user_id(0xff);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Create an aux VMO to transfer pages into the pager-backed vmo.
  fbl::RefPtr<VmObjectPaged> aux_vmo;
  status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, alloc_size, &aux_vmo);
  ASSERT_EQ(ZX_OK, status);

  EXPECT_TRUE(aux_vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*aux_vmo, 0));

  status = aux_vmo->CommitRange(0, alloc_size);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(aux_vmo->GetAttributedMemory() ==
              make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*aux_vmo, 2ul * kPageSize));

  VmPageSpliceList page_list;
  status = aux_vmo->TakePages(0, kPageSize, &page_list);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(aux_vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*aux_vmo, kPageSize));
  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  status = vmo->SupplyPages(0, kPageSize, &page_list, SupplyOptions::PagerSupply);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_TRUE(aux_vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*aux_vmo, kPageSize));

  aux_vmo.reset();

  // Create a COW clone that sees the first page.
  fbl::RefPtr<VmObject> clone;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, kPageSize, true,
                            &clone);
  ASSERT_EQ(ZX_OK, status);
  clone->set_user_id(0xfc);

  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_TRUE(clone->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, 0));

  status = clone->CommitRange(0, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_TRUE(clone->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, kPageSize));

  clone.reset();
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  END_TEST;
}

// Tests that memory attribution behaves as expected when zero pages are deduped, changing the no.
// of committed pages in the vmo.
bool vmo_attribution_dedup_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  using AttributionCounts = VmObject::AttributionCounts;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 2 * kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  status = vmo->CommitRange(0, 2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize + 0));

  vm_page_t* page;
  status = vmo->GetPageBlocking(0, 0, &page, nullptr);
  ASSERT_EQ(ZX_OK, status);

  // Dedupe the first page.
  auto vmop = static_cast<VmObjectPaged*>(vmo.get());
  ASSERT_TRUE(vmop->DebugGetCowPages()->DedupZeroPage(page, 0));
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  // Dedupe the second page.
  status = vmo->GetPageBlocking(kPageSize, 0, &page, nullptr);
  ASSERT_EQ(ZX_OK, status);
  ASSERT_TRUE(vmop->DebugGetCowPages()->DedupZeroPage(page, kPageSize));
  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Commit the range again.
  status = vmo->CommitRange(0, 2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));

  END_TEST;
}

// Test that compressing and uncompressing pages in a VMO correctly updates memory attribution
// counts.
bool vmo_attribution_compression_test() {
  BEGIN_TEST;

  // Need a compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  AutoVmScannerDisable scanner_disable;

  using AttributionCounts = VmObject::AttributionCounts;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 2 * kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  uint64_t reclamation_count = vmo->ReclamationEventCount();

  // Committing pages should not increment the reclamation count.
  status = vmo->CommitRange(0, 2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));
  EXPECT_EQ(reclamation_count, vmo->ReclamationEventCount());

  uint64_t val = 42;
  status = vmo->Write(&val, 0, sizeof(val));
  EXPECT_OK(status);
  EXPECT_EQ(reclamation_count, vmo->ReclamationEventCount());

  // Compress the first page.
  vm_page_t* page = nullptr;
  status = vmo->GetPageBlocking(0, 0, &page, nullptr);
  ASSERT_EQ(ZX_OK, status);
  {
    auto compressor = compression->AcquireCompressor();
    EXPECT_OK(compressor.get().Arm());
    ASSERT_EQ(
        compress_page(vmo, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get()), 1u);
  }
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, kPageSize));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));
  {
    const uint64_t new_reclamation_count = vmo->ReclamationEventCount();
    EXPECT_GT(new_reclamation_count, reclamation_count);
    reclamation_count = new_reclamation_count;
  }
  // Compress the second page.
  status = vmo->GetPageBlocking(kPageSize, 0, &page, nullptr);
  ASSERT_EQ(ZX_OK, status);
  {
    auto compressor = compression->AcquireCompressor();
    EXPECT_OK(compressor.get().Arm());
    ASSERT_EQ(compress_page(vmo, page, kPageSize, VmCowPages::EvictionAction::FollowHint,
                            &compressor.get()),
              1u);
  }
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(0, kPageSize));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  {
    const uint64_t new_reclamation_count = vmo->ReclamationEventCount();
    EXPECT_GT(new_reclamation_count, reclamation_count);
    reclamation_count = new_reclamation_count;
  }

  // Attempting to read the first page will require a decompress.
  status = vmo->GetPageBlocking(0, VMM_PF_FLAG_HW_FAULT, &page, nullptr);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_EQ(reclamation_count, vmo->ReclamationEventCount());

  // Reading the second page will just get the zero page.
  status = vmo->GetPageBlocking(kPageSize, VMM_PF_FLAG_HW_FAULT, &page, nullptr);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_EQ(reclamation_count, vmo->ReclamationEventCount());

  END_TEST;
}

// Test that a VmObjectPaged that is only referenced by its children gets removed by effectively
// merging into its parent and re-homing all the children. This should also drop any VmCowPages
// being held open.
bool vmo_parent_merge_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Set a user ID for testing.
  vmo->set_user_id(42);

  fbl::RefPtr<VmObject> child;
  status =
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, false, &child);
  ASSERT_EQ(ZX_OK, status);

  child->set_user_id(43);

  EXPECT_EQ(0u, vmo->parent_user_id());
  EXPECT_EQ(42u, vmo->user_id());
  EXPECT_EQ(43u, child->user_id());
  EXPECT_EQ(42u, child->parent_user_id());

  // Dropping the parent should re-home the child to an empty parent.
  vmo.reset();
  EXPECT_EQ(43u, child->user_id());
  EXPECT_EQ(0u, child->parent_user_id());

  child.reset();

  // Recreate a more interesting 3 level hierarchy with vmo->child->(child2,child3)

  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);
  vmo->set_user_id(42);
  status =
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, false, &child);
  ASSERT_EQ(ZX_OK, status);
  child->set_user_id(43);
  fbl::RefPtr<VmObject> child2;
  status = child->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, false,
                              &child2);
  ASSERT_EQ(ZX_OK, status);
  child2->set_user_id(44);
  fbl::RefPtr<VmObject> child3;
  status = child->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, false,
                              &child3);
  ASSERT_EQ(ZX_OK, status);
  child3->set_user_id(45);
  EXPECT_EQ(0u, vmo->parent_user_id());
  EXPECT_EQ(42u, child->parent_user_id());
  EXPECT_EQ(43u, child2->parent_user_id());
  EXPECT_EQ(43u, child3->parent_user_id());

  // Drop the intermediate child, child2+3 should get re-homed to vmo
  child.reset();
  EXPECT_EQ(42u, child2->parent_user_id());
  EXPECT_EQ(42u, child3->parent_user_id());

  END_TEST;
}

// Test that the discardable VMO's lock count is updated as expected via lock and unlock ops.
bool vmo_lock_count_test() {
  BEGIN_TEST;

  // Create a vmo to lock and unlock from multiple threads.
  fbl::RefPtr<VmObjectPaged> vmo;
  constexpr uint64_t kSize = 3 * kPageSize;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kDiscardable, kSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  constexpr int kNumThreads = 5;
  Thread* threads[kNumThreads];
  struct thread_state {
    VmObjectPaged* vmo;
    bool did_unlock;
  } state[kNumThreads];

  for (int i = 0; i < kNumThreads; i++) {
    state[i].vmo = vmo.get();
    state[i].did_unlock = false;

    threads[i] = Thread::Create(
        "worker",
        [](void* arg) -> int {
          zx_status_t status;
          auto state = static_cast<struct thread_state*>(arg);
          uint32_t rand_val = static_cast<uint32_t>(reinterpret_cast<uintptr_t>(state->vmo));

          // Randomly decide between try-lock and lock.
          if (rand_val = test_rand(rand_val); rand_val % 2) {
            if ((status = state->vmo->TryLockRange(0, kSize)) != ZX_OK) {
              return status;
            }
          } else {
            zx_vmo_lock_state_t lock_state = {};
            if ((status = state->vmo->LockRange(0, kSize, &lock_state)) != ZX_OK) {
              return status;
            }
          }

          // Randomly decide whether to unlock, or leave the vmo locked.
          if (rand_val = test_rand(rand_val); rand_val % 2) {
            if ((status = state->vmo->UnlockRange(0, kSize)) != ZX_OK) {
              return status;
            }
            state->did_unlock = true;
          }

          return 0;
        },
        &state[i], DEFAULT_PRIORITY);
  }

  for (auto& t : threads) {
    t->Resume();
  }

  for (auto& t : threads) {
    int ret;
    t->Join(&ret, ZX_TIME_INFINITE);
    EXPECT_EQ(0, ret);
  }

  uint64_t expected_lock_count = kNumThreads;
  for (auto& s : state) {
    if (s.did_unlock) {
      expected_lock_count--;
    }
  }

  EXPECT_EQ(expected_lock_count,
            vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugGetLockCount());

  END_TEST;
}

// Tests the state transitions for a discardable VMO. Verifies that a discardable VMO is discarded
// only when unlocked, and can be locked / unlocked again after the discard.
bool vmo_discardable_states_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmObjectPaged> vmo;
  constexpr uint64_t kSize = 3 * kPageSize;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kDiscardable, kSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // A newly created discardable vmo is not on any list yet.
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());

  // Lock and commit all pages.
  EXPECT_EQ(ZX_OK, vmo->TryLockRange(0, kSize));
  EXPECT_EQ(ZX_OK, vmo->CommitRange(0, kSize));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());

  // Cannot discard when locked.
  vm_page_t* page;
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  EXPECT_FALSE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  auto reclaimed = vmo->DebugGetCowPages()->ReclaimPage(
      page, 0, VmCowPages::EvictionAction::FollowHint, nullptr);
  EXPECT_TRUE(reclaimed.is_error());

  // Unlock.
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kSize));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());
  if (Pmm::Node().GetPageQueues()->ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsAnonymous(page));
  } else {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  }

  // Should be able to discard now.
  reclaimed = vmo->DebugGetCowPages()->ReclaimPage(page, 0, VmCowPages::EvictionAction::FollowHint,
                                                   nullptr);
  ASSERT_TRUE(reclaimed.is_ok());
  EXPECT_EQ(kSize / kPageSize, reclaimed.value().num_pages);
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());

  // Try lock should fail after discard.
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, vmo->TryLockRange(0, kSize));

  // Lock should succeed.
  zx_vmo_lock_state_t lock_state = {};
  EXPECT_EQ(ZX_OK, vmo->LockRange(0, kSize, &lock_state));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());

  // Verify the lock state returned.
  EXPECT_EQ(0u, lock_state.offset);
  EXPECT_EQ(kSize, lock_state.size);
  EXPECT_EQ(0u, lock_state.discarded_offset);
  EXPECT_EQ(kSize, lock_state.discarded_size);

  EXPECT_EQ(ZX_OK, vmo->CommitRange(0, kSize));
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  EXPECT_FALSE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));

  // Try lock should succeed now.
  EXPECT_OK(vmo->TryLockRange(0, kSize));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));

  // Lock count 2->1. So no change in reclaimable state.
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kSize));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));

  // Unlock.
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kSize));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  if (Pmm::Node().GetPageQueues()->ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsAnonymous(page));
  } else {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  }

  // Lock again and verify the lock state returned without a discard.
  EXPECT_EQ(ZX_OK, vmo->LockRange(0, kSize, &lock_state));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  EXPECT_FALSE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));

  EXPECT_EQ(0u, lock_state.offset);
  EXPECT_EQ(kSize, lock_state.size);
  EXPECT_EQ(0u, lock_state.discarded_offset);
  EXPECT_EQ(0u, lock_state.discarded_size);

  // Unlock and discard again.
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kSize));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  if (Pmm::Node().GetPageQueues()->ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsAnonymous(page));
  } else {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  }

  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  reclaimed = vmo->DebugGetCowPages()->ReclaimPage(page, 0, VmCowPages::EvictionAction::FollowHint,
                                                   nullptr);
  ASSERT_TRUE(reclaimed.is_ok());
  EXPECT_EQ(kSize / kPageSize, reclaimed.value().num_pages);
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsDiscarded());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsUnreclaimable());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugGetDiscardableTracker()->DebugIsReclaimable());

  END_TEST;
}

// Test that an unlocked discardable VMO can be discarded as expected.
bool vmo_discard_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Create a resizable discardable vmo.
  fbl::RefPtr<VmObjectPaged> vmo;
  constexpr uint64_t kSize = 3 * kPageSize;
  zx_status_t status = VmObjectPaged::Create(
      PMM_ALLOC_FLAG_ANY, VmObjectPaged::kDiscardable | VmObjectPaged::kResizable, kSize, &vmo);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_EQ(kSize, vmo->size());

  // Lock and commit all pages. Verify the size.
  EXPECT_EQ(ZX_OK, vmo->TryLockRange(0, kSize));
  EXPECT_EQ(ZX_OK, vmo->CommitRange(0, kSize));
  vm_page_t* page = nullptr;
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  EXPECT_EQ(kSize, vmo->size());
  EXPECT_TRUE(make_private_attribution_counts(kSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kSize));

  // Cannot discard when locked.
  auto reclaimed = vmo->DebugGetCowPages()->ReclaimPage(
      page, 0, VmCowPages::EvictionAction::FollowHint, nullptr);
  EXPECT_TRUE(reclaimed.is_error());
  EXPECT_TRUE(make_private_attribution_counts(kSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kSize));

  // Unlock.
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kSize));
  EXPECT_EQ(kSize, vmo->size());

  // Page should be in reclaimable queue.
  if (Pmm::Node().GetPageQueues()->ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsAnonymous(page));
  } else {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  }

  uint64_t reclamation_count = vmo->ReclamationEventCount();

  // Should be able to discard now.
  reclaimed = vmo->DebugGetCowPages()->ReclaimPage(page, 0, VmCowPages::EvictionAction::FollowHint,
                                                   nullptr);
  ASSERT_TRUE(reclaimed.is_ok());
  EXPECT_EQ(kSize / kPageSize, reclaimed.value().num_pages);
  page = nullptr;
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_GT(vmo->ReclamationEventCount(), reclamation_count);
  // Verify that the size is not affected.
  EXPECT_EQ(kSize, vmo->size());

  // Resize the discarded vmo.
  constexpr uint64_t kNewSize = 5 * kPageSize;
  EXPECT_EQ(ZX_OK, vmo->Resize(kNewSize));
  EXPECT_EQ(kNewSize, vmo->size());
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Lock the vmo.
  zx_vmo_lock_state_t lock_state = {};
  EXPECT_EQ(ZX_OK, vmo->LockRange(0, kNewSize, &lock_state));
  EXPECT_EQ(kNewSize, vmo->size());
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Commit and pin some pages, then unlock.
  EXPECT_EQ(ZX_OK, vmo->CommitRangePinned(0, kSize, false));
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  EXPECT_TRUE(make_private_attribution_counts(kSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kSize));
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kNewSize));

  // Page is pinned, so not in the reclaim queue, but in the wired queue.
  EXPECT_FALSE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsWired(page));

  reclamation_count = vmo->ReclamationEventCount();

  // Cannot discard a vmo with pinned pages.
  reclaimed = vmo->DebugGetCowPages()->ReclaimPage(page, 0, VmCowPages::EvictionAction::FollowHint,
                                                   nullptr);
  EXPECT_TRUE(reclaimed.is_error());
  EXPECT_EQ(kNewSize, vmo->size());
  EXPECT_TRUE(make_private_attribution_counts(kSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kSize));
  EXPECT_EQ(reclamation_count, vmo->ReclamationEventCount());

  // Unpin the pages. Should be able to discard now.
  vmo->Unpin(0, kSize);
  if (Pmm::Node().GetPageQueues()->ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsAnonymous(page));
  } else {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  }
  reclaimed = vmo->DebugGetCowPages()->ReclaimPage(page, 0, VmCowPages::EvictionAction::FollowHint,
                                                   nullptr);
  ASSERT_TRUE(reclaimed.is_ok());
  EXPECT_EQ(kSize / kPageSize, reclaimed.value().num_pages);
  page = nullptr;
  EXPECT_EQ(kNewSize, vmo->size());
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_GT(vmo->ReclamationEventCount(), reclamation_count);

  // Lock and commit pages, this time by writing them through a user mapping.
  EXPECT_EQ(ZX_OK, vmo->LockRange(0, kNewSize, &lock_state));
  ktl::unique_ptr<testing::UserMemory> user_memory = testing::UserMemory::Create(vmo);
  ASSERT_TRUE(user_memory);
  for (size_t offset = 0; offset < kNewSize; offset += kPageSize) {
    char val = 42;
    user_memory->put(val, offset);
  }
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kNewSize));
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  if (Pmm::Node().GetPageQueues()->ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsAnonymous(page));
  } else {
    EXPECT_TRUE(Pmm::Node().GetPageQueues()->DebugPageIsReclaim(page));
  }

  // Cannot discard a non-discardable vmo.
  vmo.reset();
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, kSize, &vmo);
  ASSERT_EQ(ZX_OK, status);
  ASSERT_OK(vmo->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, &page, nullptr));
  reclaimed = vmo->DebugGetCowPages()->ReclaimPage(page, 0, VmCowPages::EvictionAction::FollowHint,
                                                   nullptr);
  EXPECT_TRUE(reclaimed.is_error());
  EXPECT_EQ(0u, vmo->ReclamationEventCount());

  END_TEST;
}

// Test operations on a discarded VMO and verify expected failures.
bool vmo_discard_failure_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmObjectPaged> vmo;
  constexpr uint64_t kSize = 5 * kPageSize;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kDiscardable, kSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> buf;
  buf.reserve(kSize, &ac);
  ASSERT_TRUE(ac.check());

  fbl::Vector<uint8_t> fill;
  fill.reserve(kSize, &ac);
  ASSERT_TRUE(ac.check());
  fill_region(0x77, fill.data(), kSize);

  // Lock and commit all pages, write something and read it back to verify.
  EXPECT_EQ(ZX_OK, vmo->TryLockRange(0, kSize));
  EXPECT_EQ(ZX_OK, vmo->Write(fill.data(), 0, kSize));
  EXPECT_TRUE(make_private_attribution_counts(kSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kSize));
  EXPECT_EQ(ZX_OK, vmo->Read(buf.data(), 0, kSize));
  EXPECT_EQ(0, memcmp(fill.data(), buf.data(), kSize));

  // Create a test user aspace to map the vmo.
  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test aspace");
  ASSERT_NONNULL(aspace);

  VmAspace* old_aspace = Thread::Current::active_aspace();
  auto cleanup_aspace = fit::defer([&]() {
    vmm_set_active_aspace(old_aspace);
    ASSERT(aspace->Destroy() == ZX_OK);
  });
  vmm_set_active_aspace(aspace.get());

  // Map the vmo.
  constexpr uint64_t kMapSize = 3 * kPageSize;
  constexpr uint kArchFlags = kArchRwFlags | ARCH_MMU_FLAG_PERM_USER;
  auto mapping_result = aspace->RootVmar()->CreateVmMapping(0, kMapSize, 0, 0, vmo,
                                                            kSize - kMapSize, kArchFlags, "test");
  ASSERT(mapping_result.is_ok());

  // Fill with a known pattern through the mapping, and verify the contents.
  auto uptr = make_user_inout_ptr(reinterpret_cast<void*>(mapping_result->base));
  fill_region_user(0x88, uptr, kMapSize);
  EXPECT_TRUE(test_region_user(0x88, uptr, kMapSize));

  // Unlock and discard.
  EXPECT_EQ(ZX_OK, vmo->UnlockRange(0, kSize));
  vm_page_t* page;
  ASSERT_OK(vmo->GetPageBlocking(0, 0, &page, nullptr));
  auto reclaimed = vmo->DebugGetCowPages()->ReclaimPage(
      page, 0, VmCowPages::EvictionAction::FollowHint, nullptr);
  ASSERT_TRUE(reclaimed.is_ok());
  EXPECT_EQ(kSize / kPageSize, reclaimed.value().num_pages);
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_EQ(kSize, vmo->size());

  // Reads, writes, commits and pins should fail now.
  EXPECT_EQ(ZX_ERR_NOT_FOUND, vmo->Read(buf.data(), 0, kSize));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, vmo->Write(buf.data(), 0, kSize));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, vmo->CommitRange(0, kSize));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, vmo->CommitRangePinned(0, kSize, false));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Decommit and ZeroRange should trivially succeed.
  EXPECT_EQ(ZX_OK, vmo->DecommitRange(0, kSize));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_EQ(ZX_OK, vmo->ZeroRange(0, kSize));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Creating a mapping succeeds.
  auto mapping2_result = aspace->RootVmar()->CreateVmMapping(0, kMapSize, 0, 0, vmo,
                                                             kSize - kMapSize, kArchFlags, "test2");
  ASSERT(mapping2_result.is_ok());
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Lock the vmo again.
  zx_vmo_lock_state_t lock_state = {};
  EXPECT_EQ(ZX_OK, vmo->LockRange(0, kSize, &lock_state));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_EQ(kSize, vmo->size());

  // Should be able to read now. Verify that previous contents are lost and zeros are read.
  EXPECT_EQ(ZX_OK, vmo->Read(buf.data(), 0, kSize));
  memset(fill.data(), 0, kSize);
  EXPECT_EQ(0, memcmp(fill.data(), buf.data(), kSize));
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Write should succeed as well.
  fill_region(0x99, fill.data(), kSize);
  EXPECT_EQ(ZX_OK, vmo->Write(fill.data(), 0, kSize));
  EXPECT_TRUE(make_private_attribution_counts(kSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kSize));

  // Verify contents via the mapping.
  fill_region_user(0xaa, uptr, kMapSize);
  EXPECT_TRUE(test_region_user(0xaa, uptr, kMapSize));

  // Verify contents via the second mapping created when discarded.
  uptr = make_user_inout_ptr(reinterpret_cast<void*>(mapping2_result->base));
  EXPECT_TRUE(test_region_user(0xaa, uptr, kMapSize));

  // The unmapped pages should still be intact after the Write() above.
  EXPECT_EQ(ZX_OK, vmo->Read(buf.data(), 0, kSize - kMapSize));
  EXPECT_EQ(0, memcmp(fill.data(), buf.data(), kSize - kMapSize));

  END_TEST;
}

bool vmo_discardable_counts_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr int kNumVmos = 10;
  fbl::RefPtr<VmObjectPaged> vmos[kNumVmos];

  // Create some discardable vmos.
  zx_status_t status;
  for (int i = 0; i < kNumVmos; i++) {
    status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kDiscardable,
                                   (i + 1) * kPageSize, &vmos[i]);
    ASSERT_EQ(ZX_OK, status);
  }

  uint32_t rand_val = static_cast<uint32_t>(reinterpret_cast<uintptr_t>(vmos[0].get()));
  DiscardableVmoTracker::DiscardablePageCounts expected = {};

  // Lock all vmos. Unlock a few. And discard a few unlocked ones.
  // Compute the expected page counts as a result of these operations.
  for (int i = 0; i < kNumVmos; i++) {
    EXPECT_EQ(ZX_OK, vmos[i]->TryLockRange(0, (i + 1) * kPageSize));
    EXPECT_EQ(ZX_OK, vmos[i]->CommitRange(0, (i + 1) * kPageSize));

    if (rand_val = test_rand(rand_val); rand_val % 2) {
      EXPECT_EQ(ZX_OK, vmos[i]->UnlockRange(0, (i + 1) * kPageSize));

      if (rand_val = test_rand(rand_val); rand_val % 2) {
        // Discarded pages won't show up under locked or unlocked counts.
        vm_page_t* page;
        ASSERT_OK(vmos[i]->GetPageBlocking(0, 0, &page, nullptr));
        auto reclaimed = vmos[i]->DebugGetCowPages()->ReclaimPage(
            page, 0, VmCowPages::EvictionAction::FollowHint, nullptr);
        ASSERT_TRUE(reclaimed.is_ok());
        EXPECT_EQ(static_cast<uint64_t>(i + 1), reclaimed.value().num_pages);
      } else {
        // Unlocked but not discarded.
        expected.unlocked += (i + 1);
      }
    } else {
      // Locked.
      expected.locked += (i + 1);
    }
  }

  DiscardableVmoTracker::DiscardablePageCounts counts =
      DiscardableVmoTracker::DebugDiscardablePageCounts();
  // There might be other discardable vmos in the rest of the system, so the actual page counts
  // might be higher than the expected counts.
  EXPECT_LE(expected.locked, counts.locked);
  EXPECT_LE(expected.unlocked, counts.unlocked);

  END_TEST;
}

// using LookupCursor with different kinds of faults reads / writes should correctly
// decompress or return an error.
bool vmo_lookup_compressed_pages_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;
  // Need a working compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  // Create a VMO and commit a real non-zero page
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize, &vmo);
  ASSERT_OK(status);
  uint64_t data = 42;
  EXPECT_OK(vmo->Write(&data, 0, sizeof(data)));
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  // Compress the page.
  vm_page_t* page;
  status = vmo->GetPageBlocking(0, 0, &page, nullptr);
  ASSERT_OK(status);
  {
    auto compressor = compression->AcquireCompressor();
    EXPECT_OK(compressor.get().Arm());
    EXPECT_EQ(
        compress_page(vmo, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get()), 1u);
  }
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  // Looking up the page for read or write, without it being a fault, should fail and not cause the
  // page to get decompressed.
  EXPECT_NE(ZX_OK, vmo->GetPageBlocking(0, 0, nullptr, nullptr));
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  EXPECT_NE(ZX_OK, vmo->GetPageBlocking(0, VMM_PF_FLAG_WRITE, nullptr, nullptr));
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  // Read or write faults should decompress.
  ASSERT_OK(vmo->GetPageBlocking(0, VMM_PF_FLAG_HW_FAULT, &page, nullptr));
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));
  status = vmo->GetPageBlocking(0, 0, &page, nullptr);
  ASSERT_OK(status);
  {
    auto compressor = compression->AcquireCompressor();
    EXPECT_OK(compressor.get().Arm());
    EXPECT_EQ(
        compress_page(vmo, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get()), 1u);
  }
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  EXPECT_OK(vmo->GetPageBlocking(0, VMM_PF_FLAG_WRITE | VMM_PF_FLAG_SW_FAULT, &page, nullptr));
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  END_TEST;
}

bool vmo_write_does_not_commit_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Create a vmo and commit a page to it.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
  ASSERT_OK(status);

  uint64_t val = 42;
  EXPECT_OK(vmo->Write(&val, 0, sizeof(val)));

  // Create a CoW clone of the vmo.
  fbl::RefPtr<VmObject> clone;
  status =
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, false, &clone);

  // Querying the page for read in the clone should return it.
  EXPECT_OK(clone->GetPageBlocking(0, 0, nullptr, nullptr));

  // Querying for write, without any fault flags, should not work as the page is not committed in
  // the clone.
  EXPECT_EQ(ZX_ERR_NOT_FOUND, clone->GetPageBlocking(0, VMM_PF_FLAG_WRITE, nullptr, nullptr));

  // Adding a fault flag should cause the lookup to succeed.
  EXPECT_OK(clone->GetPageBlocking(0, VMM_PF_FLAG_WRITE | VMM_PF_FLAG_SW_FAULT, nullptr, nullptr));

  END_TEST;
}

bool vmo_dirty_pages_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with a single page.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  ASSERT_OK(make_committed_pager_vmo(1, /*trap_dirty=*/true, /*resizable=*/false, &page, &vmo));

  // Newly created page should be in the first pager backed page queue.
  size_t queue;
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Rotate the queues and check the page moves.
  pmm_page_queues()->RotateReclaimQueues();
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(1u, queue);

  // Accessing the page should move it back to the first queue.
  EXPECT_OK(vmo->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Now simulate a write to the page. This should move the page to the dirty queue.
  ASSERT_OK(vmo->DirtyPages(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));
  EXPECT_GT(pmm_page_queues()->QueueCounts().pager_backed_dirty, 0u);

  // Should not be able to evict a dirty page.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Accessing the page again should not move the page out of the dirty queue.
  EXPECT_OK(vmo->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  END_TEST;
}

bool vmo_dirty_pages_writeback_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with a single page.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  ASSERT_OK(make_committed_pager_vmo(1, /*trap_dirty=*/true, /*resizable=*/false, &page, &vmo));

  // Newly created page should be in the first pager backed page queue.
  size_t queue;
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Now simulate a write to the page. This should move the page to the dirty queue.
  ASSERT_OK(vmo->DirtyPages(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Should not be able to evict a dirty page.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Begin writeback on the page. This should still keep the page in the dirty queue.
  ASSERT_OK(vmo->WritebackBegin(0, kPageSize, false));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Should not be able to evict a dirty page.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Accessing the page should not move the page out of the dirty queue either.
  ASSERT_OK(vmo->GetPageBlocking(0, VMM_PF_FLAG_SW_FAULT, nullptr, nullptr));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Should not be able to evict a dirty page.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // End writeback on the page. This should finally move the page out of the dirty queue.
  ASSERT_OK(vmo->WritebackEnd(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // We should be able to rotate the page as usual.
  pmm_page_queues()->RotateReclaimQueues();
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(1u, queue);

  // Another write moves the page back to the Dirty queue.
  ASSERT_OK(vmo->DirtyPages(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Clean the page again, and try to evict it.
  ASSERT_OK(vmo->WritebackBegin(0, kPageSize, false));
  ASSERT_OK(vmo->WritebackEnd(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // We should now be able to evict the page.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 1u);
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemoryInRange(0, kPageSize))

  END_TEST;
}

bool vmo_dirty_pages_with_hints_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with a single page.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  ASSERT_OK(make_committed_pager_vmo(1, /*trap_dirty=*/true, /*resizable=*/false, &page, &vmo));

  // Newly created page should be in the first pager backed page queue.
  size_t queue;
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Now simulate a write to the page. This should move the page to the dirty queue.
  ASSERT_OK(vmo->DirtyPages(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Hint DontNeed on the page. It should remain in the dirty queue.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(page));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Should not be able to evict a dirty page.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Hint AlwaysNeed on the page. It should remain in the dirty queue.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::AlwaysNeed));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Clean the page.
  ASSERT_OK(vmo->WritebackBegin(0, kPageSize, false));
  ASSERT_OK(vmo->WritebackEnd(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Eviction should fail still because we hinted AlwaysNeed previously.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Eviction should succeed if we ignore the hint.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::IgnoreHint), 1u);
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemoryInRange(0, kPageSize))

  // Reset the vmo and retry some of the same actions as before, this time dirtying
  // the page *after* hinting.
  vmo.reset();

  ASSERT_OK(make_committed_pager_vmo(1, /*trap_dirty=*/true, /*resizable=*/false, &page, &vmo));

  // Newly created page should be in the first pager backed page queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page, &queue));
  EXPECT_EQ(0u, queue);

  // Hint DontNeed on the page. This should move the page to the Isolate queue.
  ASSERT_OK(vmo->HintRange(0, kPageSize, VmObject::EvictionHint::DontNeed));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaimIsolate(page));

  // Write to the page now. This should move it to the dirty queue.
  ASSERT_OK(vmo->DirtyPages(0, kPageSize));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaimIsolate(page));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Should not be able to evict a dirty page.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  END_TEST;
}

// Tests that pinning pager-backed pages retains backlink information.
bool vmo_pinning_backlink_test() {
  BEGIN_TEST;
  // Disable the page scanner as this test would be flaky if our pages get evicted by someone else.
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with two pages, so we can verify a non-zero offset value.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* pages[2];
  zx_status_t status =
      make_committed_pager_vmo(2, /*trap_dirty=*/false, /*resizable=*/false, pages, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Pages should be in the pager queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[1]));

  // Verify backlink information.
  auto cow = vmo->DebugGetCowPages().get();
  EXPECT_EQ(cow, pages[0]->object.get_object());
  EXPECT_EQ(0u, pages[0]->object.get_page_offset());
  EXPECT_EQ(cow, pages[1]->object.get_object());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize), pages[1]->object.get_page_offset());

  // Pin the pages.
  status = vmo->CommitRangePinned(0, 2 * kPageSize, false);
  ASSERT_EQ(ZX_OK, status);

  // Pages might get swapped out on pinning if they were loaned. Look them up again.
  pages[0] = vmo->DebugGetPage(0);
  pages[1] = vmo->DebugGetPage(kPageSize);

  // Pages should be in the wired queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsWired(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsWired(pages[1]));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(pages[1]));

  // Moving to the wired queue should retain backlink information.
  EXPECT_EQ(cow, pages[0]->object.get_object());
  EXPECT_EQ(0u, pages[0]->object.get_page_offset());
  EXPECT_EQ(cow, pages[1]->object.get_object());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize), pages[1]->object.get_page_offset());

  // Unpin the pages.
  vmo->Unpin(0, 2 * kPageSize);

  // Pages should be back in the pager queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsWired(pages[0]));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsWired(pages[1]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[0]));
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(pages[1]));

  // Verify backlink information again.
  EXPECT_EQ(cow, pages[0]->object.get_object());
  EXPECT_EQ(0u, pages[0]->object.get_page_offset());
  EXPECT_EQ(cow, pages[1]->object.get_object());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize), pages[1]->object.get_page_offset());

  END_TEST;
}

// Tests updating dirty state of pages while they are pinned.
bool vmo_pinning_dirty_state_test() {
  BEGIN_TEST;
  // Disable the page scanner as this test would be flaky if our pages get evicted by someone else.
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with a single page.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/true, /*resizable=*/false, &page, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Page should be in the pager queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page));

  // Pin the page.
  status = vmo->CommitRangePinned(0, kPageSize, false);
  ASSERT_EQ(ZX_OK, status);

  // Pages might get swapped out on pinning if they were loaned. Look up again.
  page = vmo->DebugGetPage(0);

  // Page should be in the wired queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsWired(page));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));

  // Dirty the page while pinned. So this tests a transition to Dirty with pin count > 0. This
  // should retain the page in the wired queue.
  status = vmo->DirtyPages(0, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsWired(page));

  // Unpin the page.
  vmo->Unpin(0, kPageSize);

  // Page should be back in the pager dirty queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Start writeback on the page so that its state changes to AwaitingClean. It should still be in
  // the dirty queue.
  status = vmo->WritebackBegin(0, kPageSize, false);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Pin for read, so that the dirty state is not changed. But since it is pinned, it should move to
  // the wired queue.
  status = vmo->CommitRangePinned(0, kPageSize, false);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsWired(page));

  // Now end the writeback so that the page is cleaned. So this tests a transition to Clean with pin
  // count > 0.
  status = vmo->WritebackEnd(0, kPageSize);
  ASSERT_EQ(ZX_OK, status);

  // Page should still be in the wired queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsWired(page));

  // Unpin the page.
  vmo->Unpin(0, kPageSize);

  // Pages should be back in the pager reclaim queue.
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsWired(page));

  // The only remaining transition is to AwaitingClean with pin count > 0. This cannot happen
  // because we can only move to AwaitingClean from Dirty, but if a page is Dirty with pin count >
  // 0, it will never leave the Dirty state.

  END_TEST;
}

// Tests updating dirty state of a high priority VMO.
bool vmo_high_priority_dirty_state_test() {
  BEGIN_TEST;
  // Disable the page scanner as this test would be flaky if our pages get evicted by someone else.
  AutoVmScannerDisable scanner_disable;

  // Create a pager-backed VMO with a single page.
  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/true, /*resizable=*/false, &page, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Page should be in the pager queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page));

  auto change_priority = [&vmo](int64_t delta) {
    PriorityChanger pc = vmo->MakePriorityChanger(delta);
    if (delta > 0) {
      pc.PrepareMayNotAlreadyBeHighPriority();
    }
    Guard<CriticalMutex> guard{AliasedLock, vmo->lock(), pc.lock()};
    pc.ChangeHighPriorityCountLocked();
  };

  // Mark the VMO as high priority. This will move the page to the high priority queue.
  change_priority(1);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsHighPriority(page));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsReclaim(page));

  // Dirty the page. The page moves to the dirty queue.
  status = vmo->DirtyPages(0, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // Begin writeback. Verify that the page remains in the dirty queue.
  status = vmo->WritebackBegin(0, kPageSize, false);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // End the writeback. Since the VMO is still high priority, the page will go to the high priority
  // queue.
  status = vmo->WritebackEnd(0, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsHighPriority(page));

  // Remove high priority. The page should come back to the reclaim queue.
  change_priority(-1);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page));
  EXPECT_FALSE(pmm_page_queues()->DebugPageIsHighPriority(page));

  END_TEST;
}

bool vmo_supply_compressed_pages_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;
  // Need a working compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  fbl::RefPtr<VmObjectPaged> vmop;
  ASSERT_OK(make_uncommitted_pager_vmo(1, false, false, &vmop));

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo));

  // Write non-zero data to the VMO so we can compress it.
  uint64_t data = 42;
  EXPECT_OK(vmo->Write(&data, 0, sizeof(data)));

  vm_page_t* page;
  zx_status_t status = vmo->GetPageBlocking(0, 0, &page, nullptr);
  ASSERT_OK(status);

  {
    auto compressor = compression->AcquireCompressor();
    EXPECT_OK(compressor.get().Arm());
    EXPECT_EQ(
        compress_page(vmo, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get()), 1u);
  }
  EXPECT_TRUE(make_private_attribution_counts(0, kPageSize) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  // Taking the pages should work.
  VmPageSpliceList pl;
  EXPECT_OK(vmo->TakePages(0, kPageSize, &pl));
  EXPECT_TRUE((VmObject::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // After being supplied the pager backed VMO should not have compressed pages.
  EXPECT_OK(vmop->SupplyPages(0, kPageSize, &pl, SupplyOptions::PagerSupply));
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmop->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmop, kPageSize));

  END_TEST;
}

bool is_page_zero(vm_page_t* page) {
  auto* base = reinterpret_cast<uint64_t*>(paddr_to_physmap(page->paddr()));
  for (size_t i = 0; i < kPageSize / sizeof(uint64_t); i++) {
    if (base[i] != 0)
      return false;
  }
  return true;
}

// Tests that ZeroRange does not remove pinned pages. Regression test for
// https://fxbug.dev/42052452.
bool vmo_zero_pinned_test() {
  BEGIN_TEST;

  // Create a non pager-backed VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Pin the page for write.
  status = vmo->CommitRangePinned(0, kPageSize, true);
  ASSERT_EQ(ZX_OK, status);

  // Write non-zero content to the page.
  vm_page_t* page = vmo->DebugGetPage(0);
  *reinterpret_cast<uint8_t*>(paddr_to_physmap(page->paddr())) = 0xff;

  // Zero the page and check that it is not removed.
  status = vmo->ZeroRange(0, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_EQ(page, vmo->DebugGetPage(0));

  // The page should be zero.
  EXPECT_TRUE(is_page_zero(page));

  vmo->Unpin(0, kPageSize);

  // Create a pager-backed VMO.
  fbl::RefPtr<VmObjectPaged> pager_vmo;
  vm_page_t* old_page;
  status =
      make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/true, &old_page, &pager_vmo);
  ASSERT_EQ(ZX_OK, status);

  // Pin the page for write.
  status = pager_vmo->CommitRangePinned(0, kPageSize, true);
  ASSERT_EQ(ZX_OK, status);

  // Write non-zero content to the page. Lookup the page again, as pinning might have switched out
  // the page if it was originally loaned.
  old_page = pager_vmo->DebugGetPage(0);
  *reinterpret_cast<uint8_t*>(paddr_to_physmap(old_page->paddr())) = 0xff;

  // Zero the page and check that it is not removed.
  status = pager_vmo->ZeroRange(0, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_EQ(old_page, pager_vmo->DebugGetPage(0));

  // The page should be zero.
  EXPECT_TRUE(is_page_zero(old_page));

  // Resize the VMO up, and pin a page in the newly extended range.
  status = pager_vmo->Resize(2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  status = pager_vmo->CommitRangePinned(kPageSize, kPageSize, true);
  ASSERT_EQ(ZX_OK, status);

  // Write non-zero content to the page.
  vm_page_t* new_page = pager_vmo->DebugGetPage(kPageSize);
  *reinterpret_cast<uint8_t*>(paddr_to_physmap(new_page->paddr())) = 0xff;

  // Zero the new page, and ensure that it is not removed.
  status = pager_vmo->ZeroRange(kPageSize, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_EQ(new_page, pager_vmo->DebugGetPage(kPageSize));

  // The page should be zero.
  EXPECT_TRUE(is_page_zero(new_page));

  pager_vmo->Unpin(0, 2 * kPageSize);

  END_TEST;
}

bool vmo_pinned_wrapper_test() {
  BEGIN_TEST;

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
    ASSERT_EQ(ZX_OK, status);

    PinnedVmObject pinned;
    status = PinnedVmObject::Create(vmo, 0, kPageSize, true, &pinned);
    EXPECT_OK(status);
    status = PinnedVmObject::Create(vmo, 0, kPageSize, true, &pinned);
    EXPECT_OK(status);
  }

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
    ASSERT_EQ(ZX_OK, status);

    PinnedVmObject pinned;
    status = PinnedVmObject::Create(vmo, 0, kPageSize, true, &pinned);
    EXPECT_OK(status);

    PinnedVmObject empty;
    pinned = ktl::move(empty);
  }

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
    ASSERT_EQ(ZX_OK, status);

    PinnedVmObject pinned;
    status = PinnedVmObject::Create(vmo, 0, kPageSize, true, &pinned);
    EXPECT_OK(status);

    PinnedVmObject empty;
    empty = ktl::move(pinned);
  }

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
    ASSERT_EQ(ZX_OK, status);

    PinnedVmObject pinned1;
    status = PinnedVmObject::Create(vmo, 0, kPageSize, true, &pinned1);
    EXPECT_OK(status);

    PinnedVmObject pinned2;
    status = PinnedVmObject::Create(vmo, 0, kPageSize, true, &pinned2);
    EXPECT_OK(status);

    pinned1 = ktl::move(pinned2);
  }

  END_TEST;
}

// Tests that dirty pages cannot be deduped.
bool vmo_dedup_dirty_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Our page should now be in a pager backed page queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page));

  // The page is clean. We should be able to dedup the page.
  EXPECT_TRUE(vmo->DebugGetCowPages()->DedupZeroPage(page, 0));

  // No committed pages remaining.
  EXPECT_TRUE((vm::AttributionCounts{}) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Write to the page making it dirty.
  uint8_t data = 0xff;
  status = vmo->Write(&data, 0, sizeof(data));
  ASSERT_EQ(ZX_OK, status);

  // The page should now be dirty.
  page = vmo->DebugGetPage(0);
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsPagerBackedDirty(page));

  // We should not be able to dedup the page.
  EXPECT_FALSE(vmo->DebugGetCowPages()->DedupZeroPage(page, 0));
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, kPageSize));

  END_TEST;
}

// Test that attempting to reclaim pages from a high priority VMO will not work.
bool vmo_high_priority_reclaim_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t* page;
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page, &vmo);
  ASSERT_EQ(ZX_OK, status);

  auto change_priority = [&vmo](int64_t delta) {
    PriorityChanger pc = vmo->MakePriorityChanger(delta);
    if (delta > 0) {
      pc.PrepareMayNotAlreadyBeHighPriority();
    }
    Guard<CriticalMutex> guard{AliasedLock, vmo->lock(), pc.lock()};
    pc.ChangeHighPriorityCountLocked();
  };

  // Our page should be in a pager backed page queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page));

  // Indicate our VMO is high priority.
  change_priority(1);

  // Our page should now be in a high priority page queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsHighPriority(page));
  EXPECT_GT(pmm_page_queues()->QueueCounts().high_priority, 0u);

  // Attempting to evict should fail.
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) ==
              vmo->GetAttributedMemoryInRange(0, kPageSize));

  // Page should still be in the queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsHighPriority(page));

  // Attempting to reclaim should fail.
  ASSERT_EQ(reclaim(vmo, page, 0, VmCowPages::EvictionAction::FollowHint), 0u);

  // Switch to a regular anonymous VMO.
  change_priority(-1);

  // Page should be back in the regular pager backed page queue.
  EXPECT_TRUE(pmm_page_queues()->DebugPageIsReclaim(page));

  vmo.reset();
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);
  change_priority(1);

  // Commit a single page.
  EXPECT_OK(vmo->CommitRange(0, kPageSize));
  page = vmo->DebugGetPage(0);

  // Deduping as zero should fail.
  EXPECT_FALSE(vmo->DebugGetCowPages()->DedupZeroPage(page, 0));
  EXPECT_EQ(page, vmo->DebugGetPage(0));

  // If we have a compressor, then compressing should also fail.
  VmCompression* compression = Pmm::Node().GetPageCompression();
  if (compression) {
    auto compressor = compression->AcquireCompressor();
    EXPECT_OK(compressor.get().Arm());
    EXPECT_EQ(
        compress_page(vmo, page, 0, VmCowPages::EvictionAction::IgnoreHint, &compressor.get()), 0u);
    EXPECT_EQ(page, vmo->DebugGetPage(0));
  }

  change_priority(-1);

  END_TEST;
}

// Tests that snapshot modified behaves as expected
bool vmo_snapshot_modified_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Create 3 page, pager-backed VMO.
  constexpr uint64_t kNumPages = 3;
  vm_page_t* pages[kNumPages];
  auto alloc_size = kNumPages * kPageSize;
  fbl::RefPtr<VmObjectPaged> vmo;

  zx_status_t status = make_committed_pager_vmo(kNumPages, false, false, pages, &vmo);
  ASSERT_EQ(ZX_OK, status);
  vmo->set_user_id(42);

  // Snapshot-modified all 3 pages of root.
  fbl::RefPtr<VmObject> clone;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, alloc_size,
                            false, &clone);
  ASSERT_EQ(ZX_OK, status, "vmobject full clone\n");
  ASSERT_NONNULL(clone, "vmobject full clone\n");
  clone->set_user_id(43);

  // Hang another snapshot-modified clone off root that only sees the first page.
  fbl::RefPtr<VmObject> clone2;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kPageSize, false,
                            &clone2);
  ASSERT_EQ(ZX_OK, status, "vmobject partial clone\n");
  ASSERT_NONNULL(clone2, "vmobject partial clone\n");
  clone2->set_user_id(44);

  // Ensures all pages are attributed to the root VMO, and not the clones, as the root VMO is not a
  // hidden node.
  EXPECT_TRUE(make_private_attribution_counts(alloc_size, 0) == vmo->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, alloc_size));
  EXPECT_TRUE((vm::AttributionCounts{}) == clone->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, 0));
  EXPECT_TRUE((vm::AttributionCounts{}) == clone2->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone2, 0));

  // COW page into clone & check that it is attributed.
  uint8_t data = 0xff;
  status = clone->Write(&data, 0, sizeof(data));
  ASSERT_EQ(ZX_OK, status);

  EXPECT_TRUE(make_private_attribution_counts(kPageSize, 0) == clone->GetAttributedMemory());
  EXPECT_TRUE(verify_continuous_attribution_bytes(*clone, kPageSize));

  // Try to COW a page into clone2 that it doesn't see.
  status = clone2->Write(&data, kPageSize, sizeof(data));
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, status);

  // Call snapshot-modified again on the full clone, which will create a hidden parent.
  fbl::RefPtr<VmObject> snapshot;
  status = clone->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0,
                              kPageSize * kNumPages, false, &snapshot);
  ASSERT_EQ(ZX_OK, status, "vmobject snapshot-modified\n");
  ASSERT_NONNULL(snapshot, "vmobject snapshot-modified clone\n");

  // Pages in hidden parent will be attributed to both children.
  EXPECT_TRUE((vm::AttributionCounts{.uncompressed_bytes = kPageSize,
                                     .scaled_uncompressed_bytes = vm::FractionalBytes(
                                         kPageSize, 2)}) == clone->GetAttributedMemory());
  EXPECT_TRUE((vm::AttributionCounts{.uncompressed_bytes = kPageSize,
                                     .scaled_uncompressed_bytes = vm::FractionalBytes(
                                         kPageSize, 2)}) == snapshot->GetAttributedMemory());

  // Calling CreateClone directly with SnapshotAtLeastOnWrite should upgrade to snapshot-modified.
  fbl::RefPtr<VmObject> atleastonwrite;
  status = clone->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, alloc_size,
                              false, &atleastonwrite);
  ASSERT_EQ(ZX_OK, status, "vmobject snapshot-at-least-on-write clone.\n");
  ASSERT_NONNULL(atleastonwrite, "vmobject snapshot-at-least-on-write clone\n");

  // Create a slice of the first two pages of the root VMO.
  auto kSliceSize = 2 * kPageSize;
  fbl::RefPtr<VmObject> slice;
  ASSERT_OK(vmo->CreateChildSlice(0, kSliceSize, false, &slice));
  ASSERT_NONNULL(slice, "slice root vmo");
  slice->set_user_id(45);

  // The oot VMO should have 3 children at this point.
  ASSERT_EQ(vmo->num_children(), (uint32_t)3);

  // Snapshot-modified of root-slice should work.
  fbl::RefPtr<VmObject> slicesnapshot;
  status = slice->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kSliceSize,
                              false, &slicesnapshot);
  ASSERT_EQ(ZX_OK, status, "snapshot-modified root-slice\n");
  ASSERT_NONNULL(slicesnapshot, "snapshot modified root-slice\n");
  slicesnapshot->set_user_id(46);

  // At the VMO level, the slice should see the snapshot as a child.
  ASSERT_EQ(vmo->num_children(), (uint32_t)3);
  ASSERT_EQ(slice->num_children(), (uint32_t)1);

  // The cow pages, however, should be hung off the root VMO.
  auto slicesnapshot_p = static_cast<VmObjectPaged*>(slicesnapshot.get());
  auto vmo_cow_pages = vmo->DebugGetCowPages();
  auto slicesnapshot_cow_pages = slicesnapshot_p->DebugGetCowPages();

  ASSERT_EQ(slicesnapshot_cow_pages->DebugGetParent().get(), vmo_cow_pages.get());

  // Create a slice of the clone of the root-slice.
  fbl::RefPtr<VmObject> slicesnapshot_slice;
  status = slicesnapshot->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0,
                                      kSliceSize, false, &slicesnapshot_slice);
  ASSERT_EQ(ZX_OK, status, "slice snapshot-modified-root-slice\n");
  ASSERT_NONNULL(slicesnapshot_slice, "slice snapshot-modified-root-slice\n");
  slicesnapshot_slice->set_user_id(47);

  // Check that snapshot-modified will work again on the snapshot-modified clone of the slice.
  fbl::RefPtr<VmObject> slicesnapshot2;
  status = slicesnapshot->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0,
                                      kSliceSize, false, &slicesnapshot2);
  ASSERT_EQ(ZX_OK, status, "snapshot-modified root-slice-snapshot\n");
  ASSERT_NONNULL(slicesnapshot2, "snapshot-modified root-slice-snapshot\n");
  slicesnapshot2->set_user_id(48);

  // Create a slice of a clone
  fbl::RefPtr<VmObject> cloneslice;
  ASSERT_OK(clone->CreateChildSlice(0, kSliceSize, false, &cloneslice));
  ASSERT_NONNULL(slice, "slice root vmo");

  // Snapshot-modified should not be allowed on a slice of a clone.
  fbl::RefPtr<VmObject> cloneslicesnapshot;
  status = cloneslice->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0,
                                   kSliceSize, false, &cloneslicesnapshot);
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, status, "snapshot-modified clone-slice\n");
  ASSERT_NULL(cloneslicesnapshot, "snapshot-modified clone-slice\n");

  // Tests that SnapshotModified will be upgraded to Snapshot when used on an anonymous VMO.
  fbl::RefPtr<VmObjectPaged> anon_vmo;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &anon_vmo);
  ASSERT_EQ(ZX_OK, status);
  anon_vmo->set_user_id(0x49);

  fbl::RefPtr<VmObject> anon_clone;
  status = anon_vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kPageSize,
                                 true, &anon_clone);
  ASSERT_OK(status);
  anon_clone->set_user_id(0x50);

  // Check that a hidden, common cow pages was made.
  auto anon_clone_p = static_cast<VmObjectPaged*>(anon_clone.get());
  auto anon_vmo_cow_pages = anon_vmo->DebugGetCowPages();
  auto anon_clone_cow_pages = anon_clone_p->DebugGetCowPages();

  ASSERT_EQ(anon_clone_cow_pages->DebugGetParent().get(),
            anon_vmo_cow_pages->DebugGetParent().get());

  // Snapshot-modified should also be upgraded when used on a SNAPSHOT clone.
  fbl::RefPtr<VmObject> anon_snapshot;
  status = anon_clone->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kPageSize,
                                   true, &anon_snapshot);
  ASSERT_OK(status);
  anon_snapshot->set_user_id(0x51);

  // Snapshot-modified should not be allowed on a unidirectional chain of length > 2
  fbl::RefPtr<VmObject> chain1;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, kPageSize, true,
                            &chain1);
  ASSERT_OK(status);
  chain1->set_user_id(0x52);
  uint64_t data1 = 42;
  EXPECT_OK(chain1->Write(&data1, 0, sizeof(data)));

  fbl::RefPtr<VmObject> chain2;
  status = chain1->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, kPageSize,
                               true, &chain2);
  ASSERT_OK(status);
  chain2->set_user_id(0x51);
  uint64_t data2 = 43;
  EXPECT_OK(chain2->Write(&data2, 0, sizeof(data)));

  fbl::RefPtr<VmObject> chain_snap;
  status = chain2->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kPageSize,
                               true, &chain_snap);
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, status, "snapshot-modified unidirectional chain\n");

  END_TEST;
}

// Regression test for https://fxbug.dev/42080926. Concurrent pinning of different ranges in a
// contiguous VMO that has its pages loaned.
bool vmo_pin_race_loaned_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  const uint32_t kTryCount = 5000;
  for (uint32_t try_ordinal = 0; try_ordinal < kTryCount; ++try_ordinal) {
    bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
    auto cleanup = fit::defer([loaning_was_enabled] {
      PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
    });

    const int kNumLoaned = 10;
    fbl::RefPtr<VmObjectPaged> contiguous_vmo;
    zx_status_t status =
        VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, (kNumLoaned + 1) * kPageSize,
                                        /*alignment_log2=*/0, &contiguous_vmo);
    ASSERT_EQ(ZX_OK, status);
    vm_page_t* pages[kNumLoaned];
    for (int i = 0; i < kNumLoaned; i++) {
      pages[i] = contiguous_vmo->DebugGetPage((i + 1) * kPageSize);
    }
    status = contiguous_vmo->DecommitRange(kPageSize, kNumLoaned * kPageSize);
    ASSERT_TRUE(status == ZX_OK);

    uint32_t iteration_count = 0;
    const uint32_t kMaxIterations = 1000;
    int loaned = 0;
    do {
      // Create a pager-backed VMO with a single page.
      fbl::RefPtr<VmObjectPaged> vmo;
      vm_page_t* page;
      status = make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page, &vmo);
      ASSERT_EQ(ZX_OK, status);

      // make_committed_pager_vmo is not enough to ensure vmo's only page is loaned.
      // We must explicitly call ReplacePageWithLoaned.
      fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
      uint64_t offset = 0;
      ASSERT_OK(cow_pages->ReplacePageWithLoaned(page, offset));

      // vmo's page should be a new page since we replaced the old one with
      // a loaned page.
      page = vmo->DebugGetPage(0);

      ++iteration_count;
      for (int i = 0; i < kNumLoaned; i++) {
        if (page == pages[i]) {
          ASSERT_TRUE(page->is_loaned());
          loaned++;
        }
      }
    } while (loaned < kNumLoaned && iteration_count < kMaxIterations);

    // If we hit this iteration count, something almost certainly went wrong...
    ASSERT_TRUE(iteration_count < kMaxIterations);
    ASSERT_EQ(kNumLoaned, loaned);

    Thread* threads[kNumLoaned];
    struct thread_state {
      VmObjectPaged* vmo;
      int index;
    } states[kNumLoaned];

    for (int i = 0; i < kNumLoaned; i++) {
      states[i].vmo = contiguous_vmo.get();
      states[i].index = i;
      threads[i] = Thread::Create(
          "worker",
          [](void* arg) -> int {
            auto state = static_cast<struct thread_state*>(arg);

            zx_status_t status;
            if (state->index == 0) {
              status = state->vmo->CommitRangePinned(0, 2 * kPageSize, false);
            } else {
              status =
                  state->vmo->CommitRangePinned((state->index + 1) * kPageSize, kPageSize, false);
            }
            if (status != ZX_OK) {
              return -1;
            }
            return 0;
          },
          &states[i], DEFAULT_PRIORITY);
    }

    for (int i = 0; i < kNumLoaned; i++) {
      threads[i]->Resume();
    }

    for (int i = 0; i < kNumLoaned; i++) {
      int ret;
      threads[i]->Join(&ret, ZX_TIME_INFINITE);
      EXPECT_EQ(0, ret);
    }

    for (int i = 0; i < kNumLoaned; i++) {
      EXPECT_EQ(pages[i], contiguous_vmo->DebugGetPage((i + 1) * kPageSize));
    }
    contiguous_vmo->Unpin(0, (kNumLoaned + 1) * kPageSize);
  }

  END_TEST;
}

bool vmo_prefetch_compressed_pages_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Need a working compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  // Create a VMO and commit some pages to it, ensuring they have non-zero content.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize * 2, &vmo);
  ASSERT_OK(status);
  uint64_t data = 42;
  EXPECT_OK(vmo->Write(&data, 0, sizeof(data)));
  EXPECT_OK(vmo->Write(&data, kPageSize, sizeof(data)));
  EXPECT_TRUE(make_private_attribution_counts(2ul * kPageSize, 0) == vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));

  // Compress the second page.
  vm_page_t* page;
  status = vmo->GetPageBlocking(kPageSize, 0, &page, nullptr);
  ASSERT_OK(status);
  {
    auto compressor = compression->AcquireCompressor();
    EXPECT_OK(compressor.get().Arm());
    ASSERT_TRUE(compress_page(vmo, page, kPageSize, VmCowPages::EvictionAction::FollowHint,
                              &compressor.get()));
  }
  EXPECT_TRUE(make_private_attribution_counts(kPageSize, kPageSize) == vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));

  // Prefetch the entire VMO.
  EXPECT_OK(vmo->PrefetchRange(0, kPageSize * 2));

  // Both pages should be back to being uncompressed.
  EXPECT_TRUE(make_private_attribution_counts(2ul * kPageSize, 0) == vmo->GetAttributedMemory())
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));

  END_TEST;
}

// Check that committed ranges in children correctly have range updates skipped.
bool vmo_skip_range_update_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;
  constexpr uint64_t kNumPages = 16;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize * kNumPages, &vmo));

  EXPECT_OK(vmo->CommitRange(0, kPageSize * kNumPages));

  fbl::RefPtr<VmObject> child;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0u,
                             kPageSize * kNumPages, false, &child));

  // Fork some pages into the child to have some regions that should be able to avoid range updates.
  for (auto page : {4, 5, 6, 10, 11, 12}) {
    uint64_t data = 42;
    EXPECT_OK(child->Write(&data, kPageSize * page, sizeof(data)));
  }

  // Create a user memory mapping to check if unmaps do and do not get performed.
  ktl::unique_ptr<testing::UserMemory> user_memory = testing::UserMemory::Create(child, 0, 0);
  ASSERT_TRUE(user_memory);

  // Reach into the hidden parent so we can directly perform range updates.
  fbl::RefPtr<VmCowPages> hidden_parent = vmo->DebugGetCowPages()->DebugGetParent();
  ASSERT_TRUE(hidden_parent);

  struct {
    uint64_t page_start;
    uint64_t num_pages;
    ktl::array<int, kNumPages> unmapped;
  } test_ranges[] = {
      // Simple range that is not covered in the child should get unmapped
      {0, 1, {0, -1}},
      // Various ranges fully covered by the child should have no unmappings
      {4, 1, {-1}},
      {4, 3, {-1}},
      {6, 1, {-1}},
      // Ranges that partially touch a single committed range should get trimmed
      {3, 2, {3, -1}},
      {6, 2, {7, -1}},
      // Range that spans a single gap that sees the parent should get trimmed at both ends to just
      // that gap.
      {4, 9, {7, 8, 9, -1}},
      // Spanning across a committed range causes us to still have to unnecessarily unmap.
      {3, 10, {3, 4, 5, 6, 7, 8, 9, -1}},
      {4, 10, {7, 8, 9, 10, 11, 12, 13, -1}},
      {3, 11, {3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, -1}},
  };

  for (auto& range : test_ranges) {
    // Ensure all the mappings start populated.
    for (uint64_t i = 0; i < kNumPages; i++) {
      user_memory->get<char>(i * kPageSize);
      paddr_t paddr;
      arch_mmu_flags_t mmu_flags;
      EXPECT_OK(user_memory->aspace()->arch_aspace().Query(user_memory->base() + i * kPageSize,
                                                           &paddr, &mmu_flags));
    }
    // Perform the requested range update.
    {
      VmCowPages::DeferredOps deferred(hidden_parent.get());
      Guard<CriticalMutex> guard{hidden_parent->lock()};
      const VmCowRange range_update =
          VmCowRange(range.page_start * kPageSize, range.num_pages * kPageSize);
      hidden_parent->RangeChangeUpdateLocked(range_update, VmCowPages::RangeChangeOp::Unmap,
                                             &deferred);
    }
    // Check all the mappings are either there or not there as expected.
    bool expected[kNumPages];
    for (uint64_t i = 0; i < kNumPages; i++) {
      expected[i] = true;
    }
    for (auto page : range.unmapped) {
      // page of -1 is a sentinel as we cannot use the default 0 as sentinel.
      if (page == -1) {
        break;
      }
      expected[page] = false;
    }
    for (uint64_t i = 0; i < kNumPages; i++) {
      paddr_t paddr;
      arch_mmu_flags_t mmu_flags;
      zx_status_t status = user_memory->aspace()->arch_aspace().Query(
          user_memory->base() + i * kPageSize, &paddr, &mmu_flags);
      EXPECT_EQ(expected[i] ? ZX_OK : ZX_ERR_NOT_FOUND, status);
    }
  }

  END_TEST;
}

// Tests that loaned pages can't appear in a high priority VMO through manipulation of the priority
// count.
bool vmo_loaned_page_in_high_priority_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  const bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  const auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  fbl::RefPtr<VmObjectPaged> contiguous_vmo;
  ASSERT_OK(VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, kPageSize,
                                            /*alignment_log2*/ 0, &contiguous_vmo));
  ASSERT_OK(contiguous_vmo->DecommitRange(0, kPageSize));

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_committed_pager_vmo(1, /*trap_dirty*/ false, /*resizable*/ false, nullptr, &vmo));

  fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();

  ASSERT_OK(cow->ReplacePageWithLoaned(cow->DebugGetPage(0), 0));

  auto change_priority = [&vmo](int64_t delta) {
    PriorityChanger pc = vmo->MakePriorityChanger(delta);
    if (delta > 0) {
      pc.PrepareMayNotAlreadyBeHighPriority();
    }
    Guard<CriticalMutex> guard{AliasedLock, vmo->lock(), pc.lock()};
    pc.ChangeHighPriorityCountLocked();
  };
  change_priority(1);

  vm_page_t* page = cow->DebugGetPage(0);
  EXPECT_TRUE(!page || !page->is_loaned());  // we have decomitted the page

  // Destructor checks that we go back down to high_priority_count_ == 0
  change_priority(-1);

  END_TEST;
}

// Test stream functionality in kernel objects.
bool vmo_user_stream_size_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // 4 page VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 4 * kPageSize, &vmo));

  {
    Guard<CriticalMutex> guard{vmo->lock()};
    EXPECT_EQ(vmo->size_locked(), (uint64_t)4 * kPageSize);
    // Should not have an allocated stream size.
    auto result = vmo->user_stream_size_locked();
    EXPECT_FALSE(result.has_value());
  }

  // Give VMO a user-defined stream size of 2 pages.
  fbl::RefPtr<StreamSizeManager> ssm;
  auto ssm_result = StreamSizeManager::Create(2 * kPageSize);
  ASSERT_OK(ssm_result.status_value());
  ssm = ktl::move(*ssm_result);

  vmo->SetUserStreamSize(ssm);

  {
    Guard<CriticalMutex> guard{vmo->lock()};
    auto result = vmo->user_stream_size_locked();
    ASSERT_TRUE(result.has_value());
    const uint64_t stream_size = result.value();
    EXPECT_EQ(stream_size, (uint64_t)2 * kPageSize);

    // VMO size should be unchanged.
    EXPECT_EQ(vmo->size_locked(), (uint64_t)4 * kPageSize);
  }

  END_TEST;
}

// Test that we can't get loaned pages in high priority VMOs by abusing a lack of
// visibility into the parent.
bool vmo_loaned_high_priority_parent_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  constexpr size_t parent_size_pages = 2;
  constexpr size_t parent_size_bytes = kPageSize * parent_size_pages;

  // create a contiguous VMO so that we are guaranteed to have a place to borrow a page from
  fbl::RefPtr<VmObjectPaged> contiguous_vmo;
  ASSERT_OK(VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, kPageSize,
                                            /*alignment_log2*/ 0, &contiguous_vmo));
  ASSERT_OK(contiguous_vmo->DecommitRange(0, kPageSize));

  fbl::RefPtr<VmObjectPaged> parent_vmo;
  ASSERT_OK(make_committed_pager_vmo(parent_size_pages, /*trap_dirty*/ false, /*resizable*/ false,
                                     nullptr, &parent_vmo));

  fbl::RefPtr<VmCowPages> parent_cow = parent_vmo->DebugGetCowPages();

  fbl::RefPtr<VmObject> child_vmo_no_paged;
  ASSERT_OK(parent_vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite,
                                    /*offset*/ 0, parent_size_bytes, /*copy_name*/ true,
                                    &child_vmo_no_paged));
  VmObjectPaged* child_vmo = DownCastVmObject<VmObjectPaged>(child_vmo_no_paged.get());
  ASSERT_NONNULL(child_vmo);

  // commit the first child page so that the first parent page becomes inaccessible from the
  // child
  ASSERT_OK(child_vmo->CommitRange(0, kPageSize));

  // replace the parent page with a loaned page
  ASSERT_OK(parent_cow->ReplacePageWithLoaned(parent_cow->DebugGetPage(0), 0));

  auto change_priority = [&child_vmo](int64_t delta) {
    PriorityChanger pc = child_vmo->MakePriorityChanger(delta);
    if (delta > 0) {
      pc.PrepareMayNotAlreadyBeHighPriority();
    }
    Guard<CriticalMutex> guard{AliasedLock, child_vmo->lock(), pc.lock()};
    pc.ChangeHighPriorityCountLocked();
  };

  change_priority(1);

  vm_page_t* parent_first_page = parent_cow->DebugGetPage(0);
  EXPECT_TRUE(!parent_first_page || !parent_first_page->is_loaned());

  change_priority(-1);

  END_TEST;
}

// Attempts to transfer data over a parent content marker slot where the parent content is a zero
// marker.
bool vmo_zero_marker_transfer_test() {
  BEGIN_TEST;

  // Need a compressor.
  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  AutoVmScannerDisable scanner_disable;

  const size_t kNumPages = 1;
  const size_t alloc_size = kNumPages * kPageSize;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &vmo));

  ASSERT_OK(vmo->CommitRange(0, alloc_size));

  fbl::RefPtr<VmObject> clone1;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, alloc_size, false,
                             &clone1));

  ASSERT_OK(vmo->CommitRange(0, alloc_size));

  // A second level clone is needed so that the compressor actually inserts a zero marker instead of
  // simply freeing up the deduped zero page.
  fbl::RefPtr<VmObject> clone2;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, alloc_size, false,
                             &clone2));

  // Compress the parent page by reaching into the hidden VMO parent.
  fbl::RefPtr<VmCowPages> hidden_root = vmo->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(hidden_root);
  vm_page_t* page = hidden_root->DebugGetPage(0);
  ASSERT_NONNULL(page);
  {
    auto compressor = compression->AcquireCompressor();
    ASSERT_OK(compressor.get().Arm());
    // Attempt to reclaim the page in the hidden parent.
    uint64_t reclaimed =
        reclaim(hidden_root, page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get());
    EXPECT_EQ(reclaimed, 1u);
  }
  ASSERT_TRUE(hidden_root->DebugIsMarker(0));

  // Transfer data, overwriting the parent content marker.
  fbl::RefPtr<VmObjectPaged> aux;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, alloc_size, &aux));
  ASSERT_OK(aux->CommitRange(0, alloc_size));

  VmPageSpliceList pages;
  ASSERT_OK(aux->TakePages(0, alloc_size, &pages));
  ASSERT_OK(vmo->SupplyPages(0, alloc_size, &pages, SupplyOptions::TransferData));

  END_TEST;
}

bool vmo_pager_supply_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  constexpr size_t kNumPages = 4;
  constexpr size_t alloc_size = kNumPages * kPageSize;
  constexpr size_t half_size = alloc_size / 2;

  // Aux VMO.
  fbl::RefPtr<VmObjectPaged> aux_vmo;
  ASSERT_OK(
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, alloc_size, &aux_vmo));

  // Pager-backed VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      make_uncommitted_pager_vmo(kNumPages, /*trap_dirty=*/false, /*resizable=*/false, &vmo);
  ASSERT_EQ(ZX_OK, status);
  vmo->set_user_id(0x42);

  // Supply pager VMO with 2 pages of random data.
  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> buf_rand1;
  buf_rand1.reserve(half_size, &ac);
  ASSERT_TRUE(ac.check());
  fill_region(0x77, buf_rand1.data(), half_size);
  ASSERT_OK(aux_vmo->Write(buf_rand1.data(), 0, half_size));

  VmPageSpliceList sl;
  EXPECT_OK(aux_vmo->TakePages(0, half_size, &sl));
  ASSERT_OK(vmo->SupplyPages(0, half_size, &sl, SupplyOptions::PagerSupply));
  DEBUG_ASSERT(sl.IsProcessed());

  // Change data in aux vmo.
  fbl::Vector<uint8_t> buf_rand2;
  buf_rand2.reserve(alloc_size, &ac);
  ASSERT_TRUE(ac.check());
  fill_region(0x88, buf_rand2.data(), alloc_size);
  EXPECT_OK(aux_vmo->Write(buf_rand2.data(), 0, alloc_size));

  // Supply 4 pages of new data to the VMO.
  VmPageSpliceList sl2;
  EXPECT_OK(aux_vmo->TakePages(0, alloc_size, &sl2));
  ASSERT_OK(vmo->SupplyPages(0, alloc_size, &sl2, SupplyOptions::PagerSupply));
  DEBUG_ASSERT(sl2.IsProcessed());

  fbl::Vector<uint8_t> buf_check;
  buf_check.reserve(alloc_size, &ac);
  ASSERT_TRUE(ac.check());
  EXPECT_OK(vmo->Read(buf_check.data(), 0, alloc_size));

  // First two shouldn't have been overwritten.
  int cmpres = memcmp(buf_rand1.data(), buf_check.data(), half_size);
  EXPECT_EQ(0, cmpres);

  // Second 2 pages should have new data.
  cmpres = memcmp(buf_rand2.data() + half_size, buf_check.data() + half_size, half_size);
  EXPECT_EQ(0, cmpres);

  // VMO should have 4 attributede pages.
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));

  // Clone pager-backed VMO.
  fbl::RefPtr<VmObject> clone;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, alloc_size,
                             true, &clone));
  clone->set_user_id(0x43);

  // Vmo is attributed all pages
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));
  EXPECT_TRUE(clone->GetAttributedMemory() == make_private_attribution_counts(0, 0));

  // New random data in aux_vmo.
  fbl::Vector<uint8_t> buf_rand3;
  buf_rand3.reserve(alloc_size, &ac);
  ASSERT_TRUE(ac.check());
  fill_region(0x99, buf_rand3.data(), alloc_size);
  EXPECT_OK(aux_vmo->Write(buf_rand3.data(), 0, alloc_size));

  // Supply 2 pages into the middle of the clone.
  VmPageSpliceList sl3;
  EXPECT_OK(aux_vmo->TakePages(kPageSize, half_size, &sl3));
  ASSERT_OK(clone->SupplyPages(kPageSize, half_size, &sl3, SupplyOptions::TransferData));
  DEBUG_ASSERT(sl3.IsProcessed());

  // Clone is attributed both pages, VMO is unchanged.
  EXPECT_TRUE(clone->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));

  EXPECT_OK(clone->Read(buf_check.data(), 0, alloc_size));

  // First and last page in clone should be read from parent.
  cmpres = memcmp(buf_rand1.data(), buf_check.data(), kPageSize);
  EXPECT_EQ(0, cmpres);
  cmpres = memcmp(buf_rand2.data() + (alloc_size - kPageSize),
                  buf_check.data() + (alloc_size - kPageSize), kPageSize);
  EXPECT_EQ(0, cmpres);

  // Middle pages should be new.
  cmpres = memcmp(buf_rand3.data() + kPageSize, buf_check.data() + kPageSize, half_size);
  EXPECT_EQ(0, cmpres);

  // Parent should be unchanged.
  EXPECT_OK(vmo->Read(buf_check.data(), 0, alloc_size));
  cmpres = memcmp(buf_rand1.data(), buf_check.data(), half_size);
  EXPECT_EQ(0, cmpres);
  cmpres = memcmp(buf_rand2.data() + half_size, buf_check.data() + half_size, half_size);
  EXPECT_EQ(0, cmpres);

  // New random data in aux_vmo.
  fbl::Vector<uint8_t> buf_rand4;
  buf_rand4.reserve(alloc_size, &ac);
  ASSERT_TRUE(ac.check());
  fill_region(0x99, buf_rand4.data(), alloc_size);
  EXPECT_OK(aux_vmo->Write(buf_rand4.data(), 0, alloc_size));

  // Supply new data to all pages of clone.
  VmPageSpliceList sl4;
  EXPECT_OK(aux_vmo->TakePages(0, alloc_size, &sl4));
  ASSERT_OK(clone->SupplyPages(0, alloc_size, &sl4, SupplyOptions::TransferData));
  DEBUG_ASSERT(sl4.IsProcessed());

  // Clone should have new data.
  EXPECT_OK(clone->Read(buf_check.data(), 0, alloc_size));
  cmpres = memcmp(buf_rand4.data(), buf_check.data(), alloc_size);
  EXPECT_EQ(0, cmpres);

  // Parent should be unchanged.
  EXPECT_OK(vmo->Read(buf_check.data(), 0, alloc_size));
  cmpres = memcmp(buf_rand1.data(), buf_check.data(), half_size);
  EXPECT_EQ(0, cmpres);
  cmpres = memcmp(buf_rand2.data() + half_size, buf_check.data() + half_size, half_size);
  EXPECT_EQ(0, cmpres);

  // Each have 4 attributed pages.
  EXPECT_TRUE(clone->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(4ul * kPageSize, 0));

  // Clone the clone, which should create a hidden node.
  fbl::RefPtr<VmObject> clone2;
  ASSERT_OK(clone->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, alloc_size,
                               true, &clone2));
  clone2->set_user_id(0x44);

  // Private attribution counts 0 because pages were moved into hidden node.
  EXPECT_TRUE(clone->GetAttributedMemory().total_private_bytes() == 0);
  EXPECT_TRUE(clone2->GetAttributedMemory().total_private_bytes() == 0);

  // Each clone has 2 pages of scaled bytes, as they share 4 pages.
  EXPECT_TRUE(clone->GetAttributedMemory().total_scaled_bytes() ==
              vm::FractionalBytes(2ul * kPageSize));
  EXPECT_TRUE(clone2->GetAttributedMemory().total_scaled_bytes() ==
              vm::FractionalBytes(2ul * kPageSize));

  // Change data in aux VMO.
  fbl::Vector<uint8_t> buf_rand5;
  buf_rand5.reserve(alloc_size, &ac);
  ASSERT_TRUE(ac.check());
  fill_region(0xaa, buf_rand5.data(), alloc_size);
  EXPECT_OK(aux_vmo->Write(buf_rand5.data(), 0, alloc_size));

  // Supply 2 pages to Clone2.
  VmPageSpliceList sl5;
  EXPECT_OK(aux_vmo->TakePages(0, half_size, &sl5));
  ASSERT_OK(clone2->SupplyPages(0, half_size, &sl5, SupplyOptions::TransferData));
  DEBUG_ASSERT(sl5.IsProcessed());

  // Clone2 should have the two private pages and 3 scaled pages.
  EXPECT_TRUE(clone2->GetAttributedMemory().total_private_bytes() == 2ul * kPageSize);
  EXPECT_TRUE(clone2->GetAttributedMemory().total_scaled_bytes() ==
              vm::FractionalBytes(3ul * kPageSize));

  // Clone should now have 3 scaled pages as two are no longer seen by clone2.
  EXPECT_TRUE(clone->GetAttributedMemory().total_scaled_bytes() ==
              vm::FractionalBytes(3ul * kPageSize));

  END_TEST;
}

// Test some operations on leaf VMOs, that may have a parent content marker, work correctly when the
// hidden node had a page that gets deduped to the zero page.
static bool vmo_dedup_hidden_zero_page_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  constexpr size_t kVmoSize = kPageSize * 3;

  // Helper lambda to perform the core test logic with different vmo setups.
  auto test_with_vmo = [](fbl::RefPtr<VmObjectPaged> vmo) -> bool {
    BEGIN_TEST;

    // Write some zeroes to the middle page to cause it to be committed.
    uint64_t val = 0;
    EXPECT_OK(vmo->Write(&val, kPageSize, sizeof(val)));
    EXPECT_EQ(vmo->GetAttributedMemoryInRange(0, kVmoSize).private_uncompressed_bytes, kPageSize);

    // Create a clone that sees the parent content.
    fbl::RefPtr<VmObject> child;
    ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kVmoSize,
                               true, &child));

    // Expect a hidden hierarchy with both the original vmo and the new child having no private
    // bytes, but having an uncompressed page shared between them.
    EXPECT_EQ(vmo->GetAttributedMemoryInRange(0, kVmoSize).private_uncompressed_bytes, 0u);
    EXPECT_EQ(child->GetAttributedMemoryInRange(0, kVmoSize).private_uncompressed_bytes, 0u);
    EXPECT_EQ(vmo->GetAttributedMemoryInRange(0, kVmoSize).uncompressed_bytes, kPageSize);
    EXPECT_EQ(child->GetAttributedMemoryInRange(0, kVmoSize).uncompressed_bytes, kPageSize);

    // Dedupe the page in the hidden parent to the zero page.
    vm_page_t* page = vmo->DebugGetCowPages()->DebugGetParent()->DebugGetPage(kPageSize);
    ASSERT_NONNULL(page);
    EXPECT_TRUE(vmo->DebugGetCowPages()->DebugGetParent()->DedupZeroPage(page, kPageSize));

    // If deduped there should be no content attributed to either leaf VMO.
    EXPECT_EQ(vmo->GetAttributedMemoryInRange(0, kVmoSize).private_uncompressed_bytes, 0u);
    EXPECT_EQ(child->GetAttributedMemoryInRange(0, kVmoSize).private_uncompressed_bytes, 0u);
    EXPECT_EQ(vmo->GetAttributedMemoryInRange(0, kVmoSize).uncompressed_bytes, 0u);
    EXPECT_EQ(child->GetAttributedMemoryInRange(0, kVmoSize).uncompressed_bytes, 0u);

    // Zero the child range, validating that any ParentContent markers / zero page markers are
    // handled correctly.
    EXPECT_OK(child->ZeroRange(kPageSize, kPageSize));
    EXPECT_OK(child->Read(&val, kPageSize, sizeof(val)));
    EXPECT_EQ(val, 0u);

    // Write to the original VMO, validating that handling of any ParentContent markers.
    vmo->Write(&val, kPageSize, sizeof(val));
    EXPECT_EQ(vmo->GetAttributedMemoryInRange(0, kVmoSize).private_uncompressed_bytes, kPageSize);
    EXPECT_EQ(vmo->GetAttributedMemoryInRange(0, kVmoSize).uncompressed_bytes, kPageSize);
    END_TEST;
  };

  // Test with a plain anonymous root vmo
  {
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(0, 0, kVmoSize, &vmo));
    EXPECT_TRUE(test_with_vmo(vmo));
  }
  // Test with a pager backed root in a snapshot modified hierarchy.
  {
    fbl::RefPtr<VmObjectPaged> aux_vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &aux_vmo));
    uint64_t val = 42;
    EXPECT_OK(aux_vmo->Write(&val, 0, sizeof(val)));

    fbl::RefPtr<VmObjectPaged> pager;
    ASSERT_OK(make_uncommitted_pager_vmo(3, /*trap_dirty=*/false, /*resizable=*/false, &pager));

    VmPageSpliceList sl;
    EXPECT_OK(aux_vmo->TakePages(0, kPageSize, &sl));
    ASSERT_OK(pager->SupplyPages(kPageSize, kPageSize, &sl, SupplyOptions::PagerSupply));
    DEBUG_ASSERT(sl.IsProcessed());

    fbl::RefPtr<VmObject> vmo;
    pager->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kVmoSize, true, &vmo);
    EXPECT_TRUE(test_with_vmo(DownCastVmObject<VmObjectPaged>(vmo)));
  }

  END_TEST;
}

// Regression test for https://fxbug.dev/504708573. Attempt to zero a range that has pages mapped in
// the kernel after committed pages.
static bool vmo_zero_partially_pinned_range_test() {
  BEGIN_TEST;

  // Ensure that we do not compress pages before ZeroRange acquires the VmCowPage lock, as this
  // would prevent the unmap round-up optimization from being triggered.
  AutoVmScannerDisable scanner_disable;

  auto test_vmo = [](fbl::RefPtr<VmObject> vmo) -> bool {
    BEGIN_TEST;

    // Commit a page to force an unmap when the range is zeroed.
    ASSERT_OK(vmo->CommitRange(0, kPageSize));

    auto ka = VmAspace::kernel_aspace();
    void* ptr;
    ASSERT_OK(ka->MapObjectInternal(vmo, "test", /*offset=*/kPageSize, /*size=*/kPageSize, &ptr, 0,
                                    VmAspace::VMM_FLAG_COMMIT, kArchRwFlags));
    auto cleanup_mapping =
        fit::defer([&ka, ptr] { ASSERT(ZX_OK == ka->FreeRegion(reinterpret_cast<vaddr_t>(ptr))); });

    EXPECT_OK(vmo->ZeroRange(0, 2 * kPageSize));

    END_TEST;
  };

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 2 * kPageSize, &vmo));
    EXPECT_TRUE(test_vmo(ktl::move(vmo)));
  }

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(make_committed_pager_vmo(2, /*trap_dirty=*/false, /*resizable=*/false,
                                       /*out_pages=*/nullptr, &vmo));
    EXPECT_TRUE(test_vmo(ktl::move(vmo)));
  }

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(make_committed_pager_vmo(2, /*trap_dirty=*/false, /*resizable=*/false,
                                       /*out_pages=*/nullptr, &vmo));
    fbl::RefPtr<VmObject> unidirectional_clone;
    ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, 2 * kPageSize,
                               false, &unidirectional_clone));
    EXPECT_TRUE(test_vmo(ktl::move(unidirectional_clone)));
  }

  {
    // Same as above; use the parent instead of the child though.
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(make_committed_pager_vmo(2, /*trap_dirty=*/false, /*resizable=*/false,
                                       /*out_pages=*/nullptr, &vmo));
    fbl::RefPtr<VmObject> unidirectional_clone;
    ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, 2 * kPageSize,
                               false, &unidirectional_clone));
    EXPECT_TRUE(test_vmo(ktl::move(vmo)));
  }

  {
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 2 * kPageSize, &vmo));
    ASSERT_OK(vmo->CommitRange(0, 2 * kPageSize));

    fbl::RefPtr<VmObject> bidirectional_clone;
    ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, 2 * kPageSize,
                               false, &bidirectional_clone));
    EXPECT_TRUE(test_vmo(ktl::move(bidirectional_clone)));
  }

  END_TEST;
}

// Test that unmaps propagated to copy-on-write children are not applied to kernel mappings.
static bool vmo_apply_unmap_to_child_with_kernel_mapping_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_committed_pager_vmo(4, /*trap_dirty=*/false, /*resizable=*/false,
                                     /*out_pages=*/nullptr, &vmo));

  fbl::RefPtr<VmObject> unidirectional_clone_no_paged;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, kPageSize,
                             3 * kPageSize, false, &unidirectional_clone_no_paged));
  fbl::RefPtr<VmObjectPaged> unidirectional_clone =
      DownCastVmObject<VmObjectPaged>(unidirectional_clone_no_paged);
  ASSERT_NONNULL(unidirectional_clone);

  auto ka = VmAspace::kernel_aspace();
  void* ptr;
  ASSERT_OK(ka->MapObjectInternal(unidirectional_clone, "test", /*offset=*/kPageSize,
                                  /*size=*/kPageSize, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
                                  kArchRwFlags));
  auto cleanup_mapping =
      fit::defer([&ka, ptr] { ASSERT(ZX_OK == ka->FreeRegion(reinterpret_cast<vaddr_t>(ptr))); });

  // Show that this is indeed a unidirectional clone, and that the kernel mapping will be subject to
  // the attempted unmap.
  vm_page_t* page = unidirectional_clone->DebugGetCowPages()->DebugGetPage(kPageSize);
  ASSERT_NONNULL(page);
  EXPECT_GT(page->object.pin_count, 0u);
  EXPECT_TRUE(unidirectional_clone->DebugGetCowPages()->DebugIsEmpty(0));
  EXPECT_TRUE(unidirectional_clone->DebugGetCowPages()->DebugIsEmpty(2 * kPageSize));
  EXPECT_EQ(unidirectional_clone->DebugGetCowPages()->DebugGetParent().get(),
            vmo->DebugGetCowPages().get());

  // This does not crash.
  EXPECT_OK(vmo->ZeroRange(0, 4 * kPageSize));

  END_TEST;
}

static bool vmo_compress_to_marker_pager_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kNumPages = 2;
  constexpr size_t kVmoSize = kPageSize * kNumPages;

  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> zero_buff;
  zero_buff.reserve(kVmoSize, &ac);
  ASSERT_TRUE(ac.check());
  memset(zero_buff.data(), 0, kVmoSize);

  uint32_t val = 42;

  VmCompression* compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    printf("No compression, skipping\n");
    END_TEST;
  }

  // Pager VMO

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_committed_pager_vmo(kNumPages, /*trap_dirty=*/false, /*resizable=*/false, nullptr,
                                     &vmo));

  // Clone with pages of zeros
  fbl::RefPtr<VmObject> clone1;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kVmoSize, true,
                             &clone1));

  clone1->Write(zero_buff.data(), 0, kVmoSize);

  // Clone again to move zero pages into hidden node.
  fbl::RefPtr<VmObject> clone2;
  clone1->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kVmoSize, true,
                      &clone2);

  VmObjectPaged* clone1ptr = reinterpret_cast<VmObjectPaged*>(clone1.get());
  fbl::RefPtr<VmCowPages> cow_pages_hidden = clone1ptr->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(cow_pages_hidden);
  ASSERT_FALSE(cow_pages_hidden->tree_has_parent_content_markers());
  ASSERT_EQ(1u, cow_pages_hidden->DebugGetPage(0)->object.share_count);
  ASSERT(cow_pages_hidden->DebugIsPage(0));

  // Compress pages into marker.
  vm_page_t* page = cow_pages_hidden->DebugGetPage(0);
  ASSERT_NONNULL(page);

  {
    auto compressor = compression->AcquireCompressor();
    ASSERT_OK(compressor.get().Arm());

    uint64_t reclaimed = reclaim(cow_pages_hidden, page, 0, VmCowPages::EvictionAction::FollowHint,
                                 &compressor.get());
    EXPECT_EQ(reclaimed, 1u);
  }

  ASSERT_TRUE(cow_pages_hidden->DebugIsMarker(0));
  ASSERT_EQ(1u, cow_pages_hidden->DebugGetMarkerShareCount(0));

  clone1->Write(&val, 0, sizeof(val));
  ASSERT_EQ(0u, cow_pages_hidden->DebugGetMarkerShareCount(0));

  clone2.reset();
  clone1.reset();
  vmo.reset();

  END_TEST;
}

static bool vmo_compress_to_marker_anon_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kNumPages = 2;
  constexpr size_t kVmoSize = kPageSize * kNumPages;

  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> zero_buff;
  zero_buff.reserve(kVmoSize, &ac);
  ASSERT_TRUE(ac.check());
  memset(zero_buff.data(), 0, kVmoSize);

  uint32_t val = 42;

  VmCompression* compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    printf("No compression, skipping\n");
    END_TEST;
  }

  // Write to pages.
  fbl::RefPtr<VmObjectPaged> anon_vmo;
  ASSERT_OK(VmObjectPaged::Create(0, 0, kVmoSize, &anon_vmo));
  EXPECT_OK(anon_vmo->Write(&val, 0, sizeof(val)));
  EXPECT_OK(anon_vmo->Write(&val, kPageSize, sizeof(val)));

  fbl::RefPtr<VmObject> anon_clone1;
  ASSERT_OK(anon_vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0, kVmoSize,
                                  true, &anon_clone1));

  // Write zeros into clone.
  anon_clone1->Write(zero_buff.data(), 0, kVmoSize);

  fbl::RefPtr<VmObject> anon_clone2;
  ASSERT_OK(anon_clone1->CreateClone(Resizability::NonResizable, SnapshotType::Modified, 0,
                                     kVmoSize, true, &anon_clone2));

  VmObjectPaged* anon_clone1ptr = reinterpret_cast<VmObjectPaged*>(anon_clone1.get());
  fbl::RefPtr<VmCowPages> cow_pages_hidden = anon_clone1ptr->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(cow_pages_hidden);
  ASSERT_TRUE(cow_pages_hidden->tree_has_parent_content_markers());
  ASSERT_EQ(1u, cow_pages_hidden->DebugGetPage(0)->object.share_count);
  ASSERT(cow_pages_hidden->DebugIsPage(0));

  // Compress pages into marker.
  vm_page_t* page = cow_pages_hidden->DebugGetPage(0);
  ASSERT_NONNULL(page);

  {
    auto compressor = compression->AcquireCompressor();
    ASSERT_OK(compressor.get().Arm());

    uint64_t reclaimed = reclaim(cow_pages_hidden, page, 0, VmCowPages::EvictionAction::FollowHint,
                                 &compressor.get());
    EXPECT_EQ(reclaimed, 1u);
  }

  ASSERT_TRUE(cow_pages_hidden->DebugIsMarker(0));
  ASSERT_EQ(1u, cow_pages_hidden->DebugGetMarkerShareCount(0));

  // Writing to the clone should decrement marker share count.
  anon_clone1->Write(&val, 0, sizeof(val));
  ASSERT_EQ(0u, cow_pages_hidden->DebugGetMarkerShareCount(0));

  // Writing to the second clone should clear the marker from the hidden node
  anon_clone2->Write(&val, 0, sizeof(val));
  ASSERT_TRUE(cow_pages_hidden->DebugIsEmpty(0));

  anon_clone1.reset();
  anon_clone2.reset();
  anon_vmo.reset();

  END_TEST;
}

// Verify that we don't trigger a panic during destruction of an always-pinned, but empty VMO.
//
// This is a regression test for https://fxbug.dev/511552403.
static bool vmo_always_pinned_with_no_pages_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmObjectPaged> vmo;

  // Note that this call will fail.  That's because we've requested a zero-sized always-pinned
  // VMO, which is not a valid request.  However, under the hood, we'll make it far enough to create
  // the VMO even thought it will be destroyed before the call returns.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kAlwaysPinned, 0, &vmo));

  END_TEST;
}

// Verify that LookupReadableLocked works for a simple VMO with all pages committed.
static bool vmo_lookup_readable_simple_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t page_count = 4;
  constexpr size_t alloc_size = kPageSize * page_count;

  // Create a VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &vmo);
  ASSERT_OK(status);
  ASSERT_TRUE(vmo);

  // Commit the whole VMO.
  status = vmo->CommitRange(0, alloc_size);
  ASSERT_OK(status);

  // Lookup readable on the VMO should find all 4 pages.
  size_t pages_seen = 0;
  auto lookup_fn = [&pages_seen](uint64_t offset, paddr_t pa) {
    pages_seen++;
    return ZX_ERR_NEXT;
  };

  VmCowPages* vmo_cow = vmo->DebugGetCowPages().get();
  status = vmo_cow->DebugLookupReadable(VmCowRange(0, alloc_size), lookup_fn);
  EXPECT_OK(status);
  EXPECT_EQ(page_count, pages_seen);

  END_TEST;
}

// Verify that LookupReadableLocked works when a parent VMO lookup segment is
// immediately followed by a page committed locally in the clone VMO (which splits
// the parent lookup). This is a regression test for https://fxbug.dev/513654391.
static bool vmo_lookup_readable_clone_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t page_count = 4;
  constexpr size_t alloc_size = kPageSize * page_count;

  // Create a parent VMO.
  fbl::RefPtr<VmObjectPaged> parent;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &parent);
  ASSERT_OK(status);
  ASSERT_TRUE(parent);

  parent->set_user_id(42u);

  // Commit the whole parent VMO.
  status = parent->CommitRange(0, alloc_size);
  ASSERT_OK(status);

  // Create a COW clone of the parent.
  fbl::RefPtr<VmObject> clone_no_paged;
  status = parent->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, alloc_size, false,
                               &clone_no_paged);
  ASSERT_OK(status);
  ASSERT_TRUE(clone_no_paged);

  clone_no_paged->set_user_id(43u);
  VmObjectPaged* clone = DownCastVmObject<VmObjectPaged>(clone_no_paged.get());
  ASSERT_NONNULL(clone);

  // Commit page 1 in the clone to split the parent lookup.
  status = clone->CommitRange(kPageSize, kPageSize);
  ASSERT_OK(status);

  // Lookup readable on the clone VMO should find all 4 pages.
  size_t pages_seen = 0;
  auto lookup_fn = [&pages_seen](uint64_t offset, paddr_t pa) {
    pages_seen++;
    return ZX_ERR_NEXT;
  };

  VmCowPages* clone_cow = clone->DebugGetCowPages().get();
  status = clone_cow->DebugLookupReadable(VmCowRange(0, alloc_size), lookup_fn);
  EXPECT_OK(status);
  EXPECT_EQ(page_count, pages_seen);

  END_TEST;
}

// Test that all offsets of a VMO are accessible via GetPage.
//
// This is a regression test for https://fxbug.dev/515752748.
static bool vmo_get_page_offset_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  const uint64_t size = 10 * kPageSize;
  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_partially_committed_pager_vmo(/*num_pages=*/10, /*committed_pages=*/0,
                                               /*trap_dirty=*/false, /*resizable=*/false,
                                               /*ignore_requests=*/true, nullptr, &vmo));

  for (uint64_t i = 0; i < size; i += kPageSize) {
    vm_page_t* page;

    // Use VMM_PF_FLAG_FAULT_MASK so that GetPage attempts to acquire a page if none is present in
    // the local page list.

    __UNINITIALIZED MultiPageRequest page_request;
    zx_status_t status = vmo->GetPage(i, VMM_PF_FLAG_FAULT_MASK, &page_request, &page, nullptr);
    if (status == ZX_ERR_SHOULD_WAIT) {
      // The stub page provider does not support waiting.
      page_request.CancelRequests();
    } else {
      EXPECT_OK(status);
    }
  }

  END_TEST;
}

// Tests that when creating a bidirectional clone (snapshot) of a once-pinned VMO,
// the newly created hidden parent correctly inherits the `ever_pinned_` flag.
static bool vmo_ever_pinned_hidden_parent_creation_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kVmoSize = kPageSize;

  // Create root VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(0, 0, kVmoSize, &vmo));

  // Commit a page at offset 0.
  uint32_t val = 0x42;
  ASSERT_OK(vmo->Write(&val, 0, sizeof(val)));

  fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
  ASSERT_NONNULL(cow);

  // Initially, ever_pinned_ should be false.
  EXPECT_TRUE(cow->should_delay_reuse_on_free() == PmmOptDelayReuse::Default);

  // Pin the page.
  ASSERT_OK(vmo->CommitRangePinned(0, kVmoSize, true));

  // ever_pinned_ should be true.
  EXPECT_TRUE(cow->should_delay_reuse_on_free() == PmmOptDelayReuse::Yes);

  // Unpin the page.
  vmo->Unpin(0, kVmoSize);

  // ever_pinned_ should still be true.
  EXPECT_TRUE(cow->should_delay_reuse_on_free() == PmmOptDelayReuse::Yes);

  // Create a bidirectional clone (snapshot) of the root VMO.
  fbl::RefPtr<VmObject> clone;
  ASSERT_OK(
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kVmoSize, true, &clone));

  // Retrieve the hidden parent.
  fbl::RefPtr<VmCowPages> h_cow = cow->DebugGetParent();
  ASSERT_NONNULL(h_cow);

  EXPECT_EQ(PmmOptDelayReuse::Yes, h_cow->should_delay_reuse_on_free());
  EXPECT_EQ(PmmOptDelayReuse::Default, cow->should_delay_reuse_on_free());

  END_TEST;
}

// Tests that when a once-pinned page is migrated into a sibling clone during copy-on-write page
// migration, the sibling clone correctly inherits the `ever_pinned_` flag.
static bool vmo_ever_pinned_page_migration_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kVmoSize = kPageSize;

  // Create root VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(0, 0, kVmoSize, &vmo));

  // Commit a page at offset 0.
  uint32_t val = 0x42;
  ASSERT_OK(vmo->Write(&val, 0, sizeof(val)));

  fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
  ASSERT_NONNULL(cow);

  // Pin and unpin the page.
  ASSERT_OK(vmo->CommitRangePinned(0, kVmoSize, true));
  vmo->Unpin(0, kVmoSize);

  // Create a bidirectional clone (snapshot) of the root VMO.
  fbl::RefPtr<VmObject> clone;
  ASSERT_OK(
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kVmoSize, true, &clone));

  fbl::RefPtr<VmCowPages> c_cow = DownCastVmObject<VmObjectPaged>(clone.get())->DebugGetCowPages();
  ASSERT_NONNULL(c_cow);

  // The sibling clone is created with ever_pinned_ = false.
  EXPECT_TRUE(c_cow->should_delay_reuse_on_free() == PmmOptDelayReuse::Default);

  // Write to the root VMO to fork the page. The original once-pinned page in the hidden parent is
  // now only visible to the clone.
  uint32_t val2 = 0x43;
  ASSERT_OK(vmo->Write(&val2, 0, sizeof(val2)));

  // Write to the clone to trigger page migration from the hidden parent to the clone.
  ASSERT_OK(clone->Write(&val, 0, sizeof(val)));

  // The clone should now have ever_pinned_ = true since the once-pinned page was migrated into it.
  EXPECT_EQ(PmmOptDelayReuse::Yes, c_cow->should_delay_reuse_on_free());

  END_TEST;
}

// Tests that when a hidden parent collapses and merges its pages into a child clone, the child
// clone correctly inherits the `ever_pinned_` flag.
static bool vmo_ever_pinned_parent_merge_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kVmoSize = kPageSize;

  // Create root VMO.
  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(0, 0, kVmoSize, &vmo));

  // Commit a page at offset 0.
  uint32_t val = 0x42;
  ASSERT_OK(vmo->Write(&val, 0, sizeof(val)));

  fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
  ASSERT_NONNULL(cow);

  // Pin and unpin the page.
  ASSERT_OK(vmo->CommitRangePinned(0, kVmoSize, true));
  vmo->Unpin(0, kVmoSize);

  // Create a bidirectional clone (snapshot) of the root VMO.
  fbl::RefPtr<VmObject> clone;
  ASSERT_OK(
      vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kVmoSize, true, &clone));

  fbl::RefPtr<VmCowPages> c_cow = DownCastVmObject<VmObjectPaged>(clone.get())->DebugGetCowPages();
  ASSERT_NONNULL(c_cow);

  // The sibling clone is created with ever_pinned_ = false.
  EXPECT_TRUE(c_cow->should_delay_reuse_on_free() == PmmOptDelayReuse::Default);

  // Close the root VMO. This merges the hidden parent's pages into the clone.
  vmo.reset();

  // The clone should now have ever_pinned_ = true since the hidden parent collapsed and
  // merged its pages into the clone.
  EXPECT_EQ(PmmOptDelayReuse::Yes, c_cow->should_delay_reuse_on_free());

  END_TEST;
}

UNITTEST_START_TESTCASE(vmo_tests)
VM_UNITTEST(vmo_create_test)
VM_UNITTEST(vmo_create_maximum_size)
VM_UNITTEST(vmo_pin_test)
VM_UNITTEST(vmo_pin_contiguous_test)
VM_UNITTEST(vmo_multiple_pin_test)
VM_UNITTEST(vmo_multiple_pin_contiguous_test)
VM_UNITTEST(vmo_commit_test)
VM_UNITTEST(vmo_commit_compressed_pages_test)
VM_UNITTEST(vmo_unaligned_size_test)
VM_UNITTEST(vmo_reference_attribution_commit_test)
VM_UNITTEST(vmo_create_physical_test)
VM_UNITTEST(vmo_physical_pin_test)
VM_UNITTEST(vmo_create_contiguous_test)
VM_UNITTEST(vmo_contiguous_decommit_test)
VM_UNITTEST(vmo_contiguous_decommit_disabled_test)
VM_UNITTEST(vmo_contiguous_decommit_enabled_test)
VM_UNITTEST(vmo_precommitted_map_test)
VM_UNITTEST(vmo_demand_paged_map_test)
VM_UNITTEST(vmo_dropped_ref_test)
VM_UNITTEST(vmo_remap_test)
VM_UNITTEST(vmo_double_remap_test)
VM_UNITTEST(vmo_read_write_smoke_test)
VM_UNITTEST(vmo_cache_test)
VM_UNITTEST(vmo_lookup_test)
VM_UNITTEST(vmo_lookup_slice_test)
VM_UNITTEST(vmo_lookup_clone_test)
VM_UNITTEST(vmo_clone_removes_write_test)
VM_UNITTEST(vmo_clones_of_compressed_pages_test)
VM_UNITTEST(vmo_clone_kernel_mapped_compressed_test)
VM_UNITTEST(vmo_move_pages_on_access_test)
VM_UNITTEST(vmo_eviction_hints_test)
VM_UNITTEST(vmo_always_need_evicts_loaned_test)
VM_UNITTEST(vmo_eviction_hints_clone_test)
VM_UNITTEST(vmo_unloan_test)
VM_UNITTEST(vmo_reclamation_test)
VM_UNITTEST(vmo_attribution_clones_test)
VM_UNITTEST(vmo_attribution_ops_test)
VM_UNITTEST(vmo_attribution_ops_contiguous_test)
VM_UNITTEST(vmo_attribution_pager_test)
VM_UNITTEST(vmo_attribution_dedup_test)
VM_UNITTEST(vmo_attribution_compression_test)
VM_UNITTEST(vmo_parent_merge_test)
VM_UNITTEST(vmo_lock_count_test)
VM_UNITTEST(vmo_discardable_states_test)
VM_UNITTEST(vmo_discard_test)
VM_UNITTEST(vmo_discard_failure_test)
VM_UNITTEST(vmo_discardable_counts_test)
VM_UNITTEST(vmo_lookup_compressed_pages_test)
VM_UNITTEST(vmo_write_does_not_commit_test)
VM_UNITTEST(vmo_dirty_pages_test)
VM_UNITTEST(vmo_dirty_pages_writeback_test)
VM_UNITTEST(vmo_dirty_pages_with_hints_test)
VM_UNITTEST(vmo_pinning_backlink_test)
VM_UNITTEST(vmo_pinning_dirty_state_test)
VM_UNITTEST(vmo_high_priority_dirty_state_test)
VM_UNITTEST(vmo_supply_compressed_pages_test)
VM_UNITTEST(vmo_zero_pinned_test)
VM_UNITTEST(vmo_pinned_wrapper_test)
VM_UNITTEST(vmo_dedup_dirty_test)
VM_UNITTEST(vmo_high_priority_reclaim_test)
VM_UNITTEST(vmo_snapshot_modified_test)
VM_UNITTEST(vmo_pin_race_loaned_test)
VM_UNITTEST(vmo_prefetch_compressed_pages_test)
VM_UNITTEST(vmo_skip_range_update_test)
VM_UNITTEST(vmo_user_stream_size_test)
VM_UNITTEST(vmo_loaned_high_priority_parent_test)
VM_UNITTEST(vmo_loaned_page_in_high_priority_test)
VM_UNITTEST(vmo_zero_marker_transfer_test)
VM_UNITTEST(vmo_pager_supply_test)
VM_UNITTEST(vmo_dedup_hidden_zero_page_test)
VM_UNITTEST(vmo_zero_partially_pinned_range_test)
VM_UNITTEST(vmo_apply_unmap_to_child_with_kernel_mapping_test)
VM_UNITTEST(vmo_compress_to_marker_pager_test)
VM_UNITTEST(vmo_compress_to_marker_anon_test)
VM_UNITTEST(vmo_always_pinned_with_no_pages_test)
VM_UNITTEST(vmo_lookup_readable_simple_test)
VM_UNITTEST(vmo_lookup_readable_clone_test)
VM_UNITTEST(vmo_get_page_offset_test)
VM_UNITTEST(vmo_ever_pinned_hidden_parent_creation_test)
VM_UNITTEST(vmo_ever_pinned_page_migration_test)
VM_UNITTEST(vmo_ever_pinned_parent_merge_test)
UNITTEST_END_TESTCASE(vmo_tests, "vmo", "VmObject tests")

}  // namespace

}  // namespace vm_unittest
