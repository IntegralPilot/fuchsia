// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/usb-pd-sink-policy.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/stdcompat/span.h>

#include <cstdint>

#include <gtest/gtest.h>

#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-objects.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"

namespace usb_pd {

namespace {

constexpr SinkPolicyInfo kVim3PolicyInfo = {
    .min_voltage_mv = 3'000,
    .max_voltage_mv = 12'000,
    .max_power_mw = 24'000,
};

Message SourceCapabilitiesMessage(cpp20::span<const uint32_t> power_data_objects) {
  return Message(MessageType::kSourceCapabilities, MessageId(0), PowerRole::kSource,
                 SpecRevision::kRev2, DataRole::kDownstreamFacingPort, power_data_objects);
}

uint32_t FixedPowerSupply(int32_t voltage_mv, int32_t current_ma) {
  return static_cast<uint32_t>(
      FixedPowerSupplyData().set_voltage_mv(voltage_mv).set_maximum_current_ma(current_ma));
}

class SinkPolicyTest : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(SinkPolicyTest, GetPowerRequestCommonFields) {
  SinkPolicy sink_policy(kVim3PolicyInfo);

  const uint32_t power_data_objects[] = {
      FixedPowerSupply(5'000, 3'000),
  };
  sink_policy.DidReceiveSourceCapabilities(SourceCapabilitiesMessage(power_data_objects));

  FixedVariableSupplyPowerRequestData fixed_request_data(sink_policy.GetPowerRequest());

