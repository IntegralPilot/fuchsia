// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::io::Write as _;

use crate::subsystems::prelude::*;
use crate::util;
use anyhow::Context;
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::assembly_input_bundle::{
    CompiledComponentDefinition, CompiledPackageDefinition,
};
use assembly_config_schema::platform_settings::recovery_config::{RecoveryConfig, SystemRecovery};
use assembly_constants::{
    BootfsCompiledPackageDestination, CompiledPackageDestination, FileEntry, PackageDestination,
    PackageSetDestination,
};
use assembly_images_config::VolumeConfig;
use camino::Utf8PathBuf;

pub(crate) struct RecoverySubsystem;

impl DefineSubsystemConfiguration<(&RecoveryConfig, &VolumeConfig)> for RecoverySubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        configs: &(&RecoveryConfig, &VolumeConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let (config, volume_config) = *configs;

        let gendir = context.get_gendir().context("getting gen dir for recovery subsystem")?;

        if let Some(mapping) = &config.factory_reset_trigger_config {
            // If configuration is provided for the factory-reset-trigger component, include it
            // and the configuration in the build.

            builder.platform_bundle("factory_reset_trigger")?;

            let config = serde_json::json!({
                "version": "1",
                "content": {
                    "channel_indices": mapping
                }
            });

            let config_file_path = gendir.join("forced-fdr-channel-indices.config");
            let config_file = File::create(&config_file_path).with_context(|| {
                format!("Creating factory-reset-trigger config file: {config_file_path}")
            })?;
            serde_json::to_writer_pretty(config_file, &config).with_context(|| {
                format!("Writing factory-reset-trigger config file: {config_file_path}")
            })?;

            builder
                .package("factory-reset-trigger")
                .config_data(FileEntry {
                    source: config_file_path,
                    destination: "forced-fdr-channel-indices.config".into(),
                })
                .context("Adding factory-reset-trigger config data entry")?;
        }

        if config.has_factory_reset(*context.feature_set_level) {
            builder.platform_bundle("factory_reset")?;
        }

        // factory_reset needs to know which mutable filesystem to use, in order to properly
        // reset it.  The value is always provided in case factory_reset has been added directly
        // by a product, and not through assembly.
        builder.set_config_capability(
            "fuchsia.recovery.UseFxBlob",
            Config::new(
                ConfigValueType::Bool,
                match volume_config {
                    VolumeConfig::Fxfs => true,
                    VolumeConfig::Fvm(_) => false,
                }
                .into(),
            ),
        )?;

        if let Some(system_recovery) = &config.system_recovery {
            match system_recovery {
                SystemRecovery::Fdr => {
                    context
                        .ensure_feature_set_level(&[FeatureSetLevel::Utility], "System Recovery")?;
                    builder.platform_bundle("recovery_fdr")?;
                    configure_platform_system_recovery(context, config, builder)?;
                }
                SystemRecovery::Android => {
                    context.ensure_feature_set_level(
                        &[FeatureSetLevel::Utility],
                        "Android System Recovery",
                    )?;
                    builder.platform_bundle("recovery_android")?;
                    configure_platform_system_recovery(context, config, builder)?;
                    builder.set_config_capability(
                        "fuchsia.adb.IsRecovery",
                        Config::new(ConfigValueType::Bool, true.into()),
                    )?;
                }
                SystemRecovery::Bootfs(bootfs_recovery_config) => {
                    // Bootfs recovery can be part of a product directly and so is allowed at Standard.
                    context.ensure_feature_set_level(
                        &[FeatureSetLevel::Utility, FeatureSetLevel::Standard],
                        "Bootfs Recovery",
                    )?;

                    configure_bootfs_recovery(
                        context,
                        builder,
                        &gendir,
                        &bootfs_recovery_config.product_component_url,
                        !bootfs_recovery_config.disable_eager_startup,
                    )?;
                }
            }
        }

        // system-recovery-fdr needs to know the board's display rotation so that it can
        // appropriately display the logo.
        //
        // This needs to always be set, in case recovery is being added by products directly,
        // and not via assembly.
        if let Some(display_rotation) = &context.board_config.platform.graphics.display.rotation {
            builder.set_config_capability(
                "fuchsia.recovery.DisplayRotation",
                Config::new(
                    ConfigValueType::Uint16,
                    u16::try_from(*display_rotation)
                        .context("converting 'display_rotation' to 16-bits")?
                        .into(),
                ),
            )?;
        } else {
            builder
                .set_config_capability("fuchsia.recovery.DisplayRotation", Config::new_void())?;
        }
        Ok(())
    }
}

/// Helper function to configure platform-provided system recovery
fn configure_platform_system_recovery(
    context: &ConfigurationContext<'_>,
    config: &RecoveryConfig,
    builder: &mut dyn ConfigurationBuilder,
) -> anyhow::Result<()> {
    // Create the recovery domain configuration package
    let directory = builder
        .add_domain_config(PackageSetDestination::Blob(PackageDestination::SystemRecoveryConfig))
        .directory("system-recovery-config");

    let logo_source = if let Some(logo) = &config.logo {
        logo.clone()
    } else {
        context.get_resource("fuchsia-logo.riv")
    };
    directory
        .entry(FileEntry { source: logo_source, destination: "logo.riv".to_owned() })
        .context("Adding logo to system-recovery-config")?;

    if let Some(instructions_source) = &config.instructions {
        directory
            .entry(FileEntry {
                source: instructions_source.clone(),
                destination: "instructions.txt".to_owned(),
            })
            .context("Adding instructions.txt to system-recovery-config")?;
    }

    if config.check_for_managed_mode {
        directory
            .entry_from_contents("check_fdr_restriction.json", "{}")
            .context("Adding check_fdr_restriction.json to system-recovery_config")?;
    }

    Ok(())
}

/// Helper function to configure bootfs recovery
fn configure_bootfs_recovery(
    context: &ConfigurationContext<'_>,
    builder: &mut dyn ConfigurationBuilder,
    gendir: &Utf8PathBuf,
    product_component_url: &str,
    eager_startup: bool,
) -> anyhow::Result<()> {
    let cml_template = context.get_resource("bootfs_recovery.bootstrap_shard.cml.template");
    let cml_template = std::fs::read_to_string(cml_template.clone())
        .with_context(|| format!("Reading template: {cml_template}"))?;

    let cml =
        util::render_bootfs_cml_template(product_component_url, eager_startup, &cml_template)?;

    let cml_name = "bootfs_recovery.cml";
    let cml_path = gendir.join(cml_name);
    let mut cml_file = std::fs::File::create(&cml_path)
        .with_context(|| format!("Creating cml file: {}", cml_path))?;
    cml_file
        .write_all(cml.as_bytes())
        .with_context(|| format!("Writing cml file: {}", cml_path))?;
    let components = vec![CompiledComponentDefinition {
        component_name: "bootstrap".into(),
        shards: vec![cml_path],
        cmc_features: vec![],
    }];
    let destination = CompiledPackageDestination::Boot(BootfsCompiledPackageDestination::Bootstrap);
    let def = CompiledPackageDefinition {
        name: destination.clone(),
        components,
        contents: vec![],
        includes: vec![],
        bootfs_package: true,
    };
    builder
        .compiled_package(destination.clone(), def)
        .with_context(|| format!("Inserting compiled package: {destination}"))?;

    Ok(())
}
