// Copyright 2021 The Fuchsia Authors.All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/config.h"

#include <lib/syslog/cpp/macros.h>

#include <optional>

#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/utils/storage_size.h"
#include "src/lib/files/file.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/error/en.h"
#include "third_party/rapidjson/include/rapidjson/error/error.h"
#include "third_party/rapidjson/include/rapidjson/schema.h"
#include "third_party/rapidjson/include/rapidjson/stringbuffer.h"

namespace forensics::feedback {
namespace {

template <typename T>
std::optional<T> ReadConfig(const std::string& schema_str,
                            std::function<std::optional<T>(const rapidjson::Document&)> convert_fn,
                            const std::string& filepath) {
  std::string config_str;
  if (!files::ReadFileToString(filepath, &config_str)) {
    FX_LOGS(ERROR) << "Error reading config file at " << filepath;
    return std::nullopt;
  }

  rapidjson::Document config;
  if (const rapidjson::ParseResult result = config.Parse(config_str.c_str()); !result) {
    FX_LOGS(ERROR) << "Error parsing config as JSON at offset " << result.Offset() << " "
                   << rapidjson::GetParseError_En(result.Code());
    return std::nullopt;
  }

  rapidjson::Document schema;
  if (const rapidjson::ParseResult result = schema.Parse(schema_str); !result) {
    FX_LOGS(ERROR) << "Error parsing config schema at offset " << result.Offset() << " "
                   << rapidjson::GetParseError_En(result.Code());
    return std::nullopt;
  }

  rapidjson::SchemaDocument schema_doc(schema);
  if (rapidjson::SchemaValidator validator(schema_doc); !config.Accept(validator)) {
    rapidjson::StringBuffer buf;
    validator.GetInvalidSchemaPointer().StringifyUriFragment(buf);
    FX_LOGS(ERROR) << "Config does not match schema, violating '"
                   << validator.GetInvalidSchemaKeyword() << "' rule";
    return std::nullopt;
  }

  return convert_fn(config);
}

template <typename T>
std::optional<T> GetConfig(const std::string& schema_str,
                           std::function<std::optional<T>(const rapidjson::Document&)> convert_fn,
                           const std::string& config_type, const std::string& default_path,
                           const std::optional<std::string>& override_path) {
  std::optional<T> config;
  if (override_path.has_value() && files::IsFile(*override_path)) {
    if (config = ReadConfig<T>(schema_str, convert_fn, *override_path); !config.has_value()) {
      FX_LOGS(ERROR) << "Failed to read override " << config_type << " config file at "
                     << *override_path;
    }
  }

  if (!config.has_value()) {
    if (config = ReadConfig<T>(schema_str, convert_fn, default_path); !config.has_value()) {
      FX_LOGS(ERROR) << "Failed to read default " << config_type << " config file at "
                     << default_path;
    }
  }

  return config;
}

constexpr char kSnapshotConfigSchema[] = R"({
  "type": "object",
  "properties": {
    "annotation_allowlist": {
      "type": "array",
      "items": {
        "type": "string"
      },
      "uniqueItems": true
    },
    "attachment_allowlist": {
      "type": "array",
      "items": {
        "type": "string"
      },
      "uniqueItems": true
    }
  },
  "required": [
    "annotation_allowlist",
    "attachment_allowlist"
  ],
  "additionalProperties": false
})";

std::optional<SnapshotConfig> ParseSnapshotConfig(const rapidjson::Document& json) {
  SnapshotConfig config;
  for (const auto& k : json["annotation_allowlist"].GetArray()) {
    config.default_annotations.insert(k.GetString());
  }

  for (const auto& k : json["attachment_allowlist"].GetArray()) {
    config.attachment_allowlist.insert(k.GetString());
  }

  return config;
}

// This config may be defined outside of fuchsia.git, so to allow for easier migrations these
// properties shouldn't be required.
constexpr char kSnapshotExclusionConfigSchema[] = R"({
  "type": "object",
  "properties": {
    "excluded_annotations": {
      "type": "array",
      "items": {
        "type": "string"
      },
      "uniqueItems": true
    }
  },
  "additionalProperties": false
})";

std::optional<SnapshotExclusionConfig> ParseSnapshotExclusionConfig(
    const rapidjson::Document& json) {
  if (!json.HasMember("excluded_annotations")) {
    return SnapshotExclusionConfig();
  }

  SnapshotExclusionConfig config;
  for (const auto& k : json["excluded_annotations"].GetArray()) {
    config.excluded_annotations.insert(k.GetString());
  }

  return config;
}

