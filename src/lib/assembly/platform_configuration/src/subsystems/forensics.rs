// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use crate::util;
use assembly_config_schema::developer_overrides::{
    DeveloperOnlyOptions, FeedbackBuildTypeConfig, ForensicsOptions,
};
use assembly_config_schema::platform_settings::forensics_config::{
    DiskSize, FeedbackIdComponentUrl, ForensicsConfig, SpontaneousRebootReason,
};
use assembly_config_schema::platform_settings::session_config::PlatformSessionConfig;
use assembly_constants::{FileEntry, PackageDestination, PackageSetDestination};
use serde::{Deserialize, Serialize};

// Directory of feedback config (for feedback domain config).
const FEEDBACK_CONFIG_DIRECTORY: &str = "feedback-config";
// Filename of FeedbackInternalConfig file within feedback domain config.
const FEEDBACK_CONFIG_FILENAME: &str = "feedback_config.json";

#[derive(Clone, Copy, Debug, PartialEq)]
struct DiskSizedParams {
    report_cache_size_kib: u64,
    report_tmp_size_kib: u64,
    snapshot_storage_size_mib: i64,
}

// Even on disk-constrained devices, we want to store a few reports in /cache if possible.
const SMALL_DISK: DiskSizedParams = DiskSizedParams {
    report_cache_size_kib: 512,
    report_tmp_size_kib: 4608,
    // -1 indicates that snapshots should not be persisted to disk.
    snapshot_storage_size_mib: -1,
};

const MEDIUM_DISK: DiskSizedParams = DiskSizedParams {
    report_cache_size_kib: 10240,
    report_tmp_size_kib: 10240,
    snapshot_storage_size_mib: 10,
};

const LARGE_DISK: DiskSizedParams = DiskSizedParams {
    report_cache_size_kib: 25600,
    report_tmp_size_kib: 25600,
    snapshot_storage_size_mib: 25,
};

