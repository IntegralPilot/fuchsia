// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_FIXED_POINT_UTIL_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_FIXED_POINT_UTIL_H_

#include <zircon/assert.h>

#include <cstdint>
#include <limits>

namespace amlogic_display {

// Lossless conversion from a floating-point number `value` to a U28.4 fixed
// point number.
//
// The Um.n notation is defined in:
// https://source.android.com/docs/core/audio/data_formats#q.
//
// The function will error on values that doesn't fit in a U28.4 fixed-point
// number.
//
// TODO(https://fxbug.dev/42064981): In C++20 this can be consteval.
inline constexpr uint32_t ToU28_4(double value) {
  ZX_ASSERT(value >= 0);
  uint32_t fixed_value = static_cast<uint32_t>(value * 16);
  ZX_ASSERT(static_cast<double>(fixed_value) / 16 == value);
  return fixed_value;
}

// Lossless conversion from an integer `value` to a U28.4 fixed point number.
//
// The Um.n notation is defined in:
// https://source.android.com/docs/core/audio/data_formats#q.
//
// The function will error on values that doesn't fit in a U28.4 fixed-point
// number.
//
// TODO(https://fxbug.dev/42064981): In C++20 this can be consteval.
inline constexpr uint32_t ToU28_4(int value) {
  ZX_ASSERT(value >= 0);
  ZX_ASSERT(int64_t{value} * 16 <= std::numeric_limits<uint32_t>::max());
  uint32_t fixed_value = static_cast<uint32_t>(int64_t{value} * 16);
  return fixed_value;
}

// Converts a U28.4 fixed point number `u28_4` used in HdmiClockTreeControl::
// SetFrequencyDivider.
//
// The Um.n notation is defined in:
// https://source.android.com/docs/core/audio/data_formats#q.
//
// TODO(https://fxbug.dev/42064981): In C++20 this can be consteval.
inline constexpr double U28_4ToDouble(uint32_t u28_4) {
  // This is guaranteed to be valid for all U28.4 values, since "double" has
  // 52 precision bits.
  return static_cast<double>(u28_4) / 16.0;
}

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_FIXED_POINT_UTIL_H_
