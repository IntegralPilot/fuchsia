// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/lib/common/include/common.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>

#include <zxtest/zxtest.h>

namespace block {

class TestWithLogger : public zxtest::Test {
 public:
 protected:
  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(TestWithLogger, CheckIoRangeTest) {
  EXPECT_EQ(CheckIoRange(10, 0, 100, logger_.logger()), ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(CheckIoRange(90, 11, 100, logger_.logger()), ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(CheckIoRange(100, 1, 100, logger_.logger()), ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(CheckIoRange(99, 2, 100, logger_.logger()), ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(CheckIoRange(0, 101, 100, logger_.logger()), ZX_ERR_OUT_OF_RANGE);
  EXPECT_OK(CheckIoRange(0, 1, 100, logger_.logger()));
  EXPECT_OK(CheckIoRange(99, 1, 100, logger_.logger()));
  EXPECT_OK(CheckIoRange(0, 100, 100, logger_.logger()));
}

TEST_F(TestWithLogger, CheckIoRangeMaxTransferTest) {
  EXPECT_EQ(CheckIoRange(0, 26, 100, 25, logger_.logger()), ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(CheckIoRange(99, 2, 100, 25, logger_.logger()), ZX_ERR_OUT_OF_RANGE);
  EXPECT_OK(CheckIoRange(0, 25, 100, 25, logger_.logger()));
}

TEST(EndianTest, BigEndian24Test) {
  uint8_t memory[3] = {};
  EXPECT_OK(WriteToBigEndian24(0x654321, memory));
  EXPECT_EQ(memory[0], 0x65);  // MSB
  EXPECT_EQ(memory[1], 0x43);
  EXPECT_EQ(memory[2], 0x21);  // LSB

  EXPECT_EQ(WriteToBigEndian24(0x1000000, memory), ZX_ERR_OUT_OF_RANGE);

  EXPECT_EQ(ReadFromBigEndian24(memory), 0x654321);
}

TEST(EndianTest, LittleEndian24Test) {
  uint8_t memory[3] = {};
  EXPECT_OK(WriteToLittleEndian24(0x654321, memory));
  EXPECT_EQ(memory[0], 0x21);  // LSB
  EXPECT_EQ(memory[1], 0x43);
  EXPECT_EQ(memory[2], 0x65);  // MSB

  EXPECT_EQ(WriteToLittleEndian24(0x1000000, memory), ZX_ERR_OUT_OF_RANGE);

  EXPECT_EQ(ReadFromLittleEndian24(memory), 0x654321);
}

}  // namespace block
