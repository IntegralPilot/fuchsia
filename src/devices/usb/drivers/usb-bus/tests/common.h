// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_BUS_TESTS_COMMON_H_
#define SRC_DEVICES_USB_DRIVERS_USB_BUS_TESTS_COMMON_H_

#include <fidl/fuchsia.hardware.usb.hci/cpp/wire.h>
#include <fuchsia/hardware/usb/bus/cpp/banjo.h>
#include <fuchsia/hardware/usb/hci/cpp/banjo.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>

#include <fbl/ref_counted.h>
#include <usb/request-cpp.h>

#include "src/devices/usb/drivers/usb-bus/usb-device.h"

namespace usb_bus {

template <typename T, size_t N>
constexpr T MakeConstant(const char value[N]) {
  T retval = 0;
  for (T i = 0; i < N; i++) {
    retval = static_cast<T>(retval | (static_cast<T>(value[i]) << (i * 8)));
  }
  static_assert(N <= sizeof(T));
  return retval;
}

constexpr uint8_t kVendorId = 81;
constexpr uint8_t kProductId = 35;
constexpr uint8_t kDeviceClass = 2;
constexpr uint8_t kDeviceSubclass = 6;
constexpr uint8_t kDeviceProtocol = 250;
constexpr uint32_t kDeviceId = 42;
constexpr uint32_t kHubId = 32;
constexpr uint32_t kMaxTransferSize = 9001;
// The endpoint number for which UsbHciGetMaxTransferSize returns kMaxTransferSize.
// All other endpoints will return 0.
constexpr uint8_t kTransferSizeEndpoint = 5;
constexpr uint64_t kCurrentFrame = MakeConstant<uint64_t, 7>("fuchsia");
constexpr size_t kRequestSize = 272;
extern const char16_t* kStringDescriptors[][2];

constexpr usb_speed_t kDeviceSpeed = MakeConstant<usb_speed_t, 4>("slow");

class FakeHci : public ddk::UsbHciProtocol<FakeHci>,
                public fidl::Server<fuchsia_hardware_usb_hci::UsbHci> {
 public:
  FakeHci(async_dispatcher_t* dispatcher);
  virtual ~FakeHci() = default;

  virtual uint64_t UsbHciGetCurrentFrame() { return kCurrentFrame; }

  virtual zx_status_t UsbHciConfigureHub(uint32_t device_id, usb_speed_t speed,
                                         const usb_hub_descriptor_t* desc, bool multi_tt) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t UsbHciHubDeviceAdded(uint32_t device_id, uint32_t port, usb_speed_t speed) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t UsbHciHubDeviceRemoved(uint32_t device_id, uint32_t port) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t UsbHciHubDeviceReset(uint32_t device_id, uint32_t port) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t UsbHciResetEndpoint(uint32_t device_id, uint8_t ep_address);
  virtual zx_status_t UsbHciResetDevice(uint32_t hub_address, uint32_t device_id);

  virtual size_t UsbHciGetMaxTransferSize(uint32_t device_id, uint8_t ep_address) {
    return ((device_id == kDeviceId) && (ep_address == kTransferSizeEndpoint)) ? kMaxTransferSize
                                                                               : 0;
  }

  virtual zx_status_t UsbHciCancelAll(uint32_t device_id, uint8_t ep_address);

  virtual void UsbHciSetBusInterface(const usb_bus_interface_protocol_t* bus_intf) {
    bus_intf_ = ddk::UsbBusInterfaceProtocolClient(bus_intf);
  }

  virtual size_t UsbHciGetMaxDeviceCount() { return 10; }

  virtual size_t UsbHciGetRequestSize() {
    return usb::BorrowedRequest<void>::RequestSize(sizeof(usb_request_t));
  }

  virtual void UsbHciRequestQueue(usb_request_t* usb_request_,
                                  const usb_request_complete_callback_t* complete_cb_);

  virtual zx_status_t UsbHciEnableEndpoint(uint32_t device_id,
                                           const usb_endpoint_descriptor_t* ep_desc,
                                           const usb_ss_ep_comp_descriptor_t* ss_com_desc,
                                           bool enable);

  // FIDL UsbHci methods
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void ConnectToEndpoint(ConnectToEndpointRequest& request,
                         ConnectToEndpointCompleter::Sync& completer) override;
  void GetMaxDeviceCount(GetMaxDeviceCountCompleter::Sync& completer) override;
  void EnableEndpoint(EnableEndpointRequest& request,
                      EnableEndpointCompleter::Sync& completer) override;
  void GetCurrentFrame(GetCurrentFrameCompleter::Sync& completer) override;
  void ConfigureHub(ConfigureHubRequest& request, ConfigureHubCompleter::Sync& completer) override;
  void HubDeviceAdded(HubDeviceAddedRequest& request,
                      HubDeviceAddedCompleter::Sync& completer) override;
  void HubDeviceRemoved(HubDeviceRemovedRequest& request,
                        HubDeviceRemovedCompleter::Sync& completer) override;
  void HubDeviceReset(HubDeviceResetRequest& request,
                      HubDeviceResetCompleter::Sync& completer) override;
  void ResetEndpoint(ResetEndpointRequest& request,
                     ResetEndpointCompleter::Sync& completer) override;
  void ResetDevice(ResetDeviceRequest& request, ResetDeviceCompleter::Sync& completer) override;
  void GetMaxTransferSize(GetMaxTransferSizeRequest& request,
                          GetMaxTransferSizeCompleter::Sync& completer) override;

  void SetEmptyState(bool should_return_empty) { should_return_empty_ = should_return_empty; }

  const usb_hci_protocol_t* proto() { return &proto_; }
  uint8_t configuration() { return selected_configuration_; }
  usb::BorrowedRequestQueue<void> pending_requests() { return std::move(pending_requests_); }
  void set_custom_control_handling(bool enabled) { custom_control_ = enabled; }

  void set_enable_endpoint_hook(
      fit::function<zx_status_t(uint32_t device_id, const usb_endpoint_descriptor_t* ep_desc,
                                const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable)>
          hook) {
    enable_endpoint_hook_ = std::move(hook);
  }

  uint8_t reset_endpoint() { return reset_endpoint_; }
  bool device_reset() { return device_reset_; }
  ddk::UsbBusInterfaceProtocolClient& bus_intf() { return bus_intf_; }
  fidl::WireSharedClient<fuchsia_hardware_usb_hci::UsbHciInterface>& hci_interface_client() {
    return hci_interface_client_;
  }

 private:
  async_dispatcher_t* dispatcher_;
  bool should_return_empty_ = false;
  bool device_reset_ = false;
  bool custom_control_ = false;
  uint8_t selected_configuration_ = 0;
  usb_hci_protocol_t proto_;
  uint8_t reset_endpoint_ = 0;
  fit::function<zx_status_t(uint32_t device_id, const usb_endpoint_descriptor_t* ep_desc,
                            const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable)>
      enable_endpoint_hook_;
  usb::BorrowedRequestQueue<void> pending_requests_;
  ddk::UsbBusInterfaceProtocolClient bus_intf_;
  fidl::WireSharedClient<fuchsia_hardware_usb_hci::UsbHciInterface> hci_interface_client_;
};

class FakeTimer : public UsbWaiterInterface {
 public:
  zx_status_t Wait(sync_completion_t* completion, zx_duration_t duration) override {
    return timeout_handler_(completion, duration);
  }

  void set_timeout_handler(fit::function<zx_status_t(sync_completion_t*, zx_duration_t)> handler) {
    timeout_handler_ = std::move(handler);
  }

 private:
  fit::function<zx_status_t(sync_completion_t*, zx_duration_t)> timeout_handler_;
};

}  // namespace usb_bus

#endif  // SRC_DEVICES_USB_DRIVERS_USB_BUS_TESTS_COMMON_H_
