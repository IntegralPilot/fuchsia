// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "include/vm/physical_page_provider.h"

#include <lib/counters.h>
#include <lib/dump/depth_printer.h>
#include <lib/fit/defer.h>
#include <lib/fit/result.h>
#include <lib/page/size.h>
#include <trace.h>

#include <kernel/range_check.h>
#include <object/thread_dispatcher.h>

#define LOCAL_TRACE 0

KCOUNTER(physical_reclaim_total_requests, "physical.reclaim.total_requests")
KCOUNTER(physical_reclaim_succeeded_requests, "physical.reclaim.succeeded_requests")
KCOUNTER(physical_reclaim_failed_requests, "physical.reclaim.failed_requests")

PhysicalPageProvider::PhysicalPageProvider(uint64_t size) : size_(size) { LTRACEF("\n"); }

PhysicalPageProvider::~PhysicalPageProvider() {
  LTRACEF("%p\n", this);
  // Possible we were destructed without being initialized and in error paths we can destruct
  // without detached_ or closed_ becoming true, so cannot check for those.
  if (phys_base_ == kInvalidPhysBase) {
    // If a failure is encountered during creation, we might have accumulated some pages which need
    // to be returned to the PMM. Initialization isn't complete and these pages haven't been used
    // yet, so no need to unloan and no need to delay reuse either.
    if (unlikely(!free_list_.is_empty())) {
      Pmm::Node().FreeList(&free_list_, PmmOptDelayReuse::Default);
    }
    return;
  }

  // To return our pages first retrieve any pages that are loaned, before returning everything to
  // the PMM. This is inefficient as instead of retrieving the loaned page only to put it back in
  // the PMM we could unloan the page 'in place' where it is, however contiguous VMOs that are
  // loaning pages are not expected to be destructed often (or at all) and so this is not presently
  // a needed optimization.
  UnloanRange(0, size_, &free_list_);
  ASSERT(free_list_.size_slow() == size_ / kPageSize);

  const PmmOptDelayReuse delay_reuse =
      cow_pages_ ? cow_pages_->should_delay_reuse_on_free() : PmmOptDelayReuse::Default;
  Pmm::Node().FreeList(&free_list_, delay_reuse);
}

PageSourceProperties PhysicalPageProvider::properties() const {
  return PageSourceProperties{
      .is_user_pager = false,
      .supports_request_type = {true, false, false},
  };
}

void PhysicalPageProvider::Init(VmCowPages* cow_pages, PageSource* page_source, paddr_t phys_base) {
  DEBUG_ASSERT(cow_pages);
  DEBUG_ASSERT(!IsPageRounded(kInvalidPhysBase));
  DEBUG_ASSERT(IsPageRounded(phys_base));
  DEBUG_ASSERT(!cow_pages_);
  DEBUG_ASSERT(phys_base_ == kInvalidPhysBase);
  Guard<Mutex> guard{&mtx_};
  cow_pages_ = cow_pages;
  page_source_ = page_source;
  phys_base_ = phys_base;
}

// Called under lock of contiguous VMO that needs the pages.  The request is later processed at the
// start of WaitOnEvent.
void PhysicalPageProvider::SendAsyncRequest(PageRequest* request) {
  DEBUG_ASSERT(phys_base_ != kInvalidPhysBase);
  DEBUG_ASSERT(GetRequestType(request) == page_request_type::READ);
  Guard<Mutex> guard{&mtx_};
  ASSERT(!closed_);

  // PhysicalPageProvider always operates async (similar to PagerProxy), because we'd like to (in
  // typical non-overlapping commit/decommit usage) have one batch that covers the entire commit,
  // regardless of the fact that some of the pages may already be free and therefore could be
  // immediately obtained.  Quite often at least one page will be presently owned by a different
  // VMO, so we may as well always do one big async batch that deals with all the presently
  // non-FREE_LOANED pages.
  //
  // At this point the page may be FREE_LOANED, or in use by a different VMO.
  //
  // Allocation of a new page to a VMO has an interval during which the page is not free, but also
  // isn't state == OBJECT yet.  During processing we rely on that interval occurring only under the
  // other VMO's lock, but we can't acquire the other VMO's lock here since we're already currently
  // holding the underlying owning contiguous VMO's lock.
  QueueRequestLocked(request);
}

