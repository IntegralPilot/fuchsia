// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, Result, anyhow, bail};
use assembly_config_capabilities::{Config, ConfigNestedValueType, ConfigValueType};
use assembly_config_schema::platform_settings::swd_config::{
    OtaConfigs, PolicyConfig, PolicyLabels, SwdConfig, SwdTrustStore, UpdateChecker,
    VerificationFailureAction,
};
use assembly_constants::FileEntry;
use camino::Utf8PathBuf;
use std::fs::File;

// openssl pkey -in src/sys/pkg/bin/system-updater/keys/manifest_dev_key.pem -pubout -outform DER | tail -c 32 | xxd -p -c 32
const OTA_MANIFEST_DEV_PUBLIC_KEY: &str =
    "b1d9c9414b0a97a225da46ce6ae05753fa69cf722da706132ced72ccf48213e9";
const OTA_MANIFEST_PUBLIC_KEY_HEX_LEN: usize = 64;

/// SWD Configuration implementation detail which clarifies different subsystem behavior, derived
/// from the PolicyLabels
#[derive(Debug)]
pub struct PolicyLabelDetails {
    enable_dynamic_configuration: bool,
    persisted_repos_dir: bool,
    disable_executability_restrictions: bool,
}

#[allow(dead_code)]
impl PolicyLabelDetails {
    fn from_policy_labels(policy_labels: &PolicyLabels) -> Self {
        match policy_labels {
            PolicyLabels::BaseComponentsOnly => PolicyLabelDetails {
                enable_dynamic_configuration: false,
                disable_executability_restrictions: false,
                persisted_repos_dir: false,
            },
            PolicyLabels::LocalDynamicConfig => PolicyLabelDetails {
                enable_dynamic_configuration: true,
                disable_executability_restrictions: false,
                persisted_repos_dir: false,
            },
            PolicyLabels::Unrestricted => PolicyLabelDetails {
                enable_dynamic_configuration: true,
                disable_executability_restrictions: true,
                persisted_repos_dir: true,
            },
        }
    }
}

pub(crate) struct SwdSubsystemConfig;
impl DefineSubsystemConfiguration<SwdConfig> for SwdSubsystemConfig {
    /// Configures the SWD system. If a specific field is not specified, a
    /// default will be used based on the feature set level and the build type.
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        subsystem_config: &SwdConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        match &subsystem_config.update_checker {
            // The product set a specific update checker. Use that one.
            Some(update_checker) => {
                Self::set_update_checker(update_checker, context.build_type, builder)?;
                Self::set_policy_by_build_type(context.build_type, context, builder)?;
            }
            // The product does not specify. Set based on feature set level.
            None => {
                match context.feature_set_level {
                    // Standard has an update checker
                    FeatureSetLevel::Standard => {
                        let update_checker =
                            UpdateChecker::default_by_build_type(context.build_type);
                        Self::set_update_checker(&update_checker, context.build_type, builder)?;
                        Self::set_policy_by_build_type(context.build_type, context, builder)?;
                    }
                    // Utility has no update checker
                    FeatureSetLevel::Utility => {
                        builder.platform_bundle("no_update_checker")?;
                    }
                    // Bootstrap and Embeddable have neither an update checker nor the system-update
                    // realm, so do not include `no_update_checker` AIB that requires the realm.
                    FeatureSetLevel::Bootstrap => {}
                    FeatureSetLevel::Embeddable => {}
                };
            }
        }

        for tuf_config in &subsystem_config.tuf_config_paths {
            let path: Utf8PathBuf = tuf_config.clone();
            let filename = path.file_name().ok_or_else(|| {
                anyhow!("Failed to get the filename from the tuf config: {}", tuf_config)
            })?;
            builder.package("pkg-resolver").config_data(FileEntry {
                source: tuf_config.clone(),
                destination: format!("repositories/{filename}"),
            })?;
        }

        if subsystem_config.include_configurator {
            builder.platform_bundle("system_update_configurator")?;
        }

