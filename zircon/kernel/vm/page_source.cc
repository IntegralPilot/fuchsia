// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/console.h>
#include <lib/counters.h>
#include <lib/dump/depth_printer.h>
#include <lib/page/size.h>
#include <trace.h>
#include <zircon/syscalls/port.h>

#include <kernel/lockdep.h>
#include <ktl/utility.h>
#include <vm/page_source.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

namespace {

KCOUNTER(pgreq_overlap_0, "vm.page_request.overlap_0")
KCOUNTER(pgreq_overlap_1, "vm.page_request.overlap_1")
KCOUNTER(pgreq_overlap_2, "vm.page_request.overlap_2")
KCOUNTER(pgreq_overlap_3, "vm.page_request.overlap_3")
KCOUNTER(pgreq_overlap_4, "vm.page_request.overlap_4")
KCOUNTER(pgreq_overlap_5_to_10, "vm.page_request.overlap_5_to_10")
KCOUNTER(pgreq_overlap_more_than_10, "vm.page_request.overlap_more_than_10")

}  // namespace

PageSource::PageSource(fbl::RefPtr<PageProvider>&& page_provider)
    : page_provider_properties_(page_provider->properties()),
      page_provider_(ktl::move(page_provider)) {
  LTRACEF("%p\n", this);
}

PageSource::~PageSource() {
  LTRACEF("%p\n", this);
  DEBUG_ASSERT(detached_);
  DEBUG_ASSERT(closed_);
}

void PageSource::Detach() {
  canary_.Assert();
  LTRACEF("%p\n", this);
  Guard<Mutex> guard{&page_source_mtx_};
  if (detached_) {
    return;
  }

  detached_ = true;

  // Cancel all requests except writebacks, which can be completed after detach.
  for (uint8_t type = 0; type < page_request_type::COUNT; type++) {
    if (type == page_request_type::WRITEBACK || !SupportsPageRequestType(page_request_type(type))) {
      continue;
    }
    while (!outstanding_requests_[type].is_empty()) {
      auto req = outstanding_requests_[type].pop_front();
      LTRACEF("dropping request with offset %lx len %lx\n", req->offset_, req->len_);

      // Tell the clients the request is complete - they'll fail when they
      // reattempt the page request for the same pages after failing this time.
      CompleteRequestLocked(req);
    }
  }

  // No writebacks supported yet.
  DEBUG_ASSERT(outstanding_requests_[page_request_type::WRITEBACK].is_empty());

  page_provider_->OnDetach();
}

void PageSource::Close() {
  canary_.Assert();
  LTRACEF("%p\n", this);
  // TODO: Close will have more meaning once writeback is implemented

  // This will be a no-op if the page source has already been detached.
  Detach();

  Guard<Mutex> guard{&page_source_mtx_};
  if (closed_) {
    return;
  }

  closed_ = true;
  page_provider_->OnClose();
}

void PageSource::OnPagesSupplied(uint64_t offset, uint64_t len) {
  Guard<Mutex> guard{&page_source_mtx_};
  ResolveRequestsLocked(page_request_type::READ, offset, len, ZX_OK);
}

void PageSource::OnPagesDirtied(uint64_t offset, uint64_t len) {
  Guard<Mutex> guard{&page_source_mtx_};
  ResolveRequestsLocked(page_request_type::DIRTY, offset, len, ZX_OK);
}

