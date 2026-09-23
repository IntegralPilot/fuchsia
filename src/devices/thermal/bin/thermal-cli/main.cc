// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/protocol.h>

#include "thermal-cli.h"

constexpr char kUsageMessage[] = R"""(Usage: thermal-cli <device> <command>

    temp - Read the device's thermal sensor in degrees C
    fan [value] - Get or set the fan speed
    freq <big/little> [value] - Get or set the cluster frequency in Hz
    name - Print the name of the sensor

    Example:
    thermal-cli /dev/class/thermal/000 freq big 1000000000
)""";

int main(int argc, char** argv) {
  if (argc < 3) {
    printf("%s", kUsageMessage);
    return 0;
  }

  zx::result device = component::Connect<fuchsia_hardware_thermal::Device>(argv[1]);
  if (device.is_error()) {
    fprintf(stderr, "Failed to open thermal device: %s\n", device.status_string());
    return 1;
  }

  ThermalCli thermal_cli(std::move(device.value()));

  zx_status_t status;
  if (strcmp(argv[2], "temp") == 0) {
    status = thermal_cli.PrintTemperature();
  } else if (strcmp(argv[2], "fan") == 0) {
    const char* value = argc >= 4 ? argv[3] : nullptr;
    status = thermal_cli.FanLevelCommand(value);
  } else if (strcmp(argv[2], "freq") == 0 && argc >= 4) {
    fuchsia_hardware_thermal::wire::PowerDomain cluster =
        fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain;
    if (strcmp(argv[3], "little") == 0) {
      cluster = fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain;
    }

    const char* value = argc >= 5 ? argv[4] : nullptr;
    status = thermal_cli.FrequencyCommand(cluster, value);
  } else if (strcmp(argv[2], "name") == 0) {
    const zx::result name = thermal_cli.GetSensorName();
    status = name.status_value();
    if (name.is_ok()) {
      printf("Name: %s\n", name->c_str());
    }
  } else {
    printf("%s", kUsageMessage);
    return 1;
  }

  if (status != ZX_OK) {
    return 1;
  }
  return 0;
}
