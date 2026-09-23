// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thermal-cli.h"

namespace {

zx_status_t read_argument_checked(const char* arg, uint32_t* out) {
  char* end;
  int64_t value = strtol(arg, &end, 10);
  if (*end != '\0') {
    return ZX_ERR_INVALID_ARGS;
  }
  if (value < 0 || value >= UINT32_MAX) {
    return ZX_ERR_INVALID_ARGS;
  }
  *out = static_cast<uint32_t>(value);
  return ZX_OK;
}

}  // namespace

zx_status_t ThermalCli::PrintTemperature() {
  const fidl::WireResult result = device_->GetTemperatureCelsius();
  if (!result.ok()) {
    fprintf(stderr, "DeviceGetTemperatureCelsius failed: %s\n", result.status_string());
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    fprintf(stderr, "GetTemperatureCelsius failed: %s\n", zx_status_get_string(status));
    return status;
  }
  printf("Temperature: %0.03f°C\n", response.temp);
  return ZX_OK;
}

zx_status_t ThermalCli::FanLevelCommand(const char* value) {
  if (value == nullptr) {
    const fidl::WireResult result = device_->GetFanLevel();
    if (!result.ok()) {
      fprintf(stderr, "GetFanLevel failed: %s\n", result.status_string());
      return result.status();
    }
    const fidl::WireResponse response = result.value();
    if (zx_status_t status = response.status; status != ZX_OK) {
      fprintf(stderr, "GetFanLevel failed: %s\n", zx_status_get_string(status));
      return status;
    }
    printf("Fan level: %u\n", response.fan_level);
  } else {
    uint32_t fan_level;
    if (zx_status_t status = read_argument_checked(value, &fan_level); status != ZX_OK) {
      fprintf(stderr, "invalid fan level argument: %s\n", value);
      return status;
    }
    const fidl::WireResult result = device_->SetFanLevel(fan_level);
    if (!result.ok()) {
      fprintf(stderr, "SetFanLevel failed: %s\n", result.status_string());
      return result.status();
    }
    const fidl::WireResponse response = result.value();
    if (zx_status_t status = response.status; status != ZX_OK) {
      fprintf(stderr, "SetFanLevel failed: %s\n", zx_status_get_string(status));
      return status;
    }
  }

  return ZX_OK;
}

zx_status_t ThermalCli::FrequencyCommand(fuchsia_hardware_thermal::wire::PowerDomain cluster,
                                         const char* value) {
  const fidl::WireResult result = device_->GetDvfsInfo(cluster);
  if (!result.ok()) {
    fprintf(stderr, "GetDvfsInfo failed: %s\n", result.status_string());
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    fprintf(stderr, "GetDvfsInfo failed: %s\n", zx_status_get_string(status));
    return status;
  }
  fuchsia_hardware_thermal::wire::OperatingPoint& op_info = *response.info;
  if (op_info.count > fuchsia_hardware_thermal::wire::kMaxDvfsOpps) {
    fprintf(stderr, "GetDvfsInfo reported too many operating points\n");
    return ZX_ERR_BAD_STATE;
  }

  if (value == nullptr) {
    const fidl::WireResult result = device_->GetDvfsOperatingPoint(cluster);
    if (!result.ok()) {
      fprintf(stderr, "GetDvfsOperatingPoint failed: %s\n", result.status_string());
      return result.status();
    }
    const fidl::WireResponse response = result.value();
    if (zx_status_t status = response.status; status != ZX_OK) {
      fprintf(stderr, "GetDvfsOperatingPoint failed: %s\n", zx_status_get_string(status));
      return status;
    }
    if (response.op_idx > op_info.count) {
      fprintf(stderr, "GetDvfsOperatingPoint reported an invalid operating point\n");
    }

    printf("Current frequency: %u Hz\n", op_info.opp[response.op_idx].freq_hz);

    printf("Operating points:\n");
    for (uint32_t i = 0; i < op_info.count; i++) {
      printf("%u Hz\n", op_info.opp[i].freq_hz);
    }
  } else {
    uint32_t freq;
    if (zx_status_t status = read_argument_checked(value, &freq); status != ZX_OK) {
      fprintf(stderr, "invalid frequency argument: %s\n", value);
      return status;
    }

    uint32_t op_idx;
    for (op_idx = 0; op_idx < op_info.count; op_idx++) {
      if (op_info.opp[op_idx].freq_hz == freq) {
        break;
      }
    }

    if (op_idx >= op_info.count) {
      fprintf(stderr, "No operating point found for %u Hz\n", freq);
      fprintf(stderr, "Operating points:\n");
      for (uint32_t i = 0; i < op_info.count; i++) {
        fprintf(stderr, "%u Hz\n", op_info.opp[i].freq_hz);
      }
      return ZX_ERR_NOT_FOUND;
    }

    const fidl::WireResult result =
        device_->SetDvfsOperatingPoint(static_cast<uint16_t>(op_idx), cluster);
    if (!result.ok()) {
      fprintf(stderr, "SetDvfsOperatingPoint failed: %s\n", result.status_string());
      return result.status();
    }
    const fidl::WireResponse response = result.value();
    if (zx_status_t status = response.status; status != ZX_OK) {
      fprintf(stderr, "SetDvfsOperatingPoint failed: %s\n", zx_status_get_string(status));
      return status;
    }
  }

  return ZX_OK;
}

zx::result<std::string> ThermalCli::GetSensorName() {
  const fidl::WireResult result = device_->GetSensorName();
  if (!result.ok()) {
    fprintf(stderr, "DeviceGetSensorName failed: %s\n", result.status_string());
    return zx::error(result.status());
  }
  return zx::ok(std::string(result->name.data(), result->name.size()));
}
