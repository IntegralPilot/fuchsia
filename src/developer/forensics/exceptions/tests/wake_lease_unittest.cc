// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/exceptions/handler/wake_lease.h"

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/test_base.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/async/cpp/executor.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <memory>
#include <string>
#include <type_traits>
#include <utility>

#include <gtest/gtest.h>

#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/system_activity_governor.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/errors.h"

namespace forensics::exceptions::handler {
namespace {

using ::fidl::testing::TestBase;

constexpr zx::duration kTimeout = zx::sec(5);

namespace fps = fuchsia_power_system;

class WakeLeaseTest : public UnitTestFixture {
 protected:
  WakeLeaseTest() : executor_(dispatcher()) {}

  async::Executor& GetExecutor() { return executor_; }

  template <typename Impl>
  static std::optional<std::tuple<fidl::ClientEnd<fps::ActivityGovernor>, std::unique_ptr<Impl>>>
  CreateSag(async_dispatcher_t* dispatcher) {
    static_assert(std::is_base_of_v<TestBase<fps::ActivityGovernor>, Impl>);

    auto endpoints = fidl::CreateEndpoints<fps::ActivityGovernor>();
    if (!endpoints.is_ok()) {
      return std::nullopt;
    }

    auto stub = std::make_unique<Impl>(std::move(endpoints->server), dispatcher);
    return std::make_tuple(std::move(endpoints->client), std::move(stub));
  }

 private:
  async::Executor executor_;
};

TEST_F(WakeLeaseTest, AcquiresLeaseSuccessfully) {
  auto sag_client_and_stub = CreateSag<stubs::SystemActivityGovernor>(dispatcher());
  ASSERT_TRUE(sag_client_and_stub.has_value());
  auto& [sag_client, sag] = *sag_client_and_stub;

  WakeLease wake_lease(dispatcher(), "exceptions-element-001", std::move(sag_client));

  std::optional<fps::LeaseToken> lease;
  GetExecutor().schedule_task(wake_lease.Acquire(kTimeout)
                                  .and_then([&lease](fps::LeaseToken& acquired_lease) {
                                    lease = std::move(acquired_lease);
                                  })
                                  .or_else([](const Error& error) {
                                    FX_LOGS(FATAL) << "Unexpected error while acquiring lease: "
                                                   << ToString(error);
                                  }));

  RunLoopUntilIdle();
  ASSERT_TRUE(lease.has_value());
  EXPECT_TRUE(lease->is_valid());
  EXPECT_TRUE(sag->LeaseHeld());

  lease.reset();
  RunLoopUntilIdle();
  EXPECT_FALSE(sag->LeaseHeld());
}

TEST_F(WakeLeaseTest, LeaseFails) {
  auto sag_client_and_stub = CreateSag<stubs::SystemActivityGovernorClosesConnection>(dispatcher());
  ASSERT_TRUE(sag_client_and_stub.has_value());
  auto& [sag_client, sag] = *sag_client_and_stub;

  WakeLease wake_lease(dispatcher(), "exceptions-element-001", std::move(sag_client));

  std::optional<Error> error;
  GetExecutor().schedule_task(wake_lease.Acquire(kTimeout)
                                  .and_then([](const fps::LeaseToken& acquired_lease) {
                                    FX_LOGS(FATAL) << "Unexpected success while acquiring lease";
                                  })
                                  .or_else([&error](const Error& result) { error = result; }));

  RunLoopUntilIdle();
  EXPECT_EQ(error, Error::kBadValue);
}

TEST_F(WakeLeaseTest, LeaseFailsOnTimeout) {
  auto sag_client_and_stub = CreateSag<stubs::SystemActivityGovernorNeverResponds>(dispatcher());
  ASSERT_TRUE(sag_client_and_stub.has_value());
  auto& [sag_client, sag] = *sag_client_and_stub;

  WakeLease wake_lease(dispatcher(), "exceptions-element-001", std::move(sag_client));

  std::optional<Error> error;
  GetExecutor().schedule_task(wake_lease.Acquire(kTimeout)
                                  .and_then([](const fps::LeaseToken& acquired_lease) {
                                    FX_LOGS(FATAL) << "Unexpected success while acquiring lease";
                                  })
                                  .or_else([&error](const Error& result) { error = result; }));

  RunLoopFor(kTimeout);
  EXPECT_EQ(error, Error::kTimeout);
}

}  // namespace
}  // namespace forensics::exceptions::handler
