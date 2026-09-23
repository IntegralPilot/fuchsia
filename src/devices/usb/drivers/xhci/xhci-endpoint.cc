// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/xhci-endpoint.h"

#include <lib/fit/defer.h>

#include "src/devices/usb/drivers/xhci/usb-xhci.h"

namespace usb_xhci {

Endpoint::Endpoint(UsbXhci* hci, uint32_t device_id, uint8_t address)
    : usb::EndpointServer(hci->bti(), address, usb::ScatterGatherSupport::kSupported),
      hci_(hci),
      device_id_(device_id) {}

namespace {

inline usb_setup_t ToBanjo(fuchsia_hardware_usb_descriptor::UsbSetup setup) {
  return usb_setup_t{
      .bm_request_type = setup.bm_request_type(),
      .b_request = setup.b_request(),
      .w_value = setup.w_value(),
      .w_index = setup.w_index(),
      .w_length = setup.w_length(),
  };
}

}  // namespace

zx_status_t Endpoint::Init(EventRing* event_ring, fdf::MmioBuffer* mmio) {
  return transfer_ring_.Init(hci_->GetPageSize(), hci_->bti(), event_ring,
                             hci_->Is32BitController(), mmio, hci_);
}

void Endpoint::OnUnbound(fidl::UnbindInfo info,
                         fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) {
  if (hci_->Running()) {
    auto status = hci_->UsbHciCancelAll(device_id_, ep_addr());
    if (status != ZX_OK) {
      fdf::error("Could not cancel all {}", zx_status_get_string(status));
    }

    status = hci_->RunSynchronously(kPrimaryInterrupter,
                                    hci_->UsbHciDisableEndpoint(device_id_, ep_addr()));
    if (status != ZX_OK) {
      fdf::error("Could not disable endpoint {}", zx_status_get_string(status));
    }
  }

  usb::EndpointServer::OnUnbound(info, std::move(server_end));
}

void Endpoint::QueueRequests(QueueRequestsRequest& request,
                             QueueRequestsCompleter::Sync& completer) {
  for (auto& req : request.req()) {
    QueueRequest(usb::FidlRequest{std::move(req)});
  }
  uint8_t index = static_cast<uint8_t>(XhciEndpointIndex(ep_addr()) - 1);
  hci_->RingDoorbell(hci_->GetDeviceState()[device_id_]->GetSlot(), ep_addr() ? 2 + index : 1);
}

void Endpoint::CancelAll(CancelAllCompleter::Sync& completer) {
  auto status = hci_->UsbHciCancelAll(device_id_, ep_addr());
  if (status != ZX_OK) {
    completer.Reply(fit::as_error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void Endpoint::QueueRequest(usb::RequestVariant request) {
  if (!hci_->Running()) {
    RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(request));
    return;
  }
  {
    fbl::AutoLock _(&hci_->GetDeviceState()[device_id_]->transaction_lock());
    if (!hci_->GetDeviceState()[device_id_]->GetSlot()) {
      RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(request));
      return;
    }
  }
  if (unlikely(ep_addr() == 0)) {
    return ControlRequestQueue(std::move(request));
  }
  NormalRequestQueue(std::move(request));
}

void Endpoint::ControlRequestQueue(usb::RequestVariant request) {
  fbl::AutoLock transaction_lock(&hci_->GetDeviceState()[device_id_]->transaction_lock());
  if (hci_->GetDeviceState()[device_id_]->IsDisconnecting()) {
    // Device is disconnecting. Release lock because we no longer will be using device_state,
    // complete request, and return from function.
    transaction_lock.release();
    RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(request));
    return;
  }
  if (transfer_ring_.stalled()) {
    transaction_lock.release();
    RequestComplete(ZX_ERR_IO_REFUSED, 0, std::move(request));
    return;
  }
  auto context = transfer_ring_.AllocateContext();
  if (!context) {
    transaction_lock.release();
    RequestComplete(ZX_ERR_NO_MEMORY, 0, std::move(request));
    return;
  }
  TransferRing::State transaction;
  TRB* setup;
  zx_status_t status = transfer_ring_.AllocateTRB(&setup, &transaction);
  auto rollback_transaction = [=, this]() __TA_NO_THREAD_SAFETY_ANALYSIS {
    transfer_ring_.Restore(transaction);
  };
  if (status != ZX_OK) {
    rollback_transaction();
    transaction_lock.release();
    RequestComplete(status, 0, std::move(request));
    return;
  }

  context->request.emplace(std::move(request));
  UsbRequestState pending_transfer;
  pending_transfer.context = std::move(context);
  pending_transfer.setup = setup;
  pending_transfer.transaction = transaction;
  status = ControlRequestAllocationPhase(&pending_transfer);
  auto call = fit::defer([&]() __TA_NO_THREAD_SAFETY_ANALYSIS {
    rollback_transaction();
    transaction_lock.release();
    RequestComplete(status, 0, std::move(*context->request));
  });
  if (status != ZX_OK) {
    return;
  }
  status = ControlRequestStatusPhase(&pending_transfer);
  if (status != ZX_OK) {
    return;
  }
  status = ControlRequestDataPhase(&pending_transfer);
  if (status != ZX_OK) {
    return;
  }
  ControlRequestSetupPhase(&pending_transfer);
  ControlRequestCommit(&pending_transfer);
  call.cancel();
}

zx_status_t Endpoint::ControlRequestAllocationPhase(UsbRequestState* state) {
  state->setup_cycle = state->setup->status;
  state->setup->status = 0;
  size_t length = std::holds_alternative<usb::FidlRequest>(*state->context->request)
                      ? std::get<usb::FidlRequest>(*state->context->request).length()
                      : std::get<Request>(*state->context->request).request()->header.length;
  if (length) {
    auto status = std::visit([&](auto&& req) -> zx_status_t { return req.PhysMap(hci_->bti()); },
                             *state->context->request);
    if (status != ZX_OK) {
      return status;
    }

    auto iters = get_iter(*state->context->request, k64KiB);
    if (iters.is_error()) {
      return iters.error_value();
    }

    TRB* current_trb = nullptr;
    for (auto& iter : iters.value()) {
      for (auto [paddr, len] : iter) {
        if (!len) {
          break;
        }
        state->packet_count++;
        TRB* prev = current_trb;
        zx_status_t status = transfer_ring_.AllocateTRB(&current_trb, nullptr);
        if (status != ZX_OK) {
          return status;
        }
        static_assert(sizeof(TRB*) == sizeof(uint64_t));
        if (likely(prev)) {
          prev->ptr = reinterpret_cast<uint64_t>(current_trb);
        } else {
          state->first_trb = current_trb;
        }
      }
    }
  }
  return ZX_OK;
}

zx_status_t Endpoint::ControlRequestStatusPhase(UsbRequestState* state) {
  state->interrupter = 0;
  bool status_in = true;
  // See table 4-7 in section 4.11.2.2
  auto bm_request_type =
      std::holds_alternative<Request>(*state->context->request)
          ? std::get<Request>(*state->context->request).request()->setup.bm_request_type
          : std::get<usb::FidlRequest>(*state->context->request)
                ->information()
                ->control()
                ->setup()
                ->bm_request_type();
  if (state->first_trb && (bm_request_type & USB_DIR_IN)) {
    status_in = false;
  }
  zx_status_t status = transfer_ring_.AllocateTRB(&state->status_trb_ptr, nullptr);
  if (status != ZX_OK) {
    return status;
  }
  Control::FromTRB(state->status_trb_ptr)
      .set_Cycle(state->status_trb_ptr->status)
      .set_Type(Control::Status)
      .ToTrb(state->status_trb_ptr);
  state->status_trb_ptr->status = 0;
  auto* status_trb = static_cast<Status*>(state->status_trb_ptr);
  status_trb->set_DIRECTION(status_in).set_INTERRUPTER(state->interrupter).set_IOC(1);
  return ZX_OK;
}

zx_status_t Endpoint::ControlRequestDataPhase(UsbRequestState* state) {
  // Data stage
  if (state->first_trb) {
    auto iters = get_iter(*state->context->request, k64KiB);
    if (iters.is_error()) {
      return iters.error_value();
    }

    TRB* current = state->first_trb;
    for (auto& iter : iters.value()) {
      for (auto [paddr, len] : iter) {
        if (!len) {
          break;
        }
        state->packet_count--;
        TRB* next = reinterpret_cast<TRB*>(current->ptr);
        uint32_t pcs = current->status;
        current->status = 0;
        enum Control::Type type;
        if (current == state->first_trb) {
          type = Control::Data;
          ControlData* data = reinterpret_cast<ControlData*>(current);
          auto bm_request_type =
              std::holds_alternative<Request>(*state->context->request)
                  ? std::get<Request>(*state->context->request).request()->setup.bm_request_type
                  : std::get<usb::FidlRequest>(*state->context->request)
                        ->information()
                        ->control()
                        ->setup()
                        ->bm_request_type();
          // Control transfers always get interrupter 0 (we consider those to be low-priority)
          // TODO(https://fxbug.dev/42109334): Change bus snooping options based on input from
          // higher-level drivers.
          data->set_CHAIN(next != nullptr)
              .set_DIRECTION((bm_request_type & USB_DIR_IN) != 0)
              .set_INTERRUPTER(0)
              .set_LENGTH(len)
              .set_SIZE(state->packet_count)
              .set_ISP(true)
              .set_NO_SNOOP(!hci_->HasCoherentCache());
        } else {
          type = Control::Normal;
          Normal* data = reinterpret_cast<Normal*>(current);
          data->set_CHAIN(next != nullptr)
              .set_INTERRUPTER(0)
              .set_LENGTH(len)
              .set_SIZE(state->packet_count)
              .set_ISP(true)
              .set_NO_SNOOP(!hci_->HasCoherentCache());
        }
        current->ptr = paddr;
        Control::FromTRB(current).set_Cycle(pcs).set_Type(type).ToTrb(current);
        current = next;
      }
    }
  }
  return ZX_OK;
}

void Endpoint::ControlRequestSetupPhase(UsbRequestState* state) {
  // Setup phase (4.11.2.2)
  auto bm_request_type =
      std::holds_alternative<Request>(*state->context->request)
          ? std::get<Request>(*state->context->request).request()->setup.bm_request_type
          : std::get<usb::FidlRequest>(*state->context->request)
                ->information()
                ->control()
                ->setup()
                ->bm_request_type();
  const auto& setup = std::holds_alternative<Request>(*state->context->request)
                          ? std::get<Request>(*state->context->request).request()->setup
                          : ToBanjo(*std::get<usb::FidlRequest>(*state->context->request)
                                         ->information()
                                         ->control()
                                         ->setup());
  memcpy(&state->setup->ptr, &setup, sizeof(setup));
  Setup* setup_trb = reinterpret_cast<Setup*>(state->setup);
  setup_trb->set_INTERRUPTER(state->interrupter)
      .set_length(8)
      .set_IDT(1)
      .set_TRT(((bm_request_type & USB_DIR_IN) != 0) ? Setup::IN : Setup::OUT);
  hw_mb();
}

void Endpoint::ControlRequestCommit(UsbRequestState* state) {
  // Start the transaction!
  if (std::holds_alternative<Request>(*state->context->request) && !hci_->HasCoherentCache()) {
    usb_request_cache_flush_invalidate(
        std::get<Request>(*state->context->request).request(), 0,
        std::get<Request>(*state->context->request).request()->header.length);
  }
  transfer_ring_.AssignContext(state->status_trb_ptr, std::move(state->context), state->first_trb,
                               state->setup);
  Control::FromTRB(state->setup)
      .set_Type(Control::Setup)
      .set_Cycle(state->setup_cycle)
      .ToTrb(state->setup);
  transfer_ring_.CommitTransaction(state->transaction);
}

void Endpoint::NormalRequestQueue(usb::RequestVariant request) {
  UsbRequestState pending_transfer;
  uint8_t index = static_cast<uint8_t>(XhciEndpointIndex(ep_addr()) - 1);
  fbl::AutoLock transaction_lock(&hci_->GetDeviceState()[device_id_]->transaction_lock());
  if (hci_->GetDeviceState()[device_id_]->IsDisconnecting()) {
    transaction_lock.release();
    RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(request));
    return;
  }
  if (transfer_ring_.stalled()) {
    transaction_lock.release();
    RequestComplete(ZX_ERR_IO_REFUSED, 0, std::move(request));
    return;
  }
  auto* control =
      reinterpret_cast<uint32_t*>(hci_->GetDeviceState()[device_id_]->GetInputContext()->virt());
  auto* endpoint_context = reinterpret_cast<EndpointContext*>(
      reinterpret_cast<unsigned char*>(control) + (hci_->slot_size_bytes() * (2 + (index + 1))));
  if (!transfer_ring_.active()) {
    transaction_lock.release();
    RequestComplete(ZX_ERR_INTERNAL, 0, std::move(request));
    return;
  }
  pending_transfer.burst_size = endpoint_context->MaxBurstSize() + 1;
  pending_transfer.max_packet_size = endpoint_context->MAX_PACKET_SIZE();
  pending_transfer.context = transfer_ring_.AllocateContext();
  if (!pending_transfer.context) {
    transaction_lock.release();
    RequestComplete(ZX_ERR_NO_MEMORY, 0, std::move(request));
    return;
  }
  pending_transfer.context->request.emplace(std::move(request));