constexpr char kFeedbackConfigSchema[] = R"({
  "type": "object",
  "properties": {
    "report_persistence_max_cache_size_kib": {
      "type": "integer",
      "minimum": 1
    },
    "report_persistence_max_tmp_size_kib": {
      "type": "integer",
      "minimum": 1
    },
    "snapshot_persistence_max_cache_size_mib": {
      "type": "number"
    },
    "snapshot_persistence_max_tmp_size_mib": {
      "type": "number"
    },
    "spontaneous_reboot_reason": {
      "type": "string",
      "enum": [
        "spontaneous",
        "brief_power_loss",
        "hard_reset"
      ]
    },
    "crash_report_upload_policy": {
      "type": "string",
      "enum": [
        "disabled",
        "enabled",
        "read_from_privacy_settings"
      ]
    },
    "daily_per_product_crash_report_quota": {
      "type": "number"
    },
    "enable_data_redaction": {
      "type": "boolean"
    },
    "enable_hourly_snapshots": {
      "type": "boolean"
    },
    "enable_limit_inspect_data": {
      "type": "boolean"
    },
    "remote_device_id_provider": {
      "type": "boolean"
    },
    "supports_user_initiated_poweroffs": {
      "type": "boolean"
    }
  },
  "required": [
    "report_persistence_max_cache_size_kib",
    "report_persistence_max_tmp_size_kib",
    "snapshot_persistence_max_cache_size_mib",
    "snapshot_persistence_max_tmp_size_mib",
    "spontaneous_reboot_reason",
    "crash_report_upload_policy",
    "daily_per_product_crash_report_quota",
    "enable_data_redaction",
    "enable_hourly_snapshots",
    "enable_limit_inspect_data",
    "remote_device_id_provider",
    "supports_user_initiated_poweroffs"
  ],
  "additionalProperties": false
})";

std::optional<FeedbackConfig> ParseFeedbackConfig(const rapidjson::Document& json) {
  FeedbackConfig config;

  const uint64_t max_report_cache_size_kib = json[kReportPersistenceMaxCacheSizeKey].GetUint64();
  config.report_persistence_max_cache_size = StorageSize::Kilobytes(max_report_cache_size_kib);

  const uint64_t max_report_tmp_size_kib = json[kReportPersistenceMaxTmpSizeKey].GetUint64();
  config.report_persistence_max_tmp_size = StorageSize::Kilobytes(max_report_tmp_size_kib);

  if (const int64_t max_cache_size_mib = json[kSnapshotPersistenceMaxCacheSizeKey].GetInt64();
      max_cache_size_mib > 0) {
    config.snapshot_persistence_max_cache_size = StorageSize::Megabytes(max_cache_size_mib);
  } else {
    config.snapshot_persistence_max_cache_size = std::nullopt;
  }

  if (const int64_t max_tmp_size_mib = json[kSnapshotPersistenceMaxTmpSizeKey].GetInt64();
      max_tmp_size_mib > 0) {
    config.snapshot_persistence_max_tmp_size = StorageSize::Megabytes(max_tmp_size_mib);
  } else {
    config.snapshot_persistence_max_tmp_size = std::nullopt;
  }

  if (const std::string spontaneous_reboot_reason = json["spontaneous_reboot_reason"].GetString();
      spontaneous_reboot_reason == "spontaneous" ||
      spontaneous_reboot_reason == "brief_power_loss") {
    // TODO(https://fxbug.dev/441569016): "spontaneous" is the default in assembly, but "brief power
    // loss" is the historical default in Feedback. To reduce signature churn, Feedback will treat
    // "spontaneous" as "brief power loss" until all products have had a chance to specify their
    // desired reason.
    config.spontaneous_reboot_reason = SpontaneousRebootReason::kBriefPowerLoss;
  } else if (spontaneous_reboot_reason == "hard_reset") {
    config.spontaneous_reboot_reason = SpontaneousRebootReason::kHardReset;
  }

  config.build_type_config.enable_data_redaction = json[kEnableDataRedactionKey].GetBool();
  config.build_type_config.enable_hourly_snapshots = json[kEnableHourlySnapshotsKey].GetBool();
  config.build_type_config.enable_limit_inspect_data = json[kEnableLimitInspectDataKey].GetBool();
  config.remote_device_id_provider = json["remote_device_id_provider"].GetBool();
  config.supports_user_initiated_poweroffs = json[kSupportsUserInitiatedPoweroffsKey].GetBool();

  if (const std::string policy = json[kCrashReportUploadPolicyKey].GetString();
      policy == "disabled") {
    config.build_type_config.crash_report_upload_policy = CrashReportUploadPolicy::kDisabled;
  } else if (policy == "enabled") {
    config.build_type_config.crash_report_upload_policy = CrashReportUploadPolicy::kEnabled;
  } else if (policy == "read_from_privacy_settings") {
    config.build_type_config.crash_report_upload_policy =
        CrashReportUploadPolicy::kReadFromPrivacySettings;
  } else {
    FX_LOGS(FATAL) << "Upload policy '" << policy << "' not permitted by schema";
  }

  if (const int64_t quota = json[kDailyPerProductCrashReportQuotaKey].GetInt64(); quota > 0) {
    config.build_type_config.daily_per_product_crash_report_quota = quota;
  } else {
    config.build_type_config.daily_per_product_crash_report_quota = std::nullopt;
  }

  return config;
}

}  // namespace

