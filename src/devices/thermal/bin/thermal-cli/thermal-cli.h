// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_THERMAL_BIN_THERMAL_CLI_THERMAL_CLI_H_
#define SRC_DEVICES_THERMAL_BIN_THERMAL_CLI_THERMAL_CLI_H_

#include <fidl/fuchsia.hardware.thermal/cpp/wire.h>
#include <lib/zx/result.h>

#include <string>

class ThermalCli {
 public:
  explicit ThermalCli(fidl::ClientEnd<fuchsia_hardware_thermal::Device> device)
      : device_(std::move(device)) {}

  zx_status_t PrintTemperature();
  zx_status_t FanLevelCommand(const char* value);
  zx_status_t FrequencyCommand(fuchsia_hardware_thermal::wire::PowerDomain cluster,
                               const char* value);
  zx::result<std::string> GetSensorName();

 private:
  const fidl::WireSyncClient<fuchsia_hardware_thermal::Device> device_;
};

#endif  // SRC_DEVICES_THERMAL_BIN_THERMAL_CLI_THERMAL_CLI_H_
