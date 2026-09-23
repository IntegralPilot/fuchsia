// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/edid/cta-timing.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/edid/timings.h"

namespace edid::internal {

namespace {

TEST(CtaTimingInvariantTest, HsyncPolarityIsValid) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    EXPECT_TRUE(cta_timing.horizontal_sync_polarity == display::SyncPolarity::kPositive ||
                cta_timing.horizontal_sync_polarity == display::SyncPolarity::kNegative);
  }
}

TEST(CtaTimingInvariantTest, VsyncPolarityIsValid) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    EXPECT_TRUE(cta_timing.vertical_sync_polarity == display::SyncPolarity::kPositive ||
                cta_timing.vertical_sync_polarity == display::SyncPolarity::kNegative);
  }
}

TEST(CtaTimingInvariantTest, SecondFieldMustNotHaveExtraVerticalBlankForProgressiveTimings) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    if (cta_timing.fields_per_frame == display::FieldsPerFrame::kProgressive) {
      EXPECT_FALSE(cta_timing.second_field_has_extra_vertical_blank_line);
    }
  }
}

TEST(CtaTimingInvariantTest, FieldsPerFrameIsValid) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    EXPECT_TRUE(cta_timing.fields_per_frame == display::FieldsPerFrame::kInterlaced ||
                cta_timing.fields_per_frame == display::FieldsPerFrame::kProgressive);
  }
}

TEST(CtaTimingInvariantTest, HorizontalBlankMatchesDefinition) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    EXPECT_EQ(cta_timing.horizontal_blank_px, cta_timing.horizontal_back_porch_px +
                                                  cta_timing.horizontal_sync_width_px +
                                                  cta_timing.horizontal_front_porch_px);
  }
}

TEST(CtaTimingInvariantTest, HorizontalTotalMatchesDefinition) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    EXPECT_EQ(cta_timing.horizontal_total_px,
              cta_timing.horizontal_active_px + cta_timing.horizontal_blank_px);
  }
}

TEST(CtaTimingInvariantTest, VerticalActiveIsEvenForInterlacedModes) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    if (cta_timing.fields_per_frame == display::FieldsPerFrame::kInterlaced) {
      EXPECT_EQ(cta_timing.vertical_active_lines % 2, 0);
    }
  }
}

TEST(CtaTimingInvariantTest, VerticalBlankMatchesDefinition) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    EXPECT_EQ(cta_timing.vertical_blank_lines, cta_timing.vertical_back_porch_lines +
                                                   cta_timing.vertical_sync_width_lines +
                                                   cta_timing.vertical_front_porch_lines);
  }
}

TEST(CtaTimingInvariantTest, VerticalTotalMatchesDefinition) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);
    switch (cta_timing.fields_per_frame) {
      case display::FieldsPerFrame::kProgressive: {
        EXPECT_EQ(cta_timing.vertical_total_lines,
                  cta_timing.vertical_active_lines + cta_timing.vertical_blank_lines);
        break;
      }
      case display::FieldsPerFrame::kInterlaced: {
        int extra_blank = cta_timing.second_field_has_extra_vertical_blank_line ? 1 : 0;
        EXPECT_EQ(
            cta_timing.vertical_total_lines,
            cta_timing.vertical_active_lines + cta_timing.vertical_blank_lines * 2 + extra_blank);
        break;
      }
    }
  }
}

TEST(CtaTimingInvariantTest, VerticalFieldRefreshRateMatchesDefinition) {
  for (const CtaTiming& cta_timing : kCtaTimings) {
    SCOPED_TRACE(testing::Message() << "Failed at VIC " << cta_timing.video_identification_code);

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
        cta_timing.fields_per_frame == display::FieldsPerFrame::kInterlaced ? 2 : 1;

    // The multiplication won't overflow and cause an undefined behavior.
    // `cta_timing.pixel_clock_khz` is a 31-bit value and 1'000'000 < 2^20.
    // Thus the multiplication is less than 2^52 - 1.
    const int64_t pixel_clock_millihertz = int64_t{cta_timing.pixel_clock_khz} * 1'000'000;

    // The multiplication won't overflow and cause an undefined behavior.
    // Both `cta_timing.horizontal_total_px` and `cta_timing.vertical_total_lines`
    // are at most 2^16 - 1, thus the multiplication is less than 2^32 - 1.
    const int64_t pixels_per_frame =
        int64_t{cta_timing.horizontal_total_px} * cta_timing.vertical_total_lines;

    // calculated_vertical_field_refresh_rate_millihertz
    //  = round(pixel_clock_millihertz * fields_per_frame / pixels_per_frame)
    //  = (pixel_clock_millihertz * fields_per_frame + pixels_per_frame/ 2) div
    //    pixels_per_frame
    //
    // Since `pixel_clock_millihertz * fields_per_frame` < 2^53 - 1 and
    // `pixels_per_frame` < 2^32 - 1, the divisor, dividend and the quotient
    // won't overflow in int64_t.
    const int64_t calculated_vertical_field_refresh_rate_millihertz =
        (pixel_clock_millihertz * fields_per_frame + pixels_per_frame / 2) / pixels_per_frame;

    EXPECT_EQ(int64_t{cta_timing.vertical_field_refresh_rate_millihertz},
              calculated_vertical_field_refresh_rate_millihertz);
  }
}

