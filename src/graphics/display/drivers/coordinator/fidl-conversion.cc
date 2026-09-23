// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/fidl-conversion.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <lib/fdf/cpp/arena.h>

#include "src/graphics/display/drivers/coordinator/driver-display-config.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace display_coordinator {

fuchsia_hardware_display_engine::wire::DisplayConfig ToFidlDisplayConfig(
    const DriverDisplayConfig& driver_display_config, std::span<const display::DriverLayer> layers,
    fdf::Arena& arena) {
  ZX_DEBUG_ASSERT(static_cast<size_t>(driver_display_config.layer_count) == layers.size());
  fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer> fidl_layers(arena, layers.size());
  for (size_t i = 0; i < layers.size(); ++i) {
    fidl_layers[i] = layers[i].ToFidl();
  }

  return fuchsia_hardware_display_engine::wire::DisplayConfig{
      .display_id = driver_display_config.display_id.ToFidl(),
      .mode_id = driver_display_config.mode_id.ToFidl(),
      .color_conversion = driver_display_config.color_conversion.ToFidl(),
      .layers = fidl_layers,
  };
}

}  // namespace display_coordinator
