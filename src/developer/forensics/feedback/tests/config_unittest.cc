// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/config.h"

#include <lib/inspect/testing/cpp/inspect.h>

#include <initializer_list>
#include <optional>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace forensics::feedback {
namespace {

using inspect::testing::BoolIs;
using inspect::testing::ChildrenMatch;
using inspect::testing::NameMatches;
using inspect::testing::NodeMatches;
using inspect::testing::PropertyList;
using inspect::testing::StringIs;
using inspect::testing::UintIs;
using testing::ElementsAreArray;
using testing::IsEmpty;
using testing::IsSupersetOf;

constexpr auto kConfigDisabled = CrashReportUploadPolicy::kDisabled;
constexpr auto kConfigEnabled = CrashReportUploadPolicy::kEnabled;
constexpr auto kConfigReadFromPrivacySettings = CrashReportUploadPolicy::kReadFromPrivacySettings;

auto BuildConfigMatcher(
    std::initializer_list<testing::Matcher<const inspect::PropertyValue&>> properties) {
  return ChildrenMatch(Contains(AllOf(
      NodeMatches(AllOf(NameMatches(kInspectConfigKey), PropertyList(IsSupersetOf(properties)))))));
}

class ConfigTest : public testing::Test {
 public:
  // Writes |config| to a file and returns the path of the config.
  std::string WriteConfig(const std::string& config) {
    std::string path;
    FX_CHECK(temp_dir_.NewTempFileWithData(config, &path));
    return path;
  }

 private:
  files::ScopedTempDir temp_dir_;
};

class SnapshotConfigTest : public ConfigTest {
 protected:
  std::optional<SnapshotConfig> ParseConfig(const std::string& config) {
    return GetSnapshotConfig(WriteConfig(config));
  }
};

class SnapshotExclusionConfigTest : public ConfigTest {
 protected:
  std::optional<SnapshotExclusionConfig> ParseConfig(const std::string& config) {
    return GetSnapshotExclusionConfig(WriteConfig(config));
  }
};

class FeedbackConfigTest : public ConfigTest {
 protected:
  std::optional<FeedbackConfig> ParseConfig(const std::string& config) {
    return GetFeedbackConfig(WriteConfig(config));
  }
};

using InspectConfigTest = UnitTestFixture;

TEST_F(FeedbackConfigTest, MissingCrashReportUploadPolicy) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingDailyPerProductCrashReportQuota) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingEnableDataRedaction) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingEnableHourlySnapshots) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingEnableLimitInspectData) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingRemoteDeviceIdProvider) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, CrashReportUploadPolicyDisabled) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->build_type_config.crash_report_upload_policy,
            CrashReportUploadPolicy::kDisabled);
}

TEST_F(FeedbackConfigTest, CrashReportUploadPolicyEnabled) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "enabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->build_type_config.crash_report_upload_policy,
            CrashReportUploadPolicy::kEnabled);
}

TEST_F(FeedbackConfigTest, CrashReportUploadPolicyReadFromPrivacySettings) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "read_from_privacy_settings",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->build_type_config.crash_report_upload_policy,
            CrashReportUploadPolicy::kReadFromPrivacySettings);
}

TEST_F(FeedbackConfigTest, CrashReportUploadPolicyNotAllowedValue) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "not_allowed",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, CrashReportUploadPolicyNotString) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": 0,
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, DailyPerProductCrashReportQuotaNegative) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->build_type_config.daily_per_product_crash_report_quota, std::nullopt);
}

TEST_F(FeedbackConfigTest, DailyPerProductCrashReportQuotaZero) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": 0,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->build_type_config.daily_per_product_crash_report_quota, std::nullopt);
}

TEST_F(FeedbackConfigTest, DailyPerProductCrashReportQuotaPositive) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": 100,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->build_type_config.daily_per_product_crash_report_quota, 100);
}

TEST_F(FeedbackConfigTest, DailyPerProductCrashReportQuotaNotNumber) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": "",
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, EnableDataRedactionTrue) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": true,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_TRUE(config->build_type_config.enable_data_redaction);
}

