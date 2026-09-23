// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/page/size.h>

#include <kernel/thread.h>

#include "test_helper.h"

namespace vm_unittest {

namespace {

// Helper class for managing a PmmNode with real pages. AllocRange and AllocContiguous are not
// supported by the managed PmmNode object. Only a single instance can exist at a time.
class ManagedPmmNode {
 public:
  static constexpr size_t kNumPages = 64;
  static constexpr size_t kDefaultMemEventLowerBound = kNumPages / 2;
  static constexpr size_t kDefaultShouldWaitLevel = kNumPages / 4;

  static constexpr size_t kDefaultLowMemAlloc =
      ManagedPmmNode::kNumPages - kDefaultShouldWaitLevel + 1;
  static constexpr size_t kDefaultMemEventAlloc =
      ManagedPmmNode::kNumPages - kDefaultMemEventLowerBound + 1;

  ManagedPmmNode() {
    VmPageDoublyLinkedList list;
    ZX_ASSERT(pmm_alloc_pages(kNumPages, 0, &list) == ZX_OK);
    for (auto& page : list) {
      // TODO: Prevent this page state from allowing AllocContiguous() to potentially find a run of
      // FREE pages involving some of these pages.
      page.set_state(vm_page_state::FREE);
    }
    node_.AddFreePages(&list);

    // Test node always has checking enabled.
    ASSERT(node_.EnableFreePageFilling(kPageSize, CheckFailAction::kPanic));
    node_.FillFreePagesAndArm();

    bool result = ResetDefaultMemEvent();
    ASSERT(result);

    zx_status_t status = VmObjectPaged::Create(0, 0, 0, &vmo_);
    ASSERT(status == ZX_OK);
  }

  ~ManagedPmmNode() {
    VmPageDoublyLinkedList list;
    zx_status_t status = node_.AllocPages(kNumPages, 0, &list);
    ASSERT(status == ZX_OK);
    for (auto& page : list) {
      page.set_state(vm_page_state::ALLOC);
    }
    pmm_free(&list);
  }

  bool IsEventSignaled() { return event_.Wait(Deadline::infinite_past()) == ZX_OK; }

  void UnsignalEvent() { event_.Unsignal(); }

  bool ResetDefaultMemEvent() {
    return SetFreeMemorySignal(kDefaultMemEventLowerBound, UINT64_MAX, kDefaultShouldWaitLevel);
  }

  bool SetFreeMemorySignal(uint64_t lower_bound, uint64_t higher_bound, uint64_t delay_pages) {
    return node_.SetFreeMemorySignal(lower_bound, higher_bound, delay_pages, &event_);
  }

  PmmNode& node() { return node_; }

  zx_status_t AllocLoanedPages(size_t count, vm_page_t** pages) {
    if (!scanner_disable_) {
      scanner_disable_.emplace();
    }
    for (size_t i = 0; i < count; i++) {
      zx::result<vm_page_t*> result =
          node_.AllocLoanedPage([cow = vmo_->DebugGetCowPages().get()](vm_page_t* page) {
            page->set_state(vm_page_state::OBJECT);
            page->object.set_object(nullptr);
            page->object.set_page_offset(0);
            Pmm::Node().GetPageQueues()->SetReclaim(page, cow, 0);
          });
      if (result.is_error()) {
        for (size_t j = 0; j < i; j++) {
          FreeLoanedPage(pages[j]);
        }
        return result.status_value();
      }
      pages[i] = *result;
    }
    return ZX_OK;
  }
  void FreeLoanedPage(vm_page_t* page) {
    FreeLoanedPagesHolder flph;
    node_.BeginFreeLoanedPage(
        page, [](vm_page_t* page) { Pmm::Node().GetPageQueues()->Remove(page); }, flph);
    Pmm::Node().FinishFreeLoanedPages(flph);
  }

