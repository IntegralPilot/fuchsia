// Copyright 2021 The Fuchsia Authors.All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_CRASH_REPORTS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_CRASH_REPORTS_H_

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/async/dispatcher.h>

#include "src/developer/forensics/crash_reports/crash_register.h"
#include "src/developer/forensics/crash_reports/crash_reporter.h"
#include "src/developer/forensics/crash_reports/crash_server.h"
#include "src/developer/forensics/crash_reports/info/info_context.h"
#include "src/developer/forensics/crash_reports/log_tags.h"
#include "src/developer/forensics/crash_reports/report_store.h"
#include "src/developer/forensics/feedback/annotations/annotation_manager.h"
#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/feedback_data/data_provider.h"
#include "src/developer/forensics/utils/storage_size.h"

namespace forensics::feedback {

class CrashReports {
 public:
  struct Options {
    BuildTypeConfig build_type_config;
    StorageSize report_persistence_max_tmp_size;
    StorageSize report_persistence_max_cache_size;
    StorageSize snapshot_store_max_archives_size;
    std::optional<StorageSize> snapshot_persistence_max_tmp_size;
    std::optional<StorageSize> snapshot_persistence_max_cache_size;
    zx::duration snapshot_collector_window_duration;
  };

  CrashReports(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
               timekeeper::Clock* clock, inspect::Node* inspect_root,
               feedback::AnnotationManager* annotation_manager,
               feedback_data::DataProviderInternal* data_provider, Options options);

  fuchsia::feedback::CrashReporter* CrashReporter();
  fuchsia::feedback::CrashReportingProductRegister* CrashRegister();

  void SetNetworkIsReachable(bool is_reachable);
  void ShutdownImminent();

 private:
  std::shared_ptr<crash_reports::InfoContext> info_context_;
  crash_reports::LogTags tags_;
  crash_reports::CrashServer crash_server_;
  crash_reports::ReportStore report_store_;
  crash_reports::CrashRegister crash_register_;
  crash_reports::CrashReporter crash_reporter_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_CRASH_REPORTS_H_