TEST_F(FeedbackConfigTest, EnableDataRedactionFalse) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->build_type_config.enable_data_redaction);
}

TEST_F(FeedbackConfigTest, EnableDataRedactionNotBoolean) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": "",
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, EnableHourlySnapshotsTrue) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": true,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_TRUE(config->build_type_config.enable_hourly_snapshots);
}

TEST_F(FeedbackConfigTest, EnableHourlySnapshotsFalse) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->build_type_config.enable_hourly_snapshots);
}

TEST_F(FeedbackConfigTest, EnableHourlySnapshotsNotBoolean) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": "",
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, EnableLimitInspectDataTrue) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": true,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_TRUE(config->build_type_config.enable_limit_inspect_data);
}

TEST_F(FeedbackConfigTest, EnableLimitInspectDataFalse) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->build_type_config.enable_limit_inspect_data);
}

TEST_F(FeedbackConfigTest, EnableLimitInspectDataNotBoolean) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": "",
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, RemoteDeviceIdProviderTrue) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": true,
    "supports_user_initiated_poweroffs": true
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_TRUE(config->remote_device_id_provider);
}

TEST_F(FeedbackConfigTest, RemoteDeviceIdProviderFalse) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->remote_device_id_provider);
}

TEST_F(FeedbackConfigTest, RemoteDeviceIdProviderNotBoolean) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": "",
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, SupportsUserInitiatedPoweroffsTrue) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": true
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_TRUE(config->supports_user_initiated_poweroffs);
}

TEST_F(FeedbackConfigTest, SupportsUserInitiatedPoweroffsFalse) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->supports_user_initiated_poweroffs);
}

TEST_F(FeedbackConfigTest, SupportsUserInitiatedPoweroffsNotBoolean) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": ""
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, MissingAnnotationAllowlist) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "attachment_allowlist": []
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, MissingAttachmentAllowlist) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": []
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, SpuriousField) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": [],
  "attachment_allowlist": [],
  "spurious": ""
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, AnnotationAllowlistNotArray) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": "",
  "attachment_allowlist": []
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, AttachmentAllowlistNotArray) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": [],
  "attachment_allowlist": ""
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, AnnotationAllowlistNotArrayOfStrings) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": [1],
  "attachment_allowlist": []
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, AttachmentAllowlistNotArrayOfStrings) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": [],
  "attachment_allowlist": [1]
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, AnnotationAllowlistDuplicateItems) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": ["a", "a"],
  "attachment_allowlist": []
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, AttachmentAllowlistDuplicateItems) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": [],
  "attachment_allowlist": ["a", "a"]
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotConfigTest, AnnotationAllowlistEmpty) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": [],
  "attachment_allowlist": ["a"]
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_THAT(config->default_annotations, IsEmpty());
}

TEST_F(SnapshotConfigTest, AttachmentAllowlistEmpty) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": ["a"],
  "attachment_allowlist": []
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_THAT(config->attachment_allowlist, IsEmpty());
}

TEST_F(SnapshotConfigTest, AnnotationAllowlistNonEmpty) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": ["a", "b"],
  "attachment_allowlist": []
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_THAT(config->default_annotations, ElementsAreArray({"a", "b"}));
}