 private:
  PmmNode node_;
  Event event_;
  // VMO that we will use to have a valid backlink for any loaned pages that get allocated.
  fbl::RefPtr<VmObjectPaged> vmo_;
  // An optional scanner disable that is instantiated should any loaned pages get allocated. This is
  // needed as our backlinks, while valid pointers, will confuse reclamation if it tries to reclaim
  // using them.
  ktl::optional<AutoVmScannerDisable> scanner_disable_;
};

}  // namespace

// Allocates a single page, translates it to a vm_page_t and frees it.
static bool pmm_smoke_test() {
  BEGIN_TEST;
  paddr_t pa;
  vm_page_t* page;

  zx_status_t status = pmm_alloc_page(0, &page, &pa);
  ASSERT_EQ(ZX_OK, status, "pmm_alloc single page");
  ASSERT_NONNULL(page, "pmm_alloc single page");
  ASSERT_NE(0u, pa, "pmm_alloc single page");

  vm_page_t* page2 = paddr_to_vm_page(pa);
  ASSERT_EQ(page2, page, "paddr_to_vm_page on single page");

  pmm_free_page(page);
  END_TEST;
}

// Allocates one page and frees it.
static bool pmm_alloc_contiguous_one_test() {
  BEGIN_TEST;
  VmPageDoublyLinkedList list;
  paddr_t pa;
  size_t count = 1U;
  zx_status_t status = pmm_alloc_contiguous(count, 0, kPageShift, &pa, &list);
  ASSERT_EQ(ZX_OK, status, "pmm_alloc_contiguous returned failure\n");
  ASSERT_EQ(count, list.size_slow(), "pmm_alloc_contiguous list size is wrong");
  ASSERT_NONNULL(paddr_to_physmap(pa));
  pmm_free(&list);
  END_TEST;
}

// Allocates more than one page and frees them.
static bool pmm_node_multi_alloc_test() {
  BEGIN_TEST;
  ManagedPmmNode node;
  static constexpr size_t alloc_count = ManagedPmmNode::kNumPages / 2;
  VmPageDoublyLinkedList list;

  zx_status_t status = node.node().AllocPages(alloc_count, 0, &list);
  EXPECT_EQ(ZX_OK, status, "pmm_alloc_pages a few pages");
  EXPECT_EQ(alloc_count, list.size_slow(), "pmm_alloc_pages a few pages list count");

  status = node.node().AllocPages(alloc_count, 0, &list);
  EXPECT_EQ(ZX_OK, status, "pmm_alloc_pages a few pages");
  EXPECT_EQ(2 * alloc_count, list.size_slow(), "pmm_alloc_pages a few pages list count");

  node.node().FreeList(&list);
  END_TEST;
}

// Allocates one page from the bulk allocation api.
static bool pmm_node_singleton_list_test() {
  BEGIN_TEST;
  ManagedPmmNode node;
  VmPageDoublyLinkedList list;

  zx_status_t status = node.node().AllocPages(1, 0, &list);
  EXPECT_EQ(ZX_OK, status, "pmm_alloc_pages a few pages");
  EXPECT_EQ(1ul, list.size_slow(), "pmm_alloc_pages a few pages list count");

  node.node().FreeList(&list);
  END_TEST;
}

// Loans pages back to the PmmNode, allocates them as usable pages while loaned, cancels that loan,
// reclaims the pages via "churn" (to FREE), ends the loan.
static bool pmm_node_loan_borrow_cancel_reclaim_end() {
  BEGIN_TEST;

  ManagedPmmNode node;

  bool was_loaning_enabled = PhysicalPageBorrowingConfig::Get().is_loaning_enabled();
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  auto cleanup = fit::defer([was_loaning_enabled] {
    PhysicalPageBorrowingConfig::Get().set_loaning_enabled(was_loaning_enabled);
  });

  VmPageDoublyLinkedList list;

  constexpr uint64_t kLoanCount = ManagedPmmNode::kNumPages * 3 / 4;
  constexpr uint64_t kNotLoanCount = ManagedPmmNode::kNumPages - kLoanCount;
  static_assert(kNotLoanCount <= kLoanCount);
  paddr_t paddr[kLoanCount] = {};

  zx_status_t status = node.node().AllocPages(kLoanCount, 0, &list);
  EXPECT_EQ(ZX_OK, status, "pmm_alloc_pages a few pages");
  EXPECT_EQ(kLoanCount, list.size_slow(), "pmm_alloc_pages correct # pages");

  uint32_t i = 0;
  for (auto& page : list) {
    paddr[i] = page.paddr();
    ++i;
  }

  for (auto& page : list) {
    EXPECT_FALSE(page.is_loaned());
    EXPECT_FALSE(page.is_loan_cancelled());
  }
  node.node().BeginLoan(&list);
  for (auto& page : list) {
    EXPECT_TRUE(page.is_loaned());
    EXPECT_FALSE(page.is_loan_cancelled());
  }

  EXPECT_EQ(kLoanCount, node.node().CountLoanedPages());
  EXPECT_EQ(kNotLoanCount, node.node().CountFreePages());
  EXPECT_EQ(kLoanCount, node.node().CountLoanedFreePages());
  EXPECT_EQ(0u, node.node().CountLoanCancelledPages());
  EXPECT_EQ(0u, node.node().CountLoanedNotFreePages());

  EXPECT_EQ(0u, list.size_slow());
  vm_page_t* loaned_pages[kLoanCount] = {};
  status = node.AllocLoanedPages(kLoanCount, &loaned_pages[0]);
  EXPECT_EQ(ZX_OK, status, "pmm_alloc_pages PMM_ALLOC_FLAG_LOANED");

  for (auto& p : loaned_pages) {
    for (i = 0; i < kLoanCount; ++i) {
      if (paddr[i] == p->paddr()) {
        break;
      }
    }
    // match found
    EXPECT_NE(kLoanCount, i);
  }

  for (auto& p : loaned_pages) {
    EXPECT_TRUE(p->is_loaned());
    EXPECT_FALSE(p->is_loan_cancelled());
    node.node().CancelLoan(p);
    EXPECT_TRUE(p->is_loaned());
    EXPECT_TRUE(p->is_loan_cancelled());
  }

  EXPECT_EQ(kLoanCount, node.node().CountLoanedPages());
  EXPECT_EQ(kNotLoanCount, node.node().CountFreePages());
  EXPECT_EQ(0u, node.node().CountLoanedFreePages());
  EXPECT_EQ(kLoanCount, node.node().CountLoanCancelledPages());
  EXPECT_EQ(kLoanCount, node.node().CountLoanedNotFreePages());

  for (auto& p : loaned_pages) {
    node.FreeLoanedPage(p);
  }

  EXPECT_EQ(kLoanCount, node.node().CountLoanedPages());
  EXPECT_EQ(kNotLoanCount, node.node().CountFreePages());
  EXPECT_EQ(0u, node.node().CountLoanedFreePages());
  EXPECT_EQ(kLoanCount, node.node().CountLoanCancelledPages());
  // Still not free; loan_cancelled means the page can't be allocated.
  EXPECT_EQ(kLoanCount, node.node().CountLoanedNotFreePages());

  EXPECT_EQ(0u, list.size_slow());
  status = node.AllocLoanedPages(kNotLoanCount + 1, &loaned_pages[0]);
  EXPECT_EQ(ZX_ERR_NO_RESOURCES, status, "try to allocate a loan_cancelled page");

  EXPECT_EQ(0u, list.size_slow());
  status = node.node().AllocPages(kNotLoanCount, PMM_ALLOC_FLAG_ANY, &list);
  EXPECT_EQ(ZX_OK, status, "allocate all the not-loaned pages");

  for (auto& page : list) {
    EXPECT_FALSE(page.is_loaned());
    for (i = 0; i < kLoanCount; ++i) {
      if (paddr[i] == page.paddr()) {
        break;
      }
    }
    // match not found
    EXPECT_EQ(kLoanCount, i);
  }

  node.node().FreeList(&list);

  EXPECT_EQ(0u, list.size_slow());
  for (uint32_t j = 0; j < kLoanCount; ++j) {
    vm_page_t* page = loaned_pages[j];
    EXPECT_EQ(paddr[j], page->paddr());
    node.node().EndLoan(page);
    EXPECT_FALSE(page->is_loaned());
    EXPECT_FALSE(page->is_loan_cancelled());
    list.push_back(page);
  }

  node.node().FreeList(&list);

  EXPECT_EQ(0u, node.node().CountLoanedPages());
  EXPECT_EQ(ManagedPmmNode::kNumPages, node.node().CountFreePages());
  EXPECT_EQ(0u, node.node().CountLoanedFreePages());
  EXPECT_EQ(0u, node.node().CountLoanCancelledPages());
  EXPECT_EQ(0u, node.node().CountLoanedNotFreePages());

  EXPECT_EQ(0u, list.size_slow());
  status = node.node().AllocPages(ManagedPmmNode::kNumPages, 0, &list);
  EXPECT_EQ(ZX_OK, status, "allocate all pages");
  EXPECT_EQ(ManagedPmmNode::kNumPages, list.size_slow());

  for (auto& page : list) {
    EXPECT_FALSE(page.is_loaned());
    EXPECT_FALSE(page.is_loan_cancelled());
  }

  node.node().FreeList(&list);

  EXPECT_EQ(0u, node.node().CountLoanedPages());
  EXPECT_EQ(ManagedPmmNode::kNumPages, node.node().CountFreePages());
  EXPECT_EQ(0u, node.node().CountLoanedFreePages());
  EXPECT_EQ(0u, node.node().CountLoanCancelledPages());
  EXPECT_EQ(0u, node.node().CountLoanedNotFreePages());

  END_TEST;
}

// Allocates too many pages and makes sure it fails nicely.
static bool pmm_node_oversized_alloc_test() {
  BEGIN_TEST;
  ManagedPmmNode node;
  VmPageDoublyLinkedList list;

  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kNumPages + 1, 0, &list);
  EXPECT_EQ(ZX_ERR_NO_MEMORY, status, "pmm_alloc_pages failed to alloc");
  EXPECT_TRUE(list.is_empty(), "pmm_alloc_pages list is empty");

