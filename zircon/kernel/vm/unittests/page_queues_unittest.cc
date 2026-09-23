// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <vm/page_queues.h>

#include "test_helper.h"

namespace vm_unittest {

static void InitializeTestPage(vm_page_t* page) {
  DEBUG_ASSERT(page);
  DEBUG_ASSERT(!page->queue_node.InContainer());
  // Pages are constructed in the FREE state
  DEBUG_ASSERT(page->state() == vm_page_state::FREE);
  page->set_state(vm_page_state::OBJECT);
  page->object.share_count = 0;
  page->object.pin_count = 0;
  page->object.always_need = 0;
  page->object.dirty_state = uint8_t(VmCowPages::DirtyState::Untracked);
  page->object.set_object(nullptr);
  page->object.set_page_offset(0);
  page->object.get_page_queue_ref().store(0, ktl::memory_order_relaxed);
}

static bool pq_add_remove() {
  BEGIN_TEST;

  PageQueues pq;

  // Pretend we have an allocated page
  vm_page_t test_page = {};
  InitializeTestPage(&test_page);

  // Need a VMO to claim our pages are in
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(0, 0, kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Put the page in each queue and make sure it shows up
  pq.SetWired(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsWired(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.wired = 1}));

  pq.Remove(&test_page);
  EXPECT_FALSE(pq.DebugPageIsWired(&test_page));
  EXPECT_FALSE(pq.DebugPageIsAnonymous(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  pq.SetAnonymous(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsAnonymous(&test_page));
  if (pq.ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.anonymous = 1}));
  } else {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));
  }

  pq.Remove(&test_page);
  EXPECT_FALSE(pq.DebugPageIsAnonymous(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  // Need a pager VMO to claim our page is in.
  status = make_uncommitted_pager_vmo(1, false, false, &vmo);
  ASSERT_OK(status);

  pq.SetReclaim(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));

  pq.Remove(&test_page);
  EXPECT_FALSE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  pq.SetPagerBackedDirty(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.pager_backed_dirty = 1}));

  pq.Remove(&test_page);
  EXPECT_FALSE(pq.DebugPageIsPagerBackedDirty(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  END_TEST;
}

static bool pq_move_queues() {
  BEGIN_TEST;

  PageQueues pq;

  // Pretend we have an allocated page
  vm_page_t test_page = {};
  InitializeTestPage(&test_page);

  // Need a VMO to claim our pages are in
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(0, 0, kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Move the page between queues.
  pq.SetWired(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsWired(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.wired = 1}));

  pq.MoveToAnonymous(&test_page);
  EXPECT_FALSE(pq.DebugPageIsWired(&test_page));
  EXPECT_TRUE(pq.DebugPageIsAnonymous(&test_page));
  if (pq.ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.anonymous = 1}));
  } else {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));
  }
  pq.Remove(&test_page);

  // Now try some pager backed queues.
  status = make_uncommitted_pager_vmo(1, false, false, &vmo);
  ASSERT_OK(status);

  pq.SetReclaim(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));

  pq.MoveToPagerBackedDirty(&test_page);
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.pager_backed_dirty = 1}));

  pq.MoveToReclaim(&test_page);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));

  pq.MoveToReclaimDontNeed(&test_page);
  EXPECT_FALSE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim_isolate = 1}));

  // Verify that the DontNeed page is first in line for eviction.
  auto backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  EXPECT_TRUE(backlink != ktl::nullopt && backlink->page == &test_page);

  pq.MoveToWired(&test_page);
  EXPECT_FALSE(pq.DebugPageIsReclaimIsolate(&test_page));
  EXPECT_FALSE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.DebugPageIsWired(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.wired = 1}));

  pq.Remove(&test_page);
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  END_TEST;
}

