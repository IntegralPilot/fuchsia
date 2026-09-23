// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <iostream>
#include <iterator>
#include <string>

#include "src/ui/bin/system_monitor/system_monitor.h"

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  system_monitor::SystemMonitor systemMonitor;
  systemMonitor.ConnectToArchiveAccessor();
  systemMonitor.InitializeRenderer();
  systemMonitor.PrintRecentDiagnostic();
  loop.Run();
  return 0;
}
