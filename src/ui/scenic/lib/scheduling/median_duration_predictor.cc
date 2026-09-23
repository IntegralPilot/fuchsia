// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scheduling/median_duration_predictor.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>

namespace scheduling {

MedianDurationPredictor::MedianDurationPredictor(zx::duration initial_prediction,
                                                 size_t window_size)
    : window_size_(window_size), prediction_(initial_prediction) {
  FX_CHECK(window_size_ <= kMaxWindowSize);
  window_.fill(initial_prediction);
}

zx::duration MedianDurationPredictor::GetPrediction() const { return prediction_; }

void MedianDurationPredictor::InsertNewMeasurement(zx::duration duration) {
  window_[index_] = duration;
  index_ = (index_ + 1) % window_size_;
  num_measurements_ = std::min(num_measurements_ + 1, window_size_);

  const size_t effective_window = num_measurements_;
  const size_t middle_index = effective_window / 2;

  // We must copy into `sort_buffer` because `std::nth_element` reorders elements in-place.
  // Running it directly on `window_` would scramble the chronological order of our ring
  // buffer, causing subsequent frame insertions to overwrite the wrong history elements.
  std::array<zx::duration, kMaxWindowSize> sort_buffer;
  std::copy_n(window_.begin(), effective_window, sort_buffer.begin());
  std::nth_element(sort_buffer.begin(),
                   sort_buffer.begin() + static_cast<std::ptrdiff_t>(middle_index),
                   sort_buffer.begin() + static_cast<std::ptrdiff_t>(effective_window));
  prediction_ = sort_buffer[middle_index];
}

}  // namespace scheduling
