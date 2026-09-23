// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/usb-pd-message.h"

#include <cstdint>
#include <type_traits>
#include <utility>

#include <gtest/gtest.h>

#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"

namespace usb_pd {

namespace {

static_assert(std::is_trivially_destructible_v<MessageId>);
static_assert(std::is_trivially_destructible_v<Header>);
static_assert(std::is_trivially_destructible_v<Message>);

TEST(MessageIdTest, RoundTripToUint8t) {
  uint8_t message_id_bits = 5;

  auto cast1_message_id = static_cast<MessageId>(message_id_bits);
  EXPECT_EQ(message_id_bits, static_cast<uint8_t>(cast1_message_id));

  MessageId cast2_message_id(message_id_bits);
  EXPECT_EQ(message_id_bits, static_cast<uint8_t>(cast2_message_id));
}

TEST(MessageIdTest, DefaultConstructor) {
  MessageId message_id;
  EXPECT_EQ(0u, static_cast<uint8_t>(message_id));
}

TEST(MessageIdTest, CopyConstructor) {
  MessageId source_id = static_cast<MessageId>(5);
  MessageId copied_id = source_id;
  EXPECT_EQ(5u, static_cast<uint8_t>(copied_id));
}

TEST(MessageIdTest, CopyAssignment) {
  MessageId copied_id(3);
  MessageId source_id(5);

  copied_id = source_id;
  EXPECT_EQ(5u, static_cast<uint8_t>(copied_id));
}

TEST(MessageIdTest, Equality) {
  MessageId three(3);
  MessageId five(5);

  // This test uses EXPECT_TRUE() so that the operator use is explicit.
  EXPECT_TRUE(three == MessageId(three));
  EXPECT_TRUE(MessageId(five) == five);
  EXPECT_TRUE(three != MessageId(five));
}

TEST(MessageIdTest, NextIncrements) {
  MessageId message_id;

  message_id = message_id.Next();
  EXPECT_EQ(1u, static_cast<uint8_t>(message_id));

  message_id = message_id.Next();
  EXPECT_EQ(2u, static_cast<uint8_t>(message_id));
}

TEST(MessageIdTest, NextRollsOver) {
  MessageId message_id(6);

  message_id = message_id.Next();
  EXPECT_EQ(7u, static_cast<uint8_t>(message_id));

  message_id = message_id.Next();
  EXPECT_EQ(0u, static_cast<uint8_t>(message_id));
}

TEST(MessageIdTest, Reset) {
  MessageId message_id(5);
  message_id.Reset();
  EXPECT_EQ(0u, static_cast<uint8_t>(message_id));
}

TEST(HeaderTest, CreateFromBytes) {
  Header header = Header::CreateFromBytes(0xa1, 0x11);

  static constexpr std::pair<uint8_t, uint8_t> expected_bytes = {0xa1, 0x11};
  EXPECT_EQ(expected_bytes, header.bytes());
}

TEST(HeaderTest, MessageTypeGoodCrc) {
  Header header = Header::CreateFromBytes(0x00, 0x00);
  header.set_message_type(MessageType::kGoodCrc);

  EXPECT_EQ(MessageType::kGoodCrc, header.message_type());
  static constexpr std::pair<uint8_t, uint8_t> expected_bytes = {0b0000'0001, 0b0000'0000};
  EXPECT_EQ(expected_bytes, header.bytes());
}

TEST(HeaderTest, MessageTypeSourceCapabilties) {
  Header header = Header::CreateFromBytes(0x00, 0x00);
  header.set_data_object_count(5);
  header.set_message_type(MessageType::kSourceCapabilities);

  EXPECT_EQ(MessageType::kSourceCapabilities, header.message_type());
  static constexpr std::pair<uint8_t, uint8_t> expected_bytes = {0b0000'0001, 0b0101'0000};
  EXPECT_EQ(expected_bytes, header.bytes());
}

TEST(HeaderTest, MessageId) {
  Header header = Header::CreateFromBytes(0x00, 0x00);
  header.set_message_id(MessageId(0b101));

  EXPECT_EQ(MessageId(0b101), header.message_id());
  static constexpr std::pair<uint8_t, uint8_t> expected_bytes = {0b0000'0000, 0b0000'1010};
  EXPECT_EQ(expected_bytes, header.bytes());
}

TEST(HeaderTest, SourceCapabilitiesOneObject) {
  // From the USB Type C ports of the Framework Gen 12 laptop.
  Header header = Header::CreateFromBytes(0xa1, 0x11);

  EXPECT_EQ(false, header.is_extended());
  EXPECT_EQ(1, header.data_object_count());
  EXPECT_EQ(0u, static_cast<uint8_t>(header.message_id()));
  EXPECT_EQ(PowerRole::kSource, header.power_role());
  EXPECT_EQ(SpecRevision::kRev3, header.spec_revision());
  EXPECT_EQ(DataRole::kDownstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kSourceCapabilities, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(0), header.message_id());

  EXPECT_EQ(4, header.payload_bytes());
  EXPECT_EQ(6, header.message_bytes());
}

TEST(HeaderConcreteParsingTest, GoodCrcFromSink) {
  Header header = Header::CreateFromBytes(0x41, 0x00);

  EXPECT_EQ(PowerRole::kSink, header.power_role());
  EXPECT_EQ(SpecRevision::kRev2, header.spec_revision());
  EXPECT_EQ(DataRole::kUpstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kGoodCrc, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(0), header.message_id());
  EXPECT_EQ(0, header.data_object_count());
}

TEST(HeaderConcreteParsingTest, RequestFromSink) {
  Header header = Header::CreateFromBytes(0x82, 0x10);

  EXPECT_EQ(PowerRole::kSink, header.power_role());
  EXPECT_EQ(SpecRevision::kRev3, header.spec_revision());
  EXPECT_EQ(DataRole::kUpstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kRequestPower, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(0), header.message_id());
  EXPECT_EQ(1, header.data_object_count());
}

TEST(HeaderConcreteParsingTest, GoodCrcFromSourceMessage0) {
  Header header = Header::CreateFromBytes(0x61, 0x01);

  EXPECT_EQ(PowerRole::kSource, header.power_role());
  EXPECT_EQ(SpecRevision::kRev2, header.spec_revision());
  EXPECT_EQ(DataRole::kDownstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kGoodCrc, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(0), header.message_id());
  EXPECT_EQ(0, header.data_object_count());
}

TEST(HeaderConcreteParsingTest, GoodCrcFromSourceMessage1) {
  Header header = Header::CreateFromBytes(0x61, 0x03);

  EXPECT_EQ(PowerRole::kSource, header.power_role());
  EXPECT_EQ(SpecRevision::kRev2, header.spec_revision());
  EXPECT_EQ(DataRole::kDownstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kGoodCrc, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(1), header.message_id());
  EXPECT_EQ(0, header.data_object_count());
}

TEST(HeaderConcreteParsingTest, AcceptFromSource) {
  Header header = Header::CreateFromBytes(0xa3, 0x01);

  EXPECT_EQ(PowerRole::kSource, header.power_role());
  EXPECT_EQ(SpecRevision::kRev3, header.spec_revision());
  EXPECT_EQ(DataRole::kDownstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kAccept, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(0), header.message_id());
  EXPECT_EQ(0, header.data_object_count());
}

TEST(HeaderConcreteParsingTest, SoftResetFromSource) {
  Header header = Header::CreateFromBytes(0xad, 0x01);

  EXPECT_EQ(PowerRole::kSource, header.power_role());
  EXPECT_EQ(SpecRevision::kRev3, header.spec_revision());
  EXPECT_EQ(DataRole::kDownstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kSoftReset, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(0), header.message_id());
  EXPECT_EQ(0, header.data_object_count());
}

TEST(HeaderConcreteParsingTest, GetSourceCapabilitiesFromSink) {
  Header header = Header::CreateFromBytes(0x47, 0x00);

  EXPECT_EQ(PowerRole::kSink, header.power_role());
  EXPECT_EQ(SpecRevision::kRev2, header.spec_revision());
  EXPECT_EQ(DataRole::kUpstreamFacingPort, header.data_role());
  EXPECT_EQ(MessageType::kGetSourceCapabilities, header.message_type());
  EXPECT_EQ(usb_pd::MessageId(0), header.message_id());
  EXPECT_EQ(0, header.data_object_count());
}

TEST(MessageTest, SourceCapabilitiesOneObject) {
  // From the USB Type C ports of the Framework Gen 12 laptop.
  Header header = Header::CreateFromBytes(0xa1, 0x11);
  const uint32_t kDataObjects[] = {0x2701912c};

  Message message(header, kDataObjects);

  EXPECT_EQ(MessageType::kSourceCapabilities, message.header().message_type());

  ASSERT_EQ(1u, message.data_objects().size());
  EXPECT_EQ(0x2701912cu, message.data_objects()[0]);
}

TEST(MessageTest, SourceCapabilitiesMaximumObjects) {
  // From Anker 30W Nano 511 model A2337.
  Header header = Header::CreateFromBytes(0xa1, 0x71);
  const uint32_t kDataObjects[] = {0x0a01912c, 0x0002d12c, 0x0003c0fa, 0x0004b0c8,
                                   0x00064096, 0xc8dc213c, 0xc9402128};

  Message message(header, kDataObjects);
  EXPECT_EQ(MessageType::kSourceCapabilities, message.header().message_type());

  ASSERT_EQ(7u, message.data_objects().size());
  EXPECT_EQ(0x0a01912cu, message.data_objects()[0]);
  EXPECT_EQ(0xc9402128u, message.data_objects()[6]);
}

TEST(MessageTest, GetSourceCapabilities) {
  Message message(MessageType::kGetSourceCapabilities, MessageId(0), PowerRole::kSink,
                  SpecRevision::kRev2, DataRole::kUpstreamFacingPort, {});
  static constexpr std::pair<uint8_t, uint8_t> expected_bytes = {0x47, 0x00};
  EXPECT_EQ(expected_bytes, message.header().bytes());
}

}  // namespace

}  // namespace usb_pd
