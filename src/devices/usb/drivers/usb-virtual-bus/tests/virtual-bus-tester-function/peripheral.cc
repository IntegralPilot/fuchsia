// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-virtual-bus/tests/virtual-bus-tester-function/peripheral.h"

#include <lib/driver/compat/cpp/compat.h>

#include <usb/request-cpp.h>

namespace virtualbus {

void TestFunction::ExpectControl(ExpectControlRequest& request,
                                 ExpectControlCompleter::Sync& completer) {
  if (expect_control_.has_value()) {
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  expect_control_data_ = std::move(request.in_data());
  expect_control_ = completer.ToAsync();
}

void TestFunction::ExpectOut(ExpectOutCompleter::Sync& completer) {
  if (expect_out_.has_value()) {
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  expect_out_ = completer.ToAsync();

  QueueOut();
}

void TestFunction::ExpectIn(ExpectInRequest& request, ExpectInCompleter::Sync& completer) {
  if (expect_in_.has_value()) {
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  expect_in_ = completer.ToAsync();

  QueueIn(std::move(request.data()));
}

void TestFunction::Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) {
  zx::result<> result = SetFunctionInterface(request.connect());
  if (result.is_error()) {
    fdf::error("SetFunctionInterface failed {}", result);
  }
  completer.Reply();
}

zx::result<std::vector<uint8_t>> TestFunction::DoControl(
    const fuchsia_hardware_usb_descriptor::UsbSetup& setup, std::vector<uint8_t> write_data) {
  if (!expect_control_) {
    return zx::ok(std::vector<uint8_t>{});
  }

  if (setup.b_request() != 0xFF) {
    expect_control_->Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    expect_control_.reset();
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  if (setup.w_value() != 0xA) {
    expect_control_->Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    expect_control_.reset();
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  if (setup.bm_request_type() == (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE)) {
    std::vector<uint8_t> data = expect_control_data_;
    expect_control_->Reply(zx::ok(std::vector<uint8_t>{}));
    expect_control_.reset();
    return zx::ok(std::move(data));
  }
  if (setup.bm_request_type() == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE)) {
    expect_control_->Reply(zx::ok(write_data));
    expect_control_.reset();
    return zx::ok(std::vector<uint8_t>{});
  }

  expect_control_->Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  expect_control_.reset();
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> TestFunction::Start(fdf::DriverContext context) {
  if (!incoming_) {
    incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  }
  zx::result child = AddOwnedChild(kName);
  if (child.is_error()) {
    fdf::error("Failed to add child {}", child);
    return child.take_error();
  }
  child_ = std::move(*child);

  auto serve_result =
      outgoing()->AddService<fuchsia_hardware_usb_virtualbustest::ExpectBusTestService>(
          fuchsia_hardware_usb_virtualbustest::ExpectBusTestService::InstanceHandler({
              .device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
          }));
  if (serve_result.is_error()) {
    fdf::error("Failed to add Device service {}", serve_result);
    return serve_result.take_error();
  }

  return zx::ok();
}

}  // namespace virtualbus