  if (transfer_ring_.IsIsochronous()) {
    // Isoc endpoints of the default interface (i.e. b_alternate_setting=0) are required to have a
    // w_max_packet_size=0 to prevent enumerated devices from reserving bandwidth by default. We'll
    // consider any request for a zero-length pipe as invalid (see USB 2.0 spec. 5.6.2).
    if (pending_transfer.max_packet_size == 0) {
      transaction_lock.release();
      RequestComplete(ZX_ERR_INVALID_ARGS, 0, std::move(*pending_transfer.context->request));
      return;
    }

    // Release the lock while we're sleeping to avoid blocking
    // other operations.
    hci_->GetDeviceState()[device_id_]->transaction_lock().Release();
    auto status = WaitForIsochronousReady(
        std::holds_alternative<usb::FidlRequest>(*pending_transfer.context->request)
            ? *std::get<usb::FidlRequest>(*pending_transfer.context->request)
                   ->information()
                   ->isochronous()
                   ->frame_id()
            : std::get<Request>(*pending_transfer.context->request).request()->header.frame);
    hci_->GetDeviceState()[device_id_]->transaction_lock().Acquire();
    if (status != ZX_OK) {
      transaction_lock.release();
      RequestComplete(status, 0, std::move(*pending_transfer.context->request));
      return;
    }

    // Check again since we've re-acquired lock
    if (hci_->GetDeviceState()[device_id_]->IsDisconnecting()) {
      transaction_lock.release();
      RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(*pending_transfer.context->request));
      return;
    }
    if (transfer_ring_.stalled()) {
      transaction_lock.release();
      RequestComplete(ZX_ERR_IO_REFUSED, 0, std::move(*pending_transfer.context->request));
      return;
    }
    if (!transfer_ring_.active()) {
      transaction_lock.release();
      RequestComplete(ZX_ERR_INTERNAL, 0, std::move(*pending_transfer.context->request));
      return;
    }
  }

  // Start the transaction
  pending_transfer.transaction = transfer_ring_.SaveState();
  auto rollback_transaction = [&]() __TA_NO_THREAD_SAFETY_ANALYSIS {
    transfer_ring_.Restore(pending_transfer.transaction);
  };
  auto status = StartNormalTransaction(
      &pending_transfer,
      static_cast<uint8_t>(hci_->GetDeviceState()[device_id_]->GetInterrupterTarget()));
  if (status != ZX_OK) {
    rollback_transaction();
    transaction_lock.release();
    RequestComplete(status, 0, std::move(*pending_transfer.context->request));
    return;
  }
  // Continue the transaction
  status = ContinueNormalTransaction(&pending_transfer);
  if (status != ZX_OK) {
    rollback_transaction();
    transaction_lock.release();
    RequestComplete(status, 0, std::move(*pending_transfer.context->request));
    return;
  }
  // Commit the transaction -- starting the actual transfer
  CommitNormalTransaction(&pending_transfer);
}