  END_TEST;
}

// Check that free memory events work correctly.
static bool pmm_node_free_mem_event_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  uint64_t free_count = node.node().CountFreePages();

  // Should initially be some free pages, validate this to allow us to assume the ability to -1.
  ASSERT_GT(free_count, 0u);

  // Setting an event range that does not include the current free count should be invalid.
  EXPECT_FALSE(node.SetFreeMemorySignal(free_count + 1, UINT64_MAX, 0));
  EXPECT_FALSE(node.SetFreeMemorySignal(0, free_count - 1, 0));

  // The range can be inclusive of the current free count.
  EXPECT_TRUE(node.SetFreeMemorySignal(free_count, UINT64_MAX, 0));
  EXPECT_TRUE(node.SetFreeMemorySignal(0, free_count, 0));

  // Reset back to the default event.
  EXPECT_TRUE(node.ResetDefaultMemEvent());

  // Should never have triggered the event up to this point.
  EXPECT_FALSE(node.IsEventSignaled());

  // Allocate all but 1 of the pages to trigger the event.
  VmPageDoublyLinkedList list;

  for (size_t i = 1; i < ManagedPmmNode::kDefaultMemEventAlloc; i++) {
    zx::result<vm_page_t*> result = node.node().AllocPage(0);
    ASSERT_OK(result.status_value());
    vm_page_t* page = *result;
    list.push_back(page);
    if (node.IsEventSignaled()) {
      printf("Event signaled at step %zu\n", i);
    }
  }
  // Should not have triggered the event yet.
  EXPECT_FALSE(node.IsEventSignaled());

  // Allocate the last page, this should put us over the limit and set the event.
  {
    zx::result<vm_page_t*> result = node.node().AllocPage(0);
    ASSERT_OK(result.status_value());
    vm_page_t* page = *result;
    list.push_back(page);
  }
  EXPECT_TRUE(node.IsEventSignaled());
  node.UnsignalEvent();

  // Events are one-shot, and so putting a page back and allocating it again should not re-trigger
  // the event.
  node.node().FreePage(list.pop_front());
  {
    zx::result<vm_page_t*> result = node.node().AllocPage(0);
    ASSERT_OK(result.status_value());
    vm_page_t* page = *result;
    list.push_back(page);
  }
  EXPECT_FALSE(node.IsEventSignaled());

  // Set a new free range that should trip ass we return the pages back.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages - 1, 0));

  // Take one page off the list as our final page.
  vm_page_t* page = list.pop_front();

  // Return the rest of the list.
  node.node().FreeList(&list);
  // Event should not have tripped yet.
  EXPECT_FALSE(node.IsEventSignaled());

  // Return the last page, should trip.
  node.node().FreePage(page);
  EXPECT_TRUE(node.IsEventSignaled());

  END_TEST;
}

