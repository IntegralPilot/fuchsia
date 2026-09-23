// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-device.h"

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-bus.h"

namespace usb_virtual_bus {

void UsbVirtualDevice::on_fidl_error(fidl::UnbindInfo error) {
  bus_->FinishRemove<UsbVirtualDevice>();
}
fuchsia_hardware_usb_dci::UsbDciService::InstanceHandler UsbVirtualDevice::GetInstanceHandler() {
  return fuchsia_hardware_usb_dci::UsbDciService::InstanceHandler({
      .device =
          bindings_.CreateHandler(this, bus_->async_dispatcher(), fidl::kIgnoreBindingClosure),
  });
}

void UsbVirtualDevice::ConnectToEndpoint(ConnectToEndpointRequest& request,
                                         ConnectToEndpointCompleter::Sync& completer) {
  uint8_t index = EpAddressToIndex(request.ep_addr());
  if (index >= USB_MAX_EPS) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  bus_->ep(index).device_.Connect(std::move(request.ep()));
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::SetInterface(SetInterfaceRequest& request,
                                    SetInterfaceCompleter::Sync& completer) {
  completer.Reply(bus_->SetDciInterface(std::move(request.interface())));
}

void UsbVirtualDevice::StartController(StartControllerCompleter::Sync& completer) {
  // CRITICAL: We MUST acknowledge the StartController request immediately.
  // Waiting for the full connection sequence (kConnected) to finish will cause a deadlock,
  // because the peripheral driver (the caller) often needs to be unblocked and running
  // to process the SetConnected call and the host's subsequent enumeration requests (Endpoint 0).
  bus_->OnStartDci([](zx_status_t status) {
    if (status != ZX_OK) {
      fdf::error("StartController connection sequence failed: {}", zx_status_get_string(status));
    }
  });
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::StopController(StopControllerCompleter::Sync& completer) {
  // Similarly, we acknowledge disconnection immediately to avoid holding up the driver teardown.
  bus_->OnStopDci([](zx_status_t status) {
    if (status != ZX_OK) {
      fdf::error("StopController disconnection sequence failed: {}", zx_status_get_string(status));
    }
  });
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::ConfigureEndpoint(ConfigureEndpointRequest& request,
                                         ConfigureEndpointCompleter::Sync& completer) {
  uint8_t index = EpAddressToIndex(request.ep_descriptor().b_endpoint_address());
  if (index >= USB_MAX_EPS) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  bus_->ep(index).max_packet_size_ = usb_ep_max_packet2(request.ep_descriptor());
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::DisableEndpoint(DisableEndpointRequest& request,
                                       DisableEndpointCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::EndpointSetStall(EndpointSetStallRequest& request,
                                        EndpointSetStallCompleter::Sync& completer) {
  uint8_t index = EpAddressToIndex(request.ep_address());
  if (index >= USB_MAX_EPS) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  zx_status_t status = bus_->ep(index).SetStall(true).status_value();
  if (status != ZX_OK) {
    completer.Reply(zx::error(status));
    return;
  }
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::EndpointClearStall(EndpointClearStallRequest& request,
                                          EndpointClearStallCompleter::Sync& completer) {
  uint8_t index = EpAddressToIndex(request.ep_address());
  if (index >= USB_MAX_EPS) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  zx_status_t status = bus_->ep(index).SetStall(false).status_value();
  if (status != ZX_OK) {
    completer.Reply(zx::error(status));
    return;
  }
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::CancelAll(CancelAllRequest& request, CancelAllCompleter::Sync& completer) {
  uint8_t index = EpAddressToIndex(request.ep_address());
  if (index >= USB_MAX_EPS) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  bus_->ep(index).device_.CommonCancelAll();
  completer.Reply(zx::ok());
}

void UsbVirtualDevice::GetHardwareInfo(GetHardwareInfoCompleter::Sync& completer) {
  constexpr uint16_t kMaxPacketSizeLimit = 65535;
  std::vector<fuchsia_hardware_usb_dci::SupportedEndpointInfo> supported_types(3);
  supported_types[0].endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
  supported_types[0].max_packet_size_limit(kMaxPacketSizeLimit);
  supported_types[1].endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);
  supported_types[1].max_packet_size_limit(kMaxPacketSizeLimit);
  supported_types[2].endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kIsochronous);
  supported_types[2].max_packet_size_limit(kMaxPacketSizeLimit);

  std::vector<fuchsia_hardware_usb_dci::EndpointInfo> endpoints;
  endpoints.reserve(30);

  // OUT endpoints 1 to 15.
  for (uint8_t i = 1; i <= 15; i++) {
    fuchsia_hardware_usb_dci::EndpointInfo ep_info;
    ep_info.ep_address(i);
    ep_info.supported_types(supported_types);
    endpoints.push_back(std::move(ep_info));
  }

  // IN endpoints 0x81 to 0x8F.
  for (uint8_t i = 1; i <= 15; i++) {
    fuchsia_hardware_usb_dci::EndpointInfo ep_info;
    ep_info.ep_address(static_cast<uint8_t>(0x80 | i));
    ep_info.supported_types(supported_types);
    endpoints.push_back(std::move(ep_info));
  }

  fuchsia_hardware_usb_dci::DciHardwareInfo info;
  info.endpoints(std::move(endpoints));
  info.supports_dynamic_ep_sizing(false);

  fuchsia_hardware_usb_dci::UsbDciGetHardwareInfoResponse response;
  response.info(std::move(info));

  completer.Reply(zx::ok(std::move(response)));
}

void UsbVirtualDevice::AllocEndpoint(AllocEndpointRequest& request,
                                     AllocEndpointCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void UsbVirtualDevice::FreeEndpoint(FreeEndpointRequest& request,
                                    FreeEndpointCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

}  // namespace usb_virtual_bus
