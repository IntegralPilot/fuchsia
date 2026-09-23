// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/syscalls.h>

#include <gtest/gtest.h>
#include <mock-mmio-reg/mock-mmio-reg.h>

namespace ddk_mock_test {

TEST(MockMmioReg, CopyFrom) {
  ddk_mock::MockMmioRegRegion reg_region_1(sizeof(uint32_t), 0x100);
  ddk_mock::MockMmioRegRegion reg_region_2(sizeof(uint32_t), 0x100);

  fdf::MmioBuffer dut_1 = reg_region_1.GetMmioBuffer();
  fdf::MmioBuffer dut_2 = reg_region_2.GetMmioBuffer();

  constexpr uint32_t reg_values[] = {0xdb5a95fd, 0xc1c8f880, 0x733c2bed, 0xf74e857c};
  for (size_t i = 0; i < std::size(reg_values); i++) {
    reg_region_1[0x10 + (i * 4)].ExpectRead(reg_values[i]);
    reg_region_2[0x40 + (i * 4)].ExpectWrite(reg_values[i]);
  }

  dut_2.CopyFrom32(dut_1, 0x10, 0x40, 4);

  ASSERT_NO_FATAL_FAILURE(reg_region_1.VerifyAll());
  ASSERT_NO_FATAL_FAILURE(reg_region_2.VerifyAll());
}

TEST(MockMmioReg, GetMmioBuffer) {
  // By using page sized registers we ensure that the VMO sizing does not have any chance of
  // rounding in a way that obscures an error in the constructor.
  constexpr size_t kRegArrayLength = 128;
  const size_t kRegSize = zx_system_get_page_size();
  const size_t kExpectedMmioSize = kRegArrayLength * kRegSize;
  ddk_mock::MockMmioRegRegion reg_region_no_offset(kRegSize, kRegArrayLength);
  ddk_mock::MockMmioRegRegion reg_region_with_offset(kRegSize, kRegArrayLength / 2,
                                                     kRegArrayLength / 2);

  ASSERT_EQ(kExpectedMmioSize, reg_region_no_offset.GetMmioBuffer().get_size());
  ASSERT_EQ(kExpectedMmioSize, reg_region_with_offset.GetMmioBuffer().get_size());
}

TEST(MockMmioReg, View) {
  ddk_mock::MockMmioRegRegion reg_region(sizeof(uint32_t), 0x100);

  fdf::MmioBuffer dut = reg_region.GetMmioBuffer();
  fdf::MmioView dut_view_1 = dut.View(0x40);
  fdf::MmioView dut_view_2 = dut_view_1.View(0x20);

  reg_region[0x20].ExpectRead(0x8ed43ca9).ExpectWrite(0x7a5da8d8);
  reg_region[0x80].ExpectRead(0x5be3254c).ExpectWrite(0x6ba7d0af);
  reg_region[0x60].ExpectRead(0xa1026dfe).ExpectWrite(0x0164bff2);

  EXPECT_EQ(dut.Read32(0x20), 0x8ed43ca9ul);
  EXPECT_EQ(dut_view_1.Read32(0x40), 0x5be3254cul);
  EXPECT_EQ(dut_view_2.Read32(0), 0xa1026dfeul);

  dut.Write32(0x7a5da8d8, 0x20);
  dut_view_1.Write32(0x6ba7d0af, 0x40);
  dut_view_2.Write32(0x0164bff2, 0);

  ASSERT_NO_FATAL_FAILURE(reg_region.VerifyAll());
}

TEST(MockMmioReg, Offset) {
  ddk_mock::MockMmioRegRegion reg_region(sizeof(uint32_t), 0x100, 0x1'0000 / sizeof(uint32_t));

  fdf::MmioBuffer dut = reg_region.GetMmioBuffer();

  reg_region[0x1'0020].ExpectRead(0x8ed43ca9).ExpectWrite(0x7a5da8d8);
  reg_region[0x1'0080].ExpectRead(0x5be3254c).ExpectWrite(0x6ba7d0af);
  reg_region[0x1'0060].ExpectRead(0xa1026dfe).ExpectWrite(0x0164bff2);

  EXPECT_EQ(dut.Read32(0x1'0020), 0x8ed43ca9ul);
  EXPECT_EQ(dut.Read32(0x1'0080), 0x5be3254cul);
  EXPECT_EQ(dut.Read32(0x1'0060), 0xa1026dfeul);

  dut.Write32(0x7a5da8d8, 0x1'0020);
  dut.Write32(0x6ba7d0af, 0x1'0080);
  dut.Write32(0x0164bff2, 0x1'0060);

  ASSERT_NO_FATAL_FAILURE(reg_region.VerifyAll());
}

}  // namespace ddk_mock_test
