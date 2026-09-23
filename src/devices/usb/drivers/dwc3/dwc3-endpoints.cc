// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>
#include <lib/trace/event.h>

#include "src/devices/usb/drivers/dwc3/dwc3-regs.h"
#include "src/devices/usb/drivers/dwc3/dwc3.h"

namespace dwc3 {

void Dwc3::EpEnable(Endpoint& ep, bool enable) {
  TRACE_DURATION("dwc3", "Dwc3::EpEnable", "ep_num", ep.ep_num, "enable", enable);
  auto* mmio = get_mmio();

  if (enable) {
    DALEPENA::Get().ReadFrom(mmio).EnableEp(ep.ep_num).WriteTo(mmio);
  } else {
    DALEPENA::Get().ReadFrom(mmio).DisableEp(ep.ep_num).WriteTo(mmio);
  }

  ep.enabled = enable;
}

void Dwc3::EpSetConfig(Endpoint& ep, bool enable) {
  TRACE_DURATION("dwc3", "Dwc3::EpSetConfig", "ep_num", ep.ep_num, "enable", enable);
  fdf::debug("Dwc3::EpSetConfig {}", ep.ep_num);

  if (enable) {
    CmdEpSetConfig(ep, false);
    CmdEpTransferConfig(ep);
    EpEnable(ep, true);
  } else {
    EpEnable(ep, false);
  }
}

zx_status_t Dwc3::EpSetStall(Endpoint& ep, bool stall) {
  TRACE_DURATION("dwc3", "Dwc3::EpSetStall", "ep_num", ep.ep_num, "stall", stall);
  if (!ep.enabled) {
    return ZX_ERR_BAD_STATE;
  }

  if (stall && !ep.stalled) {
    CmdEpSetStall(ep);
  } else if (!stall && ep.stalled) {
    CmdEpClearStall(ep);
  }

  ep.stalled = stall;
  return ZX_OK;
}

void Dwc3::EpStartTransfer(Endpoint& ep, TrbFifo& fifo, uint32_t type, zx_paddr_t buffer,
                           size_t length, bool zlp) {
  TRACE_DURATION("dwc3", "Dwc3::EpStartTransfer", "ep_num", ep.ep_num, "type", type, "length",
                 length, "zlp", zlp);
  fdf::debug("Dwc3::EpStartTransfer ep {} type {} length {} zlp {}", ep.ep_num, type, length, zlp);

  dwc3_trb_t* trb = fifo.AdvanceWrite();
  trb->ptr_low = static_cast<uint32_t>(buffer);
  trb->ptr_high = static_cast<uint32_t>(buffer >> 32);
  trb->status = TRB_BUFSIZ(static_cast<uint32_t>(length));

  zx_paddr_t trb_phys;
  if (zlp) {
    // NOTE: The DWC3 programming manual states support for a `Normal-ZLP`
    // transfer type that should be able to handle this. Alas, it doesn't
    // actually seem to work, so we just enqueue an actual zero length transfer
    // as a second TRB.
    // Because the data TRB and ZLP TRB are submitted as separate, unchained Transfer Descriptors
    // (TDs), both must have TRB_IOC set so the driver receives completion events to finish the
    // request.
    trb->control = type | TRB_IOC;

    dwc3_trb_t* trb2 = fifo.AdvanceWrite();
    trb2->ptr_low = 0;
    trb2->ptr_high = 0;
    trb2->status = TRB_BUFSIZ(0);
    trb2->control = type | TRB_LST | TRB_IOC | TRB_HWO;
    fifo.Write(trb2, 1);

    // memory_order_release enforces compiler memory ordering for uncached memory, preventing
    // TRB_HWO from being committed to RAM before the payload.
    std::atomic_thread_fence(std::memory_order_release);

    trb->control = type | TRB_IOC | TRB_HWO;

    trb_phys = fifo.Write(trb, 1);
  } else {
    // memory_order_release enforces compiler memory ordering for uncached memory, preventing
    // TRB_HWO from being committed to RAM before the payload.
    std::atomic_thread_fence(std::memory_order_release);
    trb->control = type | TRB_LST | TRB_IOC | TRB_HWO;
    trb_phys = fifo.Write(trb, 1);
  }

  ep.transfer_state = Endpoint::TransferState::kStartingSingle;
  CmdEpStartTransfer(ep, trb_phys);
}

void Dwc3::EpServer::FlushCancelCompleters(zx_status_t status) {
  auto completers = std::move(cancel_completers);
  for (auto& completer : completers) {
    if (status == ZX_OK) {
      completer.Reply(zx::ok());
    } else {
      completer.Reply(zx::error(status));
    }
  }
}

void Dwc3::EpServer::CancelAll(zx_status_t reason) {
  TRACE_DURATION("dwc3", "Dwc3::EpServer::CancelAll", "ep_num", uep_->ep.ep_num, "reason", reason);
  fdf::debug("Dwc3::EpServer::CancelAll ep {} reason {} reqs = ({}, {}), controller={}",
             uep_->ep.ep_num, zx_status_get_string(reason), uep_->server->active_reqs.size(),
             uep_->server->queued_reqs.size(), dwc3_->controller_started_);

  // Likely performing a full reset. We can't stop any ongoing transfers and
  // assume everything is back to original state.
  if (!dwc3_->controller_started_) {
    for (; !active_reqs.empty(); active_reqs.pop()) {
      RequestComplete(reason, 0, std::move(active_reqs.front().request), /*send_now=*/false);
    }
    for (; !queued_reqs.empty(); queued_reqs.pop()) {
      RequestComplete(reason, 0, std::move(queued_reqs.front()), /*send_now=*/false);
    }
    SendCompletions();
    uep_->ep.transfer_state = Endpoint::TransferState::kIdle;
    uep_->ep.rsrc_id = Endpoint::kInvalidResourceId;
    FlushCancelCompleters(reason);
    return;
  }

  // Any request that hasn't been enqueued yet is immediately returned.
  for (; !queued_reqs.empty(); queued_reqs.pop()) {
    RequestComplete(reason, 0, std::move(queued_reqs.front()), /*send_now=*/false);
  }
  SendCompletions();

  switch (uep_->ep.transfer_state) {
    case Endpoint::TransferState::kIdle:
    case Endpoint::TransferState::kCanceling:
    case Endpoint::TransferState::kPendingCancel:
      return;
    case Endpoint::TransferState::kActiveOngoing:
    case Endpoint::TransferState::kActiveSingle:
      pending_cancel_reason = reason;
      dwc3_->CmdEpEndTransfer(uep_->ep);
      uep_->ep.transfer_state = Endpoint::TransferState::kCanceling;
      break;
    case Endpoint::TransferState::kStartingSingle:
    case Endpoint::TransferState::kStartingOngoing:
      // We're waiting for a start transfer to be emitted, record the
      // cancelation reason and issue end transfer.
      pending_cancel_reason = reason;
      uep_->ep.transfer_state = Endpoint::TransferState::kPendingCancel;
      break;
  }
}

void Dwc3::UserEpQueueNext(UserEndpoint& uep) {
  TRACE_DURATION("dwc3", "Dwc3::UserEpQueueNext", "ep_num", uep.ep.ep_num);
  if (!uep.ep.got_not_ready || uep.server->queued_reqs.empty() || uep.ep.stalled) {
    return;
  }

  bool start_transfer;
  switch (uep.ep.transfer_state) {
    case Endpoint::TransferState::kIdle:
      start_transfer = true;
      break;
    case Endpoint::TransferState::kCanceling:
    case Endpoint::TransferState::kActiveSingle:
    case Endpoint::TransferState::kStartingSingle:
    case Endpoint::TransferState::kStartingOngoing:
    case Endpoint::TransferState::kPendingCancel:
      return;
    case Endpoint::TransferState::kActiveOngoing:
      start_transfer = false;
      break;
  }

  const bool enable_ongoing_transfer = AllowEnqueueManyTRBs(uep.ep.type);
  if (!enable_ongoing_transfer) {
    if (!start_transfer) {
      return;
    }
    UserEpQueueNextSingle(uep);
  } else {
    UserEpQueueNextOngoing(uep, start_transfer);
  }
}

void Dwc3::UserEpQueueNextSingle(UserEndpoint& uep) {
  auto& pending_req = uep.server->queued_reqs.front();

  zx::result result = uep.server->get_iter(pending_req, zx_system_get_page_size());
  if (result.is_error()) {
    fdf::error("get_iter failed: {}", result.status_string());
    auto req = std::move(pending_req);
    uep.server->queued_reqs.pop();
    uep.server->RequestComplete(result.status_value(), 0, std::move(req));
    return;
  }

  // TODO(voydanoff) scatter/gather support
  zx_paddr_t phys;
  size_t size;
  std::tie(phys, size) = *result->at(0).begin();

  size_t trb_count = 1;
  bool needs_zlp = false;
  if (uep.ep.IsInput()) {
    auto& req = std::get<usb::FidlRequest>(pending_req);
    bool short_bit = req->short_().value_or(false);
    if (short_bit && size > 0 && (size % uep.ep.max_packet_size == 0)) {
      needs_zlp = true;
      trb_count = 2;
    }
  }

  if (uep.fifo.AvailableSlots() < trb_count) {
    fdf::debug("Dwc3::UserEpQueueNext ep {} not enough FIFO slots for {}-TRB request",
               uep.ep.ep_num, trb_count);
    return;
  }

  uep.server->active_reqs.emplace(EpServer::RequestState{
      .request = std::move(pending_req),
      .total_trbs = trb_count,
      .completed_trbs = 0,
      .completed_bytes = 0,
  });
  uep.server->queued_reqs.pop();

  EpStartTransfer(uep.ep, uep.fifo, TRB_TRBCTL_NORMAL, phys, size, needs_zlp);
}

void Dwc3::UserEpQueueNextOngoing(UserEndpoint& uep, bool start_transfer) {
  // For endpoints that allow continuous transfers with an ongoing TRB ring, the
  // strategy is to issue the start transfer command *once* and then only issue
  // the update-transfer command.
  //
  // Note that a "transfer" is not mapped to a specific set of USB requests.
  // "transfer" means an ongoing session with an active TRB FIFO with the
  // controller. The programming manual refers to this as a transfer so we
  // maintain the jargon.
  //
  // This allows us to operate on the TRBs in parallel with the controller, only
  // kicking it when new requests are enqueue for it to look at the head of the
  // TRB ring.
  //
  // This strategy greatly improves performance, even when still getting
  // interrupts for every transfer, because the transfer only stops/stalls when
  // endpoint clients run out of requests to send down, as opposed to at every
  // request upon ending the transfer.

  if (start_transfer) {
    // Reset the FIFO to a clear state if we're starting a new transfer.
    uep.fifo.Reset();
  } else if (uep.ep.rsrc_id == Endpoint::kInvalidResourceId) {
    // We haven't received a resource ID yet from a started transfer. Can't
    // proceed.
    fdf::debug("Dwc3::UserEpQueueNext ep {} waiting for transfer start", uep.ep.ep_num);
    return;
  }

  zx_paddr_t first_trb_phys = 0;
  size_t enqueued = 0;
  while (!uep.server->queued_reqs.empty()) {
    auto& pending_req = uep.server->queued_reqs.front();

    zx::result result = uep.server->get_iter(pending_req, zx_system_get_page_size());
    if (result.is_error()) {
      fdf::error("get_iter failed: {}", result.status_string());
      auto req = std::move(pending_req);
      uep.server->queued_reqs.pop();
      uep.server->RequestComplete(result.status_value(), 0, std::move(req));
      continue;
    }

    zx_paddr_t phys;
    size_t size;
    std::tie(phys, size) = *result->at(0).begin();

    size_t trb_count = 1;
    bool need_zlp = false;
    if (uep.ep.IsInput()) {
      auto& req = std::get<usb::FidlRequest>(pending_req);
      bool short_bit = req->short_().value_or(false);
      if (short_bit && size > 0 && (size % uep.ep.max_packet_size == 0)) {
        trb_count = 2;
        need_zlp = true;
      }
    }

    if (uep.fifo.AvailableSlots() < trb_count) {
      fdf::debug("Dwc3::UserEpQueueNext ep {} not enough FIFO slots for {}-TRB request",
                 uep.ep.ep_num, trb_count);
      break;
    }

    dwc3_trb_t* trb = uep.fifo.AdvanceWrite();
    trb->ptr_low = static_cast<uint32_t>(phys);
    trb->ptr_high = static_cast<uint32_t>(phys >> 32);
    trb->status = TRB_BUFSIZ(static_cast<uint32_t>(size));

    uint32_t control = TRB_TRBCTL_NORMAL;
    if (uep.ep.IsOutput()) {
      control |= TRB_CSP;
    }

    // We must set the IOC bit on all unchained TRBs, otherwise the hardware halts without
    // interrupting.
    control |= TRB_IOC;

    // 1. Commit all fields without the HWO bit to prevent hardware tearing on active endpoints.
    trb->control = control;

    if (need_zlp) {
      dwc3_trb_t* zlp_trb = uep.fifo.AdvanceWrite();
      zlp_trb->ptr_low = 0;
      zlp_trb->ptr_high = 0;
      zlp_trb->status = TRB_BUFSIZ(0);
      zlp_trb->control = TRB_TRBCTL_NORMAL | TRB_IOC | TRB_HWO;
      uep.fifo.Write(zlp_trb);
    }

    // We intentionally leave the ZLP unchained to prevent the hardware from merging the payloads.
    // 2. Safely grant hardware ownership to the primary TRB spanning the release fence.
    std::atomic_thread_fence(std::memory_order_release);
    trb->control = control | TRB_HWO;
    zx_paddr_t trb_phys = uep.fifo.Write(trb);

    if (first_trb_phys == 0) {
      first_trb_phys = trb_phys;
    }

    uep.server->active_reqs.push(EpServer::RequestState{
        .request = std::move(pending_req),
        .total_trbs = trb_count,
        .completed_trbs = 0,
        .completed_bytes = 0,
    });
    uep.server->queued_reqs.pop();
    enqueued++;
  }

  if (start_transfer) {
    fdf::debug("Dwc3::UserEpQueueNext ep {} starting transfer {}/{}", uep.ep.ep_num, enqueued,
               uep.server->active_reqs.size());
    CmdEpStartTransfer(uep.ep, first_trb_phys);
    uep.ep.transfer_state = Endpoint::TransferState::kStartingOngoing;
  } else {
    CmdEpUpdateTransfer(uep.ep);
  }
}

void Dwc3::HandleEpTransferCompleteEvent(uint8_t ep_num) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEpTransferCompleteEvent", "ep_num", ep_num);
  if (is_ep0_num(ep_num)) {
    HandleEp0TransferCompleteEvent(ep_num);
    return;
  }

