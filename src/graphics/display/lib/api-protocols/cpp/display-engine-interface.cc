// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"

#include <zircon/assert.h>

#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"

namespace display {

zx::result<> DisplayEngineInterface::SetDisplayPowerMode(display::DisplayId display_id,
                                                         display::PowerMode power_mode) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> DisplayEngineInterface::StartCapture(display::DriverCaptureImageId capture_image_id) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> DisplayEngineInterface::ReleaseCapture(
    display::DriverCaptureImageId capture_image_id) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> DisplayEngineInterface::SetMinimumRgb(uint8_t minimum_rgb) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

display::ConfigCheckResult DisplayEngineInterface::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    std::span<const display::DriverLayer> layers) {
  ZX_PANIC("DisplayEngineInterface subclasses must override one CheckConfiguration() overload.");
  return display::ConfigCheckResult::kUnsupportedConfig;
}

display::ConfigCheckResult DisplayEngineInterface::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers) {
  if (color_conversion != display::ColorConversion::kIdentity) {
    return display::ConfigCheckResult::kUnsupportedConfig;
  }
  return CheckConfiguration(display_id, display_mode_id, layers);
}

void DisplayEngineInterface::SubmitConfiguration(display::DisplayId display_id,
                                                 display::ModeId display_mode_id,
                                                 std::span<const display::DriverLayer> layers,
                                                 display::DriverConfigStamp driver_config_stamp) {
  ZX_PANIC("DisplayEngineInterface subclasses must override a SubmitConfiguration() overload");
}

void DisplayEngineInterface::SubmitConfiguration(display::DisplayId display_id,
                                                 display::ModeId display_mode_id,
                                                 display::ColorConversion color_conversion,
                                                 std::span<const display::DriverLayer> layers,
                                                 display::DriverConfigStamp driver_config_stamp) {
  ZX_DEBUG_ASSERT_MSG(color_conversion == display::ColorConversion::kIdentity,
                      "Display coordinator submitted rejected ColorConversion");
  SubmitConfiguration(display_id, display_mode_id, layers, driver_config_stamp);
}

}  // namespace display