zx_status_t Endpoint::WaitForIsochronousReady(uint64_t target_frame) {
  // Cannot schedule more than 895 ms into the future per section 4.11.2.5
  // in the xHCI specification (revision 1.2)
  constexpr int kMaxSchedulingInterval = 895;
  if (target_frame) {
    uint64_t frame = hci_->UsbHciGetCurrentFrame();
    while (static_cast<int32_t>(target_frame - frame) > kMaxSchedulingInterval) {
      uint32_t time = static_cast<uint32_t>((target_frame - frame) - kMaxSchedulingInterval);
      zx::nanosleep(zx::deadline_after(zx::msec(time)));
      frame = hci_->UsbHciGetCurrentFrame();
    }

    if (target_frame < frame) {
      return ZX_ERR_IO_MISSED_DEADLINE;
    }
  }
  return ZX_OK;
}

zx_status_t Endpoint::StartNormalTransaction(UsbRequestState* state, uint8_t interrupter_target) {
  size_t packet_count = 0;

  // Normal transfer
  auto status = std::visit([&](auto&& req) -> zx_status_t { return req.PhysMap(hci_->bti()); },
                           *state->context->request);
  if (status != ZX_OK) {
    return status;
  }
  size_t pending_len = std::holds_alternative<usb::FidlRequest>(*state->context->request)
                           ? std::get<usb::FidlRequest>(*state->context->request).length()
                           : std::get<Request>(*state->context->request).request()->header.length;
  uint32_t total_len = 0;
  auto iters = get_iter(*state->context->request, k64KiB);
  if (iters.is_error()) {
    return iters.error_value();
  }

  for (auto& iter : iters.value()) {
    for (auto [paddr, len] : iter) {
      if (len > pending_len) {
        len = pending_len;
      }
      if (!paddr) {
        break;
      }
      if (!len) {
        continue;
      }
      total_len += static_cast<uint32_t>(len);
      packet_count++;
      pending_len -= len;
    }
  }

  if (pending_len) {
    // Something doesn't add up here....
    return ZX_ERR_BAD_STATE;
  }
  // Allocate contiguous memory
  auto contig_trb_info = transfer_ring_.AllocateContiguous(packet_count);
  if (contig_trb_info.is_error()) {
    return contig_trb_info.error_value();
  }
  state->info = contig_trb_info.value();
  state->total_len = total_len;
  state->packet_count = packet_count;
  state->first_cycle = state->info.first()[0].status;
  state->first_trb = state->info.first().data();
  state->last_trb = state->info.trbs.data() + (packet_count - 1);
  state->interrupter = static_cast<uint8_t>(interrupter_target);

  return ZX_OK;
}

