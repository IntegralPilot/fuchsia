// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCHEDULING_MEDIAN_DURATION_PREDICTOR_H_
#define SRC_UI_SCENIC_LIB_SCHEDULING_MEDIAN_DURATION_PREDICTOR_H_

#include <lib/zx/time.h>

#include <array>

#include "src/ui/scenic/lib/scheduling/duration_predictor.h"

namespace scheduling {

// A robust, entirely allocation-free median filter limited to a maximum window
// size of 8 frames (defaulting to 5).
class MedianDurationPredictor : public DurationPredictor {
 public:
  // Static maximum limit allowed for statically allocated capacity ring buffer.
  static constexpr size_t kMaxWindowSize = 8;

  explicit MedianDurationPredictor(zx::duration initial_prediction, size_t window_size = 5);
  ~MedianDurationPredictor() override = default;

  zx::duration GetPrediction() const override;

  void InsertNewMeasurement(zx::duration duration) override;

 private:
  // `window_`, `window_size_` and `index_` are used to maintain a ring-buffer of the last
  // `window_size_` measurements.
  std::array<zx::duration, kMaxWindowSize> window_;
  const size_t window_size_;
  size_t index_ = 0;
  size_t num_measurements_ = 0;
  // Stores the duration that is recomputed each time a new measurement is added.
  zx::duration prediction_;
};

}  // namespace scheduling

#endif  // SRC_UI_SCENIC_LIB_SCHEDULING_MEDIAN_DURATION_PREDICTOR_H_