TEST_F(SnapshotConfigTest, AttachmentAllowlistNonEmpty) {
  const std::optional<SnapshotConfig> config = ParseConfig(R"({
  "annotation_allowlist": [],
  "attachment_allowlist": ["a", "b"]
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_THAT(config->attachment_allowlist, ElementsAreArray({"a", "b"}));
}

TEST_F(SnapshotExclusionConfigTest, MissingExcludedAnnotations) {
  const std::optional<SnapshotExclusionConfig> config = ParseConfig(R"({})");

  ASSERT_TRUE(config.has_value());
  EXPECT_THAT(config->excluded_annotations, IsEmpty());
}

TEST_F(SnapshotExclusionConfigTest, SpuriousField) {
  const std::optional<SnapshotExclusionConfig> config = ParseConfig(R"({
    "excluded_annotations": [],
    "spurious": ""
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotExclusionConfigTest, ExcludedAnnotationsNotArray) {
  const std::optional<SnapshotExclusionConfig> config = ParseConfig(R"({
    "excluded_annotations": ""
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotExclusionConfigTest, ExcludedAnnotationsNotArrayOfStrings) {
  const std::optional<SnapshotExclusionConfig> config = ParseConfig(R"({
    "excluded_annotations": [1]
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotExclusionConfigTest, ExcludedAnnotationsDuplicateItems) {
  const std::optional<SnapshotExclusionConfig> config = ParseConfig(R"({
    "excluded_annotations": ["a", "a"]
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(SnapshotExclusionConfigTest, ExcludedAnnotationsEmpty) {
  const std::optional<SnapshotExclusionConfig> config = ParseConfig(R"({
    "excluded_annotations": []
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_THAT(config->excluded_annotations, IsEmpty());
}

TEST_F(SnapshotExclusionConfigTest, ExcludedAnnotationsNonEmpty) {
  const std::optional<SnapshotExclusionConfig> config = ParseConfig(R"({
    "excluded_annotations": ["a", "b"]
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_THAT(config->excluded_annotations, ElementsAreArray({"a", "b"}));
}

TEST_F(FeedbackConfigTest, MissingConfigs) {
  const std::optional<FeedbackConfig> config = GetFeedbackConfig("/bad/path");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingReportPersistenceMaxCacheSizeKib) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingReportPersistenceMaxTmpSizeKib) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingSnapshotPersistenceMaxCacheSizeMib) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingSnapshotPersistenceMaxTmpSizeMib) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, MissingSpontaneousRebootReason) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, SpuriousField) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false,
    "spurious": ""
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxCacheSizeMibPositive) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_EQ(config->report_persistence_max_cache_size, StorageSize::Kilobytes(1));
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxCacheSizeKibZero) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 0,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxCacheSizeKibNegative) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": -1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxCacheSizeKibNotNumber) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": "",
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxTmpSizeMibPositive) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_EQ(config->report_persistence_max_tmp_size, StorageSize::Kilobytes(1));
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxTmpSizeKibZero) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 0,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxTmpSizeKibNegative) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": -1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, ReportPersistenceMaxTmpSizeKibNotNumber) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": "",
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxCacheSizeMibPositive) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->snapshot_persistence_max_cache_size, StorageSize::Megabytes(1));
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxCacheSizeMibZero) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 0,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->snapshot_persistence_max_cache_size.has_value());
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxCacheSizeMibNegative) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": -1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->snapshot_persistence_max_cache_size.has_value());
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxCacheSizeMibNotNumber) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": "",
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxTmpSizeMibPositive) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->snapshot_persistence_max_tmp_size, StorageSize::Megabytes(1));
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxTmpSizeMibZero) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 0,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->snapshot_persistence_max_tmp_size.has_value());
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxTmpSizeMibNegative) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": -1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_FALSE(config->snapshot_persistence_max_tmp_size.has_value());
}

TEST_F(FeedbackConfigTest, SnapshotPersistenceMaxTmpSizeMibNotNumber) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": "",
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, SpontaneousRebootReasonNotAllowedValue) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "not_allowed",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, SpontaneousRebootReasonNotString) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": 0,
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  EXPECT_FALSE(config.has_value());
}

TEST_F(FeedbackConfigTest, SpontaneousRebootReasonSpontaneous) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "spontaneous",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->spontaneous_reboot_reason, SpontaneousRebootReason::kBriefPowerLoss);
}

TEST_F(FeedbackConfigTest, SpontaneousRebootReasonBriefPowerLoss) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "brief_power_loss",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->spontaneous_reboot_reason, SpontaneousRebootReason::kBriefPowerLoss);
}

