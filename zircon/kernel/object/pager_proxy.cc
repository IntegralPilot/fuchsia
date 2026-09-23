// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/counters.h>
#include <lib/dump/depth_printer.h>
#include <lib/flow_id.h>
#include <trace.h>
#include <zircon/syscalls-next.h>

#include <lk/init.h>
#include <object/diagnostics.h>
#include <object/pager_dispatcher.h>
#include <object/pager_proxy.h>
#include <object/thread_dispatcher.h>

#define LOCAL_TRACE 0

KCOUNTER(dispatcher_pager_overtime_wait_count, "dispatcher.pager.overtime_waits")
KCOUNTER(dispatcher_pager_ignored_suspend_count, "dispatcher.pager.ignored_suspends")
KCOUNTER(dispatcher_pager_total_request_count, "dispatcher.pager.total_requests")
KCOUNTER(dispatcher_pager_succeeded_request_count, "dispatcher.pager.succeeded_requests")
KCOUNTER(dispatcher_pager_failed_request_count, "dispatcher.pager.failed_requests")
KCOUNTER(dispatcher_pager_timed_out_request_count, "dispatcher.pager.timed_out_requests")

PagerProxy::PagerProxy(PagerDispatcher* dispatcher, fbl::RefPtr<PortDispatcher> port, uint64_t key,
                       uint32_t options)
    : pager_(dispatcher),
      pager_koid_(pager_->get_koid()),
      port_(ktl::move(port)),
      key_(key),
      options_(options) {
  LTRACEF("%p key %lx options %x\n", this, key_, options_);
}

PagerProxy::~PagerProxy() {
  LTRACEF("%p\n", this);
  // In error paths shortly after construction, we can destruct without page_source_closed_ becoming
  // true.
  DEBUG_ASSERT(!complete_pending_);
  // We vend out a raw pointer to ourselves in the form of being the allocator for our internal
  // PortPacket. Ensure that the packet is not somehow still in use, and that a PortDispatcher would
  // therefore not be able to call Free.
  DEBUG_ASSERT(!packet_busy_);
}

PageSourceProperties PagerProxy::properties() const {
  return PageSourceProperties{
      .is_user_pager = true,
      .supports_request_type = {true, !!(options_ & kTrapDirty), false},
  };
}

ktl::optional<uint64_t> PagerProxy::GetKoid() const { return pager_koid_; }

void PagerProxy::SendAsyncRequest(PageRequest* request) {
  Guard<Mutex> guard{&mtx_};
  ASSERT(!page_source_closed_);

  QueuePacketLocked(request);
}

void PagerProxy::QueuePacketLocked(PageRequest* request) {
  if (packet_busy_) {
    pending_requests_.push_back(request);
    return;
  }

  DEBUG_ASSERT(active_request_ == nullptr);
  packet_busy_ = true;
  active_request_ = request;

  uint64_t offset, length;
  uint16_t cmd;
  if (request != &complete_request_) {
    switch (GetRequestType(request)) {
      case page_request_type::READ:
        cmd = ZX_PAGER_VMO_READ;
        break;
      case page_request_type::DIRTY:
        DEBUG_ASSERT(options_ & kTrapDirty);
        cmd = ZX_PAGER_VMO_DIRTY;
        break;
      default:
        // Not reached
        ASSERT(false);
    }
    offset = GetRequestOffset(request);
    length = GetRequestLen(request);

    // The vm subsystem should guarantee this
    uint64_t unused;
    DEBUG_ASSERT(!add_overflow(offset, length, &unused));

    // Trace flow events require an enclosing duration.
    // We cannot use the PortPacket pointer address (&packet_) as the flow ID because packet_
    // is a member field reused across consecutive page requests. If a previous flow end event
    // is lost or cancelled, reusing the same pointer address causes flow_duplicate_id collisions.
    flow_id_ = flow_id_generate();
    VM_KTRACE_DURATION(1, "page_request_queue", ("vmo_id", GetRequestVmoId(request)),
                       ("offset", offset), ("length", length),
                       ("type", GetRequestType(request) == ZX_PAGER_VMO_READ ? "Read" : "Dirty"));
    VM_KTRACE_FLOW_BEGIN(1, "page_request_queue", flow_id_);
  } else {
    offset = length = 0;
    cmd = ZX_PAGER_VMO_COMPLETE;
  }

  zx_port_packet_t packet = {};
  packet.key = key_;
  packet.type = ZX_PKT_TYPE_PAGE_REQUEST;
  packet.page_request.command = cmd;
  packet.page_request.offset = offset;
  packet.page_request.length = length;

  packet_.packet = packet;

  // We can treat ZX_ERR_BAD_HANDLE as if the packet was queued
  // but the pager service never responds.
  // TODO: Bypass the port's max queued packet count to prevent ZX_ERR_SHOULD_WAIT
  ASSERT(port_->Queue(&packet_) != ZX_ERR_SHOULD_WAIT);
}