static bool pq_move_self_queue() {
  BEGIN_TEST;

  PageQueues pq;

  // Pretend we have an allocated page
  vm_page_t test_page = {};
  InitializeTestPage(&test_page);

  // Need a VMO to claim our pages are in
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(0, 0, kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Move the page into the queue it is already in.
  pq.SetWired(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsWired(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.wired = 1}));

  pq.MoveToWired(&test_page);
  EXPECT_TRUE(pq.DebugPageIsWired(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.wired = 1}));

  pq.Remove(&test_page);
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  pq.SetAnonymous(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsAnonymous(&test_page));
  if (pq.ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.anonymous = 1}));
  } else {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));
  }

  pq.MoveToAnonymous(&test_page);
  EXPECT_TRUE(pq.DebugPageIsAnonymous(&test_page));
  if (pq.ReclaimIsOnlyPagerBacked()) {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.anonymous = 1}));
  } else {
    EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));
  }

  pq.Remove(&test_page);
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  // Now try some pager backed queues.
  status = make_uncommitted_pager_vmo(1, false, false, &vmo);
  ASSERT_OK(status);

  pq.SetReclaim(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));

  pq.MoveToReclaim(&test_page);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));

  pq.Remove(&test_page);
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  pq.SetPagerBackedDirty(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.pager_backed_dirty = 1}));

  pq.MoveToPagerBackedDirty(&test_page);
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&test_page));
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.pager_backed_dirty = 1}));

  pq.Remove(&test_page);
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){}));

  END_TEST;
}

static bool pq_rotate_queue() {
  BEGIN_TEST;

  PageQueues pq;

  pq.SetActiveRatioMultiplier(0);
  pq.StartThreads(0, ZX_TIME_INFINITE);

  // Pretend we have a few allocated pages.
  vm_page_t wired_page = {};
  vm_page_t clean_pager_page = {};
  vm_page_t dirty_pager_page = {};
  InitializeTestPage(&wired_page);
  InitializeTestPage(&clean_pager_page);
  InitializeTestPage(&dirty_pager_page);

  // Need a VMO to claim our pages are in.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = make_uncommitted_pager_vmo(1, false, false, &vmo);
  ASSERT_OK(status);

  // Put the pages in and validate initial state.
  pq.SetWired(&wired_page, vmo->DebugGetCowPages().get(), 0);
  pq.SetReclaim(&clean_pager_page, vmo->DebugGetCowPages().get(), 0);
  pq.SetPagerBackedDirty(&dirty_pager_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsWired(&wired_page));
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&dirty_pager_page));
  size_t queue;
  EXPECT_TRUE(pq.DebugPageIsReclaim(&clean_pager_page, &queue));
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {1, 0, 0, 0, 0, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){1, 0}));
  EXPECT_EQ(queue, 0u);

  // Gradually rotate the queue.
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.DebugPageIsWired(&wired_page));
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&dirty_pager_page));
  EXPECT_TRUE(pq.DebugPageIsReclaim(&clean_pager_page, &queue));
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 1, 0, 0, 0, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){1, 0}));
  EXPECT_EQ(queue, 1u);

  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 0, 1, 0, 0, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){0, 1}));
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 0, 0, 1, 0, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){0, 1}));
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 0, 0, 0, 1, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 0, 0, 0, 0, 1, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 0, 0, 0, 0, 0, 1, 0}, .pager_backed_dirty = 1, .wired = 1}));
  pq.RotateReclaimQueues();
  // Further rotations might cause the page to be visible in the same queue, or the isolate,
  // depending on whether the lru processing already ran in preparation of the next aging event.
  const PageQueues::Counts counts_last = (PageQueues::Counts){
      .reclaim = {0, 0, 0, 0, 0, 0, 0, 1}, .pager_backed_dirty = 1, .wired = 1};
  const PageQueues::Counts counts_isolate =
      (PageQueues::Counts){.reclaim = {0, 0, 0, 0, 0, 0, 0, 0},
                           .reclaim_isolate = 1,
                           .pager_backed_dirty = 1,
                           .wired = 1};
  PageQueues::Counts counts = pq.QueueCounts();
  EXPECT_TRUE(counts == counts_last || counts == counts_isolate);

  // Further rotations should not move the page.
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.DebugPageIsWired(&wired_page));
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&dirty_pager_page));
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&clean_pager_page));
  counts = pq.QueueCounts();
  EXPECT_TRUE(counts == counts_isolate);
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){0, 1}));

  // Moving the page should bring it back to the first queue.
  pq.MoveToReclaim(&clean_pager_page);
  EXPECT_TRUE(pq.DebugPageIsWired(&wired_page));
  EXPECT_TRUE(pq.DebugPageIsPagerBackedDirty(&dirty_pager_page));
  EXPECT_TRUE(pq.DebugPageIsReclaim(&clean_pager_page));
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {1, 0, 0, 0, 0, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){1, 0}));

  // Just double check two rotations.
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 1, 0, 0, 0, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){1, 0}));
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){
                  .reclaim = {0, 0, 1, 0, 0, 0, 0, 0}, .pager_backed_dirty = 1, .wired = 1}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){0, 1}));

  pq.Remove(&wired_page);
  pq.Remove(&clean_pager_page);
  pq.Remove(&dirty_pager_page);

  END_TEST;
}

