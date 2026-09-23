// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/power/wake_lease/cpp/wake_lease.h"

#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/driver/power/cpp/testing/fake_activity_governor.h>
#include <lib/driver/power/cpp/testing/fidl_bound_server.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fpromise/result.h>
#include <zircon/types.h>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/test_loop_fixture.h>

namespace {

using fdf_power::testing::FakeActivityGovernor;
using fdf_power::testing::FidlBoundServer;
using fuchsia_power_system::ActivityGovernor;
using fuchsia_power_system::LeaseToken;

class WakeLeaseTest : public gtest::TestLoopFixture {};

TEST_F(WakeLeaseTest, AcquireWakeLeaseThenDropIt) {
  auto endpoints = fidl::CreateEndpoints<ActivityGovernor>().value();
  FidlBoundServer<FakeActivityGovernor> server(
      test_loop().dispatcher(), std::move(endpoints.server), test_loop().dispatcher());
  ASSERT_FALSE(server.HasActiveWakeLease());

  // Create the ActivityGovernor client and use it to take a WakeLease.
  fidl::Client<ActivityGovernor> client(std::move(endpoints.client), test_loop().dispatcher());
  examples::power::WakeLease::Take(client, "test-wake-lease")
      .then(
          // Verify that the client received its WakeLease, then immediately destroy it.
          [&server](fpromise::result<examples::power::WakeLease, examples::power::Error>& result) {
            EXPECT_FALSE(result.is_error()) << result.error();
            EXPECT_TRUE(result.is_ok());
            ASSERT_TRUE(server.HasActiveWakeLease());
            // Destroy the WakeLease at the end of this scope.
            auto wake_lease = result.take_value();
          });
  RunLoopUntilIdle();

  ASSERT_FALSE(server.HasActiveWakeLease());
}

}  // namespace
