// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-dsi/dpi-video-timing.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace designware_dsi {

namespace {

TEST(DpiVideoTimingTest, Valid) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      // Calculated so that the frame rate is 30 fps.
      .pixel_clock_frequency_hz = 61682040,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_TRUE(kTiming.IsValid());

  static constexpr int64_t kValidDphyDataLaneBytesPerSecondMin = 61682040;
  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecondMin));

  static constexpr int64_t kValidDphyDataLaneBytesPerSecond2 = int64_t{61682040} * 2;
  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecond2));
}

TEST(DpiVideoTimingTest, ValidWhenDataLaneBytesPerSecondIsMaximum) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x044,
      .horizontal_front_porch_px = 0x003,
      .horizontal_sync_width_px = 0x002,
      .horizontal_back_porch_px = 0x001,
      .vertical_active_lines = 0x055,
      .vertical_front_porch_lines = 0x006,
      .vertical_sync_width_lines = 0x007,
      .vertical_back_porch_lines = 0x008,
      // Calculated so that the frame rate is 60 fps.
      .pixel_clock_frequency_hz = 470640,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  static constexpr int64_t kValidDphyDataLaneBytesPerSecondMax =
      kTiming.pixel_clock_frequency_hz * 256;
  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecondMax));
}

TEST(InvalidDpiVideoTimingTest, DisplayTimingIsNotValid) {
  static constexpr display::DisplayTiming kTiming = {
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

  EXPECT_FALSE(kTiming.IsValid());

  static constexpr int64_t kValidDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, InterlacedFrame) {
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
      .fields_per_frame = display::FieldsPerFrame::kInterlaced,  // Unsupported
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_TRUE(kTiming.IsValid());

  static constexpr int64_t kValidDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, PixelRepeated) {
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
      .pixel_repetition = 1,  // Unsupported
  };

  EXPECT_TRUE(kTiming.IsValid());

  static constexpr int64_t kValidDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, PixelClockFrequencyNotDivisibleByDataLaneBytesPerSecond) {
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

  // `kTiming.pixel_clock_frequency_hz` is not divisible by
  // `kDphyDataLaneBytesPerSecond`.
  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682041;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, ClockFrequencyRatioLowerThanMinimum) {
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

  static constexpr int64_t kDphyDataLaneBytesPerSecondOverlyLow = int64_t{61682040} * -1;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecondOverlyLow));
}

TEST(InvalidDpiVideoTimingTest, ClockFrequencyRatioExceedsMaximum) {
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

  static constexpr int64_t kDphyDataLaneBytesPerSecondOverlyHigh = int64_t{61682040} * 257;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecondOverlyHigh));
}

