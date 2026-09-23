// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_FUNCTION_PERIPHERAL_H_
#define SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_FUNCTION_PERIPHERAL_H_

#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.virtualbustest/cpp/fidl.h>
#include <fuchsia/hardware/usb/descriptor/cpp/banjo.h>
#include <lib/driver/component/cpp/driver_base2.h>

#include <queue>

#include <usb/descriptors.h>

namespace virtualbus {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

class TestFunction : public fdf::DriverBase2,
                     public fidl::Server<fuchsia_hardware_usb_virtualbustest::ExpectBusTest> {
 protected:
  static constexpr std::string_view kName = "virtual-bus-test-peripheral";
  static constexpr auto kMaxPacketSize = 20;

 public:
  TestFunction() : fdf::DriverBase2(kName) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override { completer(zx::ok()); }

 protected:
  zx::result<std::vector<uint8_t>> DoControl(const fuchsia_hardware_usb_descriptor::UsbSetup& setup,
                                             std::vector<uint8_t> write_data);

  virtual zx::result<> SetFunctionInterface(bool connect) = 0;

  struct VirtualBusTestDescriptor {
    usb_interface_descriptor_t interface;
    usb_endpoint_descriptor_t bulk_out;
    usb_endpoint_descriptor_t bulk_in;
  } __PACKED descriptor_ = {
      .interface =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = 0,
              .b_alternate_setting = 0,
              .b_num_endpoints = 1,
              .b_interface_class = 0xFF,
              .b_interface_sub_class = 0xFF,
              .b_interface_protocol = 0xFF,
              .i_interface = 0,
          },
      .bulk_out =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = USB_ENDPOINT_OUT,
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = 512,
              .b_interval = 0,
          },
      .bulk_in =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = USB_ENDPOINT_IN,
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = 512,
              .b_interval = 0,
          },
  };

  std::optional<ExpectOutCompleter::Async> expect_out_;
  std::optional<ExpectInCompleter::Async> expect_in_;
  bool configured_ = false;

 private:
  void ExpectControl(ExpectControlRequest& request,
                     ExpectControlCompleter::Sync& completer) override;
  void ExpectOut(ExpectOutCompleter::Sync& completer) override;
  void ExpectIn(ExpectInRequest& request, ExpectInCompleter::Sync& completer) override;
  void Sync(SyncCompleter::Sync& completer) override { completer.Reply(); }
  void Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) override;

  virtual void QueueOut() = 0;
  virtual void QueueIn(std::vector<uint8_t> data) = 0;

  fdf::OwnedChildNode child_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_virtualbustest::ExpectBusTest> bindings_;

  std::vector<uint8_t> expect_control_data_;
  std::optional<ExpectControlCompleter::Async> expect_control_;

 protected:
  std::shared_ptr<fdf::Namespace> incoming_;
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }
};
}  // namespace virtualbus

#endif  // SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_FUNCTION_PERIPHERAL_H_
