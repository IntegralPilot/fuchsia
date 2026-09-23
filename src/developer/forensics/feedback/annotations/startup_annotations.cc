// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/startup_annotations.h"

#include <fuchsia/sysinfo/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <lib/fidl/cpp/string.h>
#include <lib/fidl/cpp/synchronous_interface_ptr.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/syscalls.h>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"
#include "src/developer/forensics/utils/time.h"
#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/trim.h"

namespace forensics::feedback {
namespace {

using FormatFn = fit::function<std::optional<std::string>(std::string_view)>;

std::optional<std::string> IdentityFn(const std::string_view s) { return std::string(s); }

ErrorOrString ReadAnnotation(const std::string& filepath, FormatFn format = IdentityFn) {
  std::string content;
  if (!files::ReadFileToString(filepath, &content)) {
    FX_LOGS(WARNING) << "Failed to read content from " << filepath;
    return ErrorOrString(Error::kFileReadFailure);
  }

  const std::optional<std::string> formatted = format(content);
  if (!formatted.has_value()) {
    FX_LOGS(WARNING) << "Failed to formatted content from " << filepath;
    return ErrorOrString(Error::kBadValue);
  }

  return ErrorOrString(std::string(fxl::TrimString(*formatted, "\r\n")));
}

ErrorOrString ReadAnnotationWithFallback(const std::string& filepath,
                                         const std::string& fallback_filepath) {
  std::string content;
  if (!files::ReadFileToString(filepath, &content)) {
    FX_LOGS(WARNING) << "Failed to read content from " << filepath;
    return ReadAnnotation(fallback_filepath);
  }
  return ErrorOrString(std::string(fxl::TrimString(content, "\r\n")));
}

ErrorOrString BoardName() {
  fuchsia::sysinfo::SysInfoSyncPtr sysinfo;

  if (const zx_status_t status = fdio_service_connect("/svc/fuchsia.sysinfo.SysInfo",
                                                      sysinfo.NewRequest().TakeChannel().release());
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Error connecting to sysinfo";
    return ErrorOrString(Error::kConnectionError);
  }

  ::fidl::StringPtr out_board_name;
  zx_status_t out_status;
  if (const zx_status_t status = sysinfo->GetBoardName(&out_status, &out_board_name);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to get device board name";
    return ErrorOrString(Error::kConnectionError);
  }
  if (out_status != ZX_OK) {
    FX_PLOGS(ERROR, out_status) << "Failed to get device board name";
    return ErrorOrString(Error::kBadValue);
  }
  if (!out_board_name) {
    FX_PLOGS(ERROR, out_status) << "Failed to get device board name";
    return ErrorOrString(Error::kMissingValue);
  }

  return ErrorOrString(out_board_name.value());
}

std::string NumCPUs() { return std::to_string(zx_system_get_num_cpus()); }

}  // namespace

Annotations GetStartupAnnotations(const FinalShutdownInfo& final_shutdown_info,
                                  SpontaneousRebootReason spontaneous_reboot_reason,
                                  const std::string& compilation_mode_path) {
  return {
      {kBuildBoardKey, ReadAnnotation(kBuildBoardPath)},
      {kBuildCompilationModeKey, ReadAnnotation(compilation_mode_path)},
      {kBuildProductKey, ReadAnnotation(kBuildProductPath)},
      {kBuildLatestCommitDateKey, ReadAnnotation(kBuildCommitDatePath)},
      {kBuildPlatformBackstopKey, ReadAnnotation(kBuildMinUtcStampPath, FormatSecondsSinceEpoch)},
      {kBuildVersionKey, ReadAnnotation(kCurrentBuildVersionPath)},
      {kBuildVersionPreviousBootKey, ReadAnnotation(kPreviousBuildVersionPath)},
      {kBuildPlatformVersionKey, ReadAnnotation(kCurrentBuildPlatformVersionPath)},
      {kBuildPlatformVersionPreviousBootKey, ReadAnnotation(kPreviousBuildPlatformVersionPath)},
      {kBuildProductVersionKey, ReadAnnotation(kCurrentBuildProductVersionPath)},
      {kBuildProductVersionPreviousBootKey,
       ReadAnnotationWithFallback(kPreviousBuildProductVersionPath, kPreviousBuildVersionPath)},
      {kDeviceBoardNameKey, BoardName()},
      {kDeviceNumCPUsKey, ErrorOrString(NumCPUs())},
      {kSystemBootIdCurrentKey, ReadAnnotation(kCurrentBootIdPath)},
      {kSystemBootIdPreviousKey, ReadAnnotation(kPreviousBootIdPath)},
      {kSystemBootIdTimelineKey, ReadAnnotation(kBootIdTimelinePath)},
      {kSystemLastRebootReasonKey,
       ErrorOrString(final_shutdown_info.ToSnapshotAnnotationReason(spontaneous_reboot_reason))},
      {kSystemLastRebootRuntimeKey, final_shutdown_info.ToSnapshotAnnotationRuntime()},
      {kSystemLastRebootTotalSuspendedTimeKey,
       final_shutdown_info.ToSnapshotAnnotationTotalSuspendedTime()},
      {kSystemLastRebootUptimeKey, final_shutdown_info.ToSnapshotAnnotationUptime()},
      {kSystemLastShutdownGracefulActionKey,
       final_shutdown_info.ToSnapshotAnnotationGracefulAction()},
  };
}

}  // namespace forensics::feedback
