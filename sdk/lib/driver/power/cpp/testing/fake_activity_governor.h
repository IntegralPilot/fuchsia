// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TESTING_FAKE_ACTIVITY_GOVERNOR_H_
#define LIB_DRIVER_POWER_CPP_TESTING_FAKE_ACTIVITY_GOVERNOR_H_

#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/async/cpp/wait.h>
#include <lib/fidl/cpp/wire/channel.h>

#include <memory>
#include <unordered_map>

#include "sdk/lib/driver/power/cpp/testing/fidl_test_base_default.h"

namespace fdf_power::testing {

using fuchsia_power_system::ActivityGovernor;
using fuchsia_power_system::LeaseToken;
using fuchsia_power_system::SuspendBlocker;

class FakeActivityGovernor : public FidlTestBaseDefault<ActivityGovernor> {
 public:
  explicit FakeActivityGovernor(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  bool HasActiveWakeLease() const { return !active_wake_leases_.empty(); }

 private:
  // `fuchsia.power.system/ActivityGovernor`:
  void AcquireWakeLease(AcquireWakeLeaseRequest& /* ignored */,
                        AcquireWakeLeaseCompleter::Sync& completer) override {
    completer.Reply(fit::ok(AcquireWakeLease()));
  }

  LeaseToken AcquireWakeLease() {
    LeaseToken client_token, server_token;
    LeaseToken::create(/*options=*/0u, &client_token, &server_token);

    // Start an async task to wait for EVENTPAIR_PEER_CLOSED signal on server_token.
    zx_handle_t token_handle = server_token.get();
    active_wake_leases_[token_handle] = std::move(server_token);
    auto wait = std::make_unique<async::WaitOnce>(token_handle, ZX_EVENTPAIR_PEER_CLOSED);
    wait->Begin(dispatcher_, [this, token_handle, wait = std::move(wait)](
                                 async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                                 const zx_packet_signal_t*) {
      ZX_ASSERT(status == ZX_OK);
      auto it = active_wake_leases_.find(token_handle);
      ZX_ASSERT(it != active_wake_leases_.end());
      ZX_ASSERT(token_handle == it->second.get());
      active_wake_leases_.erase(it);
    });

    return client_token;
  }

  async_dispatcher_t* dispatcher_;
  std::unordered_map<zx_handle_t, LeaseToken> active_wake_leases_;
};

class FakeSuspendBlocker : public FidlTestBaseDefault<SuspendBlocker> {
 public:
  FakeSuspendBlocker() = default;

  bool SuspendStarted() const { return suspend_started_; }

 private:
  void BeforeSuspend(BeforeSuspendCompleter::Sync& completer) override {
    suspend_started_ = true;
    completer.Reply();
  }

  // These completers must also reply for expected operation.
  void AfterResume(AfterResumeCompleter::Sync& completer) override { completer.Reply(); }

  bool suspend_started_ = false;
};

}  // namespace fdf_power::testing

#endif  // LIB_DRIVER_POWER_CPP_TESTING_FAKE_ACTIVITY_GOVERNOR_H_
