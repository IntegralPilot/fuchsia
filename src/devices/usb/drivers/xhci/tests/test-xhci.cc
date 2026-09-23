// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file is the implementation of the common functions of UsbXhci used by tests. All functions
// here return the equivalent of "not supported." Any functions that should be supported will be
// implemented in the test file itself.

#include <lib/driver/component/cpp/driver_export.h>

#include <fake-dma-buffer/fake-dma-buffer.h>

#include "src/devices/usb/drivers/xhci/tests/test-env.h"

namespace usb_xhci {

// Static Default Implementations
zx::result<> UsbXhci::TestInit(void* test_harness) {
  test_harness_ = test_harness;
  return Init(ddk_fake::CreateBufferFactory());
}

zx::result<> UsbXhci::Start(fdf::DriverContext context) { return zx::ok(); }

UsbXhci::~UsbXhci() {}

void UsbXhci::ConnectToEndpoint(ConnectToEndpointRequest& request,
                                ConnectToEndpointCompleter::Sync& completer) {
  completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
}

void UsbXhci::UsbHciSetBusInterface(const usb_bus_interface_protocol_t* bus_intf) {}

size_t UsbXhci::UsbHciGetMaxDeviceCount() { return 0; }

zx_status_t UsbXhci::UsbHciEnableEndpoint(uint32_t device_id,
                                          const usb_endpoint_descriptor_t* ep_desc,
                                          const usb_ss_ep_comp_descriptor_t* ss_com_desc,
                                          bool enable) {
  return ZX_ERR_NOT_SUPPORTED;
}

uint64_t UsbXhci::UsbHciGetCurrentFrame() { return 0; }

zx_status_t UsbXhci::UsbHciConfigureHub(uint32_t device_id, usb_speed_t speed,
                                        const usb_hub_descriptor_t* desc, bool multi_tt) {
  return ZX_ERR_NOT_SUPPORTED;
}
zx_status_t UsbXhci::UsbHciHubDeviceAdded(uint32_t device_id, uint32_t port, usb_speed_t speed) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t UsbXhci::UsbHciHubDeviceRemoved(uint32_t hub_id, uint32_t port) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t UsbXhci::UsbHciHubDeviceReset(uint32_t device_id, uint32_t port) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t UsbXhci::UsbHciResetEndpoint(uint32_t device_id, uint8_t ep_address) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t UsbXhci::UsbHciResetDevice(uint32_t hub_address, uint32_t device_id) {
  return ZX_ERR_NOT_SUPPORTED;
}

size_t UsbXhci::UsbHciGetMaxTransferSize(uint32_t device_id, uint8_t ep_address) { return 0; }

size_t UsbXhci::UsbHciGetRequestSize() { return Request::RequestSize(sizeof(usb_request_t)); }

DeviceState::~DeviceState() = default;

zx_status_t DeviceState::InitEndpoint(uint8_t ep_addr, EventRing* event_ring, fdf::MmioBuffer* mmio)
    __TA_REQUIRES(transaction_lock_) {
  rings_[ep_addr].emplace(hci_, device_id_, ep_addr);
  return rings_[ep_addr]->Init(event_ring, mmio);
}

}  // namespace usb_xhci

FUCHSIA_DRIVER_EXPORT2(usb_xhci::UsbXhci);
