// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/edid/dmt-timing.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/edid/timings.h"

namespace edid::internal {

namespace {

TEST(DmtTimingInvariantTest, HsyncPolarityIsValid) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_TRUE(dmt_timing.horizontal_sync_polarity == display::SyncPolarity::kPositive ||
                dmt_timing.horizontal_sync_polarity == display::SyncPolarity::kNegative);
  }
}

TEST(DmtTimingInvariantTest, VsyncPolarityIsValid) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_TRUE(dmt_timing.vertical_sync_polarity == display::SyncPolarity::kPositive ||
                dmt_timing.vertical_sync_polarity == display::SyncPolarity::kNegative);
  }
}

TEST(DmtTimingInvariantTest, FieldsPerFrameIsValid) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_TRUE(dmt_timing.fields_per_frame == display::FieldsPerFrame::kInterlaced ||
                dmt_timing.fields_per_frame == display::FieldsPerFrame::kProgressive);
  }
}

TEST(DmtTimingInvariantTest, HorizontalBlankMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_EQ(dmt_timing.horizontal_blank_px, dmt_timing.horizontal_back_porch_px +
                                                  dmt_timing.horizontal_sync_width_px +
                                                  dmt_timing.horizontal_front_porch_px);
  }
}

TEST(DmtTimingInvariantTest, HorizontalBlankStartMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_EQ(dmt_timing.horizontal_blank_start_px,
              dmt_timing.horizontal_active_px + dmt_timing.horizontal_right_border_px);
  }
}

TEST(DmtTimingInvariantTest, HorizontalSyncStartMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_EQ(dmt_timing.horitontal_sync_start_px, dmt_timing.horizontal_active_px +
                                                       dmt_timing.horizontal_right_border_px +
                                                       dmt_timing.horizontal_front_porch_px);
  }
}

TEST(DmtTimingInvariantTest, HorizontalTotalMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_EQ(dmt_timing.horizontal_total_px,
              dmt_timing.horizontal_active_px + dmt_timing.horizontal_right_border_px +
                  dmt_timing.horizontal_left_border_px + dmt_timing.horizontal_blank_px);
  }
}

TEST(DmtTimingInvariantTest, VerticalActiveIsEvenForInterlacedModes) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    if (dmt_timing.fields_per_frame == display::FieldsPerFrame::kInterlaced) {
      EXPECT_EQ(dmt_timing.vertical_active_lines % 2, 0);
    }
  }
}

TEST(DmtTimingInvariantTest, VerticalBlankMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_EQ(dmt_timing.vertical_blank_lines, dmt_timing.vertical_back_porch_lines +
                                                   dmt_timing.vertical_sync_width_lines +
                                                   dmt_timing.vertical_front_porch_lines);
  }
}

TEST(DmtTimingInvariantTest, VerticalBlankStartMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_EQ(dmt_timing.vertical_blank_start_lines,
              dmt_timing.vertical_active_lines + dmt_timing.vertical_bottom_border_lines);
  }
}

TEST(DmtTimingInvariantTest, VerticalSyncStartMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    EXPECT_EQ(dmt_timing.vertical_sync_start_lines, dmt_timing.vertical_active_lines +
                                                        dmt_timing.vertical_bottom_border_lines +
                                                        dmt_timing.vertical_front_porch_lines);
  }
}

TEST(DmtTimingInvariantTest, VerticalTotalMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);
    switch (dmt_timing.fields_per_frame) {
      case display::FieldsPerFrame::kProgressive:
        EXPECT_EQ(dmt_timing.vertical_total_lines,
                  dmt_timing.vertical_active_lines + dmt_timing.vertical_bottom_border_lines +
                      dmt_timing.vertical_blank_lines + dmt_timing.vertical_top_border_lines);
        break;
      case display::FieldsPerFrame::kInterlaced:
        // For interlaced display modes, `vertical_total_lines` may equal to
        //  `vertical_active_lines + vertical_bottom_border_lines
        //   + vertical_blank_lines * 2 + vertical_top_border_lines`
        // if the vblank does not alternate or
        //   `vertical_active_lines + vertical_bottom_border_lines
        //    + vertical_blank_lines * 2 + vertical_top_border_lines + 1`
        // if the vblank alternates.
        EXPECT_GE(dmt_timing.vertical_total_lines,
                  dmt_timing.vertical_active_lines + dmt_timing.vertical_bottom_border_lines +
                      dmt_timing.vertical_blank_lines * 2 + dmt_timing.vertical_top_border_lines);
        EXPECT_LE(dmt_timing.vertical_total_lines, dmt_timing.vertical_active_lines +
                                                       dmt_timing.vertical_bottom_border_lines +
                                                       dmt_timing.vertical_blank_lines * 2 +
                                                       dmt_timing.vertical_top_border_lines + 1);
        break;
    }
  }
}

