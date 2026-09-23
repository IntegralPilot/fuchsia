// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "hci_event_handler.h"

#include "logging.h"

namespace bt_hci_intel {
namespace fhbt = fuchsia_hardware_bluetooth;

HciEventHandler::HciEventHandler() = default;

void HciEventHandler::OnReceive(fuchsia_hardware_bluetooth::ReceivedPacket& packet) {
  std::lock_guard guard(queue_lock_);
  switch (packet.Which()) {
    case fhbt::ReceivedPacket::Tag::kIso: {
      infof("Ignore ISO data packet during initialization.");
      return;
    }
    case fhbt::ReceivedPacket::Tag::kEvent: {
      event_queue_.emplace(packet.event().value());
      break;
    }
    case fhbt::ReceivedPacket::Tag::kAcl: {
      event_queue_.emplace(packet.acl().value());
      break;
    }
    default:
      errorf("Unknown packet type: {}", static_cast<uint32_t>(packet.Which()));
  }
  // Assert the completion signal, it won't be de-assert until the queue is empty.
  event_in_queue_.Signal();
}

std::vector<uint8_t> HciEventHandler::WaitForPacket(zx::duration timeout) {
  zx_status_t status = event_in_queue_.Wait(timeout);
  if (status != ZX_OK) {
    errorf("Timed out on waiting for packets.");
    // The only error code that could be returned from Wait() is ZX_ERR_TIMED_OUT. Return an empty
    // vector if timed out.
    return std::vector<uint8_t>();
  }
  std::lock_guard guard(queue_lock_);
  auto packet = event_queue_.front();
  event_queue_.pop();
  if (event_queue_.empty()) {
    // No packet left in queue, de-assert the signal, since the queue will be empty after this
    // function returns.
    event_in_queue_.Reset();
  }

  return packet;
}

void HciEventHandler::on_fidl_error(fidl::UnbindInfo error) {
  errorf("HciTransport protocol closed: {}", error.FormatDescription());
}

void HciEventHandler::handle_unknown_event(
    fidl::UnknownEventMetadata<fuchsia_hardware_bluetooth::HciTransport> metadata) {
  warnf("Unknown event from Hci server: {}", metadata.event_ordinal);
}

}  // namespace bt_hci_intel
