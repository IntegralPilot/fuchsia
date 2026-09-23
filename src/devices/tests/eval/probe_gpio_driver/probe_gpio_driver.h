// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TESTS_EVAL_PROBE_GPIO_DRIVER_PROBE_GPIO_DRIVER_H_
#define SRC_DEVICES_TESTS_EVAL_PROBE_GPIO_DRIVER_PROBE_GPIO_DRIVER_H_

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.pin/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/logger.h>

class ProbeGpioDriver final : public fdf::DriverBase2,
                              public fidl::WireServer<fuchsia_hardware_pin::Debug>,
                              public fidl::WireServer<fuchsia_hardware_pin::Pin>,
                              public fidl::WireServer<fuchsia_hardware_gpio::Gpio> {
 public:
  ProbeGpioDriver();

  zx::result<> Start(fdf::DriverContext context) override;

  void ServeDebug(fidl::ServerEnd<fuchsia_hardware_pin::Debug> server);

  // fidl::WireServer<fuchsia_hardware_pin::Debug> implementation.
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void ConnectPin(fuchsia_hardware_pin::wire::DebugConnectPinRequest* request,
                  ConnectPinCompleter::Sync& completer) override;
  void ConnectGpio(fuchsia_hardware_pin::wire::DebugConnectGpioRequest* request,
                   ConnectGpioCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Debug> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_hardware_pin::Pin> implementation.
  void Configure(fuchsia_hardware_pin::wire::PinConfigureRequest* request,
                 ConfigureCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Pin> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_hardware_gpio::Gpio> implementation.
  void Read(ReadCompleter::Sync& completer) override;
  void SetBufferMode(SetBufferModeRequestView request,
                     SetBufferModeCompleter::Sync& completer) override;
  void GetInterrupt(GetInterruptRequestView request,
                    GetInterruptCompleter::Sync& completer) override;
  void ConfigureInterrupt(fuchsia_hardware_gpio::wire::GpioConfigureInterruptRequest* request,
                          ConfigureInterruptCompleter::Sync& completer) override;
  void ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  uint8_t current_value_ = 0;
  driver_devfs::Connector<fuchsia_hardware_pin::Debug> devfs_connector_;
  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::WireClient<fuchsia_driver_framework::Node> node_;
  fidl::ServerBindingGroup<fuchsia_hardware_pin::Debug> debug_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_pin::Pin> pin_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_gpio::Gpio> gpio_bindings_;
};

#endif  // SRC_DEVICES_TESTS_EVAL_PROBE_GPIO_DRIVER_PROBE_GPIO_DRIVER_H_
