// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-dsi/dpi-interface-config.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/designware-dsi/dpi-video-timing.h"

namespace designware_dsi {

namespace {

TEST(DpiLowPowerCommandTimerConfigTest, Valid) {
  static constexpr DpiLowPowerCommandTimerConfig kMaxDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 255,
      .max_vertical_active_escape_mode_command_size_bytes = 255,
  };
  EXPECT_TRUE(kMaxDpiLowPowerCommandTimerConfig.IsValid());

  static constexpr DpiLowPowerCommandTimerConfig kMinDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 0,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };
  EXPECT_TRUE(kMinDpiLowPowerCommandTimerConfig.IsValid());
}

TEST(DpiLowPowerCommandTimerConfigTest,
     InvalidIfMaxVerticalBlankEscapeModeCommandSizeBytesTooLarge) {
  static constexpr DpiLowPowerCommandTimerConfig kInvalidDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 256,
      .max_vertical_active_escape_mode_command_size_bytes = 255,
  };
  EXPECT_GT(kInvalidDpiLowPowerCommandTimerConfig.max_vertical_blank_escape_mode_command_size_bytes,
            kMaxPossibleMaxVerticalBlankEscapeModeCommandSizeBytes);
  EXPECT_FALSE(kInvalidDpiLowPowerCommandTimerConfig.IsValid());
}

TEST(DpiLowPowerCommandTimerConfigTest,
     InvalidIfMaxVerticalBlankEscapeModeCommandSizeBytesTooSmall) {
  static constexpr DpiLowPowerCommandTimerConfig kInvalidDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = -1,
      .max_vertical_active_escape_mode_command_size_bytes = 255,
  };
  EXPECT_LT(kInvalidDpiLowPowerCommandTimerConfig.max_vertical_blank_escape_mode_command_size_bytes,
            0);
  EXPECT_FALSE(kInvalidDpiLowPowerCommandTimerConfig.IsValid());
}

TEST(DpiLowPowerCommandTimerConfigTest,
     InvalidIfMaxVerticalActiveEscapeModeCommandSizeBytesTooLarge) {
  static constexpr DpiLowPowerCommandTimerConfig kInvalidDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 255,
      .max_vertical_active_escape_mode_command_size_bytes = 256,
  };
  EXPECT_GT(
      kInvalidDpiLowPowerCommandTimerConfig.max_vertical_active_escape_mode_command_size_bytes,
      kMaxPossibleMaxVerticalActiveEscapeModeCommandSizeBytes);
  EXPECT_FALSE(kInvalidDpiLowPowerCommandTimerConfig.IsValid());
}

TEST(DpiLowPowerCommandTimerConfigTest,
     InvalidIfMaxVerticalActiveEscapeModeCommandSizeBytesTooSmall) {
  static constexpr DpiLowPowerCommandTimerConfig kInvalidDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 255,
      .max_vertical_active_escape_mode_command_size_bytes = -1,
  };
  EXPECT_LT(
      kInvalidDpiLowPowerCommandTimerConfig.max_vertical_active_escape_mode_command_size_bytes, 0);
  EXPECT_FALSE(kInvalidDpiLowPowerCommandTimerConfig.IsValid());
}

TEST(DpiInterfaceConfigTest, Valid) {
  static constexpr DpiColorComponentMapping kColorComponentMapping =
      DpiColorComponentMapping::k24BitR8G8B8;
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 61682040,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  static constexpr DpiLowPowerCommandTimerConfig kDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 128,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;

  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
  EXPECT_TRUE(kDpiLowPowerCommandTimerConfig.IsValid());

  static constexpr DpiInterfaceConfig kDpiInterfaceConfig = {
      .color_component_mapping = kColorComponentMapping,
      .video_timing = kTiming,
      .low_power_command_timer_config = kDpiLowPowerCommandTimerConfig,
  };
  EXPECT_TRUE(kDpiInterfaceConfig.IsValid(kDphyDataLaneBytesPerSecond));
}

TEST(DpiInterfaceConfigTest, InvalidIfDisplayTimingIsInvalid) {
  static constexpr DpiColorComponentMapping kColorComponentMapping =
      DpiColorComponentMapping::k24BitR8G8B8;
  static constexpr display::DisplayTiming kInvalidTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = -1,  // Invalid
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 61682040,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  static constexpr DpiLowPowerCommandTimerConfig kDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 128,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;

  EXPECT_FALSE(IsValidDpiVideoTiming(kInvalidTiming, kDphyDataLaneBytesPerSecond));
  EXPECT_TRUE(kDpiLowPowerCommandTimerConfig.IsValid());

  static constexpr DpiInterfaceConfig kDpiInterfaceConfig = {
      .color_component_mapping = kColorComponentMapping,
      .video_timing = kInvalidTiming,
      .low_power_command_timer_config = kDpiLowPowerCommandTimerConfig,
  };
  EXPECT_FALSE(kDpiInterfaceConfig.IsValid(kDphyDataLaneBytesPerSecond));
}

TEST(DpiInterfaceConfigTest, InvalidIfLowPowerCommandTimerConfigIsInvalid) {
  static constexpr DpiColorComponentMapping kColorComponentMapping =
      DpiColorComponentMapping::k24BitR8G8B8;
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 61682040,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  static constexpr DpiLowPowerCommandTimerConfig kInvalidLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 256,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;

  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
  EXPECT_FALSE(kInvalidLowPowerCommandTimerConfig.IsValid());

  static constexpr DpiInterfaceConfig kDpiInterfaceConfig = {
      .color_component_mapping = kColorComponentMapping,
      .video_timing = kTiming,
      .low_power_command_timer_config = kInvalidLowPowerCommandTimerConfig,
  };
  EXPECT_FALSE(kDpiInterfaceConfig.IsValid(kDphyDataLaneBytesPerSecond));
}

}  // namespace

}  // namespace designware_dsi
