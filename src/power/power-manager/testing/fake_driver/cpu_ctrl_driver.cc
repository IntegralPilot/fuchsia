// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cpu_ctrl_driver.h"

#include <lib/driver/logging/cpp/structured_logger.h>

namespace fake_driver {

CpuCtrlDriver::CpuCtrlDriver()
    : DriverBase2("fake-driver"),
      cpu_ctrl_connector_(fit::bind_member<&CpuCtrlDriver::ServeCpuCtrl>(this)),
      cpu_ctrl_server_() {}

zx::result<> CpuCtrlDriver::Start(fdf::DriverContext context) {
  // Create driver with topo suffix `/cpu` and class path `/dev/class/cpu-ctrl/000
  auto result = AddChild(node(), "cpu", "cpu-ctrl", cpu_ctrl_connector_);
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add driver", KV("status", result.status_string()));
    return result.take_error();
  }

  fuchsia_hardware_cpu_ctrl::Service::InstanceHandler handler({
      .device = fit::bind_member<&CpuCtrlDriver::ServeCpuCtrl>(this),
  });

  result = outgoing()->AddService<fuchsia_hardware_cpu_ctrl::Service>(std::move(handler));
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add service", KV("status", result.status_string()));
    return result.take_error();
  }

  return zx::ok();
}

template <typename T>
zx::result<> CpuCtrlDriver::AddChild(const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent,
                                     std::string_view node_name, std::string_view class_name,
                                     driver_devfs::Connector<T>& devfs_connector) {
  fidl::Arena arena;
  zx::result connector = devfs_connector.Bind(dispatcher());

  if (connector.is_error()) {
    return connector.take_error();
  }

  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector(std::move(connector.value()))
                   .connector_supports(fuchsia_device_fs::ConnectionType::kDevice |
                                       fuchsia_device_fs::ConnectionType::kController)
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

  nodes_.emplace_back(std::move(node_endpoints->client));

  return zx::ok();
}

void CpuCtrlDriver::ServeCpuCtrl(fidl::ServerEnd<fuchsia_hardware_cpu_ctrl::Device> server) {
  cpu_ctrl_server_.Serve(dispatcher(), std::move(server));
}

}  // namespace fake_driver

FUCHSIA_DRIVER_EXPORT2(fake_driver::CpuCtrlDriver);
