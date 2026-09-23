// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_NTC_H_
#define SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_NTC_H_

#include <zircon/assert.h>
#include <zircon/types.h>

#include <algorithm>
#include <vector>

#include "linear_lookup_table.h"

namespace thermal {

static constexpr uint32_t kMaxProfileLen = 200;
static constexpr uint32_t kMaxNameLen = 50;

// TODO(b/448631407): Replace with a FIDL metadata type.
struct NtcTable {
  float temperature_c;
  uint32_t resistance_ohm;
};

// TODO(b/448631407): Replace with a FIDL metadata type.
struct NtcChannel {
  uint32_t adc_channel;
  uint32_t pullup_ohms;
  uint32_t profile_idx;
  char name[kMaxNameLen];
};

struct NtcInfo {
  char part[kMaxNameLen];
  NtcTable profile[kMaxProfileLen];  // profile table should be sorted in decreasing resistance
};

class Ntc : public linear_lookup_table::LinearLookupTable<float, uint32_t> {
 public:
  Ntc(NtcInfo ntc_info, uint32_t pullup_ohms)
      : linear_lookup_table::LinearLookupTable<float, uint32_t>([&ntc_info]() {
          std::vector profile(ntc_info.profile, ntc_info.profile + kMaxProfileLen);
          // Since all entries in profile table may not have been used (passed as metadata) check to
          // make sure resistance isn't out of range of the table.
          std::erase_if(profile, [](NtcTable const& table) {
            return table.resistance_ohm == kInvalidResistance;
          });

          std::vector<linear_lookup_table::LookupTableEntry<float, uint32_t>> table;
          std::for_each(profile.begin(), profile.end(), [&table](NtcTable entry) {
            return table.push_back({entry.temperature_c, entry.resistance_ohm});
          });
          return table;
        }()),
        pullup_ohms_(pullup_ohms) {}
  // we use a normalized sample [0-1] to prevent having to worry about adc resolution
  //  in this library. This assumes the call site will normalize the value appropriately
  // Since the thermistor is in series with a pullup resistor, we must convert our sample
  //  value to a resistance then lookup in the profile table.
  zx_status_t GetTemperatureCelsius(float norm_sample, float* out) const {
    // norm_sample should never be 1.0 because that would mean there is no pullup resistor. Also,
    // this ensures that division below is valid.
    if ((norm_sample < 0) || (norm_sample >= 1.0)) {
      return ZX_ERR_INVALID_ARGS;
    }
    float ratio = -(norm_sample) / (norm_sample - 1);
    float resistance_f = ratio * static_cast<float>(pullup_ohms_);
    zx::result temperature = LookupX(resistance_f);
    if (temperature.is_error()) {
      return temperature.error_value();
    }
    *out = *temperature;
    return ZX_OK;
  }

  // Returns the normalized sample. Convert from resistance.
  zx_status_t GetNormalizedSample(float temperature_c, float* out) const {
    zx::result resistance = LookupY(temperature_c);
    if (resistance.is_error()) {
      return resistance.error_value();
    }
    *out = *resistance / (*resistance + static_cast<float>(pullup_ohms_));
    return ZX_OK;
  }

 private:
  static constexpr uint32_t kInvalidResistance = 0;
  uint32_t pullup_ohms_ = 0;
};
}  // namespace thermal

#endif  // SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_NTC_H_
