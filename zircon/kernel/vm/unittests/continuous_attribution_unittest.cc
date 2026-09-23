// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>

#include <ktl/source_location.h>
#include <ktl/utility.h>
#include <vm/continuous_attribution_tracker.h>

#include "test_helper.h"

namespace vm_unittest {

namespace {

bool should_skip_no_feature(ktl::source_location caller = ktl::source_location::current()) {
  if constexpr (EXPERIMENTAL_CONTINUOUS_PER_VMO_ATTRIBUTION_ENABLED) {
    return false;
  }
  printf("Skipping %s; no support for continuous attribution feature detected.\n",
         caller.function_name());
  return true;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the continuous attribution tracker supports a "stubbed out" state.
bool continuous_attribution_tracker_stub() {
  BEGIN_TEST;

  StubContinuousAttributionTracker tracker;

  tracker.Increment(1);
  tracker.Decrement(1);

  tracker.Increment(100);
  tracker.Decrement(100);

  tracker.Increment(100);
  tracker.Increment(100);
  tracker.Increment(100);

  tracker.Decrement(150);
  tracker.Decrement(150);

  // Overflow is okay.
  tracker.Decrement(3000);

  // Do not call stub FetchCurrent and FetchHwmAndReset methods, as these unconditionally panic.

  StubContinuousAttributionTracker assigned = ktl::move(tracker);
  StubContinuousAttributionTracker moved(ktl::move(assigned));
  ktl::ignore = moved;

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the initial state of the ContinuousAttributionTracker is zero.
bool continuous_attribution_tracker_create() {
  BEGIN_TEST;

  ContinuousAttributionTracker tracker;
  EXPECT_EQ(0u, tracker.FetchCurrent());
  EXPECT_EQ(0u, tracker.FetchHwmAndReset());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the move and assignment transfers data to the new ContinuousAttributionTracker
// object.
bool continuous_attribution_tracker_transfer() {
  BEGIN_TEST;

  ContinuousAttributionTracker tracker;

  tracker.Increment(5);

  EXPECT_EQ(5u, tracker.FetchCurrent());

  ContinuousAttributionTracker assigned_stats;
  assigned_stats = ktl::move(tracker);

  // The old one has nothing...
  EXPECT_EQ(0u, tracker.FetchCurrent());

  // but the new one has the data.
  EXPECT_EQ(5u, assigned_stats.FetchCurrent());

  ContinuousAttributionTracker constructed_stats(ktl::move(assigned_stats));

  // The old one has nothing...
  EXPECT_EQ(0u, assigned_stats.FetchCurrent());

  // but the new one has the data.
  EXPECT_EQ(5u, constructed_stats.FetchCurrent());

  // Only inspect the high-water mark down here because if we checked before it would have been
  // reset.
  EXPECT_EQ(5u, constructed_stats.FetchHwmAndReset());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the high-water mark accumulates values since last reset.
bool continuous_attribution_tracker_high_water_mark() {
  BEGIN_TEST;

  ContinuousAttributionTracker tracker;

  tracker.Increment(5);
  tracker.Decrement(5);

  // The high-water mark is reset by the below.
  EXPECT_EQ(5u, tracker.FetchHwmAndReset());

  tracker.Increment(2);
  tracker.Decrement(2);
  tracker.Increment(3);
  tracker.Decrement(2);
  tracker.Decrement(1);
  tracker.Increment(2);

  EXPECT_EQ(2u, tracker.FetchCurrent());

  // The high-water mark is 3 even though the current value is 2, since that was the highest since
  // last reset.
  EXPECT_EQ(3u, tracker.FetchHwmAndReset());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the continuous attribution tracker supports large counts.
bool continuous_attribution_tracker_extreme() {
  BEGIN_TEST;

  ContinuousAttributionTracker tracker;
  tracker.Increment(ktl::numeric_limits<uint32_t>::max());
  EXPECT_EQ(ktl::numeric_limits<uint32_t>::max(), tracker.FetchCurrent());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
bool continuous_attribution_tracker_populate_vmo() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 3 * kPageSize, &vmo);
  ASSERT_OK(status);

  {
    fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
    ASSERT_NONNULL(cow_pages);
    EXPECT_EQ(0u, cow_pages->DebugGetPopulatedSlotsCount());  // There is no content.
  }

  // Write a non-zero value to the first two pages.
  {
    fbl::AllocChecker ac;
    fbl::Vector<uint8_t> a;
    a.resize(2 * kPageSize, 42, &ac);
    ASSERT_TRUE(ac.check());
    EXPECT_EQ(ZX_OK, vmo->Write(a.data(), 0, a.size()));
  }

  {
    fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
    ASSERT_NONNULL(cow_pages);
    EXPECT_EQ(2u, cow_pages->DebugGetPopulatedSlotsCount());  // There are two populated pages.
  }

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the correct tracker is provided to the unidirectional clone and parent.
bool continuous_attribution_tracker_unidirectional_child() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }
  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_partially_committed_pager_vmo(3, /*committed_pages=*/2, /*trap_dirty=*/false,
                                               /*resizable=*/false, false, nullptr, &vmo));

  // Create a unidirectional clone.
  fbl::RefPtr<VmObject> child_vmo_no_paged;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, /*offset=*/0,
                             /*size=*/3 * kPageSize, /*copy_name=*/false, &child_vmo_no_paged));

  // Assert there is no hidden parent (true unidirectional)
  {
    fbl::RefPtr<VmCowPages> hidden_parent = vmo->DebugGetCowPages()->DebugGetParent();
    ASSERT_NULL(hidden_parent);
  }

  {
    fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
    ASSERT_NONNULL(cow_pages);

    // There are two pages committed in the parent.
    EXPECT_EQ(2u, cow_pages->DebugGetPopulatedSlotsCount());
  }

  {
    VmObjectPaged *child = DownCastVmObject<VmObjectPaged>(child_vmo_no_paged.get());
    ASSERT_NONNULL(child);
    fbl::RefPtr<VmCowPages> cow_pages = child->DebugGetCowPages();
    ASSERT_NONNULL(cow_pages);

    // There are no parent content markers in this hierarchy to track, as intended.
    EXPECT_EQ(0u, cow_pages->DebugGetPopulatedSlotsCount());
  }

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the correct tracker is provided to the bidirectional clone and parent.
bool continuous_attribution_tracker_bidirectional_child() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }
  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 3 * kPageSize, &vmo));

  // Write a non-zero value to the first two pages.
  {
    fbl::AllocChecker ac;
    fbl::Vector<uint8_t> a;
    a.resize(2 * kPageSize, 42, &ac);
    ASSERT_TRUE(ac.check());
    EXPECT_EQ(ZX_OK, vmo->Write(a.data(), 0, a.size()));
  }

  // Create a bidirectional clone.
  fbl::RefPtr<VmObject> child_vmo_no_paged;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                             /*size=*/3 * kPageSize, /*copy_name=*/false, &child_vmo_no_paged));

  // Assert there is a hidden parent (true bidirectional)
  fbl::RefPtr<VmCowPages> hidden_parent = vmo->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(hidden_parent);

  {
    fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
    ASSERT_NONNULL(cow_pages);

    EXPECT_EQ(2u, cow_pages->DebugGetPopulatedSlotsCount());
  }

  {
    VmObjectPaged *child = DownCastVmObject<VmObjectPaged>(child_vmo_no_paged.get());
    ASSERT_NONNULL(child);
    fbl::RefPtr<VmCowPages> cow_pages = child->DebugGetCowPages();
    ASSERT_NONNULL(cow_pages);

    EXPECT_EQ(2u, cow_pages->DebugGetPopulatedSlotsCount());
  }

  // There are two pages committed in the parent.
  EXPECT_EQ(2u, hidden_parent->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that zeroing an anonymous VMO decreases the populated slots count.
bool continuous_attribution_tracker_zero_anonymous() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 3 * kPageSize, &vmo));

  fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
  ASSERT_NONNULL(cow_pages);

  // Write a non-zero value to the first two pages.
  {
    fbl::AllocChecker ac;
    fbl::Vector<uint8_t> a;
    a.resize(2 * kPageSize, 42, &ac);
    ASSERT_TRUE(ac.check());
    EXPECT_EQ(ZX_OK, vmo->Write(a.data(), 0, a.size()));
  }

  EXPECT_EQ(2u, cow_pages->DebugGetPopulatedSlotsCount());

  // Clear out one page, so that afterwards the VMO will only have one populated page.
  {
    __UNINITIALIZED MultiPageRequest page_request;
    VmCowPages::DeferredOps deferred(cow_pages.get());
    Guard<CriticalMutex> guard{cow_pages->lock()};
    VmCowRange range(0, kPageSize);
    // Directly call the lower-level interface, as opposed to the method on VmObject. The
    // attribution for the higher-level method is incomplete.
    auto [status, zeroed_bytes] =
        cow_pages->ZeroPagesLocked(range, /*dirty_track=*/false, deferred, &page_request);
    EXPECT_EQ(kPageSize, zeroed_bytes);
    EXPECT_OK(status);
  }

  EXPECT_EQ(1u, cow_pages->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that zeroing a pager-backed VMO decreases the populated slots count.
bool continuous_attribution_tracker_zero_pager_backed() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_partially_committed_pager_vmo(3, /*committed_pages=*/2, /*trap_dirty=*/false,
                                               /*resizable=*/false, false, nullptr, &vmo));

  fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
  ASSERT_NONNULL(cow_pages);

  EXPECT_EQ(2u, cow_pages->DebugGetPopulatedSlotsCount());

  // Clear out one page, so that afterwards the VMO will only have one populated page.
  {
    __UNINITIALIZED MultiPageRequest page_request;
    VmCowPages::DeferredOps deferred(cow_pages.get());
    Guard<CriticalMutex> guard{cow_pages->lock()};
    VmCowRange range(0, kPageSize);
    // Directly call the lower-level interface, as opposed to the method on VmObject. The
    // attribution for the higher-level method is incomplete.
    auto [status, zeroed_bytes] =
        cow_pages->ZeroPagesLocked(range, /*dirty_track=*/false, deferred, &page_request);
    EXPECT_EQ(kPageSize, zeroed_bytes);
    EXPECT_OK(status);
  }

  EXPECT_EQ(1u, cow_pages->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that zeroing a child of a pager-backed VMO correctly updates the populated bytes count.
bool continuous_attribution_tracker_zero_pager_clone() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_partially_committed_pager_vmo(3, /*committed_pages=*/2, /*trap_dirty=*/false,
                                               /*resizable=*/false, false, nullptr, &vmo));

  fbl::RefPtr<VmObject> child_vmo_no_paged;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Modified, /*offset=*/0,
                             /*size=*/3 * kPageSize, /*copy_name=*/false, &child_vmo_no_paged));

  fbl::RefPtr<VmObjectPaged> child = DownCastVmObject<VmObjectPaged>(child_vmo_no_paged);
  ASSERT_NONNULL(child);

  fbl::RefPtr<VmCowPages> child_cow_pages = child->DebugGetCowPages();

  EXPECT_EQ(0u, child_cow_pages->DebugGetPopulatedSlotsCount());

  ASSERT_OK(child->CommitRange(0, 2 * kPageSize));

  EXPECT_EQ(2u, child_cow_pages->DebugGetPopulatedSlotsCount());

  // Clear out one page, so that afterwards the VMO will only have one populated page.
  {
    __UNINITIALIZED MultiPageRequest page_request;
    VmCowPages::DeferredOps deferred(child_cow_pages.get());
    Guard<CriticalMutex> guard{child_cow_pages->lock()};
    VmCowRange range(0, kPageSize);
    // Directly call the lower-level interface, as opposed to the method on VmObject. The
    // attribution for the higher-level method is incomplete.
    auto [status, zeroed_bytes] =
        child_cow_pages->ZeroPagesLocked(range, /*dirty_track=*/false, deferred, &page_request);
    EXPECT_EQ(kPageSize, zeroed_bytes);
    EXPECT_OK(status);
  }

  EXPECT_EQ(1u, child_cow_pages->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that removing a page from a hidden parent decrements populated bytes count.
bool continuous_attribution_tracker_require_move_page() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  // Set up a hidden parent with two pages and children with no content.

  fbl::RefPtr<VmObjectPaged> child1;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 3 * kPageSize, &child1));

  // Write a non-zero value to the first two pages.
  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> a;
  a.resize(2 * kPageSize, 42, &ac);
  ASSERT_TRUE(ac.check());
  EXPECT_EQ(ZX_OK, child1->Write(a.data(), 0, a.size()));

  // Create a bidirectional clone.
  fbl::RefPtr<VmObject> child2;
  ASSERT_OK(child1->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                                /*size=*/3 * kPageSize, /*copy_name=*/false, &child2));
  VmObjectPaged *child_paged = DownCastVmObject<VmObjectPaged>(child2.get());
  ASSERT_NONNULL(child_paged);
  fbl::RefPtr<VmCowPages> child2_cow = child_paged->DebugGetCowPages();

  // Assert there is a hidden parent (true bidirectional)
  fbl::RefPtr<VmCowPages> hidden_parent_cow = child2_cow->DebugGetParent();
  ASSERT_NONNULL(hidden_parent_cow);

  // Decrement the share count for the first page by making child1 copy-on-write the first page.
  child1->CommitRange(0, kPageSize);

  // Now the share count for the first page is just one in the hidden parent.

  // The content is attributed to both the parent and the child because the pages are resident in
  // the hidden parent and the child has parent content markers.
  EXPECT_EQ(2u, hidden_parent_cow->DebugGetPopulatedSlotsCount());
  EXPECT_EQ(2u, child2_cow->DebugGetPopulatedSlotsCount());

  child2->CommitRange(0, kPageSize);

  // The hidden parent's attribution count is decremented because it no longer has the page resident
  // (it has been moved to the child).
  EXPECT_EQ(1u, hidden_parent_cow->DebugGetPopulatedSlotsCount());
  EXPECT_EQ(2u, child2_cow->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the populated slots count is decremented during the removal of parent content markers
// in hidden VMOs.
bool continuous_attribution_tracker_hidden_no_parent_content() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  // We will create a bidirectional clone chain with a 1) hidden root, 2) a hidden child of that
  // root, and 3) a visible child of that child. When we create VMO #3, it will get a hidden parent
  // whose parent content markers must be deleted from its page list. Ensure that it also decrements
  // its populated slots count.

  fbl::RefPtr<VmObjectPaged> vmo1;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 3 * kPageSize, &vmo1));

  ASSERT_OK(vmo1->CommitRange(0, 2 * kPageSize));

  fbl::RefPtr<VmObject> vmo2_no_paged;
  ASSERT_OK(vmo1->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                              /*size=*/3 * kPageSize, /*copy_name=*/false, &vmo2_no_paged));
  fbl::RefPtr<VmObjectPaged> vmo2 = DownCastVmObject<VmObjectPaged>(vmo2_no_paged);
  ASSERT_NONNULL(vmo2);

  ASSERT_OK(vmo2->CommitRange(0, kPageSize));

  fbl::RefPtr<VmObject> vmo3_no_paged;
  ASSERT_OK(vmo2->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                              /*size=*/3 * kPageSize, /*copy_name=*/false, &vmo3_no_paged));
  fbl::RefPtr<VmObjectPaged> vmo3 = DownCastVmObject<VmObjectPaged>(vmo3_no_paged);
  ASSERT_NONNULL(vmo3);

  fbl::RefPtr<VmCowPages> parent = vmo3->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(parent);
  EXPECT_EQ(1u, parent->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the populated slots count is decremented when pages are evicted from VmCowPages.
bool continuous_attribution_tracker_reclaim_page() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  vm_page_t *committed_page;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_partially_committed_pager_vmo(3, /*committed_pages=*/1, /*trap_dirty=*/false,
                                               /*resizable=*/false, false, &committed_page, &vmo));

  EXPECT_EQ(1u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  // Only pages in an isolate queue can be reclaimed, so move the page there as the scanner would.
  pmm_page_queues()->MoveToReclaimDontNeed(committed_page);

  VmCowReclaimResult result = vmo->DebugGetCowPages()->ReclaimPage(
      committed_page, 0, VmCowPages::EvictionAction::FollowHint, nullptr);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(1u, result.value().num_pages);

  EXPECT_EQ(0u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the populated slots count is decremented when a loaned page is evicted from
// VmCowPages.
bool continuous_attribution_tracker_evict_loaned_page() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  const bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  // Provide a place for ReplacePageWithLoaned to borrow from.
  fbl::RefPtr<VmObjectPaged> contiguous_vmo;
  ASSERT_OK(VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, kPageSize, /*alignment_log2=*/0,
                                            &contiguous_vmo));
  ASSERT_OK(contiguous_vmo->DecommitRange(0, kPageSize));

  vm_page_t *committed_page;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_partially_committed_pager_vmo(3, /*committed_pages=*/1, /*trap_dirty=*/false,
                                               /*resizable=*/false, false, &committed_page, &vmo));

  fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();
  ASSERT_OK(cow_pages->ReplacePageWithLoaned(committed_page, /*offset=*/0));

  vm_page_t *loaned_page = vmo->DebugGetPage(0);
  ASSERT_NONNULL(loaned_page);
  ASSERT_TRUE(loaned_page->is_loaned());

  EXPECT_EQ(1u, cow_pages->DebugGetPopulatedSlotsCount());

  ASSERT_OK(cow_pages->EvictLoanedPage(loaned_page, 0));

  EXPECT_EQ(0u, cow_pages->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the populated slots count is decremented when compression clears a slot after finding a
// zero page.
bool continuous_attribution_tracker_zero_page_compression() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo));

  ASSERT_OK(vmo->CommitRange(0, kPageSize));

  EXPECT_EQ(1u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  vm_page_t *page = vmo->DebugGetPage(0);
  ASSERT_NONNULL(page);

  auto compression = Pmm::Node().GetPageCompression();
  if (!compression) {
    END_TEST;
  }

  auto compressor = compression->AcquireCompressor();
  ASSERT_OK(compressor.get().Arm());

  ASSERT_TRUE(vmo->DebugGetCowPages()
                  ->ReclaimPage(page, 0, VmCowPages::EvictionAction::FollowHint, &compressor.get())
                  .is_ok());

  EXPECT_EQ(0u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the populated slots count is decremented when zero page deduplication finds and removes
// a zero page.
bool continuous_attribution_tracker_zero_page_deduplication() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo));

  ASSERT_OK(vmo->CommitRange(0, kPageSize));

  EXPECT_EQ(1u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  vm_page_t *page = vmo->DebugGetPage(0);
  ASSERT_NONNULL(page);

  ASSERT_TRUE(vmo->DebugGetCowPages()->DedupZeroPage(page, 0));

  EXPECT_EQ(0u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that content removed from a hidden parent due to 0 share count is reflected in the populated
// slots count.
bool continuous_attribution_tracker_release_hidden() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 2 * kPageSize, &vmo));

  ASSERT_OK(vmo->CommitRange(0, 2 * kPageSize));

  fbl::RefPtr<VmObject> child;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                             /*size=*/2 * kPageSize, /*copy_name=*/false, &child));

  // Commit the pages in the child to remove its share count of the content.
  ASSERT_OK(child->CommitRange(0, 2 * kPageSize));

  fbl::RefPtr<VmCowPages> parent_cow = vmo->DebugGetCowPages()->DebugGetParent();
  EXPECT_EQ(2u, parent_cow->DebugGetPopulatedSlotsCount());

  // Remove the remaining share count for the first page, which will trigger the hidden parent to
  // remove the content from its local page list.
  {
    fbl::RefPtr<VmCowPages> vmo_cow = vmo->DebugGetCowPages();
    __UNINITIALIZED MultiPageRequest page_request;
    VmCowPages::DeferredOps deferred(vmo_cow.get());
    Guard<CriticalMutex> guard{vmo_cow->lock()};
    VmCowRange range(0, kPageSize);
    // ZeroPagesLocked calls DecrementCowContentShareCount, which is what we're interested in
    // tracking.
    auto [status, zeroed_bytes] =
        vmo_cow->ZeroPagesLocked(range, /*dirty_track=*/false, deferred, &page_request);
    EXPECT_EQ(kPageSize, zeroed_bytes);
    EXPECT_OK(status);
  }

  EXPECT_EQ(1u, parent_cow->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that DecommitRange decrements the populated slots count based on the number of populated
// pages it removes.
bool continuous_attribution_tracker_decommit_range() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 2 * kPageSize, &vmo));

  EXPECT_EQ(0u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  ASSERT_OK(vmo->CommitRange(0, 2 * kPageSize));

  EXPECT_EQ(2u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  ASSERT_OK(vmo->DecommitRange(0, kPageSize));

  EXPECT_EQ(1u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Check that DetachSource decrements the populated slots count for the removal of clean content.
bool continuous_attribution_tracker_detach_source() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(make_partially_committed_pager_vmo(3, /*committed_pages=*/2, /*trap_dirty=*/false,
                                               /*resizable=*/false, false, nullptr, &vmo));

  EXPECT_EQ(2u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  vmo->DetachSource();

  EXPECT_EQ(0u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the populated slots count is decremented when loaned pages are removed from a VMO as
// it's upgraded to being high priority.
bool continuous_attribution_tracker_remove_loaned_high_priority() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  const bool loaning_was_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  auto cleanup = fit::defer([loaning_was_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(loaning_was_enabled);
  });

  // Provide a place for ReplacePageWithLoaned to borrow from.
  fbl::RefPtr<VmObjectPaged> contiguous_vmo;
  ASSERT_OK(VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, kPageSize, /*alignment_log2=*/0,
                                            &contiguous_vmo));

  ASSERT_OK(contiguous_vmo->DecommitRange(0, kPageSize));

  fbl::RefPtr<VmObjectPaged> vmo;
  vm_page_t *before_page;
  ASSERT_OK(make_committed_pager_vmo(1, /*trap_dirty=*/false,
                                     /*resizable=*/false, &before_page, &vmo));

  fbl::RefPtr<VmCowPages> cow_pages = vmo->DebugGetCowPages();

  ASSERT_NONNULL(before_page);
  ASSERT_OK(cow_pages->ReplacePageWithLoaned(before_page, /*offset=*/0));

  EXPECT_EQ(1u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  change_vmo_high_priority_count(*vmo, 1);

  EXPECT_EQ(0u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  change_vmo_high_priority_count(*vmo, -1);

  EXPECT_EQ(0u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that failing to add a sequence of pages correctly updates the populated slots count on
// cleanup.
bool continuous_attribution_tracker_add_pages() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 4 * kPageSize, &vmo));
  fbl::RefPtr<VmCowPages> vmo_cow = vmo->DebugGetCowPages();

  EXPECT_EQ(0u, vmo_cow->DebugGetPopulatedSlotsCount());

  ASSERT_OK(vmo->CommitRange(kPageSize, kPageSize));

  EXPECT_EQ(1u, vmo_cow->DebugGetPopulatedSlotsCount());

  {
    VmCowPages::DeferredOps deferred(vmo_cow.get());
    Guard<CriticalMutex> guard{vmo_cow->lock()};

    VmPageDoublyLinkedList list;
    const size_t count = 3;
    ASSERT_OK(pmm_alloc_pages(count, 0, &list));
    auto cleanup = fit::defer([&]() { pmm_free(&list); });

    EXPECT_EQ(ZX_ERR_ALREADY_EXISTS,
              vmo_cow->AddNewPagesLocked(0, &list, VmCowPages::CanOverwriteSlot::Empty,
                                         /*zero=*/true, &deferred));
  }

  EXPECT_EQ(1u, vmo_cow->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Regression test for https://fxbug.dev/483815044. Transfer a spurious parent content marker to a
// child, and check that it can be decommitted successfully.
bool continuous_attribution_tracker_merge_spurious_parent_content() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> child;
  // Make |child|'s only slot hold a parent content marker.
  {
    fbl::RefPtr<VmObjectPaged> vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 2 * kPageSize, &vmo));

    // Ensure we get a bidirectional clone.
    ASSERT_OK(vmo->CommitRange(0, 2 * kPageSize));

    fbl::RefPtr<VmObject> child_no_paged;
    ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize,
                               /*copy_name=*/false, &child_no_paged));
    child = DownCastVmObject<VmObjectPaged>(child_no_paged);

    fbl::RefPtr<VmCowPages> hidden_parent = vmo->DebugGetCowPages()->DebugGetParent();
    ASSERT_NONNULL(hidden_parent);

    vm_page_t *page = hidden_parent->DebugGetPage(0);
    VmCompression *compression = Pmm::Node().GetPageCompression();
    if (!compression) {
      END_TEST;
    }

    auto compressor = compression->AcquireCompressor();
    ASSERT_OK(compressor.get().Arm());

    auto result = hidden_parent->ReclaimPage(page, 0, VmCowPages::EvictionAction::IgnoreHint,
                                             &compressor.get());
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result.value().num_pages, 1u);
    // The content was removed (we actually compressed the zero page).
    EXPECT_TRUE(hidden_parent->DebugIsEmpty(0));
    // There is now a spurious parent content marker.
    EXPECT_TRUE(child->DebugGetCowPages()->DebugIsParentContent(0));

    // TODO(https://fxbug.dev/504652289): The continuous attribution system and the populated slots
    // count should eventually agree here that there are zero populated slots and zero populated
    // bytes.
    EXPECT_EQ(1u, child->DebugGetCowPages()->DebugGetPopulatedSlotsCount());
    EXPECT_EQ(0u, child->GetAttributedMemory().total_bytes());

    // Let's drop |vmo| to trigger the hidden parent to merge into |child|. That will allow
    // |child| to have no parent while still having a spurious parent content marker.
  }

  // There is 1 parent content marker.
  EXPECT_TRUE(child->DebugGetCowPages()->DebugIsParentContent(0));
  EXPECT_EQ(1u, child->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  EXPECT_OK(child->DecommitRange(0, kPageSize));

  EXPECT_TRUE(child->DebugGetCowPages()->DebugIsEmpty(0));
  EXPECT_EQ(0u, child->DebugGetCowPages()->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that the dead transition of a VMO correctly redistributes content between hidden parents and
// children.
bool continuous_attribution_tracker_merge_into_child() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  // Construct a copy-on-write hierarchy, and selectively destroy one leaf.

  fbl::RefPtr<VmCowPages> a_cow;
  fbl::RefPtr<VmObjectPaged> b;
  fbl::RefPtr<VmObjectPaged> c;
  fbl::RefPtr<VmCowPages> b_c_hidden_parent;
  fbl::RefPtr<VmCowPages> a_hidden_parent;
  {
    fbl::RefPtr<VmObjectPaged> a;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 3 * kPageSize, &a));

    ASSERT_OK(a->CommitRange(0, kPageSize));

    {
      fbl::RefPtr<VmObject> b_no_paged;
      ASSERT_OK(a->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                               /*size=*/3 * kPageSize, /*copy_name=*/false, &b_no_paged));
      ASSERT_NONNULL(b_no_paged);
      b = DownCastVmObject<VmObjectPaged>(b_no_paged);
      ASSERT_NONNULL(b);
    }

    // Ensure that we get an extra level in the hierarchy.
    ASSERT_OK(b->CommitRange(kPageSize, kPageSize));

    {
      fbl::RefPtr<VmObject> c_no_paged;
      ASSERT_OK(b->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                               /*size=*/3 * kPageSize, /*copy_name=*/false, &c_no_paged));
      ASSERT_NONNULL(c_no_paged);
      c = DownCastVmObject<VmObjectPaged>(c_no_paged);
      ASSERT_NONNULL(c);
    }

    // b and c have the same parent
    fbl::RefPtr<VmCowPages> b_cow = b->DebugGetCowPages();
    fbl::RefPtr<VmCowPages> c_cow = c->DebugGetCowPages();
    EXPECT_EQ(b_cow->DebugGetParent().get(), c_cow->DebugGetParent().get());
    b_c_hidden_parent = b_cow->DebugGetParent();
    ASSERT_NONNULL(b_c_hidden_parent);

    // b and c's parent's parent is the same as a's parent
    a_cow = a->DebugGetCowPages();
    EXPECT_EQ(b_c_hidden_parent->DebugGetParent().get(), a_cow->DebugGetParent().get());
    a_hidden_parent = a_cow->DebugGetParent();

    // Watch |b_c_hidden_parent|'s and |a_hidden_parent| populated slots as |a| dies.
    EXPECT_EQ(1u, b_c_hidden_parent->DebugGetPopulatedSlotsCount());
    EXPECT_EQ(1u, a_hidden_parent->DebugGetPopulatedSlotsCount());
  }
  EXPECT_EQ(2u, b_c_hidden_parent->DebugGetPopulatedSlotsCount());
  EXPECT_EQ(0u, a_hidden_parent->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that ReleaseOwnedPagesRangeLocked correctly updates the populated slot count when it can
// work entirely within the local page list.
bool continuous_attribution_tracker_release_owned_self() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, 10 * kPageSize, &vmo));
  fbl::RefPtr<VmCowPages> vmo_cow = vmo->DebugGetCowPages();

  ASSERT_OK(vmo->CommitRange(0, 10 * kPageSize));

  EXPECT_EQ(10u, vmo_cow->DebugGetPopulatedSlotsCount());

  // Call ReleaseOwnedPagesRangeLocked indirectly through Resize.
  ASSERT_OK(vmo->Resize(4 * kPageSize));

  EXPECT_EQ(4u, vmo_cow->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that ReleaseOwnedPagesRangeLocked correctly updates the populated slot count in hidden
// parents.
bool continuous_attribution_tracker_release_owned_parent() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> a;
  ASSERT_OK(
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, 2 * kPageSize, &a));

  ASSERT_OK(a->CommitRange(0, 2 * kPageSize));

  fbl::RefPtr<VmObject> b_no_paged;
  ASSERT_OK(a->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                           /*size=*/2 * kPageSize, /*copy_name=*/false, &b_no_paged));
  ASSERT_NONNULL(b_no_paged);
  fbl::RefPtr<VmObjectPaged> b = DownCastVmObject<VmObjectPaged>(b_no_paged);
  ASSERT_NONNULL(b);

  // Decrement the share count of the parent content.
  ASSERT_OK(b->CommitRange(kPageSize, kPageSize));

  fbl::RefPtr<VmCowPages> a_cow = a->DebugGetCowPages();
  fbl::RefPtr<VmCowPages> parent_cow = a_cow->DebugGetParent();
  ASSERT_NONNULL(parent_cow);

  EXPECT_EQ(2u, a_cow->DebugGetPopulatedSlotsCount());
  EXPECT_EQ(2u, parent_cow->DebugGetPopulatedSlotsCount());

  // Call ReleaseOwnedPagesRangeLocked indirectly through Resize.
  ASSERT_OK(a->Resize(kPageSize));

  EXPECT_EQ(1u, a_cow->DebugGetPopulatedSlotsCount());
  EXPECT_EQ(1u, parent_cow->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that TakePages decrements the populated slots count in response to content it removes from
// the page list.
bool continuous_attribution_tracker_take_pages() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 3 * kPageSize, &vmo));
  fbl::RefPtr<VmCowPages> vmo_cow = vmo->DebugGetCowPages();

  ASSERT_OK(vmo->CommitRange(0, 3 * kPageSize));

  EXPECT_EQ(3u, vmo_cow->DebugGetPopulatedSlotsCount());

  VmPageSpliceList page_list;
  ASSERT_OK(vmo->TakePages(0, 3 * kPageSize, &page_list));

  EXPECT_EQ(0u, vmo_cow->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that TakePages decrements the populated slots count in response to content it removes from
// the page list if there is a parent.
bool continuous_attribution_tracker_take_pages_parent() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  // This is a separate test from continuous_attribution_tracker_take_pages because it triggers an
  // independent branch in VmCowPages::TakePages.

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> a;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 3 * kPageSize, &a));

  ASSERT_OK(a->CommitRange(0, 3 * kPageSize));

  fbl::RefPtr<VmObject> b;
  ASSERT_OK(a->CreateClone(Resizability::NonResizable, SnapshotType::Full, /*offset=*/0,
                           /*size=*/3 * kPageSize, /*copy_name=*/false, &b));

  ASSERT_OK(a->CommitRange(0, 3 * kPageSize));

  fbl::RefPtr<VmCowPages> a_cow = a->DebugGetCowPages();

  EXPECT_EQ(3u, a_cow->DebugGetPopulatedSlotsCount());

  VmPageSpliceList page_list;
  ASSERT_OK(a->TakePages(0, 3 * kPageSize, &page_list));

  EXPECT_EQ(0u, a_cow->DebugGetPopulatedSlotsCount());

  END_TEST;
}

