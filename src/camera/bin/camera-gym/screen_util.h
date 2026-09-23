// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_CAMERA_BIN_CAMERA_GYM_SCREEN_UTIL_H_
#define SRC_CAMERA_BIN_CAMERA_GYM_SCREEN_UTIL_H_

#include <cstdint>
#include <tuple>

namespace screen_util {

// Calculate the grid size needed to fit |n| elements by alternately adding rows and columns.
std::tuple<uint32_t, uint32_t> GetGridSize(uint32_t n);

// Calculate scaling factor to fit display width and height into a bounding width and height while
// preserving the width and height ratio.
float Scale(float display_width, float display_height, float bounding_width, float bounding_height);

// Calculate the center of an element |index| in a grid with |n| elements.
std::tuple<float, float> GetCenter(uint32_t index, uint32_t n);
}  // namespace screen_util
#endif  // SRC_CAMERA_BIN_CAMERA_GYM_SCREEN_UTIL_H_
