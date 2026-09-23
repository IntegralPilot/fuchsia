// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>

#include "src/devices/usb/drivers/dwc3/dwc3-metrics.h"
#include "src/devices/usb/drivers/dwc3/dwc3-regs.h"
#include "src/devices/usb/drivers/dwc3/dwc3-types.h"
#include "src/devices/usb/drivers/dwc3/dwc3.h"

namespace dwc3 {

namespace {
const char* LinkStateToString(uint32_t info) {
  switch (info) {
    case DSTS::USBLNKST_U0 | DEVT_LINK_STATE_CHANGE_SS:
      return "U0 (SS Active)";
    case DSTS::USBLNKST_U1 | DEVT_LINK_STATE_CHANGE_SS:
      return "U1 (SS)";
    case DSTS::USBLNKST_U2 | DEVT_LINK_STATE_CHANGE_SS:
      return "U2 (SS)";
    case DSTS::USBLNKST_U3 | DEVT_LINK_STATE_CHANGE_SS:
      return "U3 (SS Suspend)";
    case DSTS::USBLNKST_ESS_DIS | DEVT_LINK_STATE_CHANGE_SS:
      return "SS Disabled";
    case DSTS::USBLNKST_RX_DET | DEVT_LINK_STATE_CHANGE_SS:
      return "Rx.Detect (SS)";
    case DSTS::USBLNKST_ESS_INACT | DEVT_LINK_STATE_CHANGE_SS:
      return "SS Inactive";
    case DSTS::USBLNKST_POLL | DEVT_LINK_STATE_CHANGE_SS:
      return "Polling (SS)";
    case DSTS::USBLNKST_RECOV | DEVT_LINK_STATE_CHANGE_SS:
      return "Recovery (SS)";
    case DSTS::USBLNKST_HRESET | DEVT_LINK_STATE_CHANGE_SS:
      return "Hot Reset (SS)";
    case DSTS::USBLNKST_CMPLY | DEVT_LINK_STATE_CHANGE_SS:
      return "Compliance Mode (SS)";
    case DSTS::USBLNKST_LPBK | DEVT_LINK_STATE_CHANGE_SS:
      return "Loopback (SS)";
    case DSTS::USBLNKST_RESUME_RESET | DEVT_LINK_STATE_CHANGE_SS:
      return "Resume/Reset (SS)";
    case DSTS::USBLNKST_ON:
      return "ON (USB 2.0)";
    case DSTS::USBLNKST_SLEEP:
      return "Sleep (USB 2.0)";
    case DSTS::USBLNKST_SUSPEND:
      return "Suspend (USB 2.0)";
    case DSTS::USBLNKST_DISCONNECTED:
      return "Disconnected (USB 2.0)";
    case DSTS::USBLNKST_EARLY_SUSPEND:
      return "Early Suspend (USB 2.0)";
    case DSTS::USBLNKST_RESET:
      return "Reset (USB 2.0)";
    case DSTS::USBLNKST_RESUME:
      return "Resume (USB 2.0)";
    default:
      return "unknown";
  }
}
}  // namespace

void Dwc3::HandleEpEvent(uint32_t event) {
  TRACE_DURATION("dwc3", "HandleEpEvent", "event", event);
  const uint32_t type = DEPEVT_TYPE(event);
  const uint8_t ep_num = DEPEVT_PHYS_EP(event);
  const uint32_t status = DEPEVT_STATUS(event);

  switch (type) {
    case DEPEVT_XFER_COMPLETE:
      fdf::debug("ep[{}] DEPEVT_XFER_COMPLETE", ep_num);
      HandleEpTransferCompleteEvent(ep_num);
      break;
    case DEPEVT_XFER_IN_PROGRESS: {
      const uint16_t param = DEPEVT_PARAMS(event);
      fdf::debug("ep[{}] DEPEVT_XFER_IN_PROGRESS: status {} param {:X}", ep_num, status, param);
      HandleEpTransferInProgressEvent(ep_num);
    } break;
    case DEPEVT_XFER_NOT_READY:
      fdf::debug("ep[{}] DEPEVT_XFER_NOT_READY reason {:s}", ep_num,
                 (event & DEPEVT_XFER_NOT_READY_REASON) ? "XferActive" : "XferNotActive");
      if (ep_num == 0 && (event & DEPEVT_XFER_NOT_READY_REASON)) {
        // The host has abandoned the transfer, stall and reset.
        ep0_.shared_fifo.Clear();
        EpSetStall(ep0_.out, true);
        Ep0QueueSetup();
        break;
      }
      HandleEpTransferNotReadyEvent(ep_num, DEPEVT_XFER_NOT_READY_STAGE(event));
      break;
    case DEPEVT_STREAM_EVT:
      fdf::debug("ep[{}] DEPEVT_STREAM_EVT ep_num: status %u", ep_num, status);
      break;
    case DEPEVT_CMD_CMPLT: {
      uint32_t cmd_type = DEPEVT_CMD_CMPLT_CMD_TYPE(event);
      uint32_t rsrc_id = DEPEVT_CMD_CMPLT_RSRC_ID(event);
      fdf::debug("ep[{}] DEPEVT_CMD_COMPLETE: type {} rsrc_id {} status {}", ep_num, cmd_type,
                 rsrc_id, status);
      if (status != 0) {
        if (is_ep0_num(ep_num)) {
          ((ep_num == kEp0Out) ? ep0_.out : ep0_.in).command_failures++;
        } else {
          UserEndpoint* const uep = get_user_endpoint(ep_num);
          if (uep) {
            uep->ep.command_failures++;
          }
        }
        metrics_.RecordEvent(
            std::format("ep[{}] command {} failed with status {}", ep_num, cmd_type, status));
      }
      switch (cmd_type) {
        case DEPCMD::DEPSTRTXFER:
          HandleEpTransferStartedEvent(ep_num, rsrc_id);
          break;
        case DEPCMD::DEPENDXFER:
          HandleEpTransferEndedEvent(ep_num);
          break;
        default:
          break;
      }
      break;
    }
    default:
      fdf::error("dwc3_handle_ep_event: unknown event type {}", type);
      break;
  }
}

void Dwc3::HandleEvent(uint32_t event) {
  TRACE_DURATION("dwc3", "HandleEvent", "event", event);
  if (!(event & DEPEVT_NON_EP)) {
    HandleEpEvent(event);
    return;
  }

  uint32_t type = DEVT_TYPE(event);
  uint32_t info = DEVT_INFO(event);

  metrics_.IncrementEventCount(type);

  switch (type) {
    case DEVT_DISCONNECT:
      fdf::debug("DEVT_DISCONNECT");
      metrics_.RecordEvent("USB Physical Disconnection");
      HandleDisconnectedEvent();
      break;
    case DEVT_USB_RESET:
      fdf::debug("DEVT_USB_RESET");
      metrics_.RecordEvent("USB Reset received from Host");
      HandleResetEvent();
      break;
    case DEVT_CONNECTION_DONE:
      fdf::debug("DEVT_CONNECTION_DONE");
      HandleConnectionDoneEvent();
      break;
    case DEVT_LINK_STATE_CHANGE: {
      const char* state_str = LinkStateToString(info);
      fdf::debug("DEVT_LINK_STATE_CHANGE: {}", state_str);
      metrics_.RecordEvent(std::format("Link State Change: {}", state_str));
      break;
    }
    case DEVT_REMOTE_WAKEUP:
      fdf::debug("DEVT_REMOTE_WAKEUP");
      break;
    case DEVT_HIBERNATE_REQUEST:
      fdf::debug("DEVT_HIBERNATE_REQUEST");
      break;
    case DEVT_SUSPEND_ENTRY:
      fdf::debug("DEVT_SUSPEND_ENTRY");
      break;
    case DEVT_SOF:
      fdf::debug("DEVT_SOF");
      break;
    case DEVT_ERRATIC_ERROR:
      fdf::debug("DEVT_ERRATIC_ERROR");
      break;
    case DEVT_COMMAND_COMPLETE:
      fdf::debug("DEVT_COMMAND_COMPLETE");
      break;
    case DEVT_EVENT_BUF_OVERFLOW:
      fdf::debug("DEVT_EVENT_BUF_OVERFLOW");
      break;
    case DEVT_VENDOR_TEST_LMP:
      fdf::debug("DEVT_VENDOR_TEST_LMP");
      break;
    case DEVT_STOPPED_DISCONNECT:
      fdf::debug("DEVT_STOPPED_DISCONNECT");
      break;
    case DEVT_L1_RESUME_DETECT:
      fdf::debug("DEVT_L1_RESUME_DETECT");
      break;
    case DEVT_LDM_RESPONSE:
      fdf::debug("DEVT_LDM_RESPONSE");
      break;
    default:
      fdf::error("dwc3_handle_event: unknown event type {}", type);
      break;
  }
}

void Dwc3::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) {
  TRACE_DURATION("dwc3", "Dwc3::HandleIrq", "status", status);
  irq_.ack();

  if (!controller_started_ || !power_on_) {
    // Ack but otherwise ignore interrupts that arrive while client has stopped us or the core is
    // powered down. A limited number of interrupts may be triggered while things are settling, and
    // we need to ack them to avoid blocking system suspend.
    return;
  }

  auto* mmio = get_mmio();

  uint32_t total_processed = 0;
  uint32_t event_bytes;
  while ((event_bytes = GEVNTCOUNT::Get(0).ReadFrom(mmio).EVNTCOUNT()) > 0) {
    uint32_t event_count = event_bytes / sizeof(uint32_t);
    total_processed += event_count;
    for (uint32_t event : event_fifo_.Read(event_count)) {
      HandleEvent(event);
    }

    event_fifo_.Advance(event_count);
    // acknowledge the events we have processed
    GEVNTCOUNT::Get(0)
        .FromValue(0)
        .set_EVNT_HANDLER_BUSY(1)
        .set_EVNTCOUNT(event_bytes)
        .WriteTo(mmio);
  }
  metrics_.UpdateMaxEventBatch(total_processed);

  // Flush any pending completions to the endpoints' fidl channels all at once
  // per IRQ.
  for (auto& uep : user_endpoints_) {
    uep.server->SendCompletions();
  }
}

