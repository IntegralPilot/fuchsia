// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-signals.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>

#include <cstdint>
#include <optional>
#include <utility>

#include <gtest/gtest.h>

#include "src/devices/power/drivers/fusb302/fusb302-fifos.h"
#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"
#include "src/lib/testing/predicates/status.h"

namespace fusb302 {

namespace {

// Register addresses from Table 16 "Register Definitions" on page 18 of the
// Rev 5 datasheet.
constexpr int kMaskAddress = 0x0a;
constexpr int kMaskAAddress = 0x0e;
constexpr int kMaskBAddress = 0x0f;
constexpr int kStatus1AAddress = 0x3d;
constexpr int kInterruptAAddress = 0x3e;
constexpr int kInterruptBAddress = 0x3f;
constexpr int kStatus0Address = 0x40;
constexpr int kStatus1Address = 0x41;
constexpr int kInterruptAddress = 0x42;
constexpr int kFifosAddress = 0x43;

// Tokens from Table 41 "Tokens used in TxFIFO" on page 29 of the Rev 5
// datasheet.
const uint8_t kTxOnTxToken = 0xa1;
const uint8_t kSync1TxToken = 0x12;
const uint8_t kSync2TxToken = 0x13;
const uint8_t kPackSymTxToken = 0x80;
const uint8_t kJamCrcTxToken = 0xff;
const uint8_t kEopTxToken = 0x14;
const uint8_t kTxOffTxToken = 0xfe;

// Tokens from Table 42 "Tokens used in RxFIFO" on page 29 of the Rev 5
// datasheet.
constexpr uint8_t kSopRxToken = 0b1110'0000;

class Fusb302SignalsTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    mock_i2c_client_ = std::move(endpoints.client);

    EXPECT_OK(loop_.StartThread());
    fidl::BindServer<fuchsia_hardware_i2c::Device>(loop_.dispatcher(), std::move(endpoints.server),
                                                   &mock_i2c_);

    sensors_.emplace(mock_i2c_client_, inspect_.GetRoot().CreateChild("Sensors"));
    fifos_.emplace(mock_i2c_client_);
    protocol_.emplace(GoodCrcGenerationMode::kTracked, fifos_.value());
    signals_.emplace(mock_i2c_client_, sensors_.value(), protocol_.value());
  }

  void TearDown() override { mock_i2c_.VerifyAndClear(); }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  inspect::Inspector inspect_;

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  mock_i2c::MockI2cGtest mock_i2c_;
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> mock_i2c_client_;
  std::optional<Fusb302Sensors> sensors_;
  std::optional<Fusb302Fifos> fifos_;
  std::optional<Fusb302Protocol> protocol_;
  std::optional<Fusb302Signals> signals_;
};

