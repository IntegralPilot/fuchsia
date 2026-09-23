// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>

#include "gtest/gtest.h"
#include "src/ui/bin/system_monitor/system_monitor.h"

namespace {

TEST(SystemMonitorTest, GetsCPUData) {
  system_monitor::SystemMonitor systemMonitor;
  systemMonitor.ConnectToArchiveAccessor();
  systemMonitor.UpdateRecentDiagnostic();
  std::string cpuData = systemMonitor.GetCPUData();
  ASSERT_FALSE(cpuData.empty());
}
}  // namespace