zx_status_t Endpoint::ContinueNormalTransaction(UsbRequestState* state) {
  // Data stage
  size_t pending_len = std::holds_alternative<usb::FidlRequest>(*state->context->request)
                           ? std::get<usb::FidlRequest>(*state->context->request).length()
                           : std::get<Request>(*state->context->request).request()->header.length;
  auto current_nop = state->info.nop.data();
  if (current_nop) {
    while (Control::FromTRB(current_nop).Type() == Control::Nop) {
      bool producer_cycle_state = current_nop->status;
      bool cycle = (current_nop == state->first_trb) ? !producer_cycle_state : producer_cycle_state;
      Control::FromTRB(current_nop).set_Cycle(cycle).ToTrb(current_nop);
      current_nop->status = 0;
      current_nop++;
    }
  }
  if (state->first_trb) {
    TRB* current = state->info.trbs.data();
    auto iters = get_iter(*state->context->request, k64KiB);
    if (iters.is_error()) {
      return iters.error_value();
    }

    for (auto& iter : iters.value()) {
      for (auto [paddr, len] : iter) {
        if (!len) {
          break;
        }
        len = std::min(len, pending_len);
        pending_len -= len;
        state->packet_count--;
        TRB* next = current + 1;
        if (next == state->last_trb + 1) {
          next = nullptr;
        }
        uint32_t pcs = current->status;
        current->status = 0;
        enum Control::Type type;
        if ((transfer_ring_.IsIsochronous() && state->first_trb == current)) {
          // Force direct mode as workaround for USB audio latency issue.
          type = Control::Isoch;
          Isoch* data = reinterpret_cast<Isoch*>(current);
          // Burst size is number of packets, not bytes
          uint32_t burst_size = state->burst_size;
          uint32_t packet_size = state->max_packet_size;
          uint32_t packet_count = state->total_len / packet_size;
          if (!packet_count) {
            packet_count = 1;
          }
          // Number of bursts - 1
          uint32_t burst_count = packet_count / burst_size;
          if (burst_count) {
            burst_count--;
          }
          // Zero-based last-burst-packet count (where 0 == 1 packet)
          uint32_t last_burst_packet_count = packet_count % burst_size;
          if (last_burst_packet_count) {
            last_burst_packet_count--;
          }
          auto frame_id = std::holds_alternative<usb::FidlRequest>(*state->context->request)
                              ? *std::get<usb::FidlRequest>(*state->context->request)
                                     ->information()
                                     ->isochronous()
                                     ->frame_id()
                              : std::get<Request>(*state->context->request).request()->header.frame;
          data->set_CHAIN(next != nullptr)
              .set_SIA(frame_id == 0)
              .set_TLBPC(last_burst_packet_count)
              .set_FrameID(frame_id % 2048)
              .set_TBC(burst_count)
              .set_INTERRUPTER(state->interrupter)
              .set_LENGTH(len)
              .set_SIZE(packet_count)
              .set_NO_SNOOP(!hci_->HasCoherentCache())
              .set_IOC(next == nullptr)
              .set_ISP(true);
        } else {
          type = Control::Normal;
          Normal* data = reinterpret_cast<Normal*>(current);
          data->set_CHAIN(next != nullptr)
              .set_INTERRUPTER(state->interrupter)
              .set_LENGTH(len)
              .set_SIZE(state->packet_count)
              .set_NO_SNOOP(!hci_->HasCoherentCache())
              .set_IOC(next == nullptr)
              .set_ISP(true);
        }

        current->ptr = paddr;
        Control::FromTRB(current)
            .set_Cycle(unlikely(current == state->first_trb) ? !pcs : pcs)
            .set_Type(type)
            .ToTrb(current);
        current = next;
      }
    }
  }
  return ZX_OK;
}

void Endpoint::CommitNormalTransaction(UsbRequestState* state) {
  hw_mb();
  // Start the transaction!
  if (std::holds_alternative<Request>(*state->context->request) && !hci_->HasCoherentCache()) {
    usb_request_cache_flush_invalidate(
        std::get<Request>(*state->context->request).request(), 0,
        std::get<Request>(*state->context->request).request()->header.length);
  }
  // In FIDL mode, we expect cache to be flushed already
  transfer_ring_.AssignContext(state->last_trb, std::move(state->context), state->first_trb);
  Control::FromTRB(state->first_trb).set_Cycle(state->first_cycle).ToTrb(state->first_trb);

  transfer_ring_.CommitTransaction(state->transaction);
}

}  // namespace usb_xhci