static bool pq_toggle_dont_need_queue() {
  BEGIN_TEST;

  PageQueues pq;

  pq.SetActiveRatioMultiplier(0);
  pq.StartThreads(0, ZX_TIME_INFINITE);

  // Pretend we have a couple of allocated pager-backed pages.
  vm_page_t page1 = {};
  vm_page_t page2 = {};
  InitializeTestPage(&page1);
  InitializeTestPage(&page2);

  // Need a VMO to claim our pager backed pages are in.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = make_uncommitted_pager_vmo(2, false, false, &vmo);
  ASSERT_OK(status);

  // Put the pages in and validate initial state.
  pq.SetReclaim(&page1, vmo->DebugGetCowPages().get(), 0);
  size_t queue;
  EXPECT_TRUE(pq.DebugPageIsReclaim(&page1, &queue));
  EXPECT_EQ(queue, 0u);
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {1, 0, 0, 0, 0, 0, 0, 0}}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){1, 0}));
  pq.SetReclaim(&page2, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&page2, &queue));
  EXPECT_EQ(queue, 0u);
  EXPECT_TRUE(pq.QueueCounts() == ((PageQueues::Counts){.reclaim = {2, 0, 0, 0, 0, 0, 0, 0}}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){2, 0}));

  // Move the pages to the DontNeed queue.
  pq.MoveToReclaimDontNeed(&page1);
  pq.MoveToReclaimDontNeed(&page2);
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&page1));
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&page2));
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){.reclaim = {0, 0, 0, 0, 0, 0, 0, 0}, .reclaim_isolate = 2}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){0, 2}));

  // Rotate the queues. This should also process the DontNeed queue.
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&page1));
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&page2));
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){.reclaim = {0, 0, 0, 0, 0, 0, 0, 0}, .reclaim_isolate = 2}));
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){0, 2}));

  // Simulate access for one of the pages. Then rotate the queues again. This should move the
  // accessed page1 out of the DontNeed queue to MRU+1 (as we've rotated the queues after access).
  pq.MarkAccessed(&page1);
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.DebugPageIsReclaim(&page1, &queue));
  EXPECT_EQ(queue, 1u);
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&page2));
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){.reclaim = {0, 1, 0, 0, 0, 0, 0, 0}, .reclaim_isolate = 1}));
  // Two active queues by default, so page1 is still considered active.
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){1, 1}));

  // Rotate the queues again. The page accessed above should move to the next pager-backed queue.
  pq.RotateReclaimQueues();
  EXPECT_TRUE(pq.DebugPageIsReclaim(&page1, &queue));
  EXPECT_EQ(queue, 2u);
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&page2));
  EXPECT_TRUE(pq.QueueCounts() ==
              ((PageQueues::Counts){.reclaim = {0, 0, 1, 0, 0, 0, 0, 0}, .reclaim_isolate = 1}));
  // page1 has now moved on past the two active queues, so it now counts as inactive.
  EXPECT_TRUE(pq.GetActiveInactiveCounts() == ((PageQueues::ActiveInactiveCounts){0, 2}));

  pq.Remove(&page1);
  pq.Remove(&page2);

  END_TEST;
}

