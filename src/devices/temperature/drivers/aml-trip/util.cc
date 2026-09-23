// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "util.h"

#include <cmath>

namespace temperature::tsensor_util {

namespace {
// Thermal calibration magic numbers from uboot.
constexpr int32_t kCalA_ = 324;
constexpr int32_t kCalB_ = 424;
constexpr int32_t kCalC_ = 3159;
constexpr int32_t kCalD_ = 9411;

constexpr uint32_t kAmlTsTempMask = 0xfff;
constexpr uint32_t kAmlTempCal = 1;

}  // namespace

// Calculate a temperature value from a temperature code.
// The unit of the temperature is degree Celsius.
TemperatureCelsius CodeToTempCelsius(uint32_t temp_code, const uint32_t trim_info) {
  uint32_t sensor_temp = temp_code;
  uint32_t uefuse = trim_info & 0xffff;

  // Referred u-boot code for below magic calculations.
  // T = 727.8*(u_real+u_efuse/(1<<16)) - 274.7
  // u_readl = (5.05*YOUT)/((1<<16)+ 4.05*YOUT)
  sensor_temp =
      ((sensor_temp * kCalB_) / 100 * (1 << 16) / (1 * (1 << 16) + kCalA_ * sensor_temp / 100));
  if (uefuse & 0x8000) {
    sensor_temp = ((sensor_temp - (uefuse & (0x7fff))) * kCalD_ / (1 << 16) - kCalC_);
  } else {
    sensor_temp = ((sensor_temp + uefuse) * kCalD_ / (1 << 16) - kCalC_);
  }
  return static_cast<TemperatureCelsius>(sensor_temp) / 10.0f;
}

// Tsensor treats temperature as a mapped temperature code.
// The temperature is converted differently depending on the calibration type.
uint32_t TempCelsiusToCode(TemperatureCelsius temp_c, bool trend, const uint32_t trim_info) {
  int32_t temp_decicelsius = static_cast<int32_t>(std::round(temp_c * 10.0f));
  int64_t sensor_code;
  uint32_t reg_code;
  uint32_t uefuse = trim_info & 0xffff;

  // Referred u-boot code for below magic calculations.
  // T = 727.8*(u_real+u_efuse/(1<<16)) - 274.7
  // u_readl = (5.05*YOUT)/((1<<16)+ 4.05*YOUT)
  // u_readl = (T + 274.7) / 727.8 - u_efuse / (1 << 16)
  // Yout =  (u_readl / (5.05 - 4.05u_readl)) *(1 << 16)
  if (uefuse & 0x8000) {
    sensor_code = ((1 << 16) * (temp_decicelsius + kCalC_) / kCalD_ +
                   (1 << 16) * (uefuse & 0x7fff) / (1 << 16));
  } else {
    sensor_code = ((1 << 16) * (temp_decicelsius + kCalC_) / kCalD_ -
                   (1 << 16) * (uefuse & 0x7fff) / (1 << 16));
  }

  sensor_code = (sensor_code * 100 / (kCalB_ - kCalA_ * sensor_code / (1 << 16)));
  if (trend) {
    reg_code = static_cast<uint32_t>((sensor_code >> 0x4) & kAmlTsTempMask) + kAmlTempCal;
  } else {
    reg_code = ((sensor_code >> 0x4) & kAmlTsTempMask);
  }
  return reg_code;
}

}  // namespace temperature::tsensor_util
