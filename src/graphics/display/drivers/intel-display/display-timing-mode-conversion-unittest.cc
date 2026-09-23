// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/display-timing-mode-conversion.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"

namespace intel_display {

TEST(ToDisplayModeTest, DmtTiming) {
  static constexpr display::DisplayTiming kDmtDisplayPanelTimings = {
      .horizontal_active_px = 1152,
      .horizontal_front_porch_px = 64,
      .horizontal_sync_width_px = 128,
      .horizontal_back_porch_px = 256,
      .vertical_active_lines = 864,
      .vertical_front_porch_lines = 1,
      .vertical_sync_width_lines = 3,
      .vertical_back_porch_lines = 32,
      .pixel_clock_frequency_hz = 108'000'000,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kPositive,
      .vsync_polarity = display::SyncPolarity::kPositive,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_EQ(kDmtDisplayPanelTimings.vertical_field_refresh_rate_millihertz(), 75'000);

  display::Mode mode = ToDisplayMode(kDmtDisplayPanelTimings);
  EXPECT_EQ(mode.active_area().width(), 1152);
  EXPECT_EQ(mode.active_area().height(), 864);
  EXPECT_EQ(mode.refresh_rate_millihertz(), 75'000);
}

}  // namespace intel_display