pub(crate) struct ForensicsSubsystem;
impl DefineSubsystemConfiguration<(&ForensicsConfig, &PlatformSessionConfig)>
    for ForensicsSubsystem
{
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        platform_config: &(&ForensicsConfig, &PlatformSessionConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let (config, session_config) = *platform_config;

        if config.feedback.include_kernel_logs_in_last_reboot_info {
            builder.platform_bundle("kernel_logs_in_reboot_info")?;
        }

        if *context.build_type != BuildType::Eng
            && let Some(DeveloperOnlyOptions {
                forensics_options: ForensicsOptions { build_type_override: Some(_), .. },
                ..
            }) = context.developer_only_options
        {
            anyhow::bail!("Feedback build type overrides only supported in eng build-types");
        }

        let build_type_config = match context.build_type {
            BuildType::User => BuildTypeConfig::user(),
            BuildType::UserDebug => BuildTypeConfig::userdebug(),
            BuildType::Eng => {
                if let Some(DeveloperOnlyOptions { forensics_options, .. }) =
                    context.developer_only_options
                {
                    match forensics_options.build_type_override {
                        Some(FeedbackBuildTypeConfig::EngWithUpload) => {
                            BuildTypeConfig::eng_with_upload()
                        }
                        Some(FeedbackBuildTypeConfig::UserDebug) => BuildTypeConfig::userdebug(),
                        Some(FeedbackBuildTypeConfig::User) => BuildTypeConfig::user(),
                        None => BuildTypeConfig::default_eng(),
                    }
                } else {
                    BuildTypeConfig::default_eng()
                }
            }
        };

        #[allow(deprecated)]
        let component_url = match (
            &config.feedback.component_url_for_remote_feedback_id,
            &config.feedback.feedback_id_component_url,
        ) {
            (FeedbackIdComponentUrl::None, url) => url,
            (url, FeedbackIdComponentUrl::None) => url,
            _ => anyhow::bail!(
                "component_url_for_remote_feedback_id and feedback_id_component_url cannot be both specified"
            ),
        };

        // Cobalt and Feedback may be added to anything utility and higher.
        if matches!(context.feature_set_level, FeatureSetLevel::Standard | FeatureSetLevel::Utility)
        {
            let config_dir = builder
                .add_domain_config(PackageSetDestination::Blob(PackageDestination::FeedbackConfig))
                .directory(FEEDBACK_CONFIG_DIRECTORY);

            config_dir.entry_from_contents(
                "snapshot_exclusion.json",
                &serde_json::to_string_pretty(&config.feedback.snapshot_exclusion)?,
            )?;

            // For backward compatibility, legacy configurations specifying `large_disk: true`
            // map to `Medium`.
            #[allow(deprecated)]
            let effective_disk_size = match (config.feedback.disk_size, config.feedback.large_disk)
            {
                (_, true) => DiskSize::Medium,
                (size, _) => size,
            };

            let disk_sized_params = match effective_disk_size {
                DiskSize::Small => SMALL_DISK,
                DiskSize::Medium => MEDIUM_DISK,
                DiskSize::Large => LARGE_DISK,
            };

            #[allow(deprecated)]
            let remote_device_id_provider = config.feedback.remote_device_id_provider
                || !matches!(component_url, FeedbackIdComponentUrl::None);

            let feedback_config = FeedbackInternalConfig {
                report_persistence_max_cache_size_kib: disk_sized_params.report_cache_size_kib,
                report_persistence_max_tmp_size_kib: disk_sized_params.report_tmp_size_kib,
                snapshot_persistence_max_cache_size_mib: disk_sized_params
                    .snapshot_storage_size_mib,
                snapshot_persistence_max_tmp_size_mib: disk_sized_params.snapshot_storage_size_mib,
                spontaneous_reboot_reason: config.feedback.spontaneous_reboot_reason,
                crash_report_upload_policy: build_type_config.crash_report_upload_policy,
                daily_per_product_crash_report_quota: build_type_config
                    .daily_per_product_crash_report_quota
                    .map_or(-1, |q| q as i64),
                enable_data_redaction: build_type_config.enable_data_redaction,
                enable_hourly_snapshots: build_type_config.enable_hourly_snapshots,
                enable_limit_inspect_data: build_type_config.enable_limit_inspect_data,
                remote_device_id_provider,
                supports_user_initiated_poweroffs: config
                    .feedback
                    .supports_user_initiated_poweroffs,
            };

            config_dir.entry_from_contents(
                FEEDBACK_CONFIG_FILENAME,
                &serde_json::to_string_pretty(&feedback_config)?,
            )?;

            match context.build_type {
                BuildType::User => builder.platform_bundle("cobalt_user_config")?,
                BuildType::UserDebug => builder.platform_bundle("cobalt_userdebug_config")?,
                BuildType::Eng => builder.platform_bundle("cobalt_default_config")?,
            }

            util::add_build_type_config_data("cobalt", context, builder)?;
            if let Some(api_key) = &config.cobalt.api_key {
                builder.package("cobalt").config_data(FileEntry {
                    source: api_key.clone(),
                    destination: "api_key.hex".into(),
                })?;
            }
        }

        match component_url {
            FeedbackIdComponentUrl::FlashTs(url) => {
                util::add_platform_declared_product_provided_component(
                    url,
                    "flash_ts_feedback_id.core_shard.cml.template",
                    context,
                    builder,
                )?;
            }
            FeedbackIdComponentUrl::SysInfo(url) => {
                util::add_platform_declared_product_provided_component(
                    url,
                    "sysinfo_feedback_id.core_shard.cml.template",
                    context,
                    builder,
                )?;
            }
            FeedbackIdComponentUrl::None => {
                if session_config.enabled {
                    builder.platform_bundle("no_remote_feedback_id")?;
                }
            }
        }

        Ok(())
    }
}

struct BuildTypeConfig {
    pub crash_report_upload_policy: CrashReportUploadPolicy,
    pub daily_per_product_crash_report_quota: Option<u64>,
    pub enable_data_redaction: bool,
    pub enable_hourly_snapshots: bool,
    pub enable_limit_inspect_data: bool,
}

impl BuildTypeConfig {
    fn default_eng() -> Self {
        Self {
            crash_report_upload_policy: CrashReportUploadPolicy::Disabled,
            daily_per_product_crash_report_quota: None,
            enable_data_redaction: false,
            enable_hourly_snapshots: false,
            enable_limit_inspect_data: false,
        }
    }

    fn user() -> Self {
        Self {
            crash_report_upload_policy: CrashReportUploadPolicy::ReadFromPrivacySettings,
            daily_per_product_crash_report_quota: Some(100),
            enable_data_redaction: true,
            enable_hourly_snapshots: false,
            enable_limit_inspect_data: true,
        }
    }

    fn userdebug() -> Self {
        Self {
            crash_report_upload_policy: CrashReportUploadPolicy::ReadFromPrivacySettings,
            daily_per_product_crash_report_quota: None,
            enable_data_redaction: false,
            enable_hourly_snapshots: true,
            enable_limit_inspect_data: false,
        }
    }

