// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_NELSON_DRIVER_H_
#define SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_NELSON_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include <optional>

#include "control_server.h"
#include "temperature_server.h"

namespace fake_driver {

class Driver : public fdf::DriverBase2 {
 public:
  Driver();

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  // Add a fake temperature driver and a control driver.
  template <typename A, typename B>
  zx::result<> AddDriverAndControl(fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
                                   std::string_view driver_node_name,
                                   std::string_view driver_class_name,
                                   driver_devfs::Connector<A>& driver_devfs_connector,
                                   driver_devfs::Connector<B>& control_devfs_connector);
  // Add a child device node and offer the service capabilities.
  template <typename T>
  zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> AddChild(
      fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, std::string_view node_name,
      std::string_view class_name, driver_devfs::Connector<T>& devfs_connector);
  // Add a series of nodes.
  zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> AddNodes(
      fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
      const std::vector<std::string_view>& nodes);
  // Add a single child node.
  zx::result<fidl::UnownedClientEnd<fuchsia_driver_framework::Node>> AddNode(
      fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, std::string_view node_name);

  // Start serving Protocol (to be called by the devfs connector when a connection is established).
  void ServeSocControl(
      fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server);
  void ServeAudioControl(
      fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server);
  void ServeThreadControl(
      fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server);
  void ServeSocTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server);
  void ServeAudioTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server);
  void ServeThreadTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server);

  driver_devfs::Connector<fuchsia_powermanager_driver_temperaturecontrol::Device>
      soc_control_connector_;
  driver_devfs::Connector<fuchsia_powermanager_driver_temperaturecontrol::Device>
      audio_control_connector_;
  driver_devfs::Connector<fuchsia_powermanager_driver_temperaturecontrol::Device>
      thread_control_connector_;
  driver_devfs::Connector<fuchsia_hardware_temperature::Device> soc_temperature_connector_;
  driver_devfs::Connector<fuchsia_hardware_temperature::Device> audio_temperature_connector_;
  driver_devfs::Connector<fuchsia_hardware_temperature::Device> thread_temperature_connector_;

  std::vector<fidl::ClientEnd<fuchsia_driver_framework::Node>> nodes_;
  std::vector<fidl::WireSyncClient<fuchsia_driver_framework::NodeController>> controllers_;

  ControlDeviceProtocolServer soc_control_server_;
  ControlDeviceProtocolServer audio_control_server_;
  ControlDeviceProtocolServer thread_control_server_;
  TemperatureDeviceProtocolServer soc_temperature_server_;
  TemperatureDeviceProtocolServer audio_temperature_server_;
  TemperatureDeviceProtocolServer thread_temperature_server_;

  float soc_temperature_ = 0.0f;
  float audio_temperature_ = 0.0f;
  float thread_temperature_ = 0.0f;
};

}  // namespace fake_driver

#endif  // SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_NELSON_DRIVER_H_
