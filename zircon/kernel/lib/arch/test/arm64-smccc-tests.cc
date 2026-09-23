// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/arm64/smccc.h>

#include <gtest/gtest.h>

namespace {

TEST(Arm64SmcccTests, ArmSmcccVersion) {
  constexpr arch::ArmSmcccVersion kDefaultConstructed;
  EXPECT_EQ(kDefaultConstructed.major, 0u);
  EXPECT_EQ(kDefaultConstructed.minor, 0u);

  constexpr arch::ArmSmcccVersion k1p0 = {1, 0};
  constexpr arch::ArmSmcccVersion k0p1 = {0, 1};

  arch::ArmSmcccVersion version = k1p0;
  version = k0p1;

  EXPECT_EQ(version, k0p1);
  EXPECT_NE(version, k1p0);

  EXPECT_LT(version, k1p0);
  EXPECT_LT(k0p1, k1p0);

  EXPECT_LE(version, k0p1);
  EXPECT_LE(version, k1p0);
  EXPECT_LE(k0p1, k1p0);

  EXPECT_GT(version, kDefaultConstructed);
  EXPECT_GT(k1p0, k0p1);

  EXPECT_GE(version, kDefaultConstructed);
  EXPECT_GE(k0p1, version);
  EXPECT_GE(k1p0, k0p1);
}

}  // namespace
