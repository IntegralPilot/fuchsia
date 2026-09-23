// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.usb.descriptor/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.policy/cpp/common_types_format.h>
#include <fidl/fuchsia.hardware.usb.policy/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>
#include <zircon/errors.h>

#include <mutex>

#include "src/devices/usb/drivers/dwc3/dwc3.h"

namespace dwc3 {

namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace fpolicy = fuchsia_hardware_usb_policy;

zx_status_t Dwc3::Ep0Init() {
  TRACE_DURATION("dwc3", "Dwc3::Ep0Init");
  // Always use a cached TRB FIFO for EP0.
  if (zx::result result = ep0_.shared_fifo.Init(bti_, /*cached=*/true); result.is_error()) {
    return result.error_value();
  }

  const std::array eps{&ep0_.out, &ep0_.in};
  for (Endpoint* ep : eps) {
    ep->max_packet_size = kEp0MaxPacketSize;
    ep->type = fdescriptor::EndpointType::kControl;
    ep->interval = 0;
  }
  ep0_.out.usb_endpoint_address = 0x00;
  ep0_.in.usb_endpoint_address = 0x80;

  return ZX_OK;
}

void Dwc3::Ep0Start() {
  TRACE_DURATION("dwc3", "Dwc3::Ep0Start");
  CmdStartNewConfig(ep0_.out, 0);
  EpSetConfig(ep0_.out, true);
  EpSetConfig(ep0_.in, true);

  Ep0QueueSetup();
}

void Dwc3::Ep0QueueSetup() {
  TRACE_DURATION("dwc3", "Dwc3::Ep0QueueSetup");
  ep0_.in.transfer_state = Endpoint::TransferState::kIdle;
  ep0_.out.transfer_state = Endpoint::TransferState::kIdle;
  if (auto status = ep0_.buffer->CacheFlushInvalidate(0, sizeof(fdescriptor::wire::UsbSetup));
      status.is_error()) {
    fdf::error("CacheFlushInvalidate failed: {}", status);
    return;
  }
  EpStartTransfer(ep0_.out, ep0_.shared_fifo, TRB_TRBCTL_SETUP, ep0_.buffer->phys(),
                  sizeof(fdescriptor::wire::UsbSetup));
  ep0_.state = Ep0::State::Setup;
}

void Dwc3::Ep0StartEndpoints() {
  TRACE_DURATION("dwc3", "Dwc3::Ep0StartEndpoints");
  fdf::debug("Dwc3::Ep0StartEndpoints");

  ep0_.in.type = fdescriptor::EndpointType::kControl;
  ep0_.in.interval = 0;
  CmdEpSetConfig(ep0_.in, true);

  // The hard-coded value of '2' here is required by specification, see 'Start
  // New Configuration (DEPSTARTCFG)' in the programming guide. We call this
  // function upon receiving a SetConfiguration call, prior to setting up
  // endpoints > 1.
  CmdStartNewConfig(ep0_.out, 2);
}

void Dwc3::HandleEp0TransferCompleteEvent(uint8_t ep_num) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEp0TransferCompleteEvent", "ep_num", ep_num);
  ZX_ASSERT(is_ep0_num(ep_num));

  // Only DataOut and DataIn states need TRB read.
  dwc3_trb_t trb{};
  if (ep0_.state == Ep0::State::DataOut || ep0_.state == Ep0::State::DataIn) {
    if (auto res = ep0_.shared_fifo.ReadOne(); res.is_ok()) {
      trb = *res;
    } else {
      fdf::error("ReadOne failed: {}", res.status_string());
      return;
    }
  }
  // Only advance the shared FIFO read pointer if the FIFO is not empty upon
  // receiving EP0 completion interrupts. When stall recovery (Ep0EndAndStall)
  // clears the TRB ring (read_ == write_), advancing unconditionally triggers
  // underflow logs.
  if (!ep0_.shared_fifo.IsEmpty()) {
    ep0_.shared_fifo.AdvanceRead();
  }