// Checks sync allocation failure when the node crosses a threshold.
static bool pmm_node_low_mem_alloc_failure_test() {
  BEGIN_TEST;
  ManagedPmmNode node;
  VmPageDoublyLinkedList list;

  // Put the node in an oom state and make sure allocation fails.
  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kDefaultLowMemAlloc, 0, &list);
  EXPECT_EQ(ZX_OK, status);
  // Should also have been signaled.
  EXPECT_TRUE(node.IsEventSignaled());

  zx::result<vm_page_t*> result = node.node().AllocPage(PMM_ALLOC_FLAG_CAN_WAIT);
  EXPECT_EQ(ZX_ERR_SHOULD_WAIT, result.status_value());

  // Waiting for an allocation should block, although to only try with a very small timeout to not
  // make this test take too long.
  EXPECT_EQ(ZX_ERR_TIMED_OUT,
            node.node()
                .WaitForSinglePageAllocation(Deadline::after_mono(ZX_MSEC(10)), true)
                .status_value());

  // Free the list.
  node.node().FreeList(&list);

  // Allocations will still be delayed until we reset the trigger.
  result = node.node().AllocPage(PMM_ALLOC_FLAG_CAN_WAIT);
  EXPECT_EQ(result.status_value(), ZX_ERR_SHOULD_WAIT);

  EXPECT_TRUE(node.ResetDefaultMemEvent());

  // Allocations should work again, but the PMM is still allowed to randomly fail requests, so we
  // cannot guarantee that any small finite number of allocation attempts will work.
  // We can check that waiting to retry an allocation completes with no timeout though.
  {
    auto alloc_page = node.node().WaitForSinglePageAllocation(Deadline::infinite_past(), true);
    ASSERT_NE(alloc_page.status_value(), ZX_ERR_TIMED_OUT);
    if (alloc_page.is_ok()) {
      node.node().FreePage(alloc_page.value());
    }
  }

  // Reset the signal.
  node.UnsignalEvent();
  // Set a threshold such that a single allocation should trip into the low mem state.
  EXPECT_TRUE(
      node.SetFreeMemorySignal(ManagedPmmNode::kNumPages, UINT64_MAX, ManagedPmmNode::kNumPages));

  // Signal should not yet be set, and allocations should not be delayed.
  EXPECT_FALSE(node.IsEventSignaled());
  {
    auto alloc_page = node.node().WaitForSinglePageAllocation(Deadline::infinite_past(), true);
    ASSERT_NE(alloc_page.status_value(), ZX_ERR_TIMED_OUT);
    if (alloc_page.is_ok()) {
      node.node().FreePage(alloc_page.value());
    }
  }

  // Allocate a single page and validate that allocations are now delayed.
  ASSERT_OK(node.node().AllocPages(1, 0, &list));
  result = node.node().AllocPage(PMM_ALLOC_FLAG_CAN_WAIT);
  EXPECT_EQ(result.status_value(), ZX_ERR_SHOULD_WAIT);
  EXPECT_EQ(ZX_ERR_TIMED_OUT,
            node.node()
                .WaitForSinglePageAllocation(Deadline::after_mono(ZX_MSEC(10)), true)
                .status_value());

  node.node().FreeList(&list);

  END_TEST;
}

// Test that deliberately putting into a no alloc state (and back out) works.
static bool pmm_node_explicit_should_wait_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Place the node directly into a state the forbids allocations.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages, UINT64_MAX));

  // Allocations that can wait should be blocked.
  zx::result<vm_page_t*> result = node.node().AllocPage(PMM_ALLOC_FLAG_CAN_WAIT);
  EXPECT_EQ(result.status_value(), ZX_ERR_SHOULD_WAIT);
  EXPECT_EQ(ZX_ERR_TIMED_OUT,
            node.node()
                .WaitForSinglePageAllocation(Deadline::after_mono(ZX_MSEC(10)), true)
                .status_value());

  // A regular allocation should work.
  result = node.node().AllocPage(0);
  ASSERT_OK(result.status_value());
  node.node().FreePage(*result);

  // Changing the delayed threshold should re-enable allocations.
  EXPECT_TRUE(node.ResetDefaultMemEvent());

  {
    auto alloc_page = node.node().WaitForSinglePageAllocation(Deadline::infinite_past(), true);
    ASSERT_NE(alloc_page.status_value(), ZX_ERR_TIMED_OUT);
    if (alloc_page.is_ok()) {
      node.node().FreePage(alloc_page.value());
    }
  }

  END_TEST;
}

struct PmmWaiterArgs {
  PmmNode* node;
  ktl::atomic<int>* timeout_count;
  ktl::atomic<int>* no_memory_count;
};

static int pmm_waiter_thread(void* arg) {
  PmmWaiterArgs* args = static_cast<PmmWaiterArgs*>(arg);
  auto result = args->node->WaitForSinglePageAllocation(Deadline::after_mono(ZX_SEC(2)), true);
  if (result.status_value() == ZX_ERR_TIMED_OUT) {
    args->timeout_count->fetch_add(1);
  } else if (result.status_value() == ZX_ERR_NO_MEMORY) {
    args->no_memory_count->fetch_add(1);
  } else if (result.is_ok()) {
    args->node->FreePage(result.value());
  }
  return 0;
}

// Verifies that once StopReturningShouldWait() is called, WaitForSinglePageAllocation
// does not block, even if the event was consumed by a previous call.
static bool pmm_node_stop_returning_should_wait_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
  VmPageDoublyLinkedList list;
  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kNumPages, 0, &list);
  EXPECT_EQ(ZX_OK, status);

  // Place the node directly into a state that forbids allocations.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages, UINT64_MAX));

  ktl::atomic<int> timeout_count = 0;
  ktl::atomic<int> no_memory_count = 0;
  PmmWaiterArgs args{&node.node(), &timeout_count, &no_memory_count};

  // Start a thread that will wait.
  Thread* thread = Thread::Create("pmm waiter", pmm_waiter_thread, &args, DEFAULT_PRIORITY);
  thread->Resume();

  // Give the thread time to block.
  Thread::Current::SleepRelative(ZX_MSEC(100));

  // Stop returning should wait. This should wake up the thread.
  node.node().StopReturningShouldWait();

  // Wait for the thread to complete.
  thread->Join(nullptr, ZX_TIME_INFINITE);

  // Verify that the thread did not time out.
  EXPECT_EQ(timeout_count.load(), 0);
  // Verify that the thread failed with NO_MEMORY.
  EXPECT_EQ(no_memory_count.load(), 1);

  // Second call: may_allocate_evt_ might be unsignaled but since should_wait_ is Never, it should
  // not wait.
  auto alloc_page2 =
      node.node().WaitForSinglePageAllocation(Deadline::after_mono(ZX_MSEC(10)), true);
  EXPECT_EQ(alloc_page2.status_value(), ZX_ERR_NO_MEMORY);

  // Clean up.
  node.node().FreeList(&list);

  END_TEST;
}