TEST(DmtTimingInvariantTest, VerticalFieldRefreshRateMatchesDefinition) {
  for (const DmtTiming& dmt_timing : kDmtTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at DMT ID " << std::hex << dmt_timing.id);

    // In the test below, we recompute the vertical field refresh rate using
    // other timing values (including pixel clock), and compare it with the
    // value documented in the standard.
    //
    // We also considered recomputing pixel clock using the documented vertical
    // field refresh rate and other values. However, this ended up adding more
    // complexity because the test needs to also account for the deviation
    // between the computed pixel clock and the documented pixel clock, which
    // is caused by loss of precision in the DMT standard's calculation of the
    // refresh rate.

    const int fields_per_frame =
        dmt_timing.fields_per_frame == display::FieldsPerFrame::kInterlaced ? 2 : 1;

    // The multiplication won't overflow and cause an undefined behavior.
    // `dmt_timing.pixel_clock_khz` is a 31-bit value and 1'000'000 < 2^20.
    // Thus the multiplication is less than 2^52 - 1.
    const int64_t pixel_clock_millihertz = int64_t{dmt_timing.pixel_clock_khz} * 1'000'000;

    // The multiplication won't overflow and cause an undefined behavior.
    // Both `dmt_timing.horizontal_total_px` and `dmt_timing.vertical_total_lines`
    // are at most 2^16 - 1, thus the multiplication is less than 2^32 - 1.
    const int64_t pixels_per_frame =
        int64_t{dmt_timing.horizontal_total_px} * dmt_timing.vertical_total_lines;

    // calculated_vertical_field_refresh_rate_millihertz
    //  = round(pixel_clock_millihertz * fields_per_frame / pixels_per_frame)
    //  = (pixel_clock_millihertz * fields_per_frame + pixels_per_frame / 2) div
    //    pixels_per_frame
    //
    // Since `pixel_clock_millihertz * fields_per_frame` < 2^53 - 1 and
    // `pixels_per_frame` < 2^32 - 1, the divisor, dividend and the quotient
    // won't overflow in int64_t.
    const int64_t calculated_vertical_field_refresh_rate_millihertz =
        (pixel_clock_millihertz * fields_per_frame + pixels_per_frame / 2) / pixels_per_frame;

    EXPECT_EQ(int64_t{dmt_timing.vertical_field_refresh_rate_millihertz},
              calculated_vertical_field_refresh_rate_millihertz);
  }
}

