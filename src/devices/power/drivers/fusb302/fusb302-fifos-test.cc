// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-fifos.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/result.h>

#include <cstdint>
#include <optional>
#include <utility>

#include <gtest/gtest.h>

#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"
#include "src/lib/testing/predicates/status.h"

namespace fusb302 {

namespace {

// Register addresses from Table 16 "Register Definitions" on page 18 of the
// Rev 5 datasheet.
constexpr int kFifosAddress = 0x43;
constexpr int kStatus0Address = 0x40;
constexpr int kStatus1Address = 0x41;

// Tokens from Table 41 "Tokens used in TxFIFO" on page 29 of the Rev 5
// datasheet.
const uint8_t kTxOnTxToken = 0xa1;
const uint8_t kSync1TxToken = 0x12;
const uint8_t kSync2TxToken = 0x13;
// const uint8_t kSync3TxToken = 0x1b;
// const uint8_t kReset1TxToken = 0x15;
// const uint8_t kReset2TxToken = 0x16;
const uint8_t kPackSymTxToken = 0x80;
const uint8_t kJamCrcTxToken = 0xff;
const uint8_t kEopTxToken = 0x14;
const uint8_t kTxOffTxToken = 0xfe;

// Tokens from Table 42 "Tokens used in RxFIFO" on page 29 of the Rev 5
// datasheet.
constexpr uint8_t kSopRxToken = 0b1110'0000;
constexpr uint8_t kSopPrimeRxToken = 0b1100'0000;

class Fusb302FifosTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    mock_i2c_client_ = std::move(endpoints.client);

    EXPECT_OK(loop_.StartThread());
    fidl::BindServer<fuchsia_hardware_i2c::Device>(loop_.dispatcher(), std::move(endpoints.server),
                                                   &mock_i2c_);

    fifos_.emplace(mock_i2c_client_);
  }

  void TearDown() override { mock_i2c_.VerifyAndClear(); }

 protected:
  void MockReceiveFifoEmpty() { mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20}); }

  void MockReceiveFifoNotEmpty() {
    mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x00});
  }

  void MockTransmitFifoEmpty() { mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x08}); }

  void MockNoBmcTransmissionDetected() {
    mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});
  }

  fdf_testing::ScopedGlobalLogger logger_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  mock_i2c::MockI2cGtest mock_i2c_;
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> mock_i2c_client_;
  std::optional<Fusb302Fifos> fifos_;
};

TEST_F(Fusb302FifosTest, TransmitMessageEncodingForControlMessage) {
  MockTransmitFifoEmpty();
  MockNoBmcTransmissionDetected();

  mock_i2c_.ExpectWriteStop({
      kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken, kSync2TxToken,  // Preamble
      kPackSymTxToken | 2, 0x61, 0x05,                                            // Packet data
      kJamCrcTxToken, kEopTxToken, kTxOffTxToken, kTxOnTxToken,                   // Trailer
  });

  usb_pd::Message good_crc(usb_pd::MessageType::kGoodCrc, usb_pd::MessageId(2),
                           usb_pd::PowerRole::kSource, usb_pd::SpecRevision::kRev2,
                           usb_pd::DataRole::kDownstreamFacingPort, {});
  EXPECT_OK(fifos_->TransmitMessage(good_crc));
}

TEST_F(Fusb302FifosTest, TransmitMessageEncodingForDataMessageWithOneObject) {
  MockTransmitFifoEmpty();
  MockNoBmcTransmissionDetected();

  mock_i2c_.ExpectWriteStop({
      kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken, kSync2TxToken,  // Preamble
      kPackSymTxToken | 6, 0x42, 0x1a, 0x2c, 0xb1, 0x04, 0x11,                    // Packet data
      kJamCrcTxToken, kEopTxToken, kTxOffTxToken, kTxOnTxToken,                   // Trailer
  });

  static constexpr uint32_t kPowerRequestDataObjects[] = {0x1104b12c};
  usb_pd::Message request(usb_pd::MessageType::kRequestPower, usb_pd::MessageId(5),
                          usb_pd::PowerRole::kSink, usb_pd::SpecRevision::kRev2,
                          usb_pd::DataRole::kUpstreamFacingPort, kPowerRequestDataObjects);
  EXPECT_OK(fifos_->TransmitMessage(request));
}

