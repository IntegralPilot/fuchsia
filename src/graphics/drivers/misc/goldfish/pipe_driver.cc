// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/misc/goldfish/pipe_driver.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>

#include <memory>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/goldfish/platform/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>

#include "src/graphics/drivers/misc/goldfish/pipe_device.h"

namespace goldfish {

PipeDriver::PipeDriver() : fdf::DriverBase2("goldfish-pipe") {}

PipeDriver::~PipeDriver() = default;

zx::result<> PipeDriver::Start(fdf::DriverContext context) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_acpi::Device>> acpi_client =
      context.incoming().Connect<fuchsia_hardware_acpi::Service::Device>("acpi");
  if (acpi_client.is_error()) {
    fdf::error("Failed to connect to ACPI service: {}", acpi_client);
    return acpi_client.take_error();
  }

  pipe_device_ =
      std::make_unique<PipeDevice>(std::move(acpi_client).value(), driver_dispatcher()->borrow());
  zx::result<> init_result = pipe_device_->Initialize();
  if (init_result.is_error()) {
    fdf::error("Failed to initialize pipe device: {}", init_result);
    return init_result.take_error();
  }

  fuchsia_hardware_goldfish_pipe::Service::InstanceHandler handler = {{
      .device = pipe_device_->fidl::WireServer<fuchsia_hardware_goldfish_pipe::Bus>::bind_handler(
          dispatcher()),
  }};
  zx::result<> add_service_result =
      outgoing()->AddService<fuchsia_hardware_goldfish_pipe::Service>(std::move(handler));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add service: {}", add_service_result);
    return add_service_result.take_error();
  }

  zx::result<fidl::ClientEnd<fuchsia_device_fs::Connector>> devfs_result =
      devfs_connector_.Bind(dispatcher());
  if (devfs_result.is_error()) {
    fdf::error("Failed to bind devfs connector: {}", devfs_result);
    return devfs_result.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs{{
      .connector = std::move(devfs_result).value(),
      .class_name = "goldfish-pipe",
      .connector_supports = fuchsia_device_fs::ConnectionType::kDevice |
                            fuchsia_device_fs::ConnectionType::kController,
  }};
  zx::result<fdf::OwnedChildNode> add_devfs_node_result = AddOwnedChild("goldfish-pipe", devfs);
  if (add_devfs_node_result.is_error()) {
    fdf::error("Failed to add devfs node: {}", add_devfs_node_result);
    return add_devfs_node_result.take_error();
  }
  devfs_child_node_ = std::move(*add_devfs_node_result);

  const std::vector<fuchsia_driver_framework::Offer> kServiceOffers = {
      fdf::MakeOffer2<fuchsia_hardware_goldfish_pipe::Service>(component::kDefaultInstance)};
  const std::vector<fuchsia_driver_framework::NodeProperty2> kControlProperties = {
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                         bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                         bind_fuchsia_goldfish_platform::BIND_PLATFORM_DEV_PID_GOLDFISH),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                         bind_fuchsia_goldfish_platform::BIND_PLATFORM_DEV_DID_PIPE_CONTROL),
  };
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> add_control_child_result =
      AddChild("goldfish-pipe-control", kControlProperties, kServiceOffers);
  if (add_control_child_result.is_error()) {
    fdf::error("Failed to add control child: {}", add_control_child_result);
    return add_control_child_result.take_error();
  }
  control_child_ = std::move(add_control_child_result).value();

  return zx::ok();
}

void PipeDriver::Stop(fdf::StopCompleter completer) {
  zx::result<> result = pipe_device_->PrepareStop();
  if (result.is_error()) {
    fdf::error("Failed to prepare pipe device for stop: {}", result);
  }
  completer(result);
}

void PipeDriver::ServePipeDevice(fidl::ServerEnd<fuchsia_hardware_goldfish::PipeDevice> server) {
  pipe_device_bindings_.AddBinding(dispatcher(), std::move(server), pipe_device_.get(),
                                   fidl::kIgnoreBindingClosure);
}

}  // namespace goldfish

FUCHSIA_DRIVER_EXPORT2(goldfish::PipeDriver);
