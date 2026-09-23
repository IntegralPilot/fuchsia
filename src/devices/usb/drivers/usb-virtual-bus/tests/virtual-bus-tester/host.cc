// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-virtual-bus/tests/virtual-bus-tester/host.h"

#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/driver/compat/cpp/compat.h>

#include <usb/request-cpp.h>

namespace virtualbus {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

void Device::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  if (request.is_in()) {
    static const size_t kMaxControlDataSize = 100;
    size_t actual;
    std::vector<uint8_t> data(kMaxControlDataSize);
    auto status =
        usb_client_.ControlIn(USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE, 0xFF, 0xA, 0,
                              ZX_TIME_INFINITE, data.data(), data.size(), &actual);
    if (status != ZX_OK) {
      completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
      return;
    }
    data.resize(actual);
    completer.Reply(zx::ok(std::move(data)));
    return;
  }

  auto status = usb_client_.ControlOut(USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE, 0xFF,
                                       0xA, 0, ZX_TIME_INFINITE, request.out_data().data(),
                                       request.out_data().size());
  if (status != ZX_OK) {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }
  completer.Reply(zx::ok(std::vector<uint8_t>{}));
}

void Device::Out(OutRequest& request, OutCompleter::Sync& completer) {
  if (out_completer_.has_value()) {
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  out_completer_ = completer.ToAsync();

  QueueOut(std::move(request.data()));
}

void Device::In(InRequest& request, InCompleter::Sync& completer) {
  if (in_completer_.has_value()) {
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  in_completer_ = completer.ToAsync();

  QueueIn(request.size());
}

zx::result<> Device::Start(fdf::DriverContext context) {
  if (!incoming_) {
    incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  }
  zx::result<ddk::UsbProtocolClient> usb = compat::ConnectBanjo<ddk::UsbProtocolClient>(incoming());
  if (usb.is_error()) {
    fdf::error("Failed to connect function {}", usb);
    return usb.take_error();
  }
  usb_client_ = *usb;

  // Find our endpoints.
  std::optional<usb::InterfaceList> usb_interface_list;
  zx_status_t status = usb::InterfaceList::Create(usb_client_, true, &usb_interface_list);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  for (auto& interface : *usb_interface_list) {
    for (auto ep_itr : interface.GetEndpointList()) {
      if (usb_ep_direction(ep_itr.descriptor()) == USB_ENDPOINT_OUT) {
        if (usb_ep_type(ep_itr.descriptor()) == fdescriptor::EndpointType::kBulk) {
          bulk_out_addr_ = ep_itr.descriptor()->b_endpoint_address;
        }
      }
      if (usb_ep_direction(ep_itr.descriptor()) == USB_ENDPOINT_IN) {
        if (usb_ep_type(ep_itr.descriptor()) == fdescriptor::EndpointType::kBulk) {
          bulk_in_addr_ = ep_itr.descriptor()->b_endpoint_address;
        }
      }
    }
  }
  if (!bulk_out_addr_) {
    fdf::error("could not find bulk out endpoint");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  if (!bulk_in_addr_) {
    fdf::error("could not find bulk in endpoint");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  zx::result child = AddOwnedChild(kName);
  if (child.is_error()) {
    fdf::error("Failed to add child {}", child);
    return child.take_error();
  }
  child_ = std::move(*child);

  auto serve_result = outgoing()->AddService<fuchsia_hardware_usb_virtualbustest::BusTestService>(
      fuchsia_hardware_usb_virtualbustest::BusTestService::InstanceHandler({
          .device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
      }));
  if (serve_result.is_error()) {
    fdf::error("Failed to add Device service {}", serve_result);
    return serve_result.take_error();
  }

  return zx::ok();
}

}  // namespace virtualbus
