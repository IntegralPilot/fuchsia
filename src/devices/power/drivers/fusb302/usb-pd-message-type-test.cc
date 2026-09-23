// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"

#include <cstdint>

#include <gtest/gtest.h>

namespace usb_pd {

namespace {

TEST(IsDataMessageType, ControlTypes) {
  // Most important types.
  EXPECT_FALSE(IsDataMessageType(MessageType::kGoodCrc));
  EXPECT_FALSE(IsDataMessageType(MessageType::kAccept));
  EXPECT_FALSE(IsDataMessageType(MessageType::kPowerSupplyReady));
  EXPECT_FALSE(IsDataMessageType(MessageType::kSoftReset));

  // Smallest control message type with bit 4 set.
  EXPECT_FALSE(IsDataMessageType(MessageType::kNotSupported));

  // Largest defined control message type.
  EXPECT_FALSE(IsDataMessageType(MessageType::kGetMaximumPdSpecRevision));
}

TEST(IsDataMessageType, DataTypes) {
  // Most important types.
  EXPECT_TRUE(IsDataMessageType(MessageType::kSourceCapabilities));
  EXPECT_TRUE(IsDataMessageType(MessageType::kRequestPower));

  // Largest defined data message type.
  EXPECT_TRUE(IsDataMessageType(MessageType::kVendorDefined));
}

TEST(MessageTypeFromControlMessageType, SelectedInputs) {
  // Most important types.
  EXPECT_EQ(MessageType::kGoodCrc, MessageTypeFromControlMessageType(ControlMessageType::kGoodCrc));
  EXPECT_EQ(MessageType::kAccept, MessageTypeFromControlMessageType(ControlMessageType::kAccept));
  EXPECT_EQ(MessageType::kPowerSupplyReady,
            MessageTypeFromControlMessageType(ControlMessageType::kPowerSupplyReady));
  EXPECT_EQ(MessageType::kSoftReset,
            MessageTypeFromControlMessageType(ControlMessageType::kSoftReset));

  // Smallest control message type with bit 4 set.
  EXPECT_EQ(MessageType::kNotSupported,
            MessageTypeFromControlMessageType(ControlMessageType::kNotSupported));

  // Largest defined control message type.
  EXPECT_EQ(MessageType::kGetMaximumPdSpecRevision,
            MessageTypeFromControlMessageType(ControlMessageType::kGetMaximumPdSpecRevision));
}

TEST(MessageTypeFromDataMessageType, SelectedInputs) {
  // Most important types.
  EXPECT_EQ(MessageType::kSourceCapabilities,
            MessageTypeFromDataMessageType(DataMessageType::kSourceCapabilities));
  EXPECT_EQ(MessageType::kRequestPower,
            MessageTypeFromDataMessageType(DataMessageType::kRequestPower));

  // Largest defined data message type.
  EXPECT_EQ(MessageType::kVendorDefined,
            MessageTypeFromDataMessageType(DataMessageType::kVendorDefined));
}

TEST(ControlMessageTypeFromMessageType, SelectedInputs) {
  // Most important types.
  EXPECT_EQ(ControlMessageType::kGoodCrc, ControlMessageTypeFromMessageType(MessageType::kGoodCrc));
  EXPECT_EQ(ControlMessageType::kAccept, ControlMessageTypeFromMessageType(MessageType::kAccept));
  EXPECT_EQ(ControlMessageType::kPowerSupplyReady,
            ControlMessageTypeFromMessageType(MessageType::kPowerSupplyReady));
  EXPECT_EQ(ControlMessageType::kSoftReset,
            ControlMessageTypeFromMessageType(MessageType::kSoftReset));

  // Smallest control message type with bit 4 set.
  EXPECT_EQ(ControlMessageType::kNotSupported,
            ControlMessageTypeFromMessageType(MessageType::kNotSupported));

  // Largest defined control message type.
  EXPECT_EQ(ControlMessageType::kGetMaximumPdSpecRevision,
            ControlMessageTypeFromMessageType(MessageType::kGetMaximumPdSpecRevision));
}

TEST(DataMessageTypeFromMessageType, SelectedInputs) {
  // Most important types.
  EXPECT_EQ(DataMessageType::kSourceCapabilities,
            DataMessageTypeFromMessageType(MessageType::kSourceCapabilities));
  EXPECT_EQ(DataMessageType::kRequestPower,
            DataMessageTypeFromMessageType(MessageType::kRequestPower));

  // Largest defined data message type.
  EXPECT_EQ(DataMessageType::kVendorDefined,
            DataMessageTypeFromMessageType(MessageType::kVendorDefined));
}

}  // namespace

}  // namespace usb_pd
