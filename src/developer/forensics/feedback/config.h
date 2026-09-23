// Copyright 2021 The Fuchsia Authors.All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_CONFIG_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_CONFIG_H_

#include <lib/inspect/cpp/vmo/types.h>

#include <optional>
#include <set>
#include <string>

#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/utils/storage_size.h"

namespace forensics::feedback {

// Policy defining whether to upload pending and future crash reports to a remote crash server.
enum class CrashReportUploadPolicy {
  // Crash reports should not be uploaded and be kept in the store.
  kDisabled,

  // Crash reports should be uploaded and on success removed from the store, if present.
  // If the upload is unsuccessful and the policy changes to kDisabled, the crash report should
  // follow the kDisabled policy.
  kEnabled,

  // Policy should not be read from the config, but instead from the privacy settings.
  kReadFromPrivacySettings,
};

enum class SpontaneousRebootReason : std::uint8_t {
  kSpontaneous,
  kBriefPowerLoss,
  kHardReset,
};

struct BuildTypeConfig {
  CrashReportUploadPolicy crash_report_upload_policy;
  std::optional<uint64_t> daily_per_product_crash_report_quota;
  bool enable_data_redaction;
  bool enable_hourly_snapshots;
  bool enable_limit_inspect_data;
};

struct SnapshotConfig {
  std::set<std::string> default_annotations;
  std::set<std::string> attachment_allowlist;
};

struct SnapshotExclusionConfig {
  std::set<std::string> excluded_annotations;
};

// This should match FeedbackInternalConfig in
// //src/lib/assembly/platform_configuration/src/subsystems/forensics.rs.
struct FeedbackConfig {
  StorageSize report_persistence_max_cache_size;
  StorageSize report_persistence_max_tmp_size;
  std::optional<StorageSize> snapshot_persistence_max_cache_size;
  std::optional<StorageSize> snapshot_persistence_max_tmp_size;
  SpontaneousRebootReason spontaneous_reboot_reason;
  bool remote_device_id_provider;
  bool supports_user_initiated_poweroffs;
  BuildTypeConfig build_type_config;
};

std::optional<SnapshotConfig> GetSnapshotConfig(
    const std::string& path = kDefaultSnapshotConfigPath);

std::optional<SnapshotExclusionConfig> GetSnapshotExclusionConfig(
    const std::string& path = kDefaultSnapshotExclusionConfigPath);

std::optional<FeedbackConfig> GetFeedbackConfig(const std::string& path = kFeedbackConfigPath);

// Exposes the static configuration based on build type and product.
void ExposeConfig(inspect::Node& inspect_root, const FeedbackConfig& feedback_config);

// Returns the string version of the enum.
std::string ToString(CrashReportUploadPolicy upload_policy);

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_CONFIG_H_