TEST_F(Fusb302SignalsTest, InitInterruptUnitAllWrites) {
  mock_i2c_.ExpectWriteStop({kMaskAddress, 0x44});
  mock_i2c_.ExpectWriteStop({kMaskAAddress, 0x26});

  mock_i2c_.ExpectWrite({kMaskBAddress}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kMaskBAddress, 0xfe});

  // Check that pending interrupts are discarded.
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0xff});

  EXPECT_OK(signals_->InitInterruptUnit());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsNoOp) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsVbusChange) {
  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kCc1);

  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x80});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Sensors::UpdateComparatorsResult()
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(true, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  EXPECT_EQ(true, sensors_->vbus_power_good());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsVbusChangeNoOp) {
  sensors_->SetConfiguration(usb_pd::PowerRole::kSink, usb_pd::ConfigChannelPinSwitch::kNone);

  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x80});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Sensors::UpdateComparatorsResult()
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x00});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  EXPECT_EQ(false, sensors_->vbus_power_good());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsPowerRoleDetectionFinish) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x40});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Sensors::UpdatePowerRoleDetectionResult()
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x28});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(true, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  EXPECT_EQ(PowerRoleDetectionState::kSinkOnCC1, sensors_->power_role_detection_state());
  EXPECT_EQ(usb_pd::PowerRole::kSink, sensors_->detected_power_role());
  EXPECT_EQ(usb_pd::ConfigChannelPinSwitch::kCc1, sensors_->detected_wired_cc_pin());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsPowerRoleDetectionFinishNoOp) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x40});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Sensors::UpdatePowerRoleDetectionResult()
  mock_i2c_.ExpectWrite({kStatus1AAddress}).ExpectReadStop({0x00});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  EXPECT_EQ(PowerRoleDetectionState::kDetecting, sensors_->power_role_detection_state());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsReceivedCorrectCrcSourceCapabilities) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x10});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Fifos::Receive()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0xa1, 0x11});
  mock_i2c_.ExpectWrite({kFifosAddress})
      .ExpectReadStop({0x2c, 0x91, 0x01, 0x27, 0xb1, 0x9b, 0x26, 0x94});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  ASSERT_TRUE(protocol_->HasUnreadMessage());
  EXPECT_EQ(usb_pd::MessageType::kSourceCapabilities,
            protocol_->FirstUnreadMessage().header().message_type());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsReceivedCorrectCrcAndTransmittedGoodCrc) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x10});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x01});

  // Fusb302Fifos::Receive()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0xa1, 0x11});
  mock_i2c_.ExpectWrite({kFifosAddress})
      .ExpectReadStop({0x2c, 0x91, 0x01, 0x27, 0xb1, 0x9b, 0x26, 0x94});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  ASSERT_TRUE(protocol_->HasUnreadMessage());
  EXPECT_EQ(usb_pd::MessageType::kSourceCapabilities,
            protocol_->FirstUnreadMessage().header().message_type());

  // Not expected to generate any I2C activity.
  EXPECT_OK(protocol_->MarkMessageAsRead());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsReceivedCorrectCrcSourceCapabilitiesOutOfOrder) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x10});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Fifos::Receive()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0xa1, 0x13});
  mock_i2c_.ExpectWrite({kFifosAddress})
      .ExpectReadStop({0x2c, 0x91, 0x01, 0x27, 0xb1, 0x9b, 0x26, 0x94});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsReceivedCorrectCrcGoodCrc) {
  // Fusb302Fifos::Transmit()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x08});
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});
  mock_i2c_.ExpectWriteStop({kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken,
                             kSync2TxToken, kPackSymTxToken | 2, 0x87, 0x00, kJamCrcTxToken,
                             kEopTxToken, kTxOffTxToken, kTxOnTxToken});

  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x10});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Fifos::Receive()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0x61, 0x01});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({0x8f, 0x78, 0x38, 0x4a});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});

  EXPECT_OK(protocol_->Transmit(usb_pd::Message(
      usb_pd::MessageType::kGetSourceCapabilities, usb_pd::MessageId(0), usb_pd::PowerRole::kSink,
      usb_pd::SpecRevision::kRev3, usb_pd::DataRole::kUpstreamFacingPort, {})));

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  EXPECT_EQ(TransmissionState::kSuccess, protocol_->transmission_state());
}

TEST_F(Fusb302SignalsTest, ServiceInterruptsReceivedCorrectCrcGoodCrcOutOfOrder) {
  // Fusb302Fifos::Transmit()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x08});
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});
  mock_i2c_.ExpectWriteStop({kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken,
                             kSync2TxToken, kPackSymTxToken | 2, 0x87, 0x00, kJamCrcTxToken,
                             kEopTxToken, kTxOffTxToken, kTxOnTxToken});

  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x10});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Fifos::Receive()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0x61, 0x03});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({0x8f, 0x78, 0x38, 0x4a});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});

  EXPECT_OK(protocol_->Transmit(usb_pd::Message(
      usb_pd::MessageType::kGetSourceCapabilities, usb_pd::MessageId(0), usb_pd::PowerRole::kSink,
      usb_pd::SpecRevision::kRev3, usb_pd::DataRole::kUpstreamFacingPort, {})));

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  EXPECT_EQ(TransmissionState::kPending, protocol_->transmission_state());
}

class Fusb302SignalsHardwareGeneratedGoodCrcAssumedTest : public Fusb302SignalsTest {
 public:
  void SetUp() override {
    Fusb302SignalsTest::SetUp();

    protocol_.emplace(GoodCrcGenerationMode::kAssumed, fifos_.value());
    signals_.emplace(mock_i2c_client_, sensors_.value(), protocol_.value());
  }
};

