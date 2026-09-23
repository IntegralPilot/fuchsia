// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, Result};
use assembly_config_schema::board_config::Architecture;
use assembly_config_schema::platform_settings::development_support_config::DevelopmentSupportConfig;
use assembly_config_schema::platform_settings::graphics_config::GraphicsConfig;
use assembly_config_schema::platform_settings::health_check_config::HealthCheckConfig;
use assembly_config_schema::platform_settings::memory_allocator_config::MemoryAllocatorConfig;
use assembly_config_schema::platform_settings::starnix_config::PlatformStarnixConfig;
use assembly_config_schema::product_settings::ComponentPolicyConfig;
use assembly_constants::{BootfsDestination, FileEntry};
use camino::Utf8PathBuf;
use cm_config::{InjectedCapabilities, InjectedUse, InjectedUseProtocol};
use component_manager_config::{Args, compile};
use std::fs::File;
use std::path::PathBuf;

pub(crate) struct ComponentConfig<'a> {
    pub policy: &'a ComponentPolicyConfig,
    pub development_support: &'a DevelopmentSupportConfig,
    pub starnix: &'a PlatformStarnixConfig,
    pub health_check: &'a HealthCheckConfig,
    pub memory_allocator: &'a MemoryAllocatorConfig,
    pub graphics: &'a GraphicsConfig,
}