    fn eng_with_upload() -> Self {
        Self {
            crash_report_upload_policy: CrashReportUploadPolicy::Enabled,
            daily_per_product_crash_report_quota: None,
            enable_data_redaction: false,
            enable_hourly_snapshots: true,
            enable_limit_inspect_data: false,
        }
    }
}

// LINT.IfChange
#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum CrashReportUploadPolicy {
    #[default]
    // Crash reports will not be uploaded.
    Disabled,

    // The upload policy will be read from the privacy settings.
    ReadFromPrivacySettings,

    // Crash reports will be uploaded.
    Enabled,
}

/// The config that the Feedback component will actually consume. This is different than
/// FeedbackConfig because product owners don't need fine-grained control over some of the
/// fields.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
struct FeedbackInternalConfig {
    // The non-snapshot portions of reports, like annotations and the minidump, are stored on disk
    // under /cache if upload fails. Once full, reports will continue to be stored in memory-backed
    // /tmp. These values MUST be greater than 0.
    pub report_persistence_max_cache_size_kib: u64,
    pub report_persistence_max_tmp_size_kib: u64,

    // The snapshots of reports are stored on disk under /cache if upload fails. Once full,
    // snapshots will continue to be stored in memory-backed /tmp. A value of -1 should be used to
    // indicate that snapshots should not be stored in that location.
    pub snapshot_persistence_max_cache_size_mib: i64,
    pub snapshot_persistence_max_tmp_size_mib: i64,

    pub spontaneous_reboot_reason: SpontaneousRebootReason,

    pub supports_user_initiated_poweroffs: bool,

    pub crash_report_upload_policy: CrashReportUploadPolicy,
    pub daily_per_product_crash_report_quota: i64,
    pub enable_data_redaction: bool,
    pub enable_hourly_snapshots: bool,
    pub enable_limit_inspect_data: bool,
    pub remote_device_id_provider: bool,
}
// LINT.ThenChange(//src/developer/forensics/feedback/config.cc)

#[cfg(test)]
mod test {
    use assembly_config_schema::developer_overrides::{DeveloperOnlyOptions, ForensicsOptions};
    use assembly_config_schema::platform_settings::forensics_config::{DiskSize, FeedbackConfig};
    use camino::Utf8Path;

    use super::*;
    use crate::subsystems::ConfigurationBuilderImpl;
    use crate::{DomainConfig, DomainConfigDirectory, FileOrContents};

    fn get_feedback_config(
        build_type: BuildType,
        forensics_config: ForensicsConfig,
        developer_only_options: Option<&DeveloperOnlyOptions>,
    ) -> FeedbackInternalConfig {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &build_type,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options,
        };

        let session_config: PlatformSessionConfig = Default::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result = ForensicsSubsystem::define_configuration(
            &context,
            &(&forensics_config, &session_config),
            &mut builder,
        );
        assert!(result.is_ok());

        let config = builder.build();
        let domain_config: &DomainConfig = config
            .domain_configs
            .entries
            .get(&PackageSetDestination::Blob(PackageDestination::FeedbackConfig))
            .unwrap();

        let domain_config_directory: &DomainConfigDirectory =
            domain_config.directories.entries.get(FEEDBACK_CONFIG_DIRECTORY).unwrap();
        let feedback_config: &FileOrContents =
            domain_config_directory.entries.get(FEEDBACK_CONFIG_FILENAME).unwrap();
        let string_contents: &String = match &feedback_config {
            FileOrContents::Contents(string_contents) => string_contents,
            _ => panic!("FileOrContents::Contents expected"),
        };

