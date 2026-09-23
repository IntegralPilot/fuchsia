// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_ADB_TESTING_CLIENT_ADB_CLIENT_H_
#define SRC_DEVELOPER_ADB_TESTING_CLIENT_ADB_CLIENT_H_

#include <fidl/fuchsia.hardware.usb.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb/cpp/fidl.h>
#include <fidl/fuchsia.testing.adb/cpp/fidl.h>
#include <lib/async/dispatcher.h>

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "src/developer/adb/third_party/adb/adb-protocol.h"

class AdbClientImpl : public fidl::Server<fuchsia_testing_adb::Client>,
                      public fidl::AsyncEventHandler<fuchsia_hardware_usb_endpoint::Endpoint> {
 public:
  explicit AdbClientImpl(async_dispatcher_t* dispatcher);

  void Setup(SetupCompleter::Sync& completer) override;
  void Connect(ConnectCompleter::Sync& completer) override;
  void ExecuteCommand(ExecuteCommandRequest& request,
                      ExecuteCommandCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_testing_adb::Client> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  void OnCompletion(
      fidl::Event<fuchsia_hardware_usb_endpoint::Endpoint::OnCompletion>& event) override;
  void on_fidl_error(fidl::UnbindInfo error) override;

 private:
  zx_status_t DiscoverAndConnect();
  zx_status_t ProcessDevice(fidl::SyncClient<fuchsia_hardware_usb_device::Device>& device,
                            std::string_view instance);
  zx_status_t FindAdbInterface(std::string_view instance, const uint8_t* data, size_t len);
  zx_status_t ConnectEndpoints(std::string_view instance);
  zx_status_t SendPacket(apacket* p);
  zx_status_t QueueReadRequest();

  async_dispatcher_t* dispatcher_;
  bool usb_connected_ = false;
  bool handshake_complete_ = false;
  std::optional<ConnectCompleter::Async> connect_completer_;
  uint8_t bulk_in_addr_ = 0;
  uint8_t bulk_out_addr_ = 0;
  fidl::Client<fuchsia_hardware_usb_endpoint::Endpoint> bulk_in_;
  fidl::Client<fuchsia_hardware_usb_endpoint::Endpoint> bulk_out_;

  std::optional<ExecuteCommandCompleter::Async> execute_completer_;
  std::string command_output_;
  uint32_t local_id_ = 1;
  uint32_t remote_id_ = 0;
  size_t expecting_payload_bytes_ = 0;
};

#endif  // SRC_DEVELOPER_ADB_TESTING_CLIENT_ADB_CLIENT_H_
