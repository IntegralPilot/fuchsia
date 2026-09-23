// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/input/drivers/hid-input-report/driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace hid_input_report_dev {

namespace finput = fuchsia_hardware_input;

zx::result<> InputReportDriver::Start(fdf::DriverContext context) {
  zx::result<fidl::ClientEnd<finput::Controller>> controller =
      context.incoming().Connect<finput::Service::Controller>();
  if (controller.is_error()) {
    fdf::error("Failed to connect to fuchsia_hardware_input service: {}", controller);
    return controller.take_error();
  }

  {
    auto [client, server] = fidl::Endpoints<finput::Device>::Create();
    auto result = fidl::WireCall(controller.value())->OpenSession(std::move(server));
    if (!result.ok()) {
      return zx::error(result.status());
    }

    input_report_ = std::make_unique<InputReport>(std::move(client));
  }

  // Expose the driver's inspect data.

  // Start the inner DFv1 driver.
  auto status = input_report_->Start();
  if (status != ZX_OK) {
    fdf::error("Failed to start input report {}", status);
    return zx::error(status);
  }

  // Export our InputReport protocol.
  auto result = outgoing()->component().AddUnmanagedProtocol<fuchsia_input_report::InputDevice>(
      input_report_bindings_.CreateHandler(input_report_.get(), dispatcher(),
                                           fidl::kIgnoreBindingClosure),
      kDeviceName);
  if (result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = CreateDevfsNode(); result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

void InputReportDriver::Stop(fdf::StopCompleter completer) {
  input_report_.reset();
  completer(zx::ok());
}

zx::result<> InputReportDriver::CreateDevfsNode() {
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args{
      {.connector = std::move(connector).value(), .class_name = "input-report"}};

  zx::result child = AddOwnedChild(kDeviceName, devfs_args);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child).value();

  return zx::ok();
}

}  // namespace hid_input_report_dev

FUCHSIA_DRIVER_EXPORT2(hid_input_report_dev::InputReportDriver);
