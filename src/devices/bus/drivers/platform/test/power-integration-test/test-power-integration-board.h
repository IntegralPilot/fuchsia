// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_INTEGRATION_BOARD_H_
#define SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_INTEGRATION_BOARD_H_

#include <lib/driver/component/cpp/driver_base2.h>

namespace power_integration_board {

class PowerIntegrationBoard : public fdf::DriverBase2 {
 public:
  PowerIntegrationBoard() : fdf::DriverBase2("power-integration-board") {}
  ~PowerIntegrationBoard() override = default;
  zx::result<> Start(fdf::DriverContext context) override;

 private:
  fidl::WireSyncClient<fuchsia_driver_framework::Node> node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
};
}  // namespace power_integration_board

#endif  // SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_INTEGRATION_BOARD_H_