// Verifies that once StopReturningShouldWait() is called, all threads blocked on
// WaitForSinglePageAllocation are woken up.
static bool pmm_node_stop_returning_should_wait_concurrent_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
  VmPageDoublyLinkedList list;
  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kNumPages, 0, &list);
  EXPECT_EQ(ZX_OK, status);

  // Place the node directly into a state that forbids allocations.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages, UINT64_MAX));

  ktl::atomic<int> timeout_count = 0;
  ktl::atomic<int> no_memory_count = 0;
  PmmWaiterArgs args{&node.node(), &timeout_count, &no_memory_count};

  constexpr int kNumWaiters = 3;
  Thread* threads[kNumWaiters];

  for (int i = 0; i < kNumWaiters; ++i) {
    threads[i] = Thread::Create("pmm waiter", pmm_waiter_thread, &args, DEFAULT_PRIORITY);
    threads[i]->Resume();
  }

  // Give threads time to block.
  Thread::Current::SleepRelative(ZX_MSEC(100));

  // Stop returning should wait.
  node.node().StopReturningShouldWait();

  // Wait for all threads to complete.
  for (auto& t : threads) {
    t->Join(nullptr, ZX_TIME_INFINITE);
  }

  // Verify that NO threads timed out.
  EXPECT_EQ(timeout_count.load(), 0);
  // Verify that all threads failed with NO_MEMORY.
  EXPECT_EQ(no_memory_count.load(), kNumWaiters);

  // Clean up.
  node.node().FreeList(&list);

  END_TEST;
}

struct PmmSuspendKillWaiterArgs {
  PmmNode* node;
  bool suspendable;
  zx_duration_mono_t timeout;
  ktl::atomic<zx_status_t> result;
};

static int pmm_suspend_kill_waiter_thread(void* arg) {
  PmmSuspendKillWaiterArgs* args = static_cast<PmmSuspendKillWaiterArgs*>(arg);
  auto res = args->node->WaitForSinglePageAllocation(Deadline::after_mono(args->timeout),
                                                     args->suspendable);
  args->result.store(res.status_value());
  if (res.is_ok()) {
    args->node->FreePage(res.value());
  }
  return 0;
}

// Verifies that WaitForSinglePageAllocation with suspendable = true is interrupted immediately
// when the thread is suspended, returning ZX_ERR_INTERNAL_INTR_RETRY.
static bool pmm_node_suspendable_wait_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
  VmPageDoublyLinkedList list;
  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kNumPages, 0, &list);
  EXPECT_EQ(ZX_OK, status);

  // Place the node directly into a state that forbids allocations.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages, UINT64_MAX));

  PmmSuspendKillWaiterArgs args{&node.node(), true, ZX_SEC(5), ZX_OK};

  // Start a thread that will wait in a suspendable state.
  Thread* thread = Thread::Create("pmm suspendable waiter", pmm_suspend_kill_waiter_thread, &args,
                                  DEFAULT_PRIORITY);
  thread->Resume();

  // Give the thread time to block.
  Thread::Current::SleepRelative(ZX_MSEC(100));

  // Suspend the thread.
  thread->Suspend();

  // Wait for the thread to complete (it should exit immediately due to suspension).
  thread->Join(nullptr, ZX_TIME_INFINITE);

  // Verify that the thread returned ZX_ERR_INTERNAL_INTR_RETRY.
  EXPECT_EQ(args.result.load(), ZX_ERR_INTERNAL_INTR_RETRY);

  // Clean up.
  node.node().FreeList(&list);

  END_TEST;
}

// Verifies that WaitForSinglePageAllocation with suspendable = false ignores suspend signals
// and blocks until the timeout expires, returning ZX_ERR_TIMED_OUT.
static bool pmm_node_non_suspendable_wait_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
  VmPageDoublyLinkedList list;
  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kNumPages, 0, &list);
  EXPECT_EQ(ZX_OK, status);

  // Place the node directly into a state that forbids allocations.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages, UINT64_MAX));

  // Use a short 200ms timeout so the test completes quickly.
  PmmSuspendKillWaiterArgs args{&node.node(), false, ZX_MSEC(200), ZX_OK};

  // Start a thread that will wait in a non-suspendable state.
  Thread* thread = Thread::Create("pmm non-suspendable waiter", pmm_suspend_kill_waiter_thread,
                                  &args, DEFAULT_PRIORITY);
  thread->Resume();

  // Give the thread time to block.
  Thread::Current::SleepRelative(ZX_MSEC(50));

  // Suspend the thread (which should be ignored by the Wait loop).
  thread->Suspend();

  // Wait for the thread to complete (it should wait out the full 200ms timeout).
  thread->Join(nullptr, ZX_TIME_INFINITE);

  // Verify that the thread returned ZX_ERR_TIMED_OUT instead of ZX_ERR_INTERNAL_INTR_RETRY.
  EXPECT_EQ(args.result.load(), ZX_ERR_TIMED_OUT);

  // Clean up.
  node.node().FreeList(&list);

  END_TEST;
}