  UserEndpoint* const uep = get_user_endpoint(ep_num);
  ZX_ASSERT(uep != nullptr);
  UserEpCompleteTransfers(*uep);
  uep->ep.transfer_state = Endpoint::TransferState::kIdle;
  uep->ep.rsrc_id = Endpoint::kInvalidResourceId;
  UserEpQueueNext(*uep);
}

void Dwc3::HandleEpTransferInProgressEvent(uint8_t ep_num) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEpTransferInProgressEvent", "ep_num", ep_num);
  if (is_ep0_num(ep_num)) {
    return;
  }
  UserEndpoint* const uep = get_user_endpoint(ep_num);
  ZX_ASSERT(uep != nullptr);
  UserEpCompleteTransfers(*uep);
  UserEpQueueNext(*uep);
}

void Dwc3::UserEpCompleteTransfers(UserEndpoint& uep) {
  if (uep.server->active_reqs.empty()) {
    return;
  }

  // Complete all finished TRBs.
  while (!uep.server->active_reqs.empty()) {
    auto trb_res = uep.fifo.ReadOne();
    if (trb_res.is_error()) {
      fdf::error("ReadOne failed: {}", trb_res.status_string());
      break;
    }
    const dwc3_trb_t& trb = *trb_res;

    if (trb.control & TRB_HWO) {
      // not yet completed by the controller
      break;
    }

    auto& current_req = uep.server->active_reqs.front();
    auto& req = std::get<usb::FidlRequest>(current_req.request);
    size_t actual =
        req->data()->size() > current_req.completed_trbs
            ? req->data()->at(current_req.completed_trbs).size().value() - TRB_BUFSIZ(trb.status)
            // If we have more completed TRBs than data regions, it means we
            // have completed a ZLP.
            : 0;
    current_req.completed_trbs++;
    current_req.completed_bytes += actual;
    uep.fifo.AdvanceRead();

    if (current_req.completed_trbs != current_req.total_trbs) {
      continue;
    }

    uep.ep.total_transfers++;
    uep.ep.total_bytes += current_req.completed_bytes;
    uep.server->RequestComplete(ZX_OK, current_req.completed_bytes, std::move(req),
                                /*send_now=*/false);
    uep.server->active_reqs.pop();
  }
}

