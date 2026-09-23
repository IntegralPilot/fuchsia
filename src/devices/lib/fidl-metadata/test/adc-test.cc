// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/fidl-metadata/adc.h"

#include <fidl/fuchsia.hardware.adcimpl/cpp/fidl.h>

#include <zxtest/zxtest.h>

namespace {

static void check_encodes(const cpp20::span<const fidl_metadata::adc::Channel> adc_channels) {
  // Encode.
  auto result = fidl_metadata::adc::AdcChannelsToFidl(adc_channels);
  ASSERT_OK(result.status_value());
  std::vector<uint8_t>& data = result.value();

  // Decode.
  fit::result decoded = fidl::Unpersist<fuchsia_hardware_adcimpl::Metadata>(cpp20::span(data));
  ASSERT_TRUE(decoded.is_ok(), "%s", decoded.error_value().FormatDescription().c_str());

  auto& metadata = decoded.value();

  // Check everything looks sensible.
  ASSERT_TRUE(metadata.channels().has_value());
  auto channels = *metadata.channels();
  ASSERT_EQ(channels.size(), adc_channels.size());

  for (size_t i = 0; i < adc_channels.size(); i++) {
    ASSERT_TRUE(channels[i].idx().has_value());
    EXPECT_EQ(*channels[i].idx(), adc_channels[i].idx);
    ASSERT_TRUE(channels[i].name().has_value());
    EXPECT_STREQ(*channels[i].name(), adc_channels[i].name);
  }
}

TEST(AdcMetadataTest, TestEncode) {
  static fidl_metadata::adc::Channel kAdcChannels[] = {{
      .idx = 0,
      .name = "test",
  }};

  ASSERT_NO_FATAL_FAILURE(check_encodes(kAdcChannels));
}

TEST(AdcMetadataTest, TestEncodeMacro) {
  static fidl_metadata::adc::Channel kAdcChannels[] = {DECL_ADC_CHANNEL(2)};

  ASSERT_NO_FATAL_FAILURE(check_encodes(kAdcChannels));
}

TEST(AdcMetadataTest, TestEncodeMacroWithName) {
  static fidl_metadata::adc::Channel kAdcChannels[] = {DECL_ADC_CHANNEL_WITH_NAME(2, "test")};

  ASSERT_NO_FATAL_FAILURE(check_encodes(kAdcChannels));
}

TEST(AdcMetadataTest, TestEncodeMany) {
  static fidl_metadata::adc::Channel kAdcChannels[] = {
      DECL_ADC_CHANNEL(0), DECL_ADC_CHANNEL_WITH_NAME(2, "test"), DECL_ADC_CHANNEL(6)};

  ASSERT_NO_FATAL_FAILURE(check_encodes(kAdcChannels));
}

TEST(AdcMetadataTest, TestEncodeNone) {
  ASSERT_NO_FATAL_FAILURE(check_encodes(cpp20::span<const fidl_metadata::adc::Channel>()));
}

}  // namespace
