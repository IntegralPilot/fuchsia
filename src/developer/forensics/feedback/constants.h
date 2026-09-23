// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_CONSTANTS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_CONSTANTS_H_

#include <lib/zx/time.h>

#include "src/developer/forensics/utils/storage_size.h"

namespace forensics::feedback {

const char kDefaultSnapshotConfigPath[] = "/pkg/data/snapshot/default_config.json";

const char kFeedbackConfigPath[] = "/feedback-config/feedback_config.json";
const char kDefaultSnapshotExclusionConfigPath[] = "/feedback-config/snapshot_exclusion.json";

constexpr char kInspectConfigKey[] = "config";

constexpr uint64_t kPersistedLogsNumFiles = 8;
constexpr StorageSize kPersistedLogsTotalSize = StorageSize::Kilobytes(512);

constexpr char kReportPersistenceMaxTmpSizeKey[] = "report_persistence_max_tmp_size_kib";
constexpr char kReportPersistenceMaxCacheSizeKey[] = "report_persistence_max_cache_size_kib";
constexpr char kSnapshotPersistenceMaxTmpSizeKey[] = "snapshot_persistence_max_tmp_size_mib";
constexpr char kSnapshotPersistenceMaxCacheSizeKey[] = "snapshot_persistence_max_cache_size_mib";

constexpr char kCrashReportUploadPolicyKey[] = "crash_report_upload_policy";
constexpr char kDailyPerProductCrashReportQuotaKey[] = "daily_per_product_crash_report_quota";
constexpr char kEnableDataRedactionKey[] = "enable_data_redaction";
constexpr char kEnableHourlySnapshotsKey[] = "enable_hourly_snapshots";
constexpr char kEnableLimitInspectDataKey[] = "enable_limit_inspect_data";
constexpr char kSupportsUserInitiatedPoweroffsKey[] = "supports_user_initiated_poweroffs";

///////////////////////////////////////////////////////////////////////////////////////////////////
// Reboot reporting
///////////////////////////////////////////////////////////////////////////////////////////////////

// TODO(https://fxbug.dev/450904906): Stop reading from legacy file paths once it's guaranteed they
// no longer exist.
constexpr char kLegacyPreviousGracefulRebootReasonFile[] = "/tmp/graceful_reboot_reason.txt";
constexpr char kLegacyCurrentGracefulRebootReasonFile[] = "/data/graceful_reboot_reason.txt";

constexpr char kPreviousGracefulShutdownInfoFile[] = "/tmp/graceful_shutdown_info.json";
constexpr char kCurrentGracefulShutdownInfoFile[] = "/data/graceful_shutdown_info.json";
constexpr char kFinalShutdownInfoPath[] = "/tmp/final_shutdown_info.json";

constexpr char kNotAFdrFile[] = "/data/not_a_fdr.txt";

// We file the crash report with a 90s delay to increase the likelihood that Inspect data (at
// all and specifically the data from memory_monitor) is included in the snapshot.zip generated
// by the Feedback service. The memory_monitor Inspect data is critical to debug OOM crash
// reports.
// TODO(https://fxbug.dev/42122827, https://fxbug.dev/42125347): remove delay.
constexpr zx::duration kOOMCrashReportingDelay = zx::sec(90);

///////////////////////////////////////////////////////////////////////////////////////////////////
// Crash reporting
///////////////////////////////////////////////////////////////////////////////////////////////////

const char kBuildVersionPath[] = "/config/build-info/version";
const char kBuildPlatformVersionPath[] = "/config/build-info/platform_version";
const char kBuildProductVersionPath[] = "/config/build-info/product_version";
const char kBuildBoardPath[] = "/config/build-info/board";
const char kBuildProductPath[] = "/config/build-info/product";
const char kBuildCommitDatePath[] = "/config/build-info/latest-commit-date";
const char kBuildMinUtcStampPath[] = "/boot/config/build_info/minimum_utc_stamp";
const char kBuildCompilationModePath[] = "/pkg/data/build_compilation_mode.txt";

constexpr char kCrashRegisterPath[] = "/tmp/crash_register.json";
constexpr char kProductQuotasPath[] = "/cache/product_quotas.json";
constexpr char kCrashServerUrl[] = "https://clients2.google.com/cr/report";
constexpr char kGarbageCollectedSnapshotsPath[] = "/tmp/garbage_collected_snapshots.txt";

// Snapshots can occupy up to 10 MB of memory.
constexpr StorageSize kSnapshotArchivesMaxSize = StorageSize::Megabytes(10);

// If a crash report arrives within |kSnapshotSharedRequestWindow| of a call to
// SnapshotManager::GetSnapshotUuid that schedules a call to
// fuchsia.feedback.DataProvider/GetSnapshot, the returned snapshot will be used in the resulting
// report.
//
// If the value it too large, crash reports may take too long to generate, but if the value is too
// small, the benefits of combining calls to fuchsia.feedback.DataProvider/GetSnapshot may not be
// fully realized.
constexpr zx::duration kSnapshotSharedRequestWindow = zx::sec(5);

///////////////////////////////////////////////////////////////////////////////////////////////////
// Feedback data
///////////////////////////////////////////////////////////////////////////////////////////////////
constexpr char kDeviceIdPath[] = "/data/device_id.txt";
constexpr char kCurrentLogsDir[] = "/cache/current_system_logs";
constexpr char kPreviousLogsFilePath[] = "/tmp/log.system.previous_boot.txt";
constexpr char kPreviousBootKernelLogPath[] = "/tmp/log.kernel.previous_boot.txt";
constexpr char kPreviousBootInspectPath[] = "/tmp/inspect.previous_boot.json";
constexpr char kPreviousDiskBackedLogsMetadataPath[] = "/tmp/disk_backed_logs_metadata.json";
constexpr char kCurrentDiskBackedLogsMetadataPath[] = "/cache/disk_backed_logs_metadata.json";
constexpr char kPreviousBootIdPath[] = "/tmp/boot_id.txt";
constexpr char kCurrentBootIdPath[] = "/data/boot_id.txt";
constexpr char kPreviousSystemTimePath[] = "/tmp/system_time.json";
constexpr char kCurrentSystemTimePath[] = "/cache/system_time.json";
constexpr char kBootIdTimelinePath[] = "/data/boot_id_timeline.txt";
constexpr char kPreviousBuildVersionPath[] = "/tmp/build_version.txt";
constexpr char kCurrentBuildVersionPath[] = "/data/build_version.txt";
constexpr char kPreviousBuildPlatformVersionPath[] = "/tmp/build_platform_version.txt";
constexpr char kCurrentBuildPlatformVersionPath[] = "/data/build_platform_version.txt";
constexpr char kPreviousBuildProductVersionPath[] = "/tmp/build_product_version.txt";
constexpr char kCurrentBuildProductVersionPath[] = "/data/build_product_version.txt";
constexpr char kDataRegisterPath[] = "/tmp/data_register.json";
constexpr char kKernelBootOptionsPath[] = "/boot/kernel/boot-options.txt";

constexpr zx::duration kSystemTimeTrackerWritePeriod = zx::sec(1);

constexpr size_t kRedactionIdCacheCapacity = 1500;

constexpr bool kEnableDiskBackedCurrentBootLogs = true;

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_CONSTANTS_H_
