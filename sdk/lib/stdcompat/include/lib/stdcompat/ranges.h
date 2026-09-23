// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_STDCOMPAT_RANGES_H_
#define LIB_STDCOMPAT_RANGES_H_

#if __cpp_lib_containers_ranges >= 202202L
#include <ranges>
#endif

namespace cpp23 {

#if __cpp_lib_containers_ranges >= 202202L
// Use the standard from_range_t if available.
using std::from_range;
using std::from_range_t;
#else
// Polyfill for C++23 std::from_range_t.
struct from_range_t {
  explicit from_range_t() = default;
};
inline constexpr from_range_t from_range{};
#endif

}  // namespace cpp23

#endif  // LIB_STDCOMPAT_RANGES_H_
