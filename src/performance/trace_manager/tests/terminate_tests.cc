// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include "src/performance/trace_manager/tests/trace_manager_test.h"

namespace tracing {
namespace test {

TEST_F(TraceManagerTest, TerminateOnClose) {
  ConnectToProvisionerService();

  ASSERT_TRUE(InitializeSession());

  EXPECT_EQ(GetSessionState(), SessionState::kInitialized);

  ASSERT_TRUE(StartSession());

  EXPECT_EQ(GetSessionState(), SessionState::kStarted);

  DisconnectFromControllerService();

  RunLoopUntilIdle();
  EXPECT_EQ(GetSessionState(), SessionState::kNonexistent);
}

}  // namespace test
}  // namespace tracing