void PhysicalPageProvider::QueueRequestLocked(PageRequest* request) {
  DEBUG_ASSERT(phys_base_ != kInvalidPhysBase);
  DEBUG_ASSERT(GetRequestType(request) == page_request_type::READ);
  ASSERT(!closed_);
  pending_requests_.push_back(request);
}

void PhysicalPageProvider::ClearAsyncRequest(PageRequest* request) {
  DEBUG_ASSERT(phys_base_ != kInvalidPhysBase);
  DEBUG_ASSERT(GetRequestType(request) == page_request_type::READ);
  Guard<Mutex> guard{&mtx_};
  ASSERT(!closed_);

  if (fbl::InContainer<PageProviderTag>(*request)) {
    pending_requests_.erase(*request);
  }

  // No need to chase down any currently-processing request here, since before processing a request,
  // we stash the values of all fields we need from the PageRequest under the lock.  So any
  // currently-processing request is independent from the PageRequest that started it.
}

void PhysicalPageProvider::SwapAsyncRequest(PageRequest* old, PageRequest* new_req) {
  DEBUG_ASSERT(phys_base_ != kInvalidPhysBase);
  DEBUG_ASSERT(GetRequestType(old) == page_request_type::READ);
  DEBUG_ASSERT(GetRequestType(new_req) == page_request_type::READ);
  Guard<Mutex> guard{&mtx_};
  ASSERT(!closed_);

  if (fbl::InContainer<PageProviderTag>(*old)) {
    pending_requests_.insert(*old, new_req);
    pending_requests_.erase(*old);
  }
}

void PhysicalPageProvider::FreePages(VmPageDoublyLinkedList* pages) {
  {
    // Check if we are detached, and if so put the pages straight in the free_list_ instead of
    // loaning them out, since we will be being closed soon and will only have to go and retrieve
    // them.
    // Also check for the case where we're not fully initialized yet. It's possible to fail partway
    // through inserting pages during creation, which will require freeing any already inserted
    // pages back to the PMM.
    Guard<Mutex> guard{&mtx_};
    if (DetachedLocked() || phys_base_ == kInvalidPhysBase) {
      free_list_.splice(free_list_.end(), *pages);
      return;
    }
  }
  // This should always be called in a way that is serialized with other operations on our
  // cow_pages_, otherwise there is a race where an operation on the cow_pages_ could observe an
  // absence of pages, but then be unable to retrieve them because it is not synchronized with this
  // FreePages call. Both parties using the paged_vmo_lock serves to create a synchronization that
  // avoids this scenario.
  ASSERT(page_source_);
  AssertHeld(*page_source_->paged_vmo_lock());
  // This marks the pages loaned, and makes them FREE_LOANED for potential use by other clients that
  // are ok with getting loaned pages when allocating. Must hold the loaned_state_lock_ as we are
  // manipulating the loaned state of pages that could get inspected by UnloanRange due to
  // interactions with cancelled page requests.
  Guard<Mutex> guard{&loaned_state_lock_};
  const PmmOptDelayReuse delay_reuse =
      cow_pages_ ? cow_pages_->should_delay_reuse_on_free() : PmmOptDelayReuse::Default;
  Pmm::Node().BeginLoan(pages, delay_reuse);
}

bool PhysicalPageProvider::DebugIsPageOk(vm_page_t* page, uint64_t offset) {
  Guard<Mutex> guard{&mtx_};
  DEBUG_ASSERT((cow_pages_ != nullptr) == (phys_base_ != kInvalidPhysBase));
  // Assume pages added before we know the cow_pages_ or phys_base_ are ok.
  if (!cow_pages_) {
    return true;
  }
  return (page->paddr() - phys_base_) == offset;
}