pub(crate) struct ComponentSubsystem;
impl DefineSubsystemConfiguration<ComponentConfig<'_>> for ComponentSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &ComponentConfig<'_>,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let gendir = context.get_gendir().context("Getting gendir for component subsystem")?;

        // If heapdump has been enabled on at least one program, verify that it's allowed and
        // include the bundle containing heapdump's collector package. If the
        // "enable_assembly_heapdump" feature is on, forcefully enable it in compatible builds.
        let mut heapdump_config = config.development_support.heapdump.clone();
        if cfg!(feature = "enable_assembly_heapdump")
            && *context.build_type == BuildType::Eng
            && *context.feature_set_level == FeatureSetLevel::Standard
        {
            heapdump_config.component_manager = true;
            heapdump_config.monikers = vec!["/**".to_string()];
        };
        if heapdump_config.is_enabled() {
            context.ensure_build_type_and_feature_set_level(
                &[BuildType::Eng],
                &[FeatureSetLevel::Standard],
                "heapdump",
            )?;
            builder.platform_bundle("heapdump_global_collector")?;
        }

        // Select the component manager bundle to use.
        if heapdump_config.component_manager {
            builder.platform_bundle("component_manager_with_tracing_and_heapdump")?;
        } else if config
            .development_support
            .tracing_enabled(*context.feature_set_level, *context.build_type)?
        {
            builder.platform_bundle("component_manager_with_tracing")?;
        } else {
            builder.platform_bundle("component_manager")?;
        }

        // Add base policies.
        let mut input = vec![
            context.get_resource("component_manager_policy_base.json5"),
            context.get_resource("component_manager_policy_build_type_base.json5"),
            context.get_resource("bootfs_config.json5"),
        ];

        // Apply platform policies specific to subsystems.
        if config.starnix.enabled {
            input.push(context.get_resource("component_manager_policy_starnix.json5"));
        }

        // LINT.IfChange(vulkan_loader_lavapipe)
        let default_lavapipe = matches!(
            (&context.board_config.arch, context.build_type),
            (Architecture::X64, BuildType::Eng)
        );
        // LINT.ThenChange(//src/lib/assembly/platform_configuration/src/subsystems/graphics.rs:vulkan_loader_lavapipe)
        let allow_lavapipe = config.graphics.vulkan_icd.allow_lavapipe.unwrap_or(default_lavapipe);
        if allow_lavapipe {
            input.push(context.get_resource("component_manager_policy_vulkan_loader.json5"));
        }

        let monikers: Vec<&str> = config
            .health_check
            .verify_components
            .iter()
            .map(|verify_component| verify_component.source_moniker())
            .collect();

        let write_config = |name: &str, value: serde_json::Value| -> Result<Utf8PathBuf> {
            let path = gendir.join(name);
            let file = File::create(&path).with_context(|| format!("Creating config: {name}"))?;
            serde_json::to_writer_pretty(file, &value)
                .with_context(|| format!("Writing config: {name}"))?;
            Ok(path)
        };
        let health_checks_source = write_config(
            "ota_health_check_config.json",
            serde_json::json!(
            {
                "health_check" : {
                    "monikers": monikers,
                },
            }
            ),
        )?;

        input.push(health_checks_source);

        if !heapdump_config.monikers.is_empty() {
            let injected_capabilities = InjectedCapabilities {
                components: heapdump_config
                    .monikers
                    .iter()
                    .map(|m| m.parse())
                    .collect::<Result<_, _>>()
                    .context("Invalid moniker in Heapdump configuration")?,
                use_: vec![InjectedUse::Protocol(InjectedUseProtocol {
                    // The two hardcoded strings below are guaranteed to be a valid capability name
                    // and a valid path. Therefore, parsing will never fail.
                    source_name: "fuchsia.memory.heapdump.process.Registry"
                        .parse()
                        .expect("failed to parse Heapdump's capability name"),
                    target_path: "/svc/fuchsia.memory.heapdump.process.Registry"
                        .parse()
                        .expect("failed to parse Heapdump's path"),
                })],
            };
            let heapdump_monikers_source = write_config(
                "heapdump_monikers.json",
                serde_json::json!(
                    {
                        "inject_capabilities": [injected_capabilities],
                    }
                ),
            )?;
            input.push(heapdump_monikers_source);
        }

        if !config.memory_allocator.scudo_options.is_empty() {
            let scudo_config_source = write_config(
                "scudo_config.json",
                serde_json::json!(
                    {
                        "scudo_options": config.memory_allocator.scudo_options.iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect::<Vec<_>>()
                            .join(",")
                    }
                ),
            )?;
            input.push(scudo_config_source);
        }

        // Collect the platform policies based on build-type.
        match (context.build_type, config.development_support.include_sl4f) {
            // The eng policies are given to Eng and UserDebug builds that also include sl4f.
            (BuildType::Eng, _) | (BuildType::UserDebug, true) => {
                input.push(context.get_resource("component_manager_policy.json5"));
                input.push(context.get_resource("component_manager_policy_eng.json5"));
            }
            (BuildType::UserDebug, false) => {
                input.push(context.get_resource("component_manager_policy_userdebug.json5"));
            }
            (BuildType::User, _) => {
                input.push(context.get_resource("component_manager_policy_user.json5"));
            }
        }

        let input = input.into_iter().map(PathBuf::from).collect();

        // Collect the product policies.
        let product =
            config.policy.product_policies.iter().map(|p| PathBuf::from(p.as_std_path())).collect();

        // Compile the final policy config file.
        let config = gendir.join("config.json5");
        let output = config.clone().into();
        let args = Args { input, product, output };
        compile(args).context("Compiling the component_manager config")?;

        // Add the policy to the system.
        builder
            .bootfs()
            .file(FileEntry {
                source: config,
                destination: BootfsDestination::ComponentManagerConfig,
            })
            .context("Adding component_manager config")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::define_configuration;
    use crate::subsystems::{AssemblyMode, BoardConfig, PlatformSettings, ProductSettings};
    use assembly_config_schema::platform_settings::memory_allocator_config::MemoryAllocatorConfig;
    use assembly_platform_artifacts::PlatformArtifacts;
    use camino::Utf8Path;
    use fidl::unpersist;
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    fn compile_component_manager_config(
        platform: PlatformSettings,
    ) -> anyhow::Result<fidl_fuchsia_component_internal::Config> {
        let product = ProductSettings::default();
        let gendir = tempdir().unwrap();
        let gendir_path = Utf8Path::from_path(gendir.path()).unwrap();

        let resdir = tempdir().unwrap();
        let resdir_path = Utf8Path::from_path(resdir.path()).unwrap();
        std::fs::write(resdir_path.join("component_manager_policy_base.json5"), "{}").unwrap();
        std::fs::write(resdir_path.join("component_manager_policy_build_type_base.json5"), "{}")
            .unwrap();
        std::fs::write(resdir_path.join("bootfs_config.json5"), "{}").unwrap();
        std::fs::write(resdir_path.join("component_manager_policy.json5"), "{}").unwrap();
        std::fs::write(resdir_path.join("component_manager_policy_eng.json5"), "{}").unwrap();
        std::fs::write(resdir_path.join("component_manager_policy_vulkan_loader.json5"), "{}")
            .unwrap();
        std::fs::write(resdir_path.join("core_component_id_index.json5"), "{ instances: [] }")
            .unwrap();
        std::fs::write(
            resdir_path.join("default_sampler_config.json5"),
            "{ fire_project_templates: [], fire_component_configs: [], project_configs: [] }",
        )
        .unwrap();

        let _result = define_configuration(
            PlatformArtifacts::empty_for_test(),
            &platform,
            &product,
            &BoardConfig::default(),
            gendir_path,
            resdir_path,
            None,  // developer_only_options
            false, // include_example_aib_for_tests
            &AssemblyMode::default(),
        )
        .unwrap();

        Ok(unpersist::<fidl_fuchsia_component_internal::Config>(
            &fs::read(gendir_path.join("component/config.json5")).unwrap(),
        )?)
    }

    #[test]
    fn test_component_manager_minimal_configuration() {
        let platform = PlatformSettings { build_type: BuildType::Eng, ..Default::default() };
        let config = compile_component_manager_config(platform).unwrap();
        assert_eq!(
            config,
            fidl_fuchsia_component_internal::Config {
                security_policy: Some(Default::default()),
                health_check: Some(fidl_fuchsia_component_internal::HealthCheck {
                    monikers: Some(vec![]),
                    ..Default::default()
                }),
                inject_capabilities: Some(vec![]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_component_manager_with_scudo_options() {
        let platform = PlatformSettings {
            build_type: BuildType::Eng,
            memory_allocator: MemoryAllocatorConfig {
                scudo_options: BTreeMap::from([
                    ("A".to_string(), "B".to_string()),
                    ("C".to_string(), "D".to_string()),
                ]),
            },
            ..Default::default()
        };
        let config = compile_component_manager_config(platform).unwrap();
        assert_eq!(config.scudo_options, Some("A=B,C=D".to_string()));
    }
}
