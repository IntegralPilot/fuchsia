// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, bail};
use assembly_cli_args::AssemblyMode;
use assembly_config_schema::developer_overrides::DeveloperOnlyOptions;
use assembly_config_schema::platform_settings::PlatformSettings;
use assembly_config_schema::product_settings::ProductSettings;
use assembly_config_schema::{BoardConfig, ExampleConfig};
use assembly_platform_artifacts::PlatformArtifacts;
use camino::Utf8Path;

use crate::common::{CompletedConfiguration, ConfigurationBuilderImpl, ConfigurationContextBase};

pub(crate) mod prelude {

    #[allow(unused)]
    pub(crate) use crate::common::{
        BoardConfigExt, ComponentConfigBuilderExt, ConfigurationBuilder, ConfigurationContext,
        DefaultByBuildType, DefineSubsystemConfiguration, OptionDefaultByBuildTypeExt,
    };

    #[allow(unused)]
    pub(crate) use assembly_config_schema::{BuildType, FeatureSetLevel};
}

use prelude::*;

pub mod example;

mod battery;
mod bluetooth;
mod build_info;
mod component;
mod connectivity;
mod development;
mod diagnostics;
mod driver_framework;
mod factory_store_providers;
mod fonts;
mod forensics;
mod graphics;
mod hwinfo;
mod icu;
mod intl;
mod kernel;
mod media;
mod memory_monitor;
mod paravirtualization;
mod power;
mod radar;
mod rcs;
mod recovery;
mod sensors;
mod session;
mod setui;
mod starnix;
mod storage;
mod swd;
mod sysmem;
mod system_sounds;
mod tee;
mod thermal;
mod timekeeper;
mod trusted_apps;
mod ui;
mod usb;
mod virtualization;

/// Convert the high-level description of product configuration into a series of configuration
/// value files with concrete package/component tuples.
///
/// Returns a map from package names to configuration updates.
#[allow(clippy::too_many_arguments)]
pub fn define_configuration(
    platform_artifacts: PlatformArtifacts,
    platform: &PlatformSettings,
    product: &ProductSettings,
    board_config: &BoardConfig,
    gendir: impl AsRef<Utf8Path>,
    resource_dir: impl AsRef<Utf8Path>,
    developer_only_options: Option<&DeveloperOnlyOptions>,
    include_example_aib_for_tests: bool,
    mode: &AssemblyMode,
) -> anyhow::Result<CompletedConfiguration> {
    let icu_config = &platform.icu;
    let mut builder = ConfigurationBuilderImpl::new(icu_config.clone());

    // Only perform configuration if the mode is not a test mode.
    if AssemblyMode::should_configure_subsystems(mode) {
        let build_type = &platform.build_type;
        let gendir = gendir.as_ref().to_path_buf();
        let resource_dir = resource_dir.as_ref().to_path_buf();

        // Set up the context that's used by each subsystem to get the generally-
        // available platform information.
        let context = ConfigurationContextBase {
            base_context: ConfigurationContext {
                feature_set_level: &platform.feature_set_level,
                build_type,
                board_config,
                gendir,
                resource_dir,
                developer_only_options,
            },
        };

        // Call the configuration functions for each subsystem.
        configure_subsystems(
            &context,
            platform_artifacts,
            platform,
            product,
            &mut builder,
            include_example_aib_for_tests,
        )?;
    }

    Ok(builder.build())
}

