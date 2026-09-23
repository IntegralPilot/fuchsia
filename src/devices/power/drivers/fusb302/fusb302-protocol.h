// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_PROTOCOL_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_PROTOCOL_H_

// The comments in this file reference the USB Power Delivery Specification,
// downloadable at https://usb.org/document-library/usb-power-delivery
//
// usbpd3.1 is Revision 3.1, Version 1.7, published January 2023.

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cstdint>

#include <fbl/ring_buffer.h>

#include "src/devices/power/drivers/fusb302/fusb302-fifos.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"

namespace fusb302 {

// Tracks the acknowledgement of the last transmitted message.
enum class TransmissionState : uint8_t {
  // Waiting for GoodCRC on last transmitted message.
  //
  // The message may be re-transmitted, according to the PD spec. Transmit()
  // must not be called in this state.
  kPending,

  // Timed out waiting for GoodCRC on last transmitted message.
  kTimedOut,

  // Last transmitted message was received successfully.
  kSuccess,
};

// Method used to generate or track GoodCRC replies to incoming USB PD packets.
enum class GoodCrcGenerationMode : uint8_t {
  // No hardware-accelerated GoodCRC generation is available.
  //
  // In this mode, the software USB PD Protocol Layer implementation generates
  // GoodCRC packets and drives the hardware to transmit them.
  //
  // This mode assumes that the hardware is not configured to generate any
  // packets. For example, the FUSB302B's AutoCRC functionality would be
  // disabled.
  //
  // This mode has proven to be too slow when the fusb302 Fuchsia driver is used
  // with the FUSB302B chip on the Khadas VIM3 board. The logic may be useful in
  // an RTOS environment.
  kSoftware,

  // Hardware-accelerated GoodCRC generation blocks PD packet transmission.
  //
  // In this mode, the software USB PD Protocol Layer implementation waits to be
  // notified that the hardware-accelerated GoodCRC generation and transmission
  // is done, before driving the transmission of a PD packet.
  //
  // Waiting ensures that all PD packets are transmitted after the GoodCRCs
  // acknowledging any packets that they may reply to. Waiting comes at the cost
  // of delaying transmissions until the hardware-accelerated GoodCRC
  // notifications are delivered to the driver.
  kTracked,

  // Hardware-accelerated GoodCRC generation is not tracked by software.
  //
  // In this mode, the software USB PD Protocol Layer implementation assumes
  // that hardware-accelerated GoodCRC generation is enabled, and that it
  // completes before the software attempts to transmit a new PD packet.
  kAssumed,
};

// FUSB302-specific implementation of the USB PD Protocol Layer.
//
// The FUSB302 hardware can take on some aspects of the PD Protocol Layer. This
// class is responsible for a complete implementation, while delegating some
// parts to hardware.
//
// Receiving messages works as follows:
// * The hardware control logic is responsible for calling DrainReceiveFifo()
//   when the hardware signals that its receive FIFO is not empty.
// * The USB PD implementation uses HasUnreadMessage() and FirstUnreadMessage()
//   to process received messages. MarkMessageAsRead() must be called after a
//   message is processed, before transmitting any reply to the message.
// * The hardware control logic is responsible for calling
//   DidTransmitHardwareGeneratedGoodCrc() when the hardware signals that it
//   generated and transmitted a GoodCRC reply for a received PD packet.
//
// Transmitting messages works as follows:
// * next_transmitted_message_id() produces the MessageID to be used in the
//   usb_pd::Header constructor for a usb_pd::Message.
// * Transmit() drives the hardware to transmit a usb_pd::Message over the PD
//   connection.
// * DrainReceiveFifo(), documented in the reception section above, tracks
//   the USB PD connection partner's acknowledgement of Transmit() messages.
// * The hardware control logic is responsible for calling
//   DidTimeoutWaitingForGoodCrc() when a message transmitted via Transmit()
//   is not acknowledged (via GoodCRC) by the other side of the USB PD
//   connection within the time mandated by the USB PD specification.
//
// The implementation supports 3 integration models with a hardware-accelerated
// GoodCRC generation module, which map to the 3 member variables of the
// `GoodCrcGenerationMode` enum.
//
// * kSoftware - MarkMessageAsRead() generates a GoodCRC packet for the returned
//   packet, and drives the hardware to transmit it
// * kTracked - DidTransmitHardwareGeneratedGoodCrc() is called when the
//   hardware generates and transmits a GoodCRC packet; Transmit() queues its
//   argument for transmission if any previously received packet lacks a GoodCRC
//   reply; DidTransmitHardwareGeneratedGoodCrc() transmits any queued packet
// * kAssumed - GoodCRC generation is completely ignored by the software
//   implementation
class Fusb302Protocol {
 public:
  // `fifos` must remain alive throughout the new instance's lifetime.
  explicit Fusb302Protocol(GoodCrcGenerationMode good_crc_generation_mode, Fusb302Fifos& fifos);

  Fusb302Protocol(const Fusb302Protocol&) = delete;
  Fusb302Protocol& operator=(const Fusb302Protocol&) = delete;

  // Trivially destructible.
  ~Fusb302Protocol() = default;

  // See `TransmissionState` member comments.
  TransmissionState transmission_state() const { return transmission_state_; }

  // True if the unread queue is not empty.
  //
  // All the messages in the unread queue must be processed and acknowledged via
  // `MarkMessageAsRead()` before `DrainReceiveFifo()` is called.
  bool HasUnreadMessage() const { return !received_message_queue_.empty(); }

  // Returns the first message in the unread messages queue.
  //
  // `HasUnreadMessage()` must be true.
  const usb_pd::Message& FirstUnreadMessage() {
    ZX_DEBUG_ASSERT(HasUnreadMessage());
    return received_message_queue_.front();
  }

