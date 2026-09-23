// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_USB_HID_FUNCTION_TWO_ENDPOINT_HID_FUNCTION_H_
#define SRC_UI_INPUT_DRIVERS_USB_HID_FUNCTION_TWO_ENDPOINT_HID_FUNCTION_H_

#include <fidl/fuchsia.hardware.hidbus/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>

#include <memory>
#include <optional>
#include <string>
#include <vector>

#include <fbl/condition_variable.h>
#include <usb-endpoint/usb-endpoint-client.h>
#include <usb/hid.h>
#include <usb/request-cpp.h>
#include <usb/usb.h>

namespace two_endpoint_hid_function {

// This driver is for testing the USB-HID driver. It binds as a peripheral USB
// device and sends fake HID report descriptors and HID reports. The tests for
// this driver and the USB-HID driver are with the other usb-virtual-bus tests.
class FakeUsbHidFunction
    : public fdf::DriverBase2,
      public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  static constexpr std::string kDriverName = "FakeUsbHidFunction";

  explicit FakeUsbHidFunction();

  zx::result<> Start(fdf::DriverContext context) override;

  void UsbEndpointOutCallback(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);

  // fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface>
  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override;
  void SetConfigured(SetConfiguredRequest& request,
                     SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;
  std::vector<fidl::ClientEnd<fuchsia_hardware_usb_endpoint::Endpoint>> endpoints_;

  usb::EndpointClient<FakeUsbHidFunction> out_ep_{
      usb::EndpointType::INTERRUPT, this, std::mem_fn(&FakeUsbHidFunction::UsbEndpointOutCallback)};

  std::vector<uint8_t> report_desc_;
  std::vector<uint8_t> report_;

  struct fake_usb_hid_descriptor_t {
    usb_interface_descriptor_t interface;
    usb_endpoint_descriptor_t interrupt_in;
    usb_endpoint_descriptor_t interrupt_out;
    usb_hid_descriptor_t hid_descriptor;
  } __PACKED;

  struct DescriptorDeleter {
    void operator()(fake_usb_hid_descriptor_t* desc) { free(desc); }
  };
  std::unique_ptr<fake_usb_hid_descriptor_t, DescriptorDeleter> descriptor_;
  size_t descriptor_size_;

  fuchsia_hardware_hidbus::wire::HidProtocol hid_protocol_ =
      fuchsia_hardware_hidbus::wire::HidProtocol::kReport;

  bool active_ = false;
};

}  // namespace two_endpoint_hid_function

#endif  // SRC_UI_INPUT_DRIVERS_USB_HID_FUNCTION_TWO_ENDPOINT_HID_FUNCTION_H_
