// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/display-timing-mode-conversion.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"

namespace amlogic_display {

TEST(ToDisplayModeTest, AstroDisplayTiming) {
  static constexpr display::DisplayTiming kAstroDisplayPanelTimings = {
      .horizontal_active_px = 600,
      .horizontal_front_porch_px = 40,
      .horizontal_sync_width_px = 24,
      .horizontal_back_porch_px = 36,
      .vertical_active_lines = 1024,
      .vertical_front_porch_lines = 19,
      .vertical_sync_width_lines = 2,
      .vertical_back_porch_lines = 8,
      .pixel_clock_frequency_hz = 44'226'000,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_EQ(kAstroDisplayPanelTimings.vertical_field_refresh_rate_millihertz(), 60'000);

  display::Mode mode = ToDisplayMode(kAstroDisplayPanelTimings);
  EXPECT_EQ(mode.active_area().width(), 600);
  EXPECT_EQ(mode.active_area().height(), 1024);
  EXPECT_EQ(mode.refresh_rate_millihertz(), 60'000);
}

TEST(ToDisplayModeTest, SherlockDisplayTiming) {
  static constexpr display::DisplayTiming kSherlockDisplayPanelTimings = {
      .horizontal_active_px = 800,
      .horizontal_front_porch_px = 20,
      .horizontal_sync_width_px = 20,
      .horizontal_back_porch_px = 50,
      .vertical_active_lines = 1280,
      .vertical_front_porch_lines = 20,
      .vertical_sync_width_lines = 4,
      .vertical_back_porch_lines = 20,
      .pixel_clock_frequency_hz = 70'702'000,  // refresh rate: 60 Hz
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_EQ(kSherlockDisplayPanelTimings.vertical_field_refresh_rate_millihertz(), 60'000);

  display::Mode mode = ToDisplayMode(kSherlockDisplayPanelTimings);
  EXPECT_EQ(mode.active_area().width(), 800);
  EXPECT_EQ(mode.active_area().height(), 1280);
  EXPECT_EQ(mode.refresh_rate_millihertz(), 60'000);
}

TEST(ToDisplayModeTest, NelsonDisplayTiming) {
  static constexpr display::DisplayTiming kNelsonDisplayPanelTimings = {
      .horizontal_active_px = 600,
      .horizontal_front_porch_px = 80,
      .horizontal_sync_width_px = 10,
      .horizontal_back_porch_px = 80,
      .vertical_active_lines = 1024,
      .vertical_front_porch_lines = 20,
      .vertical_sync_width_lines = 6,
      .vertical_back_porch_lines = 20,
      .pixel_clock_frequency_hz = 49'434'000,  // refresh rate: 60 Hz
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_EQ(kNelsonDisplayPanelTimings.vertical_field_refresh_rate_millihertz(), 60'000);

  display::Mode mode = ToDisplayMode(kNelsonDisplayPanelTimings);
  EXPECT_EQ(mode.active_area().width(), 600);
  EXPECT_EQ(mode.active_area().height(), 1024);
  EXPECT_EQ(mode.refresh_rate_millihertz(), 60'000);
}

}  // namespace amlogic_display
