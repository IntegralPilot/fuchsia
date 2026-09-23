// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_UTIL_ENUM_UTILS_INTERNAL_H_
#define SRC_UI_LIB_ESCHER_UTIL_ENUM_UTILS_INTERNAL_H_

#include <cstddef>
#include <optional>
#include <string_view>
#include <utility>

namespace escher::internal {

// Template magic to parse __PRETTY_FUNCTION__ and detect if `V` is a valid enum val of type `E`.
// A valid enum val will result in a __PRETTY_FUNCTION__ like:
//   constexpr bool escher::internal::IsValidEnum() [E = MyEnum, V = MyEnum::kValue]
// whereas an *invalid* enum val of 999 will result in a __PRETTY_FUNCTION__ like:
//   constexpr bool escher::internal::IsValidEnum() [E = MyEnum, V = (MyEnum)999]
//
// After stripping the terminating "]/0", it iterates backward from the end until it finds something
// non-alphanumeric-or-underscore and trims that prefix.  For an invalid enum, the resulting string
// view will be a number, therefore the result is false.
template <typename E, E V>
constexpr bool EnumIsValid() {
  std::string_view name{__PRETTY_FUNCTION__, sizeof(__PRETTY_FUNCTION__) - 2};

  for (std::size_t i = name.size(); i > 0; --i) {
    // Includes numbers.
    if ((name[i - 1] < '0' || name[i - 1] > '9') && (name[i - 1] < 'a' || name[i - 1] > 'z') &&
        (name[i - 1] < 'A' || name[i - 1] > 'Z') && name[i - 1] != '_') {
      name.remove_prefix(i);
      break;
    }
  }

  // Does NOT include numbers.
  return !name.empty() && ((name.front() >= 'a' && name.front() <= 'z') ||
                           (name.front() >= 'A' && name.front() <= 'Z') || (name.front() == '_'));
}

template <typename E, int Begin, std::size_t... Is>
constexpr std::optional<size_t> EnumMaxElementValueHelper(std::index_sequence<Is...>) {
  constexpr bool valid[] = {EnumIsValid<E, static_cast<E>(Begin + Is)>()...};
  for (size_t i = sizeof...(Is); i > 0; --i) {
    if (valid[i - 1]) {
      return Begin + i - 1;
    }
  }
  return std::nullopt;
}

}  // namespace escher::internal

#endif  // SRC_UI_LIB_ESCHER_UTIL_ENUM_UTILS_INTERNAL_H_
