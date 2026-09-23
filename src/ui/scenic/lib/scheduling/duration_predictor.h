// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCHEDULING_DURATION_PREDICTOR_H_
#define SRC_UI_SCENIC_LIB_SCHEDULING_DURATION_PREDICTOR_H_

#include <lib/zx/time.h>

namespace scheduling {

// Interface for predicting future durations based on previous measurements.
class DurationPredictor {
 public:
  virtual ~DurationPredictor() = default;

  virtual zx::duration GetPrediction() const = 0;

  virtual void InsertNewMeasurement(zx::duration duration) = 0;
};

}  // namespace scheduling

#endif  // SRC_UI_SCENIC_LIB_SCHEDULING_DURATION_PREDICTOR_H_
