// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver-unit-test/utils.h>

#include <soc/aml-common/aml-ram.h>
#include <zxtest/zxtest.h>

#include "aml-ram.h"

namespace amlogic_ram {

void AmlRam::MeasureBandwidthTest(const std::array<uint32_t, MEMBW_MAX_CHANNELS>& channels,
                                  uint32_t cycles_to_measure) {
  // The test won't work if the worker thread has been started.
  ASSERT_FALSE(thread_.joinable());

  uint32_t channels_enabled = 0u;
  for (size_t ix = 0; ix != MEMBW_MAX_CHANNELS; ++ix) {
    channels_enabled |= (channels[ix] != 0) ? (1u << ix) : 0;
    mmio_.Write32(channels[ix], dmc_offsets_.ctrl1_offset[ix]);
    mmio_.Write32(0xffff, dmc_offsets_.ctrl2_offset[ix]);
  }

  mmio_.Write32(cycles_to_measure, dmc_offsets_.timer_offset);
  mmio_.Write32(channels_enabled | DMC_QOS_ENABLE_CTRL, dmc_offsets_.port_ctrl_offset);

  zx_port_packet_t packet;
  EXPECT_OK(port_.wait(zx::time::infinite(), &packet));
  EXPECT_OK(irq_.ack());
  EXPECT_EQ(packet.key, 0);

  EXPECT_EQ(mmio_.Read32(dmc_offsets_.port_ctrl_offset) & DMC_QOS_CLEAR_CTRL, DMC_QOS_CLEAR_CTRL);
  mmio_.Write32(0x0f | DMC_QOS_CLEAR_CTRL, dmc_offsets_.port_ctrl_offset);
}

class AmlRamHardwareTest : public zxtest::Test {
 public:
  void SetUp() override {
    zx::result device = AmlRam::Create(driver_unit_test::GetParent());
    ASSERT_TRUE(device.is_ok());
    device_ = std::move(device.value());
  }

 protected:
  // Pick numbers that make each test case take about 10 seconds on Sherlock.
  static constexpr uint32_t kUnitTestIterations = 200;
  static constexpr uint32_t kUnitTestIterationsHighPrecision = 10'000;

  // Test values taken from //src/developer/memory/monitor/monitor.cc.
  static constexpr uint32_t kMemCyclesToMeasure = 792'000'000 / 20;
  static constexpr uint64_t kMemCyclesToMeasureHighPrecision = 792'000'000 / 1'000;

  static constexpr std::array<uint32_t, MEMBW_MAX_CHANNELS> kDefaultChannels{
      aml_ram::kDefaultChannelCpu,
      aml_ram::kDefaultChannelGpu,
      aml_ram::kDefaultChannelVDec,
      aml_ram::kDefaultChannelVpu,
  };

  static constexpr std::array<uint32_t, MEMBW_MAX_CHANNELS> kCameraChannels{
      aml_ram::kDefaultChannelCpu,
      aml_ram::kPortIdMipiIsp,
      aml_ram::kPortIdGDC,
      aml_ram::kPortIdGe2D,
  };

  bool DeviceHasCamera() { return device_->device_pid_ == PDEV_PID_AMLOGIC_T931; }
  void MeasureBandwidthTest(const std::array<uint32_t, MEMBW_MAX_CHANNELS>& channels,
                            uint32_t cycles_to_measure) {
    device_->MeasureBandwidthTest(channels, cycles_to_measure);
  }

 private:
  std::unique_ptr<AmlRam> device_;
};

TEST_F(AmlRamHardwareTest, MeasureDefaultChannels) {
  for (uint32_t i = 0; i < kUnitTestIterations; i++) {
    ASSERT_NO_FAILURES(MeasureBandwidthTest(kDefaultChannels, kMemCyclesToMeasure));
  }
}

TEST_F(AmlRamHardwareTest, MeasureCameraChannels) {
  if (!DeviceHasCamera()) {
    ZXTEST_SKIP("Skipping camera channels measurement test");
  }

  for (uint32_t i = 0; i < kUnitTestIterations; i++) {
    ASSERT_NO_FAILURES(MeasureBandwidthTest(kCameraChannels, kMemCyclesToMeasure));
  }
}

TEST_F(AmlRamHardwareTest, MeasureDefaultChannelsHighPrecision) {
  for (uint32_t i = 0; i < kUnitTestIterationsHighPrecision; i++) {
    ASSERT_NO_FAILURES(MeasureBandwidthTest(kDefaultChannels, kMemCyclesToMeasureHighPrecision));
  }
}

TEST_F(AmlRamHardwareTest, MeasureCameraChannelsHighPrecision) {
  if (!DeviceHasCamera()) {
    ZXTEST_SKIP("Skipping camera channels measurement test");
  }

  for (uint32_t i = 0; i < kUnitTestIterationsHighPrecision; i++) {
    ASSERT_NO_FAILURES(MeasureBandwidthTest(kCameraChannels, kMemCyclesToMeasureHighPrecision));
  }
}

}  // namespace amlogic_ram
