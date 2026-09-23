// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSICAL_PAGE_PROVIDER_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSICAL_PAGE_PROVIDER_H_

#include <sys/types.h>
#include <zircon/types.h>

#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>
#include <object/port_dispatcher.h>
#include <vm/page_source.h>

// Page provider implementation that provides requested loaned physical pages.
//
// This is used by contiguous VMOs which have had pages decommitted, when the pages are again
// committed.  The reason we use a PageProvider for this is it lines up well with the pager model in
// the sense that a page request can be processed while not holding the contiguous VMO's lock.
//
// PhysicalPageProvider always operates async (similar to PagerProxy), because we'd like to (in
// typical non-overlapping commit/decommit usage) have one batch that covers the entire commit,
// regardless of the fact that some of the pages may already be free and therefore could be
// immediately obtained.  Quite often at least one page will be presently owned by a different
// VMO, so we may as well always do one big async batch that deals with all the presently
// non-FREE pages.
class PhysicalPageProvider : public PageProvider {
 public:
  explicit PhysicalPageProvider(uint64_t size);
  ~PhysicalPageProvider() override;

  // Called shortly after construction, before any use.
  void Init(VmCowPages* cow_pages, PageSource* page_source, paddr_t phys_base);

 private:
  // PageProvider methods.
  PageSourceProperties properties() const final;
  void SendAsyncRequest(PageRequest* request) final;
  void ClearAsyncRequest(PageRequest* request) final;
  void SwapAsyncRequest(PageRequest* old, PageRequest* new_req) final;
  void FreePages(VmPageDoublyLinkedList* pages) final;
  bool DebugIsPageOk(vm_page_t* page, uint64_t offset) final;
  // This also calls pmm_delete_lender()
  void OnClose() final;
  void OnDetach() final;
  // Before actually waiting on the event, uses the calling thread (which isn't holding any locks)
  // to process all the requests in pending_requests_.
  zx_status_t WaitOnEvent(Event* event, bool suspendable) final;

  void Dump(uint depth, uint32_t max_items) final;

  // If there are no more requests in pending_requests_, returns false and doesn't modify the out
  // parameters.
  //
  // If pending_requests_ has another request, returns true and sets *reqeust_offset to the offset
  // and *request_length to the length.
  bool DequeueRequest(uint64_t* request_offset, uint64_t* request_length);

  // Helper that unloans any loaned pages in the specified range and puts them, in offset order, in
  // |pages|. Any pages that are not loaned will not be returned, and the caller must tolerate gaps
  // in the list.
  void UnloanRange(uint64_t offset, uint64_t length, VmPageDoublyLinkedList* pages);

  static constexpr paddr_t kInvalidPhysBase = static_cast<paddr_t>(-1);
  // Only written once, under mtx_, just after construction.  Reads after that don't need to be
  // under mtx_.
  //
  // This is set to the physical base address of the associated VmCowPages.  We track this here
  // and not in VmCowPages because VmCowPages intentionally doesn't directly know about contiguous
  // concerns such as phys_base_.
  paddr_t phys_base_ = kInvalidPhysBase;
  // Set to the total size in bytes of the associated VmCowPages, to allow for asserts that requests
  // are in range.  While we could ask VmCowPages each time, the size_ doesn't change for contiguous
  // VMOs, so caching here keeps things simpler (for now).
  const uint64_t size_ = 0;

  VmCowPages* cow_pages_ = nullptr;
  PageSource* page_source_ = nullptr;

  mutable DECLARE_MUTEX(PhysicalPageProvider) mtx_;

  // Loaned pages should only have their loaned state manipulated by their 'owner', and should only
  // have their loaned state inspected when it is known to be stable. Due to the cancellable nature
  // of page requests we generally cannot know when inspecting a page if it is actually loaned out,
  // or was previously loaned out but this is an old page requests and we could be racing with it
  // no longer being loaned (or potentially be loaned again). As such we use this lock to ensure
  // that, whenever we cannot be certain that there is no other actor that could be manipulating or
  // inspecting the loaned state, we force a serialization.
  mutable DECLARE_MUTEX(PhysicalPageProvider) loaned_state_lock_;

  // To cease loaning pages all pages first become owned by the physical page provider and get
  // collected in |free_list_|, upon which they can be returned (i.e. freed) to the PMM.
  VmPageDoublyLinkedList free_list_ TA_GUARDED(mtx_);

  // Queue of page_request_t's that have come in while packet_ is busy. The
  // head of this queue is sent to the port when packet_ is freed.
  fbl::TaggedDoublyLinkedList<PageRequest*, PageProviderTag> pending_requests_ TA_GUARDED(mtx_);

  // Modifications to detached are guarded by both our lock and the cow_pages_ lock to provide us a
  // way to synchronize the processing of requests (which happen in a different thread context) with
  // becoming detached, as these can otherwise happen concurrently. Through the use of detached the
  // WaitOnEvent logic can ensure that when it calls back into the cow_pages_ it is not racing with
  // OnDeadTransition and potentially supply pages to closed or closing VMO. See comments in
  // WaitOnEvent and OnDetach for more.
  bool detached_ TA_GUARDED(mtx_) TA_GUARDED(cow_pages_->lock()) = false;

  bool closed_ TA_GUARDED(mtx_) = false;

  // Helper for reading detached when holding our lock. This is safe since if holding at least mtx_
  // the value cannot change.
  bool DetachedLocked() const TA_REQ(mtx_) TA_NO_THREAD_SAFETY_ANALYSIS { return detached_; }

  // Queues the page request, putting it in pending_requests_;
  void QueueRequestLocked(PageRequest* request) TA_REQ(mtx_);
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSICAL_PAGE_PROVIDER_H_
