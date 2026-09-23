// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/usb-pd-message-objects.h"

#include <cstdint>

#include <gtest/gtest.h>

namespace usb_pd {

namespace {

static_assert(std::is_trivially_destructible_v<PowerData>);
static_assert(std::is_trivially_destructible_v<FixedPowerSupplyData>);
static_assert(std::is_trivially_destructible_v<VariablePowerSupplyData>);
static_assert(std::is_trivially_destructible_v<BatteryPowerSupplyData>);

static_assert(std::is_trivially_destructible_v<PowerRequestData>);
static_assert(std::is_trivially_destructible_v<FixedVariableSupplyPowerRequestData>);
static_assert(std::is_trivially_destructible_v<BatteryPowerRequestData>);

TEST(FixedPowerSupplyDataTest, VoltageMv) {
  {
    FixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_voltage_mv(50);
    EXPECT_EQ(1u, fixed_power_supply.voltage_50mv());
    EXPECT_EQ(50, fixed_power_supply.voltage_mv());
  }
  {
    FixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_voltage_mv(51'150);
    EXPECT_EQ(1'023u, fixed_power_supply.voltage_50mv());
    EXPECT_EQ(51'150, fixed_power_supply.voltage_mv());
  }
}

TEST(FixedPowerSupplyDataTest, MaximumCurrentMa) {
  {
    FixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_maximum_current_ma(10);
    EXPECT_EQ(1u, fixed_power_supply.maximum_current_10ma());
    EXPECT_EQ(10, fixed_power_supply.maximum_current_ma());
  }
  {
    FixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_maximum_current_ma(10'230);
    EXPECT_EQ(1'023u, fixed_power_supply.maximum_current_10ma());
    EXPECT_EQ(10'230, fixed_power_supply.maximum_current_ma());
  }
}

TEST(FixedPowerSupplyDataTest, LaptopPowerSupply) {
  PowerData power_data1(0x0b01912c);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data1.supply_type());

  FixedPowerSupplyData fixed_power_data1(power_data1);
  EXPECT_EQ(5'000, fixed_power_data1.voltage_mv());
  EXPECT_EQ(3'000, fixed_power_data1.maximum_current_ma());
  EXPECT_EQ(PeakCurrentSupport::kNoOverload, fixed_power_data1.peak_current());
  EXPECT_FALSE(fixed_power_data1.supports_dual_role_power());
  EXPECT_FALSE(fixed_power_data1.requires_usb_suspend());
  EXPECT_TRUE(fixed_power_data1.has_unconstrained_power());
  EXPECT_FALSE(fixed_power_data1.supports_usb_communications());
  EXPECT_TRUE(fixed_power_data1.supports_dual_role_data());
  EXPECT_TRUE(fixed_power_data1.supports_unchunked_extended_messages());
  EXPECT_FALSE(fixed_power_data1.supports_extended_power_range());

  PowerData power_data2(0x00002d12c);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data2.supply_type());

  FixedPowerSupplyData fixed_power_data2(power_data2);
  EXPECT_EQ(9'000, fixed_power_data2.voltage_mv());
  EXPECT_EQ(3'000, fixed_power_data2.maximum_current_ma());

  PowerData power_data3(0x0004b12c);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data3.supply_type());

  FixedPowerSupplyData fixed_power_data3(power_data3);
  EXPECT_EQ(15'000, fixed_power_data3.voltage_mv());
  EXPECT_EQ(3'000, fixed_power_data3.maximum_current_ma());

  PowerData power_data4(0x00064145);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data4.supply_type());

  FixedPowerSupplyData fixed_power_data4(power_data4);
  EXPECT_EQ(20'000, fixed_power_data4.voltage_mv());
  EXPECT_EQ(3'250, fixed_power_data4.maximum_current_ma());
}

TEST(FixedPowerSupplyDataTest, FrameworkLaptopPort) {
  PowerData power_data3(0x0004b12c);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data3.supply_type());

  FixedPowerSupplyData fixed_power_data1(PowerData(0x2701912c));
  EXPECT_EQ(5'000, fixed_power_data1.voltage_mv());
  EXPECT_EQ(3'000, fixed_power_data1.maximum_current_ma());
  EXPECT_TRUE(fixed_power_data1.supports_dual_role_power());
  EXPECT_FALSE(fixed_power_data1.requires_usb_suspend());
  EXPECT_FALSE(fixed_power_data1.has_unconstrained_power());
  EXPECT_TRUE(fixed_power_data1.supports_usb_communications());
  EXPECT_TRUE(fixed_power_data1.supports_dual_role_data());
  EXPECT_TRUE(fixed_power_data1.supports_unchunked_extended_messages());
  EXPECT_FALSE(fixed_power_data1.supports_extended_power_range());
}

TEST(FixedPowerSupplyDataTest, AnkerModelA2377) {
  // Anker 30W Nano 511 - Model A2377

  PowerData power_data1(0x0a01912c);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data1.supply_type());

  FixedPowerSupplyData fixed_power_data1(power_data1);
  EXPECT_EQ(5'000, fixed_power_data1.voltage_mv());
  EXPECT_EQ(3'000, fixed_power_data1.maximum_current_ma());
  EXPECT_EQ(PeakCurrentSupport::kNoOverload, fixed_power_data1.peak_current());
  EXPECT_FALSE(fixed_power_data1.supports_dual_role_power());
  EXPECT_FALSE(fixed_power_data1.requires_usb_suspend());
  EXPECT_TRUE(fixed_power_data1.has_unconstrained_power());
  EXPECT_FALSE(fixed_power_data1.supports_usb_communications());
  EXPECT_TRUE(fixed_power_data1.supports_dual_role_data());
  EXPECT_FALSE(fixed_power_data1.supports_unchunked_extended_messages());
  EXPECT_FALSE(fixed_power_data1.supports_extended_power_range());

  PowerData power_data2(0x00002d12c);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data2.supply_type());

  FixedPowerSupplyData fixed_power_data2(power_data2);
  EXPECT_EQ(9'000, fixed_power_data2.voltage_mv());
  EXPECT_EQ(3'000, fixed_power_data2.maximum_current_ma());

  PowerData power_data3(0x0003c0fa);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data3.supply_type());

  FixedPowerSupplyData fixed_power_data3(power_data3);
  EXPECT_EQ(12'000, fixed_power_data3.voltage_mv());
  EXPECT_EQ(2'500, fixed_power_data3.maximum_current_ma());

  PowerData power_data4(0x0004b0c8);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data4.supply_type());

  FixedPowerSupplyData fixed_power_data4(power_data4);
  EXPECT_EQ(15'000, fixed_power_data4.voltage_mv());
  EXPECT_EQ(2'000, fixed_power_data4.maximum_current_ma());

  PowerData power_data5(0x00064096);
  ASSERT_EQ(PowerSupplyType::kFixedSupply, power_data5.supply_type());

  FixedPowerSupplyData fixed_power_data5(power_data5);
  EXPECT_EQ(20'000, fixed_power_data5.voltage_mv());
  EXPECT_EQ(1'500, fixed_power_data5.maximum_current_ma());
}

TEST(AugmentedPowerDataTest, AnkerModelA2377) {
  PowerData power_data6(0xc8dc213c);
  ASSERT_EQ(PowerSupplyType::kAugmentedPowerDataObject, power_data6.supply_type());

  PowerData power_data7(0xc9402128);
  ASSERT_EQ(PowerSupplyType::kAugmentedPowerDataObject, power_data7.supply_type());
}

TEST(VariablePowerSupplyDataTest, MaximumVoltageMv) {
  {
    VariablePowerSupplyData variable_power_supply;
    variable_power_supply.set_maximum_voltage_mv(50);
    EXPECT_EQ(1u, variable_power_supply.maximum_voltage_50mv());
    EXPECT_EQ(50, variable_power_supply.maximum_voltage_mv());
  }
  {
    VariablePowerSupplyData variable_power_supply;
    variable_power_supply.set_maximum_voltage_mv(51'150);
    EXPECT_EQ(1'023u, variable_power_supply.maximum_voltage_50mv());
    EXPECT_EQ(51'150, variable_power_supply.maximum_voltage_mv());
  }
}

TEST(VariablePowerSupplyDataTest, MinimumVoltageMv) {
  {
    VariablePowerSupplyData variable_power_supply;
    variable_power_supply.set_minimum_voltage_mv(50);
    EXPECT_EQ(1u, variable_power_supply.minimum_voltage_50mv());
    EXPECT_EQ(50, variable_power_supply.minimum_voltage_mv());
  }
  {
    VariablePowerSupplyData variable_power_supply;
    variable_power_supply.set_minimum_voltage_mv(51'150);
    EXPECT_EQ(1'023u, variable_power_supply.minimum_voltage_50mv());
    EXPECT_EQ(51'150, variable_power_supply.minimum_voltage_mv());
  }
}

TEST(VariablePowerSupplyDataTest, MaximumCurrentMa) {
  {
    VariablePowerSupplyData variable_power_supply;
    variable_power_supply.set_maximum_current_ma(10);
    EXPECT_EQ(1u, variable_power_supply.maximum_current_10ma());
    EXPECT_EQ(10, variable_power_supply.maximum_current_ma());
  }
  {
    VariablePowerSupplyData variable_power_supply;
    variable_power_supply.set_maximum_current_ma(10'230);
    EXPECT_EQ(1'023u, variable_power_supply.maximum_current_10ma());
    EXPECT_EQ(10'230, variable_power_supply.maximum_current_ma());
  }
}

TEST(BatteryPowerSupplyDataTest, MaximumVoltageMv) {
  {
    BatteryPowerSupplyData battery_power_supply;
    battery_power_supply.set_maximum_voltage_mv(50);
    EXPECT_EQ(1u, battery_power_supply.maximum_voltage_50mv());
    EXPECT_EQ(50, battery_power_supply.maximum_voltage_mv());
  }
  {
    BatteryPowerSupplyData battery_power_supply;
    battery_power_supply.set_maximum_voltage_mv(51'150);
    EXPECT_EQ(1'023u, battery_power_supply.maximum_voltage_50mv());
    EXPECT_EQ(51'150, battery_power_supply.maximum_voltage_mv());
  }
}

TEST(BatteryPowerSupplyDataTest, MinimumVoltageMv) {
  {
    BatteryPowerSupplyData battery_power_supply;
    battery_power_supply.set_minimum_voltage_mv(50);
    EXPECT_EQ(1u, battery_power_supply.minimum_voltage_50mv());
    EXPECT_EQ(50, battery_power_supply.minimum_voltage_mv());
  }
  {
    BatteryPowerSupplyData battery_power_supply;
    battery_power_supply.set_minimum_voltage_mv(51'150);
    EXPECT_EQ(1'023u, battery_power_supply.minimum_voltage_50mv());
    EXPECT_EQ(51'150, battery_power_supply.minimum_voltage_mv());
  }
}

TEST(BatteryPowerSupplyDataTest, MaximumPowerMw) {
  {
    BatteryPowerSupplyData battery_power_supply;
    battery_power_supply.set_maximum_power_mw(250);
    EXPECT_EQ(1u, battery_power_supply.maximum_power_250mw());
    EXPECT_EQ(250, battery_power_supply.maximum_power_mw());
  }
  {
    BatteryPowerSupplyData battery_power_supply;
    battery_power_supply.set_maximum_power_mw(255'750);
    EXPECT_EQ(1'023u, battery_power_supply.maximum_power_250mw());
    EXPECT_EQ(255'750, battery_power_supply.maximum_power_mw());
  }
}

TEST(SinkFixedPowerSupplyDataTest, VoltageMv) {
  {
    SinkFixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_voltage_mv(50);
    EXPECT_EQ(1u, fixed_power_supply.voltage_50mv());
    EXPECT_EQ(50, fixed_power_supply.voltage_mv());
  }
  {
    SinkFixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_voltage_mv(51'150);
    EXPECT_EQ(1'023u, fixed_power_supply.voltage_50mv());
    EXPECT_EQ(51'150, fixed_power_supply.voltage_mv());
  }
}

TEST(SinkFixedPowerSupplyDataTest, MaximumCurrentMa) {
  {
    SinkFixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_maximum_current_ma(10);
    EXPECT_EQ(1u, fixed_power_supply.maximum_current_10ma());
    EXPECT_EQ(10, fixed_power_supply.maximum_current_ma());
  }
  {
    SinkFixedPowerSupplyData fixed_power_supply;
    fixed_power_supply.set_maximum_current_ma(10'230);
    EXPECT_EQ(1'023u, fixed_power_supply.maximum_current_10ma());
    EXPECT_EQ(10'230, fixed_power_supply.maximum_current_ma());
  }
}

TEST(FixedVariableSupplyPowerRequestDataTest, CreateForPosition) {
  FixedVariableSupplyPowerRequestData request_data1 =
      FixedVariableSupplyPowerRequestData::CreateForPosition(1);
  EXPECT_EQ(1u, request_data1.related_power_data_object_position());

  FixedVariableSupplyPowerRequestData request_data7 =
      FixedVariableSupplyPowerRequestData::CreateForPosition(7);
  EXPECT_EQ(7u, request_data7.related_power_data_object_position());
}

TEST(FixedVariableSupplyPowerRequestDataTest, OperatingCurrentMa) {
  {
    FixedVariableSupplyPowerRequestData request_data =
        FixedVariableSupplyPowerRequestData::CreateForPosition(1);
    request_data.set_operating_current_ma(10);
    EXPECT_EQ(1u, request_data.operating_current_10ma());
    EXPECT_EQ(10, request_data.operating_current_ma());
  }

  {
    FixedVariableSupplyPowerRequestData request_data =
        FixedVariableSupplyPowerRequestData::CreateForPosition(1);
    request_data.set_operating_current_ma(10'230);
    EXPECT_EQ(1'023u, request_data.operating_current_10ma());
    EXPECT_EQ(10'230, request_data.operating_current_ma());
  }
}

TEST(FixedVariableSupplyPowerRequestDataTest, LimitCurrentMa) {
  {
    FixedVariableSupplyPowerRequestData request_data =
        FixedVariableSupplyPowerRequestData::CreateForPosition(1);
    request_data.set_limit_current_ma(10);
    EXPECT_EQ(1u, request_data.limit_current_10ma());
    EXPECT_EQ(10, request_data.limit_current_ma());
  }
  {
    FixedVariableSupplyPowerRequestData request_data =
        FixedVariableSupplyPowerRequestData::CreateForPosition(1);
    request_data.set_limit_current_ma(10'230);
    EXPECT_EQ(1'023u, request_data.limit_current_10ma());
    EXPECT_EQ(10'230, request_data.limit_current_ma());
  }
}

TEST(FixedVariableSupplyPowerRequestDataTest, PixelPhoneToLaptopPort) {
  FixedVariableSupplyPowerRequestData request_data(0x1304b12c);

  EXPECT_EQ(3'000, request_data.operating_current_ma());
  EXPECT_EQ(3'000, request_data.limit_current_ma());
  EXPECT_EQ(1u, request_data.related_power_data_object_position());
  EXPECT_EQ(false, request_data.supports_power_give_back());
  EXPECT_EQ(false, request_data.capability_mismatch());
  EXPECT_EQ(true, request_data.supports_usb_communications());
  EXPECT_EQ(true, request_data.prefers_waiving_usb_suspend());
  EXPECT_EQ(false, request_data.supports_unchunked_extended_messages());
  EXPECT_EQ(false, request_data.supports_extended_power_range());
}

TEST(BatteryPowerRequestDataTest, OperatingPowerMw) {
  {
    BatteryPowerRequestData request_data = BatteryPowerRequestData::CreateForPosition(1);
    request_data.set_operating_power_mw(250);
    EXPECT_EQ(1u, request_data.operating_power_250mw());
    EXPECT_EQ(250, request_data.operating_power_mw());
  }
  {
    BatteryPowerRequestData request_data = BatteryPowerRequestData::CreateForPosition(1);
    request_data.set_operating_power_mw(255'750);
    EXPECT_EQ(1'023u, request_data.operating_power_250mw());
    EXPECT_EQ(255'750, request_data.operating_power_mw());
  }
}

TEST(BatteryPowerRequestDataTest, LimitPowerMw) {
  {
    BatteryPowerRequestData request_data = BatteryPowerRequestData::CreateForPosition(1);
    request_data.set_limit_power_mw(250);
    EXPECT_EQ(1u, request_data.limit_power_250mw());
    EXPECT_EQ(250, request_data.limit_power_mw());
  }
  {
    BatteryPowerRequestData request_data = BatteryPowerRequestData::CreateForPosition(1);
    request_data.set_limit_power_mw(255'750);
    EXPECT_EQ(1'023u, request_data.limit_power_250mw());
    EXPECT_EQ(255'750, request_data.limit_power_mw());
  }
}

}  // namespace

}  // namespace usb_pd
