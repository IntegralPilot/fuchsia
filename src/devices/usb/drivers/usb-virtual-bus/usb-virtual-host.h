// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_HOST_H_
#define SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_HOST_H_

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.usb.hci/cpp/fidl.h>
#include <fuchsia/hardware/usb/hci/cpp/banjo.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/platform/cpp/bind.h>
#include <fbl/macros.h>
#include <usb/descriptors.h>
#include <usb/request-fidl.h>

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-endpoint.h"

namespace usb_virtual_bus {

class UsbVirtualBus;
class UsbVirtualHost;

// This class implements the virtual USB host controller protocol.
class UsbVirtualHost
    : public ddk::UsbHciProtocol<UsbVirtualHost>,
      public fidl::Server<fuchsia_hardware_usb_hci::UsbHci>,
      public fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController> {
 public:
  using Service = fuchsia_hardware_usb_hci::UsbHciService;
  static constexpr std::string kName = "usb-virtual-host";
  static std::vector<fuchsia_driver_framework::NodeProperty2> GetProperties() {
    return {fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                               bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_VIRTUAL_BUS),
            fdf::MakeProperty2(bind_fuchsia::PROTOCOL, static_cast<uint32_t>(ZX_PROTOCOL_USB_HCI))};
  }

  explicit UsbVirtualHost(UsbVirtualBus* bus) : bus_(bus) {}

  // USB host controller protocol implementation.
  void UsbHciRequestQueue(usb_request_t* usb_request,
                          const usb_request_complete_callback_t* complete_cb);
  void UsbHciSetBusInterface(const usb_bus_interface_protocol_t* bus_intf);
  size_t UsbHciGetMaxDeviceCount();
  zx_status_t UsbHciEnableEndpoint(uint32_t device_id, const usb_endpoint_descriptor_t* ep_desc,
                                   const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable);
  uint64_t UsbHciGetCurrentFrame();
  zx_status_t UsbHciConfigureHub(uint32_t device_id, usb_speed_t speed,
                                 const usb_hub_descriptor_t* desc, bool multi_tt);
  zx_status_t UsbHciHubDeviceAdded(uint32_t device_id, uint32_t port, usb_speed_t speed);
  zx_status_t UsbHciHubDeviceRemoved(uint32_t device_id, uint32_t port);
  zx_status_t UsbHciHubDeviceReset(uint32_t device_id, uint32_t port);
  zx_status_t UsbHciResetEndpoint(uint32_t device_id, uint8_t ep_address);
  zx_status_t UsbHciResetDevice(uint32_t hub_address, uint32_t device_id);
  size_t UsbHciGetMaxTransferSize(uint32_t device_id, uint8_t ep_address);
  zx_status_t UsbHciCancelAll(uint32_t device_id, uint8_t ep_address);
  size_t UsbHciGetRequestSize();

  // fuchsia_hardware_usb_hci.UsbHci protocol implementation.
  void ConnectToEndpoint(ConnectToEndpointRequest& request,
                         ConnectToEndpointCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
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

  fuchsia_hardware_usb_hci::UsbHciService::InstanceHandler GetInstanceHandler();
  compat::DeviceServer::BanjoConfig GetBanjoConfig() {
    compat::DeviceServer::BanjoConfig banjo_config;
    banjo_config.callbacks[ZX_PROTOCOL_USB_HCI] = banjo_server_.callback();
    return banjo_config;
  }
  compat::SyncInitializedDeviceServer& compat_server() { return compat_server_; }
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController>& controller() {
    return controller_;
  }

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(UsbVirtualHost);

  // fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController>
  void on_fidl_error(fidl::UnbindInfo error) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {}

  UsbVirtualBus* bus_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  compat::SyncInitializedDeviceServer compat_server_;
  compat::BanjoServer banjo_server_{ZX_PROTOCOL_USB_HCI, this, &usb_hci_protocol_ops_};
  fidl::ServerBindingGroup<fuchsia_hardware_usb_hci::UsbHci> bindings_;
};

}  // namespace usb_virtual_bus

#endif  // SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_HOST_H_
