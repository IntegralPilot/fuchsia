// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TEMPERATURE_DRIVERS_AML_TRIP_UTIL_H_
#define SRC_DEVICES_TEMPERATURE_DRIVERS_AML_TRIP_UTIL_H_

#include <stdint.h>

namespace temperature {

using TemperatureCelsius = float;

namespace tsensor_util {

TemperatureCelsius CodeToTempCelsius(uint32_t temp_code, uint32_t trim_info);
uint32_t TempCelsiusToCode(TemperatureCelsius temp_c, bool trend, uint32_t trim_info);

}  // namespace tsensor_util
}  // namespace temperature

#endif  // SRC_DEVICES_TEMPERATURE_DRIVERS_AML_TRIP_UTIL_H_
