// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_DEVICE_CONFIG_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_DEVICE_CONFIG_H_

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"

namespace fake_display {

struct FakeDisplayDeviceConfig {
  // The driver reports one display with the ID below.
  display::DisplayId display_id = display::DisplayId(1);

  // The driver reports one display mode with the ID below.
  display::ModeId display_mode_id = display::ModeId(1);

  // The emulated display's mode.
  display::Mode display_mode = display::Mode(
      {.active_width = 1280, .active_height = 800, .refresh_rate_millihertz = 60'000});

  // The driver reports the capabilities below.
  //
  // If `supports_capture` is true, the driver emulates display capture via
  // software compositing.  To make this work, the driver adds BufferCollection
  // constraints requesting CPU access to image data during Sysmem negotiation.
  display::EngineInfo engine_info;

  // Enables periodically-generated VSync events.
  //
  // By default, this member is false. Tests must call `FakeDisplay::TriggerVsync()`
  // explicitly to get VSync events.
  //
  // If set to true, the `FakeDisplay` implementation will periodically generate
  // VSync events. These periodically-generated VSync events are a source of
  // non-determinism. They can lead to flaky tests, when coupled with overly
  // strict assertions around event timing.
  bool periodic_vsync = false;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_DEVICE_CONFIG_H_
