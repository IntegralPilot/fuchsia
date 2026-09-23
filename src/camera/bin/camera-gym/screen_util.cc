// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/camera/bin/camera-gym/screen_util.h"

#include <algorithm>
namespace screen_util {
std::tuple<uint32_t, uint32_t> GetGridSize(uint32_t n) {
  uint32_t rows = 0;
  uint32_t cols = 0;
  while (rows * cols < n) {
    if (rows == cols) {
      ++cols;
    } else {
      ++rows;
    }
  }
  return {rows, cols};
}

float Scale(float display_width, float display_height, float bounding_width,
            float bounding_height) {
  float x_scale = bounding_width / display_width;
  float y_scale = bounding_height / display_height;
  return std::min(x_scale, y_scale);
}

std::tuple<float, float> GetCenter(uint32_t index, uint32_t n) {
  auto [rows, cols] = GetGridSize(n);
  uint32_t row = index / cols;
  uint32_t col = index % cols;
  float y = (static_cast<float>(row) + 0.5f) / static_cast<float>(rows);
  float x = (static_cast<float>(col) + 0.5f) / static_cast<float>(cols);
  // Center-align the last row if it is not fully filled.
  if (row == rows - 1) {
    x += static_cast<float>(rows * cols - n) * 0.5f / static_cast<float>(cols);
  }
  return {x, y};
}
}  // namespace screen_util
