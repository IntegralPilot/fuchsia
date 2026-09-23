// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_BIN_SYSTEM_MONITOR_SYSTEM_MONITOR_H_
#define SRC_UI_BIN_SYSTEM_MONITOR_SYSTEM_MONITOR_H_

#include <fuchsia/diagnostics/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>

#include "src/ui/bin/system_monitor/system_monitor_renderer.h"

namespace system_monitor {

// This class uses archiveAccessor and batchIterator to get diagnostic
// information like platform metrics which include CPU usage.
class SystemMonitor {
 public:
  // Class constructor initializes the parameters of INSPECT data
  SystemMonitor();
  // These functions uses the iterator to access inspect diagnostic and turn
  // all the json format content into std::string and
  // returns a vector of strings.
  void ConnectToArchiveAccessor();
  void InitializeRenderer();
  void UpdateRecentDiagnostic();
  void PrintRecentDiagnostic();
  std::vector<std::string> ParseBatch(
      const std::vector<fuchsia::diagnostics::FormattedContent>& batch);
  std::string GetTargetFromDiagnostics(std::vector<std::string> recent_diagnostics);
  std::string GetCPUData();

 private:
  // batchIterator result initialized in the UpdateRecentDiagnostic() function.
  fuchsia::diagnostics::BatchIterator_GetNext_Result iterator_result_;
  // a batchIterator pointer initialized in the UpdateRecentDiagnostic() function.
  fuchsia::diagnostics::BatchIteratorSyncPtr iterator_;
  // archiveAccessor pointer initialized in the UpdateRecentDiagnostic() function to get a new batch
  // of inspect data.
  fuchsia::diagnostics::ArchiveAccessorSyncPtr accessor_;
  // streamParameters initialized in the class constructor.
  fuchsia::diagnostics::StreamParameters params_;
  SystemMonitorRenderer renderer_;
};
}  // namespace system_monitor

#endif  // SRC_UI_BIN_SYSTEM_MONITOR_SYSTEM_MONITOR_H_
