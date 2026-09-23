// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_FUNCTION_PERIPHERAL_FIDL_H_
#define SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_FUNCTION_PERIPHERAL_FIDL_H_

#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>

#include <usb-endpoint/usb-endpoint-client.h>

#include "src/devices/usb/drivers/usb-virtual-bus/tests/virtual-bus-tester-function/peripheral.h"

namespace virtualbus {

class FidlTestFunction : public TestFunction,
                         public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  FidlTestFunction() = default;

  zx::result<> Start(fdf::DriverContext context) override;

  // UsbFunctionInterface
  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override;
  void SetConfigured(SetConfiguredRequest& request,
                     SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  zx::result<> SetFunctionInterface(bool connect) override;
  void QueueOut() override;
  void QueueIn(std::vector<uint8_t> data) override;

  void OutComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void InComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);

  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      binding_;

  fidl::ClientEnd<fuchsia_hardware_usb_endpoint::Endpoint> ep_out_client_;
  fidl::ClientEnd<fuchsia_hardware_usb_endpoint::Endpoint> ep_in_client_;

  usb::EndpointClient<FidlTestFunction> bulk_out_ep_{usb::EndpointType::BULK, this,
                                                     std::mem_fn(&FidlTestFunction::OutComplete)};
  usb::EndpointClient<FidlTestFunction> bulk_in_ep_{usb::EndpointType::BULK, this,
                                                    std::mem_fn(&FidlTestFunction::InComplete)};
};

}  // namespace virtualbus

#endif  // SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_FUNCTION_PERIPHERAL_FIDL_H_
