// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_DISPLAY_TIMING_MODE_CONVERSION_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_DISPLAY_TIMING_MODE_CONVERSION_H_

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"

namespace amlogic_display {

constexpr display::Mode ToDisplayMode(const display::DisplayTiming& timing) {
  int32_t active_width = timing.horizontal_active_px;
  int32_t active_height = timing.vertical_active_lines;
  int32_t refresh_rate_millihertz = timing.vertical_field_refresh_rate_millihertz();
  return display::Mode({
      .active_width = active_width,
      .active_height = active_height,
      .refresh_rate_millihertz = refresh_rate_millihertz,
  });
}

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_DISPLAY_TIMING_MODE_CONVERSION_H_
