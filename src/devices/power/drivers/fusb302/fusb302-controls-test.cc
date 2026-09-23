// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-controls.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>
#include <zircon/errors.h>

#include <cstdint>
#include <optional>
#include <utility>

#include <gtest/gtest.h>

#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/lib/testing/predicates/status.h"

namespace fusb302 {

namespace {

// Register addresses from Table 16 "Register Definitions" on page 18 of the
// Rev 5 datasheet.
constexpr int kSwitches0Address = 0x02;
constexpr int kSwitches1Address = 0x03;
constexpr int kMeasureAddress = 0x04;
constexpr int kControl0Address = 0x06;
constexpr int kControl1Address = 0x07;
constexpr int kControl2Address = 0x08;
constexpr int kControl3Address = 0x09;
constexpr int kPowerAddress = 0x0b;
constexpr int kResetAddress = 0x0c;
constexpr int kControl4Address = 0x10;

class Fusb302ControlsTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    mock_i2c_client_ = std::move(endpoints.client);

    EXPECT_OK(loop_.StartThread());
    fidl::BindServer<fuchsia_hardware_i2c::Device>(loop_.dispatcher(), std::move(endpoints.server),
                                                   &mock_i2c_);

    sensors_.emplace(mock_i2c_client_, inspect_.GetRoot().CreateChild("Sensors"));
    controls_.emplace(mock_i2c_client_, sensors_.value(),
                      inspect_.GetRoot().CreateChild("Controls"));
  }

  void TearDown() override { mock_i2c_.VerifyAndClear(); }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  inspect::Inspector inspect_;

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  mock_i2c::MockI2cGtest mock_i2c_;
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> mock_i2c_client_;
  std::optional<Fusb302Sensors> sensors_;
  std::optional<Fusb302Controls> controls_;
};

TEST_F(Fusb302ControlsTest, ResetIntoPowerRoleDiscoveryFromSoftwareResetState) {
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0x03});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0x20});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});

  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x24});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x04});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x06});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x31});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});

  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0x02});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x23});

  EXPECT_OK(controls_->ResetIntoPowerRoleDiscovery());
}

TEST_F(Fusb302ControlsTest, ResetIntoPowerRoleDiscoveryNoWrites) {
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0x03});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0x20});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x04});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x30});
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0x23});

  EXPECT_OK(controls_->ResetIntoPowerRoleDiscovery());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kNone, controls_->wired_cc_pin());
  EXPECT_EQ(usb_pd::PowerRole::kSink, controls_->power_role());
  EXPECT_EQ(usb_pd::DataRole::kUpstreamFacingPort, controls_->data_role());
  EXPECT_EQ(usb_pd::SpecRevision::kRev2, controls_->spec_revision());
}

TEST_F(Fusb302ControlsTest, ResetIntoPowerRoleDiscoveryAllWrites) {
  // The read values have all the documented bits set. Power's read value is
  // zero, because the tested method sets all the documented bits, and we want
  // to see it changing the register.
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kSwitches0Address, 0x03});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0xf7});
  mock_i2c_.ExpectWriteStop({kSwitches1Address, 0x20});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x04});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWriteStop({kControl1Address, 0x00});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kControl4Address, 0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0xef});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x23});

  EXPECT_OK(controls_->ResetIntoPowerRoleDiscovery());
}

TEST_F(Fusb302ControlsTest, ConfigureAllRolesSinkOnCc1AllWrites) {
  // The read values have all the documented bits set. Power's read value is
  // zero, because the tested method sets all the documented bits, and we want
  // to see it changing the register.
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0xef});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x22});
  mock_i2c_.ExpectWriteStop({kResetAddress, 0x02});
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kSwitches0Address, 0x07});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0xf7});
  mock_i2c_.ExpectWriteStop({kSwitches1Address, 0x25});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x40});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWriteStop({kControl1Address, 0x04});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kControl4Address, 0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x2f});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x73});

  EXPECT_OK(controls_->ConfigureAllRoles(
      usb_pd::ConfigChannelPinSwitch::kCc1, usb_pd::PowerRole::kSink,
      usb_pd::DataRole::kUpstreamFacingPort, usb_pd::SpecRevision::kRev2));

  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kCc1, controls_->wired_cc_pin());
  EXPECT_EQ(usb_pd::PowerRole::kSink, controls_->power_role());
  EXPECT_EQ(usb_pd::DataRole::kUpstreamFacingPort, controls_->data_role());
  EXPECT_EQ(usb_pd::SpecRevision::kRev2, controls_->spec_revision());
}

TEST_F(Fusb302ControlsTest, ConfigureAllRolesSinkOnCc2AllWrites) {
  // The read values have all the documented bits set. Power's read value is
  // zero, because the tested method sets all the documented bits, and we want
  // to see it changing the register. Reset's read value is zero, because we
  // only flip bits 0->1 there.
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0xef});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x22});
  mock_i2c_.ExpectWriteStop({kResetAddress, 0x02});
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kSwitches0Address, 0x0b});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0xf7});
  mock_i2c_.ExpectWriteStop({kSwitches1Address, 0x26});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x40});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWriteStop({kControl1Address, 0x04});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kControl4Address, 0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x73});

  EXPECT_OK(controls_->ConfigureAllRoles(
      usb_pd::ConfigChannelPinSwitch::kCc2, usb_pd::PowerRole::kSink,
      usb_pd::DataRole::kUpstreamFacingPort, usb_pd::SpecRevision::kRev2));

  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kCc2, controls_->wired_cc_pin());
  EXPECT_EQ(usb_pd::PowerRole::kSink, controls_->power_role());
  EXPECT_EQ(usb_pd::DataRole::kUpstreamFacingPort, controls_->data_role());
  EXPECT_EQ(usb_pd::SpecRevision::kRev2, controls_->spec_revision());
}