void PageSource::EarlyWakeRequestLocked(PageRequest* request, uint64_t req_start,
                                        uint64_t req_end) {
  DEBUG_ASSERT(request);
  DEBUG_ASSERT(req_end >= req_start);
  DEBUG_ASSERT(req_start == request->wake_offset_);
  // By default set the wake_offset_ to the end of the supplied range so the caller can just
  // wait again. In practice they are likely to call GetPages first, which will reinitialize
  // wake_offset_ to what is actually the next desired offset, which may or may not be the
  // same as what is set here.
  request->wake_offset_ = req_end;
  request->event_.Signal(request->complete_status_);
  // For simplicity convert the request relative range back into a provider (aka VMO) range.
  const uint64_t provider_start = req_start + request->offset_;
  const uint64_t provider_end = req_end + request->offset_;
  for (PageRequest& overlap : request->overlap_) {
    if (!overlap.early_wake_) {
      continue;
    }
    // If the parent range has processed beyond the overlap wake offset then this is most
    // likely a sign of pages having not been linearly supplied, so want to wake up this
    // request anyway.
    if (provider_start > overlap.offset_ &&
        provider_start - overlap.offset_ > overlap.wake_offset_) {
      // In the case that something unusual has happened we do not want to keep on waking up the
      // request for any future completions, since it is waiting for wake_offset_ and either it
      // already got supplied and we missed it, which will get fixed by doing this signal, or it's
      // not been supplied and this (and any future) signal is a waste. To prevent future wakes we
      // set the wake_offset_ to a large so that we do not continuously signal as the parent request
      // is given more content. The largest obvious safe value, i.e. would not overflow anywhere,
      // is the end of the parent request range.
      overlap.wake_offset_ = request->len_;
      overlap.event_.Signal(overlap.complete_status_);
      continue;
    }
    // If there is otherwise no overlap, then can skip.
    if (!overlap.RangeOverlaps(provider_start, provider_end)) {
      continue;
    }
    // Get the overlapping portion and see if it intersects with the wake_offset_.
    auto [overlap_start, overlap_end] =
        overlap.TrimRangeToRequestSpace(provider_start, provider_end);
    if (overlap_start <= overlap.wake_offset_ && overlap_end > overlap.wake_offset_) {
      overlap.wake_offset_ = overlap_end;
      overlap.event_.Signal(overlap.complete_status_);
    }
  }
}

void PageSource::ResolveRequestsLocked(page_request_type type, uint64_t offset, uint64_t len,
                                       zx_status_t error_status) {
  canary_.Assert();
  LTRACEF_LEVEL(2, "%p offset %lx, len %lx\n", this, offset, len);
  uint64_t end;
  bool overflow = add_overflow(offset, len, &end);
  DEBUG_ASSERT(!overflow);  // vmobject should have already validated overflow
  DEBUG_ASSERT(type < page_request_type::COUNT);

  if (detached_) {
    return;
  }

  // The first possible request we could fulfill is the one with the smallest
  // end address that is greater than offset. Then keep looking as long as the
  // target request's start offset is less than the end.
  auto start = outstanding_requests_[type].upper_bound(offset);
  while (start.IsValid() && start->offset_ < end) {
    auto cur = start;
    ++start;

    // Because of upper_bound and our loop condition we know the range partially overlaps this
    // request.
    DEBUG_ASSERT(cur->RangeOverlaps(offset, end));

    // Calculate how many pages were resolved in this request by finding the start and
    // end offsets of the operation in this request.
    auto [req_offset, req_end] = cur->TrimRangeToRequestSpace(offset, end);

    if (error_status != ZX_OK) {
      if (req_offset == 0 || req_offset == cur->wake_offset_) {
        cur->complete_status_ = error_status;
      }
      for (PageRequest& overlap : cur->overlap_) {
        // If there is otherwise no overlap, then can skip.
        if (!overlap.RangeOverlaps(offset, end)) {
          continue;
        }
        auto [overlap_start, overlap_end] = overlap.TrimRangeToRequestSpace(offset, end);
        if (overlap_start == 0 || overlap_start == overlap.wake_offset_) {
          overlap.complete_status_ = error_status;
        }
      }
    }

    uint64_t fulfill = req_end - req_offset;

    // If we're not done, continue to the next request.
    if (fulfill < cur->pending_size_) {
      // Only Signal if the offset being supplied is exactly at the wake_offset_. The wake_offset_
      // is the next one that the caller wants, and so waking up for anything before this is
      // pointless. In the case where the page source supplies this offset last it does mean we will
      // still block until the full request is provided.
      if (req_offset == cur->wake_offset_) {
        EarlyWakeRequestLocked(&*cur, req_offset, req_end);
      }
      cur->pending_size_ -= fulfill;
      continue;
    } else if (fulfill > cur->pending_size_) {
      // This just means that part of the request was decommitted. That's not
      // an error, but it's good to know when we're tracing.
      LTRACEF("%p, excessive page count\n", this);
    }

    LTRACEF_LEVEL(2, "%p, signaling %lx\n", this, cur->offset_);

    // Notify anything waiting on this range.
    CompleteRequestLocked(outstanding_requests_[type].erase(cur));
  }
}