// TODO(https://fxbug.dev/549880387): Delete in favor of the Rust version in
// continuous_attribution_unittest.rs.
// Test that deduplicating a zero page in a hidden parent creates a persistent
// disconnect between the child's attribution tracker and GetAttributedMemory.
//
// TODO(https://fxbug.dev/504652289): Update this test when the disconnect has been repaired.
bool continuous_attribution_tracker_dedup_hidden_parent_disconnect() {
  BEGIN_TEST;

  if (should_skip_no_feature()) {
    END_TEST;
  }

  AutoVmScannerDisable disable_scanner;

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo));

  // Ensure there is a resident (zero) page.
  ASSERT_OK(vmo->CommitRange(0, kPageSize));

  fbl::RefPtr<VmObject> clone_no_paged;
  ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, false,
                             &clone_no_paged));
  fbl::RefPtr<VmObjectPaged> clone = DownCastVmObject<VmObjectPaged>(clone_no_paged);

  fbl::RefPtr<VmCowPages> hidden_parent = vmo->DebugGetCowPages()->DebugGetParent();
  ASSERT_NONNULL(hidden_parent);

  EXPECT_EQ(1u, hidden_parent->DebugGetPopulatedSlotsCount());
  EXPECT_EQ(kPageSize, vmo->GetAttributedMemory().total_bytes());

  vm_page_t *page = hidden_parent->DebugGetPage(0);
  ASSERT_NONNULL(page);
  ASSERT_TRUE(hidden_parent->DedupZeroPage(page, 0));

  // Note the disconnect between the continuously tracked populated slots count and
  // GetAttributedMemory.
  EXPECT_EQ(1u, vmo->DebugGetCowPages()->DebugGetPopulatedSlotsCount());
  EXPECT_EQ(0u, vmo->GetAttributedMemory().total_bytes());

  END_TEST;
}