void PagerProxy::ClearAsyncRequest(PageRequest* request) {
  Guard<Mutex> guard{&mtx_};
  ASSERT(!page_source_closed_);

  if (request == active_request_) {
    if (request != &complete_request_) {
      // Trace flow events require an enclosing duration.
      VM_KTRACE_DURATION(
          1, "page_request_queue",
          ("vmo_id", KTRACE_ANNOTATED_VALUE(AssertHeld(mtx_), GetRequestVmoId(active_request_))),
          ("offset", KTRACE_ANNOTATED_VALUE(AssertHeld(mtx_), GetRequestOffset(active_request_))),
          ("length", KTRACE_ANNOTATED_VALUE(AssertHeld(mtx_), GetRequestLen(active_request_))),
          ("type", KTRACE_ANNOTATED_VALUE(
                       AssertHeld(mtx_),
                       GetRequestType(active_request_) == ZX_PAGER_VMO_READ ? "Read" : "Dirty")));
      VM_KTRACE_FLOW_END(1, "page_request_queue", flow_id_);
    }
    // This request is being taken back by the PageSource, so we can't hold a reference to it
    // anymore. This will remain null until OnPacketFreedLocked is called (and a new packet gets
    // queued as a result), either by us here below or by PagerProxy::Free, since packet_busy_ is
    // true and will be true until OnPacketFreedLocked is called.
    active_request_ = nullptr;
    // Condition on whether or not we actually cancel the packet, to make sure
    // we don't race with a call to PagerProxy::Free.
    if (port_->CancelQueued(&packet_)) {
      OnPacketFreedLocked();
    }
  } else if (fbl::InContainer<PageProviderTag>(*request)) {
    pending_requests_.erase(*request);
  }
}

void PagerProxy::SwapAsyncRequest(PageRequest* old, PageRequest* new_req) {
  Guard<Mutex> guard{&mtx_};
  ASSERT(!page_source_closed_);

  if (fbl::InContainer<PageProviderTag>(*old)) {
    pending_requests_.insert(*old, new_req);
    pending_requests_.erase(*old);
  } else if (old == active_request_) {
    active_request_ = new_req;
  }
}

bool PagerProxy::DebugIsPageOk(vm_page_t* page, uint64_t offset) { return true; }

void PagerProxy::OnDetach() {
  Guard<Mutex> guard{&mtx_};
  ASSERT(!page_source_closed_);

  complete_pending_ = true;
  QueuePacketLocked(&complete_request_);
}

void PagerProxy::OnClose() {
  fbl::RefPtr<PagerProxy> self_ref;
  fbl::RefPtr<PageSource> self_src;
  Guard<Mutex> guard{&mtx_};
  ASSERT(!page_source_closed_);

  page_source_closed_ = true;
  // If there isn't a complete packet pending, we're free to clean up our ties with the PageSource
  // and PagerDispatcher right now, as we're not expecting a PagerProxy::Free to perform final
  // delayed clean up later. The PageSource is closing, so it won't need to send us any more
  // requests. The PagerDispatcher doesn't need to refer to us anymore as we won't be queueing any
  // more pager requests.
  if (!complete_pending_) {
    // We know PagerDispatcher::on_zero_handles hasn't been invoked, since that would
    // have already closed this pager proxy via OnDispatcherClose. Therefore we are free to
    // immediately clean up.
    DEBUG_ASSERT(pager_ != nullptr);
    self_ref = pager_->ReleaseProxy(this);
    self_src = ktl::move(page_source_);
  } else {
    // There is still a pending complete message that we would like to wait to be received and so we
    // do not perform CancelQueued like OnDispatcherClose does. However, we must leave the reference
    // to ourselves in pager_ so that OnDispatcherClose (and the forced packet cancelling) can
    // happen if needed. Otherwise final delayed cleanup will happen in PagerProxy::Free.
  }
}

