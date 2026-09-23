// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/perfect-symbol-table.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <ranges>
#include <utility>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

using ::testing::Pair;
using ::testing::UnorderedElementsAre;

constexpr auto kFooSymbols = elfldltl::PerfectSymbolTable({
    "foo",
    "bar",
});

TEST(ElfldltlPerfectSymbolTableTests, PerfectSymbolMap) {
  // The gmock container matchers don't like plain ranges, so copy the map's
  // contents results into a real container for checking.
  using MapContents = std::vector<std::pair<elfldltl::SymbolName, char>>;

  elfldltl::PerfectSymbolMap<char, kFooSymbols> fronts;
  auto all_fronts = fronts.Enumerate();
  for (auto [s, c] : all_fronts) {
    static_assert(std::is_same_v<decltype(c), char&>);
    c = s.front();
  }

  EXPECT_EQ(fronts["foo"], 'f');
  EXPECT_EQ(fronts["bar"], 'b');

  EXPECT_THAT(MapContents(all_fronts.begin(), all_fronts.end()),
              UnorderedElementsAre(Pair("foo", 'f'), Pair("bar", 'b')));
}

}  // namespace