TEST_F(Fusb302FifosTest, TransmitMessageEncodingForDataMessageWithMultipleObjects) {
  MockTransmitFifoEmpty();
  MockNoBmcTransmissionDetected();

  // clang-format off
  mock_i2c_.ExpectWriteStop({
      // Preamble
      kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken, kSync2TxToken,
      // Packet data
      kPackSymTxToken | 18, 0xa1, 0x4b, 0x2c, 0x91, 0x01, 0x0b, 0x2c, 0xd1, 0x02, 0x00, 0x2c, 0xb1,
      0x04, 0x00, 0x45, 0x41, 0x06, 0x00,
      // Trailer
      kJamCrcTxToken, kEopTxToken, kTxOffTxToken, kTxOnTxToken,
  });
  // clang-format on

  static constexpr uint32_t kPowerDataObjects[] = {0x0b01912c, 0x0002d12c, 0x0004b12c, 0x00064145};
  usb_pd::Message capabilities(usb_pd::MessageType::kSourceCapabilities, usb_pd::MessageId(5),
                               usb_pd::PowerRole::kSource, usb_pd::SpecRevision::kRev3,
                               usb_pd::DataRole::kDownstreamFacingPort, kPowerDataObjects);
  EXPECT_OK(fifos_->TransmitMessage(capabilities));
}

TEST_F(Fusb302FifosTest, TransmitMessageWithNonEmptyFifo) {
  // Transmit FIFO not empty, other status indicators set.
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x04});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x20});
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x10});

  // Transmit FIFO empty, good to go.
  mock_i2c_.ExpectWrite({kStatus1Address}).ExpectReadStop({0x08});

  MockNoBmcTransmissionDetected();
  mock_i2c_.ExpectWriteStop({
      kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken, kSync2TxToken,  // Preamble
      kPackSymTxToken | 2, 0x61, 0x05,                                            // Packet data
      kJamCrcTxToken, kEopTxToken, kTxOffTxToken, kTxOnTxToken,                   // Trailer
  });

  usb_pd::Message good_crc(usb_pd::MessageType::kGoodCrc, usb_pd::MessageId(2),
                           usb_pd::PowerRole::kSource, usb_pd::SpecRevision::kRev2,
                           usb_pd::DataRole::kDownstreamFacingPort, {});
  EXPECT_OK(fifos_->TransmitMessage(good_crc));
}

TEST_F(Fusb302FifosTest, TransmitMessageWithCcActivity) {
  MockTransmitFifoEmpty();

  // CC activity, VBUS power good flicker (so only CC activity is on).
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0xc0});
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x40});
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0xc0});

  // CC activity gone, good to go.
  mock_i2c_.ExpectWrite({kStatus0Address}).ExpectReadStop({0x80});
  mock_i2c_.ExpectWriteStop({
      kFifosAddress, kSync1TxToken, kSync1TxToken, kSync1TxToken, kSync2TxToken,  // Preamble
      kPackSymTxToken | 2, 0x61, 0x05,                                            // Packet data
      kJamCrcTxToken, kEopTxToken, kTxOffTxToken, kTxOnTxToken,                   // Trailer
  });

  usb_pd::Message good_crc(usb_pd::MessageType::kGoodCrc, usb_pd::MessageId(2),
                           usb_pd::PowerRole::kSource, usb_pd::SpecRevision::kRev2,
                           usb_pd::DataRole::kDownstreamFacingPort, {});
  EXPECT_OK(fifos_->TransmitMessage(good_crc));
}

TEST_F(Fusb302FifosTest, ReadReceivedMessageEmptyFifo) {
  MockReceiveFifoEmpty();

  zx::result<std::optional<usb_pd::Message>> result = fifos_->ReadReceivedMessage();
  ASSERT_OK(result);
  EXPECT_FALSE(result.value().has_value());
}

