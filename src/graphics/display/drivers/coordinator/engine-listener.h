// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ENGINE_LISTENER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ENGINE_LISTENER_H_

#include <lib/zx/time.h>

#include "src/graphics/display/drivers/coordinator/added-display-info.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator {

// Interface of display engine listener event callbacks.
//
// All functions must run on the same dispatcher.
//
// Designed to work with the [`fuchsia.hardware.display.engine/EngineListener`]
// FIDL adapter.
class EngineListener {
 public:
  virtual void OnDisplayAdded(std::unique_ptr<AddedDisplayInfo> added_display_info) = 0;
  virtual void OnDisplayRemoved(display::DisplayId display_id) = 0;
  virtual void OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                              display::DriverConfigStamp driver_config_stamp) = 0;
  virtual void OnCaptureComplete() = 0;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ENGINE_LISTENER_H_