void Dwc3::HandleEpTransferNotReadyEvent(uint8_t ep_num, uint32_t stage) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEpTransferNotReadyEvent", "ep_num", ep_num, "stage", stage);
  if (is_ep0_num(ep_num)) {
    HandleEp0TransferNotReadyEvent(ep_num, stage);
    return;
  }

  UserEndpoint* const uep = get_user_endpoint(ep_num);
  ZX_ASSERT(uep != nullptr);
  uep->ep.got_not_ready = true;
  UserEpQueueNext(*uep);
}

void Dwc3::HandleEpTransferStartedEvent(uint8_t ep_num, uint32_t rsrc_id) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEpTransferStartedEvent", "ep_num", ep_num, "rsrc_id",
                 rsrc_id);
  if (is_ep0_num(ep_num)) {
    ((ep_num == kEp0Out) ? ep0_.out : ep0_.in).rsrc_id = rsrc_id;
    return;
  }

  UserEndpoint* const uep = get_user_endpoint(ep_num);
  ZX_ASSERT(uep != nullptr);
  uep->ep.rsrc_id = rsrc_id;
  switch (uep->ep.transfer_state) {
    case Endpoint::TransferState::kIdle:
    case Endpoint::TransferState::kCanceling:
    case Endpoint::TransferState::kActiveOngoing:
    case Endpoint::TransferState::kActiveSingle:
      fdf::warn("Dwc3::HandleEpTransferStartedEvent ep {} in unexpected state {}", ep_num,
                uep->ep.transfer_state);
      break;
    case Endpoint::TransferState::kStartingSingle:
      uep->ep.transfer_state = Endpoint::TransferState::kActiveSingle;
      break;
    case Endpoint::TransferState::kStartingOngoing:
      uep->ep.transfer_state = Endpoint::TransferState::kActiveOngoing;
      break;
    case Endpoint::TransferState::kPendingCancel:
      // We've been requested to end the transfer.
      uep->ep.transfer_state = Endpoint::TransferState::kCanceling;
      CmdEpEndTransfer(uep->ep);
      break;
  }
  // Attempt to enqueue more things now that we have a resource ID. States that
  // can't enqueue any more are handled within.
  UserEpQueueNext(*uep);
}

