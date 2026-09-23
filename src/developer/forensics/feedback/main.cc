// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fuchsia/feedback/cpp/fidl.h>
#include <fuchsia/process/lifecycle/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <zircon/processargs.h>

#include <cstdlib>
#include <memory>

#include <fbl/unique_fd.h>

#include "src/developer/forensics/feedback/annotations/startup_annotations.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback/main_service.h"
#include "src/developer/forensics/feedback/namespace_init.h"
#include "src/developer/forensics/feedback/reboot_log/reboot_log.h"
#include "src/developer/forensics/feedback/redactor_factory.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/disk_backed_logs_metadata.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/writer.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/component/component.h"
#include "src/developer/forensics/utils/storage_size.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/uuid/uuid.h"

namespace forensics::feedback {

namespace {

zx::job GetRootJobForInspect() {
  zx::result client_end = ::component::Connect<fuchsia_kernel::RootJobForInspect>();
  if (!client_end.is_ok()) {
    FX_PLOGS(ERROR, client_end.status_value())
        << "Failed to connect to fuchsia.kernel.RootJobForInspect";
    return zx::job();
  }

  fidl::SyncClient client(std::move(*client_end));
  auto result = client->Get();
  if (!result.is_ok()) {
    FX_LOGS(ERROR) << "Failed to get root job from fuchsia.kernel.RootJobForInspect: "
                   << result.error_value().FormatDescription();
    return zx::job();
  }

  return std::move(result->job());
}

}  // namespace

int main() {
  forensics::component::Component component;
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"forensics", "feedback"}).BuildAndInitialize();

  if (const zx_status_t status = zx::job::default_job()->set_critical(
          ZX_JOB_CRITICAL_PROCESS_RETCODE_NONZERO, *zx::process::self());
      status != ZX_OK) {
    FX_LOGS(WARNING) << "Failed to set process as critical to its job: " << status;
  }

  const std::optional<SnapshotConfig> snapshot_config = GetSnapshotConfig();
  if (!snapshot_config) {
    FX_LOGS(FATAL) << "Failed to get config for snapshot";
    return EXIT_FAILURE;
  }

  // Assembly will add an empty snapshot exclusion file even if the product didn't specify a
  // snapshot exclusion config.
  const std::optional<SnapshotExclusionConfig> snapshot_exclusion_config =
      GetSnapshotExclusionConfig();
  if (!snapshot_config) {
    FX_LOGS(FATAL) << "Failed to get config for snapshot exclusion";
    return EXIT_FAILURE;
  }

  const std::optional<FeedbackConfig> feedback_config = GetFeedbackConfig();
  if (!feedback_config) {
    FX_LOGS(FATAL) << "Failed to get feedback config";
    return EXIT_FAILURE;
  }

  std::unique_ptr<cobalt::Logger> cobalt = std::make_unique<cobalt::Logger>(
      component.Dispatcher(), component.Services(), component.Clock());

  if (component.IsFirstInstance()) {
    MoveFile(/*from=*/kLegacyCurrentGracefulRebootReasonFile,
             /*to=*/kLegacyPreviousGracefulRebootReasonFile);
    MoveFile(/*from=*/kCurrentGracefulShutdownInfoFile, /*to=*/kPreviousGracefulShutdownInfoFile);
    MoveFile(/*from=*/kCurrentSystemTimePath, /*to=*/kPreviousSystemTimePath);
    MoveFile(/*from=*/kCurrentDiskBackedLogsMetadataPath,
             /*to=*/kPreviousDiskBackedLogsMetadataPath);
    CreatePreviousLogsFile(cobalt.get(), kPersistedLogsTotalSize);
    MoveAndRecordBootId(uuid::Generate());
    if (std::string build_version; files::ReadFileToString(kBuildVersionPath, &build_version)) {
      MoveAndRecordBuildVersion(build_version, kPreviousBuildVersionPath, kCurrentBuildVersionPath);
    }

    if (std::string build_platform_version;
        files::ReadFileToString(kBuildPlatformVersionPath, &build_platform_version)) {
      MoveAndRecordBuildVersion(build_platform_version, kPreviousBuildPlatformVersionPath,
                                kCurrentBuildPlatformVersionPath);
    }

    if (std::string build_product_version;
        files::ReadFileToString(kBuildProductVersionPath, &build_product_version)) {
      MoveAndRecordBuildVersion(build_product_version, kPreviousBuildProductVersionPath,
                                kCurrentBuildProductVersionPath);
    }
  }

  ExposeConfig(*component.InspectRoot(), *feedback_config);

  std::unique_ptr<RedactorBase> redactor =
      RedactorFromConfig(component.InspectRoot(), feedback_config->build_type_config);

  RebootLog reboot_log = RebootLog::ParseRebootLog(
      "/boot/log/last-panic.txt", kPreviousGracefulShutdownInfoFile,
      kLegacyPreviousGracefulRebootReasonFile, kPreviousSystemTimePath, kPreviousBootKernelLogPath,
      kFinalShutdownInfoPath, TestAndSetNotAFdr(),
      feedback_config->supports_user_initiated_poweroffs, component.IsFirstInstance(),
      redactor.get());

  if (component.IsFirstInstance()) {
    const std::optional<feedback_data::system_log_recorder::DiskBackedLogsMetadata> metadata =
        feedback_data::system_log_recorder::DiskBackedLogsMetadata::FromFile(
            kPreviousDiskBackedLogsMetadataPath,
            feedback_data::system_log_recorder::SystemLogWriter::kFirstFileNumber);
    if (metadata.has_value()) {
      metadata->LogToCobalt(*cobalt, reboot_log.GetFinalShutdownInfo().Uptime());
    }
    files::DeletePath(kPreviousDiskBackedLogsMetadataPath, /*recursive=*/false);
  }

  std::optional<std::string> local_device_id_path = kDeviceIdPath;
  if (feedback_config->remote_device_id_provider) {
    local_device_id_path = std::nullopt;
  }

  std::optional<zx::duration> delete_previous_boot_logs_time(std::nullopt);
  if (files::IsFile(kPreviousLogsFilePath)) {
    delete_previous_boot_logs_time = zx::hour(24);
  }

  const Annotations startup_annotations =
      GetStartupAnnotations(reboot_log.GetFinalShutdownInfo(),
                            feedback_config->spontaneous_reboot_reason, kBuildCompilationModePath);
  zx::channel lifecycle_channel(zx_take_startup_handle(PA_LIFECYCLE));

  std::unique_ptr<MainService> main_service = std::make_unique<MainService>(
      component.Dispatcher(), component.Services(), component.Clock(), component.InspectRoot(),
      cobalt.get(), startup_annotations,
      fidl::InterfaceRequest<fuchsia::process::lifecycle::Lifecycle>(std::move(lifecycle_channel)),
      std::move(redactor),
      MainService::Options{
          local_device_id_path, kCurrentGracefulShutdownInfoFile, kCurrentSystemTimePath,
          LastReboot::Options{
              .is_first_instance = component.IsFirstInstance(),
              .reboot_log = std::move(reboot_log),
              .oom_crash_reporting_delay = kOOMCrashReportingDelay,
              .spontaneous_reboot_reason = feedback_config->spontaneous_reboot_reason,
          },
          CrashReports::Options{
              .build_type_config = feedback_config->build_type_config,
              .report_persistence_max_tmp_size = feedback_config->report_persistence_max_tmp_size,
              .report_persistence_max_cache_size =
                  feedback_config->report_persistence_max_cache_size,
              .snapshot_store_max_archives_size = kSnapshotArchivesMaxSize,
              .snapshot_persistence_max_tmp_size =
                  feedback_config->snapshot_persistence_max_tmp_size,
              .snapshot_persistence_max_cache_size =
                  feedback_config->snapshot_persistence_max_cache_size,
              .snapshot_collector_window_duration = kSnapshotSharedRequestWindow,
          },
          FeedbackData::Options{
              .snapshot_config = *snapshot_config,
              .snapshot_exclusion_config = *snapshot_exclusion_config,
              .is_first_instance = component.IsFirstInstance(),
              .limit_inspect_data = feedback_config->build_type_config.enable_limit_inspect_data,
              .delete_previous_boot_logs_time = delete_previous_boot_logs_time,
              .root_job = GetRootJobForInspect(),
          }});

  component.AddPublicService(main_service->GetHandler<fuchsia::feedback::LastRebootInfoProvider>());
  component.AddPublicService(main_service->GetHandler<fuchsia::feedback::CrashReporter>());
  component.AddPublicService(
      main_service->GetHandler<fuchsia::feedback::CrashReportingProductRegister>());
  component.AddPublicService(main_service->GetHandler<fuchsia::feedback::ComponentDataRegister>());
  component.AddPublicService(main_service->GetHandler<fuchsia::feedback::DataProvider>());

  component.RunLoop();
  return EXIT_SUCCESS;
}

}  // namespace forensics::feedback
