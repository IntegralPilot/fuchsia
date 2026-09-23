// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/thermal/linear_lookup_table.h>

#include <gtest/gtest.h>

namespace thermal {

namespace {

const linear_lookup_table::LinearLookupTable<int, int> kAscending({2, 4, 6, 8}, {10, 8, 6, 4});
const linear_lookup_table::LinearLookupTable<int, int> kDescending({{2, 6}, {4, 8}, {6, 10}});

}  // namespace

TEST(LinearLookupTableTests, AscendingLookupY) {
  auto result = kAscending.LookupY(3);
  ASSERT_TRUE(result.is_ok());
  EXPECT_FLOAT_EQ(*result, 9);
}

TEST(LinearLookupTableTests, AscendingLookupX) {
  auto result = kAscending.LookupX(5);
  ASSERT_TRUE(result.is_ok());
  EXPECT_FLOAT_EQ(*result, 7);
}

TEST(LinearLookupTableTests, DescendingLookupY) {
  auto result = kDescending.LookupY(3);
  ASSERT_TRUE(result.is_ok());
  EXPECT_FLOAT_EQ(*result, 7);
}

TEST(LinearLookupTableTests, DescendingLookupX) {
  auto result = kDescending.LookupX(9);
  ASSERT_TRUE(result.is_ok());
  EXPECT_FLOAT_EQ(*result, 5);
}

TEST(LinearLookupTableTests, OutOfRange) {
  {
    auto result = kAscending.LookupY(1);
    ASSERT_TRUE(result.is_ok());
    EXPECT_FLOAT_EQ(*result, 10);
  }

  {
    auto result = kAscending.LookupY(9);
    ASSERT_TRUE(result.is_ok());
    EXPECT_FLOAT_EQ(*result, 4);
  }

  {
    auto result = kAscending.LookupX(11);
    ASSERT_TRUE(result.is_ok());
    EXPECT_FLOAT_EQ(*result, 2);
  }

  {
    auto result = kAscending.LookupX(3);
    ASSERT_TRUE(result.is_ok());
    EXPECT_FLOAT_EQ(*result, 8);
  }
}

}  // namespace thermal