  switch (ep0_.state) {
    case Ep0::State::Setup: {
      // Control Endpoint stall is cleared upon receiving SETUP.
      ep0_.out.stalled = false;

      if (auto setup = ep0_.buffer->ReadStruct<decltype(ep0_.cur_setup)>(); setup.is_ok()) {
        ep0_.cur_setup = *setup;
      } else {
        fdf::error("ReadStruct failed: {}", setup.status_string());
        Ep0EndAndStall(ep0_.out);
        Ep0QueueSetup();
        break;
      }

      fdf::debug("got setup: type: 0x{:02x} req: {} value: {} index: {} length: {}",
                 ep0_.cur_setup.bm_request_type, ep0_.cur_setup.b_request, ep0_.cur_setup.w_value,
                 ep0_.cur_setup.w_index, ep0_.cur_setup.w_length);

      const bool is_two_stage = ep0_.cur_setup.w_length == 0;
      const bool is_out = ((ep0_.cur_setup.bm_request_type & USB_DIR_MASK) == USB_DIR_OUT);

      if (is_two_stage) {
        ep0_.state = Ep0::State::TwoStage;
        HandleEp0Setup(0);
        break;
      }

      // For out-type three-stage transfers, data is first read from the host and then passed up
      // through the stack. For all in-type transfers, the stack generates in-data, and then
      // transfers it to the host.
      if (is_out) {
        ep0_.cur_transfer_len = ep0_.buffer->size();
        EpStartTransfer(ep0_.out, ep0_.shared_fifo, TRB_TRBCTL_CONTROL_DATA, ep0_.buffer->phys(),
                        ep0_.buffer->size());
        ep0_.state = Ep0::State::DataOut;
      } else {
        ep0_.state = Ep0::State::DataIn;
        HandleEp0Setup(ep0_.buffer->size());
      }
      break;
    }
    case Ep0::State::DataOut: {
      if (ep_num != kEp0Out) {
        // This indicates a disagreement between the host and controller about the directionality of
        // the data exchange. In this case, the setup packet indicated a control-write (OUT-type
        // transfer), which would involve a DataOut packet. The controller actually received an
        // unexpected DataIn packet from the host. To recover, gracefully stall and reset the
        // transfer.

        fdf::warn(
            "host/target data direction disagreement, expected data-out, got data-in "
            "(cur_setup: req_type=0x{:02x}, req=0x{:02x}, val=0x{:04x}, idx=0x{:04x}, len={})",
            ep0_.cur_setup.bm_request_type, ep0_.cur_setup.b_request, ep0_.cur_setup.w_value,
            ep0_.cur_setup.w_index, ep0_.cur_setup.w_length);
        Ep0EndAndStall(ep0_.out);
        Ep0QueueSetup();
        break;
      }

      zx_off_t received = ep0_.cur_transfer_len - TRB_BUFSIZ(trb.status);
      ep0_.out.total_transfers++;
      ep0_.out.total_bytes += received;
      ep0_.state = Ep0::State::WaitNrdyIn;
      if (auto status = ep0_.buffer->CacheFlushInvalidate(0, ep0_.buffer->size());
          status.is_error()) {
        fdf::error("CacheFlushInvalidate failed: {}", status);
        Ep0EndAndStall(ep0_.out);
        Ep0QueueSetup();
        break;
      }
      HandleEp0Setup(received);
      break;
    }
    case Ep0::State::DataIn: {
      if (ep_num != kEp0In) {
        // See above, but reverse the directionality for a control-read.
        fdf::warn(
            "host/target data direction disagreement, expected data-in, got data-out "
            "(cur_setup: req_type=0x{:02x}, req=0x{:02x}, val=0x{:04x}, idx=0x{:04x}, len={})",
            ep0_.cur_setup.bm_request_type, ep0_.cur_setup.b_request, ep0_.cur_setup.w_value,
            ep0_.cur_setup.w_index, ep0_.cur_setup.w_length);
        Ep0EndAndStall(ep0_.in);
        Ep0QueueSetup();
        break;
      }
      zx_off_t transferred = ep0_.cur_transfer_len - TRB_BUFSIZ(trb.status);
      ep0_.in.total_transfers++;
      ep0_.in.total_bytes += transferred;

      if (transferred < ep0_.cur_setup.w_length && (transferred % ep0_.in.max_packet_size) == 0 &&
          transferred > 0) {
        ep0_.state = Ep0::State::WaitZlpIn;
        ep0_.cur_transfer_len = 0;
        EpStartTransfer(ep0_.in, ep0_.shared_fifo, TRB_TRBCTL_CONTROL_DATA, 0, 0);
        break;
      }

      ep0_.state = Ep0::State::WaitNrdyOut;
      break;
    }
    case Ep0::State::WaitZlpIn: {
      if (ep_num != kEp0In) {
        fdf::warn(
            "host/target data direction disagreement in WaitZlpIn: expected EP0 IN ({}), got {}, request=0x{:02x}",
            kEp0In, ep_num, ep0_.cur_setup.b_request);
        Ep0EndAndStall(ep0_.in);
        Ep0QueueSetup();
        break;
      }
      ep0_.in.total_transfers++;
      ep0_.state = Ep0::State::WaitNrdyOut;
      break;
    }
    case Ep0::State::Status:
      Ep0QueueSetup();
      break;
    default:
      fdf::error("unexpected XferComplete state={}", ep0_.state);
      break;
  }
}

