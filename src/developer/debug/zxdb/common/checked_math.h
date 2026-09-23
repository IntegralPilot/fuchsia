// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_COMMON_CHECKED_MATH_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_COMMON_CHECKED_MATH_H_

#include <optional>

namespace zxdb {

// Returns the result of adding |lhs| and |rhs|. If the result overflows, then std::nullopt is
// returned. The sizes of the types for |lhs| and |rhs| may differ. The returned value will be
// stored in the larger of the two and is the type that must overflow for this function to return
// std::nullopt.
//
// Use it like:
//
// uint64_t some_number = 0;
// if (auto checked_sum = CheckedAdd(1, 2)) {
//   some_number = *checked_sum;
// } else {
//   return Err("Overflow!");
// }
template <typename T, typename U, typename R = std::conditional_t<sizeof(T) >= sizeof(U), T, U>>
constexpr std::optional<R> CheckedAdd(T lhs, U rhs) {
#if __has_builtin(__builtin_add_overflow)
  T res = 0;
  if (__builtin_add_overflow(lhs, rhs, &res)) {
    return std::nullopt;
  }

  return std::optional<T>(res);
#else
#error "No overflow intrinsic available!"
#endif
}

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_COMMON_CHECKED_MATH_H_
