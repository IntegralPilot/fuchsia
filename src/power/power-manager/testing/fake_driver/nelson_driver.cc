// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "nelson_driver.h"

#include <lib/driver/logging/cpp/structured_logger.h>

namespace fake_driver {

Driver::Driver()
    : DriverBase2("fake-driver"),
      soc_control_connector_(fit::bind_member<&Driver::ServeSocControl>(this)),
      audio_control_connector_(fit::bind_member<&Driver::ServeAudioControl>(this)),
      thread_control_connector_(fit::bind_member<&Driver::ServeThreadControl>(this)),
      soc_temperature_connector_(fit::bind_member<&Driver::ServeSocTemperature>(this)),
      audio_temperature_connector_(fit::bind_member<&Driver::ServeAudioTemperature>(this)),
      thread_temperature_connector_(fit::bind_member<&Driver::ServeThreadTemperature>(this)),
      soc_control_server_(&soc_temperature_),
      audio_control_server_(&audio_temperature_),
      thread_control_server_(&thread_temperature_),
      soc_temperature_server_("CPU thermal", &soc_temperature_),
      audio_temperature_server_("therm-audio", &audio_temperature_),
      thread_temperature_server_("therm-thread", &thread_temperature_) {}

zx::result<> Driver::Start(fdf::DriverContext context) {
  auto soc_nodes_result = AddNodes(node(), {"aml-thermal-pll", "aml_thermal_pll"});
  if (soc_nodes_result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add `aml-thermal-pll/aml_thermal_pll`",
             KV("status", soc_nodes_result.status_string()));
    return soc_nodes_result.take_error();
  }

  auto result = AddDriverAndControl(soc_nodes_result.value(), "thermal", "thermal",
                                    soc_temperature_connector_, soc_control_connector_);
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add `/thermal` or `/thermal/control`",
             KV("status", result.status_string()));
    return result.take_error();
  }

  auto thermistor_nodes_result = AddNodes(node(), {"thermistor", "thermistor-device"});
  if (thermistor_nodes_result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add `thermistor/thermistor-device`",
             KV("status", thermistor_nodes_result.status_string()));
    return thermistor_nodes_result.take_error();
  }

  result = AddDriverAndControl(thermistor_nodes_result.value(), "therm-thread", "temperature",
                               thread_temperature_connector_, thread_control_connector_);
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add `therm-thread` or `therm-thread/control`",
             KV("status", result.status_string()));
    return result.take_error();
  }

  result = AddDriverAndControl(thermistor_nodes_result.value(), "therm-audio", "temperature",
                               audio_temperature_connector_, audio_control_connector_);
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add `therm-audio` or `therm-audio/control`",
             KV("status", result.status_string()));
    return result.take_error();
  }

  return zx::ok();
}

// Add a fake temperature driver and a control driver
template <typename A, typename B>
zx::result<> Driver::AddDriverAndControl(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
    std::string_view driver_node_name, std::string_view driver_class_name,
    driver_devfs::Connector<A>& driver_devfs_connector,
    driver_devfs::Connector<B>& control_devfs_connector) {
  zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> add_child_result =
      AddChild(parent, driver_node_name, driver_class_name, driver_devfs_connector);
  if (add_child_result.is_error()) {
    return add_child_result.take_error();
  }

  auto result = AddChild(add_child_result.value(), "control", "test", control_devfs_connector);
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok();
}

// Add a child device node and offer the service capabilities.
template <typename T>
zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> Driver::AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, std::string_view node_name,
    std::string_view class_name, driver_devfs::Connector<T>& devfs_connector) {
  fidl::Arena arena;
  zx::result connector = devfs_connector.Bind(dispatcher());

  if (connector.is_error()) {
    return connector.take_error();
  }

  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector(std::move(connector.value()))
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
    FDF_SLOG(ERROR, "Failed to add child", KV("status", result.FormatDescription()));
    return zx::error(result.status());
  }

  controllers_.emplace_back(std::move(controller_endpoints->client));
  nodes_.emplace_back(std::move(node_endpoints->client));

  return zx::ok(nodes_.back().borrow());
}

// Add a series of nodes.
zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> Driver::AddNodes(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
    const std::vector<std::string_view>& nodes) {
  zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> add_node_result =
      zx::ok(parent);
  for (const auto& node : nodes) {
    add_node_result = AddNode(add_node_result.value(), node);
    if (add_node_result.is_error()) {
      return add_node_result.take_error();
    }
  }

  return add_node_result;
}

// Add a single child node.
zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> Driver::AddNode(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, std::string_view node_name) {
  fidl::Arena arena;

  auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, node_name).Build();

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

  return zx::ok(nodes_.back().borrow());
}

void Driver::ServeSocTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server) {
  soc_temperature_server_.Serve(dispatcher(), std::move(server));
}

void Driver::ServeThreadTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server) {
  thread_temperature_server_.Serve(dispatcher(), std::move(server));
}

void Driver::ServeAudioTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server) {
  audio_temperature_server_.Serve(dispatcher(), std::move(server));
}

void Driver::ServeSocControl(
    fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server) {
  soc_control_server_.Serve(dispatcher(), std::move(server));
}

void Driver::ServeThreadControl(
    fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server) {
  thread_control_server_.Serve(dispatcher(), std::move(server));
}

void Driver::ServeAudioControl(
    fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server) {
  audio_control_server_.Serve(dispatcher(), std::move(server));
}

}  // namespace fake_driver

FUCHSIA_DRIVER_EXPORT2(fake_driver::Driver);