void PhysicalPageProvider::OnDetach() {
  // Its possible for destruction to happen prior to initialization completing, and this can lead
  // to OnDetach being called from what would be the cow_pages_, but prior to |Init| being called
  // and the cow_pages_ pointer being setup. In this case we can safely ignore locking requirements
  // and just set detached.
  Guard<Mutex> guard{&mtx_};
  ASSERT(!closed_);
  if (phys_base_ == kInvalidPhysBase) {
    ASSERT(!cow_pages_);
    // As we cannot assert the cow_pages_ lock, due to it being a nullptr, we must temporarily
    // disable analysis.
    [&]() TA_NO_THREAD_SAFETY_ANALYSIS { detached_ = true; }();
  } else {
    // The current synchronization strategy relies on OnDetach being called with the VMO lock being
    // held. This allows us to assume that the detached_ flag only transitions under the VmCowPages
    // lock.
    AssertHeld(cow_pages_->lock_ref());
    detached_ = true;
  }
}

void PhysicalPageProvider::OnClose() {
  Guard<Mutex> guard{&mtx_};
  ASSERT(!closed_);
  closed_ = true;
}

bool PhysicalPageProvider::DequeueRequest(uint64_t* request_offset, uint64_t* request_length) {
  Guard<Mutex> guard{&mtx_};
  // closed_ can be true here, but if closed_ is true, then pending_requests_ is also empty, so
  // we won't process any more requests once closed_ is true. However, there is also no point in
  // processing requests if we have detached, as these requests will be cancelled anyhow.
  DEBUG_ASSERT(!closed_ || pending_requests_.is_empty());
  if (pending_requests_.is_empty() || DetachedLocked()) {
    // Done with all requests (or remaining requests cancelled).
    return false;
  }
  PageRequest* request = pending_requests_.pop_front();
  DEBUG_ASSERT(request);
  DEBUG_ASSERT(GetRequestType(request) == page_request_type::READ);
  *request_offset = GetRequestOffset(request);
  *request_length = GetRequestLen(request);
  DEBUG_ASSERT(InRange(*request_offset, *request_length, size_));
  return true;
}

