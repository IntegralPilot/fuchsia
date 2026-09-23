// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/common.h"

#include <gtest/gtest.h>

namespace amlogic_display {

namespace {

TEST(SetFieldValue32, SetFieldToAllOne) {
  EXPECT_EQ(0xa1'b2'c3'dfu, SetFieldValue32(0xa1'b2'c3'd4u, 0, 4, 0xf));
  EXPECT_EQ(0xa1'b2'c3'f4u, SetFieldValue32(0xa1'b2'c3'd4u, 4, 4, 0xf));
  EXPECT_EQ(0xa1'b2'cf'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 8, 4, 0xf));
  EXPECT_EQ(0xa1'b2'f3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 12, 4, 0xf));
  EXPECT_EQ(0xa1'bf'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 16, 4, 0xf));
  EXPECT_EQ(0xa1'f2'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 20, 4, 0xf));
  EXPECT_EQ(0xaf'b2'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 24, 4, 0xf));
  EXPECT_EQ(0xf1'b2'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 28, 4, 0xf));

  EXPECT_EQ(0xa1'b2'c3'ffu, SetFieldValue32(0xa1'b2'c3'd4u, 0, 8, 0xff));
  EXPECT_EQ(0xa1'b2'ff'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 8, 8, 0xff));
  EXPECT_EQ(0xa1'ff'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 16, 8, 0xff));
  EXPECT_EQ(0xff'b2'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 24, 8, 0xff));

  EXPECT_EQ(0xa1'b2'ff'ffu, SetFieldValue32(0xa1'b2'c3'd4u, 0, 16, 0xff'ff));
  EXPECT_EQ(0xa1'ff'ff'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 8, 16, 0xff'ff));
  EXPECT_EQ(0xff'ff'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 16, 16, 0xff'ff));

  EXPECT_EQ(0xff'ff'ff'ffu, SetFieldValue32(0xa1'b2'c3'd4u, 0, 32, 0xff'ff'ff'ff));
}

TEST(SetFieldValue32, SetFieldToAllZero) {
  EXPECT_EQ(0xa1'b2'c3'd0u, SetFieldValue32(0xa1'b2'c3'd4u, 0, 4, 0));
  EXPECT_EQ(0xa1'b2'c3'04u, SetFieldValue32(0xa1'b2'c3'd4u, 4, 4, 0));
  EXPECT_EQ(0xa1'b2'c0'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 8, 4, 0));
  EXPECT_EQ(0xa1'b2'03'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 12, 4, 0));
  EXPECT_EQ(0xa1'b0'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 16, 4, 0));
  EXPECT_EQ(0xa1'02'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 20, 4, 0));
  EXPECT_EQ(0xa0'b2'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 24, 4, 0));
  EXPECT_EQ(0x01'b2'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 28, 4, 0));

  EXPECT_EQ(0xa1'b2'c3'00u, SetFieldValue32(0xa1'b2'c3'd4u, 0, 8, 0));
  EXPECT_EQ(0xa1'b2'00'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 8, 8, 0));
  EXPECT_EQ(0xa1'00'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 16, 8, 0));
  EXPECT_EQ(0x00'b2'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 24, 8, 0));

  EXPECT_EQ(0xa1'b2'00'00u, SetFieldValue32(0xa1'b2'c3'd4u, 0, 16, 0));
  EXPECT_EQ(0xa1'00'00'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 8, 16, 0));
  EXPECT_EQ(0x00'00'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 16, 16, 0));

  EXPECT_EQ(0x00'00'00'00u, SetFieldValue32(0xa1'b2'c3'd4u, 0, 32, 0));
}

TEST(SetFieldValue32, EndiannessMustMatch) {
  EXPECT_EQ(0xa1'b2'ab'cdu, SetFieldValue32(0xa1'b2'c3'd4u, 0, 16, 0xab'cdu));
  EXPECT_EQ(0xa1'ab'cd'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 8, 16, 0xab'cdu));
  EXPECT_EQ(0xab'cd'c3'd4u, SetFieldValue32(0xa1'b2'c3'd4u, 16, 16, 0xab'cdu));
}

TEST(SetFieldValue32, FieldOfSize32) {
  EXPECT_EQ(0x5e'6f'7a'8bu, SetFieldValue32(0xa1'b2'c3'd4u, 0, 32, 0x5e'6f'7a'8bu));
}

TEST(GetFieldValue32, FieldStartingAtBitZero) {
  EXPECT_EQ(0x00'00'00'04u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 4));
  EXPECT_EQ(0x00'00'00'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 8));
  EXPECT_EQ(0x00'00'03'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 12));
  EXPECT_EQ(0x00'00'c3'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 16));
  EXPECT_EQ(0x00'02'c3'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 20));
  EXPECT_EQ(0x00'b2'c3'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 24));
  EXPECT_EQ(0x01'b2'c3'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 28));
  EXPECT_EQ(0xa1'b2'c3'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 32));
}

TEST(GetFieldValue32, FieldStartingAtNonZeroBits) {
  EXPECT_EQ(0x4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 4));
  EXPECT_EQ(0xdu, GetFieldValue32(0xa1'b2'c3'd4u, 4, 4));
  EXPECT_EQ(0x3u, GetFieldValue32(0xa1'b2'c3'd4u, 8, 4));
  EXPECT_EQ(0xcu, GetFieldValue32(0xa1'b2'c3'd4u, 12, 4));
  EXPECT_EQ(0x2u, GetFieldValue32(0xa1'b2'c3'd4u, 16, 4));
  EXPECT_EQ(0xbu, GetFieldValue32(0xa1'b2'c3'd4u, 20, 4));
  EXPECT_EQ(0x1u, GetFieldValue32(0xa1'b2'c3'd4u, 24, 4));
  EXPECT_EQ(0xau, GetFieldValue32(0xa1'b2'c3'd4u, 28, 4));
}

TEST(GetFieldValue32, EndiannessMustMatch) {
  EXPECT_EQ(0xc3'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 16));
  EXPECT_EQ(0xb2'c3u, GetFieldValue32(0xa1'b2'c3'd4u, 8, 16));
  EXPECT_EQ(0xa1'b2u, GetFieldValue32(0xa1'b2'c3'd4u, 16, 16));
}

TEST(GetFieldValue32, FieldOfSize32) {
  EXPECT_EQ(0xa1'b2'c3'd4u, GetFieldValue32(0xa1'b2'c3'd4u, 0, 32));
}

}  // namespace

}  // namespace amlogic_display