TEST(DmtTimingToDisplayTiming, InterlacedWithAlternatingVblank) {
  constexpr DmtTiming kDmtTiming = {
      .id = 0x99,

      // This field is computed to be consistent with the other fields,
      // which have synthetic values.
      .vertical_field_refresh_rate_millihertz = 61'049,
      .pixel_clock_khz = 1'000'000,

      .fields_per_frame = display::FieldsPerFrame::kInterlaced,
      .horizontal_sync_polarity = display::SyncPolarity::kPositive,
      .vertical_sync_polarity = display::SyncPolarity::kNegative,

      .horizontal_total_px = 0x10'10,
      .horizontal_active_px = 0x0a'0a,
      .horizontal_blank_start_px = 0x0a'0a,
      .horizontal_blank_px = 0x06'06,
      .horitontal_sync_start_px = 0x0b'0b,

      .horizontal_right_border_px = 0,
      .horizontal_front_porch_px = 0x01'01,
      .horizontal_sync_width_px = 0x02'02,
      .horizontal_back_porch_px = 0x03'03,
      .horizontal_left_border_px = 0,

      .vertical_total_lines = 0x1f'1f,
      .vertical_active_lines = 0x07'07,
      .vertical_blank_start_lines = 0x07'07,
      .vertical_blank_lines = 0x0c'0c,
      .vertical_sync_start_lines = 0x0a'0a,

      .vertical_bottom_border_lines = 0,
      .vertical_front_porch_lines = 0x03'03,
      .vertical_sync_width_lines = 0x04'04,
      .vertical_back_porch_lines = 0x05'05,
      .vertical_top_border_lines = 0,
  };

  const display::DisplayTiming kConverted = ToDisplayTiming(kDmtTiming);
  EXPECT_EQ(kConverted.horizontal_total_px(), 0x10'10);
  EXPECT_EQ(kConverted.horizontal_active_px, 0x0a'0a);
  EXPECT_EQ(kConverted.horizontal_blank_px(), 0x06'06);
  EXPECT_EQ(kConverted.horizontal_front_porch_px, 0x01'01);
  EXPECT_EQ(kConverted.horizontal_sync_width_px, 0x02'02);
  EXPECT_EQ(kConverted.horizontal_back_porch_px, 0x03'03);
  EXPECT_EQ(kConverted.vertical_total_lines(), 0x1f'1f);
  EXPECT_EQ(kConverted.vertical_active_lines, 0x07'07);
  EXPECT_EQ(kConverted.vertical_blank_lines(), 0x0c'0c);
  EXPECT_EQ(kConverted.vertical_front_porch_lines, 0x03'03);
  EXPECT_EQ(kConverted.vertical_sync_width_lines, 0x04'04);
  EXPECT_EQ(kConverted.vertical_back_porch_lines, 0x05'05);
  EXPECT_EQ(kConverted.pixel_clock_frequency_hz, 1'000'000'000);
  EXPECT_EQ(kConverted.fields_per_frame, display::FieldsPerFrame::kInterlaced);
  EXPECT_EQ(kConverted.hsync_polarity, display::SyncPolarity::kPositive);
  EXPECT_EQ(kConverted.vsync_polarity, display::SyncPolarity::kNegative);
  EXPECT_EQ(kConverted.vblank_alternates, false);
  EXPECT_EQ(kConverted.pixel_repetition, false);
  EXPECT_EQ(kConverted.vertical_field_refresh_rate_millihertz(), 61'049);
}

TEST(DmtTimingToDisplayTiming, InterlacedWithConstantVblank) {
  constexpr DmtTiming kDmtTiming = {
      .id = 0x99,

      // This field is computed to be consistent with the other fields,
      // which have synthetic values.
      .vertical_field_refresh_rate_millihertz = 61'042,
      .pixel_clock_khz = 1'000'000,

      .fields_per_frame = display::FieldsPerFrame::kInterlaced,
      .horizontal_sync_polarity = display::SyncPolarity::kPositive,
      .vertical_sync_polarity = display::SyncPolarity::kNegative,

      .horizontal_total_px = 0x10'10,
      .horizontal_active_px = 0x0a'0a,
      .horizontal_blank_start_px = 0x0a'0a,
      .horizontal_blank_px = 0x06'06,
      .horitontal_sync_start_px = 0x0b'0b,

      .horizontal_right_border_px = 0,
      .horizontal_front_porch_px = 0x01'01,
      .horizontal_sync_width_px = 0x02'02,
      .horizontal_back_porch_px = 0x03'03,
      .horizontal_left_border_px = 0,

      .vertical_total_lines = 0x1f'20,
      .vertical_active_lines = 0x07'07,
      .vertical_blank_start_lines = 0x07'07,
      .vertical_blank_lines = 0x0c'0c,
      .vertical_sync_start_lines = 0x0a'0a,

      .vertical_bottom_border_lines = 0,
      .vertical_front_porch_lines = 0x03'03,
      .vertical_sync_width_lines = 0x04'04,
      .vertical_back_porch_lines = 0x05'05,
      .vertical_top_border_lines = 0,
  };

  const display::DisplayTiming kConverted = ToDisplayTiming(kDmtTiming);
  EXPECT_EQ(kConverted.horizontal_total_px(), 0x10'10);
  EXPECT_EQ(kConverted.horizontal_active_px, 0x0a'0a);
  EXPECT_EQ(kConverted.horizontal_blank_px(), 0x06'06);
  EXPECT_EQ(kConverted.horizontal_front_porch_px, 0x01'01);
  EXPECT_EQ(kConverted.horizontal_sync_width_px, 0x02'02);
  EXPECT_EQ(kConverted.horizontal_back_porch_px, 0x03'03);
  EXPECT_EQ(kConverted.vertical_total_lines(), 0x1f'20);
  EXPECT_EQ(kConverted.vertical_active_lines, 0x07'07);
  EXPECT_EQ(kConverted.vertical_blank_lines(), 0x0c'0c);
  EXPECT_EQ(kConverted.vertical_front_porch_lines, 0x03'03);
  EXPECT_EQ(kConverted.vertical_sync_width_lines, 0x04'04);
  EXPECT_EQ(kConverted.vertical_back_porch_lines, 0x05'05);
  EXPECT_EQ(kConverted.pixel_clock_frequency_hz, 1'000'000'000);
  EXPECT_EQ(kConverted.fields_per_frame, display::FieldsPerFrame::kInterlaced);
  EXPECT_EQ(kConverted.hsync_polarity, display::SyncPolarity::kPositive);
  EXPECT_EQ(kConverted.vsync_polarity, display::SyncPolarity::kNegative);
  EXPECT_EQ(kConverted.vblank_alternates, true);
  EXPECT_EQ(kConverted.pixel_repetition, false);
  EXPECT_EQ(kConverted.vertical_field_refresh_rate_millihertz(), 61'042);
}

TEST(DmtTimingToDisplayTiming, Progressive) {
  constexpr DmtTiming kDmtTiming = {
      .id = 0x99,

      // This field is computed to be consistent with the other fields,
      // which have synthetic values.
      .vertical_field_refresh_rate_millihertz = 49'804,
      .pixel_clock_khz = 1'000'000,

      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .horizontal_sync_polarity = display::SyncPolarity::kPositive,
      .vertical_sync_polarity = display::SyncPolarity::kNegative,

      .horizontal_total_px = 0x10'10,
      .horizontal_active_px = 0x0a'0a,
      .horizontal_blank_start_px = 0x0a'0a,
      .horizontal_blank_px = 0x06'06,
      .horitontal_sync_start_px = 0x0b'0b,

      .horizontal_right_border_px = 0,
      .horizontal_front_porch_px = 0x01'01,
      .horizontal_sync_width_px = 0x02'02,
      .horizontal_back_porch_px = 0x03'03,
      .horizontal_left_border_px = 0,

      .vertical_total_lines = 0x13'13,
      .vertical_active_lines = 0x07'07,
      .vertical_blank_start_lines = 0x07'07,
      .vertical_blank_lines = 0x0c'0c,
      .vertical_sync_start_lines = 0x0a'0a,

      .vertical_bottom_border_lines = 0,
      .vertical_front_porch_lines = 0x03'03,
      .vertical_sync_width_lines = 0x04'04,
      .vertical_back_porch_lines = 0x05'05,
      .vertical_top_border_lines = 0,
  };

  const display::DisplayTiming kConverted = ToDisplayTiming(kDmtTiming);
  EXPECT_EQ(kConverted.horizontal_total_px(), 0x10'10);
  EXPECT_EQ(kConverted.horizontal_active_px, 0x0a'0a);
  EXPECT_EQ(kConverted.horizontal_blank_px(), 0x06'06);
  EXPECT_EQ(kConverted.horizontal_front_porch_px, 0x01'01);
  EXPECT_EQ(kConverted.horizontal_sync_width_px, 0x02'02);
  EXPECT_EQ(kConverted.horizontal_back_porch_px, 0x03'03);
  EXPECT_EQ(kConverted.vertical_total_lines(), 0x13'13);
  EXPECT_EQ(kConverted.vertical_active_lines, 0x07'07);
  EXPECT_EQ(kConverted.vertical_blank_lines(), 0x0c'0c);
  EXPECT_EQ(kConverted.vertical_front_porch_lines, 0x03'03);
  EXPECT_EQ(kConverted.vertical_sync_width_lines, 0x04'04);
  EXPECT_EQ(kConverted.vertical_back_porch_lines, 0x05'05);
  EXPECT_EQ(kConverted.pixel_clock_frequency_hz, 1'000'000'000);
  EXPECT_EQ(kConverted.fields_per_frame, display::FieldsPerFrame::kProgressive);
  EXPECT_EQ(kConverted.hsync_polarity, display::SyncPolarity::kPositive);
  EXPECT_EQ(kConverted.vsync_polarity, display::SyncPolarity::kNegative);
  EXPECT_EQ(kConverted.vblank_alternates, false);
  EXPECT_EQ(kConverted.pixel_repetition, false);
  EXPECT_EQ(kConverted.vertical_field_refresh_rate_millihertz(), 49'804);
}

TEST(DmtTimingToDisplayTiming, ProgressiveWithBorder) {
  constexpr DmtTiming kDmtTiming = {
      .id = 0x99,

      // This field is computed to be consistent with the other fields,
      // which have synthetic values.
      .vertical_field_refresh_rate_millihertz = 48'125,
      .pixel_clock_khz = 1'000'000,

      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .horizontal_sync_polarity = display::SyncPolarity::kPositive,
      .vertical_sync_polarity = display::SyncPolarity::kNegative,

      .horizontal_total_px = 0x10'40,
      .horizontal_active_px = 0x0a'0a,
      .horizontal_blank_start_px = 0x0a'1a,
      .horizontal_blank_px = 0x06'06,
      .horitontal_sync_start_px = 0x0b'1b,

      .horizontal_right_border_px = 0x00'10,
      .horizontal_front_porch_px = 0x01'01,
      .horizontal_sync_width_px = 0x02'02,
      .horizontal_back_porch_px = 0x03'03,
      .horizontal_left_border_px = 0x00'20,

      .vertical_total_lines = 0x13'83,
      .vertical_active_lines = 0x07'07,
      .vertical_blank_start_lines = 0x07'37,
      .vertical_blank_lines = 0x0c'0c,
      .vertical_sync_start_lines = 0x0a'3a,

      .vertical_bottom_border_lines = 0x00'30,
      .vertical_front_porch_lines = 0x03'03,
      .vertical_sync_width_lines = 0x04'04,
      .vertical_back_porch_lines = 0x05'05,
      .vertical_top_border_lines = 0x00'40,
  };

  const display::DisplayTiming kConverted = ToDisplayTiming(kDmtTiming);
  EXPECT_EQ(kConverted.horizontal_total_px(), 0x10'40);
  EXPECT_EQ(kConverted.horizontal_active_px, 0x0a'0a);
  EXPECT_EQ(kConverted.horizontal_blank_px(), 0x06'36);
  EXPECT_EQ(kConverted.horizontal_front_porch_px, 0x01'11);
  EXPECT_EQ(kConverted.horizontal_sync_width_px, 0x02'02);
  EXPECT_EQ(kConverted.horizontal_back_porch_px, 0x03'23);
  EXPECT_EQ(kConverted.vertical_total_lines(), 0x13'83);
  EXPECT_EQ(kConverted.vertical_active_lines, 0x07'07);
  EXPECT_EQ(kConverted.vertical_blank_lines(), 0x0c'7c);
  EXPECT_EQ(kConverted.vertical_front_porch_lines, 0x03'33);
  EXPECT_EQ(kConverted.vertical_sync_width_lines, 0x04'04);
  EXPECT_EQ(kConverted.vertical_back_porch_lines, 0x05'45);
  EXPECT_EQ(kConverted.pixel_clock_frequency_hz, 1'000'000'000);
  EXPECT_EQ(kConverted.fields_per_frame, display::FieldsPerFrame::kProgressive);
  EXPECT_EQ(kConverted.hsync_polarity, display::SyncPolarity::kPositive);
  EXPECT_EQ(kConverted.vsync_polarity, display::SyncPolarity::kNegative);
  EXPECT_EQ(kConverted.vblank_alternates, false);
  EXPECT_EQ(kConverted.pixel_repetition, false);
  EXPECT_EQ(kConverted.vertical_field_refresh_rate_millihertz(), 48'125);
}

}  // namespace

}  // namespace edid::internal
