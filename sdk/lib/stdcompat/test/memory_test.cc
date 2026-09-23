// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/memory.h>
#include <lib/stdcompat/optional.h>

#include <array>
#include <memory>
#include <type_traits>

#include "gtest.h"

struct arrow {
  struct inner {
  } value;
  const inner* operator->() const { return &value; }
};

template <typename T>
struct weird_ptr {
  const T& operator->() const { return value; }
  T value;
};

struct mismatched_traits {
  int value = 42;
  constexpr int* operator->() const { return const_cast<int*>(&value); }
};

struct dereference_only {
  int value = 100;
  constexpr int& operator*() const { return const_cast<int&>(value); }
};

namespace std {
template <>
struct pointer_traits<arrow> {
  using element_type = const typename arrow::inner;
};
template <typename T>
struct pointer_traits<weird_ptr<T>> {
  using element_type = const T;
};
template <>
struct pointer_traits<mismatched_traits> {
  using element_type = double;
};
}  // namespace std

namespace {

TEST(MemoryTest, ToAddressWithRawReturnsRightPointer) {
  constexpr int* a = nullptr;
  static_assert(cpp20::to_address(a) == nullptr, "To Address returns right raw ptr.");
}

TEST(MemoryTest, ToAddressWithFancyReturnsRightPointer) {
  std::unique_ptr<int> a = std::make_unique<int>(1);
  EXPECT_EQ(a.get(), cpp20::to_address(a));
}

TEST(MemoryTest, ToAddressWithArrowReturnsRightPointer) {
  std::optional<const int> a(13);
  EXPECT_EQ(&*a, cpp20::to_address(a));

  arrow b;
  EXPECT_EQ(&b.value, cpp20::to_address(b));

  // We only go one level because std::optional<T>::operator->() returns T*, which goes to the
  // first overload of cpp20::to_address and there is no recursion.
  std::optional<const arrow> c;
  EXPECT_EQ(&*c, cpp20::to_address(c));

  // TODO(https://fxbug.dev/42149777): libc++ and libstdc++ currently both have broken
  // implementations of std::to_address that require specializing std::pointer_traits and don't
  // allow operator-> to return non-raw-pointers, so the chaining in this test case (which seems to
  // be intentionally allowed by the standard, given that there is a recursive path through the
  // function) is impossible.
  //
  // Here we do go two levels because weird_ptr<T>::operator->() returns const T&, which goes back
  // to the overload for operator->() and then we get a raw pointer.
  // weird_ptr<std::optional<const int>> e;
  // EXPECT_EQ(&*e.value, cpp20::to_address(e));
}

// TODO(https://fxbug.dev/42149777): This is only to be bug-compatible with the standard library
// implementations; change asserts when the linked bug is resolved.
TEST(MemoryTest, BannedUses) {
  struct banned {
    const int* operator->() const { return &value; }
    int value;
  };

  // These tests must *not* compile to keep compatibility with the curent standard library
  // implementations. Uncomment them to verify that they error out both for the polyfill and the
  // standard version.

  // No std::pointer_traits specialization
  // banned a;
  // EXPECT_EQ(&a.value, cpp20::to_address(a));

  // Doesn't ever get to a raw pointer
  // weird_ptr<const int> b;
  // EXPECT_EQ(&b.value, cpp20::to_address(b));

  // Incorrect attempt at chaining
  // std::optional<const std::optional<int>> c(13);
  // EXPECT_EQ(&**c, cpp20::to_address(c));

  // No chaining
  // weird_ptr<std::optional<const int>> d;
  // EXPECT_EQ(&*d.value, cpp20::to_address(d));
}

#if defined(LIB_STDCOMPAT_USE_POLYFILLS) || __cpp_lib_to_address < 201711L

TEST(MemoryTest, ToAddressHandlesMismatchedTraits) {
  mismatched_traits ptr;
  static_assert(std::is_same_v<decltype(cpp20::to_address(ptr)), int*>);
  EXPECT_EQ(cpp20::to_address(ptr), &ptr.value);
}

TEST(MemoryTest, ToAddressHandlesDereferenceOnlyFallback) {
  dereference_only ptr;
  static_assert(std::is_same_v<decltype(cpp20::to_address(ptr)), int*>);
  EXPECT_EQ(cpp20::to_address(ptr), &ptr.value);
}

#endif

// TODO(https://fxbug.dev/42180908) Disable these tests until we can avoid taking  the address of
// std:: functions #if __cpp_lib_to_address >= 201711L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

// template <typename T>
// constexpr void check_to_address_alias() {
//// Need so the compiler picks the right overload.
// constexpr auto (*cpp20_to_address)(const T&) = &cpp20::to_address<T>;
// constexpr auto (*std_to_address)(const T&) = &std::to_address<T>;
// static_assert(cpp20_to_address == std_to_address);
//}

// TEST(MemoryTest, ToAddressIsAliasForStdWhenAvailable) {
// check_to_address_alias<int*>();
// check_to_address_alias<std::unique_ptr<int>>();
//}

// #endif

}  // namespace