  EXPECT_FALSE(fixed_request_data.supports_power_give_back());
  EXPECT_TRUE(fixed_request_data.supports_usb_communications());
  EXPECT_TRUE(fixed_request_data.prefers_waiving_usb_suspend());
  EXPECT_FALSE(fixed_request_data.supports_unchunked_extended_messages());
  EXPECT_FALSE(fixed_request_data.supports_extended_power_range());
}

TEST_F(SinkPolicyTest, GetPowerRequestOnePowerDataObject) {
  SinkPolicy sink_policy(kVim3PolicyInfo);

  const uint32_t power_data_objects[] = {
      FixedPowerSupply(5'000, 3'000),
  };
  sink_policy.DidReceiveSourceCapabilities(SourceCapabilitiesMessage(power_data_objects));

  FixedVariableSupplyPowerRequestData fixed_request_data(sink_policy.GetPowerRequest());

  EXPECT_EQ(1u, fixed_request_data.related_power_data_object_position());
  EXPECT_EQ(3'000, fixed_request_data.limit_current_ma());
  EXPECT_EQ(3'000, fixed_request_data.operating_current_ma());
  EXPECT_FALSE(fixed_request_data.capability_mismatch());
}

TEST_F(SinkPolicyTest, GetPowerRequestHighestPowerWins) {
  SinkPolicy sink_policy(kVim3PolicyInfo);

  const uint32_t power_data_objects[] = {
      FixedPowerSupply(5'000, 2'000),
      FixedPowerSupply(9'000, 2'000),
      FixedPowerSupply(12'000, 2'000),
  };
  sink_policy.DidReceiveSourceCapabilities(SourceCapabilitiesMessage(power_data_objects));

  FixedVariableSupplyPowerRequestData fixed_request_data(sink_policy.GetPowerRequest());

  EXPECT_EQ(3u, fixed_request_data.related_power_data_object_position());
  EXPECT_EQ(2'000, fixed_request_data.limit_current_ma());
  EXPECT_EQ(2'000, fixed_request_data.operating_current_ma());
  EXPECT_FALSE(fixed_request_data.capability_mismatch());
}

TEST_F(SinkPolicyTest, GetPowerRequestCurrentCappedToPowerConsumption) {
  SinkPolicy sink_policy(kVim3PolicyInfo);

  const uint32_t power_data_objects[] = {
      FixedPowerSupply(5'000, 2'500),
      FixedPowerSupply(9'000, 2'500),
      FixedPowerSupply(12'000, 2'500),
  };
  sink_policy.DidReceiveSourceCapabilities(SourceCapabilitiesMessage(power_data_objects));

  FixedVariableSupplyPowerRequestData fixed_request_data(sink_policy.GetPowerRequest());

  EXPECT_EQ(3u, fixed_request_data.related_power_data_object_position());
  EXPECT_EQ(2'000, fixed_request_data.limit_current_ma());
  EXPECT_EQ(2'000, fixed_request_data.operating_current_ma());
  EXPECT_FALSE(fixed_request_data.capability_mismatch());
}

TEST_F(SinkPolicyTest, GetPowerRequestMinimumVoltageThatHitsPowerTarget) {
  SinkPolicy sink_policy(kVim3PolicyInfo);

  const uint32_t power_data_objects[] = {
      FixedPowerSupply(5'000, 3'000),
      FixedPowerSupply(9'000, 3'000),
      FixedPowerSupply(12'000, 3'000),
  };
  sink_policy.DidReceiveSourceCapabilities(SourceCapabilitiesMessage(power_data_objects));

  FixedVariableSupplyPowerRequestData fixed_request_data(sink_policy.GetPowerRequest());

  EXPECT_EQ(2u, fixed_request_data.related_power_data_object_position());
  EXPECT_EQ(2'660, fixed_request_data.limit_current_ma());
  EXPECT_EQ(2'660, fixed_request_data.operating_current_ma());
  EXPECT_FALSE(fixed_request_data.capability_mismatch());
}

TEST_F(SinkPolicyTest, GetPowerRequestIgnoresUnknownTypes) {
  SinkPolicy sink_policy(kVim3PolicyInfo);

  // From Anker 30W Nano 511 model A2337.
  const uint32_t power_data_objects[] = {
      FixedPowerSupply(5'000, 3'000),
      FixedPowerSupply(9'000, 3'000),
      FixedPowerSupply(12'000, 2'500),
      FixedPowerSupply(15'000, 2'000),
      FixedPowerSupply(20'000, 1'500),
      0xc8dc213c,  // PPS 3.30-11.00 V @ 3.00 A
      0xc9402128,  // PPS 3.30-16.00 V @ 2.00 A
  };
  sink_policy.DidReceiveSourceCapabilities(SourceCapabilitiesMessage(power_data_objects));

  FixedVariableSupplyPowerRequestData fixed_request_data(sink_policy.GetPowerRequest());

  EXPECT_EQ(2u, fixed_request_data.related_power_data_object_position());
  EXPECT_EQ(2'660, fixed_request_data.limit_current_ma());
  EXPECT_EQ(2'660, fixed_request_data.operating_current_ma());
  EXPECT_FALSE(fixed_request_data.capability_mismatch());
}

TEST_F(SinkPolicyTest, GetSinkCapabilitiesMultipleObjects) {
  SinkPolicy sink_policy(kVim3PolicyInfo);

  cpp20::span<const uint32_t> sink_capabilities = sink_policy.GetSinkCapabilities();
  ASSERT_EQ(3u, sink_capabilities.size());

  PowerData first_capability(sink_capabilities[0]);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, first_capability.supply_type());
  FixedPowerSupplyData first_fixed_supply(first_capability);
  EXPECT_EQ(5'000, first_fixed_supply.voltage_mv());
  EXPECT_EQ(4'800, first_fixed_supply.maximum_current_ma());

  PowerData second_capability(sink_capabilities[1]);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, second_capability.supply_type());
  FixedPowerSupplyData second_fixed_supply(second_capability);
  EXPECT_EQ(9'000, second_fixed_supply.voltage_mv());
  EXPECT_EQ(2'660, second_fixed_supply.maximum_current_ma());

  PowerData third_capability(sink_capabilities[2]);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, third_capability.supply_type());
  FixedPowerSupplyData third_fixed_supply(third_capability);
  EXPECT_EQ(12'000, third_fixed_supply.voltage_mv());
  EXPECT_EQ(2'000, third_fixed_supply.maximum_current_ma());
}

}  // namespace

}  // namespace usb_pd