void PagerProxy::OnDispatcherClose() {
  fbl::RefPtr<PageSource> self_src;
  Guard<Mutex> guard{&mtx_};

  // The PagerDispatcher is going away and there won't be a way to service any pager requests. Close
  // the PageSource from our end so that no more requests can be sent. Closing the PageSource will
  // clear/cancel any outstanding requests that it had forwarded, i.e. any requests except the
  // complete request (which is owned by us and is not visible to the PageSource).
  if (!page_source_closed_) {
    // page_source_ is only reset to nullptr if we already closed it.
    DEBUG_ASSERT(page_source_);
    self_src = page_source_;
    // Call Close without the lock to
    //  * Not violate lock ordering
    //  * Allow it to call back into ::OnClose
    guard.CallUnlocked([&self_src]() mutable { self_src->Close(); });
  }

  // The pager dispatcher's reference to this object is the only one we completely control. Now
  // that it's gone, we need to make sure that port_ doesn't end up with an invalid pointer
  // to packet_ if all external RefPtrs to this object go away.
  // As the Pager dispatcher is going away, we are not content to keep these objects alive
  // indefinitely until messages are read, instead we want to cancel everything as soon as possible
  // to avoid memory leaks. Therefore we will attempt to cancel any queued final packet.
  if (complete_pending_) {
    if (port_->CancelQueued(&packet_)) {
      // We successfully cancelled the message, so we don't have to worry about
      // PagerProxy::Free being called, and can immediately break the refptr cycle.
      complete_pending_ = false;
    } else {
      // If we failed to cancel the message, then there is a pending call to PagerProxy::Free. It
      // will cleanup the RefPtr cycle, although only if page_source_closed_ is true, which should
      // be the case since we performed the Close step earlier.
      DEBUG_ASSERT(page_source_closed_);
    }
  } else {
    // Either the complete message had already been dispatched when this object was closed or
    // PagerProxy::Free was called between this object being closed and this method taking the
    // lock. In either case, the port no longer has a reference, any RefPtr cycles have been broken
    // and cleanup is already done.
    DEBUG_ASSERT(!page_source_);
  }
  // The pager dispatcher calls OnDispatcherClose when it is going away on zero handles.  Clear
  // pager_ to ensure we don't attempt to access an already-freed pager dispatcher.
  pager_ = nullptr;
}

void PagerProxy::Free(PortPacket* packet) {
  fbl::RefPtr<PagerProxy> self_ref;
  fbl::RefPtr<PageSource> self_src;

  Guard<Mutex> guard{&mtx_};
  if (active_request_ != &complete_request_) {
    // This request is still active, i.e. it has not been taken back by the PageSource with
    // ClearAsyncRequest. So we are responsible for relinquishing ownership of the request.
    if (active_request_ != nullptr) {
      // Trace flow events require an enclosing duration.
      VM_KTRACE_DURATION(
          1, "page_request_queue",
          ("vmo_id", KTRACE_ANNOTATED_VALUE(AssertHeld(mtx_), GetRequestVmoId(active_request_))),
          ("offset", KTRACE_ANNOTATED_VALUE(AssertHeld(mtx_), GetRequestOffset(active_request_))),
          ("length", KTRACE_ANNOTATED_VALUE(AssertHeld(mtx_), GetRequestLen(active_request_))),
          ("type", KTRACE_ANNOTATED_VALUE(
                       AssertHeld(mtx_),
                       GetRequestType(active_request_) == ZX_PAGER_VMO_READ ? "Read" : "Dirty")));
      VM_KTRACE_FLOW_END(1, "page_request_queue", flow_id_);
      active_request_ = nullptr;
    }
    OnPacketFreedLocked();
  } else {
    // Freeing the complete_request_ indicates we have completed a pending action that might have
    // been delaying cleanup.
    complete_pending_ = false;
    // Should be nothing else queued.
    DEBUG_ASSERT(pending_requests_.is_empty());
    active_request_ = nullptr;
    packet_busy_ = false;
    // If the source is closed, we need to do delayed cleanup. Make sure we are not still in the
    // pager's proxy list (if the pager is not closed yet), and then break our refptr cycle.
    if (page_source_closed_) {
      DEBUG_ASSERT(page_source_);
      // If the PagerDispatcher is already closed, the proxy has already been released.
      if (pager_) {
        // self_ref could be a nullptr if we have ended up racing with
        // PagerDispatcher::on_zero_handles which calls PagerProxy::OnDispatcherClose *after*
        // removing the proxy from its list. This is fine as the proxy will be removed from the
        // pager's proxy list either way.
        self_ref = pager_->ReleaseProxy(this);
      }
      self_src = ktl::move(page_source_);
    }
  }
  // At this point it is possible that self_ref and self_src are nullptrs, and we have set
  // complete_pending_ to false if we were freeing the complete_request_. In this case the moment
  // guard is dropped it is possible for some other thread to take the lock, observe this, and then
  // delete this object. This is fine as we do not reference `this` after the lock is dropped, with
  // this->mtx_ defined (by the Mutex implementation) as not being referenced once another thread is
  // able to acquire the lock.
}