        builder.set_config_capability(
            "fuchsia.pkgcache.AllPackagesExecutable",
            Config::new(ConfigValueType::Bool, (context.build_type == &BuildType::Eng).into()),
        )?;
        builder.set_config_capability(
            "fuchsia.pkgcache.RequireSystemImage",
            Config::new(ConfigValueType::Bool, true.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.pkgcache.EnableUpgradablePackages",
            Config::new(ConfigValueType::Bool, subsystem_config.enable_upgradable_packages.into()),
        )?;

        let manifest_public_keys = if subsystem_config.ota_manifest_public_keys.is_empty()
            && matches!(context.build_type, BuildType::UserDebug | BuildType::Eng)
        {
            // Include a default dev key in userdebug and eng builds.
            [OTA_MANIFEST_DEV_PUBLIC_KEY].into()
        } else {
            for key in &subsystem_config.ota_manifest_public_keys {
                if key.len() != OTA_MANIFEST_PUBLIC_KEY_HEX_LEN {
                    bail!(
                        "OTA manifest public key must be {} hex characters, but {:?} has {}",
                        OTA_MANIFEST_PUBLIC_KEY_HEX_LEN,
                        key,
                        key.len(),
                    );
                }
            }
            (&*subsystem_config.ota_manifest_public_keys).into()
        };

        builder.set_config_capability(
            "fuchsia.system-updater.ManifestPublicKeys",
            Config::new(
                ConfigValueType::Vector {
                    nested_type: ConfigNestedValueType::String {
                        max_size: OTA_MANIFEST_PUBLIC_KEY_HEX_LEN as u32,
                    },
                    max_count: 10,
                },
                manifest_public_keys,
            ),
        )?;

        if matches!(context.feature_set_level, FeatureSetLevel::Standard | FeatureSetLevel::Utility)
        {
            let bundle_name = match subsystem_config.trust_store {
                SwdTrustStore::Public => "swd_trust_store_public",
                SwdTrustStore::Restricted => "swd_trust_store_restricted",
            };
            builder.platform_bundle(bundle_name)?;
        }

        Ok(())
    }
}

impl SwdSubsystemConfig {
    /// Configure which AIB to select based on the UpdateChecker
    fn set_update_checker(
        update_checker: &UpdateChecker,
        build_type: &BuildType,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        match update_checker {
            UpdateChecker::OmahaClient(OtaConfigs {
                channels_path,
                policy_config,
                include_empty_eager_config,
                ..
            }) => {
                if *include_empty_eager_config {
                    if build_type == &BuildType::User {
                        bail!("The empty_eager_config cannot be enabled on user builds");
                    }
                    builder.platform_bundle("omaha_client_empty_eager_config")?;
                }

                builder.platform_bundle("omaha_client")?;
                builder.set_config_capability(
                    "fuchsia.omaha.PeriodicIntervalMinutes",
                    Config::new(
                        ConfigValueType::Uint16,
                        policy_config.periodic_interval_minutes.into(),
                    ),
                )?;
                builder.set_config_capability(
                    "fuchsia.omaha.StartupDelaySeconds",
                    Config::new(
                        ConfigValueType::Uint16,
                        policy_config.startup_delay_seconds.into(),
                    ),
                )?;
                builder.set_config_capability(
                    "fuchsia.omaha.AllowRebootWhenIdle",
                    Config::new(ConfigValueType::Bool, policy_config.allow_reboot_when_idle.into()),
                )?;
                builder.set_config_capability(
                    "fuchsia.omaha.RetryDelaySeconds",
                    Config::new(ConfigValueType::Uint16, policy_config.retry_delay_seconds.into()),
                )?;
                builder.set_config_capability(
                    "fuchsia.omaha.FuzzPercentageRange",
                    Config::new(ConfigValueType::Uint8, policy_config.fuzz_percentage_range.into()),
                )?;

                if let Some(channel_config) = channels_path {
                    builder.package("omaha-client").config_data(FileEntry {
                        source: channel_config.clone(),
                        destination: "channel_config.json".into(),
                    })?;
                }
            }
            UpdateChecker::SystemUpdateChecker => {
                builder.platform_bundle("system_update_checker")?;
            }
            UpdateChecker::ProductProvided => {
                builder.platform_bundle("product_provided_update_checker")?;
            }
        }
        Ok(())
    }

