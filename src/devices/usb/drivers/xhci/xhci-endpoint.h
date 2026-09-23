// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_ENDPOINT_H_
#define SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_ENDPOINT_H_

#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fuchsia/hardware/usb/request/c/banjo.h>
#include <lib/sync/cpp/completion.h>

#include <fbl/ref_ptr.h>

#include "src/devices/usb/drivers/xhci/xhci-context.h"
#include "src/devices/usb/drivers/xhci/xhci-transfer-ring.h"

namespace usb_xhci {

inline uint8_t XhciEndpointIndex(uint8_t ep_address) {
  if (ep_address == 0)
    return 0;
  uint8_t index = static_cast<uint8_t>(2 * (ep_address & ~USB_ENDPOINT_DIR_MASK));
  if ((ep_address & USB_ENDPOINT_DIR_MASK) == USB_ENDPOINT_OUT)
    index--;
  return index;
}

// Inverse of XhciEndpointIndex. In particular takes an XhciEndpointIndex and returns ep_address.
inline uint8_t XhciEndpointIndexInverse(uint8_t idx) {
  return ((idx + 1) / 2) | ((idx % 2) ? USB_ENDPOINT_OUT : USB_ENDPOINT_IN);
}

class UsbXhci;
class DeviceState;

// Endpoints are 1:1 with TransferRing.
class Endpoint : public usb::EndpointServer {
 public:
  Endpoint(UsbXhci* hci, uint32_t device_id, uint8_t address);
  // DeInits the transfer ring.
  ~Endpoint() { transfer_ring_.DeinitIfActive(); }

  // Initializes the transfer ring.
  zx_status_t Init(EventRing* event_ring, fdf::MmioBuffer* mmio);

  // fuchsia_hardware_usb_new.Endpoint protocol implementation. RegisterVmos/UnregisterVmos are
  // defined by usb::EndpointServer.
  void set_ep_type(fuchsia_hardware_usb_descriptor::EndpointType ep_type) { ep_type_ = ep_type; }

  void GetInfo(GetInfoCompleter::Sync& completer) override {
    if (!ep_type_.has_value()) {
      if (ep_addr() == 0) {
        completer.Reply(fit::ok(fuchsia_hardware_usb_endpoint::EndpointInfo::WithControl({})));
        return;
      }
      completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
      return;
    }
    switch (*ep_type_) {
      case fuchsia_hardware_usb_descriptor::EndpointType::kControl:
        completer.Reply(fit::ok(fuchsia_hardware_usb_endpoint::EndpointInfo::WithControl({})));
        break;
      case fuchsia_hardware_usb_descriptor::EndpointType::kBulk:
        completer.Reply(fit::ok(fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk(
            fuchsia_hardware_usb_endpoint::BulkEndpointInfo().supports_scatter_gather(true))));
        break;
      case fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt:
        completer.Reply(fit::ok(fuchsia_hardware_usb_endpoint::EndpointInfo::WithInterrupt(
            fuchsia_hardware_usb_endpoint::InterruptEndpointInfo().supports_scatter_gather(true))));
        break;
      case fuchsia_hardware_usb_descriptor::EndpointType::kIsochronous:
        completer.Reply(fit::ok(fuchsia_hardware_usb_endpoint::EndpointInfo::WithIsochronous(
            fuchsia_hardware_usb_endpoint::IsochronousEndpointInfo()
                .lead_time(0)
                .supports_scatter_gather(true))));
        break;
      default:
        completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
        break;
    }
  }
  void QueueRequests(QueueRequestsRequest& request,
                     QueueRequestsCompleter::Sync& completer) override;
  void CancelAll(CancelAllCompleter::Sync& completer) override;

  TransferRing& transfer_ring() { return transfer_ring_; }

  void QueueRequest(usb::RequestVariant request);

 private:
  // In addition to usb::EndpointServer::OnUnbound, calls CancelAll and DisableEndpoint.
  void OnUnbound(fidl::UnbindInfo info,
                 fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) override;

  struct UsbRequestState {
    // Max burst size (value of the max burst size register + 1, since it is zero-based)
    uint32_t burst_size;

    // Max packet size
    uint32_t max_packet_size;

    // First TRB in the transfer
    // This is owned by the transfer ring.
    TRB* first_trb = nullptr;

    // Value to set the cycle bit on the first TRB to
    bool first_cycle;

    // TransferRing transaction state
    TransferRing::State transaction;

    ContiguousTRBInfo info;

    // Transfer context
    std::unique_ptr<TRBContext> context;

    // The number of packets in the transfer
    size_t packet_count = 0;

    // Total length of the transfer
    uint32_t total_len = 0;

    // The setup TRB
    // This is owned by the transfer ring.
    TRB* setup;

    // The interrupter to use
    uint8_t interrupter = 0;

    // Pointer to the status TRB
    // This is owned by the transfer ring.
    TRB* status_trb_ptr = nullptr;

    // Cycle bit of the setup TRB during the allocation phase
    bool setup_cycle;
    // Last TRB in the transfer
    // This is owned by the transfer ring.
    TRB* last_trb;
  };

  // Helper functions for ControlRequestQueue
  void ControlRequestQueue(usb::RequestVariant request);
  zx_status_t ControlRequestAllocationPhase(UsbRequestState* state);
  zx_status_t ControlRequestStatusPhase(UsbRequestState* state);
  zx_status_t ControlRequestDataPhase(UsbRequestState* state);
  void ControlRequestSetupPhase(UsbRequestState* state);
  void ControlRequestCommit(UsbRequestState* state);

  // Helper functions for NormalRequestQueue
  void NormalRequestQueue(usb::RequestVariant request);
  zx_status_t WaitForIsochronousReady(uint64_t target_frame);
  zx_status_t StartNormalTransaction(UsbRequestState* state, uint8_t interrupter_target);
  zx_status_t ContinueNormalTransaction(UsbRequestState* state);
  void CommitNormalTransaction(UsbRequestState* state);

  UsbXhci* hci_;
  uint32_t device_id_;
  TransferRing transfer_ring_;
  std::optional<fuchsia_hardware_usb_descriptor::EndpointType> ep_type_;
};

}  // namespace usb_xhci

#endif  // SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_ENDPOINT_H_
