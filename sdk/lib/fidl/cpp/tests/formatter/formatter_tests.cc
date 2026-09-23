// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.types/cpp/common_types.h>
#include <fidl/test.types/cpp/common_types_format.h>

#include <format>

#include <gtest/gtest.h>

TEST(Formatter, StrictBits) {
  EXPECT_EQ(std::format("{}", test_types::StrictBits::kB), "test_types::StrictBits(kB)");
  EXPECT_EQ(std::format("{}", test_types::StrictBits::kB | test_types::StrictBits::kD),
            "test_types::StrictBits(kB|kD)");
  EXPECT_EQ(std::format("{}", test_types::StrictBits::kB | test_types::StrictBits(128)),
            "test_types::StrictBits(kB)");
  EXPECT_EQ(std::format("{}", test_types::StrictBits(128)), "test_types::StrictBits()");
}

TEST(Formatter, FlexibleBits) {
  EXPECT_EQ(std::format("{}", test_types::FlexibleBits::kB), "test_types::FlexibleBits(kB)");
  EXPECT_EQ(std::format("{}", test_types::FlexibleBits::kB | test_types::FlexibleBits::kD),
            "test_types::FlexibleBits(kB|kD)");
  EXPECT_EQ(std::format("{}", test_types::FlexibleBits::kB | test_types::FlexibleBits(128)),
            "test_types::FlexibleBits(kB|128)");
  EXPECT_EQ(std::format("{}", test_types::FlexibleBits(128)), "test_types::FlexibleBits(128)");
}

TEST(Formatter, StrictEnum) {
  EXPECT_EQ(std::format("{}", test_types::StrictEnum::kB), "test_types::StrictEnum::kB");
  EXPECT_EQ(std::format("{}", test_types::StrictEnum::kD), "test_types::StrictEnum::kD");
}

TEST(Formatter, FlexibleEnum) {
  EXPECT_EQ(std::format("{}", test_types::FlexibleEnum::kB), "test_types::FlexibleEnum::kB");
  EXPECT_EQ(std::format("{}", test_types::FlexibleEnum::kD), "test_types::FlexibleEnum::kD");
  EXPECT_EQ(std::format("{}", test_types::FlexibleEnum(43)), "test_types::FlexibleEnum::UNKNOWN");
}