TEST_F(FeedbackConfigTest, SpontaneousRebootReasonHardReset) {
  const std::optional<FeedbackConfig> config = ParseConfig(R"({
    "report_persistence_max_cache_size_kib": 1,
    "report_persistence_max_tmp_size_kib": 1,
    "snapshot_persistence_max_cache_size_mib": 1,
    "snapshot_persistence_max_tmp_size_mib": 1,
    "spontaneous_reboot_reason": "hard_reset",
    "crash_report_upload_policy": "disabled",
    "daily_per_product_crash_report_quota": -1,
    "enable_data_redaction": false,
    "enable_hourly_snapshots": false,
    "enable_limit_inspect_data": false,
    "remote_device_id_provider": false,
    "supports_user_initiated_poweroffs": false
})");

  ASSERT_TRUE(config.has_value());
  EXPECT_EQ(config->spontaneous_reboot_reason, SpontaneousRebootReason::kHardReset);
}

TEST_F(InspectConfigTest, ExposeConfig_UploadDisabled) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .crash_report_upload_policy = kConfigDisabled,
                                      },
                              });

  EXPECT_THAT(
      InspectTree(),
      BuildConfigMatcher({StringIs(kCrashReportUploadPolicyKey, ToString(kConfigDisabled))}));
}

TEST_F(InspectConfigTest, ExposeConfig_UploadEnabled) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .crash_report_upload_policy = kConfigEnabled,
                                      },
                              });

  EXPECT_THAT(
      InspectTree(),
      BuildConfigMatcher({StringIs(kCrashReportUploadPolicyKey, ToString(kConfigEnabled))}));
}

TEST_F(InspectConfigTest, ExposeConfig_UploadReadFromPrivacySettings) {
  ExposeConfig(InspectRoot(),
               FeedbackConfig{
                   .build_type_config =
                       BuildTypeConfig{
                           .crash_report_upload_policy = kConfigReadFromPrivacySettings,
                       },
               });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({StringIs(kCrashReportUploadPolicyKey,
                                           ToString(kConfigReadFromPrivacySettings))}));
}

TEST_F(InspectConfigTest, ExposeConfig_DailyPerProductCrashReportQuotaNone) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .daily_per_product_crash_report_quota = std::nullopt,
                                      },
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({StringIs(kDailyPerProductCrashReportQuotaKey, "none")}));
}

TEST_F(InspectConfigTest, ExposeConfig_DailyPerProductCrashReportQuotaPositive) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .daily_per_product_crash_report_quota = 1,
                                      },
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({StringIs(kDailyPerProductCrashReportQuotaKey, "1")}));
}

TEST_F(InspectConfigTest, ExposeConfig_EnableDataRedactionFalse) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .enable_data_redaction = false,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({BoolIs(kEnableDataRedactionKey, false)}));
}

TEST_F(InspectConfigTest, ExposeConfig_EnableDataRedactionTrue) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .enable_data_redaction = true,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({BoolIs(kEnableDataRedactionKey, true)}));
}

TEST_F(InspectConfigTest, ExposeConfig_EnableHourlySnapshotsFalse) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .enable_hourly_snapshots = false,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({BoolIs(kEnableHourlySnapshotsKey, false)}));
}

TEST_F(InspectConfigTest, ExposeConfig_EnableHourlySnapshotsTrue) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .enable_hourly_snapshots = true,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({BoolIs(kEnableHourlySnapshotsKey, true)}));
}

TEST_F(InspectConfigTest, ExposeConfig_EnableLimitInspectDataFalse) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .enable_limit_inspect_data = false,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({BoolIs(kEnableLimitInspectDataKey, false)}));
}

TEST_F(InspectConfigTest, ExposeConfig_EnableLimitInspectDataTrue) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .enable_limit_inspect_data = true,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({BoolIs(kEnableLimitInspectDataKey, true)}));
}

TEST_F(InspectConfigTest, ExposeConfig_BuildTypeEnableAll) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .crash_report_upload_policy = kConfigEnabled,
                                          .daily_per_product_crash_report_quota = 1,
                                          .enable_data_redaction = true,
                                          .enable_hourly_snapshots = true,
                                          .enable_limit_inspect_data = true,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({
                                 StringIs(kCrashReportUploadPolicyKey, ToString(kConfigEnabled)),
                                 StringIs(kDailyPerProductCrashReportQuotaKey, "1"),
                                 BoolIs(kEnableDataRedactionKey, true),
                                 BoolIs(kEnableHourlySnapshotsKey, true),
                                 BoolIs(kEnableLimitInspectDataKey, true),
                             }));
}

