// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "probe_gpio_driver.h"

ProbeGpioDriver::ProbeGpioDriver()
    : fdf::DriverBase2("probe-gpio"),
      devfs_connector_(fit::bind_member<&ProbeGpioDriver::ServeDebug>(this)) {}

zx::result<> ProbeGpioDriver::Start(fdf::DriverContext context) {
  fidl::Arena arena;
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector(std::move(connector.value()))
                   .class_name("gpio")
                   .Build();

  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, "probe-gpio")
                  .devfs_args(devfs)
                  .Build();

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
  if (node_endpoints.is_error()) {
    return node_endpoints.take_error();
  }

  fidl::WireResult result = fidl::WireCall(node())->AddChild(
      args, std::move(controller_endpoints.server), std::move(node_endpoints->server));
  if (!result.ok()) {
    FDF_LOG(ERROR, "Failed to add child devfs node: %s", result.status_string());
    return zx::error(result.status());
  }
  controller_.Bind(std::move(controller_endpoints.client), dispatcher());
  node_.Bind(std::move(node_endpoints->client), dispatcher());
  return zx::ok();
}

void ProbeGpioDriver::ServeDebug(fidl::ServerEnd<fuchsia_hardware_pin::Debug> server) {
  debug_bindings_.AddBinding(dispatcher(), std::move(server), this, fidl::kIgnoreBindingClosure);
}

void ProbeGpioDriver::GetProperties(GetPropertiesCompleter::Sync& completer) {
  fidl::Arena arena;
  auto properties = fuchsia_hardware_pin::wire::DebugGetPropertiesResponse::Builder(arena)
                        .name("probe-gpio")
                        .pin(0)
                        .Build();
  completer.Reply(properties);
}

void ProbeGpioDriver::ConnectPin(fuchsia_hardware_pin::wire::DebugConnectPinRequest* request,
                                 ConnectPinCompleter::Sync& completer) {
  pin_bindings_.AddBinding(dispatcher(), std::move(request->server), this,
                           fidl::kIgnoreBindingClosure);
  completer.ReplySuccess();
}

void ProbeGpioDriver::ConnectGpio(fuchsia_hardware_pin::wire::DebugConnectGpioRequest* request,
                                  ConnectGpioCompleter::Sync& completer) {
  gpio_bindings_.AddBinding(dispatcher(), std::move(request->server), this,
                            fidl::kIgnoreBindingClosure);
  completer.ReplySuccess();
}

void ProbeGpioDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Debug> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

void ProbeGpioDriver::Configure(fuchsia_hardware_pin::wire::PinConfigureRequest* request,
                                ConfigureCompleter::Sync& completer) {
  completer.ReplySuccess(request->config);
}

void ProbeGpioDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Pin> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

void ProbeGpioDriver::Read(ReadCompleter::Sync& completer) {
  FDF_LOG(INFO, "Received read request: %u", current_value_);
  completer.ReplySuccess(current_value_ != 0);
}

void ProbeGpioDriver::SetBufferMode(SetBufferModeRequestView request,
                                    SetBufferModeCompleter::Sync& completer) {
  current_value_ = (request->mode == fuchsia_hardware_gpio::BufferMode::kOutputHigh) ? 1 : 0;
  FDF_LOG(INFO, "Received write request: %u", current_value_);
  completer.ReplySuccess();
}

void ProbeGpioDriver::GetInterrupt(GetInterruptRequestView request,
                                   GetInterruptCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void ProbeGpioDriver::ConfigureInterrupt(
    fuchsia_hardware_gpio::wire::GpioConfigureInterruptRequest* request,
    ConfigureInterruptCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void ProbeGpioDriver::ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void ProbeGpioDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

FUCHSIA_DRIVER_EXPORT2(ProbeGpioDriver);
