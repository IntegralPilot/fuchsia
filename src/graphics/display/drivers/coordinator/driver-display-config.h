// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_DRIVER_DISPLAY_CONFIG_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_DRIVER_DISPLAY_CONFIG_H_

#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace display_coordinator {

// Represents the display configuration parameters in the FIDL
// [`fuchsia.hardware.display.engine/DisplayConfig`] struct, minus
// the actual layers in the display config.
struct DriverDisplayConfig {
  display::DisplayId display_id = display::kInvalidDisplayId;
  display::ModeId mode_id = display::kInvalidModeId;
  display::ColorConversion color_conversion = display::ColorConversion::kIdentity;
  int layer_count = 0;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_DRIVER_DISPLAY_CONFIG_H_
