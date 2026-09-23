// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include "src/performance/trace_manager/tests/trace_manager_test.h"

namespace tracing {
namespace test {

using SessionState = TraceManagerTest::SessionState;

// This only works when no other condition could cause the loop to exit.
// E.g., This doesn't work if the state is kStopping or kTerminating as the
// transition to kStopped,kTerminated will also cause the loop to exit.
template <typename T>
fit::result<fuchsia_tracing_controller::StartError> TryStart(TraceManagerTest* fixture,
                                                             const T& interface_ptr) {
  fit::result<fuchsia_tracing_controller::StartError> start_result = fit::ok();
  fuchsia_tracing_controller::StartOptions start_options{
      TraceManagerTest::GetDefaultStartOptions()};
  bool start_completed = false;
  interface_ptr->StartTracing({{std::move(start_options)}})
      .ThenExactlyOnce(
          [fixture, &start_completed,
           &start_result](fidl::Result<fuchsia_tracing_controller::Session::StartTracing>& result) {
            start_completed = true;
            if (result.is_error()) {
              if (result.error_value().is_domain_error()) {
                start_result = fit::error(result.error_value().domain_error());
              } else {
                start_result = fit::error(fuchsia_tracing_controller::StartError::kNotInitialized);
              }
            } else {
              start_result = fit::ok();
            }
            fixture->QuitLoop();
          });
  fixture->RunLoopUntilIdle();
  FX_LOGS(DEBUG) << "Start loop done";
  EXPECT_TRUE(start_completed);
  return start_result;
}

template <typename T>
void TryExtraStart(TraceManagerTest* fixture, const T& interface_ptr) {
  auto start_result{TryStart(fixture, interface_ptr)};
  EXPECT_EQ(fixture->GetSessionState(), SessionState::kStarted);
  EXPECT_TRUE(start_result.is_error());
  EXPECT_EQ(start_result.error_value(), fuchsia_tracing_controller::StartError::kAlreadyStarted);
}

TEST_F(TraceManagerTest, ExtraStart) {
  ConnectToProvisionerService();

  EXPECT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name));

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());

  // Now try starting again.
  TryExtraStart(this, session_client());
}

TEST_F(TraceManagerTest, StartWhileStopping) {
  ConnectToProvisionerService();

  EXPECT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name));

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());

  fuchsia_tracing_controller::StopOptions stop_options{GetDefaultStopOptions()};
  session_client()->StopTracing({{std::move(stop_options)}}).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.is_ok());
  });
  RunLoopUntilIdle();
  // The loop will exit for the transition to kStopping.
  FX_LOGS(DEBUG) << "Loop done, expecting session stopping";
  ASSERT_EQ(GetSessionState(), SessionState::kStopping);

  // Now try a Start while we're still in |kStopping|.
  // The provider doesn't advance state until we tell it to, so we should
  // still remain in |kStopping|.
  fit::result<fuchsia_tracing_controller::StartError> start_result = fit::ok();
  fuchsia_tracing_controller::StartOptions start_options{GetDefaultStartOptions()};
  bool start_completed = false;
  session_client()
      ->StartTracing({{std::move(start_options)}})
      .ThenExactlyOnce(
          [this, &start_completed,
           &start_result](fidl::Result<fuchsia_tracing_controller::Session::StartTracing>& result) {
            start_completed = true;
            if (result.is_error()) {
              if (result.error_value().is_domain_error()) {
                start_result = fit::error(result.error_value().domain_error());
              } else {
                start_result = fit::error(fuchsia_tracing_controller::StartError::kNotInitialized);
              }
            } else {
              start_result = fit::ok();
            }
            QuitLoop();
          });
  RunLoopUntilIdle();
  FX_LOGS(DEBUG) << "Start loop done";
  EXPECT_TRUE(GetSessionState() == SessionState::kStopping);
  ASSERT_TRUE(start_completed);
  ASSERT_TRUE(start_result.is_error());
  EXPECT_EQ(start_result.error_value(), fuchsia_tracing_controller::StartError::kStopping);

  MarkAllProvidersStopped();
  RunLoopUntilIdle();
}

}  // namespace test
}  // namespace tracing