        serde_json::from_str::<FeedbackInternalConfig>(string_contents).unwrap()
    }

    #[test]
    fn test_build_type_user() {
        let config =
            get_feedback_config(BuildType::User, ForensicsConfig::default(), Default::default());

        assert_eq!(
            config.crash_report_upload_policy,
            CrashReportUploadPolicy::ReadFromPrivacySettings
        );
        assert_eq!(config.daily_per_product_crash_report_quota, 100);
        assert!(config.enable_data_redaction);
        assert!(!config.enable_hourly_snapshots);
        assert!(config.enable_limit_inspect_data);
    }

    #[test]
    fn test_build_type_userdebug() {
        let config = get_feedback_config(
            BuildType::UserDebug,
            ForensicsConfig::default(),
            Default::default(),
        );
        assert_eq!(
            config.crash_report_upload_policy,
            CrashReportUploadPolicy::ReadFromPrivacySettings
        );
        assert_eq!(config.daily_per_product_crash_report_quota, -1);
        assert!(!config.enable_data_redaction);
        assert!(config.enable_hourly_snapshots);
        assert!(!config.enable_limit_inspect_data);
    }

    #[test]
    fn test_build_type_eng() {
        let config =
            get_feedback_config(BuildType::Eng, ForensicsConfig::default(), Default::default());
        assert_eq!(config.crash_report_upload_policy, CrashReportUploadPolicy::Disabled);
        assert_eq!(config.daily_per_product_crash_report_quota, -1);
        assert!(!config.enable_data_redaction);
        assert!(!config.enable_hourly_snapshots);
        assert!(!config.enable_limit_inspect_data);
    }

    #[test]
    fn test_build_type_eng_override_user() {
        let developer_only_options = DeveloperOnlyOptions {
            forensics_options: ForensicsOptions {
                build_type_override: Some(FeedbackBuildTypeConfig::User),
            },
            ..Default::default()
        };
        let config = get_feedback_config(
            BuildType::Eng,
            ForensicsConfig::default(),
            Some(&developer_only_options),
        );

        assert_eq!(
            config.crash_report_upload_policy,
            CrashReportUploadPolicy::ReadFromPrivacySettings
        );
        assert_eq!(config.daily_per_product_crash_report_quota, 100);
        assert!(config.enable_data_redaction);
        assert!(!config.enable_hourly_snapshots);
        assert!(config.enable_limit_inspect_data);
    }

    #[test]
    fn test_build_type_eng_override_userdebug() {
        let developer_only_options = DeveloperOnlyOptions {
            forensics_options: ForensicsOptions {
                build_type_override: Some(FeedbackBuildTypeConfig::UserDebug),
            },
            ..Default::default()
        };
        let config = get_feedback_config(
            BuildType::Eng,
            ForensicsConfig::default(),
            Some(&developer_only_options),
        );

        assert_eq!(
            config.crash_report_upload_policy,
            CrashReportUploadPolicy::ReadFromPrivacySettings
        );
        assert_eq!(config.daily_per_product_crash_report_quota, -1);
        assert!(!config.enable_data_redaction);
        assert!(config.enable_hourly_snapshots);
        assert!(!config.enable_limit_inspect_data);
    }

    #[test]
    fn test_build_type_eng_override_eng_with_upload() {
        let developer_only_options = DeveloperOnlyOptions {
            forensics_options: ForensicsOptions {
                build_type_override: Some(FeedbackBuildTypeConfig::EngWithUpload),
            },
            ..Default::default()
        };
        let config = get_feedback_config(
            BuildType::Eng,
            ForensicsConfig::default(),
            Some(&developer_only_options),
        );

        assert_eq!(config.crash_report_upload_policy, CrashReportUploadPolicy::Enabled);
        assert_eq!(config.daily_per_product_crash_report_quota, -1);
        assert!(!config.enable_data_redaction);
        assert!(config.enable_hourly_snapshots);
        assert!(!config.enable_limit_inspect_data);
    }

    #[test]
    fn test_build_type_override_bails_on_user_builds() {
        // Build type overrides are only allowed for eng builds.
        let developer_only_options = DeveloperOnlyOptions {
            forensics_options: ForensicsOptions {
                build_type_override: Some(FeedbackBuildTypeConfig::UserDebug),
            },
            ..Default::default()
        };

        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::User,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Some(&developer_only_options),
        };

        let forensics_config: ForensicsConfig = Default::default();
        let session_config: PlatformSessionConfig = Default::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result = ForensicsSubsystem::define_configuration(
            &context,
            &(&forensics_config, &session_config),
            &mut builder,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_build_type_override_bails_on_userdebug_builds() {
        // Build type overrides are only allowed for eng builds.
        let developer_only_options = DeveloperOnlyOptions {
            forensics_options: ForensicsOptions {
                build_type_override: Some(FeedbackBuildTypeConfig::EngWithUpload),
            },
            ..Default::default()
        };

        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::UserDebug,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Some(&developer_only_options),
        };

        let forensics_config: ForensicsConfig = Default::default();
        let session_config: PlatformSessionConfig = Default::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result = ForensicsSubsystem::define_configuration(
            &context,
            &(&forensics_config, &session_config),
            &mut builder,
        );

        assert!(result.is_err());
    }

    #[test]
    fn flash_ts_feedback_id_core_shard() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        std::fs::File::create(
            resource_dir.path().join("flash_ts_feedback_id.core_shard.cml.template"),
        )
        .unwrap();
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Utf8Path::from_path(resource_dir.path()).unwrap().to_path_buf(),
            developer_only_options: Default::default(),
        };

        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig {
                component_url_for_remote_feedback_id: FeedbackIdComponentUrl::FlashTs(
                    "fuchsia-pkg://fuchsia.com/test-package#meta/test-component.cm".to_string(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let session_config: PlatformSessionConfig = Default::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result = ForensicsSubsystem::define_configuration(
            &context,
            &(&forensics_config, &session_config),
            &mut builder,
        );

        assert!(result.is_ok());
        assert!(
            builder
                .build()
                .core_shards
                .contains(&"flash_ts_feedback_id.core_shard.cml.template.rendered.cml".into())
        );
    }

    #[test]
    fn sysinfo_feedback_id_core_shard() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        std::fs::File::create(
            resource_dir.path().join("sysinfo_feedback_id.core_shard.cml.template"),
        )
        .unwrap();
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Utf8Path::from_path(resource_dir.path()).unwrap().to_path_buf(),
            developer_only_options: Default::default(),
        };

        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig {
                component_url_for_remote_feedback_id: FeedbackIdComponentUrl::SysInfo(
                    "fuchsia-pkg://fuchsia.com/test-package#meta/test-component.cm".to_string(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let session_config: PlatformSessionConfig = Default::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result = ForensicsSubsystem::define_configuration(
            &context,
            &(&forensics_config, &session_config),
            &mut builder,
        );

        assert!(result.is_ok());
        assert!(
            builder
                .build()
                .core_shards
                .contains(&"sysinfo_feedback_id.core_shard.cml.template.rendered.cml".into())
        );
    }

    #[test]
    fn feedback_config_default_spontaneous_reboot_reason() {
        let config =
            get_feedback_config(BuildType::Eng, ForensicsConfig::default(), Default::default());
        assert_eq!(config.spontaneous_reboot_reason, SpontaneousRebootReason::Spontaneous);
    }

    #[test]
    fn feedback_config_nondefault_spontaneous_reboot_reason() {
        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig {
                spontaneous_reboot_reason: SpontaneousRebootReason::BriefPowerLoss,
                ..Default::default()
            },
            ..Default::default()
        };

        let config = get_feedback_config(BuildType::Eng, forensics_config, Default::default());
        assert_eq!(config.spontaneous_reboot_reason, SpontaneousRebootReason::BriefPowerLoss);
    }

    #[test]
    fn feedback_config_default_disk_size() {
        let config =
            get_feedback_config(BuildType::Eng, ForensicsConfig::default(), Default::default());

        assert_eq!(config.report_persistence_max_cache_size_kib, SMALL_DISK.report_cache_size_kib);
        assert_eq!(config.report_persistence_max_tmp_size_kib, SMALL_DISK.report_tmp_size_kib);
        assert_eq!(
            config.snapshot_persistence_max_cache_size_mib,
            SMALL_DISK.snapshot_storage_size_mib
        );
        assert_eq!(
            config.snapshot_persistence_max_tmp_size_mib,
            SMALL_DISK.snapshot_storage_size_mib
        );
    }

    #[test]
    #[allow(deprecated)]
    fn feedback_config_large_disk() {
        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig { large_disk: true, ..Default::default() },
            ..Default::default()
        };
        let config = get_feedback_config(BuildType::Eng, forensics_config, Default::default());

        assert_eq!(config.report_persistence_max_cache_size_kib, MEDIUM_DISK.report_cache_size_kib);
        assert_eq!(config.report_persistence_max_tmp_size_kib, MEDIUM_DISK.report_tmp_size_kib);
        assert_eq!(
            config.snapshot_persistence_max_cache_size_mib,
            MEDIUM_DISK.snapshot_storage_size_mib
        );
        assert_eq!(
            config.snapshot_persistence_max_tmp_size_mib,
            MEDIUM_DISK.snapshot_storage_size_mib
        );
    }

    #[test]
    fn feedback_config_disk_size_small() {
        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig { disk_size: DiskSize::Small, ..Default::default() },
            ..Default::default()
        };
        let config = get_feedback_config(BuildType::Eng, forensics_config, Default::default());

        assert_eq!(config.report_persistence_max_cache_size_kib, SMALL_DISK.report_cache_size_kib);
        assert_eq!(config.report_persistence_max_tmp_size_kib, SMALL_DISK.report_tmp_size_kib);
        assert_eq!(
            config.snapshot_persistence_max_cache_size_mib,
            SMALL_DISK.snapshot_storage_size_mib
        );
        assert_eq!(
            config.snapshot_persistence_max_tmp_size_mib,
            SMALL_DISK.snapshot_storage_size_mib
        );
    }

    #[test]
    fn feedback_config_disk_size_medium() {
        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig { disk_size: DiskSize::Medium, ..Default::default() },
            ..Default::default()
        };
        let config = get_feedback_config(BuildType::Eng, forensics_config, Default::default());

        assert_eq!(config.report_persistence_max_cache_size_kib, MEDIUM_DISK.report_cache_size_kib);
        assert_eq!(config.report_persistence_max_tmp_size_kib, MEDIUM_DISK.report_tmp_size_kib);
        assert_eq!(
            config.snapshot_persistence_max_cache_size_mib,
            MEDIUM_DISK.snapshot_storage_size_mib
        );
        assert_eq!(
            config.snapshot_persistence_max_tmp_size_mib,
            MEDIUM_DISK.snapshot_storage_size_mib
        );
    }

    #[test]
    fn feedback_config_disk_size_large() {
        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig { disk_size: DiskSize::Large, ..Default::default() },
            ..Default::default()
        };
        let config = get_feedback_config(BuildType::Eng, forensics_config, Default::default());

        assert_eq!(config.report_persistence_max_cache_size_kib, LARGE_DISK.report_cache_size_kib);
        assert_eq!(config.report_persistence_max_tmp_size_kib, LARGE_DISK.report_tmp_size_kib);
        assert_eq!(
            config.snapshot_persistence_max_cache_size_mib,
            LARGE_DISK.snapshot_storage_size_mib
        );
        assert_eq!(
            config.snapshot_persistence_max_tmp_size_mib,
            LARGE_DISK.snapshot_storage_size_mib
        );
    }

    #[test]
    fn feedback_config_remote_device_id_provider_default_false() {
        let forensics_config = ForensicsConfig::default();
        let config = get_feedback_config(BuildType::Eng, forensics_config, Default::default());

        assert!(!config.remote_device_id_provider);
    }

    #[test]
    fn feedback_config_remote_device_id_provider_inferred_true() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        std::fs::File::create(
            resource_dir.path().join("flash_ts_feedback_id.core_shard.cml.template"),
        )
        .unwrap();
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Utf8Path::from_path(resource_dir.path()).unwrap().to_path_buf(),
            developer_only_options: Default::default(),
        };

        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig {
                component_url_for_remote_feedback_id: FeedbackIdComponentUrl::FlashTs(
                    "fuchsia-pkg://fuchsia.com/test-package#meta/test-component.cm".to_string(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let session_config: PlatformSessionConfig = Default::default();
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result = ForensicsSubsystem::define_configuration(
            &context,
            &(&forensics_config, &session_config),
            &mut builder,
        );
        assert!(result.is_ok());

        let config = builder.build();
        let domain_config: &DomainConfig = config
            .domain_configs
            .entries
            .get(&PackageSetDestination::Blob(PackageDestination::FeedbackConfig))
            .unwrap();
        let domain_config_directory: &DomainConfigDirectory =
            domain_config.directories.entries.get(FEEDBACK_CONFIG_DIRECTORY).unwrap();
        let feedback_config: &FileOrContents =
            domain_config_directory.entries.get(FEEDBACK_CONFIG_FILENAME).unwrap();
        let string_contents: &String = match &feedback_config {
            FileOrContents::Contents(string_contents) => string_contents,
            _ => panic!("FileOrContents::Contents expected"),
        };
        let forensics_config =
            serde_json::from_str::<FeedbackInternalConfig>(string_contents).unwrap();
        assert!(forensics_config.remote_device_id_provider);
    }

    #[test]
    #[allow(deprecated)]
    fn feedback_config_remote_device_id_provider_true() {
        let forensics_config = ForensicsConfig {
            feedback: FeedbackConfig { remote_device_id_provider: true, ..Default::default() },
            ..Default::default()
        };
        let config = get_feedback_config(BuildType::Eng, forensics_config, Default::default());

        assert!(config.remote_device_id_provider);
    }
}