TEST_F(Fusb302SignalsHardwareGeneratedGoodCrcAssumedTest, InitInterruptUnitAllWrites) {
  mock_i2c_.ExpectWriteStop({kMaskAddress, 0x44});
  mock_i2c_.ExpectWriteStop({kMaskAAddress, 0x26});
  mock_i2c_.ExpectWrite({kMaskBAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWriteStop({kMaskBAddress, 0x01});

  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  EXPECT_OK(signals_->InitInterruptUnit());
}

class Fusb302SignalsHardwareGeneratedGoodCrcTrackedTest : public Fusb302SignalsTest {
 public:
  void SetUp() override {
    Fusb302SignalsTest::SetUp();

    protocol_.emplace(GoodCrcGenerationMode::kTracked, fifos_.value());
    signals_.emplace(mock_i2c_client_, sensors_.value(), protocol_.value());
  }
};

TEST_F(Fusb302SignalsHardwareGeneratedGoodCrcTrackedTest, InitInterruptUnitAllWrites) {
  mock_i2c_.ExpectWriteStop({kMaskAddress, 0x44});
  mock_i2c_.ExpectWriteStop({kMaskAAddress, 0x26});
  mock_i2c_.ExpectWrite({kMaskBAddress}).ExpectReadStop({0xff});
  mock_i2c_.ExpectWriteStop({kMaskBAddress, 0xfe});

  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  EXPECT_OK(signals_->InitInterruptUnit());
}

class Fusb302SignalsSoftwareGeneratedGoodCrcTrackedTest : public Fusb302SignalsTest {
 public:
  void SetUp() override {
    Fusb302SignalsTest::SetUp();

    protocol_.emplace(GoodCrcGenerationMode::kSoftware, fifos_.value());
    signals_.emplace(mock_i2c_client_, sensors_.value(), protocol_.value());
  }
};

TEST_F(Fusb302SignalsSoftwareGeneratedGoodCrcTrackedTest,
       ServiceInterruptsReceivedCorrectCrcFollowedByMarkMessageAsRead) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x10});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Fifos::Receive()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0xa1, 0x11});
  mock_i2c_.ExpectWrite({kFifosAddress})
      .ExpectReadStop({0x2c, 0x91, 0x01, 0x27, 0xb1, 0x9b, 0x26, 0x94});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});

  // Fusb302Fifos::MarkMessageAsRead()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x08});
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});
  mock_i2c_.ExpectWriteStop({kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken,
                             kSync2TxToken, kPackSymTxToken | 2, 0x41, 0x00, kJamCrcTxToken,
                             kEopTxToken, kTxOffTxToken, kTxOnTxToken});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(false, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  ASSERT_TRUE(protocol_->HasUnreadMessage());
  EXPECT_EQ(usb_pd::MessageType::kSourceCapabilities,
            protocol_->FirstUnreadMessage().header().message_type());

  // Verify that ServiceInterrupts() didn't incorrectly report an auto-reply.
  EXPECT_OK(protocol_->MarkMessageAsRead());
}

TEST_F(Fusb302SignalsSoftwareGeneratedGoodCrcTrackedTest,
       ServiceInterruptsReceivedCorrectCrcSoftReset) {
  mock_i2c_.ExpectWrite({kInterruptAddress}).ExpectReadStop({0x10});
  mock_i2c_.ExpectWrite({kInterruptAAddress}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kInterruptBAddress}).ExpectReadStop({0x00});

  // Fusb302Fifos::Receive()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0xad, 0x01});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({0xef, 0x8f, 0x4c, 0x92});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});

  // Fusb302Fifos::MarkMessageAsRead()
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x08});
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});
  mock_i2c_.ExpectWriteStop({kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken,
                             kSync2TxToken, kPackSymTxToken | 2, 0x41, 0x00, kJamCrcTxToken,
                             kEopTxToken, kTxOffTxToken, kTxOnTxToken});

  HardwareStateChanges changes = signals_->ServiceInterrupts();
  EXPECT_EQ(false, changes.port_state_changed);
  EXPECT_EQ(true, changes.received_reset);
  EXPECT_EQ(false, changes.timer_signaled);

  EXPECT_FALSE(protocol_->HasUnreadMessage());
}

}  // namespace

}  // namespace fusb302