void PageSource::OnPagesFailed(uint64_t offset, uint64_t len, zx_status_t error_status) {
  canary_.Assert();
  LTRACEF_LEVEL(2, "%p offset %lx, len %lx\n", this, offset, len);

  DEBUG_ASSERT(PageSource::IsValidInternalFailureCode(error_status));

  uint64_t end;
  bool overflow = add_overflow(offset, len, &end);
  DEBUG_ASSERT(!overflow);  // vmobject should have already validated overflow

  Guard<Mutex> guard{&page_source_mtx_};
  if (detached_) {
    return;
  }

  for (uint8_t type = 0; type < page_request_type::COUNT; type++) {
    if (!SupportsPageRequestType(page_request_type(type))) {
      continue;
    }
    ResolveRequestsLocked(static_cast<page_request_type>(type), offset, len, error_status);
  }
}

// static
bool PageSource::IsValidExternalFailureCode(zx_status_t error_status) {
  switch (error_status) {
    case ZX_ERR_IO:
    case ZX_ERR_IO_DATA_INTEGRITY:
    case ZX_ERR_BAD_STATE:
    case ZX_ERR_NO_SPACE:
    case ZX_ERR_BUFFER_TOO_SMALL:
      return true;
    default:
      return false;
  }
}

// static
bool PageSource::IsValidInternalFailureCode(zx_status_t error_status) {
  switch (error_status) {
    case ZX_ERR_NO_MEMORY:
      return true;
    default:
      return IsValidExternalFailureCode(error_status);
  }
}

PageSource::ContinuationType PageSource::RequestContinuationTypeLocked(const PageRequest* request,
                                                                       uint64_t offset,
                                                                       uint64_t len,
                                                                       page_request_type type) {
  // Check for obvious mismatches in initialization.
  if (!request->IsInitialized()) {
    return ContinuationType::NotContinuation;
  }
  if (request->src_.get() != this) {
    return ContinuationType::NotContinuation;
  }
  if (request->type_ != type) {
    return ContinuationType::NotContinuation;
  }
  // If the start of the new range overlaps at all with the existing request then we can continue
  // using the existing request. For any portion of the new range that extends beyond the existing
  // request, this is fine as once the current request is completed that range can be re-generated
  // a new request for just that range can be made.
  if (offset >= request->offset_ && offset < request->offset_ + request->len_) {
    return ContinuationType::SameRequest;
  }
  // The new request is for a completely different range, so we cannot keep using the current one.
  // A typical cause for this would be racing with a operations on a clone that bypassed the need
  // to wait for the original request. In this case we already checked that the source and type are
  // the same above.
  return ContinuationType::SameSource;
}

zx_status_t PageSource::PopulateRequest(PageRequest* request, uint64_t offset, uint64_t len,
                                        VmoDebugInfo vmo_debug_info, page_request_type type) {
  canary_.Assert();
  DEBUG_ASSERT(request);
  DEBUG_ASSERT(len > 0);
  DEBUG_ASSERT(IsPageRounded(offset));
  DEBUG_ASSERT(IsPageRounded(len));

  if (!SupportsPageRequestType(type)) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  LTRACEF_LEVEL(2, "%p offset %" PRIx64 " prefetch_len %" PRIx64, this, offset, len);

  Guard<Mutex> guard{&page_source_mtx_};
  if (detached_) {
    return ZX_ERR_BAD_STATE;
  }

  if (request->IsInitialized()) {
    // The only time we should see an already initialized request is if the request was an early
    // waking one and this new request is a valid continuation of that one, anything else is a
    // programming error.
    DEBUG_ASSERT(request->early_wake_);
    switch (RequestContinuationTypeLocked(request, offset, len, type)) {
      case ContinuationType::NotContinuation:
        // If the request is initialized and the new request is not some kind of continuation then
        // we consider this a hard error. Although we could just cancel the existing request and
        // generate a new one, this case indicates a serious logic error in the page request
        // handling code and we should not attempt to continue.
        panic("Request at offset %" PRIx64 " len %" PRIx64 " is not any kind of continuation",
              offset, len);
        return ZX_ERR_INTERNAL;
      case ContinuationType::SameRequest:
        DEBUG_ASSERT(offset >= request->offset_);
        // By default the wake_offset_ was previously incremented by whatever was supplied, but to
        // accommodate a page source supply pages out of order we reset our wake offset to the next
        // actual offset that is missing.
        request->wake_offset_ = offset - request->offset_;
        return ZX_ERR_SHOULD_WAIT;
      case ContinuationType::SameSource:
        // The requested range does not overlap the existing request, but it's for the same source
        // so this is just a case of the original range no longer being needed and so can cancel the
        // request and make a new one.
        CancelRequestLocked(request);
        break;
    }
  }
  return PopulateRequestLocked(request, offset, len, vmo_debug_info, type);
}