void Dwc3::StartEvents() {
  TRACE_DURATION("dwc3", "Dwc3::StartEvents");
  zx::result result = event_fifo_.Init(bti_, /*cached=*/true);
  if (result.is_error()) {
    fdf::error("Failed to init event fifo {}", result);
    return;
  }

  auto* mmio = get_mmio();

  // set event buffer pointer and size
  // keep interrupts masked until we are ready
  zx_paddr_t paddr = event_fifo_.GetPhys();
  ZX_ASSERT(paddr != 0);

  GEVNTADR::Get(0).FromValue(0).set_EVNTADR(paddr).WriteTo(mmio);
  GEVNTSIZ::Get(0).FromValue(0).set_EVENTSIZ(kBufferSize).set_EVNTINTRPTMASK(0).WriteTo(mmio);
  GEVNTCOUNT::Get(0).FromValue(0).set_EVNTCOUNT(0).set_EVNT_HANDLER_BUSY(1).WriteTo(mmio);

  // enable events
  DEVTEN::Get()
      .FromValue(0)
      .set_L1SUSPEN(1)
      .set_U3L2L1SuspEn(1)
      .set_CONNECTDONEEVTEN(1)
      .set_USBRSTEVTEN(1)
      .set_DISSCONNEVTEN(1)
      .WriteTo(mmio);
}

void Dwc3::StopEvents() {
  if (!power_on_) {
    return;
  }

  auto* mmio = get_mmio();

  // Mask the event interrupt to prevent further interrupts.
  GEVNTSIZ::Get(0).ReadFrom(mmio).set_EVNTINTRPTMASK(1).WriteTo(mmio);

  // Clear GEVNTCOUNT to release any pending level-triggered interrupts.
  uint32_t event_bytes;
  while ((event_bytes = GEVNTCOUNT::Get(0).ReadFrom(mmio).EVNTCOUNT()) > 0) {
    uint32_t event_count = event_bytes / sizeof(uint32_t);
    event_fifo_.Advance(event_count);
    GEVNTCOUNT::Get(0)
        .FromValue(0)
        .set_EVNT_HANDLER_BUSY(1)
        .set_EVNTCOUNT(event_bytes)
        .WriteTo(mmio);
  }
}

}  // namespace dwc3
