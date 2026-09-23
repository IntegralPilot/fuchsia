// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_TEMPERATURE_DRIVER_H_
#define SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_TEMPERATURE_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include "control_server.h"
#include "temperature_server.h"

namespace fake_driver {

class Driver : public fdf::DriverBase2 {
 public:
  Driver();

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  // Add a child device node and offer the service capabilities.
  template <typename A, typename B>
  zx::result<> AddDriverAndControl(const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent,
                                   std::string_view driver_node_name,
                                   std::string_view driver_class_name,
                                   driver_devfs::Connector<A>& driver_devfs_connector,
                                   driver_devfs::Connector<B>& control_devfs_connector);
  template <typename T>
  zx::result<const fidl::ClientEnd<fuchsia_driver_framework::Node>*> AddChild(
      const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent, std::string_view node_name,
      std::string_view class_name, driver_devfs::Connector<T>& devfs_connector);

  // Start serving Protocol (to be called by the devfs connector when a connection is established).
  void ServeTemperature(fidl::ServerEnd<fuchsia_hardware_temperature::Device> server);
  void ServeControl(fidl::ServerEnd<fuchsia_powermanager_driver_temperaturecontrol::Device> server);

  driver_devfs::Connector<fuchsia_hardware_temperature::Device> temperature_connector_;
  driver_devfs::Connector<fuchsia_powermanager_driver_temperaturecontrol::Device>
      control_connector_;

  std::vector<fidl::ClientEnd<fuchsia_driver_framework::Node>> nodes_;
  std::vector<fidl::WireSyncClient<fuchsia_driver_framework::NodeController>> controllers_;

  TemperatureDeviceProtocolServer temperature_server_;
  ControlDeviceProtocolServer control_server_;

  float temperature_ = 0.0f;
};

}  // namespace fake_driver

#endif  // SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_TEMPERATURE_DRIVER_H_
