// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/abi-span.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <gtest/gtest.h>

namespace {

// First two default template arguments.
static_assert(std::is_same_v<elfldltl::AbiSpan<int>,
                             elfldltl::AbiSpan<int, std::dynamic_extent, elfldltl::Elf<>>>);

FORMAT_TYPED_TEST_SUITE(ElfldltlAbiSpanTests);

template <class Span>
constexpr void BasicCtorAndAssignTests() {
  static_assert(std::is_same_v<typename Span::element_type, typename Span::Ptr::value_type>);
  static_assert(std::is_same_v<typename Span::size_type, typename Span::Ptr::size_type>);
  static_assert(std::is_same_v<typename Span::Addr, typename Span::Ptr::Addr>);

  [[maybe_unused]] constexpr Span default_ctor;
  [[maybe_unused]] constexpr Span copy_ctor(default_ctor);
  [[maybe_unused]] Span assign = default_ctor;
  assign = default_ctor;
}

TYPED_TEST(ElfldltlAbiSpanTests, Basic) {
  using Elf = typename TestFixture::Elf;
  using LocalSpan =
      elfldltl::AbiSpan<const int, std::dynamic_extent, Elf, elfldltl::LocalAbiTraits>;
  using RemoteSpan =
      elfldltl::AbiSpan<const int, std::dynamic_extent, Elf, elfldltl::RemoteAbiTraits>;

  static_assert(std::is_same_v<LocalSpan, elfldltl::AbiSpan<const int, std::dynamic_extent, Elf>>);

  BasicCtorAndAssignTests<LocalSpan>();

  LocalSpan local;
  EXPECT_TRUE(local.empty());
  EXPECT_EQ(local.size(), 0u);
  EXPECT_EQ(local.size_bytes(), 0u);
  EXPECT_TRUE(local.get().empty());

  int var;
  local = LocalSpan{std::span{&var, 1}};
  EXPECT_FALSE(local.empty());
  EXPECT_EQ(local.size(), 1u);
  EXPECT_EQ(local.size_bytes(), sizeof(int));
  EXPECT_EQ(local.data(), &var);

  auto direct = local.get();
  static_assert(std::is_same_v<decltype(direct), std::span<const int>>);
  EXPECT_EQ(direct.data(), &var);
  EXPECT_EQ(direct.size(), 1u);

  local = LocalSpan{typename LocalSpan::Ptr{&var}, 1};
  EXPECT_FALSE(local.empty());
  EXPECT_EQ(local.size(), 1u);
  EXPECT_EQ(local.size_bytes(), sizeof(int));
  EXPECT_EQ(local.data(), &var);

  local = LocalSpan{local};
  EXPECT_FALSE(local.empty());
  EXPECT_EQ(local.size(), 1u);
  EXPECT_EQ(local.size_bytes(), sizeof(int));
  EXPECT_EQ(local.data(), &var);

  BasicCtorAndAssignTests<RemoteSpan>();

  RemoteSpan remote;
  EXPECT_TRUE(remote.empty());
  EXPECT_FALSE(remote.ptr());
  EXPECT_EQ(remote.size(), 0u);
  EXPECT_EQ(remote.size_bytes(), 0u);

  remote = RemoteSpan{RemoteSpan::Ptr::FromAddress(420), 1};
  EXPECT_FALSE(remote.empty());
  EXPECT_EQ(remote.size(), 1u);
  EXPECT_EQ(remote.size_bytes(), sizeof(int));

  remote = RemoteSpan{remote};
  EXPECT_FALSE(remote.empty());
  EXPECT_EQ(remote.size(), 1u);
  EXPECT_EQ(remote.size_bytes(), sizeof(int));
}

template <class Span>
void TestSubspan() {
  using value_type = typename Span::value_type;

  const auto ptr = Span::Ptr::FromAddress(sizeof(value_type) * 1);
  const Span five{ptr, 5};

  const Span last_2 = five.subspan(3);
  EXPECT_EQ(last_2.size(), 2u);
  EXPECT_EQ(last_2.ptr(), ptr + 3);

  const Span middle_3 = five.subspan(1, 3);
  EXPECT_EQ(middle_3.size(), 3u);
  EXPECT_EQ(middle_3.ptr(), ptr + 1);

  const auto fixed_last_2 = five.template subspan<3>();
  EXPECT_EQ(fixed_last_2.size(), 2u);
  EXPECT_EQ(fixed_last_2.ptr(), ptr + 3);

  const auto fixed_middle_3 = five.template subspan<1, 3>();
  EXPECT_EQ(fixed_middle_3.size(), 3u);
  EXPECT_EQ(fixed_middle_3.ptr(), ptr + 1);

  const Span first_2 = five.first(2);
  EXPECT_EQ(first_2.ptr(), five.ptr());
  EXPECT_EQ(first_2.size(), 2u);

  const Span last_1 = five.last(1);
  EXPECT_EQ(last_1.ptr(), five.ptr() + 4);
  EXPECT_EQ(last_1.size(), 1u);

  const auto fixed_first_2 = five.template first<2>();
  EXPECT_EQ(fixed_first_2.ptr(), five.ptr());
  EXPECT_EQ(fixed_first_2.size(), 2u);

  const auto fixed_last_1 = five.template last<1>();
  EXPECT_EQ(fixed_last_1.ptr(), five.ptr() + 4);
  EXPECT_EQ(fixed_last_1.size(), 1u);
}

TYPED_TEST(ElfldltlAbiSpanTests, SubSpan) {
  using Elf = typename TestFixture::Elf;
  using LocalSpan =
      elfldltl::AbiSpan<const int, std::dynamic_extent, Elf, elfldltl::LocalAbiTraits>;
  using RemoteSpan =
      elfldltl::AbiSpan<const int, std::dynamic_extent, Elf, elfldltl::RemoteAbiTraits>;

  TestSubspan<LocalSpan>();
  TestSubspan<RemoteSpan>();
}

TYPED_TEST(ElfldltlAbiSpanTests, Iterators) {
  using Elf = typename TestFixture::Elf;
  using Span = elfldltl::AbiSpan<const int, std::dynamic_extent, Elf>;

  constexpr int kInts[] = {1, 2, 3, 4, 5};
  const std::span direct{kInts};
  const Span five{direct};

  EXPECT_EQ(five.begin(), direct.begin());
  EXPECT_EQ(five.end(), direct.end());
  EXPECT_EQ(five.rbegin(), direct.rbegin());
  EXPECT_EQ(five.rend(), direct.rend());
}

TYPED_TEST(ElfldltlAbiSpanTests, Access) {
  using Elf = typename TestFixture::Elf;
  using Span = elfldltl::AbiSpan<const int, std::dynamic_extent, Elf>;

  constexpr int kInts[] = {1, 2, 3, 4, 5};
  const std::span direct{kInts};
  const Span five{direct};

  EXPECT_EQ(&five.front(), &kInts[0]);
  EXPECT_EQ(&five.back(), &kInts[4]);
  EXPECT_EQ(&five[2], &kInts[2]);
  EXPECT_EQ(five.data(), kInts);
}

TYPED_TEST(ElfldltlAbiSpanTests, StringView) {
  using Elf = typename TestFixture::Elf;
  using LocalStringView = elfldltl::AbiStringView<Elf, elfldltl::LocalAbiTraits>;
  using RemoteStringView = elfldltl::AbiStringView<Elf, elfldltl::RemoteAbiTraits>;

  LocalStringView local;
  EXPECT_TRUE(local.empty());
  EXPECT_EQ(local.size(), 0u);
  EXPECT_EQ(local.length(), 0u);
  std::string_view str = local;
  str = local.get();
  EXPECT_TRUE(str.empty());
  EXPECT_EQ(str.data(), nullptr);

  RemoteStringView remote;
  EXPECT_TRUE(remote.empty());
  EXPECT_EQ(remote.size(), 0u);
  EXPECT_EQ(remote.length(), 0u);

  constexpr std::string_view kFoobar = "foobar";
  local = LocalStringView(kFoobar);
  EXPECT_FALSE(local.empty());
  EXPECT_EQ(local.size(), 6u);
  EXPECT_EQ(local.length(), 6u);
  EXPECT_EQ(local.data(), kFoobar.data());
  EXPECT_EQ(local.get(), kFoobar);
}

}  // namespace