static bool pq_multiple_queues_fifo_order() {
  BEGIN_TEST;

  PageQueues pq;

  pq.SetActiveRatioMultiplier(0);
  pq.StartThreads(0, ZX_TIME_INFINITE);

  vm_page_t old_page = {};
  vm_page_t new_page = {};
  InitializeTestPage(&old_page);
  InitializeTestPage(&new_page);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = make_uncommitted_pager_vmo(2, false, false, &vmo);
  ASSERT_OK(status);

  // Set up old_page and rotate once to push it to the next queue.
  pq.SetReclaim(&old_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&old_page));
  pq.RotateReclaimQueues();

  // Set up new_page.
  pq.SetReclaim(&new_page, vmo->DebugGetCowPages().get(), 1);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&new_page));

  // Rotate queues until both pages reach the isolate queue. Since new_page is one
  // generation behind old_page, waiting for new_page ensures old_page is already there.
  size_t rotations = 0;
  while (!pq.DebugPageIsReclaimIsolate(&new_page) && rotations < 20) {
    pq.RotateReclaimQueues();
    rotations++;
  }
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&old_page));
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&new_page));

  // Peek the isolate queue. Since old_page was aged first (older bucket), it should
  // be peeked first (FIFO ordering at the bucket level).
  auto backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(backlink.has_value());
  EXPECT_EQ(backlink->page, &old_page);

  // Remove old_page to verify that the next page peeked is new_page.
  pq.Remove(&old_page);
  auto next_backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(next_backlink.has_value());
  EXPECT_EQ(next_backlink->page, &new_page);

  pq.Remove(&new_page);

  END_TEST;
}

static bool pq_single_queue_fifo_order() {
  BEGIN_TEST;

  PageQueues pq;

  vm_page_t old_page = {};
  vm_page_t new_page = {};
  InitializeTestPage(&old_page);
  InitializeTestPage(&new_page);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = make_uncommitted_pager_vmo(2, false, false, &vmo);
  ASSERT_OK(status);

  pq.SetReclaim(&old_page, vmo->DebugGetCowPages().get(), 0);
  pq.SetReclaim(&new_page, vmo->DebugGetCowPages().get(), 1);

  size_t queue_a = 0, queue_b = 0;
  EXPECT_TRUE(pq.DebugPageIsReclaim(&old_page, &queue_a));
  EXPECT_TRUE(pq.DebugPageIsReclaim(&new_page, &queue_b));
  EXPECT_EQ(queue_a, queue_b);  // Verify they are in the same queue (same generation)

  // Age pages until they reach the LRU queue.
  for (size_t i = 0; i < PageQueues::kNumReclaim - 1; i++) {
    pq.RotateReclaimQueues();
  }

  // Peek the isolate list using the public PeekIsolate method.
  // We expect pages to be processed in age order (oldest first, i.e., old_page before new_page).
  // Since they are added to the tail of the isolate list, the oldest page (old_page) should be at
  // the head of the isolate list.
  auto backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(backlink.has_value());
  EXPECT_EQ(backlink->page, &old_page);

  pq.Remove(&old_page);
  auto next_backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(next_backlink.has_value());
  EXPECT_EQ(next_backlink->page, &new_page);

  pq.Remove(&new_page);

  END_TEST;
}

static bool pq_isolate_dont_need_fifo_order() {
  BEGIN_TEST;

  PageQueues pq;

  vm_page_t old_page = {};
  vm_page_t new_page = {};
  InitializeTestPage(&old_page);
  InitializeTestPage(&new_page);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = make_uncommitted_pager_vmo(2, false, false, &vmo);
  ASSERT_OK(status);

  // Set up both pages and mark them "Don't Need".
  // old_page is marked first, then new_page. Use different offsets to represent distinct pages.
  pq.SetReclaim(&old_page, vmo->DebugGetCowPages().get(), 0);
  pq.MoveToReclaimDontNeed(&old_page);
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&old_page));

  pq.SetReclaim(&new_page, vmo->DebugGetCowPages().get(), 1);
  pq.MoveToReclaimDontNeed(&new_page);
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&new_page));

  // Peek the isolate queue. It should return old_page first (FIFO for Don't Need).
  auto backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(backlink.has_value());
  EXPECT_EQ(backlink->page, &old_page);

  // Remove old_page to verify that the next page peeked is new_page.
  pq.Remove(&old_page);
  auto next_backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(next_backlink.has_value());
  EXPECT_EQ(next_backlink->page, &new_page);

  pq.Remove(&new_page);

  END_TEST;
}

