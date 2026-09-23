// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/power-regs.h"

#include <gtest/gtest.h>

namespace amlogic_display {

namespace {

TEST(PowerRegsTest, AlwaysOnGeneralPowerSleepAddress) {
  // MMIO address from A311D datasheet section 8.2.3 "EE Top Level Power Modes",
  // Table 8-6 "Power Sequence of VPU", page 88.
  auto general_power_sleep = AlwaysOnGeneralPowerSleep::Get().FromValue(0);
  EXPECT_EQ(0xe8u, general_power_sleep.reg_addr());
}

TEST(PowerRegsTest, AlwaysOnGeneralPowerIsolationAddress) {
  // MMIO address from A311D datasheet section 8.2.3 "EE Top Level Power Modes",
  // Table 8-5 "Power Sequence of DOS", page 88.
  auto general_power_sleep = AlwaysOnGeneralPowerIsolationS905D3::Get().FromValue(0);
  EXPECT_EQ(0xecu, general_power_sleep.reg_addr());
}

TEST(PowerRegsTest, AlwaysOnGeneralPowerStatus) {
  // MMIO address from A311D datasheet section 8.2.3 "EE Top Level Power Modes",
  // Table 8-6 "Power Sequence of VPU", page 88.
  auto general_power_status = AlwaysOnGeneralPowerStatus::Get().FromValue(0);
  EXPECT_EQ(0xf0u, general_power_status.reg_addr());
}

TEST(PowerRegsTest, MemoryPower0Address) {
  // MMIO address from S905D2 datasheet section 6.2.3 "EE Top Level Power
  // Modes", Table 6-4 "Power Sequence of EE Domain", row "HDMI Memory PD",
  // page 84.
  auto memory_power0 = MemoryPower0::Get().FromValue(0);
  EXPECT_EQ(0x100u, memory_power0.reg_addr());
}

TEST(PowerRegsTest, VpuMemoryPower0Address) {
  // MMIO address from S905D2 datasheet section 6.2.3 "EE Top Level Power
  // Modes", Table 6-4 "Power Sequence of EE Domain", row "VPU Memory PD",
  // page 84.
  auto vpu_memory_power0 = VpuMemoryPower0::Get().FromValue(0);
  EXPECT_EQ(0x104u, vpu_memory_power0.reg_addr());
}

TEST(PowerRegsTest, VpuMemoryPower1Address) {
  // MMIO address from S905D2 datasheet section 6.2.3 "EE Top Level Power
  // Modes", Table 6-4 "Power Sequence of EE Domain", row "VPU Memory PD",
  // page 84.
  auto vpu_memory_power1 = VpuMemoryPower1::Get().FromValue(0);
  EXPECT_EQ(0x108u, vpu_memory_power1.reg_addr());
}

TEST(PowerRegsTest, VpuMemoryPower2Address) {
  // MMIO address from A311D datasheet section 8.2.3 "EE Top Level Power Modes",
  // Table 8-6 "Power Sequence of VPU", page 88.
  auto vpu_memory_power2 = VpuMemoryPower2::Get().FromValue(0);
  EXPECT_EQ(0x134u, vpu_memory_power2.reg_addr());
}

}  // namespace

}  // namespace amlogic_display