void Dwc3::HandleEp0TransferNotReadyEvent(uint8_t ep_num, uint32_t stage) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEp0TransferNotReadyEvent", "ep_num", ep_num, "stage", stage);
  fdf::debug("Dwc3::HandleEp0TransferNotReadyEvent state {} stage {}", ep0_.state, stage);

  ZX_ASSERT(is_ep0_num(ep_num));

  switch (ep0_.state) {
    case Ep0::State::Setup:
      if ((stage == DEPEVT_XFER_NOT_READY_STAGE_DATA) ||
          (stage == DEPEVT_XFER_NOT_READY_STAGE_STATUS)) {
        // Stall if we receive XferNotReady(Data/Status) while waiting for setup to complete
        ep0_.shared_fifo.Clear();
        EpSetStall(ep0_.out, true);
        Ep0QueueSetup();
      }
      break;
    case Ep0::State::TwoStage:
      ZX_ASSERT(stage);  // Must be 1 or 2.
      if (stage == DEPEVT_XFER_NOT_READY_STAGE_DATA) {
        ep0_.shared_fifo.Clear();
        EpSetStall(ep0_.out, true);
        Ep0QueueSetup();
      } else {
        ep0_.state = Ep0::State::WaitFidl;
      }
      break;
    case Ep0::State::WaitHost:
      ZX_ASSERT(stage);  // Must be 1 or 2.
      if (stage == DEPEVT_XFER_NOT_READY_STAGE_DATA) {
        ep0_.shared_fifo.Clear();
        EpSetStall(ep0_.out, true);
        Ep0QueueSetup();
      } else {
        EpStartTransfer(ep0_.in, ep0_.shared_fifo, TRB_TRBCTL_STATUS_2, 0, 0);
        ep0_.state = Ep0::State::Status;
      }
      break;
    case Ep0::State::DataOut:
      if ((ep_num == kEp0In) && (stage == DEPEVT_XFER_NOT_READY_STAGE_DATA)) {
        // End transfer and stall if we receive XferNotReady(Data) in the opposite direction.
        Ep0EndAndStall(ep0_.out);
        Ep0QueueSetup();
      }
      break;
    case Ep0::State::DataIn:
      if ((ep_num == kEp0Out) && (stage == DEPEVT_XFER_NOT_READY_STAGE_DATA)) {
        // End transfer and stall if we receive XferNotReady(Data) in the opposite direction.
        Ep0EndAndStall(ep0_.in);
        Ep0QueueSetup();
      }
      break;
    case Ep0::State::WaitNrdyOut:
      if (ep_num == kEp0Out) {
        EpStartTransfer(ep0_.out, ep0_.shared_fifo, TRB_TRBCTL_STATUS_3, 0, 0);
        ep0_.state = Ep0::State::Status;
      }
      break;
    case Ep0::State::WaitNrdyIn:
      if (ep_num == kEp0In) {
        EpStartTransfer(ep0_.in, ep0_.shared_fifo, TRB_TRBCTL_STATUS_3, 0, 0);
        ep0_.state = Ep0::State::Status;
      }
      break;
    case Ep0::State::WaitZlpIn:
      if ((ep_num == kEp0Out) && (stage == DEPEVT_XFER_NOT_READY_STAGE_DATA)) {
        // End transfer and stall if we receive XferNotReady(Data) in the opposite direction.
        Ep0EndAndStall(ep0_.in);
        Ep0QueueSetup();
      }
      break;
    case Ep0::State::Status:
    default:
      fdf::error("ready unhandled state {}", ep0_.state);
      break;
  }
}

void Dwc3::Ep0EndAndStall(Endpoint& ep) {
  ep0_.shared_fifo.Clear();
  if (ep.rsrc_id != Endpoint::kInvalidResourceId) {
    CmdEpEndTransfer(ep);
  }
  EpSetStall(ep, true);
}