UNITTEST_START_TESTCASE(continuous_attribution_tests)
VM_UNITTEST(continuous_attribution_tracker_stub)
VM_UNITTEST(continuous_attribution_tracker_create)
VM_UNITTEST(continuous_attribution_tracker_transfer)
VM_UNITTEST(continuous_attribution_tracker_high_water_mark)
VM_UNITTEST(continuous_attribution_tracker_extreme)
VM_UNITTEST(continuous_attribution_tracker_populate_vmo)
VM_UNITTEST(continuous_attribution_tracker_unidirectional_child)
VM_UNITTEST(continuous_attribution_tracker_bidirectional_child)
VM_UNITTEST(continuous_attribution_tracker_zero_anonymous)
VM_UNITTEST(continuous_attribution_tracker_zero_pager_backed)
VM_UNITTEST(continuous_attribution_tracker_zero_pager_clone)
VM_UNITTEST(continuous_attribution_tracker_require_move_page)
VM_UNITTEST(continuous_attribution_tracker_hidden_no_parent_content)
VM_UNITTEST(continuous_attribution_tracker_reclaim_page)
VM_UNITTEST(continuous_attribution_tracker_evict_loaned_page)
VM_UNITTEST(continuous_attribution_tracker_zero_page_compression)
VM_UNITTEST(continuous_attribution_tracker_zero_page_deduplication)
VM_UNITTEST(continuous_attribution_tracker_release_hidden)
VM_UNITTEST(continuous_attribution_tracker_decommit_range)
VM_UNITTEST(continuous_attribution_tracker_detach_source)
VM_UNITTEST(continuous_attribution_tracker_remove_loaned_high_priority)
VM_UNITTEST(continuous_attribution_tracker_add_pages)
VM_UNITTEST(continuous_attribution_tracker_merge_spurious_parent_content)
VM_UNITTEST(continuous_attribution_tracker_merge_into_child)
VM_UNITTEST(continuous_attribution_tracker_release_owned_self)
VM_UNITTEST(continuous_attribution_tracker_release_owned_parent)
VM_UNITTEST(continuous_attribution_tracker_take_pages)
VM_UNITTEST(continuous_attribution_tracker_take_pages_parent)
VM_UNITTEST(continuous_attribution_tracker_dedup_hidden_parent_disconnect)
UNITTEST_END_TESTCASE(continuous_attribution_tests, "continuous_attribution",
                      "Tests for populated bytes high-water mark")

}  // namespace
}  // namespace vm_unittest
