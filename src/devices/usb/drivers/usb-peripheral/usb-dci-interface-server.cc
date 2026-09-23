// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-dci-interface-server.h"

#include <fidl/fuchsia.hardware.usb.dci/cpp/wire.h>
#include <lib/trace/event.h>

#include "src/devices/usb/drivers/usb-peripheral/usb-peripheral.h"

// This header appears unused, but is required to appease the forward declaration of UsbFunction
// brought in by way of usb-peripheral.h (which is required).
#include "src/devices/usb/drivers/usb-peripheral/usb-function.h"

namespace usb_peripheral {

// We have a constant in fuchsia.hardware.usb.dci to be able to check maximum
// size of control payloads, leave a change detector here to ensure we have the
// constant at the correct value (targeting the maximum FIDL message size).
static_assert(fidl::MaxSizeInChannel<fuchsia_hardware_usb_dci::wire::UsbDciInterfaceControlRequest,
                                     fidl::MessageDirection::kSending>() ==
              ZX_CHANNEL_MAX_MSG_BYTES);
// Mention the constant because an unbounded message always ends up with the
// maximum channel message length either way.
static_assert(fuchsia_hardware_usb_dci::wire::kMaxControlRequestLen != 0);

void UsbDciInterfaceServer::Control(ControlRequestView req, ControlCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);

  cpp20::span<uint8_t> span_write = req->write.get();

  uint8_t request_type = req->setup.bm_request_type;
  uint8_t request = req->setup.b_request;
  uint16_t value = le16toh(req->setup.w_value);
  uint16_t index = le16toh(req->setup.w_index);
  uint16_t length = le16toh(req->setup.w_length);

  drv_->CommonControl(
      req->setup, span_write,
      [drv = drv_, request_type, request, value, index, length,
       completer = completer.ToAsync()](zx::result<std::vector<uint8_t>> result) mutable {
        zx_status_t status = ZX_OK;
        size_t actual_len = 0;
        if (result.is_error()) {
          status = result.error_value();
          completer.ReplyError(status);
        } else {
          actual_len = result.value().size();
          fidl::Arena arena;
          completer.buffer(arena).ReplySuccess(
              fidl::VectorView<uint8_t>::FromExternal(result.value()));
        }
        drv->dci_inspect().RecordControlTransfer({
            .request_type = request_type,
            .request = request,
            .value = value,
            .index = index,
            .length = length,
            .status = status,
            .actual_length = actual_len,
        });
      });
}

void UsbDciInterfaceServer::SetConnected(SetConnectedRequestView req,
                                         SetConnectedCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  drv_->OnHostConnectionChanged(req->is_connected);
  drv_->dci_inspect().RecordEvent(
      std::format("host connection changed: {}", req->is_connected ? "connected" : "disconnected"));
  completer.ReplySuccess();
}

void UsbDciInterfaceServer::SetSpeed(SetSpeedRequestView req, SetSpeedCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  drv_->speed_ = static_cast<usb_speed_t>(req->speed);
  const char* speed_str = usb_inspect::SpeedToString(drv_->speed_);
  bool connected;
  {
    fbl::AutoLock lock(&drv_->lock_);
    connected = drv_->state_ == UsbPeripheral::DeviceState::kHostConnected;
  }
  drv_->dci_inspect().UpdateConnectionStatus(connected, drv_->speed_);
  drv_->dci_inspect().RecordEvent(std::format("speed set to: {}", speed_str));
  completer.ReplySuccess();
}

void UsbDciInterfaceServer::Stop() {
  TRACE_DURATION("usb-peripheral", __func__);
  dispatcher_.ShutdownAsync();
  // Ensure the dispatcher is completely shut down before proceeding,
  // preventing any concurrent access to driver resources during teardown.
  dispatcher_shutdown_.Wait();
}

}  // namespace usb_peripheral
