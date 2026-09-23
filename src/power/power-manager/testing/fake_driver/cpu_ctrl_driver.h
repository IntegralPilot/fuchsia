// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CPU_CTRL_DRIVER_H_
#define SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CPU_CTRL_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include "cpu_ctrl_server.h"

namespace fake_driver {

class CpuCtrlDriver : public fdf::DriverBase2 {
 public:
  CpuCtrlDriver();

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  // Add a child device node and offer the service capabilities.
  template <typename T>
  zx::result<> AddChild(const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent,
                        std::string_view node_name, std::string_view class_name,
                        driver_devfs::Connector<T>& devfs_connector);

  // Start serving Protocol (to be called by the devfs connector when a connection is established).
  void ServeCpuCtrl(fidl::ServerEnd<fuchsia_hardware_cpu_ctrl::Device> server);

  driver_devfs::Connector<fuchsia_hardware_cpu_ctrl::Device> cpu_ctrl_connector_;

  std::vector<fidl::ClientEnd<fuchsia_driver_framework::Node>> nodes_;

  CpuCtrlProtocolServer cpu_ctrl_server_;
};

}  // namespace fake_driver

#endif  // SRC_POWER_POWER_MANAGER_TESTING_FAKE_DRIVER_CPU_CTRL_DRIVER_H_
