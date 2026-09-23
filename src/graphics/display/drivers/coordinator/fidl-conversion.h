// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_FIDL_CONVERSION_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_FIDL_CONVERSION_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <lib/fdf/cpp/arena.h>

#include <span>

#include "src/graphics/display/drivers/coordinator/driver-display-config.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"

namespace display_coordinator {

// Converts a `DriverDisplayConfig` and a list of `DriverLayer`s to a FIDL
// `fuchsia_hardware_display_engine::wire::DisplayConfig`.
//
// This function allocates memory for the layers from the provided arena.
fuchsia_hardware_display_engine::wire::DisplayConfig ToFidlDisplayConfig(
    const DriverDisplayConfig& driver_display_config, std::span<const display::DriverLayer> layers,
    fdf::Arena& arena);

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_FIDL_CONVERSION_H_