void PhysicalPageProvider::UnloanRange(uint64_t range_offset, uint64_t length,
                                       VmPageDoublyLinkedList* pages) {
  // Notify borrowing config that an unloan is starting. This temporarily disables background
  // page borrowing/sweeping in ProcessLruQueue to avoid lock contention on the borrowing VMOs'
  // paged_vmo_lock_ (which are part of the active working set) between the LRU thread and this
  // unloaning thread.
  PhysicalPageBorrowingConfig::Get().NotifyUnloanStarted();
  auto cleanup = fit::defer([] { PhysicalPageBorrowingConfig::Get().NotifyUnloanFinished(); });

  Guard<Mutex> guard{&loaned_state_lock_};
  // Evict needed physical pages from other VMOs, so that needed physical pages become free.  This
  // is iterating over the destination offset in cow_pages_.  The needed pages can be scattered
  // around in various VMOs and offsets of those VMOs and, by the time we get to looking at them,
  // could even already be returned to the PMM and free.
  uint64_t range_end = range_offset + length;
  for (uint64_t offset = range_offset; offset < range_end; offset += kPageSize) {
    vm_page_t* page = paddr_to_vm_page(phys_base_ + offset);
    DEBUG_ASSERT(page);
    // Page should never have entered the regular FREE state as it should either be loaned out in a
    // VMO, in the FREE_LOANED state, or owned by us in our VmCowPages.
    DEBUG_ASSERT(!page->is_free());
    // If the page is not currently loaned out, then skip, our work here is done.
    if (!page->is_loaned()) {
      continue;
    }
    // Cancel the loan allowing us to track the page down.
    Pmm::Node().CancelLoan(page);

    // Cancelling the loan took the loaned pages lock and so just prior to that completing we knew
    // that every page was either:
    //  1. Found in the LOANED_FREE state, and is therefore still in that state.
    //  2. Completely installed in a VMO with a valid backlink.
    if (!page->is_free_loaned()) {
      // Between cancelling the loan and now, the page could be in the progress of migrating back
      // to the PMM. If we just perform GetCowForLoanedPage then we could observe a scenario where
      // the page is still in the OBJECT state, but has its backlink cleared. To avoid this we
      // perform the lookup under the loaned pages lock, ensuring we either see the page while it
      // is still in the VMO, with a valid backlink, or after it has fully migrated back to the
      // PMM.
      ktl::optional<PageQueues::VmoBacklink> maybe_vmo_backlink;
      Pmm::Node().WithLoanedPage(page, [&maybe_vmo_backlink](vm_page_t* page) {
        maybe_vmo_backlink = pmm_page_queues()->GetCowForLoanedPage(page);
      });
      // Either we got a backlink, or the page is already back in the PMM.
      DEBUG_ASSERT(maybe_vmo_backlink || page->is_free_loaned());
      if (maybe_vmo_backlink) {
        // As we will be calling back into the VMO we want to drop the loaned state lock to both
        // avoid lock ordering issues, and to not excessively hold the lock. During this section we
        // are not modifying or inspecting the loaned state so the lock is not needed. The cow page
        // we call may inspect the loaned state, but it does so iff it knows it is the owner and so
        // it may safely do so.
        guard.CallUnlocked([&] {
          auto& vmo_backlink = maybe_vmo_backlink.value();
          DEBUG_ASSERT(vmo_backlink.cow);
          auto& cow_container = vmo_backlink.cow;

          DEBUG_ASSERT(!page->object.always_need);
          bool needs_evict = true;

          // Check if we should attempt to replace the page to avoid eviction.
          if (PhysicalPageBorrowingConfig::Get().is_replace_on_unloan_enabled()) {
            __UNINITIALIZED AnonymousPageRequest page_request;
            zx_status_t replace_result = cow_container->ReplacePage(page, vmo_backlink.offset,
                                                                    false, nullptr, &page_request);
            // If replacement failed for any reason, fall back to eviction. If replacement succeeded
            // then the page got directly returned to the pmm.
            needs_evict = replace_result != ZX_OK;
            if (replace_result == ZX_ERR_SHOULD_WAIT) {
              page_request.Cancel();
            }
            // If replacement succeeded, i.e. we are not going to fall back to eviction, then the
            // page should be back in the PMM.
            DEBUG_ASSERT(needs_evict || page->is_free_loaned());
          }
          if (needs_evict) {
            [[maybe_unused]] zx_status_t reclaimed =
                cow_container->EvictLoanedPage(page, vmo_backlink.offset);
            // Either we succeeded eviction, or another thread raced and did it first. If another
            // thread did it first then it would have done so under the VMO lock, which we have
            // since acquired, and so we know the page is either on the way (in a
            // FreeLoanedPagesHolder) or in the PMM. We can ensure the page is fully migrated to the
            // PMM by waiting for any holding to be concluded.
            DEBUG_ASSERT(reclaimed != ZX_OK || page->is_free_loaned());
            if (!page->is_free_loaned()) {
              Pmm::Node().WithLoanedPage(page, [](vm_page_t* page) {});
              DEBUG_ASSERT(page->is_free_loaned());
            }
          }
        });
      }
      // For all the scenarios, no backlink, successful replacement or eviction attempts, the page
      // must have ended up in the PMM.
      ASSERT(page->is_free_loaned());
    }
    // Now that the page is definitely in the FREE_LOANED state, gain ownership from the PMM.
    Pmm::Node().EndLoan(page);
    DEBUG_ASSERT(page->state() == vm_page_state::ALLOC);
    pages->push_back(page);
  }  // for pages of request
}

