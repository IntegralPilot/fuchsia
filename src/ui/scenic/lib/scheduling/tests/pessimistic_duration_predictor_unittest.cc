// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scheduling/pessimistic_duration_predictor.h"

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace scheduling {
namespace test {

TEST(PessimisticDurationPredictor, FirstPredictionIsInitialPrediction) {
  const size_t kWindowSize = 4;
  const zx::duration kInitialPrediction = zx::usec(500);
  PessimisticDurationPredictor predictor(kWindowSize, kInitialPrediction);
  EXPECT_EQ(predictor.GetPrediction(), kInitialPrediction);
}

TEST(PessimisticDurationPredictor, PredictionAfterWindowFlushIsMeasurement) {
  const size_t kWindowSize = 4;
  const zx::duration kInitialPrediction = zx::msec(1);
  PessimisticDurationPredictor predictor(kWindowSize, kInitialPrediction);

  const zx::duration measurement = zx::msec(5);
  EXPECT_GT(measurement, kInitialPrediction);
  predictor.InsertNewMeasurement(measurement);

  for (size_t i = 0; i < kWindowSize - 1; ++i) {
    predictor.InsertNewMeasurement(measurement);
  }
  EXPECT_EQ(predictor.GetPrediction(), measurement);
}

TEST(PessimisticDurationPredictor, PredictionAsMeasurementsIncrease) {
  size_t window_size = 8;
  PessimisticDurationPredictor predictor(window_size, /* initial prediction */ zx::usec(0));

  for (size_t i = 1; i <= window_size; ++i) {
    predictor.InsertNewMeasurement(zx::msec(i));
    EXPECT_EQ(predictor.GetPrediction(), zx::msec(i));
  }
}

TEST(PessimisticDurationPredictor, PredictionAsMeasurementsDecrease) {
  size_t window_size = 8;
  PessimisticDurationPredictor predictor(window_size, /* initial prediction */ zx::usec(0));

  for (size_t i = window_size; i > 0; --i) {
    predictor.InsertNewMeasurement(zx::msec(i));
    EXPECT_EQ(predictor.GetPrediction(), zx::msec(window_size));
  }
}

TEST(PessimisticDurationPredictor, PredictionIsLargestInWindow) {
  size_t window_size = 8;
  PessimisticDurationPredictor predictor(window_size, /* initial prediction */ zx::usec(0));

  const std::vector<zx::duration> measurements{
      zx::msec(12), zx::msec(4),  zx::msec(5), zx::msec(2), zx::msec(8),
      zx::msec(15), zx::msec(13), zx::msec(6), zx::msec(8), zx::msec(9)};
  for (const auto& m : measurements) {
    predictor.InsertNewMeasurement(m);
  }
  EXPECT_EQ(predictor.GetPrediction(), zx::msec(15));
}

TEST(PessimisticDurationPredictor, MaxIsResetWhenLargestIsOutOfWindow) {
  size_t window_size = 4;
  PessimisticDurationPredictor predictor(window_size, /* initial prediction */ zx::usec(0));

  const std::vector<zx::duration> measurements{
      zx::msec(12), zx::msec(4),  zx::msec(5), zx::msec(2), zx::msec(8),
      zx::msec(55), zx::msec(13), zx::msec(6), zx::msec(8), zx::msec(9)};
  for (const auto& m : measurements) {
    predictor.InsertNewMeasurement(m);
  }
  EXPECT_EQ(predictor.GetPrediction(), zx::msec(13));
}

TEST(PessimisticDurationPredictor, WindowSizeOfOneWorks) {
  size_t window_size = 1;
  PessimisticDurationPredictor predictor(window_size, /* initial prediction */ zx::usec(0));

  for (size_t i = 0; i < 5; ++i) {
    predictor.InsertNewMeasurement(zx::msec(i));
  }
  EXPECT_EQ(predictor.GetPrediction(), zx::msec(4));
}

TEST(PessimisticDurationPredictor, MaxWindowSizeWorks) {
  size_t window_size = 8;
  PessimisticDurationPredictor predictor(window_size, /* initial prediction */ zx::usec(0));

  for (size_t i = 1; i <= 20; ++i) {
    predictor.InsertNewMeasurement(zx::msec(i));
  }
  // The window contains elements from i=13 to 20. The max should be 20.
  EXPECT_EQ(predictor.GetPrediction(), zx::msec(20));
}

TEST(PessimisticDurationPredictor, PushIdenticalElementsCorrectlyMaintained) {
  size_t window_size = 4;
  PessimisticDurationPredictor predictor(window_size, /* initial prediction */ zx::usec(100));

  predictor.InsertNewMeasurement(zx::usec(50));
  predictor.InsertNewMeasurement(zx::usec(50));
  predictor.InsertNewMeasurement(zx::usec(50));

  // The initial prediction (100) is still in the window!
  EXPECT_EQ(predictor.GetPrediction(), zx::usec(100));

  predictor.InsertNewMeasurement(zx::usec(50));
  // Now initial prediction is pushed out, max should be 50.
  EXPECT_EQ(predictor.GetPrediction(), zx::usec(50));
}

}  // namespace test
}  // namespace scheduling