TEST_F(Fusb302FifosTest, ReadReceivedMessageDecodingForControlMessage) {
  MockReceiveFifoNotEmpty();
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0x61, 0x05});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({0xcd, 0xcd, 0xcd, 0xcd});

  zx::result<std::optional<usb_pd::Message>> result = fifos_->ReadReceivedMessage();
  ASSERT_OK(result);
  ASSERT_TRUE(result.value().has_value());

  EXPECT_FALSE(result->header().is_extended());
  EXPECT_EQ(usb_pd::MessageId(2), result->header().message_id());
  EXPECT_EQ(usb_pd::PowerRole::kSource, result->header().power_role());
  EXPECT_EQ(usb_pd::SpecRevision::kRev2, result->header().spec_revision());
  EXPECT_EQ(usb_pd::DataRole::kDownstreamFacingPort, result->header().data_role());
  EXPECT_EQ(usb_pd::MessageType::kGoodCrc, result->header().message_type());
}

TEST_F(Fusb302FifosTest, RedReceivedMessageDecodingforDataMessageWithOneObject) {
  MockReceiveFifoNotEmpty();
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0x42, 0x1a});
  mock_i2c_.ExpectWrite({kFifosAddress})
      .ExpectReadStop({
          0x2c, 0xb1, 0x04, 0x11,  // Packet data
          0xcd, 0xcd, 0xcd, 0xcd,  // Ignored CRC
      });

  zx::result<std::optional<usb_pd::Message>> result = fifos_->ReadReceivedMessage();
  ASSERT_OK(result);
  ASSERT_TRUE(result.value().has_value());

  EXPECT_FALSE(result->header().is_extended());
  EXPECT_EQ(usb_pd::MessageId(5), result->header().message_id());
  EXPECT_EQ(usb_pd::PowerRole::kSink, result->header().power_role());
  EXPECT_EQ(usb_pd::SpecRevision::kRev2, result->header().spec_revision());
  EXPECT_EQ(usb_pd::DataRole::kUpstreamFacingPort, result->header().data_role());
  EXPECT_EQ(usb_pd::MessageType::kRequestPower, result->header().message_type());

  ASSERT_EQ(1u, result->data_objects().size());
  EXPECT_EQ(0x1104b12cu, result->data_objects()[0]);
}

TEST_F(Fusb302FifosTest, RedReceivedMessageDecodingforDataMessageWithMultipleObjects) {
  MockReceiveFifoNotEmpty();
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopRxToken, 0xa1, 0x4b});
  mock_i2c_.ExpectWrite({kFifosAddress})
      .ExpectReadStop({
          0x2c, 0x91, 0x01, 0x0b, 0x2c, 0xd1, 0x02, 0x00,
          0x2c, 0xb1, 0x04, 0x00, 0x45, 0x41, 0x06, 0x00,  // Packet data
          0xcd, 0xcd, 0xcd, 0xcd,                          // Ignored CRC
      });

  zx::result<std::optional<usb_pd::Message>> result = fifos_->ReadReceivedMessage();
  ASSERT_OK(result);
  ASSERT_TRUE(result.value().has_value());

  EXPECT_FALSE(result->header().is_extended());
  EXPECT_EQ(usb_pd::MessageId(5), result->header().message_id());
  EXPECT_EQ(usb_pd::PowerRole::kSource, result->header().power_role());
  EXPECT_EQ(usb_pd::SpecRevision::kRev3, result->header().spec_revision());
  EXPECT_EQ(usb_pd::DataRole::kDownstreamFacingPort, result->header().data_role());
  EXPECT_EQ(usb_pd::MessageType::kSourceCapabilities, result->header().message_type());

  ASSERT_EQ(4u, result->data_objects().size());
  EXPECT_EQ(0x0b01912cu, result->data_objects()[0]);
  EXPECT_EQ(0x0002d12cu, result->data_objects()[1]);
  EXPECT_EQ(0x0004b12cu, result->data_objects()[2]);
  EXPECT_EQ(0x00064145u, result->data_objects()[3]);
}

TEST_F(Fusb302FifosTest, ReadReceivedMessageNonSopToken) {
  MockReceiveFifoNotEmpty();
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({kSopPrimeRxToken, 0x61, 0x05});
  mock_i2c_.ExpectWrite({kFifosAddress}).ExpectReadStop({0xcd, 0xcd, 0xcd, 0xcd});

  zx::result<std::optional<usb_pd::Message>> result = fifos_->ReadReceivedMessage();
  ASSERT_OK(result);
  EXPECT_FALSE(result.value().has_value());
}

}  // namespace

}  // namespace fusb302
