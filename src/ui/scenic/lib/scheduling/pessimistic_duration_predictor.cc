// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scheduling/pessimistic_duration_predictor.h"

#include <lib/syslog/cpp/macros.h>

#include <cstring>

namespace scheduling {

PessimisticDurationPredictor::PessimisticDurationPredictor(size_t window_size,
                                                           zx::duration initial_prediction)
    : window_size_(window_size) {
  FX_CHECK(window_size_ <= kMaxWindowSize);
  window_.fill(initial_prediction);
  current_maximum_duration_index_ = window_size_ - 1;
}

zx::duration PessimisticDurationPredictor::GetPrediction() const {
  return window_[current_maximum_duration_index_];
}

void PessimisticDurationPredictor::InsertNewMeasurement(zx::duration duration) {
  // Move window forward.
  std::memmove(&window_[1], &window_[0], (window_size_ - 1) * sizeof(zx::duration));
  window_[0] = duration;
  ++current_maximum_duration_index_;

  if (current_maximum_duration_index_ >= window_size_) {
    // If old max went out of scope, find the new max.
    current_maximum_duration_index_ = 0;
    for (size_t i = 1; i < window_size_; ++i) {
      if (window_[i] > window_[current_maximum_duration_index_]) {
        current_maximum_duration_index_ = i;
      }
    }
  } else if (window_[0] >= window_[current_maximum_duration_index_]) {
    // Use newest possible maximum.
    current_maximum_duration_index_ = 0;
  }
}

}  // namespace scheduling