  // Removes a message from the unread queue. Transmits a GoodCRC if neceesary.
  //
  // `HasUnreadMessage()` must be true.
  //
  // Returns an error if an I/O error occurred while transmitting a GoodCRC
  // acknowledging the read message.
  zx::result<> MarkMessageAsRead();

  // Not meaningful while `transmission_state()` is `kPending`.
  usb_pd::MessageId next_transmitted_message_id() const {
    ZX_DEBUG_ASSERT(transmission_state_ != TransmissionState::kPending);
    return next_transmitted_message_id_;
  }

  // Reads any PD messages that may be pending in the Rx (receive) FIFO.
  //
  // Returns an error if retrieving the PD message from the PHY layer encounters
  // an I/O error. Otherwise, performs PD Protocol Layer processing (mostly
  // MessageID validation), and updates (TBD: queue name).
  zx::result<> DrainReceiveFifo();

  // Transmits a PD message.
  //
  // `message` must not be a GoodCRC. MarkMessageAsRead() dispatches any
  // necessary GoodCRC message internally.
  //
  // `message`'s MessageID header field must equal
  // `next_transmitted_message_id()`.
  //
  // Must not be called while `transmission_status()` is `kPending`. This is
  // because PD messages (with the excepton of GoodCRC) form a synchronous
  // stream that is blocked on the other side's GoodCRC acknowledgements.
  zx::result<> Transmit(const usb_pd::Message& message);

  // The template will be used as-is (modulo MessageID) for GoodCRC messages.
  void SetGoodCrcTemplate(usb_pd::Header good_crc_template) {
    good_crc_template_ = good_crc_template;
  }

  // PD protocol layer reset.
  //
  // This method can be used when a new Type C connection is established, or
  // right before sending a Soft Reset message.
  //
  // `DidReceiveSoftReset()` must be called instead of this method when a Soft
  // Reset message is received, because that situation requires different
  // initial values for MessageID counters.
  void FullReset();

  // PD protocol layer reset, used after receiving a Soft Reset packet.
  //
  // This must only be used for a soft reset initiated by a port partner
  // message. Incorrect use will result in incorrect initial MessageID values,
  // which will break future communication.
  void DidReceiveSoftReset();

  // Hardware-side PD protocol layer says it gave up waiting for a GoodCRC.
  //
  // This signal comes from the interrupt unit.
  void DidTimeoutWaitingForGoodCrc();

  // Hardware-side PD protocol layer says it replied with a GoodCRC message.
  //
  // This signal comes from the interrupt unit.
  void DidTransmitHardwareGeneratedGoodCrc();

  // If false, `DidTransmitHardwareGeneratedGoodCrc()` must never be called.
  bool UsesHardwareAcceleratedGoodCrcNotifications() const {
    return good_crc_generation_mode_ == GoodCrcGenerationMode::kTracked;
  }

 private:
  // Prepare `good_crc_template_` for transmission.
  //
  // The `good_crc_transmission_pending_` flag will be consumed. (Must be true,
  // will be set to false.)
  void StampGoodCrcTemplate();

  // Reads a PD message out of the Rx (receive) FIFO.
  //
  // Returns an error if retrieving the PD message from the PHY layer encounters
  // an I/O error. Otherwise, performs PD Protocol Layer processing (mostly
  // MessageID validation). The unread message queue will be updated if the
  // message is accepted by the protocol layer.
  void ProcessReceivedMessage(const usb_pd::Message& message);

  // Guaranteed to outlive this instance, because it's owned by this instance's
  // owner (Fusb302).
  Fusb302Fifos& fifos_;

  const GoodCrcGenerationMode good_crc_generation_mode_;

  // The USB PD packet header for the next software-generated GoodCRC.
  //
  // Not used when hardware-generated GoodCRC is used.
  usb_pd::Header good_crc_template_;

  // Received messages that haven't been processed yet.
  //
  // With hardware-generated GoodCRC replies, it's possible to have at least 4
  // messages queued up: a GoodCRC for a Request, two replies (Accept, PS_RDY),
  // and a follow-up query such as Get_Sink_Capabilities.
  fbl::RingBuffer<usb_pd::Message, 8> received_message_queue_;

  // Message that must be transmitted after a hardware-generated GoodCRC.
  //
  // Only used when the software USB PD protocol implementation tracks
  // hardware-generated GoodCRC replies.
  std::optional<usb_pd::Message> queued_transmission_;

  // If `transmission_state` is `kPending`, this MessageID was used, and we're
  // waiting for a GoodCRC. Otherwise, this is MessageID will be used for the
  // next transmitted message.
  usb_pd::MessageId next_transmitted_message_id_;

  // The expected MessageID for the next packet from the other side, if known.
  //
  // This PD protocol implementation supports being started up in the middle of
  // a PD packet stream, where the other side has already sent some packets.
  // This support is absolutely necessary for hardware that automatically
  // generates GoodCRC packets (such as the FUSB302B), because sending a GoodCRC
  // acknowledges the original packet and commits us to replying to it.
  //
  // We "lock onto" the first valid PD packet we receive, which determines the
  // next expected MessageID. After this first packet, we only accept packets
  // with correct MessageID sequence numbers.
  //
  // If `good_crc_transmission_pending_` is true, we're waiting to send a
  // GoodCRC for the last received message (which has the MessageID).
  std::optional<usb_pd::MessageId> next_expected_message_id_;

  // Only used when waiting on notifications for hardware-generated GoodCRC.
  bool good_crc_transmission_pending_ = false;

  TransmissionState transmission_state_ = TransmissionState::kSuccess;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_PROTOCOL_H_
