// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "driver.h"

#include <fidl/fuchsia.power.battery/cpp/natural_types.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/fit/function.h>

#include <utility>

namespace fake_battery {

Driver::Driver() : DriverBase2("fake-battery") {}

zx::result<> Driver::Start(fdf::DriverContext context) {
  protocol_server_battery_ = std::make_unique<BatteryProtocolServer>(dispatcher());
  zx_status_t status = protocol_server_battery_->Init(outgoing());
  if (status != ZX_OK) {
    fdf::error("Failed to init battery protocol server: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  hardware_battery_server_ = std::make_unique<HardwareBatteryServer>(dispatcher());
  status = hardware_battery_server_->Init(outgoing());
  if (status != ZX_OK) {
    fdf::error("Failed to init hardware battery server: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  fdf::info("Successfully started fake-battery driver components");
  return zx::ok();
}

}  // namespace fake_battery

FUCHSIA_DRIVER_EXPORT2(fake_battery::Driver);