static bool pq_isolate_queues_priority() {
  BEGIN_TEST;

  PageQueues pq;

  pq.SetActiveRatioMultiplier(0);
  pq.StartThreads(0, ZX_TIME_INFINITE);

  vm_page_t aged_page = {};
  vm_page_t dont_need_page = {};
  InitializeTestPage(&aged_page);
  InitializeTestPage(&dont_need_page);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = make_uncommitted_pager_vmo(2, false, false, &vmo);
  ASSERT_OK(status);

  // Set up the first page and rotate queues until it reaches the isolate queue.
  // Aged pages are placed in the standard isolate queue (isolate_queues_[1]).
  pq.SetReclaim(&aged_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_TRUE(pq.DebugPageIsReclaim(&aged_page));

  size_t rotations = 0;
  while (!pq.DebugPageIsReclaimIsolate(&aged_page) && rotations < 20) {
    pq.RotateReclaimQueues();
    rotations++;
  }
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&aged_page));

  // Set up the second page and mark it "Don't Need".
  // "Don't Need" pages are placed in the high-priority isolate queue (isolate_queues_[0]).
  pq.SetReclaim(&dont_need_page, vmo->DebugGetCowPages().get(), 0);
  pq.MoveToReclaimDontNeed(&dont_need_page);
  EXPECT_TRUE(pq.DebugPageIsReclaimIsolate(&dont_need_page));

  // Peek the isolate queue. PeekIsolate checks the high-priority queue (index 0)
  // before the standard queue (index 1). Thus, the "Don't Need" page is returned first,
  // verifying it is prioritized for eviction over the aged page.
  auto backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(backlink.has_value());
  EXPECT_EQ(backlink->page, &dont_need_page);

  // Remove the high-priority "Don't Need" page to verify that the next page
  // peeked from the isolate queue is the standard aged page.
  pq.Remove(&dont_need_page);
  auto next_backlink = pq.PeekIsolate(PageQueues::kNumReclaim - 1);
  ASSERT_TRUE(next_backlink.has_value());
  EXPECT_EQ(next_backlink->page, &aged_page);

  pq.Remove(&aged_page);

  END_TEST;
}

static bool pq_is_page_reclaimable() {
  BEGIN_TEST;

  PageQueues pq;

  vm_page_t test_page = {};
  InitializeTestPage(&test_page);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = make_uncommitted_pager_vmo(1, false, false, &vmo);
  ASSERT_OK(status);

  // Only pages in the isolate queue sholud be considered relcaimable.
  pq.SetReclaim(&test_page, vmo->DebugGetCowPages().get(), 0);
  EXPECT_FALSE(pq.IsPageReclaimable(&test_page));

  // Moving the page to the "Don't Need" queue should move to isolate.
  pq.MoveToReclaimDontNeed(&test_page);
  EXPECT_TRUE(pq.IsPageReclaimable(&test_page));

  pq.MoveToReclaim(&test_page);
  EXPECT_FALSE(pq.IsPageReclaimable(&test_page));

  pq.Remove(&test_page);

  END_TEST;
}

UNITTEST_START_TESTCASE(page_queues_tests)
VM_UNITTEST(pq_add_remove)
VM_UNITTEST(pq_move_queues)
VM_UNITTEST(pq_move_self_queue)
VM_UNITTEST(pq_rotate_queue)
VM_UNITTEST(pq_toggle_dont_need_queue)
VM_UNITTEST(pq_single_queue_fifo_order)
VM_UNITTEST(pq_multiple_queues_fifo_order)
VM_UNITTEST(pq_isolate_dont_need_fifo_order)
VM_UNITTEST(pq_isolate_queues_priority)
VM_UNITTEST(pq_is_page_reclaimable)
UNITTEST_END_TESTCASE(page_queues_tests, "pq", "PageQueues tests")

}  // namespace vm_unittest
