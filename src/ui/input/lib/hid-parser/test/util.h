// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_LIB_HID_PARSER_TEST_UTIL_H_
#define SRC_UI_INPUT_LIB_HID_PARSER_TEST_UTIL_H_

#include <type_traits>

// Converts `value` to the type of its underlying value.
// The type of `value` must be an enum or enum class.
template <typename T>
std::underlying_type_t<T> ToUnderlyingType(T value) {
  static_assert(std::is_enum_v<T>);
  return static_cast<std::underlying_type_t<T>>(value);
}

#endif  // SRC_UI_INPUT_LIB_HID_PARSER_TEST_UTIL_H_
