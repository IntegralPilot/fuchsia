// Copyright 2017 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SAFEMATH_RANGES_H_
#define SAFEMATH_RANGES_H_

#include <cmath>
#include <type_traits>

namespace safemath {

template <typename T>
constexpr bool IsApproximatelyEqual(T lhs, T rhs, T tolerance) {
  static_assert(std::is_arithmetic_v<T>, "Argument must be arithmetic");
  return std::abs(rhs - lhs) <= tolerance;
}

}  // namespace safemath

#endif  // SAFEMATH_RANGES_H_