fn configure_subsystems(
    context_base: &ConfigurationContextBase<'_>,
    platform_artifacts: PlatformArtifacts,
    platform: &PlatformSettings,
    product: &ProductSettings,
    builder: &mut dyn ConfigurationBuilder,
    include_example_aib_for_tests: bool,
) -> anyhow::Result<()> {
    builder.add_auto_include_bundles(
        &platform_artifacts,
        &platform.feature_set_level,
        &platform.build_type,
    );

    // Configure the Product Assembly + Structured Config example, if enabled.
    if include_example_aib_for_tests {
        example::ExampleSubsystemConfig::define_configuration(
            &context_base.for_subsystem("example"),
            &platform.example_config,
            builder,
        )?;
    } else if platform.example_config != ExampleConfig::default() {
        bail!(
            "Config options were set for the example subsystem, but the example is not enabled to be configured."
        );
    }

    // The real platform subsystems

    battery::BatterySubsystemConfig::define_configuration(
        &context_base.for_subsystem("battery"),
        &platform.battery,
        builder,
    )
    .context("Configuring the 'battery' subsystem")?;

    bluetooth::BluetoothSubsystemConfig::define_configuration(
        &context_base.for_subsystem("bluetooth"),
        &(&platform.bluetooth, &platform.media),
        builder,
    )
    .context("Configuring the `bluetooth` subsystem")?;

    build_info::BuildInfoSubsystem::define_configuration(
        &context_base.for_subsystem("build_info"),
        &product.build_info,
        builder,
    )
    .context("Configuring the 'build_info' subsystem")?;

    let component_config = component::ComponentConfig {
        policy: &product.component_policy,
        development_support: &platform.development_support,
        starnix: &platform.starnix,
        health_check: &platform.health_check,
        memory_allocator: &platform.memory_allocator,
        graphics: &platform.graphics,
    };
    component::ComponentSubsystem::define_configuration(
        &context_base.for_subsystem("component"),
        &component_config,
        builder,
    )
    .context("Configuring the 'component' subsystem")?;

    connectivity::ConnectivitySubsystemConfig::define_configuration(
        &context_base.for_subsystem("connectivity"),
        &platform.connectivity,
        builder,
    )
    .context("Configuring the 'connectivity' subsystem")?;

    development::DevelopmentConfig::define_configuration(
        &context_base.for_subsystem("development"),
        &platform.development_support,
        builder,
    )
    .context("Configuring the 'development' subsystem")?;

    let diagnostics_config = diagnostics::DiagnosticsSubsystemConfig {
        diagnostics: &platform.diagnostics,
        storage: &platform.storage,
    };
    diagnostics::DiagnosticsSubsystem::define_configuration(
        &context_base.for_subsystem("diagnostics"),
        &diagnostics_config,
        builder,
    )
    .context("Configuring the 'diagnostics' subsystem")?;

    driver_framework::DriverFrameworkSubsystemConfig::define_configuration(
        &context_base.for_subsystem("driver_framework"),
        &(&platform.driver_framework, &platform.storage, &platform.development_support),
        builder,
    )
    .context("Configuring the 'driver_framework' subsystem")?;

    graphics::GraphicsSubsystemConfig::define_configuration(
        &context_base.for_subsystem("graphics"),
        &(&platform.graphics, &platform.ui),
        builder,
    )
    .context("Configuring the 'graphics' subsystem")?;

    hwinfo::HwinfoSubsystem::define_configuration(
        &context_base.for_subsystem("hwinfo"),
        &product.info,
        builder,
    )
    .context("Configuring the 'hwinfo' subsystem")?;

    icu::IcuSubsystem::define_configuration(
        &context_base.for_subsystem("icu"),
        &platform.icu,
        builder,
    )
    .context("Configuring the 'icu' subsystem")?;

    media::MediaSubsystem::define_configuration(
        &context_base.for_subsystem("media"),
        &platform.media,
        builder,
    )
    .context("Configuring the 'media' subsystem")?;

    memory_monitor::MemoryMonitorSubsystem::define_configuration(
        &context_base.for_subsystem("memory_monitor"),
        &platform.memory_monitor,
        builder,
    )
    .context("Configuring the memory monitoring subsystem")?;

    power::PowerManagementSubsystem::define_configuration(
        &context_base.for_subsystem("power"),
        &platform.power,
        builder,
    )
    .context("Configuring the 'power' subsystem")?;

    paravirtualization::ParavirtualizationSubsystem::define_configuration(
        &context_base.for_subsystem("paravirtualization"),
        &platform.paravirtualization,
        builder,
    )
    .context("Configuring the 'paravirtualization' subsystem")?;

    radar::RadarSubsystemConfig::define_configuration(
        &context_base.for_subsystem("radar"),
        &(),
        builder,
    )
    .context("Configuring the 'radar' subsystem")?;

    recovery::RecoverySubsystem::define_configuration(
        &context_base.for_subsystem("recovery"),
        &(&platform.recovery, &platform.storage.filesystems.volume),
        builder,
    )
    .context("Configuring the 'recovery' subsystem")?;

    rcs::RcsSubsystemConfig::define_configuration(&context_base.for_subsystem("rcs"), &(), builder)
        .context("Configuring the 'rcs' subsystem")?;

    sensors::SensorsSubsystemConfig::define_configuration(
        &context_base.for_subsystem("sensors"),
        &platform.starnix,
        builder,
    )
    .context("Configuring the 'sensors' subsystem")?;

    session::SessionConfig::define_configuration(
        &context_base.for_subsystem("session"),
        &(&platform.session, &product.session, &platform.software_delivery),
        builder,
    )
    .context("Configuring the 'session' subsystem")?;

    starnix::StarnixSubsystem::define_configuration(
        &context_base.for_subsystem("starnix"),
        &(&platform.starnix, &platform.connectivity),
        builder,
    )
    .context("Configuring the starnix subsystem")?;

    storage::StorageSubsystemConfig::define_configuration(
        &context_base.for_subsystem("storage"),
        &(&platform.storage, &platform.development_support.tools.storage, &platform.recovery),
        builder,
    )
    .context("Configuring the 'storage' subsystem")?;

    swd::SwdSubsystemConfig::define_configuration(
        &context_base.for_subsystem("swd"),
        &platform.software_delivery,
        builder,
    )
    .context("Configuring the 'software_delivery' subsystem")?;

    sysmem::SysmemConfig::define_configuration(
        &context_base.for_subsystem("sysmem"),
        &platform.sysmem,
        builder,
    )
    .context("Configuring 'sysmem'")?;

    thermal::ThermalSubsystem::define_configuration(
        &context_base.for_subsystem("thermal"),
        &(),
        builder,
    )
    .context("Configuring the 'thermal' subsystem")?;

    ui::UiSubsystem::define_configuration(&context_base.for_subsystem("ui"), &platform.ui, builder)
        .context("Configuring the 'ui' subsystem")?;

    virtualization::VirtualizationSubsystem::define_configuration(
        &context_base.for_subsystem("virtualization"),
        &platform.virtualization,
        builder,
    )
    .context("Configuring the 'virtualization' subsystem")?;

    fonts::FontsSubsystem::define_configuration(
        &context_base.for_subsystem("fonts"),
        &platform.fonts,
        builder,
    )
    .context("Configuring the 'fonts' subsystem")?;

    factory_store_providers::FactoryStoreProvidersSubsystem::define_configuration(
        &context_base.for_subsystem("factory_store_providers"),
        &platform.factory_store_providers,
        builder,
    )
    .context("Configuring the 'factory_store_providers' subsystem")?;

    intl::IntlSubsystem::define_configuration(
        &context_base.for_subsystem("intl"),
        &(&platform.intl, &platform.session),
        builder,
    )
    .context("Confguring the 'intl' subsystem")?;

    setui::SetUiSubsystem::define_configuration(
        &context_base.for_subsystem("setui"),
        &platform.setui,
        builder,
    )
    .context("Confguring the 'SetUI' subsystem")?;

    system_sounds::SystemSoundsSubsystem::define_configuration(
        &context_base.for_subsystem("system_sounds"),
        &platform.system_sounds,
        builder,
    )
    .context("Confguring the 'SystemSounds' subsystem")?;

    kernel::KernelSubsystem::define_configuration(
        &context_base.for_subsystem("kernel"),
        &platform.kernel,
        builder,
    )
    .context("Configuring the 'kernel' subsystem")?;

    forensics::ForensicsSubsystem::define_configuration(
        &context_base.for_subsystem("forensics"),
        &(&platform.forensics, &platform.session),
        builder,
    )
    .context("Configuring the 'Forensics' subsystem")?;

    timekeeper::TimekeeperSubsystem::define_configuration(
        &context_base.for_subsystem("timekeeper"),
        &platform.timekeeper,
        builder,
    )
    .context("Configuring the 'timekeeper' subsystem")?;

    trusted_apps::TrustedAppsSubsystem::define_configuration(
        &context_base.for_subsystem("trusted_apps"),
        &(&product.trusted_apps, platform.storage.filesystems.image_mode),
        builder,
    )
    .context("Configuring the 'trusted_apps' subsystem")?;

    usb::UsbSubsystem::define_configuration(
        &context_base.for_subsystem("usb"),
        &(&platform.usb, &platform.starnix),
        builder,
    )
    .context("Configuring the 'usb' subsystem")?;

    tee::TeeConfig::define_configuration(
        &context_base.for_subsystem("tee"),
        &(&product.tee, &product.tee_clients, &platform.recovery, &platform.session),
        builder,
    )
    .context("configuring the 'tee' subsystem")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_config_schema::ProductConfig;
    use assembly_util as util;

    #[test]
    fn test_example_config_without_configure_example_returns_err() {
        let json5 = r#"
            {
            platform: {
                build_type: "eng",
                example_config: {
                    include_example_aib: true
                }
            },
            product: {},
            }
        "#;

        let mut cursor = std::io::Cursor::new(json5);
        let ProductConfig { platform, product, .. } = util::from_reader(&mut cursor).unwrap();
        let result = define_configuration(
            PlatformArtifacts::empty_for_test(),
            &platform,
            &product,
            &BoardConfig::default(),
            "",
            "",
            None,
            true,
            &AssemblyMode::default(),
        );

        assert!(result.is_err());
    }
}
