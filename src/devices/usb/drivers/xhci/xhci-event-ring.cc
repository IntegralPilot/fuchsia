// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/xhci-event-ring.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/fit/defer.h>
#include <lib/fpromise/promise.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <optional>

#include "src/devices/usb/drivers/xhci/usb-xhci.h"
#include "src/devices/usb/drivers/xhci/xhci-enumeration.h"

namespace usb_xhci {

// The minimum required number of event ring segment table entries.
static constexpr uint16_t kMinERSTEntries = 16;

// The target number of TRBs we would like to have in our event ring, available
// to the HC, at startup.
static constexpr size_t kTargetEventRingTRBs = 2048;

// The number of TRBs we should update ERDP Reg after. According to note in 4.9.4, we should process
// as many Events as possible before writing to ERDP. But practically, we need to limit the maximum
// number of TRBs before writing. Otherwise, the lag between consuming an ED and moving the ERDP
// forward may cause the ring to become full with consumed EDs which have yet to be relinquished.
static constexpr size_t kUpdateERDPAfterTRBs = 128;

zx_status_t EventRingSegmentTable::Init(size_t page_size, const zx::bti& bti, bool is_32bit,
                                        uint32_t erst_max, ERSTSZ erst_size,
                                        const dma_buffer::BufferFactory& factory,
                                        fdf::MmioBuffer* mmio) {
  erst_size_ = erst_size;
  bti_ = &bti;
  page_size_ = page_size;
  is_32bit_ = is_32bit;
  mmio_.emplace(mmio->View(0));
  zx_status_t status =
      factory.CreatePaged(bti, page_size_, dma_buffer::CacheOptions::kDisabled, &erst_);
  if (status != ZX_OK) {
    return status;
  }
  if (is_32bit && (erst_->phys()[0] >= UINT32_MAX)) {
    return ZX_ERR_NO_MEMORY;
  }

  count_ = page_size / sizeof(ERSTEntry);
  if (count_ > erst_max) {
    count_ = erst_max;
  }
  entries_ = static_cast<ERSTEntry*>(erst_->virt());
  return ZX_OK;
}

zx_status_t EventRingSegmentTable::AddSegment(zx_paddr_t paddr) {
  if (offset_ >= count_) {
    if (offset_ > count_) {
      return ZX_ERR_BAD_STATE;
    }
    return ZX_ERR_NO_MEMORY;
  }
  ERSTEntry entry;
  entry.address_low = static_cast<uint32_t>(paddr & UINT32_MAX);
  entry.address_high = static_cast<uint32_t>(paddr >> 32);
  entry.size = static_cast<uint16_t>(page_size_ / kMinERSTEntries);
  entries_[offset_] = entry;
  hw_mb();
  offset_++;
  erst_size_.set_TableSize(offset_).WriteTo(&mmio_.value());
  erst_pressure_++;
  return ZX_OK;
}

zx_status_t EventRing::Init(size_t page_size, const zx::bti& bti, fdf::MmioBuffer* buffer,
                            bool is_32bit, uint32_t erst_max, ERSTSZ erst_size, ERDP erdp_reg,
                            IMAN iman_reg, uint8_t cap_length, HCSPARAMS1 hcs_params_1,
                            CommandRing* command_ring, DoorbellOffset doorbell_offset, UsbXhci* hci,
                            HCCPARAMS1 hcc_params_1, uint64_t* dcbaa, uint16_t interrupter,
                            inspect::Node* interrupter_node) {
  fbl::AutoLock l(&segment_mutex_);
  erdp_reg_ = erdp_reg;
  hcs_params_1_ = hcs_params_1;
  mmio_ = buffer;
  bti_ = &bti;
  page_size_ = page_size;
  is_32bit_ = is_32bit;
  mmio_ = buffer;
  iman_reg_ = iman_reg;
  cap_length_ = cap_length;
  command_ring_ = command_ring;
  doorbell_offset_ = doorbell_offset;
  hci_ = hci;
  hcc_params_1_ = hcc_params_1;
  dcbaa_ = dcbaa;
  interrupter_ = interrupter;

  if (interrupter_node != nullptr) {
    total_event_trbs_ = interrupter_node->CreateUint("Total Event TRBs", 0);
    max_single_irq_event_trbs_ = interrupter_node->CreateUint("Max single IRQ event TRBs", 0);
    events_ = interrupter_node->CreateChild("Events");
  }

  zx_status_t status =
      segments_.Init(page_size, bti, is_32bit, erst_max, erst_size, hci->buffer_factory(), mmio_);
  if (status != ZX_OK) {
    return status;
  }

  // Attempt to grow our event ring to the desired size.
  do {
    status = AddSegment(true);
    if (status != ZX_OK) {
      fdf::warn(
          "Event ring failed to add a segment during initialization. "
          "The EventRing currently has space for %zu TRBs (status %d)",
          segments_.TrbCount(), status);

      if (segments_.TrbCount() == 0) {
        return status;
      }
      break;
    }
  } while (segments_.CanGrow() && (segments_.TrbCount() < kTargetEventRingTRBs));

  return ZX_OK;
}

void EventRing::RemovePressure() {
  fbl::AutoLock l(&segment_mutex_);
  segments_.RemovePressure();
}

size_t EventRing::GetPressure() {
  fbl::AutoLock l(&segment_mutex_);
  return segments_.Pressure();
}

zx_status_t EventRing::AddSegmentIfNone() {
  if (!erdp_phys_) {
    return AddSegment();
  }
  return ZX_OK;
}

zx_status_t EventRing::AddSegment(bool initialization) {
  if (segments_.Pressure() < segments_.SegmentCount()) {
    segments_.AddPressure();
    return ZX_OK;
  }
  std::unique_ptr<dma_buffer::ContiguousBuffer> buffer;
  {
    std::unique_ptr<dma_buffer::ContiguousBuffer> buffer_tmp;
    zx_status_t status = hci_->buffer_factory().CreateContiguous(
        *bti_, page_size_,
        static_cast<uint32_t>(page_size_ == zx_system_get_page_size() ? 0 : page_size_ >> 12),
        dma_buffer::CacheOptions::kEnabled, &buffer_tmp);
    if (status != ZX_OK) {
      return status;
    }
    buffer = std::move(buffer_tmp);
  }
  if (is_32bit_ && (buffer->phys() >= UINT32_MAX)) {
    return ZX_ERR_NO_MEMORY;
  }
  zx_status_t status = segments_.AddSegment(buffer->phys());
  if (status != ZX_OK) {
    return status;
  }
  bool needs_iterator = false;
  if (!erdp_phys_) {
    erdp_phys_ = buffer->phys();
    erdp_virt_ = static_cast<TRB*>(buffer->virt());
    needs_iterator = true;
  }
  // Segments added to event ring on initialization (before event ring is used) are not considered
  // "new segments"
  buffers_.push_back(std::make_unique<SegmentBuf>(std::move(buffer), !initialization));
  if (needs_iterator) {
    buffers_it_ = buffers_.begin();
  }
  return ZX_OK;
}

fpromise::promise<void, zx_status_t> EventRing::HandlePortStatusChangeEvent(uint8_t port_id) {
  auto sc = PORTSC::Get(cap_length_, port_id).ReadFrom(mmio_);
  std::optional<fpromise::promise<void, zx_status_t>> pending_enumeration;
  // Read status bits
  bool needs_enum = false;

  // xHCI doesn't provide a way of retrieving the port speed prior to a device being fully
  // online (without using ACPI or another out-of-band mechanism).
  // In order to correctly enumerate devices, we use heuristics to try and determine
  // whether or not a port is 2.0 or 3.0.
  if (sc.CCS()) {
    // Wait for the port to exit polling state, if applicable.
    // Only 2.0 ports should go into a polling state, so if we get here,
    // we can be sure that it's a 2.0 port. Some controllers may skip this step though....
    if ((sc.PLS() == PORTSC::Polling) || (hci_->GetPortState()[port_id - 1].is_connected &&
                                          !hci_->GetPortState()[port_id - 1].is_USB3)) {
      // USB 2.0 port connect
      if (!hci_->GetPortState()[port_id - 1].is_connected) {
        // USB 2.0 requires a port reset to advance to U0
        Usb2DeviceAttach(port_id);
        needs_enum = true;
        fdf::debug("Port {} is a USB 2 device and will be enumerated.", port_id);
      }
    } else {
      // USB 3.0 port connect, since we got a connect status bit set,
      // and were not polling.
      if (!hci_->GetPortState()[port_id - 1].is_connected) {
        Usb3DeviceAttach(port_id);
        needs_enum = true;
        fdf::debug("Port {} is a USB 3 device and will be enumerated.", port_id);
      }
      if ((sc.PLS() == PORTSC::U0) && (sc.PED()) && (!sc.PR()) &&
          !hci_->GetPortState()[port_id - 1].link_active) {
        // Set the link active bit here to prevent us from onlining the same device twice.
        hci_->GetPortState()[port_id - 1].link_active = true;
        needs_enum = false;
        pending_enumeration = LinkUp(port_id);
      }
    }

    // Link could be active from connect status change above.
    // To prevent enumerating a device twice, we ensure that the link wasn't previously active
    // before enumerating.
    if ((sc.PLS() == PORTSC::U0) && sc.CCS() && !(hci_->GetPortState()[port_id - 1].link_active)) {
      if (!hci_->GetPortState()[port_id - 1].is_connected) {
        // Spontaneous initialization of USB 3.0 port without going through
        // CSC event. We know this is USB 3.0 since this cannot possibly happen
        // with a 2.0 port.
        hci_->GetPortState()[port_id - 1].is_USB3 = true;
        hci_->GetPortState()[port_id - 1].is_connected = true;
      }
      hci_->GetPortState()[port_id - 1].link_active = true;
      if (!hci_->GetPortState()[port_id - 1].is_USB3) {
        // USB 2.0 specification section 9.2.6.3
        // states that we must wait 10 milliseconds.
        needs_enum = false;
        pending_enumeration =
            hci_->Timeout(interrupter_, zx::deadline_after(zx::msec(10)))
                .and_then([port_id, this]() { return LinkUp(static_cast<uint8_t>(port_id)); })
                .box();
      } else {
        needs_enum = false;
        pending_enumeration = LinkUp(static_cast<uint8_t>(port_id));
      }
    }

  } else {
    // For hubs, we need to take the device offline from the bus's standpoint before tearing down
    // the hub. This means that the slot has to be kept alive until the hub driver is removed.
    hci_->GetPortState()[port_id - 1].retry = false;
    hci_->GetPortState()[port_id - 1].link_active = false;
    hci_->GetPortState()[port_id - 1].is_connected = false;
    hci_->GetPortState()[port_id - 1].is_USB3 = false;
    if (hci_->GetPortState()[port_id - 1].slot_id) {
      ScheduleTask(hci_->DeviceOffline(hci_->GetPortState()[port_id - 1].slot_id));
    }
  }

  // Update registers if not init
  if (sc.OCC()) {
    bool overcurrent = sc.OCA();
    PORTSC::Get(cap_length_, port_id)
        .FromValue(0)
        .set_CCS(sc.CCS())
        .set_PortSpeed(sc.PortSpeed())
        .set_PIC(sc.PIC())
        .set_PLS(sc.PLS())
        .set_PP(sc.PP())
        .set_OCC(1)
        .WriteTo(mmio_);
    if (overcurrent) {
      fdf::error("Port {} has overcurrent active.", static_cast<int>(port_id));
    } else {
      fdf::error("Overcurrent event on port {} cleared.", static_cast<int>(port_id));
    }
  }
  if (sc.CSC()) {
    // Connect status change
    hci_->GetPortState()[port_id - 1].retry = false;
    PORTSC::Get(cap_length_, port_id)
        .FromValue(0)
        .set_CCS(sc.CCS())
        .set_PLC(sc.PLC())
        .set_PortSpeed(sc.PortSpeed())
        .set_PIC(sc.PIC())
        .set_PLS(sc.PLS())
        .set_PP(sc.PP())
        .set_CSC(sc.CSC())
        .WriteTo(mmio_);
  }
  if (sc.PEC()) {
    return fpromise::make_error_promise<zx_status_t>(ZX_ERR_BAD_STATE);
  }
  if (sc.PRC() || sc.WRC()) {
    PORTSC::Get(cap_length_, port_id)
        .FromValue(0)
        .set_CCS(sc.CCS())
        .set_PortSpeed(sc.PortSpeed())
        .set_PIC(sc.PIC())
        .set_PLS(sc.PLS())
        .set_PP(sc.PP())
        .set_PRC(sc.PRC())
        .set_WRC(sc.WRC())
        .WriteTo(mmio_);
  }
  if (pending_enumeration.has_value()) {
    return *std::move(pending_enumeration);
  }
  if (needs_enum) {
    return WaitForPortStatusChange(port_id)
        .and_then([port_id, this]() {
          // Retry enumeration
          SchedulePortStatusChange(port_id, true);
          return fpromise::ok();
        })
        .box();
  }
  return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
}

fpromise::promise<void, zx_status_t> EventRing::WaitForPortStatusChange(uint8_t port_id) {
  fpromise::bridge<TRB*, zx_status_t> bridge;
  auto context = hci_->GetCommandRing()->AllocateContext();
  context->completer = std::move(bridge.completer);
  hci_->GetPortState()[port_id - 1].wait_for_port_status_change_ = std::move(context);
  return bridge.consumer.promise().discard_value();
}

void EventRing::CallPortStatusChanged(fbl::RefPtr<PortStatusChangeState> state) {
  if (state->port_index < state->port_count) {
    ScheduleTask(HandlePortStatusChangeEvent(static_cast<uint8_t>(state->port_index))
                     .then([=, this](fpromise::result<void, zx_status_t>& result)
                               -> fpromise::result<void, zx_status_t> {
                       if (result.is_error()) {
                         if (result.error() == ZX_ERR_BAD_STATE) {
                           return fpromise::error(ZX_ERR_BAD_STATE);
                         }
                       }
                       state->port_index++;
                       CallPortStatusChanged(state);
                       return fpromise::ok();
                     })
                     .box());
  } else {
    if (enumeration_queue_.is_empty()) {
      enumerating_ = false;
    } else {
      enumerating_ = true;
      auto enum_task = enumeration_queue_.pop_front();
      ScheduleTask(HandlePortStatusChangeEvent(enum_task->port_number)
                       .then([this, state, task = std::move(enum_task)](
                                 fpromise::result<void, zx_status_t>& result) mutable
                                 -> fpromise::result<void, zx_status_t> {
                         if (result.is_error()) {
                           if (result.error() == ZX_ERR_BAD_STATE) {
                             return fpromise::error(ZX_ERR_BAD_STATE);
                           }
                           task->completer->complete_error(result.error());
                         } else {
                           task->completer->complete_ok(nullptr);
                         }
                         state->port_index = state->port_count;
                         CallPortStatusChanged(state);
                         return result;
                       }));
    }
  }
}

void EventRing::SchedulePortStatusChange(uint8_t port_id, bool preempt) {
  auto ctx = hci_->GetCommandRing()->AllocateContext();
  ctx->port_number = port_id;
  fpromise::bridge<TRB*, zx_status_t> bridge;
  ctx->completer = std::move(bridge.completer);
  ScheduleTask(bridge.consumer.promise().then(
      [=](fpromise::result<TRB*, zx_status_t>& result) { return result; }));
  if (preempt) {
    enumeration_queue_.push_front(std::move(ctx));
  } else {
    enumeration_queue_.push_back(std::move(ctx));
  }
  if (!enumerating_) {
    auto state = fbl::MakeRefCounted<PortStatusChangeState>(0, 0);
    CallPortStatusChanged(std::move(state));
  }
}

zx_status_t EventRing::Ring0Bringup() {
  hci_->WaitForBringup();
  enumerating_ = false;
  return ZX_OK;
}

void EventRing::ScheduleTask(fpromise::promise<void, zx_status_t> promise) {
  auto continuation = promise.or_else([=, this](const zx_status_t& status) {
    // ZX_ERR_BAD_STATE is a special value that we use to signal
    // a fatal error in xHCI. When this occurs, we should immediately
    // attempt to shutdown the controller. This error cannot be recovered from.
    if (status == ZX_ERR_BAD_STATE) {
      fdf::error("Scheduled task returned a fatal error, shutting down");
      hci_->Shutdown(status);
    } else if (status != ZX_ERR_IO_NOT_PRESENT) {
      fdf::warn("Scheduled task failed: {}", zx_status_get_string(status));
    }
  });
  executor_.schedule_task(std::move(continuation));
}

void EventRing::RunUntilIdle() { executor_.run_until_idle(); }

bool EventRing::StallWorkaroundForDefectiveHubs(std::unique_ptr<TRBContext>& context) {
  ZX_ASSERT(std::holds_alternative<Request>(*context->request));
  // Workaround for full-speed hub issue in Gateway keyboard
  auto request = std::get<Request>(*context->request).request();
  if ((request->header.ep_address == 0) && (request->setup.b_request == USB_REQ_GET_DESCRIPTOR) &&
      (request->setup.w_index == 0) && (request->setup.w_value == (USB_DT_DEVICE_QUALIFIER << 8))) {
    usb_device_qualifier_descriptor_t* desc;
    if ((std::get<Request>(*context->request).Mmap(reinterpret_cast<void**>(&desc)) == ZX_OK) &&
        (request->header.length >= sizeof(desc))) {
      desc->b_device_protocol =
          0;  // Don't support multi-TT unless we're sure the device supports it.
      ScheduleTask(
          hci_->UsbHciResetEndpointAsync(request->header.device_id, 0)
              .then([ctx = std::move(context)](fpromise::result<void, zx_status_t>& result) {
                if (result.is_ok()) {
                  std::get<Request>(*ctx->request).Complete(ZX_OK, sizeof(*desc));
                } else {
                  std::get<Request>(*ctx->request).Complete(result.error(), 0);
                }
                return fpromise::result<void, zx_status_t>(fpromise::ok());
              }));

      return true;
    }
  }
  return false;
}

void EventRing::AdvanceErdp() {
  enum {
    STOP = 0,
    INCREMENT = 1,
    NEXT = 2,
    WRAP_AROUND = 3,
  } action = STOP;

  reevaluate_ = false;
  fbl::AutoLock l(&segment_mutex_);
  if (unlikely((reinterpret_cast<size_t>(erdp_virt_ + 1) / 4096) !=
               (reinterpret_cast<size_t>(erdp_virt_) / 4096))) {
    // Page transition -- next buffer
    auto next_buffer = buffers_it_;
    next_buffer++;
    ZX_DEBUG_ASSERT(next_buffer != buffers_it_);
    if (unlikely(next_buffer == buffers_.end())) {
      // Last buffer: wrap around
      action = WRAP_AROUND;
      buffers_it_->new_segment = false;
    } else if (unlikely(next_buffer->new_segment)) {
      // New segment. Check for valid Completion Code to see if HW is using the new segment yet.
      if (static_cast<CommandCompletionEvent*>(next_buffer->buf->virt())->CompletionCode() ==
          CommandCompletionEvent::Invalid) {
        // Invalid completion code. New segment not in use yet, so we need to check if HW has
        // wrapped around to the first segment in the ring. We need to do this because adding a new
        // segment can race with the position of HW's enqueue pointer, it might have already started
        // writing to the first segment before seeing that there is a new segment available.
        if (Control::FromTRB(reinterpret_cast<TRB*>(buffers_.front().buf->virt())).Cycle() ==
            ccs_) {
          // HW hasn't started using the first segment, or the new segment, yet so we can't tell
          // which direction the enqueue pointer will move next. Set reevaluate_ and return an
          // invalid TRB to wait for the next interrupt and try again.
          reevaluate_ = true;
          return;
        }
        // HW has wrapped around to use the first segment.
        action = WRAP_AROUND;
      } else {
        // Valid completion code. New segment already in use.
        action = NEXT;
      }
    } else {
      // Not new segment.
      action = NEXT;
    }
    buffers_it_->new_segment = false;
  } else {
    action = INCREMENT;
  }

  switch (action) {
    case INCREMENT: {
      // Increment within segment
      erdp_virt_++;
      erdp_phys_ += sizeof(TRB);
    } break;
    case NEXT: {
      // Next segment
      buffers_it_++;
      erdp_virt_ = reinterpret_cast<TRB*>((*buffers_it_).buf->virt());
      erdp_phys_ = (*buffers_it_).buf->phys();
      segment_index_ = (segment_index_ + 1) & 0b111;
    } break;
    case WRAP_AROUND: {
      // Wrap around to first segment
      ccs_ = !ccs_;
      buffers_it_ = buffers_.begin();
      erdp_virt_ = reinterpret_cast<TRB*>((*buffers_it_).buf->virt());
      erdp_phys_ = (*buffers_it_).buf->phys();
      segment_index_ = 0;
    } break;
    default: {
      fdf::error("This should not happen.");
    } break;
  }
}

std::optional<Control> EventRing::CurrentErdp() {
  if (reevaluate_) {
    return std::nullopt;
  }

  {
    fbl::AutoLock l(&segment_mutex_);
    if (unlikely(buffers_it_->new_segment)) {
      // On the first pass through a new segment the cycle bit is invalid and software should use
      // the completion code to check if the event TRB is valid. Section 4.9.4.1.
      if (static_cast<CommandCompletionEvent*>(erdp_virt_)->CompletionCode() ==
          CommandCompletionEvent::Invalid) {
        return std::nullopt;
      }

      // Barrier to ensure that the read of erdp_virt_ is ordered after the above validity check.
      hw_rmb();
      return Control::FromTRB(erdp_virt_);
    }
  }

  auto control = Control::FromTRB(erdp_virt_);
  if (control.Cycle() != ccs_) {
    return std::nullopt;
  }

  // Barrier to ensure that all subsequent reads from erdp_virt_ are after the above cycle bit
  // check.
  hw_rmb();

  return control;
}

zx_paddr_t EventRing::UpdateErdpReg(zx_paddr_t last_phys, size_t processed_trb_count) {
  if (last_phys != erdp_phys_) {
    if (async_id_.has_value()) {
      TRACE_ASYNC_END("UsbXhci", "EventRing::UpdateErdpReg", async_id_.value(),
                      "processed_trb_count", processed_trb_count);
    }
    executor_.run_until_idle();
    {
      fbl::AutoLock l(&segment_mutex_);
      erdp_reg_ =
          erdp_reg_.set_Pointer(erdp_phys_).set_DESI(segment_index_).set_EHB(1).WriteTo(mmio_);
    }
    last_phys = erdp_phys_;
    async_id_.emplace(TRACE_NONCE());
    TRACE_ASYNC_BEGIN("UsbXhci", "EventRing::UpdateErdpReg", async_id_.value(), erdp_phys_);
  }
  return last_phys;
}

zx_status_t EventRing::HandleIRQ() {
  iman_reg_.set_IP(1).set_IE(1).WriteTo(mmio_);
  bool avoid_yield = false;
  zx_paddr_t last_phys = 0;
  uint64_t processed_trbs{0};

  auto update_inspect_data = fit::defer([&processed_trbs, this]() {
    total_event_trbs_.Add(processed_trbs);
    if (max_single_irq_event_trbs_value_ < processed_trbs) {
      max_single_irq_event_trbs_value_ = processed_trbs;
      max_single_irq_event_trbs_.Set(max_single_irq_event_trbs_value_);
    }
  });

  // avoid_yield is used to indicate that we are in "realtime mode". When in this mode, we should
  // avoid yielding our timeslice to the scheduler if at all possible, because yielding could result
  // in us getting behind on our deadlines. Currently; we only ever need this on systems that don't
  // support cache coherency where we may have to go through the loop several times due to stale
  // values in the cache (after invalidating of course). On systems with a coherent cache this isn't
  // necessary. Additionally; if we had a guarantee from the scheduler that we would be woken up in
  // <125 microseconds (length of USB frame), we could safely yield after flushing our caches and
  // wouldn't need this loop.
  do {
    avoid_yield = false;

    if (reevaluate_) {
      AdvanceErdp();
    }

    std::optional<Control> control = CurrentErdp();
    while (control.has_value()) {
      ++processed_trbs;
      switch (control->Type()) {
        case Control::PortStatusChangeEvent:
          if (!port_status_change_event_) {
            port_status_change_event_ = events_.CreateUint("PortStatusChangeEvent", 0);
          }
          port_status_change_event_->Add(1);
          HandlePortStatusChangeInterrupt();
          break;
        case Control::CommandCompletionEvent: {
          zx_status_t status = HandleCommandCompletionInterrupt();
          if (status != ZX_OK) {
            return status;
          }
        } break;
        case Control::TransferEvent:
          HandleTransferInterrupt();
          break;
        case Control::HostControllerEvent:
          if (!host_controller_event_) {
            host_controller_event_ = events_.CreateUint("HostControllerEvent", 0);
          }
          host_controller_event_->Add(1);
          // NOTE: We can't really do anything here. This typically indicates some kind of error
          // condition.
          fdf::debug("Host controller event: {}",
                     static_cast<CommandCompletionEvent*>(erdp_virt_)->CompletionCode());
          break;
        default:
          if (!unhandled_events_) {
            unhandled_events_ = events_.CreateLinearUintHistogram("UnhandledEvents", 1, 1, 40);
          }
          unhandled_events_->Insert(control->Type());
          fdf::error("Unexpected transfer event: {}", control->Type());
          break;
      }

      AdvanceErdp();
      control = CurrentErdp();

      if (processed_trbs % kUpdateERDPAfterTRBs) {
        last_phys = UpdateErdpReg(last_phys, processed_trbs / kUpdateERDPAfterTRBs + 1);
      }
    }

    last_phys = UpdateErdpReg(last_phys, processed_trbs / kUpdateERDPAfterTRBs + 1);
    if (!hci_->HasCoherentState()) {
      // Check for stale value in cache
      InvalidatePageCache(erdp_virt_, ZX_CACHE_FLUSH_INVALIDATE | ZX_CACHE_FLUSH_DATA);
      if (CurrentErdp().has_value()) {
        avoid_yield = true;
      }
    }
  } while (avoid_yield);
  return ZX_OK;
}

void EventRing::HandlePortStatusChangeInterrupt() {
  // Section 4.3 -- USB device intialization
  // Section 6.4.2.3 (Port Status change TRB)
  auto change_event = static_cast<PortStatusChangeEvent*>(erdp_virt_);
  uint8_t port_id = static_cast<uint8_t>(change_event->PortID());
  auto event = std::move(hci_->GetPortState()[port_id - 1].wait_for_port_status_change_);
  // Resume interrupted wait
  if (event) {
    event->completer->complete_ok(nullptr);
  } else {
    SchedulePortStatusChange(port_id);
  }
}

zx_status_t EventRing::HandleCommandCompletionInterrupt() {
  auto completion_event = static_cast<CommandCompletionEvent*>(erdp_virt_);
  if (!command_completion_event_) {
    command_completion_event_ =
        events_.CreateLinearUintHistogram("CommandCompletionEvent", 1, 1, 40);
  }
  command_completion_event_->Insert(completion_event->CompletionCode());
  // We do not expect to receive command completion events with invalid TRB pointers, so we always
  // check the pointer against the command ring pending TRB queue. Section 6.4.2.2 states that not
  // all command completion events have valid TRB pointers, but it's not clear in what case that
  // would actually occur.
  TRB* trb = command_ring_->PhysToVirt(erdp_virt_->ptr);
  std::unique_ptr<TRBContext> context;
  zx_status_t status = command_ring_->CompleteTRB(trb, &context);
  if (status != ZX_OK) {
    fdf::error("command_ring_->CompleteTRB(): {}", zx_status_get_string(status));
    hci_->Shutdown(ZX_ERR_BAD_STATE);
    return ZX_ERR_BAD_STATE;
  }

  if (completion_event->CompletionCode() != CommandCompletionEvent::Success) {
    // Log all failing commands for now. As error handling is added to the command callbacks, this
    // can be reduced.
    fdf::warn("Received command completion event with completion code: {}",
              completion_event->CompletionCode());
  }

  if (context->completer.has_value()) {
    context->completer.value().complete_ok(completion_event);
  }
  return ZX_OK;
}

void EventRing::HandleTransferInterrupt() {
  if (!transfer_event_) {
    transfer_event_ = events_.CreateLinearUintHistogram("TransferEvent", 1, 1, 40);
  }
  auto transfer_event = static_cast<TransferEvent*>(erdp_virt_);
  transfer_event_->Insert(transfer_event->CompletionCode());

  auto device_state = hci_->GetDeviceState()[transfer_event->SlotID() - 1];
  if (!device_state) {
    fdf::warn("Device state invalid");
    return;
  }
  fbl::AutoLock l(&device_state->transaction_lock());
  if (!device_state->IsValid()) {
    fdf::warn("Device state invalid");
    return;
  }

  Endpoint* ep;
  uint8_t endpoint_id = static_cast<uint8_t>(transfer_event->EndpointID() - 1);
  if (unlikely(endpoint_id == 0)) {
    ep = &device_state->GetEndpoint();
  } else {
    ep = &device_state->GetEndpoint(endpoint_id - 1);
  }

  if (transfer_event->CompletionCode() == CommandCompletionEvent::RingOverrun) {
    fdf::debug("Transfer ring overrrun on slot {} endpoint {}", transfer_event->SlotID(),
               transfer_event->EndpointID());
    return;
  }
  if (transfer_event->CompletionCode() == CommandCompletionEvent::RingUnderrun) {
    fdf::debug("Transfer ring underrun on slot {} endpoint {}", transfer_event->SlotID(),
               transfer_event->EndpointID());
    return;
  }

  if (transfer_event->CompletionCode() == CommandCompletionEvent::EndpointNotEnabledError) {
    fdf::warn("Endpoint not enabled error for slot {} endpoint {}", transfer_event->SlotID(),
              transfer_event->EndpointID());
    return;
  }

  auto& ring = ep->transfer_ring();
  if (transfer_event->CompletionCode() == CommandCompletionEvent::MissedServiceError) {
    fdf::debug("Missed service error on slot {} endpoint {}", transfer_event->SlotID(),
               transfer_event->EndpointID());
    std::unique_ptr<TRBContext> context;
    ring.CompleteTRB(nullptr, &context);
    ep->RequestComplete(ZX_ERR_IO_MISSED_DEADLINE, 0, std::move(*context->request));

    // set resynchronize_ to wait for next successful transfer.
    resynchronize_ = true;
    return;
  }

  if (transfer_event->CompletionCode() == CommandCompletionEvent::StallError) {
    fdf::debug("Transfer ring stall on slot {} endpoint {}", transfer_event->SlotID(),
               transfer_event->EndpointID());
    ring.set_stall(true);
    auto completions = ring.TakePendingTRBs();

    // A stall error may not have an associated transfer TRB. Section 4.17.4.
    std::unique_ptr<TRBContext> context;
    if (erdp_virt_->ptr) {
      TRB* trb = ring.PhysToVirt(erdp_virt_->ptr);
      ring.CompleteTRB(trb, &context);
    }
    l.release();

    if (context) {
      if (completions.is_empty()) {
        bool handled = StallWorkaroundForDefectiveHubs(context);
        if (handled) {
          fdf::debug("Handled stall with workaround for defective hubs");
          return;
        }
      }
      ep->RequestComplete(ZX_ERR_IO_REFUSED, 0, std::move(*context->request));
    }
    while (true) {
      auto completion = completions.pop_front();
      if (!completion) {
        break;
      }
      ep->RequestComplete(ZX_ERR_IO_REFUSED, 0, std::move(*completion->request));
    }
    return;
  }

  // If there is no transfer TRB pointer here then it probably indicates an event type that we don't
  // handle. Section 4.17.4.
  if (unlikely(!erdp_virt_->ptr)) {
    fdf::error("Unhandled event (completion code {}) with no transfer TRB pointer.",
               transfer_event->CompletionCode());
    return;
  }

  TRB* trb = ring.PhysToVirt(erdp_virt_->ptr);

  if (transfer_event->CompletionCode() == CommandCompletionEvent::ShortPacket) {
    TRB* last_trb = trb;
    zx_status_t status = ring.HandleShortPacket(trb, transfer_event->TransferLength(), &last_trb);

    // If the TRB is not at the head of the pending list then we're out of sync with the device,
    // which indicates a bug in the driver.
    ZX_ASSERT(status == ZX_OK);

    if (trb != last_trb) {
      // We'll get a second event for this TRB, we have recorded TransferLength in the appropriate
      // TRBContext.
      return;
    }
  }

  fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> resynchronize_completions;
  auto cleanup = fit::defer([&resynchronize_completions, &ep]() {
    while (true) {
      auto completion = resynchronize_completions.pop_front();
      if (!completion) {
        break;
      }
      ep->RequestComplete(ZX_ERR_IO_MISSED_DEADLINE, 0, std::move(*completion->request));
    }
  });
  if (resynchronize_) {
    resynchronize_ = false;
    // Resynchronize (4.11.2.5.2). Just find the next successful TRB.
    resynchronize_completions = ring.TakePendingTRBsUntil(trb);
  }

  // If the TRB is not at the head of the pending list then we're out of sync with the device, which
  // indicates a bug in the driver.
  std::unique_ptr<TRBContext> context;
  zx_status_t status = ring.CompleteTRB(trb, &context);

  // TODO(https://fxbug.dev/42059338): Once we reliably keep track of TRBs, this error handling
  // should be removed and replaced by: ZX_ASSERT(status == ZX_OK).
  if (status != ZX_OK) {
    fdf::error("Lost a TRB! Completion code is {}", transfer_event->CompletionCode());

    auto completions = ring.TakePendingTRBs();
    l.release();

    // CompleteTRB already popped the head off the list, so complete it first.
    if (context) {
      ep->RequestComplete(ZX_ERR_IO, 0, std::move(*context->request));
    }

    size_t index = 1;
    bool found = false;
    while (true) {
      auto completion = completions.pop_front();
      if (!completion) {
        break;
      }

      if (completion->trb == trb) {
        fdf::error("Current TRB was found at index {}", index);
        found = true;
      }
      ep->RequestComplete(ZX_ERR_IO, 0, std::move(*completion->request));
      index++;
    }
    if (!found) {
      fdf::error("Current TRB was not found");
    }

    return;
  }

  l.release();

  if ((transfer_event->CompletionCode() != CommandCompletionEvent::Success) &&
      (transfer_event->CompletionCode() != CommandCompletionEvent::ShortPacket)) {
    // asix-88179 will stall the endpoint if we're sending data too fast. The driver expects us to
    // give it a ZX_ERR_IO_INVALID response when this happens.
    fdf::warn("transfer_event->CompletionCode() == {}", transfer_event->CompletionCode());
    ep->RequestComplete(ZX_ERR_IO_INVALID, 0, std::move(*context->request));
    return;
  }

  size_t length;
  if (context->short_transfer_len.has_value()) {
    length = context->short_transfer_len.value();
  } else {
    length = std::holds_alternative<Request>(*context->request)
                 ? std::get<Request>(*context->request).request()->header.length
                 : std::get<usb::FidlRequest>(*context->request).length();
  }

  ep->RequestComplete(ZX_OK, length, std::move(*context->request));
}

fpromise::promise<void, zx_status_t> EventRing::LinkUp(uint8_t port_id) {
  // Port is in U0 state (link up)
  // Enumerate device
  fdf::debug("Event Link Up {}.", port_id);
  return EnumerateDevice(hci_, port_id, std::nullopt);
}

void EventRing::Usb2DeviceAttach(uint16_t port_id) {
  hci_->GetPortState()[port_id - 1].is_connected = true;
  hci_->GetPortState()[port_id - 1].is_USB3 = false;
  auto sc = PORTSC::Get(cap_length_, port_id).ReadFrom(mmio_);
  PORTSC::Get(cap_length_, port_id)
      .FromValue(0)
      .set_CCS(sc.CCS())
      .set_PortSpeed(sc.PortSpeed())
      .set_PIC(sc.PIC())
      .set_PLS(sc.PLS())
      .set_PP(sc.PP())
      .set_PR(1)
      .WriteTo(mmio_);
}

void EventRing::Usb3DeviceAttach(uint16_t port_id) {
  hci_->GetPortState()[port_id - 1].is_connected = true;
  hci_->GetPortState()[port_id - 1].is_USB3 = true;
}

}  // namespace usb_xhci