void Dwc3::HandleEpTransferEndedEvent(uint8_t ep_num) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEpTransferEndedEvent", "ep_num", ep_num);
  if (is_ep0_num(ep_num)) {
    return;
  }

  UserEndpoint* const uep = get_user_endpoint(ep_num);
  ZX_ASSERT(uep != nullptr);
  fdf::debug("Dwc3::HandleEpTransferEndedEvent ep {}", ep_num);

  if (uep->server) {
    // Reason may not be set if the endpoint is reset/halted from under us, fallback to
    // IO_REFUSED.
    zx_status_t reason = uep->server->pending_cancel_reason.value_or(ZX_ERR_IO_REFUSED);
    uep->server->pending_cancel_reason.reset();
    size_t pending_trbs = 0;
    while (!uep->server->active_reqs.empty()) {
      auto& request_state = uep->server->active_reqs.front();
      pending_trbs += request_state.total_trbs - request_state.completed_trbs;
      uep->server->RequestComplete(reason, 0, std::move(request_state.request), /*send_now=*/false);
      uep->server->active_reqs.pop();
    }
    uep->server->SendCompletions();
    size_t active_count = uep->fifo.GetActiveCount();
    ZX_ASSERT_MSG(active_count == pending_trbs, "%ld == %ld", active_count, pending_trbs);

    uep->server->FlushCancelCompleters(ZX_OK);
  }
  uep->fifo.Clear();
  uep->ep.transfer_state = Endpoint::TransferState::kIdle;
  uep->ep.rsrc_id = Endpoint::kInvalidResourceId;
  UserEpQueueNext(*uep);
}

}  // namespace dwc3
