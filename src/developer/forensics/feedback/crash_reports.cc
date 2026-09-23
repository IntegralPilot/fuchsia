// Copyright 2021 The Fuchsia Authors.All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/crash_reports.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/zx/clock.h>
#include <zircon/utc.h>

#include <utility>

#include "src/developer/forensics/crash_reports/constants.h"
#include "src/developer/forensics/crash_reports/snapshot_persistence.h"
#include "src/developer/forensics/feedback/constants.h"

namespace forensics::feedback {

namespace {

std::optional<crash_reports::SnapshotPersistence::Root> GetSnapshotRoot(
    const std::string& root_dir, std::optional<StorageSize> root_size) {
  if (!root_size.has_value()) {
    return std::nullopt;
  }

  return crash_reports::SnapshotPersistence::Root{root_dir, *root_size};
}

}  // namespace

CrashReports::CrashReports(async_dispatcher_t* dispatcher,
                           std::shared_ptr<sys::ServiceDirectory> services,
                           timekeeper::Clock* clock, inspect::Node* inspect_root,
                           feedback::AnnotationManager* annotation_manager,
                           feedback_data::DataProviderInternal* data_provider,
                           const Options options)
    : info_context_(
          std::make_shared<crash_reports::InfoContext>(inspect_root, clock, dispatcher, services)),
      tags_(),
      crash_server_(dispatcher, services, kCrashServerUrl, &tags_, annotation_manager, clock),
      report_store_(&tags_, info_context_, annotation_manager,
                    /*temp_reports_root=*/
                    crash_reports::ReportStore::Root{crash_reports::kReportStoreTmpPath,
                                                     options.report_persistence_max_tmp_size},
                    /*persistent_reports_root=*/
                    crash_reports::ReportStore::Root{crash_reports::kReportStoreCachePath,
                                                     options.report_persistence_max_cache_size},
                    /*temp_snapshots_root=*/
                    GetSnapshotRoot(crash_reports::kSnapshotStoreTmpPath,
                                    options.snapshot_persistence_max_tmp_size),
                    /*persistent_snapshots_root=*/
                    GetSnapshotRoot(crash_reports::kSnapshotStoreCachePath,
                                    options.snapshot_persistence_max_cache_size),
                    kGarbageCollectedSnapshotsPath, options.snapshot_store_max_archives_size),
      crash_register_(info_context_, kCrashRegisterPath),
      crash_reporter_(dispatcher, services, clock, zx::unowned_clock(zx_utc_reference_get()),
                      info_context_, options.build_type_config, &crash_register_, &tags_,
                      &crash_server_, &report_store_, data_provider,
                      options.snapshot_collector_window_duration) {}

fuchsia::feedback::CrashReporter* CrashReports::CrashReporter() { return &crash_reporter_; }

fuchsia::feedback::CrashReportingProductRegister* CrashReports::CrashRegister() {
  return &crash_register_;
}

void CrashReports::SetNetworkIsReachable(const bool is_reachable) {
  crash_reporter_.SetNetworkIsReachable(is_reachable);
}

void CrashReports::ShutdownImminent() { crash_reporter_.PersistAllCrashReports(); }

}  // namespace forensics::feedback
