// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_HCI_EVENT_HANDLER_H_
#define SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_HCI_EVENT_HANDLER_H_

#include <fidl/fuchsia.hardware.bluetooth/cpp/fidl.h>
#include <lib/sync/cpp/completion.h>

#include <mutex>
#include <queue>

namespace bt_hci_intel {

class HciEventHandler : public fidl::AsyncEventHandler<fuchsia_hardware_bluetooth::HciTransport> {
 public:
  HciEventHandler();

  void OnReceive(fuchsia_hardware_bluetooth::ReceivedPacket&) override;
  void on_fidl_error(fidl::UnbindInfo error) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_hardware_bluetooth::HciTransport> metadata) override;
  // Wait until there's any packet presence in the queue. The vector return value introduces extra
  // data copy but it's acceptable since we only use HciTransport here for initialization.
  std::vector<uint8_t> WaitForPacket(zx::duration timeout);

 private:
  fit::function<void(fuchsia_hardware_bluetooth::ReceivedPacket)> on_receive_callback_;
  std::mutex queue_lock_;
  libsync::Completion event_in_queue_;
  std::queue<std::vector<uint8_t>> event_queue_;
};

}  // namespace bt_hci_intel

#endif  // SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_HCI_EVENT_HANDLER_H_
