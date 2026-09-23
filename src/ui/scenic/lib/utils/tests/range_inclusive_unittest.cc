// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/range_inclusive.h"

#include <limits>

#include <gtest/gtest.h>

namespace utils {

namespace {

TEST(Range, RangeWithoutBounds) {
  const RangeInclusive<int> range{};
  EXPECT_FALSE(range.HasBounds());

  EXPECT_TRUE(range.Contains(0));
  EXPECT_TRUE(range.Contains(1));
  EXPECT_TRUE(range.Contains(-1));

  EXPECT_TRUE(range.Contains(std::numeric_limits<int>::max()));
  EXPECT_TRUE(range.Contains(std::numeric_limits<int>::min()));
}

TEST(Range, RangeWithoutUpperBound) {
  const RangeInclusive<int> range(100, PositiveInfinity{});
  EXPECT_TRUE(range.HasBounds());

  EXPECT_TRUE(range.Contains(100));
  EXPECT_TRUE(range.Contains(101));
  EXPECT_FALSE(range.Contains(99));
  EXPECT_FALSE(range.Contains(0));

  EXPECT_FALSE(range.Contains(std::numeric_limits<int>::min()));
  EXPECT_TRUE(range.Contains(std::numeric_limits<int>::max()));
}

TEST(Range, RangeWithoutLowerBound) {
  const RangeInclusive<int> range(NegativeInfinity{}, 100);
  EXPECT_TRUE(range.HasBounds());

  EXPECT_TRUE(range.Contains(99));
  EXPECT_TRUE(range.Contains(100));
  EXPECT_FALSE(range.Contains(101));

  EXPECT_TRUE(range.Contains(std::numeric_limits<int>::min()));
  EXPECT_FALSE(range.Contains(std::numeric_limits<int>::max()));
}

TEST(Range, RangeWithBothBounds) {
  const RangeInclusive<int> range(1, 100);
  EXPECT_TRUE(range.HasBounds());

  EXPECT_FALSE(range.Contains(0));
  EXPECT_TRUE(range.Contains(1));
  EXPECT_TRUE(range.Contains(50));
  EXPECT_TRUE(range.Contains(100));
  EXPECT_FALSE(range.Contains(101));
}

TEST(Range, RangeWithSingleNumber) {
  const RangeInclusive<int> range(100, 100);

  EXPECT_FALSE(range.Contains(99));
  EXPECT_TRUE(range.Contains(100));
  EXPECT_FALSE(range.Contains(101));
}

}  // namespace

}  // namespace utils
