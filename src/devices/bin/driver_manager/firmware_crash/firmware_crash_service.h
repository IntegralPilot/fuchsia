// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_FIRMWARE_CRASH_FIRMWARE_CRASH_SERVICE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_FIRMWARE_CRASH_FIRMWARE_CRASH_SERVICE_H_

#include <fidl/fuchsia.firmware.crash/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

namespace driver_manager {

class FirmwareCrashService final : public fidl::Server<fuchsia_firmware_crash::Reporter> {
 public:
  explicit FirmwareCrashService(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void Publish(component::OutgoingDirectory& outgoing);

 private:
  // fuchsia.firmware.crash/Reporter>
  void Report(ReportRequest& request, ReportCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_firmware_crash::Reporter> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  class Watcher final : public fidl::Server<fuchsia_firmware_crash::Watcher> {
   public:
    explicit Watcher(FirmwareCrashService* parent,
                     fidl::ServerEnd<fuchsia_firmware_crash::Watcher> request,
                     async_dispatcher_t* dispatcher);

    void NewCrashAvailable();

   private:
    // fuchsia.firmware.crash/Watcher>
    void GetCrash(GetCrashCompleter::Sync& completer) override;
    void handle_unknown_method(
        fidl::UnknownMethodMetadata<fuchsia_firmware_crash::Watcher> metadata,
        fidl::UnknownMethodCompleter::Sync& completer) override {}

    FirmwareCrashService* parent_;
    // Last crash reported to the client.
    size_t crash_index_ = 0;
    // Outstanding completer, waiting for a new crash to occur.
    std::optional<GetCrashCompleter::Async> completer_;
    fidl::ServerBinding<fuchsia_firmware_crash::Watcher> binding_;
  };

  friend class Watcher;

  // List of all crashes that have occurred.
  std::unordered_map<std::string, uint32_t> crash_count_;
  std::vector<fuchsia_firmware_crash::Crash> crashes_;

  fidl::ServerBindingGroup<fuchsia_firmware_crash::Reporter> bindings_;
  std::vector<std::unique_ptr<Watcher>> watchers_;

  async_dispatcher_t* const dispatcher_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_FIRMWARE_CRASH_FIRMWARE_CRASH_SERVICE_H_
