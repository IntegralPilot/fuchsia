// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/registers-gmbus.h"

#include <gtest/gtest.h>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace intel_display {

namespace {

TEST(GMBusDataTest, DataGetter) {
  auto gmbus_data = registers::GMBusData::Get().FromValue(0);
  gmbus_data.set_data_byte_0(0x50);
  gmbus_data.set_data_byte_1(0x51);
  gmbus_data.set_data_byte_2(0x52);
  gmbus_data.set_data_byte_3(0x53);

  auto data = gmbus_data.data();
  EXPECT_EQ(4u, data.size());
  EXPECT_EQ(0x50u, data[0]);
  EXPECT_EQ(0x51u, data[1]);
  EXPECT_EQ(0x52u, data[2]);
  EXPECT_EQ(0x53u, data[3]);
}

TEST(GMBusDataTest, DataSetter) {
  auto gmbus_data = registers::GMBusData::Get().FromValue(0);

  // Test writing all 4 bytes.
  {
    std::vector<uint8_t> data = {0x50, 0x51, 0x52, 0x53};
    gmbus_data.set_data(data);
  }
  EXPECT_EQ(0x50u, gmbus_data.data_byte_0());
  EXPECT_EQ(0x51u, gmbus_data.data_byte_1());
  EXPECT_EQ(0x52u, gmbus_data.data_byte_2());
  EXPECT_EQ(0x53u, gmbus_data.data_byte_3());

  // If data size is smaller than 4, only write the first few bytes.
  {
    std::vector<uint8_t> data = {0x94, 0x93};
    gmbus_data.set_data(data);
  }
  EXPECT_EQ(0x94u, gmbus_data.data_byte_0());
  EXPECT_EQ(0x93u, gmbus_data.data_byte_1());
  EXPECT_EQ(0x52u, gmbus_data.data_byte_2());
  EXPECT_EQ(0x53u, gmbus_data.data_byte_3());

  // Data size must not exceed 4.
  {
    std::vector<uint8_t> invalid_data = {0x94, 0x93, 0x92, 0x91, 0x90};
    // Drivers may override the ZX_ASSERT_LEVEL to enable ZX_DEBUG_ASSERT on
    // non-debug builds, so EXPECT_DEBUG_DEATH() macro doesn't work here.
#ifdef ZX_DEBUG_ASSERT_IMPLEMENTED
    EXPECT_DEATH_IF_SUPPORTED(gmbus_data.set_data(invalid_data), "data.size\\(\\) <= 4");
#endif
  }
}

}  // namespace

}  // namespace intel_display