    fn set_policy_by_build_type(
        build_type: &BuildType,
        context: &ConfigurationContext<'_>,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let gendir = context.get_gendir().context("Getting gendir for swd")?;

        let policy = PolicyLabels::default_by_build_type(build_type);
        let policy = PolicyLabelDetails::from_policy_labels(&policy);

        let write_config = |name: &str, value: serde_json::Value| -> Result<Utf8PathBuf> {
            let path = gendir.join(name);
            let file = File::create(&path).with_context(|| format!("Creating config: {name}"))?;
            serde_json::to_writer_pretty(file, &value)
                .with_context(|| format!("Writing config: {name}"))?;
            Ok(path)
        };

        if policy.persisted_repos_dir {
            let source = write_config(
                "pkg_resolver_repo_config.json",
                serde_json::json!({
                    "persisted_repos_dir": "repos",
                }),
            )?;
            builder
                .package("pkg-resolver")
                .config_data(FileEntry { source, destination: "persisted_repos_dir.json".into() })
                .context("Adding persisted repos dir config")?;
        }

        if policy.enable_dynamic_configuration {
            let source = write_config(
                "pkg_resolver_config.json",
                serde_json::json!({
                    "enable_dynamic_configuration": true,
                }),
            )?;
            builder
                .package("pkg-resolver")
                .config_data(FileEntry { source, destination: "config.json".into() })
                .context("Adding dynamic configuration config")?;
        }

        if policy.disable_executability_restrictions {
            builder.platform_bundle("pkgfs_disable_executability_restrictions")?;
        }

        Ok(())
    }
}

impl DefaultByBuildType for UpdateChecker {
    fn default_by_build_type(build_type: &BuildType) -> Self {
        match build_type {
            BuildType::Eng => UpdateChecker::SystemUpdateChecker,
            BuildType::UserDebug => UpdateChecker::OmahaClient(OtaConfigs {
                channels_path: None,
                server_url: None,
                policy_config: PolicyConfig::default(),
                include_empty_eager_config: false,
            }),
            BuildType::User => UpdateChecker::OmahaClient(OtaConfigs {
                channels_path: None,
                server_url: None,
                policy_config: PolicyConfig::default(),
                include_empty_eager_config: false,
            }),
        }
    }
}
impl DefaultByBuildType for PolicyLabels {
    fn default_by_build_type(build_type: &BuildType) -> Self {
        match build_type {
            BuildType::Eng => PolicyLabels::Unrestricted,
            BuildType::UserDebug => PolicyLabels::LocalDynamicConfig,
            BuildType::User => PolicyLabels::BaseComponentsOnly,
        }
    }
}

/// SWD Configuration implementation detail which allows the product owner to define custom failure
/// recovery rules for the system-update-committer
#[derive(Clone, Debug, PartialEq)]
struct VerificationFailureConfigDetails {
    enable: bool,
    blobfs: String,
}

