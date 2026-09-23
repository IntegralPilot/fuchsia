// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_SYSTEM_ACTIVITY_GOVERNOR_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_SYSTEM_ACTIVITY_GOVERNOR_H_

#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/syslog/cpp/macros.h>

#include <unordered_map>

#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics::stubs {

class SystemActivityGovernor : public FidlServer<fuchsia_power_system::ActivityGovernor> {
 public:
  SystemActivityGovernor(fidl::ServerEnd<fuchsia_power_system::ActivityGovernor> server_end,
                         async_dispatcher_t* dispatcher)
      : dispatcher_(dispatcher),
        binding_(dispatcher, std::move(server_end), this, &SystemActivityGovernor::OnFidlClosed) {}

  void AcquireWakeLease(AcquireWakeLeaseRequest& request,
                        AcquireWakeLeaseCompleter::Sync& completer) override;

  bool LeaseHeld() { return !active_wake_leases_.empty(); }

  static void OnFidlClosed(const fidl::UnbindInfo error) { FX_LOGS(ERROR) << error; }

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ServerBinding<fuchsia_power_system::ActivityGovernor> binding_;
  std::unordered_map<zx_handle_t, fuchsia_power_system::LeaseToken> active_wake_leases_;
};

class SystemActivityGovernorClosesConnection
    : public FidlServer<fuchsia_power_system::ActivityGovernor> {
 public:
  SystemActivityGovernorClosesConnection(
      fidl::ServerEnd<fuchsia_power_system::ActivityGovernor> server_end,
      async_dispatcher_t* dispatcher)
      : binding_(dispatcher, std::move(server_end), this,
                 &SystemActivityGovernorClosesConnection::OnFidlClosed) {}

  void AcquireWakeLease(AcquireWakeLeaseRequest& request,
                        AcquireWakeLeaseCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_PEER_CLOSED);
  }

  static void OnFidlClosed(const fidl::UnbindInfo error) { FX_LOGS(ERROR) << error; }

 private:
  fidl::ServerBinding<fuchsia_power_system::ActivityGovernor> binding_;
};

class SystemActivityGovernorNeverResponds
    : public FidlServer<fuchsia_power_system::ActivityGovernor> {
 public:
  SystemActivityGovernorNeverResponds(
      fidl::ServerEnd<fuchsia_power_system::ActivityGovernor> server_end,
      async_dispatcher_t* dispatcher)
      : binding_(dispatcher, std::move(server_end), this,
                 &SystemActivityGovernorClosesConnection::OnFidlClosed) {}

  void AcquireWakeLease(AcquireWakeLeaseRequest& request,
                        AcquireWakeLeaseCompleter::Sync& completer) override {
    // Completers will panic if they're destructed before a reply is sent.
    completers_.push_back(completer.ToAsync());
  }

  static void OnFidlClosed(const fidl::UnbindInfo error) { FX_LOGS(ERROR) << error; }

 private:
  fidl::ServerBinding<fuchsia_power_system::ActivityGovernor> binding_;
  std::vector<AcquireWakeLeaseCompleter::Async> completers_;
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_SYSTEM_ACTIVITY_GOVERNOR_H_