void PageSource::FreePages(VmPageDoublyLinkedList* pages) { page_provider_->FreePages(pages); }

zx_status_t PageSource::PopulateRequestLocked(PageRequest* request, uint64_t offset, uint64_t len,
                                              VmoDebugInfo vmo_debug_info, page_request_type type) {
  DEBUG_ASSERT(request);
  DEBUG_ASSERT(IsPageRounded(offset));
  DEBUG_ASSERT(len > 0);
  DEBUG_ASSERT(IsPageRounded(len));
  DEBUG_ASSERT(type < page_request_type::COUNT);
  DEBUG_ASSERT(!request->IsInitialized());

  request->Init(fbl::RefPtr<PageRequestInterface>(this), offset, type, vmo_debug_info);

  // Assert on overflow, since it means vmobject is trying to get out-of-bounds pages.
  [[maybe_unused]] bool overflowed = add_overflow(request->len_, len, &request->len_);
  DEBUG_ASSERT(!overflowed);
  DEBUG_ASSERT(request->len_ >= kPageSize);

  uint64_t cur_end;
  overflowed = add_overflow(request->offset_, request->len_, &cur_end);
  DEBUG_ASSERT(!overflowed);

  auto node = outstanding_requests_[request->type_].upper_bound(request->offset_);
  if (node.IsValid()) {
    if (request->offset_ >= node->offset_ && cur_end >= node->GetEnd()) {
      // If the beginning part of this request is covered by an existing request, end the request
      // at the existing request's end and wait for that request to be resolved first.
      request->len_ = node->GetEnd() - request->offset_;
    } else if (request->offset_ < node->offset_ && cur_end >= node->offset_) {
      // If offset is less than node->GetOffset(), then we end the request when we'd start
      // overlapping.
      request->len_ = node->offset_ - request->offset_;
    }
  }

  SendRequestToProviderLocked(request);

  return ZX_ERR_SHOULD_WAIT;
}

bool PageSource::DebugIsPageOk(vm_page_t* page, uint64_t offset) {
  return page_provider_->DebugIsPageOk(page, offset);
}

void PageSource::SendRequestToProviderLocked(PageRequest* request) {
  LTRACEF_LEVEL(2, "%p %p\n", this, request);
  DEBUG_ASSERT(request->type_ < page_request_type::COUNT);
  DEBUG_ASSERT(request->IsInitialized());
  DEBUG_ASSERT(SupportsPageRequestType(request->type_));
  // Find the node with the smallest endpoint greater than offset and then
  // check to see if offset falls within that node.
  auto overlap = outstanding_requests_[request->type_].upper_bound(request->offset_);
  if (overlap.IsValid() && overlap->offset_ <= request->offset_) {
    // GetPage guarantees that if offset lies in an existing node, then it is
    // completely contained in that node.
    overlap->overlap_.push_back(request);
  } else {
    DEBUG_ASSERT(!request->provider_owned_);
    request->pending_size_ = request->len_;

    DEBUG_ASSERT(!fbl::InContainer<PageProviderTag>(*request));
    request->provider_owned_ = true;
    page_provider_->SendAsyncRequest(request);
    outstanding_requests_[request->type_].insert(request);
  }
}

void PageSource::CompleteRequestLocked(PageRequest* request) {
  VM_KTRACE_DURATION(1, "page_request_complete", ("vmo_id", request->vmo_debug_info_.vmo_id),
                     ("offset", request->offset_), ("length", request->len_),
                     ("type", request->type_ == ZX_PAGER_VMO_READ ? "Read" : "Dirty"));
  DEBUG_ASSERT(request->type_ < page_request_type::COUNT);
  DEBUG_ASSERT(SupportsPageRequestType(request->type_));

  // Take the request back from the provider before waking up the corresponding thread. Once the
  // request has been taken back we are also free to modify offset_.
  page_provider_->ClearAsyncRequest(request);
  request->provider_owned_ = false;

  if (likely(request->overlap_.is_empty())) {
    pgreq_overlap_0.Add(1);
  } else {
    // Even though we're doing size_slow(), this is okay because the vast majority of page requests
    // have no overlap and won't end up in this else-block in the first place.
    size_t overlap_count = request->overlap_.size_slow();
    DEBUG_ASSERT(overlap_count > 0);
    switch (overlap_count) {
      case 1:
        pgreq_overlap_1.Add(1);
        break;
      case 2:
        pgreq_overlap_2.Add(1);
        break;
      case 3:
        pgreq_overlap_3.Add(1);
        break;
      case 4:
        pgreq_overlap_4.Add(1);
        break;
      default:
        if (overlap_count <= 10) {
          pgreq_overlap_5_to_10.Add(1);
        } else {
          pgreq_overlap_more_than_10.Add(1);
        }
        break;
    }
  }

  while (!request->overlap_.is_empty()) {
    auto waiter = request->overlap_.pop_front();
    VM_KTRACE_FLOW_BEGIN(1, "page_request_signal", reinterpret_cast<uintptr_t>(waiter));
    DEBUG_ASSERT(!waiter->provider_owned_);
    waiter->offset_ = UINT64_MAX;
    waiter->event_.Signal(waiter->complete_status_);
  }
  VM_KTRACE_FLOW_BEGIN(1, "page_request_signal", reinterpret_cast<uintptr_t>(request));
  request->offset_ = UINT64_MAX;
  request->event_.Signal(request->complete_status_);
}