TEST_F(InspectConfigTest, ExposeConfig_SnapshotPersistenceMaxTmpSizeNone) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .snapshot_persistence_max_tmp_size = std::nullopt,
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({StringIs(kSnapshotPersistenceMaxTmpSizeKey, "none")}));
}

TEST_F(InspectConfigTest, ExposeConfig_SnapshotPersistenceMaxTmpSizePositive) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .snapshot_persistence_max_tmp_size = StorageSize::Megabytes(1),
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({StringIs(kSnapshotPersistenceMaxTmpSizeKey, "1")}));
}

TEST_F(InspectConfigTest, ExposeConfig_SnapshotPersistenceMaxCacheSizeNone) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .snapshot_persistence_max_cache_size = std::nullopt,
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({StringIs(kSnapshotPersistenceMaxCacheSizeKey, "none")}));
}

TEST_F(InspectConfigTest, ExposeConfig_SnapshotPersistenceMaxCacheSizePositive) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .snapshot_persistence_max_cache_size = StorageSize::Megabytes(1),
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({StringIs(kSnapshotPersistenceMaxCacheSizeKey, "1")}));
}

TEST_F(InspectConfigTest, ExposeConfig_SupportsUserInitiatedPoweroffsFalse) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .supports_user_initiated_poweroffs = false,
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({BoolIs(kSupportsUserInitiatedPoweroffsKey, false)}));
}

TEST_F(InspectConfigTest, ExposeConfig_SupportsUserInitiatedPoweroffsTrue) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .supports_user_initiated_poweroffs = true,
                              });

  EXPECT_THAT(InspectTree(),
              BuildConfigMatcher({BoolIs(kSupportsUserInitiatedPoweroffsKey, true)}));
}

TEST_F(InspectConfigTest, ExposeConfig_FeedbackConfigEnableAll) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .snapshot_persistence_max_cache_size = StorageSize::Megabytes(1),
                                  .snapshot_persistence_max_tmp_size = StorageSize::Megabytes(1),
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({
                                 StringIs(kSnapshotPersistenceMaxTmpSizeKey, "1"),
                                 StringIs(kSnapshotPersistenceMaxCacheSizeKey, "1"),
                                 BoolIs(kSupportsUserInitiatedPoweroffsKey, false),
                             }));
}

TEST_F(InspectConfigTest, ExposeConfig_EnableAll) {
  ExposeConfig(InspectRoot(), FeedbackConfig{
                                  .snapshot_persistence_max_cache_size = StorageSize::Megabytes(1),
                                  .snapshot_persistence_max_tmp_size = StorageSize::Megabytes(1),
                                  .supports_user_initiated_poweroffs = true,
                                  .build_type_config =
                                      BuildTypeConfig{
                                          .crash_report_upload_policy = kConfigEnabled,
                                          .daily_per_product_crash_report_quota = 1,
                                          .enable_data_redaction = true,
                                          .enable_hourly_snapshots = true,
                                          .enable_limit_inspect_data = true,
                                      },
                              });

  EXPECT_THAT(InspectTree(), BuildConfigMatcher({
                                 StringIs(kCrashReportUploadPolicyKey, ToString(kConfigEnabled)),
                                 StringIs(kDailyPerProductCrashReportQuotaKey, "1"),
                                 BoolIs(kEnableDataRedactionKey, true),
                                 BoolIs(kEnableHourlySnapshotsKey, true),
                                 BoolIs(kEnableLimitInspectDataKey, true),
                                 StringIs(kSnapshotPersistenceMaxTmpSizeKey, "1"),
                                 StringIs(kSnapshotPersistenceMaxCacheSizeKey, "1"),
                                 BoolIs(kSupportsUserInitiatedPoweroffsKey, true),
                             }));
}

}  // namespace
}  // namespace forensics::feedback
