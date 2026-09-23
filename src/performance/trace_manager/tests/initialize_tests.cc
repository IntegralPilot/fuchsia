// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include "src/performance/trace_manager/tests/trace_manager_test.h"

namespace tracing {
namespace test {

TEST_F(TraceManagerTest, DuplicateInitialization) {
  ConnectToProvisionerService();

  ASSERT_TRUE(InitializeSession());

  zx::socket our_socket, their_socket;
  zx_status_t status = zx::socket::create(0u, &our_socket, &their_socket);
  ASSERT_EQ(status, ZX_OK);

  fuchsia_tracing_controller::TraceConfig config{GetDefaultTraceConfig()};
  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
  auto result = provisioner_client()->InitializeTracing(
      {{std::move(endpoints.server), std::move(config), std::move(their_socket)}});
  ASSERT_TRUE(result.is_ok());
  // There's no state transition here that would trigger a call to
  // |LoopQuit()|. Nor is there a result.
  // Mostly we just want to verify things don't hang.
  RunLoopUntilIdle();
}

TEST_F(TraceManagerTest, LargeBuffers) {
  ConnectToProvisionerService();

  fuchsia_tracing_controller::TraceConfig config{GetDefaultTraceConfig()};
  config.buffer_size_megabytes_hint(1024);
  ASSERT_TRUE(InitializeSession(std::move(config)));
  EXPECT_EQ(GetSessionState(), SessionState::kInitialized);
}

TEST_F(TraceManagerTest, InitializeWithoutProviders) {
  ConnectToProvisionerService();

  ASSERT_TRUE(InitializeSession());

  EXPECT_EQ(GetSessionState(), SessionState::kInitialized);
}

}  // namespace test
}  // namespace tracing