void PagerProxy::OnPacketFreedLocked() {
  // We are here because the active request has been freed. And packet_busy_ is still true, so no
  // new request will have become active yet.
  DEBUG_ASSERT(active_request_ == nullptr);
  packet_busy_ = false;
  if (!pending_requests_.is_empty()) {
    QueuePacketLocked(pending_requests_.pop_front());
  }
}

void PagerProxy::SetPageSourceUnchecked(fbl::RefPtr<PageSource> src) {
  // SetPagerSource is a private function and is only called by the PagerDispatcher just after
  // construction, unfortunately it needs to be called under the PagerDispatcher lock and lock
  // ordering is always PagerProxy->PagerDispatcher, and so we cannot acquire the lock here.
  auto func = [this, &src]() TA_NO_THREAD_SAFETY_ANALYSIS { page_source_ = ktl::move(src); };
  func();
}

namespace {

// Helper to calculate the pager wait deadline.
Deadline make_deadline() {
  if (BootOptions::Get()->userpager_overtime_wait_seconds == 0) {
    return Deadline::infinite();
  }
  return Deadline::after_mono(ZX_SEC(BootOptions::Get()->userpager_overtime_wait_seconds));
}

// Helper to determine if we've waited on the pager for longer than the specified timeout.
bool waited_too_long(uint32_t waited) {
  return BootOptions::Get()->userpager_overtime_timeout_seconds > 0 &&
         waited * BootOptions::Get()->userpager_overtime_wait_seconds >=
             BootOptions::Get()->userpager_overtime_timeout_seconds;
}

}  // namespace

zx_status_t PagerProxy::WaitOnEvent(Event* event, bool suspendable) {
  ThreadDispatcher::AutoBlocked by(ThreadDispatcher::Blocked::PAGER);
  ktl::optional<ScopedMemoryStall> memory_stall;
  if (BootOptions::Get()->experimental_expand_memory_stall) {
    // Note: This counts *all* pager waits as stalls, not just ones resulting from a refault (which
    // is what Linux measures). This can result in false positives, e.g. during boot or app launch
    // when there is a lot of pager activity, we might incorrectly conclude that there is a memory
    // stall even in the absence of memory pressure. However, incorporating this distinction
    // involves non trivial complexity so as an approximation for now just count every pager wait.
    // See https://fxbug.dev/508774420
    memory_stall.emplace();
  }
  kcounter_add(dispatcher_pager_total_request_count, 1);
  uint32_t waited = 0;
  // Ignore the suspend signal if not suspendable.
  const uint signal_mask = suspendable ? 0 : THREAD_SIGNAL_SUSPEND;
  zx_status_t result;
  do {
    result = event->Wait(make_deadline(), signal_mask);

    if (result == ZX_ERR_INTERNAL_INTR_RETRY) {
      if (suspendable) {
        // Terminate the wait early if suspendable.
        kcounter_add(dispatcher_pager_failed_request_count, 1);
        return result;
      }
      // Count how often we ignore suspend signals as a debugging aid.
      dispatcher_pager_ignored_suspend_count.Add(1);
    } else if (result == ZX_ERR_TIMED_OUT) {
      waited++;
      // We might trigger this loop multiple times as we exceed multiples of the overtime counter,
      // but we only want to count each unique overtime event in the kcounter.
      if (waited == 1) {
        dispatcher_pager_overtime_wait_count.Add(1);
      }

      // Error out if we've been waiting for longer than the specified timeout, to allow the rest of
      // the system to make progress (if possible).
      if (waited_too_long(waited)) {
        void* src;
        {
          Guard<Mutex> guard{&mtx_};
          src = page_source_.get();
        }
        printf("ERROR Page source %p blocked for %" PRIu64 " seconds. Page request timed out.\n",
               src, BootOptions::Get()->userpager_overtime_timeout_seconds);
        Dump(0, BootOptions::Get()->userpager_overtime_printout_limit);

        // This function is called from the context of waiting on a page request, so we know that we
        // don't hold any locks. It should be safe to iterate the root job tree to dump handle info.
        printf("Dumping all handles for the pager object:\n");
        DumpHandlesForKoid(pager_koid_);
        printf("Dumping all handles for the pager port object:\n");
        DumpHandlesForKoid(port_->get_koid());

        Thread::Current::Dump(false);
        kcounter_add(dispatcher_pager_timed_out_request_count, 1);
        return ZX_ERR_TIMED_OUT;
      }

      // Do an informational printout of the source and ourselves if the overtime period has
      // elapsed.
      PrintOvertime(waited * BootOptions::Get()->userpager_overtime_wait_seconds);
    }

    // Hold off on suspension until after the page request is resolved (or fails with a timeout).
  } while (result == ZX_ERR_TIMED_OUT || result == ZX_ERR_INTERNAL_INTR_RETRY);

  if (result == ZX_OK) {
    kcounter_add(dispatcher_pager_succeeded_request_count, 1);
  } else {
    // Only counts failures that are *not* pager timeouts. Timeouts are tracked with
    // dispatcher_pager_timed_out_request_count, which is updated above when we
    // return early with ZX_ERR_TIMED_OUT.
    kcounter_add(dispatcher_pager_failed_request_count, 1);
  }

  return result;
}

