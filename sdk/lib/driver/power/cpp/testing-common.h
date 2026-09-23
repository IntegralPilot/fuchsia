// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TESTING_COMMON_H_
#define LIB_DRIVER_POWER_CPP_TESTING_COMMON_H_

#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire_natural_conversions.h>
#include <zircon/availability.h>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace power_lib_test {
class SystemActivityGovernor
    : public fidl::testing::TestBase<fuchsia_power_system::ActivityGovernor> {
 public:
  SystemActivityGovernor(zx::event wake_handling_assertive, async_dispatcher_t* dispatcher)
      : suspend_blocker_(std::nullopt),
        wake_handling_assertive_(std::move(wake_handling_assertive)),
        dispatcher_(dispatcher) {}

  void GetPowerElements(GetPowerElementsCompleter::Sync& completer) override {
    fuchsia_power_system::PowerElements elements;
    zx::event aa_element;
    wake_handling_assertive_.duplicate(ZX_RIGHT_SAME_RIGHTS, &aa_element);

    fuchsia_power_system::ApplicationActivity application_activity = {
        {.assertive_dependency_token = std::move(aa_element)}};

    elements = {{.application_activity = std::move(application_activity)}};
    completer.Reply({{std::move(elements)}});
  }

  void RegisterSuspendBlocker(RegisterSuspendBlockerRequest& request,
                              RegisterSuspendBlockerCompleter::Sync& completer) override {
    fidl::ClientEnd<fuchsia_power_system::SuspendBlocker> suspend_blocker =
        std::move(request.suspend_blocker().value());

    // only support one suspend blocker
    ASSERT_EQ(suspend_blocker_, std::nullopt);

    suspend_blocker_.emplace(fidl::Client<fuchsia_power_system::SuspendBlocker>(
        std::move(suspend_blocker), dispatcher_));

    // Use a fake lease token. Our fake doesn't currently keep it alive or use it to prevent
    // suspension.
    zx::eventpair client_token, server_token;
    zx::eventpair::create(0, &client_token, &server_token);
    server_lease_token_.emplace(std::move(server_token));
    fuchsia_power_system::ActivityGovernorRegisterSuspendBlockerResponse response;
    response.token() = std::move(client_token);

    completer.Reply(fit::ok(std::move(response)));
  }

  void SendBeforeSuspend() {
    if (!suspend_blocker_) {
      return;
    }

    // If there's an error, do something?
    suspend_blocker_.value()->BeforeSuspend().ThenExactlyOnce(
        [](fidl::Result<fuchsia_power_system::SuspendBlocker::BeforeSuspend>& result) {});
  }

  void SendAfterResume() {
    if (!suspend_blocker_) {
      return;
    }

    // Ensure that the wake lease was dropped.
    ASSERT_TRUE(server_lease_token_);
    ASSERT_EQ(ZX_OK, server_lease_token_->wait_one(ZX_EVENTPAIR_PEER_CLOSED, zx::time::infinite(),
                                                   nullptr));
    server_lease_token_.reset();

    // If there's an error, do something?
    suspend_blocker_.value()->AfterResume().ThenExactlyOnce(
        [](fidl::Result<fuchsia_power_system::SuspendBlocker::AfterResume>& result) {});
  }

  void AcquireWakeLease(AcquireWakeLeaseRequest& request,
                        AcquireWakeLeaseCompleter::Sync& completer) override {
    zx::eventpair one;
    zx::eventpair two;
    zx::eventpair::create(0, &one, &two);
    completer.Reply(fit::success<fuchsia_power_system::ActivityGovernorAcquireWakeLeaseResponse>(
        std::move(two)));
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_system::ActivityGovernor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE() << name << " is not implemented";
  }

 private:
  std::optional<fidl::Client<fuchsia_power_system::SuspendBlocker>> suspend_blocker_;
  std::optional<zx::eventpair> server_lease_token_;
  zx::event wake_handling_assertive_;
  async_dispatcher_t* dispatcher_;
};
}  // namespace power_lib_test

#endif
#endif  // LIB_DRIVER_POWER_CPP_TESTING_COMMON_H_
