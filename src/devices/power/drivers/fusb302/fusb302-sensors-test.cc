// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>
#include <lib/stdcompat/span.h>

#include <cstdint>
#include <optional>
#include <utility>

#include <gtest/gtest.h>

#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/lib/testing/predicates/status.h"

namespace fusb302 {

namespace {

// Register addresses from Table 16 "Register Definitions" on page 18 of the
// Rev 5 datasheet.
constexpr int kStatus1AAddress = 0x3d;
constexpr int kStatus0Address = 0x40;

class Fusb302SensorsTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    mock_i2c_client_ = std::move(endpoints.client);

    EXPECT_OK(loop_.StartThread());
    fidl::BindServer<fuchsia_hardware_i2c::Device>(loop_.dispatcher(), std::move(endpoints.server),
                                                   &mock_i2c_);

    sensors_.emplace(mock_i2c_client_, inspect_.GetRoot().CreateChild("Sensors"));
  }

  void TearDown() override { mock_i2c_.VerifyAndClear(); }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  inspect::Inspector inspect_;

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  mock_i2c::MockI2cGtest mock_i2c_;
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> mock_i2c_client_;
  std::optional<Fusb302Sensors> sensors_;
};

TEST_F(Fusb302SensorsTest, UpdatePowerRoleDetectionResultRunning) {
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x00});

  EXPECT_EQ(false, sensors_->UpdatePowerRoleDetectionResult());

  EXPECT_EQ(PowerRoleDetectionState::kDetecting, sensors_->power_role_detection_state());
  EXPECT_EQ(usb_pd::PowerRole::kSink, sensors_->detected_power_role());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kNone, sensors_->detected_wired_cc_pin());
}

TEST_F(Fusb302SensorsTest, UpdatePowerRoleDetectionResultSourceOnCc1) {
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x08});

  EXPECT_EQ(true, sensors_->UpdatePowerRoleDetectionResult());

  EXPECT_EQ(PowerRoleDetectionState::kSourceOnCC1, sensors_->power_role_detection_state());
  EXPECT_EQ(usb_pd::PowerRole::kSource, sensors_->detected_power_role());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kCc1, sensors_->detected_wired_cc_pin());
}

TEST_F(Fusb302SensorsTest, UpdatePowerRoleDetectionResultSourceOnCc2) {
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x10});

  EXPECT_EQ(true, sensors_->UpdatePowerRoleDetectionResult());

  EXPECT_EQ(PowerRoleDetectionState::kSourceOnCC2, sensors_->power_role_detection_state());
  EXPECT_EQ(usb_pd::PowerRole::kSource, sensors_->detected_power_role());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kCc2, sensors_->detected_wired_cc_pin());
}

TEST_F(Fusb302SensorsTest, UpdatePowerRoleDetectionResultSinkOnCc1) {
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x28});

  EXPECT_EQ(true, sensors_->UpdatePowerRoleDetectionResult());

  EXPECT_EQ(PowerRoleDetectionState::kSinkOnCC1, sensors_->power_role_detection_state());
  EXPECT_EQ(usb_pd::PowerRole::kSink, sensors_->detected_power_role());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kCc1, sensors_->detected_wired_cc_pin());
}

TEST_F(Fusb302SensorsTest, UpdatePowerRoleDetectionResultSinkOnCc2) {
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x30});

  EXPECT_EQ(true, sensors_->UpdatePowerRoleDetectionResult());

  EXPECT_EQ(PowerRoleDetectionState::kSinkOnCC2, sensors_->power_role_detection_state());
  EXPECT_EQ(usb_pd::PowerRole::kSink, sensors_->detected_power_role());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kCc2, sensors_->detected_wired_cc_pin());
}

TEST_F(Fusb302SensorsTest, UpdatePowerRoleDetectionResultSinkToRunning) {
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x30});
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x00});

  EXPECT_EQ(true, sensors_->UpdatePowerRoleDetectionResult());

  // Make sure that going to "Running" updates the derived values.
  EXPECT_EQ(true, sensors_->UpdatePowerRoleDetectionResult());

  EXPECT_EQ(PowerRoleDetectionState::kDetecting, sensors_->power_role_detection_state());
  EXPECT_EQ(usb_pd::PowerRole::kSink, sensors_->detected_power_role());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kNone, sensors_->detected_wired_cc_pin());
}

TEST_F(Fusb302SensorsTest, UpdateComparatorsResultSinkNoPowerRa) {
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x00});

  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kCc1);
  EXPECT_EQ(true, sensors_->UpdateComparatorsResult());

  EXPECT_EQ(false, sensors_->vbus_power_good());
  EXPECT_EQ(usb_pd::ConfigChannelTermination::kRa, sensors_->cc_termination());
}

TEST_F(Fusb302SensorsTest, UpdateComparatorsResultSinkPowerGoodRa) {
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});

  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kCc1);
  EXPECT_EQ(true, sensors_->UpdateComparatorsResult());

  EXPECT_TRUE(sensors_->vbus_power_good());
  EXPECT_EQ(usb_pd::ConfigChannelTermination::kRa, sensors_->cc_termination());
}

TEST_F(Fusb302SensorsTest, UpdateComparatorsResultSinkPowerGoodRpUsbStandard) {
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x81});

  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kCc1);
  EXPECT_EQ(true, sensors_->UpdateComparatorsResult());

  EXPECT_EQ(true, sensors_->vbus_power_good());
  EXPECT_EQ(usb_pd::ConfigChannelTermination::kRpStandardUsb, sensors_->cc_termination());
}

TEST_F(Fusb302SensorsTest, UpdateComparatorsResultSinkPowerGoodRp1500mA) {
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x82});

  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kCc1);
  EXPECT_EQ(true, sensors_->UpdateComparatorsResult());

  EXPECT_EQ(true, sensors_->vbus_power_good());
  EXPECT_EQ(usb_pd::ConfigChannelTermination::kRp1500mA, sensors_->cc_termination());
}

TEST_F(Fusb302SensorsTest, UpdateComparatorsResultSinkPowerGoodRp3000mA) {
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x83});

  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kCc1);
  EXPECT_EQ(true, sensors_->UpdateComparatorsResult());

  EXPECT_EQ(true, sensors_->vbus_power_good());
  EXPECT_EQ(usb_pd::ConfigChannelTermination::kRp3000mA, sensors_->cc_termination());
}

TEST_F(Fusb302SensorsTest, UpdateComparatorsResultSinkPowerGoodCcOverVoltage) {
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0xa3});

  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kCc1);
  EXPECT_EQ(true, sensors_->UpdateComparatorsResult());

  EXPECT_EQ(true, sensors_->vbus_power_good());
  EXPECT_EQ(usb_pd::ConfigChannelTermination::kUnknown, sensors_->cc_termination());
}

}  // namespace

}  // namespace fusb302
