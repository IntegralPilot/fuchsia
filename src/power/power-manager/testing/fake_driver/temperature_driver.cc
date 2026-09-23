// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "temperature_driver.h"

#include <lib/driver/logging/cpp/structured_logger.h>

namespace fake_driver {

Driver::Driver()
    : DriverBase2("fake-driver"),
      temperature_connector_(fit::bind_member<&Driver::ServeTemperature>(this)),
      control_connector_(fit::bind_member<&Driver::ServeControl>(this)),
      temperature_server_("soc_thermal", &temperature_),
      control_server_(&temperature_) {}

zx::result<> Driver::Start(fdf::DriverContext context) {
  auto result = AddDriverAndControl(node(), "soc_thermal", "temperature", temperature_connector_,
                                    control_connector_);
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add driver and its control", KV("status", result.status_string()));
    return result.take_error();
  }

  return zx::ok();
}

template <typename A, typename B>
zx::result<> Driver::AddDriverAndControl(
    const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent,
    std::string_view driver_node_name, std::string_view driver_class_name,
    driver_devfs::Connector<A>& driver_devfs_connector,
    driver_devfs::Connector<B>& control_devfs_connector) {
  // Create driver with topo suffix `/soc_thermal` and class path `/dev/class/temperature/000
  zx::result<const fidl::ClientEnd<fuchsia_driver_framework::Node>*> add_child_result =
      AddChild(parent, driver_node_name, driver_class_name, driver_devfs_connector);
  if (add_child_result.is_error()) {
    return add_child_result.take_error();
  }
  // Create control driver with topo suffix `/soc_thermal/control` and class path
  // `/dev/class/test/000
  auto result = AddChild(*add_child_result.value(), "control", "test", control_devfs_connector);
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok();
}

// Add a child device node and offer the service capabilities.
template <typename T>
zx::result<const fidl::ClientEnd<fuchsia_driver_framework::Node>*> Driver::AddChild(
    const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent, std::string_view node_name,
    std::string_view class_name, driver_devfs::Connector<T>& devfs_connector) {
  fidl::Arena arena;
  zx::result connector = devfs_connector.Bind(dispatcher());

  if (connector.is_error()) {
    return connector.take_error();
  }

  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector(std::move(connector.value()))
                   .connector_supports(fuchsia_device_fs::ConnectionType::kDevice)
                   .class_name(class_name);

  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, node_name)
                  .devfs_args(devfs.Build())
                  .Build();

  // Create endpoints of the `NodeController` for the node.
  auto controller_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (controller_endpoints.is_error()) {
    FDF_SLOG(ERROR, "Failed to create node controller endpoint",
             KV("status", controller_endpoints.status_string()));
    return zx::error(controller_endpoints.status_value());
  }

  zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
  if (node_endpoints.is_error()) {
    FDF_SLOG(ERROR, "Failed to create node endpoint", KV("status", node_endpoints.status_string()));
    return zx::error(node_endpoints.status_value());
  }

  auto result = fidl::WireCall(parent)->AddChild(args, std::move(controller_endpoints->server),
                                                 std::move(node_endpoints->server));
  if (!result.ok()) {
    FDF_SLOG(ERROR, "Failed to add child", KV("status", result.status_string()));
    return zx::error(result.status());
  }

  controllers_.emplace_back(std::move(controller_endpoints->client));
  nodes_.emplace_back(std::move(node_endpoints->client));

  return zx::ok(&nodes_.back());
}

void Driver::ServeTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server) {
  temperature_server_.Serve(dispatcher(), std::move(server));
}

void Driver::ServeControl(
    fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server) {
  control_server_.Serve(dispatcher(), std::move(server));
}

}  // namespace fake_driver

FUCHSIA_DRIVER_EXPORT2(fake_driver::Driver);