void Dwc3::HandleEp0Setup(size_t length) {
  TRACE_DURATION("dwc3", "Dwc3::HandleEp0Setup", "length", length);
  // Copy the setup packet to ensure it is correctly captured in the Then closure.
  fdescriptor::wire::UsbSetup setup = ep0_.cur_setup;

  if (setup.bm_request_type == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE)) {
    // handle some special setup requests in this driver
    switch (setup.b_request) {
      case USB_REQ_SET_ADDRESS:
        SetDeviceAddress(setup.w_value);
        ep0_.state = Ep0::State::WaitHost;
        return;
      case USB_REQ_SET_CONFIGURATION:
        ResetConfiguration();
        Ep0StartEndpoints();
        break;
      default:
        // fall through to the common DoControlCall
        break;
    }
  }

  auto fail = [this]() {
    ep0_.shared_fifo.Clear();
    EpSetStall(ep0_.out, true);
    Ep0QueueSetup();
  };
  if (!dci_intf_.is_valid()) {
    fail();
    return;
  }

  const bool is_out = (setup.bm_request_type & USB_DIR_MASK) == USB_DIR_OUT;

  // We can't fit this in FIDL so we can't dispatch. Log loudly and fail.
  if (is_out && length > fuchsia_hardware_usb_dci::kMaxControlRequestLen) {
    fdf::error(
        "control data request too large ({}) bm_request_type=0x{:02X}, "
        "b_request=0x{:02X}, w_value=0x{:04X}, w_index=0x{:04X}, w_length={}",
        length, setup.bm_request_type, setup.b_request, setup.w_value, setup.w_index,
        setup.w_length);
    fail();
    return;
  }

  fidl::Arena arena;
  fidl::VectorView<uint8_t> out_payload;
  if (is_out && length > 0) {
    out_payload = fidl::VectorView<uint8_t>(arena, length);
    if (auto status = ep0_.buffer->Read(0, length, out_payload.data()); status.is_error()) {
      fdf::error("ep0 buffer Read failed: {}", status.status_string());
      fail();
      return;
    }
  }

  dci_intf_.buffer(arena)
      ->Control(setup, out_payload)
      .Then([this, is_out, fail, length,
             setup](fidl::WireUnownedResult<fuchsia_hardware_usb_dci::UsbDciInterface::Control>&
                        result) {
        if (!power_on_ || !controller_started_) {
          // Return in case the core was powered off or disabled between the setup event and
          // the reply from our child.
          return;
        }

        if (!result.ok()) {
          fdf::error("(framework) Control() length = {}: {}", length, result.FormatDescription());
          metrics_.RecordEvent(
              std::format("ep0: Stalled setup request "
                          "[type=0x{:02x} req=0x{:02x} val=0x{:04x} idx=0x{:04x} len={}] "
                          "(framework error: {})",
                          setup.bm_request_type, setup.b_request, setup.w_value, setup.w_index,
                          setup.w_length, result.status_string()));
          fail();
          return;
        }
        if (result->is_error()) {
          if (result->error_value() != ZX_ERR_NOT_SUPPORTED) {
            fdf::error(
                "Control([type=0x{:02x} req=0x{:02x} val=0x{:04x} idx=0x{:04x} len={}]) request failed: {}",
                setup.bm_request_type, setup.b_request, setup.w_value, setup.w_index,
                setup.w_length, zx_status_get_string(result->error_value()));
          } else {
            fdf::debug("Control request failed: {}", zx_status_get_string(result->error_value()));
          }
          metrics_.RecordEvent(
              std::format("ep0: Stalled setup request "
                          "[type=0x{:02x} req=0x{:02x} val=0x{:04x} idx=0x{:04x} len={}] "
                          "(error: {})",
                          setup.bm_request_type, setup.b_request, setup.w_value, setup.w_index,
                          setup.w_length, zx_status_get_string(result->error_value())));
          fail();
          return;
        }

        switch (ep0_.state) {
          case Ep0::State::TwoStage:
            ep0_.state = Ep0::State::WaitHost;
            break;
          case Ep0::State::WaitFidl:
            EpStartTransfer(ep0_.in, ep0_.shared_fifo, TRB_TRBCTL_STATUS_2, 0, 0);
            ep0_.state = Ep0::State::Status;
            break;
          case Ep0::State::WaitHost:
            // Nonsensical case that should never happen. See state commentary.
            fdf::error("Invalid Ep0 state");
            fail();
            break;
          default:
            if (!is_out) {
              if (ep0_.state == Ep0::State::None) {
                fdf::error(
                    "BUG TRIPPED: Async IN control callback handling None state! (CRASH IMMINENT)");
                // Sleep to allow syslog to flush this to serial before the instant hardware
                // lockup!
                zx::nanosleep(zx::deadline_after(zx::msec(500)));
              }
              // A lightweight byte-span is used to make it easier to process the read data.
              cpp20::span<uint8_t> read_data{result.value()->read.get()};
              // Don't blow out caller's buffer.
              if (read_data.size_bytes() > length) {
                fail();
                return;
              }

              if (!read_data.empty()) {
                if (auto status = ep0_.buffer->Write(read_data.data(), 0, read_data.size_bytes());
                    status.is_error()) {
                  fdf::error("Write failed: {}", status);
                  fail();
                  return;
                }
              }

              fdf::debug("HandleSetup success: actual {}", read_data.size_bytes());
              // queue a write for the data phase
              ep0_.cur_transfer_len = read_data.size_bytes();
              EpStartTransfer(ep0_.in, ep0_.shared_fifo, TRB_TRBCTL_CONTROL_DATA,
                              ep0_.buffer->phys(), read_data.size_bytes());
            }
        }

        if (setup.bm_request_type == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE) &&
            setup.b_request == USB_REQ_SET_CONFIGURATION) {
          SetDeviceState(fpolicy::DeviceState::kConfigured);
        }
      });
}

}  // namespace dwc3
