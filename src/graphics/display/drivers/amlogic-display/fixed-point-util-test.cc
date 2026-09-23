// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/fixed-point-util.h"

#include <gtest/gtest.h>

namespace amlogic_display {

TEST(DoubleToU28p4Test, Correctness) {
  EXPECT_EQ(ToU28_4(0.0), 0x0'0u);
  EXPECT_EQ(ToU28_4(1.0), 0x1'0u);
  EXPECT_EQ(ToU28_4(2.0), 0x2'0u);
  EXPECT_EQ(ToU28_4(2.5), 0x2'8u);
  EXPECT_EQ(ToU28_4(3.0), 0x3'0u);
  EXPECT_EQ(ToU28_4(3.5), 0x3'8u);
  EXPECT_EQ(ToU28_4(3.75), 0x3'cu);
  EXPECT_EQ(ToU28_4(4.0), 0x4'0u);
  EXPECT_EQ(ToU28_4(5.0), 0x5'0u);
  EXPECT_EQ(ToU28_4(6.0), 0x6'0u);
  EXPECT_EQ(ToU28_4(6.25), 0x6'4u);
  EXPECT_EQ(ToU28_4(7.0), 0x7'0u);
  EXPECT_EQ(ToU28_4(7.5), 0x7'8u);
  EXPECT_EQ(ToU28_4(12.0), 0xc'0u);
  EXPECT_EQ(ToU28_4(14.0), 0xe'0u);
  EXPECT_EQ(ToU28_4(15.0), 0xf'0u);

  // The maximum number which can be represented in U28.4 format.
  EXPECT_EQ(ToU28_4(268435455.9375), 0xffff'fff'fu);
}

TEST(IntToU28p4Test, Correctness) {
  EXPECT_EQ(ToU28_4(0), 0x0'0u);
  EXPECT_EQ(ToU28_4(1), 0x1'0u);
  EXPECT_EQ(ToU28_4(2), 0x2'0u);
  EXPECT_EQ(ToU28_4(3), 0x3'0u);
  EXPECT_EQ(ToU28_4(4), 0x4'0u);
  EXPECT_EQ(ToU28_4(5), 0x5'0u);
  EXPECT_EQ(ToU28_4(6), 0x6'0u);
  EXPECT_EQ(ToU28_4(7), 0x7'0u);
  EXPECT_EQ(ToU28_4(12), 0xc'0u);
  EXPECT_EQ(ToU28_4(14), 0xe'0u);
  EXPECT_EQ(ToU28_4(15), 0xf'0u);

  // The maximum int number which can be represented in U28.4 format.
  EXPECT_EQ(ToU28_4(0xFFF'FFFF), 0xFFFF'FFF0u);
}

TEST(U28p4ToDouble, ConversionIsPrecise) {
  EXPECT_EQ(U28_4ToDouble(0x0'0), 0.0);
  EXPECT_EQ(U28_4ToDouble(0x1'0), 1.0);
  EXPECT_EQ(U28_4ToDouble(0x2'0), 2.0);
  EXPECT_EQ(U28_4ToDouble(0x2'8), 2.5);
  EXPECT_EQ(U28_4ToDouble(0x3'0), 3.0);
  EXPECT_EQ(U28_4ToDouble(0x3'8), 3.5);
  EXPECT_EQ(U28_4ToDouble(0x3'c), 3.75);
  EXPECT_EQ(U28_4ToDouble(0x4'0), 4.0);
  EXPECT_EQ(U28_4ToDouble(0x5'0), 5.0);
  EXPECT_EQ(U28_4ToDouble(0x6'0), 6.0);
  EXPECT_EQ(U28_4ToDouble(0x6'4), 6.25);
  EXPECT_EQ(U28_4ToDouble(0x7'0), 7.0);
  EXPECT_EQ(U28_4ToDouble(0x7'8), 7.5);
  EXPECT_EQ(U28_4ToDouble(0xc'0), 12.0);
  EXPECT_EQ(U28_4ToDouble(0xe'0), 14.0);
  EXPECT_EQ(U28_4ToDouble(0xf'0), 15.0);

  // The maximum number which can be represented in U28.4 format.
  EXPECT_EQ(U28_4ToDouble(0xffff'fff'f), 268435455.9375);
}

}  // namespace amlogic_display