TEST_F(Fusb302ControlsTest, ConfigureAllRolesTxFlushSlow) {
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0xef});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x22});
  mock_i2c_.ExpectWriteStop({kResetAddress, 0x02});
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kSwitches0Address, 0x07});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0xf7});
  mock_i2c_.ExpectWriteStop({kSwitches1Address, 0x25});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x40});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWriteStop({kControl1Address, 0x04});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kControl4Address, 0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x2f});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x73});

  EXPECT_OK(controls_->ConfigureAllRoles(
      usb_pd::ConfigChannelPinSwitch::kCc1, usb_pd::PowerRole::kSink,
      usb_pd::DataRole::kUpstreamFacingPort, usb_pd::SpecRevision::kRev2));
}

TEST_F(Fusb302ControlsTest, ConfigureAllRolesRxFlushSlow) {
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0xef});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x22});
  mock_i2c_.ExpectWriteStop({kResetAddress, 0x02});
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kSwitches0Address, 0x07});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0xf7});
  mock_i2c_.ExpectWriteStop({kSwitches1Address, 0x25});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x40});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWriteStop({kControl1Address, 0x04});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kControl4Address, 0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x2f});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x73});

  EXPECT_OK(controls_->ConfigureAllRoles(
      usb_pd::ConfigChannelPinSwitch::kCc1, usb_pd::PowerRole::kSink,
      usb_pd::DataRole::kUpstreamFacingPort, usb_pd::SpecRevision::kRev2));
}

TEST_F(Fusb302ControlsTest, ConfigureAllRolesTxFlushFailed) {
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0xef});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x22});
  mock_i2c_.ExpectWriteStop({kResetAddress, 0x02});
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kSwitches0Address, 0x07});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0xf7});
  mock_i2c_.ExpectWriteStop({kSwitches1Address, 0x25});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x40});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWriteStop({kControl1Address, 0x04});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kControl4Address, 0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});

  // Brittle test: This relies an implementation detail (10 retries).
  for (int retry_count = 0; retry_count < 10; ++retry_count) {
    mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  }

  zx::result<> result = controls_->ConfigureAllRoles(
      usb_pd::ConfigChannelPinSwitch::kCc1, usb_pd::PowerRole::kSink,
      usb_pd::DataRole::kUpstreamFacingPort, usb_pd::SpecRevision::kRev2);
  EXPECT_EQ(ZX_ERR_TIMED_OUT, result.status_value());
}

TEST_F(Fusb302ControlsTest, ConfigureAllRolesRxFlushFailed) {
  mock_i2c_.ExpectWrite({kControl2Address}).ExpectReadStop({0xef});
  mock_i2c_.ExpectWriteStop({kControl2Address, 0x22});
  mock_i2c_.ExpectWriteStop({kResetAddress, 0x02});
  mock_i2c_.ExpectWrite({kSwitches0Address}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kSwitches0Address, 0x07});
  mock_i2c_.ExpectWrite({kSwitches1Address}).ExpectReadStop({0xf7});
  mock_i2c_.ExpectWriteStop({kSwitches1Address, 0x25});
  mock_i2c_.ExpectWrite({kPowerAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kPowerAddress, 0x0f});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x6f});
  mock_i2c_.ExpectWriteStop({kControl0Address, 0x40});
  mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  mock_i2c_.ExpectWriteStop({kControl1Address, 0x04});
  mock_i2c_.ExpectWrite({kControl3Address}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kControl3Address, 0x07});
  mock_i2c_.ExpectWrite({kControl4Address}).ExpectReadStop({0x01});
  mock_i2c_.ExpectWriteStop({kControl4Address, 0x00});
  mock_i2c_.ExpectWrite({kMeasureAddress}).ExpectReadStop({0x7f});
  mock_i2c_.ExpectWriteStop({kMeasureAddress, 0x30});
  mock_i2c_.ExpectWrite({kControl0Address}).ExpectReadStop({0x2f});

  // Brittle test: This relies an implementation detail (10 retries).
  for (int retry_count = 0; retry_count < 10; ++retry_count) {
    mock_i2c_.ExpectWrite({kControl1Address}).ExpectReadStop({0x77});
  }

  zx::result<> result = controls_->ConfigureAllRoles(
      usb_pd::ConfigChannelPinSwitch::kCc1, usb_pd::PowerRole::kSink,
      usb_pd::DataRole::kUpstreamFacingPort, usb_pd::SpecRevision::kRev2);
  EXPECT_EQ(ZX_ERR_TIMED_OUT, result.status_value());
}

}  // namespace

}  // namespace fusb302