// Verifies that WaitForSinglePageAllocation returns ZX_ERR_INTERNAL_INTR_KILLED
// when the thread is killed.
static bool pmm_node_killed_wait_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
  VmPageDoublyLinkedList list;
  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kNumPages, 0, &list);
  EXPECT_EQ(ZX_OK, status);

  // Place the node directly into a state that forbids allocations.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages, UINT64_MAX));

  // Use a long timeout so the test doesn't time out.
  PmmSuspendKillWaiterArgs args{&node.node(), true, ZX_SEC(5), ZX_OK};

  // Start a thread that will wait.
  Thread* thread =
      Thread::Create("pmm killed waiter", pmm_suspend_kill_waiter_thread, &args, DEFAULT_PRIORITY);
  thread->Resume();

  // Give the thread time to block.
  Thread::Current::SleepRelative(ZX_MSEC(100));

  // Kill the thread.
  thread->Kill();

  // Wait for the thread to complete.
  thread->Join(nullptr, ZX_TIME_INFINITE);

  // Verify that the thread returned ZX_ERR_INTERNAL_INTR_KILLED.
  EXPECT_EQ(args.result.load(), ZX_ERR_INTERNAL_INTR_KILLED);

  // Clean up.
  node.node().FreeList(&list);

  END_TEST;
}

// Verifies that WaitForSinglePageAllocation with suspendable = false ignores suspend signals
// but is interrupted immediately when the thread is killed, returning ZX_ERR_INTERNAL_INTR_KILLED.
static bool pmm_node_suspend_then_killed_wait_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
  VmPageDoublyLinkedList list;
  zx_status_t status = node.node().AllocPages(ManagedPmmNode::kNumPages, 0, &list);
  EXPECT_EQ(ZX_OK, status);

  // Place the node directly into a state that forbids allocations.
  EXPECT_TRUE(node.SetFreeMemorySignal(0, ManagedPmmNode::kNumPages, UINT64_MAX));

  // Use a long timeout so the test doesn't time out naturally.
  PmmSuspendKillWaiterArgs args{&node.node(), false, ZX_SEC(5), ZX_OK};

  // Start a thread that will wait in a non-suspendable state.
  Thread* thread = Thread::Create("pmm suspend-then-killed waiter", pmm_suspend_kill_waiter_thread,
                                  &args, DEFAULT_PRIORITY);
  thread->Resume();

  // Give the thread time to block.
  Thread::Current::SleepRelative(ZX_MSEC(100));

  // Suspend the thread (which should be ignored).
  thread->Suspend();

  // Give it some time to ensure it's still blocked.
  Thread::Current::SleepRelative(ZX_MSEC(50));

  // Now kill the thread.
  thread->Kill();

  // Wait for the thread to complete.
  thread->Join(nullptr, ZX_TIME_INFINITE);

  // Verify that the thread returned ZX_ERR_INTERNAL_INTR_KILLED.
  EXPECT_EQ(args.result.load(), ZX_ERR_INTERNAL_INTR_KILLED);

  // Clean up.
  node.node().FreeList(&list);

  END_TEST;
}

static bool pmm_checker_test_with_fill_size(size_t fill_size) {
  BEGIN_TEST;

  PmmChecker checker;

  // Starts off unarmed.
  EXPECT_FALSE(checker.IsArmed());

  // Borrow a real page from the PMM, ask the checker to validate it.  See that because the checker
  // is not armed, |ValidatePattern| still returns true even though the page has no pattern.
  vm_page_t* page;
  EXPECT_EQ(pmm_alloc_page(0, &page), ZX_OK);
  page->set_state(vm_page_state::FREE);
  auto p = static_cast<uint8_t*>(paddr_to_physmap(page->paddr()));
  memset(p, 0, kPageSize);
  EXPECT_TRUE(checker.ValidatePattern(page));
  checker.AssertPattern(page);

  // Set the fill size and see that |GetFillSize| returns the size.
  checker.SetFillSize(fill_size);
  EXPECT_EQ(fill_size, checker.GetFillSize());

  // Arm the checker and see that |ValidatePattern| returns false.
  checker.Arm();
  EXPECT_TRUE(checker.IsArmed());
  EXPECT_FALSE(checker.ValidatePattern(page));

  // Fill with pattern one less than the fill size and see that it does not pass validation.
  memset(p, 0, fill_size - 1);
  EXPECT_FALSE(checker.ValidatePattern(page));

  // Fill with the full pattern and see that it validates.
  checker.FillPattern(page);
  for (size_t i = 0; i < fill_size; ++i) {
    EXPECT_NE(0, p[i]);
  }
  EXPECT_TRUE(checker.ValidatePattern(page));

  // Corrupt the page after the first |fill_size| bytes and see that the corruption is not detected.
  if (fill_size < kPageSize) {
    p[fill_size] = 1;
    EXPECT_TRUE(checker.ValidatePattern(page));
  }

  // Corrupt the page within the first |fill_size| bytes and see that the corruption is detected.
  p[fill_size - 1] = 1;
  EXPECT_FALSE(checker.ValidatePattern(page));

  page->set_state(vm_page_state::ALLOC);
  pmm_free_page(page);

  END_TEST;
}

static bool pmm_checker_test() {
  BEGIN_TEST;

  EXPECT_TRUE(pmm_checker_test_with_fill_size(8));
  EXPECT_TRUE(pmm_checker_test_with_fill_size(16));
  EXPECT_TRUE(pmm_checker_test_with_fill_size(512));
  EXPECT_TRUE(pmm_checker_test_with_fill_size(kPageSize));

  END_TEST;
}

static bool pmm_checker_is_valid_fill_size_test() {
  BEGIN_TEST;

  EXPECT_FALSE(PmmChecker::IsValidFillSize(0));
  EXPECT_FALSE(PmmChecker::IsValidFillSize(7));
  EXPECT_FALSE(PmmChecker::IsValidFillSize(9));
  EXPECT_FALSE(PmmChecker::IsValidFillSize(kPageSize + 8));
  EXPECT_FALSE(PmmChecker::IsValidFillSize(kPageSize * 2));

  EXPECT_TRUE(PmmChecker::IsValidFillSize(8));
  EXPECT_TRUE(PmmChecker::IsValidFillSize(16));
  EXPECT_TRUE(PmmChecker::IsValidFillSize(24));
  EXPECT_TRUE(PmmChecker::IsValidFillSize(512));
  EXPECT_TRUE(PmmChecker::IsValidFillSize(kPageSize));

  END_TEST;
}

