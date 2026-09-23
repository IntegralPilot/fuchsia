// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include "src/performance/trace_manager/tests/trace_manager_test.h"

namespace tracing {
namespace test {

using SessionState = TraceManagerTest::SessionState;

TEST_F(TraceManagerTest, RetryAfterFailedStop) {
  ConnectToProvisionerService();

  EXPECT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name));

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());

  fuchsia_tracing_controller::StopOptions stop_options{GetDefaultStopOptions()};
  session_client()->StopTracing({{std::move(stop_options)}}).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), fuchsia_tracing_controller::StopError::kAborted);
  });
  RunLoopUntilIdle();
  ASSERT_EQ(GetSessionState(), SessionState::kStopping);

  // Drop the socket before marking all providers stopped. This causes
  // an error while writing the buffer to the socket. Trace session should
  // abort and terminate.
  DropSocket();
  MarkAllProvidersStopped();
  RunLoopUntilIdle();
  // The session will already be gone because we dropped the socket.
  ASSERT_EQ(GetSessionState(), SessionState::kNonexistent);
  MarkAllProvidersTerminated();
  RunLoopUntilIdle();
  ASSERT_EQ(GetSessionState(), SessionState::kNonexistent);

  // Initialize a new session and verify that everything works fine
  zx::socket our_socket, their_socket;
  zx_status_t status = zx::socket::create(0u, &our_socket, &their_socket);
  ASSERT_EQ(status, ZX_OK);

  fuchsia_tracing_controller::TraceConfig config{GetDefaultTraceConfig()};
  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
  session_client() = fidl::Client(std::move(endpoints.client), dispatcher(), this);
  auto initialize_result = provisioner_client()->InitializeTracing(
      {{std::move(endpoints.server), std::move(config), std::move(their_socket)}});
  ASSERT_TRUE(initialize_result.is_ok());

  RunLoopUntilIdle();
  ASSERT_EQ(GetSessionState(), SessionState::kInitialized);
  RunLoopUntilIdle();

  fuchsia_tracing_controller::StartOptions start_options{GetDefaultStartOptions()};
  bool start_completed = false;
  session_client()
      ->StartTracing({{std::move(start_options)}})
      .ThenExactlyOnce([this, &start_completed](auto& result) {
        start_completed = true;
        QuitLoop();
      });
  RunLoopUntilIdle();
  MarkAllProvidersStarted();
  RunLoopUntilIdle();
  ASSERT_EQ(GetSessionState(), SessionState::kStarted);

  ASSERT_TRUE(StopSession());

  ASSERT_TRUE(TerminateSession());
}

template <typename T>
void TryExtraStop(TraceManagerTest* fixture, const T& interface_ptr) {
  fuchsia_tracing_controller::StopOptions stop_options{fixture->GetDefaultStopOptions()};
  bool stop_completed = false;
  interface_ptr->StopTracing({{std::move(stop_options)}})
      .ThenExactlyOnce([fixture, &stop_completed](auto& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error());
        ASSERT_EQ(result.error_value().domain_error(),
                  fuchsia_tracing_controller::StopError::kNotStarted);
        stop_completed = true;
        fixture->QuitLoop();
      });
  fixture->RunLoopUntilIdle();
  FX_LOGS(DEBUG) << "Loop done, expecting session still stopped";
  EXPECT_TRUE(stop_completed);
  EXPECT_EQ(fixture->GetSessionState(), SessionState::kStopped);
}

TEST_F(TraceManagerTest, ExtraStop) {
  ConnectToProvisionerService();

  EXPECT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name));

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());

  ASSERT_TRUE(StopSession());

  // Now try stopping again.
  // This should complete with a NOT_STARTED error
  TryExtraStop(this, session_client());
}

TEST_F(TraceManagerTest, StopWhileStopping) {
  ConnectToProvisionerService();

  FakeProvider* provider;
  EXPECT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider));

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());

  fuchsia_tracing_controller::StopOptions stop1_options{GetDefaultStopOptions()};
  session_client()->StopTracing({{std::move(stop1_options)}}).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.is_ok());
  });
  RunLoopUntilIdle();
  // The loop will exit for the transition to kStopping.
  FX_LOGS(DEBUG) << "Loop done, expecting session stopping";
  EXPECT_EQ(GetSessionState(), SessionState::kStopping);

  // Now try another Stop while we're still in |kStopping|.
  // The provider doesn't advance state until we tell it to, so we should
  // still remain in |kStopping|.
  fuchsia_tracing_controller::StopOptions stop2_options{GetDefaultStopOptions()};
  bool stop_completed = false;
  session_client()
      ->StopTracing({{std::move(stop2_options)}})
      .ThenExactlyOnce([this, &stop_completed](auto& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error());
        ASSERT_EQ(result.error_value().domain_error(),
                  fuchsia_tracing_controller::StopError::kNotStarted);
        stop_completed = true;
        QuitLoop();
      });
  RunLoopUntilIdle();
  FX_LOGS(DEBUG) << "Stop loop done";
  EXPECT_TRUE(stop_completed);
  EXPECT_TRUE(GetSessionState() == SessionState::kStopping);

  MarkAllProvidersStopped();
  RunLoopUntilIdle();
}

}  // namespace test
}  // namespace tracing