TEST(CtaTimingToDisplayTiming, InterlacedWithAlternatingVblank) {
  constexpr CtaTiming kCtaTiming = {
      .video_identification_code = 0x99,

      .horizontal_active_px = 0x0a'0a,
      .vertical_active_lines = 0x07'07,
      .fields_per_frame = display::FieldsPerFrame::kInterlaced,

      .horizontal_total_px = 0x10'10,
      .horizontal_blank_px = 0x06'06,
      .horizontal_front_porch_px = 0x01'01,
      .horizontal_sync_width_px = 0x02'02,
      .horizontal_back_porch_px = 0x03'03,
      .horizontal_sync_polarity = display::SyncPolarity::kPositive,

      .vertical_total_lines = 0x1f'1f,
      .vertical_blank_lines = 0x0c'0c,
      .second_field_has_extra_vertical_blank_line = false,
      .vertical_front_porch_lines = 0x03'03,
      .vertical_sync_width_lines = 0x04'04,
      .vertical_back_porch_lines = 0x05'05,
      .vertical_sync_polarity = display::SyncPolarity::kNegative,

      .vertical_field_refresh_rate_millihertz = 61'049,
      .pixel_clock_khz = 1'000'000,
  };

  const display::DisplayTiming kConverted = ToDisplayTiming(kCtaTiming);
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

TEST(CtaTimingToDisplayTiming, InterlacedWithConstantVblank) {
  constexpr CtaTiming kCtaTiming = {
      .video_identification_code = 0x99,

      .horizontal_active_px = 0x0a'0a,
      .vertical_active_lines = 0x07'07,
      .pixel_repeated = false,
      .fields_per_frame = display::FieldsPerFrame::kInterlaced,

      .horizontal_total_px = 0x10'10,
      .horizontal_blank_px = 0x06'06,
      .horizontal_front_porch_px = 0x01'01,
      .horizontal_sync_width_px = 0x02'02,
      .horizontal_back_porch_px = 0x03'03,
      .horizontal_sync_polarity = display::SyncPolarity::kPositive,

      .vertical_total_lines = 0x1f'20,
      .vertical_blank_lines = 0x0c'0c,
      .second_field_has_extra_vertical_blank_line = true,
      .vertical_front_porch_lines = 0x03'03,
      .vertical_sync_width_lines = 0x04'04,
      .vertical_back_porch_lines = 0x05'05,
      .vertical_sync_polarity = display::SyncPolarity::kNegative,

      .vertical_field_refresh_rate_millihertz = 61'042,
      .pixel_clock_khz = 1'000'000,
  };

  const display::DisplayTiming kConverted = ToDisplayTiming(kCtaTiming);
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

TEST(CtaTimingToDisplayTiming, Progressive) {
  constexpr CtaTiming kCtaTiming = {
      .video_identification_code = 0x99,

      .horizontal_active_px = 0x0a'0a,
      .vertical_active_lines = 0x07'07,
      .pixel_repeated = false,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,

      .horizontal_total_px = 0x10'10,
      .horizontal_blank_px = 0x06'06,
      .horizontal_front_porch_px = 0x01'01,
      .horizontal_sync_width_px = 0x02'02,
      .horizontal_back_porch_px = 0x03'03,
      .horizontal_sync_polarity = display::SyncPolarity::kPositive,

      .vertical_total_lines = 0x13'13,
      .vertical_blank_lines = 0x0c'0c,
      .second_field_has_extra_vertical_blank_line = false,
      .vertical_front_porch_lines = 0x03'03,
      .vertical_sync_width_lines = 0x04'04,
      .vertical_back_porch_lines = 0x05'05,
      .vertical_sync_polarity = display::SyncPolarity::kNegative,

      .vertical_field_refresh_rate_millihertz = 49'804,
      .pixel_clock_khz = 1'000'000,
  };

  const display::DisplayTiming kConverted = ToDisplayTiming(kCtaTiming);
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

}  // namespace

}  // namespace edid::internal