void PagerProxy::PrintOvertime(uint64_t waited_seconds) {
  bool do_printout = false;
  fbl::RefPtr<PageSource> src;
  {
    Guard<Mutex> guard{&mtx_};
    src = page_source_;
    const zx_instant_mono_t now = current_mono_time();
    if (now >= zx_time_add_duration(last_overtime_dump_,
                                    ZX_SEC(BootOptions::Get()->userpager_overtime_wait_seconds))) {
      do_printout = true;
      last_overtime_dump_ = now;
    }
  }
  printf("WARNING Page source %p blocked for %" PRIu64 " seconds. %s\n", src.get(), waited_seconds,
         do_printout ? "Dump:" : "Dump skipped.");
  // Dump out the rest of the state of the outstanding requests.
  if (do_printout) {
    Dump(0, BootOptions::Get()->userpager_overtime_printout_limit);
    if (src) {
      // Use DumpSelf to avoid it calling our Dump method that we already performed.
      src->DumpSelf(0, BootOptions::Get()->userpager_overtime_printout_limit);
    }
  }
}

void PagerProxy::Dump(uint depth, uint32_t max_items) {
  Guard<Mutex> guard{&mtx_};
  dump::DepthPrinter printer(depth);
  char name[ZX_MAX_NAME_LEN];
  if (pager_) {
    pager_->get_debug_name(name, sizeof(name));
  } else {
    strlcpy(name, "<closed>", sizeof(name));
  }
  printer.Emit("pager_dispatcher <%s> page_source %p key %lu", name, page_source_.get(), key_);

  printer.Emit("  source_closed %d pager_closed %d packet_busy %d complete_pending %d",
               page_source_closed_, (pager_ == nullptr), packet_busy_, complete_pending_);

  if (active_request_) {
    if (active_request_ != &complete_request_) {
      printer.Emit("  active %s request on pager port [0x%lx, 0x%lx) (port koid 0x%lx)",
                   PageRequestTypeToString(GetRequestType(active_request_)),
                   GetRequestOffset(active_request_),
                   GetRequestOffset(active_request_) + GetRequestLen(active_request_),
                   port_.get()->get_koid());
    } else {
      printer.Emit("  active COMPLETE request on pager port (port koid 0x%lx)",
                   port_.get()->get_koid());
    }
  } else {
    printer.Emit("  no active request on pager port");
  }

  if (pending_requests_.is_empty()) {
    printer.Emit("  no pending requests to queue on pager port");
    return;
  }

  printer.BeginList(max_items);
  for (auto& req : pending_requests_) {
    if (&req != &complete_request_) {
      printer.Emit("  pending %s req to queue on pager port [0x%lx, 0x%lx)",
                   PageRequestTypeToString(GetRequestType(&req)), GetRequestOffset(&req),
                   GetRequestOffset(&req) + GetRequestLen(&req));
    } else {
      printer.Emit("  pending COMPLETE request to queue on pager port");
    }
  }
  printer.EndList();
}
