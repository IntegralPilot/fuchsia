// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-hdmi/ddc-controller-regs.h"

#include <gtest/gtest.h>

namespace designware_hdmi::registers {

namespace {

TEST(DdcControllerReadBufferTest, Get) {
  EXPECT_EQ(DdcControllerReadBuffer::Get(0).addr(), 0x7e20u);
  EXPECT_EQ(DdcControllerReadBuffer::Get(1).addr(), 0x7e21u);
  EXPECT_EQ(DdcControllerReadBuffer::Get(2).addr(), 0x7e22u);
  EXPECT_EQ(DdcControllerReadBuffer::Get(3).addr(), 0x7e23u);
  EXPECT_EQ(DdcControllerReadBuffer::Get(4).addr(), 0x7e24u);
  EXPECT_EQ(DdcControllerReadBuffer::Get(5).addr(), 0x7e25u);
  EXPECT_EQ(DdcControllerReadBuffer::Get(6).addr(), 0x7e26u);
  EXPECT_EQ(DdcControllerReadBuffer::Get(7).addr(), 0x7e27u);
}

}  // namespace

}  // namespace designware_hdmi::registers
