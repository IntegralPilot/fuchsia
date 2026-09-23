// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/edid/timings.h"

#include <array>
#include <cstddef>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/edid/cta-timing.h"
#include "src/graphics/display/lib/edid/dmt-timing.h"

namespace edid::internal {

constexpr std::array<display::DisplayTiming, kDmtTimings.size()> kDmtDisplayTimingsArray = [] {
  std::array<display::DisplayTiming, kDmtTimings.size()> params = {};
  for (size_t i = 0; i < kDmtTimings.size(); i++) {
    params[i] = ToDisplayTiming(kDmtTimings[i]);
  }
  return params;
}();

const std::span<const display::DisplayTiming> kDmtDisplayTimings(kDmtDisplayTimingsArray);

constexpr std::array<display::DisplayTiming, kCtaTimings.size()> kCtaDisplayTimingsArray = [] {
  std::array<display::DisplayTiming, kCtaTimings.size()> params = {};
  for (size_t i = 0; i < kCtaTimings.size(); i++) {
    params[i] = ToDisplayTiming(kCtaTimings[i]);
  }
  return params;
}();

const std::span<const display::DisplayTiming> kCtaDisplayTimings(kCtaDisplayTimingsArray);

}  // namespace edid::internal