std::optional<SnapshotConfig> GetSnapshotConfig(const std::string& default_path) {
  return GetConfig<SnapshotConfig>(kSnapshotConfigSchema, ParseSnapshotConfig, "snapshot",
                                   default_path, std::nullopt);
}

std::optional<SnapshotExclusionConfig> GetSnapshotExclusionConfig(const std::string& path) {
  return GetConfig<SnapshotExclusionConfig>(kSnapshotExclusionConfigSchema,
                                            ParseSnapshotExclusionConfig, "snapshot exclusion",
                                            path, std::nullopt);
}

std::optional<FeedbackConfig> GetFeedbackConfig(const std::string& path) {
  return GetConfig<FeedbackConfig>(kFeedbackConfigSchema, ParseFeedbackConfig, "feedback", path,
                                   std::nullopt);
}

void ExposeConfig(inspect::Node& inspect_root, const FeedbackConfig& feedback_config) {
  const std::string crash_report_quota =
      feedback_config.build_type_config.daily_per_product_crash_report_quota.has_value()
          ? std::to_string(*feedback_config.build_type_config.daily_per_product_crash_report_quota)
          : "none";

  const std::string snapshot_persistence_tmp_size =
      feedback_config.snapshot_persistence_max_tmp_size.has_value()
          ? std::to_string(feedback_config.snapshot_persistence_max_tmp_size->ToMegabytes())
          : "none";

  const std::string snapshot_persistence_cache_size =
      feedback_config.snapshot_persistence_max_cache_size.has_value()
          ? std::to_string(feedback_config.snapshot_persistence_max_cache_size->ToMegabytes())
          : "none";

  inspect_root.RecordChild(
      kInspectConfigKey, [&feedback_config, &crash_report_quota, &snapshot_persistence_tmp_size,
                          &snapshot_persistence_cache_size](inspect::Node& node) {
        node.RecordString(kCrashReportUploadPolicyKey,
                          ToString(feedback_config.build_type_config.crash_report_upload_policy));
        node.RecordString(kDailyPerProductCrashReportQuotaKey, crash_report_quota);
        node.RecordBool(kEnableDataRedactionKey,
                        feedback_config.build_type_config.enable_data_redaction);
        node.RecordBool(kEnableHourlySnapshotsKey,
                        feedback_config.build_type_config.enable_hourly_snapshots);
        node.RecordBool(kEnableLimitInspectDataKey,
                        feedback_config.build_type_config.enable_limit_inspect_data);
        node.RecordBool(kSupportsUserInitiatedPoweroffsKey,
                        feedback_config.supports_user_initiated_poweroffs);

        node.RecordString(kSnapshotPersistenceMaxTmpSizeKey, snapshot_persistence_tmp_size);
        node.RecordString(kSnapshotPersistenceMaxCacheSizeKey, snapshot_persistence_cache_size);
      });
}

std::string ToString(const CrashReportUploadPolicy upload_policy) {
  switch (upload_policy) {
    case CrashReportUploadPolicy::kDisabled:
      return "DISABLED";
    case CrashReportUploadPolicy::kEnabled:
      return "ENABLED";
    case CrashReportUploadPolicy::kReadFromPrivacySettings:
      return "READ_FROM_PRIVACY_SETTINGS";
  }
}

}  // namespace forensics::feedback
