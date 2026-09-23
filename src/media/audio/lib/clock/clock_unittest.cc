// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/lib/clock/clock.h"

#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/media/audio/lib/timeline/timeline_rate.h"

//
// media_audio::Clock is a pure virtual class, but this file tests its static methods.
//

using media::TimelineRate;

namespace media_audio {
namespace {

// Rate _adjustments_ are relative to unity 1/1 and stated in integral parts-per-million.
// Thus, we describe a rate of 1999/2000 (999'500/1'000'000) as a rate adjustment of -500ppm.
TEST(ClockTest, TimelineRateToRateAdjustmentPpm) {
  // Unity
  EXPECT_EQ(Clock::TimelineRateToRateAdjustmentPpm(TimelineRate(1, 1)), 0);

  // Rate adjustment 0.5 ppm (rate 1'000'000.5 / 1'000'000) should be rounded "out" to 1ppm.
  EXPECT_EQ(Clock::TimelineRateToRateAdjustmentPpm(TimelineRate(10'000'005, 10'000'000)), 1);
  // Rate adjustment -0.5 ppm (rate 999'999.5 / 1'000'000) should be rounded "out" to -1ppm.
  EXPECT_EQ(Clock::TimelineRateToRateAdjustmentPpm(TimelineRate(9'999'995, 10'000'000)), -1);

  // Rate adjustment 0.499 ppm (rate 1'000'000.499 / 1'000'000) should be rounded "in" to 0ppm.
  EXPECT_EQ(Clock::TimelineRateToRateAdjustmentPpm(TimelineRate(1'000'000'499, 1'000'000'000)), 0);
  // Rate adjustment -0.499 ppm (rate 999'999.501 / 1'000'000) should be rounded "in" to 0ppm.
  EXPECT_EQ(Clock::TimelineRateToRateAdjustmentPpm(TimelineRate(999'999'501, 1'000'000'000)), 0);
}

// This method clamps ints to [ZX_CLOCK_UPDATE_MAX_RATE_ADJUST, ZX_CLOCK_UPDATE_MIN_RATE_ADJUST].
TEST(ClockTest, ClampZxClockPpm) {
  EXPECT_EQ(Clock::ClampZxClockPpm(0), 0);
  EXPECT_EQ(Clock::ClampZxClockPpm(ZX_CLOCK_UPDATE_MAX_RATE_ADJUST + 1),
            ZX_CLOCK_UPDATE_MAX_RATE_ADJUST);
  EXPECT_EQ(Clock::ClampZxClockPpm(ZX_CLOCK_UPDATE_MIN_RATE_ADJUST - 1),
            ZX_CLOCK_UPDATE_MIN_RATE_ADJUST);
}

// This method accepts a zero-based double and converts it to an integer parts-per-million value,
// first rounding _away from zero_ (i.e. it rounds 0.5 to 1, and -0.5 to -1), then clamping the
// result to the [ZX_CLOCK_UPDATE_MAX_RATE_ADJUST,ZX_CLOCK_UPDATE_MIN_RATE_ADJUST] range.
TEST(ClockTest, ClampDoubleToZxClockPpm) {
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(0.0), 0);

  // Exact +/-N.5 values should be rounded "out" to +/-(N+1).
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(0.5 / 1'000'000.0), 1);
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(42.5 / 1'000'000.0), 43);
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(-0.5 / 1'000'000.0), -1);
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(-67.5 / 1'000'000.0), -68);

  // Values just shy of +/-N.5 (i.e. toward zero) should be rounded "in" to +/-N.
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(0.499999 / 1'000'000.0), 0);
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(42.499999 / 1'000'000.0), 42);
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(-0.499999 / 1'000'000.0), 0);
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(-67.499999 / 1'000'000.0), -67);

  // Out-of-range values must be clamped to [...MAX_RATE_ADJUST, ...MIN_RATE_ADJUST].
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(
                static_cast<double>(2 * ZX_CLOCK_UPDATE_MAX_RATE_ADJUST) / 1'000'000.0),
            ZX_CLOCK_UPDATE_MAX_RATE_ADJUST);
  EXPECT_EQ(Clock::ClampDoubleToZxClockPpm(
                static_cast<double>(2 * ZX_CLOCK_UPDATE_MIN_RATE_ADJUST) / 1'000'000.0),
            ZX_CLOCK_UPDATE_MIN_RATE_ADJUST);
}

}  // namespace
}  // namespace media_audio
