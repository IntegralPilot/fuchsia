// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/fidl-metadata/i2c.h"

#include <fidl/fuchsia.hardware.i2c.businfo/cpp/wire.h>

#include <zxtest/zxtest.h>

static void check_encodes(const uint32_t bus_id,
                          const cpp20::span<const fidl_metadata::i2c::Channel> i2c_channels) {
  // Encode.
  auto result = fidl_metadata::i2c::I2CChannelsToFidl(bus_id, i2c_channels);
  ASSERT_OK(result.status_value());
  std::vector<uint8_t>& data = result.value();

  // Decode.
  fit::result decoded =
      fidl::InplaceUnpersist<fuchsia_hardware_i2c_businfo::wire::I2CBusMetadata>(cpp20::span(data));
  ASSERT_TRUE(decoded.is_ok(), "%s", decoded.error_value().FormatDescription().c_str());

  auto metadata = *decoded.value();

  // Check everything looks sensible.
  ASSERT_TRUE(metadata.has_bus_id());
  EXPECT_EQ(metadata.bus_id(), bus_id);
  ASSERT_TRUE(metadata.has_channels());
  auto channels = metadata.channels();
  ASSERT_EQ(channels.size(), i2c_channels.size());

  for (size_t i = 0; i < i2c_channels.size(); i++) {
    ASSERT_TRUE(channels[i].has_address());
    ASSERT_EQ(channels[i].address(), i2c_channels[i].address);
    if (i2c_channels[i].did || i2c_channels[i].vid || i2c_channels[i].pid) {
      ASSERT_TRUE(channels[i].has_vid());
      ASSERT_EQ(channels[i].vid(), i2c_channels[i].vid);
      ASSERT_TRUE(channels[i].has_pid());
      ASSERT_EQ(channels[i].pid(), i2c_channels[i].pid);
      ASSERT_TRUE(channels[i].has_did());
      ASSERT_EQ(channels[i].did(), i2c_channels[i].did);
    }
  }
}

TEST(I2cMetadataTest, TestEncodeNoPlatformIDs) {
  static constexpr fidl_metadata::i2c::Channel kI2cChannels[] = {{
      .address = 0x01,
  }};

  ASSERT_NO_FATAL_FAILURE(check_encodes(4, kI2cChannels));
}

TEST(I2cMetadataTest, TestEncodeManyChannels) {
  static constexpr fidl_metadata::i2c::Channel kI2cChannels[] = {
      {
          .address = 0x49,

          .vid = 10,
          .pid = 9,
          .did = 8,
      },
      {
          .address = 0x47,

          .vid = 8,
          .pid = 9,
          .did = 9,
      },
      {
          .address = 0xaa,

          .vid = 0,
          .pid = 0,
          .did = 0,
      },
  };

  ASSERT_NO_FATAL_FAILURE(check_encodes(92, kI2cChannels));
}

TEST(I2cMetadataTest, TestEncodeNoChannels) {
  ASSERT_NO_FATAL_FAILURE(check_encodes(1, cpp20::span<const fidl_metadata::i2c::Channel>()));
}