static bool pmm_get_arena_info_test() {
  BEGIN_TEST;

  const size_t num_arenas = pmm_num_arenas();
  ASSERT_GT(num_arenas, 0u);

  fbl::AllocChecker ac;
  auto buffer = ktl::unique_ptr<pmm_arena_info_t[]>(new (&ac) pmm_arena_info_t[num_arenas]);
  ASSERT(ac.check());
  const size_t buffer_size = num_arenas * sizeof(pmm_arena_info_t);

  // Not enough room for one.
  zx_status_t status = pmm_get_arena_info(1, 0, buffer.get(), sizeof(pmm_arena_info_t) - 1);
  ASSERT_EQ(status, ZX_ERR_BUFFER_TOO_SMALL);

  // Asking for none.
  status = pmm_get_arena_info(0, 0, buffer.get(), buffer_size);
  ASSERT_EQ(status, ZX_ERR_OUT_OF_RANGE);

  // Asking for more than exist.
  status = pmm_get_arena_info(num_arenas + 1, 0, buffer.get(), buffer_size);
  ASSERT_EQ(status, ZX_ERR_OUT_OF_RANGE);

  // Attempting to skip them all.
  status = pmm_get_arena_info(1, num_arenas, buffer.get(), buffer_size);
  ASSERT_EQ(status, ZX_ERR_OUT_OF_RANGE);

  // Asking for one.
  status = pmm_get_arena_info(1, 0, buffer.get(), buffer_size);
  ASSERT_EQ(status, ZX_OK);

  // Asking for them all.
  status = pmm_get_arena_info(num_arenas, 0, buffer.get(), buffer_size);
  ASSERT_EQ(status, ZX_OK);

  // See they are in ascending order by base.
  paddr_t prev = 0;
  for (unsigned i = 0; i < num_arenas; ++i) {
    if (i == 0) {
      ASSERT_GE(buffer[i].base, prev);
    } else {
      ASSERT_GT(buffer[i].base, prev);
    }
    prev = buffer[i].base;
    ASSERT_GT(buffer[i].size, 0u);
  }

  END_TEST;
}

static void SetPageStateRange(enum vm_page_state state, vm_page_t* start, int count) {
  for (int i = 0; i < count; ++i) {
    (start + i)->set_state(state);
  }
}

