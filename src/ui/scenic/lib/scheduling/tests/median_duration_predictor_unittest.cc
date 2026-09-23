// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scheduling/median_duration_predictor.h"

#include <gtest/gtest.h>

namespace scheduling {
namespace test {

TEST(MedianDurationPredictor, FirstPredictionIsInitialPrediction) {
  const zx::duration kInitialPrediction = zx::usec(500);
  MedianDurationPredictor predictor(kInitialPrediction);
  EXPECT_EQ(predictor.GetPrediction(), kInitialPrediction);
}

TEST(MedianDurationPredictor, CustomWindowSizeAdaptsSuccessfully) {
  const zx::duration kTarget = zx::msec(16);
  const zx::duration kNewTarget = zx::msec(40);

  // Initialize with a custom 3-frame window
  MedianDurationPredictor predictor(kTarget, /*window_size=*/3);

  // In a partial window, a single frame bounds are exactly 1 element. So it swings to 40ms
  // instantly!
  predictor.InsertNewMeasurement(kNewTarget);
  EXPECT_EQ(predictor.GetPrediction(), kNewTarget);

  // Once 2 frames arrive, 40ms is the majority of the 3-frame window, so it snaps instantly!
  predictor.InsertNewMeasurement(kNewTarget);
  EXPECT_EQ(predictor.GetPrediction(), kNewTarget);
}

TEST(MedianDurationPredictor, Ignores3msJitterAround16ms) {
  const zx::duration kBaseline = zx::usec(16660);           // 16.66ms
  const zx::duration kHighSpike = kBaseline + zx::msec(3);  // 19.66ms
  const zx::duration kLowSpike = kBaseline - zx::msec(3);   // 13.66ms

  MedianDurationPredictor predictor(kBaseline, /*window_size=*/5);

  // With scope bounds exactly 1, the Median filter matches high spike instantly
  predictor.InsertNewMeasurement(kHighSpike);
  EXPECT_EQ(predictor.GetPrediction(), kHighSpike);

  predictor.InsertNewMeasurement(kBaseline);
  // Scope limit 2: [kBaseline, kHighSpike]. Index 1 matches high spike!
  EXPECT_EQ(predictor.GetPrediction(), kHighSpike);

  predictor.InsertNewMeasurement(kLowSpike);
  // Since 3 of the 5 frames in the window are still the 16.66ms baseline or bounded by it,
  // the median remains completely locked onto the true 16.66ms baseline despite the 3ms swings!
  EXPECT_EQ(predictor.GetPrediction(), kBaseline);
}

TEST(MedianDurationPredictor, MedianDuringPartialWindow) {
  const zx::duration kTarget = zx::msec(16);
  const zx::duration kNewTarget = zx::msec(40);

  MedianDurationPredictor predictor(kTarget, /*window_size=*/5);
  EXPECT_EQ(predictor.GetPrediction(), kTarget);

  predictor.InsertNewMeasurement(kNewTarget);
  EXPECT_EQ(predictor.GetPrediction(), kNewTarget);

  predictor.InsertNewMeasurement(zx::msec(25));
  // Window contains [40ms, 25ms]. When sorted: [25ms, 40ms]. With effective size 2, middle index is
  // 1, so median is 40ms.
  EXPECT_EQ(predictor.GetPrediction(), kNewTarget);
}

}  // namespace test
}  // namespace scheduling