impl VerificationFailureConfigDetails {
    #[allow(dead_code)]
    fn from_verification_failure_config(vfc: VerificationFailureAction) -> Self {
        match vfc {
            VerificationFailureAction::Reboot => VerificationFailureConfigDetails {
                enable: true,
                blobfs: "reboot_on_failure".to_string(),
            },
            VerificationFailureAction::Disabled => {
                VerificationFailureConfigDetails { enable: false, blobfs: "ignore".to_string() }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swd_config_default_from_build_type_eng() {
        let build_type = &BuildType::Eng;
        let config = SwdConfig {
            policy: None,
            update_checker: None,
            on_verification_failure: VerificationFailureAction::default(),
            tuf_config_paths: vec![],
            ..Default::default()
        };
        let policy = config.policy.value_or_default_from_build_type(build_type);
        let update_checker = config.update_checker.value_or_default_from_build_type(build_type);
        let on_verification_failure = VerificationFailureAction::default();

        assert_eq!(policy, PolicyLabels::Unrestricted);
        assert_eq!(update_checker, UpdateChecker::SystemUpdateChecker);
        assert_eq!(on_verification_failure, VerificationFailureAction::Reboot);
    }
    #[test]
    fn test_swd_config_default_from_build_type_userdebug() {
        let build_type = &BuildType::UserDebug;
        let config = SwdConfig {
            policy: None,
            update_checker: None,
            on_verification_failure: VerificationFailureAction::default(),
            tuf_config_paths: vec![],
            ..Default::default()
        };
        let policy = config.policy.value_or_default_from_build_type(build_type);
        let update_checker = config.update_checker.value_or_default_from_build_type(build_type);
        let on_verification_failure = VerificationFailureAction::default();

        assert_eq!(policy, PolicyLabels::LocalDynamicConfig);
        assert_eq!(
            update_checker,
            UpdateChecker::OmahaClient(OtaConfigs {
                channels_path: None,
                server_url: None,
                policy_config: PolicyConfig::default(),
                include_empty_eager_config: false,
            })
        );
        assert_eq!(on_verification_failure, VerificationFailureAction::Reboot);
    }
    #[test]
    fn test_swd_config_default_from_build_type_user() {
        let build_type = &BuildType::User;
        let config = SwdConfig {
            policy: None,
            update_checker: None,
            on_verification_failure: VerificationFailureAction::default(),
            tuf_config_paths: vec![],
            ..Default::default()
        };
        let policy = config.policy.value_or_default_from_build_type(build_type);
        let update_checker = config.update_checker.value_or_default_from_build_type(build_type);
        let on_verification_failure = VerificationFailureAction::default();

        assert_eq!(policy, PolicyLabels::BaseComponentsOnly);
        assert_eq!(
            update_checker,
            UpdateChecker::OmahaClient(OtaConfigs {
                channels_path: None,
                server_url: None,
                policy_config: PolicyConfig::default(),
                include_empty_eager_config: false,
            })
        );
        assert_eq!(on_verification_failure, VerificationFailureAction::Reboot);
    }

    #[test]
    fn test_swd_trust_store_config() {
        use crate::subsystems::ConfigurationBuilderImpl;

        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };

        // Case 1: Default (should be public)
        {
            let mut builder: ConfigurationBuilderImpl = Default::default();
            let config = SwdConfig::default();
            let result = SwdSubsystemConfig::define_configuration(&context, &config, &mut builder);
            assert!(result.is_ok());
            let completed_config = builder.build();
            assert!(completed_config.bundles.contains("swd_trust_store_public"));
            assert!(!completed_config.bundles.contains("swd_trust_store_restricted"));
        }

        // Case 2: Explicitly "restricted"
        {
            let mut builder: ConfigurationBuilderImpl = Default::default();
            let config = SwdConfig { trust_store: SwdTrustStore::Restricted, ..Default::default() };
            let result = SwdSubsystemConfig::define_configuration(&context, &config, &mut builder);
            assert!(result.is_ok());
            let completed_config = builder.build();
            assert!(completed_config.bundles.contains("swd_trust_store_restricted"));
            assert!(!completed_config.bundles.contains("swd_trust_store_public"));
        }

        // Case 3: Explicitly "public"
        {
            let mut builder: ConfigurationBuilderImpl = Default::default();
            let config = SwdConfig { trust_store: SwdTrustStore::Public, ..Default::default() };
            let result = SwdSubsystemConfig::define_configuration(&context, &config, &mut builder);
            assert!(result.is_ok());
            let completed_config = builder.build();
            assert!(completed_config.bundles.contains("swd_trust_store_public"));
            assert!(!completed_config.bundles.contains("swd_trust_store_restricted"));
        }
    }
}