static bool pmm_arena_find_free_contiguous_test() {
  BEGIN_TEST;

  static constexpr size_t kNumPages = 8;
  const vaddr_t base = 0x1001000;
  const pmm_arena_info_t info{"test arena", 0, base, kNumPages * kPageSize};

  vm_page_t page_array[kNumPages]{};
  PmmArena arena;
  arena.InitForTest(info, page_array);

  // page_array is as follow (0 == free, 1 == allocated):
  //
  // [00000000]
  //
  // Ask for some sizes and alignments that can't possibly succeed.
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(kNumPages + 1, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(kNumPages + 2, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(kNumPages + 3, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(kNumPages + 4, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(1, 24));  // 16MB aligned
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(1, 25));  // 32MB aligned
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(1, 26));  // 64MB aligned
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(1, 27));  // 128MB aligned

  // [00000000]
  //
  // Ask for 4 pages,  aligned on a 2-page boundary.  See that the first page is skipped.
  vm_page_t* result = arena.FindFreeContiguous(4, kPageShift + 1);
  ASSERT_EQ(&page_array[1], result);
  SetPageStateRange(vm_page_state::ALLOC, result, 4);

  // [01111000]
  //
  // Ask for various sizes and see that they all fail.
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(4, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(5, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(6, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(7, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(8, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(9, kPageShift));

  // [01111000]
  //
  // Ask for 3 pages.
  result = arena.FindFreeContiguous(3, kPageShift);
  ASSERT_EQ(&page_array[5], result);
  SetPageStateRange(vm_page_state::ALLOC, result, 3);

  // [01111111]
  //
  // Ask for various sizes and see that they all fail.
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(2, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(3, kPageShift));
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(4, kPageShift));

  // [01111111]
  //
  // Ask for the last remaining page.
  result = arena.FindFreeContiguous(1, kPageShift);
  ASSERT_EQ(&page_array[0], result);
  SetPageStateRange(vm_page_state::ALLOC, result, 1);

  // [11111111]
  //
  // See there are none left.
  ASSERT_EQ(nullptr, arena.FindFreeContiguous(1, kPageShift));

  END_TEST;
}

// Check that when AllocPages appends to an existing list it does not run the checker on the pages
// already in the list.
static bool pmm_alloc_append_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  VmPageDoublyLinkedList alloc_list;
  auto cleanup = fit::defer([&]() {
    if (!alloc_list.is_empty()) {
      node.node().FreeList(&alloc_list);
    }
  });

  // Allocate a single page into the list first.
  ASSERT_OK(node.node().AllocPages(1, 0u, &alloc_list));

  // Zero the page as a modification.
  memset(paddr_to_physmap(alloc_list.front().paddr()), 0, kPageSize);

  // Now append more pages to the list. If this runs the checker on the page already in the list
  // that we modified then it will panic.
  EXPECT_OK(node.node().AllocPages(node.kNumPages / 2, 0, &alloc_list));

  END_TEST;
}

#if __has_feature(address_sanitizer)
static bool kasan_detects_use_after_free() {
  BEGIN_TEST;
  // TODO(https://fxbug.dev/42104852): Enable on arm64 when kasan poisoning works there.
#if defined(__x86_64__)
  ManagedPmmNode node;

  zx::result<vm_page_t*> result = node.node().AllocPage(PMM_ALLOC_FLAG_ANY);
  ASSERT_EQ(ZX_OK, result.status_value(), "pmm_alloc_page one page");
  vm_page_t* page = result.value();
  paddr_t paddr = page->paddr();
  ASSERT_NE(paddr, 0UL);
  EXPECT_EQ(0UL, asan_region_is_poisoned(reinterpret_cast<uintptr_t>(paddr_to_physmap(paddr)),
                                         kPageSize));
  node.node().FreePage(page);
  EXPECT_TRUE(asan_entire_region_is_poisoned(reinterpret_cast<uintptr_t>(paddr_to_physmap(paddr)),
                                             kPageSize));
#endif
  END_TEST;
}
#endif  // __has_feature(address_sanitizer)

static bool pmm_page_to_from_index_test() {
  // Assert that indexes have zero bits, can roundtrip and are distinct for distinct pages.
  BEGIN_TEST;

  paddr_t pa;
  vm_page_t* page0;
  zx_status_t status = pmm_alloc_page(0, &page0, &pa);
  ASSERT_EQ(ZX_OK, status, "pmm_alloc single page");
  uint32_t index0 = Pmm::Node().PageToIndex(page0);
  ASSERT_NE(0u, index0);
  uint32_t zero_bits_mask = (1u << PmmNode::kIndexZeroBits) - 1;
  ASSERT_EQ(0u, index0 & zero_bits_mask);
  vm_page_t* same_page = Pmm::Node().IndexToPage(index0);
  ASSERT_EQ(page0, same_page);
  ASSERT_EQ(page0->paddr(), Pmm::Node().IndexToPaddr(index0));

  vm_page_t* page1;
  status = pmm_alloc_page(0, &page1, &pa);
  ASSERT_EQ(ZX_OK, status, "pmm_alloc single page");
  uint32_t index1 = Pmm::Node().PageToIndex(page1);
  ASSERT_NE(0u, index1);

  ASSERT_NE(index0, index1);

  pmm_free_page(page0);
  pmm_free_page(page1);
  END_TEST;
}

// Test reporting allocation failures and latching the first failure.
static bool pmm_node_alloc_failure_reporting_test() {
  BEGIN_TEST;

  ManagedPmmNode node;

  // Initially, no allocation failure should be recorded.
  EXPECT_FALSE(node.node().has_alloc_failed_no_mem());
  PmmNode::AllocFailure initial_failure = node.node().GetFirstAllocFailure();
  EXPECT_EQ(PmmNode::AllocFailure::Type::None, initial_failure.type);
  EXPECT_EQ(0u, initial_failure.size);

  // Report a first allocation failure.
  PmmNode::AllocFailure failure1 = {
      .type = PmmNode::AllocFailure::Type::Heap,
      .size = 1024,
      .free_count = 10,
  };
  node.node().ReportAllocFailure(failure1);

  EXPECT_TRUE(node.node().has_alloc_failed_no_mem());
  PmmNode::AllocFailure recorded_failure = node.node().GetFirstAllocFailure();
  EXPECT_EQ(PmmNode::AllocFailure::Type::Heap, recorded_failure.type);
  EXPECT_EQ(1024u, recorded_failure.size);
  // For non-PMM types, free_count is overwritten with the current node's free_count_.
  EXPECT_EQ(ManagedPmmNode::kNumPages, recorded_failure.free_count);

  // Report a second allocation failure with different parameters.
  PmmNode::AllocFailure failure2 = {
      .type = PmmNode::AllocFailure::Type::Pmm,
      .size = 4096,
      .free_count = 5,
  };
  node.node().ReportAllocFailure(failure2);

  // The node should still retain the first recorded failure.
  PmmNode::AllocFailure latched_failure = node.node().GetFirstAllocFailure();
  EXPECT_EQ(PmmNode::AllocFailure::Type::Heap, latched_failure.type);
  EXPECT_EQ(1024u, latched_failure.size);
  EXPECT_EQ(ManagedPmmNode::kNumPages, latched_failure.free_count);

  END_TEST;
}

UNITTEST_START_TESTCASE(pmm_tests)
VM_UNITTEST(pmm_smoke_test)
VM_UNITTEST(pmm_alloc_contiguous_one_test)
VM_UNITTEST(pmm_node_multi_alloc_test)
VM_UNITTEST(pmm_node_singleton_list_test)
VM_UNITTEST(pmm_node_loan_borrow_cancel_reclaim_end)
VM_UNITTEST(pmm_node_oversized_alloc_test)
VM_UNITTEST(pmm_node_free_mem_event_test)
VM_UNITTEST(pmm_node_low_mem_alloc_failure_test)
VM_UNITTEST(pmm_node_alloc_failure_reporting_test)
VM_UNITTEST(pmm_node_explicit_should_wait_test)
VM_UNITTEST(pmm_node_stop_returning_should_wait_test)
VM_UNITTEST(pmm_node_stop_returning_should_wait_concurrent_test)
VM_UNITTEST(pmm_node_suspendable_wait_test)
VM_UNITTEST(pmm_node_non_suspendable_wait_test)
VM_UNITTEST(pmm_node_killed_wait_test)
VM_UNITTEST(pmm_node_suspend_then_killed_wait_test)
VM_UNITTEST(pmm_checker_test)
VM_UNITTEST(pmm_checker_is_valid_fill_size_test)
VM_UNITTEST(pmm_get_arena_info_test)
VM_UNITTEST(pmm_arena_find_free_contiguous_test)
VM_UNITTEST(pmm_alloc_append_test)
VM_UNITTEST(pmm_page_to_from_index_test)
UNITTEST_END_TESTCASE(pmm_tests, "pmm", "Physical memory manager tests")

#if __has_feature(address_sanitizer)
UNITTEST_START_TESTCASE(kasan_pmm_tests)
VM_UNITTEST(kasan_detects_use_after_free)
UNITTEST_END_TESTCASE(kasan_pmm_tests, "kasan_pmm", "kasan pmm tests")
#endif  // __has_feature(address_sanitizer)

}  // namespace vm_unittest
