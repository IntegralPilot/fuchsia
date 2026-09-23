// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/memory.h>
#include <lib/stdcompat/string_view.h>
#include <lib/stdcompat/utility.h>

#include "gtest.h"

namespace {
constexpr bool ExchangeCheck() {
  int a = 1;
  int b = 2;

  b = cpp20::exchange(a, std::move(b));

  return a == 2 && b == 1;
}

constexpr bool ExchangeCheck2() {
  std::string_view a = "1";
  std::string_view b = "2";

  b = cpp20::exchange(a, std::move(b));

  return a == "2" && b == "1";
}

bool ExchangeCheck3() {
  std::string_view a = "1";
  std::string_view b = "2";

  b = cpp20::exchange(a, std::move(b));

  return a == "2" && b == "1";
}

TEST(ExchangeTest, IsConstexpr) {
  static_assert(ExchangeCheck(), "exchange evaluates incorrectly in constexpr context.");
  static_assert(ExchangeCheck2(), "exchange evaluates incorrectly in constexpr context.");
}

TEST(ExchangeTest, Runtime) { ASSERT_TRUE(ExchangeCheck3()); }

#if defined(__cpp_lib_constexpr_algorithms) && __cpp_lib_constexpr_algorithms >= 201806L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(ExchangeTest, IsAliasWhenAvailable) {
  constexpr int (*cpp20_exchange)(int&, int&&) = &cpp20::exchange<int>;
  constexpr int (*std_exchange)(int&, int&&) = &std::exchange<int>;
  static_assert(cpp20_exchange == std_exchange,
                "cpp20::exchange must be an alias for std::exchange in c++20.");
}

#endif

// Note since these are static asserts the testing actually happens at compile
// time.
TEST(ToUnderlying, TypesMatch) {
  enum class E1 : char { e };

  static_assert(std::is_same_v<char, decltype(cpp23::to_underlying(E1::e))>);

  enum struct E2 : long { e };

  static_assert(std::is_same_v<long, decltype(cpp23::to_underlying(E2::e))>);

  enum E3 : unsigned { e };

  static_assert(std::is_same_v<unsigned, decltype(cpp23::to_underlying(e))>);
}

// TODO(https://fxbug.dev/42180908)
// #if defined(__cpp_lib_as_const) && __cpp_lib_as_const >= 201510L &&
// !defined(LIB_STDCOMPAT_USE_POLYFILLS)

// TEST(AsConstTest, IsAliasWhenAvailable) {
// constexpr const int& (*cpp17_as_const)(int&) = &std::as_const<int>;
// constexpr const int& (*std_as_const)(int&) = &std::as_const<int>;
// static_assert(cpp17_as_const == std_as_const,
//"cpp17::as_const must be an alias for std::as_const in c++17.");
//}

// #endif

}  // namespace
