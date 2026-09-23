// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/abi-ptr.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <gtest/gtest.h>

namespace {

// Test the first default template argument.
static_assert(std::is_same_v<elfldltl::AbiPtr<int>, elfldltl::AbiPtr<int, elfldltl::Elf<>>>);

FORMAT_TYPED_TEST_SUITE(ElfldltlAbiPtrTests);

struct S {
  int x;
};

template <class Ptr>
constexpr void BasicCtorAndAssignTests() {
  [[maybe_unused]] constexpr Ptr default_ctor;
  [[maybe_unused]] constexpr Ptr copy_ctor(default_ctor);
  [[maybe_unused]] Ptr assign = default_ctor;
  assign = default_ctor;
}

TYPED_TEST(ElfldltlAbiPtrTests, Basic) {
  using Elf = typename TestFixture::Elf;
  using LocalPtr = elfldltl::AbiPtr<const S, Elf, elfldltl::LocalAbiTraits>;
  using RemotePtr = elfldltl::AbiPtr<const S, Elf, elfldltl::RemoteAbiTraits>;

  static_assert(std::is_same_v<LocalPtr, elfldltl::AbiPtr<const S, Elf>>);

  BasicCtorAndAssignTests<LocalPtr>();

  LocalPtr local;
  EXPECT_FALSE(local);
  EXPECT_EQ(local.get(), nullptr);
  EXPECT_EQ(local.address(), 0u);

  constexpr S var = {42};
  local = LocalPtr{&var};
  EXPECT_TRUE(local);
  EXPECT_EQ(local.address(),
            static_cast<typename Elf::size_type>(reinterpret_cast<uintptr_t>(&var)));
  EXPECT_EQ(local.get(), &var);
  EXPECT_EQ(&(*local), &var);
  EXPECT_EQ(local->x, 42);

  BasicCtorAndAssignTests<RemotePtr>();

  RemotePtr remote;
  EXPECT_FALSE(remote);
  EXPECT_EQ(remote.address(), 0u);

  remote = RemotePtr::FromAddress(42);
  EXPECT_TRUE(remote);
  EXPECT_EQ(remote.address(), 42u);
}

template <class Ptr>
void TestComparison() {
  using value_type = typename Ptr::value_type;

  const Ptr one = Ptr::FromAddress(sizeof(value_type) * 1);
  const Ptr two = Ptr::FromAddress(sizeof(value_type) * 2);
  const Ptr also_one = one;

  EXPECT_LT(one, two);
  EXPECT_LE(one, two);

  EXPECT_GT(two, one);
  EXPECT_GE(two, one);

  EXPECT_NE(one, two);
  EXPECT_NE(two, one);

  EXPECT_EQ(one, also_one);
  EXPECT_EQ(also_one, one);
  EXPECT_LE(one, also_one);
  EXPECT_LE(also_one, one);
  EXPECT_GE(one, also_one);
  EXPECT_GE(also_one, one);
}

TYPED_TEST(ElfldltlAbiPtrTests, Comparison) {
  using Elf = typename TestFixture::Elf;
  using LocalPtr = elfldltl::AbiPtr<const S, Elf, elfldltl::LocalAbiTraits>;
  using RemotePtr = elfldltl::AbiPtr<const S, Elf, elfldltl::RemoteAbiTraits>;

  TestComparison<LocalPtr>();
  TestComparison<RemotePtr>();
}

template <class Ptr>
void TestArithmetic() {
  using value_type = typename Ptr::value_type;

  const Ptr one = Ptr::FromAddress(sizeof(value_type) * 1);
  const Ptr two = Ptr::FromAddress(sizeof(value_type) * 2);

  EXPECT_EQ(one + 1, two);
  EXPECT_EQ(two - 1, one);
  EXPECT_EQ(two - one, 1u);

  Ptr x = one;
  x += 1;
  EXPECT_EQ(x, two);
  x -= 1;
  EXPECT_EQ(x, one);
}

TYPED_TEST(ElfldltlAbiPtrTests, Arithmetic) {
  using Elf = typename TestFixture::Elf;
  using LocalPtr = elfldltl::AbiPtr<const S, Elf, elfldltl::LocalAbiTraits>;
  using RemotePtr = elfldltl::AbiPtr<const S, Elf, elfldltl::RemoteAbiTraits>;

  TestArithmetic<LocalPtr>();
  TestArithmetic<RemotePtr>();
}

}  // namespace