TEST(InvalidDpiVideoTimingTest, HorizontalBackPorchExceedsLimit) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x1000,  // Exceeds the size limit
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

  EXPECT_GT(kTiming.horizontal_back_porch_px, kMaxHorizontalBackPorchPx);

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalSyncWidthExceedsLimit) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x1000,  // Exceeds the size limit
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

  EXPECT_GT(kTiming.horizontal_sync_width_px, kMaxHorizontalSyncWidthPx);

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalTotalExceedsLimit) {
  static constexpr display::DisplayTiming kTiming = {
      // Picked so that horizontal_total_px() equals to 0x8000.
      .horizontal_active_px = 0x7f9a,
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

  // Exceeds the size limit of horizontal_total_px().
  EXPECT_EQ(kTiming.horizontal_total_px(), 0x8000);
  EXPECT_GT(kTiming.horizontal_total_px(), kMaxHorizontalTotalPx);

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, VerticalActiveExceedsLimit) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x4000,  // Exceeds the limit
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

  EXPECT_GT(kTiming.vertical_active_lines, kMaxVerticalActiveLines);

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, VerticalFrontPorchExceedsLimit) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x400,  // Exceeds the limit
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 61682040,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_GT(kTiming.vertical_front_porch_lines, kMaxVerticalFrontPorchLines);

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, VerticalSyncWidthExceedsLimit) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x400,  // Exceeds the limit
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 61682040,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_GT(kTiming.vertical_sync_width_lines, kMaxVerticalSyncWidthLines);

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, VerticalBackPorchExceedsLimit) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x400,  // Exceeds the limit
      .pixel_clock_frequency_hz = 61682040,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };

  EXPECT_GT(kTiming.vertical_back_porch_lines, kMaxVerticalBackPorchLines);

  static constexpr int64_t kDphyDataLaneBytesPerSecond = 61682040;
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalSyncTimeLaneByteClockCyclesDoesNotExceedMaximum) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x400,
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

  static constexpr int64_t kValidDphyDataLaneBytesPerSecond = kTiming.pixel_clock_frequency_hz * 3;
  const int32_t horizontal_sync_time_lane_byte_clock_cycles = DpiPixelToDphyLaneByteClockCycle(
      kTiming.horizontal_sync_width_px, kTiming.pixel_clock_frequency_hz,
      kValidDphyDataLaneBytesPerSecond);
  EXPECT_LE(horizontal_sync_time_lane_byte_clock_cycles, kMaxHorizontalSyncTimeLaneByteClockCycles);
  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalSyncTimeLaneByteClockCyclesExceedsMaximum) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x400,
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

  static constexpr int64_t kInvalidDphyDataLaneBytesPerSecond =
      kTiming.pixel_clock_frequency_hz * 4;
  const int32_t horizontal_sync_time_lane_byte_clock_cycles = DpiPixelToDphyLaneByteClockCycle(
      kTiming.horizontal_sync_width_px, kTiming.pixel_clock_frequency_hz,
      kInvalidDphyDataLaneBytesPerSecond);
  EXPECT_GT(horizontal_sync_time_lane_byte_clock_cycles, kMaxHorizontalSyncTimeLaneByteClockCycles);
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kInvalidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalBackPorchTimeLaneByteClockCyclesDoesNotExceedMaximum) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x400,
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

  static constexpr int64_t kValidDphyDataLaneBytesPerSecond = kTiming.pixel_clock_frequency_hz * 3;
  const int32_t horizontal_back_porch_time_lane_byte_clock_cycles =
      DpiPixelToDphyLaneByteClockCycle(kTiming.horizontal_back_porch_px,
                                       kTiming.pixel_clock_frequency_hz,
                                       kValidDphyDataLaneBytesPerSecond);
  EXPECT_LE(horizontal_back_porch_time_lane_byte_clock_cycles,
            kMaxHorizontalBackPorchTimeLaneByteClockCycles);
  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalBackPorchTimeLaneByteClockCyclesExceedsMaximum) {
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x400,
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

  static constexpr int64_t kInvalidDphyDataLaneBytesPerSecond =
      kTiming.pixel_clock_frequency_hz * 4;
  const int32_t horizontal_back_porch_time_lane_byte_clock_cycles =
      DpiPixelToDphyLaneByteClockCycle(kTiming.horizontal_back_porch_px,
                                       kTiming.pixel_clock_frequency_hz,
                                       kInvalidDphyDataLaneBytesPerSecond);
  EXPECT_GT(horizontal_back_porch_time_lane_byte_clock_cycles,
            kMaxHorizontalBackPorchTimeLaneByteClockCycles);
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kInvalidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalTotalTimeLaneByteClockCyclesDoesNotExceedMaximum) {
  static constexpr display::DisplayTiming kTiming = {
      // Picked so that horizontal_total_px() equals to 0x1000.
      .horizontal_active_px = 0xf9a,
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

  EXPECT_EQ(kTiming.horizontal_total_px(), 0x1000);

  static constexpr int64_t kValidDphyDataLaneBytesPerSecond = kTiming.pixel_clock_frequency_hz * 7;
  const int32_t horizontal_total_time_lane_byte_clock_cycles = DpiPixelToDphyLaneByteClockCycle(
      kTiming.horizontal_total_px(), kTiming.pixel_clock_frequency_hz,
      kValidDphyDataLaneBytesPerSecond);
  EXPECT_LE(horizontal_total_time_lane_byte_clock_cycles,
            kMaxHorizontalTotalTimeLaneByteClockCycles);
  EXPECT_TRUE(IsValidDpiVideoTiming(kTiming, kValidDphyDataLaneBytesPerSecond));
}

TEST(InvalidDpiVideoTimingTest, HorizontalTotalTimeLaneByteClockCyclesExceedsMaximum) {
  static constexpr display::DisplayTiming kTiming = {
      // Picked so that horizontal_total_px() equals to 0x1000.
      .horizontal_active_px = 0xf9a,
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

  EXPECT_EQ(kTiming.horizontal_total_px(), 0x1000);

  static constexpr int64_t kInvalidDphyDataLaneBytesPerSecond =
      kTiming.pixel_clock_frequency_hz * 8;
  const int32_t horizontal_total_time_lane_byte_clock_cycles = DpiPixelToDphyLaneByteClockCycle(
      kTiming.horizontal_total_px(), kTiming.pixel_clock_frequency_hz,
      kInvalidDphyDataLaneBytesPerSecond);
  EXPECT_GT(horizontal_total_time_lane_byte_clock_cycles,
            kMaxHorizontalTotalTimeLaneByteClockCycles);
  EXPECT_FALSE(IsValidDpiVideoTiming(kTiming, kInvalidDphyDataLaneBytesPerSecond));
}

}  // namespace

}  // namespace designware_dsi
