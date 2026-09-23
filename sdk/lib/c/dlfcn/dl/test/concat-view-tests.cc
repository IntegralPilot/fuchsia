// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "../concat-view.h"

namespace {

using ::testing::ElementsAre;

TEST(DlTests, ConcatView) {
  // TODO(https://github.com/google/googletest/issues/4512): The gmock
  // container matchers don't handle ranges.  Ranges don't provide things like
  // value_type that the matchers wants, and instead expect you to use
  // std::ranges::range_value_t et al.  So just copy into a vector for the
  // matcher comparison.
  constexpr auto check = [](auto&& view) {
    std::vector<int> vec;
    for (int x : view) {
      vec.push_back(x);
    }
    return vec;
  };

  std::vector<int> vec{1, 0, 2, 0, 3, 0};

  auto vec_view = std::ranges::ref_view(vec);
  EXPECT_THAT(check(vec_view), ElementsAre(1, 0, 2, 0, 3, 0));

  constexpr auto is_nonzero = [](int x) -> bool { return x; };
  auto nonzero_view = std::views::filter(vec_view, is_nonzero);
  EXPECT_THAT(check(nonzero_view), ElementsAre(1, 2, 3));

  EXPECT_THAT(check(dl::ConcatView(vec_view, nonzero_view)),
              ElementsAre(1, 0, 2, 0, 3, 0, 1, 2, 3));

  // filter_view doesn't have const overloads so it can't be used as const and
  // thus can't be directly in a const concat_view.  However, ref_view has
  // const overloads that don't need the referenced view to have them.
  const dl::ConcatView const_view{vec_view, std::ranges::ref_view(nonzero_view)};
  EXPECT_THAT(check(const_view), ElementsAre(1, 0, 2, 0, 3, 0, 1, 2, 3));
}

}  // namespace
