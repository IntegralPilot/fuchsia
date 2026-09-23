// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fpromise/promise.h>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/coordinator-getter/client.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

// Tests the code path when the service routing is available.
class GetHardwareDisplayCoordinatorTest : public gtest::RealLoopFixture {};

// FIDL and Async executor should be able to run on a single dispatcher.
TEST_F(GetHardwareDisplayCoordinatorTest, SingleDispatcher) {
  std::optional<fpromise::result<CoordinatorClientChannels, zx_status_t>> coordinator;
  async::Executor executor(dispatcher());

  component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher;
  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
      watcher.GetNextInstance(/*stop_at_idle=*/false);
  ASSERT_OK(provider_result);
  fidl::ClientEnd<fuchsia_hardware_display::Provider> provider = std::move(provider_result).value();

  executor.schedule_task(
      GetCoordinator(std::move(provider), dispatcher())
          .then([&coordinator](fpromise::result<CoordinatorClientChannels, zx_status_t>& result) {
            coordinator = std::move(result);
          }));

  // After the service is opened, an fuchsia.io.Node.OnOpen() event will be
  // dispatched to the FIDL dispatcher, before which the loop will be idle.
  // Tests should use RunLoopUntil() to wait until the coordinator is fetched.
  RunLoopUntil([&] { return coordinator.has_value(); });
  ASSERT_TRUE(coordinator.value().is_ok()) << "Failed to get coordinator client end: "
                                           << zx_status_get_string(coordinator.value().error());
  EXPECT_TRUE(coordinator.value().value().coordinator_client_end.is_valid());
  EXPECT_TRUE(coordinator.value().value().coordinator_listener_server_end.is_valid());
}

}  // namespace

}  // namespace display
