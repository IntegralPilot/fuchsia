// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-protocol.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <cstdint>
#include <optional>
#include <utility>

#include "src/devices/power/drivers/fusb302/fusb302-fifos.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"

namespace fusb302 {

Fusb302Protocol::Fusb302Protocol(GoodCrcGenerationMode good_crc_generation_mode,
                                 Fusb302Fifos& fifos)
    : fifos_(fifos),
      good_crc_generation_mode_(good_crc_generation_mode),
      good_crc_template_(usb_pd::MessageType::kGoodCrc, /*data_object_count=*/0,
                         usb_pd::MessageId(0), usb_pd::PowerRole::kSink,
                         usb_pd::SpecRevision::kRev2, usb_pd::DataRole::kUpstreamFacingPort),
      next_transmitted_message_id_(0),
      next_expected_message_id_(std::nullopt) {}

zx::result<> Fusb302Protocol::MarkMessageAsRead() {
  ZX_DEBUG_ASSERT(HasUnreadMessage());
  ZX_DEBUG_ASSERT_MSG(next_expected_message_id_.has_value(),
                      "next_expected_message_id_ should be known after having received a message");

  const usb_pd::MessageId read_message_id = received_message_queue_.front().header().message_id();
  received_message_queue_.pop();

  if (!good_crc_transmission_pending_) {
    // Hardware replied with GoodCRC.
    ZX_DEBUG_ASSERT_MSG(good_crc_generation_mode_ != GoodCrcGenerationMode::kSoftware,
                        "Software-generated GoodCRC is only done in MarkMessageAsRead()");
    return zx::ok();
  }

  if (read_message_id != next_expected_message_id_) {
    // There is an unacknowledged message, but it's not this one.
    return zx::ok();
  }

  StampGoodCrcTemplate();

  switch (good_crc_generation_mode_) {
    case GoodCrcGenerationMode::kSoftware: {
      usb_pd::Message good_crc(good_crc_template_, {});
      return fifos_.TransmitMessage(good_crc);
    }
    case GoodCrcGenerationMode::kTracked:
    case GoodCrcGenerationMode::kAssumed:
      return zx::ok();
  }
}

zx::result<> Fusb302Protocol::DrainReceiveFifo() {
  while (true) {
    zx::result<std::optional<usb_pd::Message>> result = fifos_.ReadReceivedMessage();
    if (result.is_error()) {
      return result.take_error();
    }

    if (!result.value().has_value()) {
      return zx::ok();
    }

    ProcessReceivedMessage(result.value().value());
  }
}

void Fusb302Protocol::ProcessReceivedMessage(const usb_pd::Message& message) {
  const usb_pd::Header& header = message.header();
  if (header.message_type() == usb_pd::MessageType::kGoodCrc) {
    // Discard repeated GoodCRCs.
    if (transmission_state_ != TransmissionState::kPending) {
      fdf::warn(
          "PD protocol de-synchronization: discarded GoodCRC with MessageID {}. No unacknowledged message.",
          static_cast<uint8_t>(header.message_id()));
      return;
    }

    if (header.message_id() != next_transmitted_message_id_) {
      fdf::warn(
          "PD protocol de-synchronization: discarded GoodCRC with MessageID {}; while waiting for a GoodCRC for MessageID is {}",
          static_cast<uint8_t>(header.message_id()),
          static_cast<uint8_t>(next_transmitted_message_id_));
      return;
    }

    next_transmitted_message_id_ = next_transmitted_message_id_.Next();
    transmission_state_ = TransmissionState::kSuccess;
    return;
  }

  if (header.message_type() == usb_pd::MessageType::kSoftReset) {
    fdf::warn("PD protocol de-synchronization: received Soft Reset with MessageID {}",
              static_cast<uint8_t>(header.message_id()));

    // usbpd3.1 6.8.1 "Soft Reset and Protocol error" states that the MessageID
    // counter must be reset before sending the Soft Reset / Accept messages in
    // the soft reset sequence. This implies that Soft Reset messages must
    // always have a Message ID of zero.
    if (header.message_id() != usb_pd::MessageId(0)) {
      fdf::warn("Received Soft Reset with non-zero Message ID {}",
                static_cast<uint8_t>(header.message_id()));
    }

    // Both the Source and Sink sub-sections in usbpd3.1 8.3.3.4 "SOP Soft Reset
    // and Protocol Error State Diagrams" mandate that the sender of a Soft
    // Reset waits for an Accept before sending any other message.
    //
    // That being said, resetting PD protocol state here let us recognize the
    // MessageIDs of any messages coming our way from a non-compliant Port
    // partner.
    DidReceiveSoftReset();

    // Drop all messages received before the Soft Reset. It's too late to act on
    // them now, and we have to produce an Accept reply in 15ms / 30ms
    // (tSenderResponse / tReceiverResponse in usbpd3.1 6.6.2 "Sender Response
    // Timer").
    received_message_queue_.clear();

    received_message_queue_.push(message);
    return;
  }

  // Discard repeated messages.
  if (good_crc_transmission_pending_) {
    ZX_DEBUG_ASSERT_MSG(
        next_expected_message_id_.has_value(),
        "next_expected_message_id_ should be known after having received a message");
    if (header.message_id() == next_expected_message_id_.value().Next()) {
      fdf::warn(
          "Received message with MessageID {} while expecting to have to send GoodCRC for Message ID {}. Fixing state, assuming GoodCRC was auto-generated.",
          static_cast<uint8_t>(header.message_id()),
          static_cast<uint8_t>(next_expected_message_id_.value()));
      next_expected_message_id_ = header.message_id();
    } else {
      fdf::warn(
          "PD protocol de-synchronization: discarded message with MessageID {} because we still need to send GoodCRC for MessageID {}",
          static_cast<uint8_t>(header.message_id()),
          static_cast<uint8_t>(next_expected_message_id_.value()));
      return;
    }
  } else {
    if (next_expected_message_id_.has_value()) {
      if (header.message_id() != next_expected_message_id_.value()) {
        fdf::warn(
            "PD re-transmission: discarded message with MessageID {} because next expected MessageID is {}",
            static_cast<uint8_t>(header.message_id()),
            static_cast<uint8_t>(next_expected_message_id_.value()));
        return;
      }
    } else {
      next_expected_message_id_ = header.message_id();
      fdf::info("PD protocol stream started at MessageID {}",
                static_cast<uint8_t>(next_expected_message_id_.value()));
    }
  }

  good_crc_transmission_pending_ = true;

  if (received_message_queue_.full()) {
    fdf::warn("PD received message queue (size {}) full! Dropping oldest message.",
              received_message_queue_.size());
    received_message_queue_.pop();
  }
  received_message_queue_.push(message);
}

zx::result<> Fusb302Protocol::Transmit(const usb_pd::Message& message) {
  ZX_DEBUG_ASSERT(message.header().message_type() != usb_pd::MessageType::kGoodCrc);
  ZX_DEBUG_ASSERT(transmission_state_ != TransmissionState::kPending);
  ZX_DEBUG_ASSERT(message.header().message_id() == next_transmitted_message_id_);

  if (good_crc_generation_mode_ == GoodCrcGenerationMode::kTracked) {
    if (good_crc_transmission_pending_) {
      ZX_DEBUG_ASSERT_MSG(
          !queued_transmission_.has_value(),
          "Attempted to transmit multiple messages before hardware-generated GoodCRC");
      queued_transmission_ = message;
      return zx::ok();
    }
  }

  zx::result<> result = fifos_.TransmitMessage(message);
  if (!result.is_ok()) {
    return result.take_error();
  }
  transmission_state_ = TransmissionState::kPending;
  return zx::ok();
}

void Fusb302Protocol::FullReset() {
  ZX_DEBUG_ASSERT_MSG(!queued_transmission_.has_value() ||
                          good_crc_generation_mode_ == GoodCrcGenerationMode::kTracked,
                      "Transmitted message queued despite not tracking hardware-generated GoodCRC");
  queued_transmission_.reset();

  next_expected_message_id_ = std::nullopt;
  next_transmitted_message_id_.Reset();
  transmission_state_ = TransmissionState::kSuccess;
  good_crc_transmission_pending_ = false;
}

void Fusb302Protocol::DidReceiveSoftReset() {
  ZX_DEBUG_ASSERT_MSG(!queued_transmission_.has_value() ||
                          good_crc_generation_mode_ == GoodCrcGenerationMode::kTracked,
                      "Transmitted message queued despite not tracking hardware-generated GoodCRC");
  queued_transmission_.reset();

  // usbpd3.1 6.8.1 "Soft Reset and Protocol error" states that the MessageID
  // counter must be reset before sending the Soft Reset / Accept messages in
  // the soft reset sequence. This implies that Soft Reset message we received
  // must have had a Message ID of zero.
  next_expected_message_id_ = usb_pd::MessageId(0);

  // Table 8-28 "Steps for a Soft Reset" in the USB PD spec states that the Soft
  // Reset message must be acknowledged via GoodCRC, just like any other
  // message. Table 8-28 is usbpd3.1 8.3.2.5 "Soft Reset" under usbpd3.1 8.3.2
  // "Atomic Message diagrams".
  //
  // We discard any previously pending GoodCRC when we receive a Soft Reset.
  // GoodCRC messages do flow control, and we're about to reset the entire
  // message flow.
  good_crc_transmission_pending_ = true;

  next_transmitted_message_id_.Reset();
  transmission_state_ = TransmissionState::kSuccess;
}

void Fusb302Protocol::DidTimeoutWaitingForGoodCrc() {
  if (transmission_state_ != TransmissionState::kPending) {
    fdf::warn("Hardware PD layer reported GoodCRC timeout, but we weren't expecting any GoodCRC.");
    return;
  }
  transmission_state_ = TransmissionState::kTimedOut;
}

void Fusb302Protocol::DidTransmitHardwareGeneratedGoodCrc() {
  ZX_DEBUG_ASSERT_MSG(
      UsesHardwareAcceleratedGoodCrcNotifications(),
      "Received hardware-generated GoodCRC notification in a mode that does not require it.");

  if (!good_crc_transmission_pending_) {
    fdf::warn("Hardware PD layer reported transmitting a GoodCRC, but we didn't need to send one");
  }

  // We will not be using the GoodCRC template, but stamping also performs all
  // GoodCRC-related state updates.
  StampGoodCrcTemplate();

  if (queued_transmission_.has_value()) {
    ZX_DEBUG_ASSERT_MSG(
        good_crc_generation_mode_ == GoodCrcGenerationMode::kTracked,
        "Transmitted message queueing is only needed when tracking hardware-generated GoodCRC");
    zx::result<> transmit_result = Transmit(queued_transmission_.value());
    queued_transmission_ = std::nullopt;
    if (transmit_result.is_error()) {
      fdf::warn("Failed to transmit queued PD message: {}", transmit_result);
    }
  }
}

void Fusb302Protocol::StampGoodCrcTemplate() {
  ZX_DEBUG_ASSERT(good_crc_transmission_pending_);
  ZX_DEBUG_ASSERT_MSG(next_expected_message_id_.has_value(),
                      "next_expected_message_id_ should be known after having received a message");

  if (good_crc_generation_mode_ == GoodCrcGenerationMode::kSoftware) {
    good_crc_template_.set_message_id(next_expected_message_id_.value());
  }
  next_expected_message_id_ = next_expected_message_id_.value().Next();
  good_crc_transmission_pending_ = false;
}

}  // namespace fusb302
