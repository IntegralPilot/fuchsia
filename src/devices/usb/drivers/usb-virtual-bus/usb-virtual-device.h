// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_DEVICE_H_
#define SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_DEVICE_H_

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.dci/cpp/fidl.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/platform/cpp/bind.h>
#include <fbl/macros.h>

namespace usb_virtual_bus {

class UsbVirtualBus;

// This class implements the virtual USB device controller protocol.
class UsbVirtualDevice
    : public fidl::Server<fuchsia_hardware_usb_dci::UsbDci>,
      public fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController> {
 public:
  using Service = fuchsia_hardware_usb_dci::UsbDciService;
  static constexpr std::string kName = "usb-virtual-device";
  static std::vector<fuchsia_driver_framework::NodeProperty2> GetProperties() {
    return {fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                               bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_VIRTUAL_DEVICE)};
  }

  explicit UsbVirtualDevice(UsbVirtualBus* bus) : bus_(bus) {}

  fuchsia_hardware_usb_dci::UsbDciService::InstanceHandler GetInstanceHandler();

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController>& controller() {
    return controller_;
  }

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(UsbVirtualDevice);

  // fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController>
  void on_fidl_error(fidl::UnbindInfo error) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {}

  // fuchsia_hardware_usb.UsbDci protocol implementation.
  void ConnectToEndpoint(ConnectToEndpointRequest& request,
                         ConnectToEndpointCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void StartController(StartControllerCompleter::Sync& completer) override;
  void StopController(StopControllerCompleter::Sync& completer) override;
  void ConfigureEndpoint(ConfigureEndpointRequest& request,
                         ConfigureEndpointCompleter::Sync& completer) override;
  void DisableEndpoint(DisableEndpointRequest& request,
                       DisableEndpointCompleter::Sync& completer) override;
  void EndpointSetStall(EndpointSetStallRequest& request,
                        EndpointSetStallCompleter::Sync& completer) override;
  void EndpointClearStall(EndpointClearStallRequest& request,
                          EndpointClearStallCompleter::Sync& completer) override;
  void CancelAll(CancelAllRequest& request, CancelAllCompleter::Sync& completer) override;
  void GetHardwareInfo(GetHardwareInfoCompleter::Sync& completer) override;
  void AllocEndpoint(AllocEndpointRequest& request,
                     AllocEndpointCompleter::Sync& completer) override;
  void FreeEndpoint(FreeEndpointRequest& request, FreeEndpointCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_usb_dci::UsbDci> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::warn("usb-virtual-device: received unknown UsbDci method: {}", metadata.method_ordinal);
  }

  UsbVirtualBus* bus_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_dci::UsbDci> bindings_;
};

}  // namespace usb_virtual_bus

#endif  // SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_DEVICE_H_
