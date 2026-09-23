// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_EVENTS_INTERFACE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_EVENTS_INTERFACE_H_

#include <lib/zx/time.h>

#include <cstdint>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display {

// Equivalent to the FIDL protocol [`fuchsia.hardware.display.engine/EngineListener`].
class DisplayEngineEventsInterface {
 public:
  DisplayEngineEventsInterface() = default;

  DisplayEngineEventsInterface(const DisplayEngineEventsInterface&) = delete;
  DisplayEngineEventsInterface(DisplayEngineEventsInterface&&) = delete;
  DisplayEngineEventsInterface& operator=(const DisplayEngineEventsInterface&) = delete;
  DisplayEngineEventsInterface& operator=(DisplayEngineEventsInterface&&) = delete;

  virtual void OnDisplayAdded(display::DisplayId display_id,
                              std::span<const display::ModeAndId> preferred_modes,
                              std::span<const display::PixelFormat> pixel_formats) = 0;
  virtual void OnDisplayRemoved(display::DisplayId display_id) = 0;
  virtual void OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                              display::DriverConfigStamp config_stamp) = 0;

  // OOT drivers must not use the capture API.
  // The interface is not stabilized and will change.
  virtual void OnCaptureComplete() = 0;

 protected:
  // Destruction via base class pointer is not supported intentionally.
  // Instances are not expected to be owned by pointers to base classes.
  ~DisplayEngineEventsInterface() = default;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_EVENTS_INTERFACE_H_