// TODO(https://fxbug.dev/42084841): Reason about the use of |suspendable|, ignored for now.
zx_status_t PhysicalPageProvider::WaitOnEvent(Event* event,
                                              bool suspendable) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Before processing any events we synchronize with OnDetach and retrieve a RefPtr to the
  // cow_pages_. This ensures we can safely dereference the cow_pages_ later on, knowing we are not
  // racing with its destructor.
  fbl::RefPtr<VmCowPages> cow_ref;
  {
    Guard<Mutex> guard{&mtx_};
    if (DetachedLocked()) {
      return ZX_OK;
    }
    cow_ref = fbl::MakeRefPtrUpgradeFromRaw(cow_pages_, mtx_);
    // As we hold the lock, and had not yet detached, the cow_pages_ must not have run
    // OnDeadTransition, and so it must still be possible to get a ref to it.
    ASSERT(cow_ref);
  }
  // When WaitOnEvent is called, we know that the event being waited on is associated with a request
  // that's already been queued, so we can use this thread to process _all_ the queued requests
  // first, and then wait on the event which then won't have any reason to block this thread, since
  // every page of every request that existed on entry to this method has been succeeded or failed
  // by the time we wait on the passed-in event.
  uint64_t request_offset;
  uint64_t request_length;
  while (DequeueRequest(&request_offset, &request_length)) {
    DEBUG_ASSERT(request_offset + request_length > request_offset);

    // These are ordered by cow_pages_ offsets (destination offsets), but may have gaps due to not
    // all the pages being loaned.
    VmPageDoublyLinkedList unloaned_pages;
    UnloanRange(request_offset, request_length, &unloaned_pages);

    VmPageDoublyLinkedList contiguous_pages;
    // Process all the loaned pages by finding any contiguous runs and processing those as a batch.
    // There is no correctness reason to process them in batches, but it is more efficient for the
    // cow_pages_ to receive a run where possible instead of repeatedly being given single pages.
    while (!unloaned_pages.is_empty()) {
      vm_page_t* page = &unloaned_pages.front();
      if (contiguous_pages.is_empty() ||
          contiguous_pages.back().paddr() + kPageSize == page->paddr()) {
        unloaned_pages.pop_front();
        contiguous_pages.push_back(page);
        // Generally want to keep trying to find more contiguous pages, unless there are no more
        // pages.
        if (!unloaned_pages.is_empty()) {
          continue;
        }
      }

      // An interfering decommit can occur after we've moved these pages into VmCowPages, but not
      // yet moved the entire commit request into VmCowPages.  If not all pages end up present in
      // cow_pages_ on return to the user from the present commit, due to concurrent decommit,
      // that's just normal commit semantics.
      //
      // Supply the pages we got to cow_pages_.  Also tell it what range to claim is supplied now
      // for convenience.
      //
      // If there's an interfering decommit, then that decommit can only interfere after we've added
      // the pages to VmCowPages, so isn't an immediate concern here.
      //
      // We want to use VmCowPages::SupplyPages() to avoid a proliferation of VmCowPages code that
      // calls OnPagesSupplied() / OnPagesFailed(), so to call SupplyPages() we need a
      // VmPageSpliceList.  We put all the pages in the "head" portion of the VmPageSpliceList since
      // there are no VmPageListNode(s) involved in this path.  We also zero the pages here, since
      // SupplyPages() doesn't do that.
      //
      // We can zero the pages before we supply them, which avoids holding the VmCowPages::lock_
      // while zeroing, and also allows us to flush the zeroes to RAM here just in case any client
      // is (incorrectly) assuming that non-pinned pages necessarily remain cache clean once they
      // are cache clean.
      uint64_t supply_length = 0;
      for (auto& p : contiguous_pages) {
        void* ptr = paddr_to_physmap(p.paddr());
        DEBUG_ASSERT(ptr);
        arch_zero_page(ptr);
        supply_length += kPageSize;
        arch_clean_invalidate_cache_range(reinterpret_cast<vaddr_t>(ptr), kPageSize);
      }
      uint64_t supply_offset = contiguous_pages.front().paddr() - phys_base_;

      VmPageSpliceList splice_list;

      // Any pages that do not get supplied for any reason, due to failures from supply or because
      // we got detached, should be re-loaned.
      auto loan_pages = fit::defer([&] {
        while (!splice_list.IsProcessed()) {
          VmPageOrMarker page_or_marker = splice_list.Pop();
          if (page_or_marker.IsPage()) {
            vm_page_t* p = page_or_marker.ReleasePage();
            DEBUG_ASSERT(!p->queue_node.InContainer());
            contiguous_pages.push_back(p);
          }
        }
        if (!contiguous_pages.is_empty()) {
          Guard<Mutex> guard{&loaned_state_lock_};
          // We're about to return these pages to the PMM.  There is no need to delay their reuse
          // because we know these pages would have never been involved in a DMA, because they have
          // never been pinned, because we don't pin loaned pages.  See
          // |VmCowPages::ReplacePagesWithNonLoanedLocked|.
          const PmmOptDelayReuse delay_reuse = PmmOptDelayReuse::Default;
          Pmm::Node().BeginLoan(&contiguous_pages, delay_reuse);
        }
      });

      zx_status_t status =
          VmPageSpliceList::CreateFromPageList(supply_length, &contiguous_pages, &splice_list);
      if (status != ZX_OK) {
        // Only possible error is out of memory.
        ASSERT(status == ZX_ERR_NO_MEMORY);
        DEBUG_ASSERT(PageSource::IsValidInternalFailureCode(status));
        page_source_->OnPagesFailed(supply_offset, supply_length, status);
        // Do not attempt to then supply the pages, move to the next range.
        continue;
      }
      DEBUG_ASSERT(contiguous_pages.is_empty());

      // First take the VMO lock before taking our lock to ensure lock ordering is correct. As we
      // hold a RefPtr we know that even if racing with OnClose this is a valid object.
      VmCowPages::DeferredOps deferred(cow_pages_);
      Guard<CriticalMutex> cow_lock{cow_pages_->lock()};
      bool detached;
      // Now take our lock and check to see if we have been detached.
      {
        Guard<Mutex> guard{&mtx_};
        detached = detached_;
      }
      // We can use the cached value of detached_ since OnDetach only runs with the VMO lock held,
      // which we presently hold. This means that if detached is false we can safely call VMO
      // methods knowing that we are not racing with any detach or close attempts.
      if (!detached) {
        // The splice list being inserted only contains true vm_page_t, so we don't need to call
        // ProcessPagesForSupply do dereference them.
        zx_status_t supply_result =
            cow_pages_->SupplyPagesLocked(VmCowRange(supply_offset, supply_length), &splice_list,
                                          SupplyOptions::PhysicalPageProvider, deferred, nullptr);
        if (supply_result != ZX_OK) {
          // Supply can only fail due to being out of memory as we currently hold the lock and know
          // that it cannot be racing with a detach for close.
          DEBUG_ASSERT(supply_result == ZX_ERR_NO_MEMORY);
          DEBUG_ASSERT(PageSource::IsValidInternalFailureCode(supply_result));
          page_source_->OnPagesFailed(supply_offset, supply_length, supply_result);
        }
      }
    }
    DEBUG_ASSERT(contiguous_pages.is_empty() && unloaned_pages.is_empty());
  }  // while have requests to process

  kcounter_add(physical_reclaim_total_requests, 1);
  // The event now should be in one of three states:
  //  1. We processed the related request above in the loop, and the event got signaled as a
  //     consequence of the pages we supplied.
  //  2. A different thread dequeued the request and either processed it, or is still processing it.
  //  3. The request is in the process of being cancelled and the underlying request packet got
  //     dequeued, and the event has or will be signaled.
  // In all cases it is a *kernel* thread under our control that should signal the thread, and so
  // although we may need to wait, only a kernel bug should cause this to block indefinitely.
  // To attempt to detect such bugs we wait with a generous timeout before making some noise.
  constexpr zx_duration_t kReportWaitTime = ZX_SEC(60);
  zx_status_t wait_result = ZX_OK;
  uint32_t waited = 0;
  while ((wait_result = event->Wait(Deadline::after_mono(kReportWaitTime))) == ZX_ERR_TIMED_OUT) {
    waited++;
    printf("WARNING: PhysicalPageProvider has waited %" PRIi64 " seconds on event.\n",
           (kReportWaitTime * waited) / ZX_SEC(1));
  }
  if (wait_result == ZX_OK) {
    kcounter_add(physical_reclaim_succeeded_requests, 1);
  } else {
    kcounter_add(physical_reclaim_failed_requests, 1);
  }
  return wait_result;
}

void PhysicalPageProvider::Dump(uint depth, uint32_t max_items) {
  Guard<Mutex> guard{&mtx_};
  dump::DepthPrinter printer(depth);
  printer.Emit("physical_page_provider %p cow_pages_ %p phys_base_ 0x%" PRIx64 " closed %d", this,
               cow_pages_, phys_base_, closed_);
  printer.BeginList(max_items);
  for (auto& req : pending_requests_) {
    DEBUG_ASSERT(GetRequestType(&req) == page_request_type::READ);
    printer.Emit("  pending req [0x%lx, 0x%lx)", GetRequestOffset(&req), GetRequestLen(&req));
  }
  printer.EndList();
}
