// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/boot-shim.h>
#include <lib/boot-shim/item-base.h>

#include <string>
#include <tuple>
#include <type_traits>
#include <utility>

#include <zxtest/zxtest.h>

namespace {

struct Base {};

template <typename T>
struct IsIntegralOrDerivedFromBase {
  static constexpr bool value = std::is_integral_v<T> || std::is_base_of_v<Base, T>;
};

using TestShim = boot_shim::BootShim<int, bool, std::string>;
using ExpectedType = std::tuple<int&, bool&>;

constexpr auto to_tuple = [](auto... items) { return std::forward_as_tuple(items...); };

static_assert(std::is_same_v<ExpectedType,
                             decltype(std::declval<TestShim>()
                                          .OnSelectItems<IsIntegralOrDerivedFromBase>(to_tuple))>);

TEST(BootShimTests, OnInvocableItems) {
  TestShim shim("BootShimTests.OnInvocableItems");
  {
    bool saw_bool = false, saw_string = false;
    shim.OnInvocableItems(
        // Not invocable on something not in the items list at all.
        [](Base&) { ADD_FAILURE(); },
        // Invocable on bool item.
        [&saw_bool](bool&) { saw_bool = true; },
        // Invocable on std::string item.
        [&saw_string](std::string&) { saw_string = true; });
    EXPECT_TRUE(saw_bool);
    EXPECT_TRUE(saw_string);
  }
  {
    bool saw_bool = false, saw_string = false;
    std::as_const(shim).OnInvocableItems(
        // Not invocable on something not in the items list at all.
        [](const Base&) { ADD_FAILURE(); },
        // Not invocable on const item.
        [](bool&) { ADD_FAILURE(); },
        // Invocable on bool item.
        [&saw_bool](const bool&) { saw_bool = true; },
        // Invocable on std::string item.
        [&saw_string](const std::string&) { saw_string = true; });
    EXPECT_TRUE(saw_bool);
    EXPECT_TRUE(saw_string);
  }
}

}  // namespace
