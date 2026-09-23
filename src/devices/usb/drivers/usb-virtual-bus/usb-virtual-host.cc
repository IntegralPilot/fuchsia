// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-host.h"

#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <fbl/auto_lock.h>
#include <usb/request-fidl.h>
#include <usb/usb-request.h>

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-bus.h"

namespace usb_virtual_bus {

void UsbVirtualHost::on_fidl_error(fidl::UnbindInfo error) { bus_->FinishRemove<UsbVirtualHost>(); }
fuchsia_hardware_usb_hci::UsbHciService::InstanceHandler UsbVirtualHost::GetInstanceHandler() {
  return fuchsia_hardware_usb_hci::UsbHciService::InstanceHandler({
      .device =
          bindings_.CreateHandler(this, bus_->async_dispatcher(), fidl::kIgnoreBindingClosure),
  });
}

void UsbVirtualHost::UsbHciRequestQueue(usb_request_t* req,
                                        const usb_request_complete_callback_t* complete_cb) {
  Request request(req, *complete_cb, sizeof(usb_request_t));

  uint8_t index = EpAddressToIndex(request.request()->header.ep_address);
  if (index >= USB_MAX_EPS) {
    fdf::error("usb_virtual_bus_host_queue bad endpoint {}\n",
               request.request()->header.ep_address);
    request.Complete(ZX_ERR_INVALID_ARGS, 0);
    return;
  }

  async::PostTask(bus_->async_dispatcher(), [this, index, request = std::move(request)]() mutable {
    bus_->ep(index).host_.QueueRequest(std::move(request));
  });
}

void UsbVirtualHost::UsbHciSetBusInterface(const usb_bus_interface_protocol_t* bus_intf) {}

void UsbVirtualHost::SetInterface(SetInterfaceRequest& request,
                                  SetInterfaceCompleter::Sync& completer) {
  zx::result result = bus_->SetBusInterface(std::move(request.interface()));
  if (result.is_error()) {
    fdf::error("Failed to set bus interface {}", result);
    completer.Reply(result.take_error());
    return;
  }
  completer.Reply(zx::ok());
}

size_t UsbVirtualHost::UsbHciGetMaxDeviceCount() { return 1; }

void UsbVirtualHost::GetMaxDeviceCount(GetMaxDeviceCountCompleter::Sync& completer) {
  completer.Reply(UsbHciGetMaxDeviceCount());
}

zx_status_t UsbVirtualHost::UsbHciEnableEndpoint(uint32_t device_id,
                                                 const usb_endpoint_descriptor_t* ep_desc,
                                                 const usb_ss_ep_comp_descriptor_t* ss_com_desc,
                                                 bool enable) {
  return ZX_OK;
}

void UsbVirtualHost::EnableEndpoint(EnableEndpointRequest& request,
                                    EnableEndpointCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

uint64_t UsbVirtualHost::UsbHciGetCurrentFrame() { return 0; }

void UsbVirtualHost::GetCurrentFrame(GetCurrentFrameCompleter::Sync& completer) {
  completer.Reply(UsbHciGetCurrentFrame());
}

zx_status_t UsbVirtualHost::UsbHciConfigureHub(uint32_t device_id, usb_speed_t speed,
                                               const usb_hub_descriptor_t* desc, bool multi_tt) {
  return ZX_OK;
}

void UsbVirtualHost::ConfigureHub(ConfigureHubRequest& request,
                                  ConfigureHubCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

zx_status_t UsbVirtualHost::UsbHciHubDeviceAdded(uint32_t device_id, uint32_t port,
                                                 usb_speed_t speed) {
  return ZX_OK;
}

void UsbVirtualHost::HubDeviceAdded(HubDeviceAddedRequest& request,
                                    HubDeviceAddedCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

zx_status_t UsbVirtualHost::UsbHciHubDeviceRemoved(uint32_t device_id, uint32_t port) {
  return ZX_OK;
}

void UsbVirtualHost::HubDeviceRemoved(HubDeviceRemovedRequest& request,
                                      HubDeviceRemovedCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

zx_status_t UsbVirtualHost::UsbHciHubDeviceReset(uint32_t device_id, uint32_t port) {
  return ZX_ERR_NOT_SUPPORTED;
}

void UsbVirtualHost::HubDeviceReset(HubDeviceResetRequest& request,
                                    HubDeviceResetCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

zx_status_t UsbVirtualHost::UsbHciResetEndpoint(uint32_t device_id, uint8_t ep_address) {
  return ZX_ERR_NOT_SUPPORTED;
}

void UsbVirtualHost::ResetEndpoint(ResetEndpointRequest& request,
                                   ResetEndpointCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

zx_status_t UsbVirtualHost::UsbHciResetDevice(uint32_t hub_address, uint32_t device_id) {
  return ZX_ERR_NOT_SUPPORTED;
}

void UsbVirtualHost::ResetDevice(ResetDeviceRequest& request,
                                 ResetDeviceCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

size_t UsbVirtualHost::UsbHciGetMaxTransferSize(uint32_t device_id, uint8_t ep_address) {
  return 65536;
}

void UsbVirtualHost::GetMaxTransferSize(GetMaxTransferSizeRequest& request,
                                        GetMaxTransferSizeCompleter::Sync& completer) {
  completer.Reply(zx::ok(UsbHciGetMaxTransferSize(request.device_id(), request.ep_address())));
}

zx_status_t UsbVirtualHost::UsbHciCancelAll(uint32_t device_id, uint8_t ep_address) {
  uint8_t index = EpAddressToIndex(ep_address);
  if (index >= USB_MAX_EPS) {
    return ZX_ERR_INVALID_ARGS;
  }

  bus_->ep(index).host_.CommonCancelAll();
  return ZX_OK;
}

size_t UsbVirtualHost::UsbHciGetRequestSize() {
  return Request::RequestSize(sizeof(usb_request_t));
}

void UsbVirtualHost::ConnectToEndpoint(ConnectToEndpointRequest& request,
                                       ConnectToEndpointCompleter::Sync& completer) {
  uint8_t index = EpAddressToIndex(request.ep_addr());
  if (index >= USB_MAX_EPS) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  bus_->ep(index).host_.Connect(std::move(request.ep()));
  completer.Reply(zx::ok());
}

}  // namespace usb_virtual_bus