void PageSource::CancelRequest(PageRequest* request) {
  canary_.Assert();
  Guard<Mutex> guard{&page_source_mtx_};
  CancelRequestLocked(request);
}

void PageSource::CancelRequestLocked(PageRequest* request) {
  LTRACEF("%p %lx\n", this, request->offset_);

  if (!request->IsInitialized()) {
    return;
  }
  DEBUG_ASSERT(request->type_ < page_request_type::COUNT);
  DEBUG_ASSERT(SupportsPageRequestType(request->type_));

  if (fbl::InContainer<PageSourceTag>(*request)) {
    LTRACEF("Overlap node\n");
    // This node is overlapping some other node, so just remove the request
    auto main_node = outstanding_requests_[request->type_].upper_bound(request->offset_);
    ASSERT(main_node.IsValid());
    main_node->overlap_.erase(*request);
  } else if (!request->overlap_.is_empty()) {
    LTRACEF("Outstanding with overlap\n");
    // This node is an outstanding request with overlap, so replace it with the
    // first overlap node.
    auto new_node = request->overlap_.pop_front();
    DEBUG_ASSERT(!new_node->provider_owned_);
    new_node->overlap_.swap(request->overlap_);
    new_node->offset_ = request->offset_;
    new_node->len_ = request->len_;
    new_node->pending_size_ = request->pending_size_;
    DEBUG_ASSERT(new_node->type_ == request->type_);

    DEBUG_ASSERT(!fbl::InContainer<PageProviderTag>(*new_node));
    outstanding_requests_[request->type_].erase(*request);
    outstanding_requests_[request->type_].insert(new_node);

    new_node->provider_owned_ = true;
    page_provider_->SwapAsyncRequest(request, new_node);
    request->provider_owned_ = false;
  } else if (static_cast<fbl::WAVLTreeContainable<PageRequest*>*>(request)->InContainer()) {
    LTRACEF("Outstanding no overlap\n");
    // This node is an outstanding request with no overlap
    outstanding_requests_[request->type_].erase(*request);
    page_provider_->ClearAsyncRequest(request);
    request->provider_owned_ = false;
  }

  // Request has been cleared from the PageProvider, so we're free to modify the offset_
  request->offset_ = UINT64_MAX;
}

zx_status_t PageSource::WaitOnRequest(PageRequest* request, bool suspendable) {
  canary_.Assert();

  // If we have been detached the request will already have been completed in ::Detach and so the
  // provider should instantly wake from the event.
  return page_provider_->WaitOnEvent(&request->event_, suspendable);
}

void PageSource::DumpSelf(uint depth, uint max_items) const {
  Guard<Mutex> guard{&page_source_mtx_};
  dump::DepthPrinter printer(depth);
  printer.Emit("page_source %p detached %d closed %d", this, detached_, closed_);
  for (uint8_t type = 0; type < page_request_type::COUNT; type++) {
    printer.BeginList(max_items);
    for (auto& req : outstanding_requests_[type]) {
      printer.Emit("  vmo <%s>/k%lu %s req [0x%lx, 0x%lx) pending 0x%lx overlap %lu %s",
                   req.vmo_debug_info_.vmo_name, req.vmo_debug_info_.vmo_id,
                   PageRequestTypeToString(page_request_type(type)), req.offset_, req.GetEnd(),
                   req.pending_size_, req.overlap_.size_slow(),
                   req.provider_owned_ ? "[sent]" : "");
    }
    printer.EndList();
  }
}

