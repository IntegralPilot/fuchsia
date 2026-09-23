// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_UTILS_RANGE_INCLUSIVE_H_
#define SRC_UI_SCENIC_LIB_UTILS_RANGE_INCLUSIVE_H_

#include <zircon/assert.h>

#include <optional>
#include <type_traits>

namespace utils {

struct NegativeInfinity {};
struct PositiveInfinity {};

// A range bounded inclusively below and above its bounds.
//
// The value type must be a built-in integral type.
template <typename T>
class RangeInclusive {
 public:
  static_assert(std::is_integral_v<T>);

  // Creates a range containing all values: (-infinity, +infinity).
  RangeInclusive() : left_(std::nullopt), right_(std::nullopt) {}

  // Creates a range containing all values <= `right`: (-infinity, right].
  RangeInclusive(NegativeInfinity, T right) : left_(std::nullopt), right_(right) {}

  // Creates a range containing all values <= `left`: [left, +infinity).
  RangeInclusive(T left, PositiveInfinity) : left_(left), right_(std::nullopt) {}

  // Creates a range containing all values >= `left` and <= `right`: [left, right].
  //
  // `left` must be <= `right`.
  RangeInclusive(T left, T right) : left_(left), right_(right) { ZX_DEBUG_ASSERT(left <= right); }

  bool HasBounds() const { return left_.has_value() || right_.has_value(); }

  bool Contains(T value) const {
    if (left_.has_value() && value < *left_) {
      return false;
    }
    if (right_.has_value() && value > *right_) {
      return false;
    }
    return true;
  }

 private:
  std::optional<T> left_;
  std::optional<T> right_;
};

}  // namespace utils

#endif  // SRC_UI_SCENIC_LIB_UTILS_RANGE_INCLUSIVE_H_
