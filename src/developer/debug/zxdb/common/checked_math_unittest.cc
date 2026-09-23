// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/common/checked_math.h"

#include <limits>

#include <gtest/gtest.h>

namespace zxdb {

TEST(CheckedAdd, NoOverflow) {
  constexpr uint64_t kLhs = 1024;
  constexpr uint64_t kRhs = 2048;
  auto res = CheckedAdd(kLhs, kRhs);

  EXPECT_TRUE(res);
  EXPECT_EQ(*res, 3072ul);
}

TEST(CheckedAdd, DifferentTypesSelectsLarger) {
  constexpr uint8_t kLhs = 5;
  constexpr uint32_t kRhs = 10;

  auto res = CheckedAdd(kLhs, kRhs);

  // The result type should be uint32_t.
  EXPECT_TRUE(res);
  EXPECT_EQ(sizeof(*res), sizeof(uint32_t));
  EXPECT_EQ(*res, 15u);
}

TEST(CheckedAdd, OverflowSameType) {
  constexpr uint8_t kLhs = std::numeric_limits<uint8_t>::max() - 2;
  constexpr uint8_t kRhs = 25;

  auto res = CheckedAdd(kLhs, kRhs);

  EXPECT_FALSE(res);
}

TEST(CheckedAdd, OverflowDifferentType) {
  constexpr uint16_t kLhs = 10;
  constexpr uint32_t kRhs = std::numeric_limits<uint32_t>::max() - 2;

  auto res = CheckedAdd(kLhs, kRhs);

  EXPECT_FALSE(res);
}

}  // namespace zxdb
