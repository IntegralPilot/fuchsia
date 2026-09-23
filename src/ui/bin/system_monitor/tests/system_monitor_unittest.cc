// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/bin/system_monitor/system_monitor.h"

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/test_loop_fixture.h>

class SystemMonitorUnitTest : public gtest::TestLoopFixture {};

TEST_F(SystemMonitorUnitTest, GetTargetFromDiagnostics_EmptyDiagnostics_ReturnsEmpty) {
  std::vector<std::string> input = {};
  system_monitor::SystemMonitor systemMonitor;
  const std::string& output = systemMonitor.GetTargetFromDiagnostics(input);
  ASSERT_TRUE(output.empty());
}

TEST_F(SystemMonitorUnitTest, GetTargetFromDiagnostics_DiagnosticsIncludeTarget_ReturnsContent) {
  constexpr char kTargetString[] = "this contains platform_metrics";
  std::vector<std::string> input = {"command", "testing", "123", "42", "hello", kTargetString};
  system_monitor::SystemMonitor systemMonitor;
  const std::string& actual_output = systemMonitor.GetTargetFromDiagnostics(input);
  EXPECT_EQ(kTargetString, actual_output);
}

TEST_F(SystemMonitorUnitTest, GetTargetFromDiagnostics_DiagnosticsExcludeTarget_ReturnsEmpty) {
  std::vector<std::string> input = {"command", "testing", "123", "42", "hello"};
  system_monitor::SystemMonitor systemMonitor;
  const std::string& output = systemMonitor.GetTargetFromDiagnostics(input);
  ASSERT_TRUE(output.empty());
}
