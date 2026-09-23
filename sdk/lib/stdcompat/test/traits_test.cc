// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/type_traits.h>

#include <functional>
#include <type_traits>

#include "gtest.h"

namespace {

TEST(ArrayTraitsTest, BoundedUnboundedArrayIsOk) {
  static_assert(cpp20::is_bounded_array_v<void> == false, "");
  static_assert(cpp20::is_bounded_array_v<int> == false, "");
  static_assert(cpp20::is_bounded_array_v<int*> == false, "");
  static_assert(cpp20::is_bounded_array_v<int (*)[]> == false, "");
  static_assert(cpp20::is_bounded_array_v<int (&)[]> == false, "");
  static_assert(cpp20::is_bounded_array_v<int (&&)[]> == false, "");
  static_assert(cpp20::is_bounded_array_v<int (*)[10]> == false, "");
  static_assert(cpp20::is_bounded_array_v<int (&)[10]> == false, "");
  static_assert(cpp20::is_bounded_array_v<int (&&)[10]> == false, "");
  static_assert(cpp20::is_bounded_array_v<int[10]> == true, "");
  static_assert(cpp20::is_bounded_array_v<int[]> == false, "");

  static_assert(cpp20::is_unbounded_array_v<void> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int*> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int (*)[]> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int (&)[]> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int (&&)[]> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int (*)[10]> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int (&)[10]> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int (&&)[10]> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int[10]> == false, "");
  static_assert(cpp20::is_unbounded_array_v<int[]> == true, "");
}

#if defined(__cpp_lib_bounded_array_traits) && __cpp_lib_bounded_array_traits >= 201902L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(ArrayTraitsTest, IsAliasForStd) {
  static_assert(std::is_same_v<cpp20::is_bounded_array<void>, std::is_bounded_array<void>>);
  static_assert(std::is_same_v<cpp20::is_bounded_array<int[]>, std::is_bounded_array<int[]>>);
  static_assert(std::is_same_v<cpp20::is_bounded_array<int[10]>, std::is_bounded_array<int[10]>>);

  static_assert(std::is_same_v<cpp20::is_unbounded_array<void>, std::is_unbounded_array<void>>);
  static_assert(std::is_same_v<cpp20::is_unbounded_array<int[]>, std::is_unbounded_array<int[]>>);
  static_assert(
      std::is_same_v<cpp20::is_unbounded_array<int[10]>, std::is_unbounded_array<int[10]>>);
}

#endif  // __cpp_lib_bounded_array_traits >= 201902L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(RemoveCvrefTest, RemoveCvrefIsOk) {
  static_assert(std::is_same_v<cpp20::remove_cvref_t<void>, void>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<const volatile void>, void>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<int>, int>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<int*>, int*>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<int*&>, int*>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<int*&&>, int*>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<int&>, int>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<int&&>, int>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<const int>, int>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<const int&>, int>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<const int&&>, int>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref_t<const volatile int&&>, int>, "");
}

#if defined(__cpp_lib_remove_cvref) && __cpp_lib_remove_cvref >= 201711L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(RemoveCvrefTest, IsAliasForStd) {
  static_assert(std::is_same_v<cpp20::remove_cvref<void>, std::remove_cvref<void>>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref<int>, std::remove_cvref<int>>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref<int*>, std::remove_cvref<int*>>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref<int&>, std::remove_cvref<int&>>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref<int&&>, std::remove_cvref<int&&>>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref<const int>, std::remove_cvref<const int>>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref<const int&>, std::remove_cvref<const int&>>, "");
  static_assert(std::is_same_v<cpp20::remove_cvref<const int&&>, std::remove_cvref<const int&&>>,
                "");
  static_assert(std::is_same_v<cpp20::remove_cvref<const volatile int&&>,
                               std::remove_cvref<const volatile int&&>>,
                "");
}

#endif  // __cpp_lib_remove_cvref >= 201711L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(TypeIdentityTest, TypeIdentityIsOk) {
  static_assert(std::is_same_v<cpp20::type_identity_t<void>, void>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int>, int>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int*>, int*>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int*&>, int*&>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int*&&>, int*&&>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int&>, int&>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int&&>, int&&>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<const int>, const int>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<const int&>, const int&>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<const int&&>, const int&&>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<const volatile int&&>, const volatile int&&>,
                "");
}

