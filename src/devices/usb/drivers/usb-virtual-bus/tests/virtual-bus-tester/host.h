// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_HOST_H_
#define SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_HOST_H_

#include <fidl/fuchsia.hardware.usb.virtualbustest/cpp/fidl.h>
#include <fuchsia/hardware/usb/cpp/banjo.h>
#include <lib/driver/component/cpp/driver_base2.h>

namespace virtualbus {

class Device : public fdf::DriverBase2,
               public fidl::Server<fuchsia_hardware_usb_virtualbustest::BusTest> {
 private:
  static constexpr std::string_view kName = "virtual-bus-test";

 public:
  Device() : fdf::DriverBase2(kName) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override { completer(zx::ok()); }

 protected:
  ddk::UsbProtocolClient usb_client_ = {};

  std::optional<OutCompleter::Async> out_completer_;
  std::optional<InCompleter::Async> in_completer_;

  uint8_t bulk_out_addr_ = 0;
  uint8_t bulk_in_addr_ = 0;

 private:
  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override;
  void Out(OutRequest& request, OutCompleter::Sync& completer) override;
  void In(InRequest& request, InCompleter::Sync& completer) override;

  virtual void QueueOut(std::vector<uint8_t> data) = 0;
  virtual void QueueIn(size_t size) = 0;

  fdf::OwnedChildNode child_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_virtualbustest::BusTest> bindings_;

 protected:
  std::shared_ptr<fdf::Namespace> incoming_;
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }
};

}  // namespace virtualbus

#endif  // SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_TESTS_VIRTUAL_BUS_TESTER_HOST_H_
