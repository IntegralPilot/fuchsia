// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_UTIL_ENUM_UTILS_H_
#define SRC_UI_LIB_ESCHER_UTIL_ENUM_UTILS_H_

#include <array>
#include <optional>
#include <type_traits>
#include <utility>

#include "src/ui/lib/escher/util/enum_utils_internal.h"

namespace escher {

// Helper to safely cast an enum class value to its underlying type.
template <typename E>
constexpr std::underlying_type_t<E> EnumCast(E x) {
  return static_cast<std::underlying_type_t<E>>(x);
}

// Return the number of elements in an enum, which must properly define
// kEnumCount: they should start at zero and monotonically increase by 1,
// so that kEnumCount is equal to the number of previous values in the enum.
template <typename E>
constexpr size_t EnumCount() {
  return static_cast<size_t>(E::kEnumCount);
}

// Cycle through an enum's values, safely wrapping around in either direction.
// The enum must meet the requirements of EnumCount().
template <typename E>
E EnumCycle(E e, bool reverse = false) {
  size_t count = EnumCount<E>();
  auto underlying_value = EnumCast(e);
  underlying_value = (underlying_value + (reverse ? count - 1 : 1)) %
                     static_cast<decltype(underlying_value)>(count);
  return static_cast<E>(underlying_value);
}

// Return an array populated with all of the enum's values.  The enum must meet the requirements
// of EnumCount().
template <typename E>
std::array<E, EnumCount<E>()> EnumArray() {
  std::array<E, EnumCount<E>()> result;
  for (size_t i = 0; i < EnumCount<E>(); ++i) {
    result[i] = E(i);
  }
  return result;
}

// Returns the maximum value of the enum E within the range [Begin, End).
//
// This is evaluated at compile time and scans the integer range using compiler-specific
// __PRETTY_FUNCTION__ string parsing. The range of checked values defaults to [-128, 128)
// to prevent excessive template instantiation overhead.
//
// Returns std::nullopt if no defined enum elements are found within the range.
template <typename E, int Begin = -128, int End = 128>
constexpr std::optional<size_t> EnumMaxElementValue() {
  static_assert(std::is_enum_v<E>, "Non-enum type is not supported!");
  static_assert(End > Begin, "End must be greater than Begin");
  return internal::EnumMaxElementValueHelper<E, Begin>(std::make_index_sequence<End - Begin>{});
}

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_UTIL_ENUM_UTILS_H_