void PageSource::Dump(uint depth, uint max_items) const {
  DumpSelf(depth, max_items);
  page_provider_->Dump(depth, max_items);
}

PageRequest::~PageRequest() { CancelRequest(); }

void PageRequest::Init(fbl::RefPtr<PageRequestInterface> src, uint64_t offset,
                       page_request_type type, VmoDebugInfo vmo_debug_info) {
  DEBUG_ASSERT(!IsInitialized());
  vmo_debug_info_ = vmo_debug_info;
  len_ = 0;
  offset_ = offset;
  if (early_wake_) {
    wake_offset_ = 0;
  }
  DEBUG_ASSERT(type < page_request_type::COUNT);
  type_ = type;
  src_ = ktl::move(src);
  complete_status_ = ZX_OK;

  event_.Unsignal();
}

ktl::pair<uint64_t, uint64_t> PageRequest::TrimRangeToRequestSpace(uint64_t start,
                                                                   uint64_t end) const {
  uint64_t req_offset, req_end;
  if (start >= offset_) {
    // The operation started partway into this request.
    req_offset = start - offset_;
  } else {
    // The operation started before this request.
    req_offset = 0;
  }
  if (end < GetEnd()) {
    // The operation ended partway into this request.
    req_end = end - offset_;

    uint64_t unused;
    DEBUG_ASSERT(!sub_overflow(end, offset_, &unused));
  } else {
    // The operation ended past the end of this request.
    req_end = len_;
  }
  DEBUG_ASSERT(req_end >= req_offset);
  return {req_offset, req_end};
}

zx_status_t PageRequest::Wait(bool suspendable) {
  lockdep::AssertNoLocksHeld();
  VM_KTRACE_DURATION(1, "page_request_wait", ("vmo_id", vmo_debug_info_.vmo_id),
                     ("offset", offset_), ("length", len_),
                     ("type", type_ == ZX_PAGER_VMO_READ ? "Read" : "Dirty"));
  zx_status_t status = src_->WaitOnRequest(this, suspendable);
  VM_KTRACE_FLOW_END(1, "page_request_signal", reinterpret_cast<uintptr_t>(this));
  if (status != ZX_OK && !PageSource::IsValidInternalFailureCode(status)) {
    src_->CancelRequest(this);
  }
  return status;
}

void PageRequest::CancelRequest() {
  // Nothing to cancel if the request isn't initialized yet.
  // We may inspect src_ here as src_ is only set by Init, and Init and CancelRequest must only be
  // called by the owner of the PageRequest, providing implicit serialization.
  if (!src_) {
    return;
  }
  src_->CancelRequest(this);
}

PageRequest* LazyPageRequest::get() {
  if (!is_initialized()) {
    request_.emplace(early_wake_);
  }
  return &*request_;
}

zx_status_t MultiPageRequest::Wait(bool suspendable) {
  if (anonymous_.is_active()) {
    DEBUG_ASSERT(!dirty_active_ && !read_active_);
    return anonymous_.Allocate(suspendable).status_value();
  }
  // Exactly one of read and dirty should be considered active.
  DEBUG_ASSERT(dirty_active_ ^ read_active_);
  read_active_ = false;
  dirty_active_ = false;
  return page_request_->Wait(suspendable);
}

void MultiPageRequest::CancelRequests() {
  anonymous_.Cancel();
  // In case a request is still initialized, despite being waited on, explicitly cancel regardless
  // of active status.
  if (page_request_.is_initialized()) {
    page_request_->CancelRequest();
  }
  read_active_ = false;
  dirty_active_ = false;
}

static int cmd_page_source(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 2) {
  notenoughargs:
    printf("not enough arguments\n");
  usage:
    printf("usage:\n");
    printf("%s dump <address>\n", argv[0].str);
    return ZX_ERR_INTERNAL;
  }

  if (!strcmp(argv[1].str, "dump")) {
    if (argc < 3) {
      goto notenoughargs;
    }
    reinterpret_cast<PageSource*>(argv[2].u)->Dump(0, UINT32_MAX);
  } else {
    printf("unknown command\n");
    goto usage;
  }

  return ZX_OK;
}

STATIC_COMMAND_START
STATIC_COMMAND("vm_page_source", "page source debug commands", &cmd_page_source)
STATIC_COMMAND_END(ps_object)