#if defined(__cpp_lib_type_identity) && __cpp_lib_type_identity >= 201806L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(TypeIdentityTest, IsAliasForStd) {
  static_assert(std::is_same_v<cpp20::type_identity_t<void>, std::type_identity_t<void>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int>, std::type_identity_t<int>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int*>, std::type_identity_t<int*>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int*&>, std::type_identity_t<int*&>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int*&&>, std::type_identity_t<int*&&>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int&>, std::type_identity_t<int&>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<int&&>, std::type_identity_t<int&&>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<const int>, std::type_identity_t<const int>>,
                "");
  static_assert(
      std::is_same_v<cpp20::type_identity_t<const int&>, std::type_identity_t<const int&>>, "");
  static_assert(
      std::is_same_v<cpp20::type_identity_t<const int&&>, std::type_identity_t<const int&&>>, "");
  static_assert(std::is_same_v<cpp20::type_identity_t<const volatile int&&>,
                               std::type_identity_t<const volatile int&&>>,
                "");
}

#endif  // __cpp_lib_type_identity >= 201806L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

struct static_constants {
  [[maybe_unused]] static constexpr int kRed = 0;
  [[maybe_unused]] static constexpr int kGreen = 1;
  [[maybe_unused]] static constexpr int kBlue = 2;
};
enum color { kRed, kGreen, kBlue };
enum class scoped_color { kRed, kGreen, kBlue };
enum class scoped_color_char : char { kRed, kGreen, kBlue };

TEST(ScopedEnumTest, ScopedEnumIsOk) {
  static_assert(cpp23::is_scoped_enum_v<void> == false, "");
  static_assert(cpp23::is_scoped_enum_v<int> == false, "");
  static_assert(cpp23::is_scoped_enum_v<static_constants> == false, "");
  static_assert(cpp23::is_scoped_enum_v<color> == false, "");
  static_assert(cpp23::is_scoped_enum_v<scoped_color> == true, "");
  static_assert(cpp23::is_scoped_enum_v<scoped_color_char> == true, "");
}

#if defined(__cpp_lib_is_scoped_enum) && __cpp_lib_is_scoped_enum >= 202011L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(ScopedEnumTest, IsAliasForStd) {
  static_assert(std::is_same_v<cpp23::is_scoped_enum<void>, std::is_scoped_enum<void>>, "");
  static_assert(std::is_same_v<cpp23::is_scoped_enum<int>, std::is_scoped_enum<int>>, "");
  static_assert(std::is_same_v<cpp23::is_scoped_enum<static_constants>,
                               std::is_scoped_enum<static_constants>>,
                "");
  static_assert(std::is_same_v<cpp23::is_scoped_enum<color>, std::is_scoped_enum<color>>, "");
  static_assert(
      std::is_same_v<cpp23::is_scoped_enum<scoped_color>, std::is_scoped_enum<scoped_color>>, "");
  static_assert(std::is_same_v<cpp23::is_scoped_enum<scoped_color_char>,
                               std::is_scoped_enum<scoped_color_char>>,
                "");
}

#endif  // __cpp_lib_is_scoped_enum >= 202011L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

[[maybe_unused]] int func_add_one(int i) { return i + 1; }

struct member_pointers {
  virtual int pmf_add_one(int i) { return i + 1; }
  int (*pmd_add_one)(int) = func_add_one;
};

struct liar : member_pointers {
  int pmf_add_one(int i) override { return i + 2; }
};

struct independent_liar {
  int pmf_add_one(int i) { return i + 3; }
};

constexpr int TestIsConstantEvaluated(int x) {
  if (cpp20::is_constant_evaluated()) {
    return x + 2;
  }
  return x + 3;
}

TEST(ContextTraits, IsConstantEvaluated) {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wconstant-evaluated"
  constexpr decltype(auto) is = cpp20::is_constant_evaluated();
#pragma GCC diagnostic pop
  static_assert(std::is_same_v<std::decay_t<decltype(is)>, bool>, "");

  decltype(auto) is_not = cpp20::is_constant_evaluated();
  static_assert(std::is_same_v<decltype(is_not), bool>, "");
  EXPECT_FALSE(is_not);

  int x = 2;
  int y = TestIsConstantEvaluated(x);

#if defined(__cpp_lib_is_constant_evaluated) || \
    (defined(__has_builtin) && __has_builtin(__builtin_is_constant_evaluated))
  static_assert(is, "");
  constexpr int z = TestIsConstantEvaluated(2);
  static_assert(z == 4, "");
  EXPECT_EQ(5, y);
#else
  EXPECT_EQ(4, y);
#endif
}

#if defined(__cpp_lib_is_constant_evaluated) && __cpp_lib_is_constant_evaluated >= 201811L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)
TEST(ContextTraits, IsConstantEvaluatedIsAliasForStdWhenAvailable) {
  static_assert(&cpp20::is_constant_evaluated == &std::is_constant_evaluated, "");
}
#endif

}  // namespace
