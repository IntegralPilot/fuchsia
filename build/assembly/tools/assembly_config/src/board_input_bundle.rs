// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::BoardInputBundleArgs;

use anyhow::{Context, Result};
use assembly_config_schema::{
    BoardInputBundle, BoardProvidedConfig, PackageDetails, PackageSet, PackagedDriverDetails,
};
use assembly_container::AssemblyContainer;
use assembly_release_info::ReleaseInfo;
use assembly_util::{get_release_repository, get_release_version};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

pub fn new(args: &BoardInputBundleArgs) -> Result<()> {
    let BoardInputBundleArgs {
        name,
        include_in,
        output,
        drivers,
        base_packages,
        bootfs_packages,
        bootfs_or_base_packages,
        energy_model_config,
        kernel_boot_args,
        battery_manager_config,
        power_manager_config,
        system_power_mode_config,
        cpu_manager_config,
        thermal_config,
        thread_roles,
        triage_detect_configs,
        power_metrics_recorder_config,
        sysmem_format_costs_config,
        version,
        version_file,
        repo,
        repo_file,
        depfile,
    } = args.to_owned();

    // Collect the drivers.
    let mut collected_drivers = vec![];
    if let Some(drivers_path) = drivers {
        let drivers_config: DriversInformationHelper =
            assembly_util::read_config(&drivers_path).context("Loading driver information")?;
        for driver in drivers_config.drivers {
            let DriverInformation { package, set, components } = driver;
            collected_drivers.push(PackagedDriverDetails {
                package: package.into(),
                set,
                components,
            })
        }
    }

    // Collect the packages.
    let mut packages = vec![];
    for (pkg_set, pkgs) in [
        (PackageSet::Base, base_packages),
        (PackageSet::Bootfs, bootfs_packages),
        (PackageSet::BootfsOrBase, bootfs_or_base_packages),
    ] {
        for package_manifest_path in pkgs {
            packages.push(PackageDetails {
                package: package_manifest_path.into(),
                set: pkg_set.clone(),
            })
        }
    }

    // Create the configuration.
    let configuration = if battery_manager_config.is_some()
        || cpu_manager_config.is_some()
        || energy_model_config.is_some()
        || power_manager_config.is_some()
        || power_metrics_recorder_config.is_some()
        || system_power_mode_config.is_some()
        || thermal_config.is_some()
        || !thread_roles.is_empty()
        || !triage_detect_configs.is_empty()
        || !sysmem_format_costs_config.is_empty()
    {
        Some(BoardProvidedConfig {
            battery_manager: battery_manager_config.as_ref().map(|f| f.into()),
            cpu_manager: cpu_manager_config.as_ref().map(|f| f.into()),
            energy_model: energy_model_config.as_ref().map(|f| f.into()),
            power_manager: power_manager_config.as_ref().map(|f| f.into()),
            power_metrics_recorder: power_metrics_recorder_config.as_ref().map(|f| f.into()),
            system_power_mode: system_power_mode_config.as_ref().map(|f| f.into()),
            thermal: thermal_config.as_ref().map(|f| f.into()),
            thread_roles: thread_roles.iter().map(|f| f.into()).collect(),
            triage_detect_configs: triage_detect_configs.iter().map(|f| f.into()).collect(),
            sysmem_format_costs: sysmem_format_costs_config.iter().map(|f| f.into()).collect(),
        })
    } else {
        None
    };

    // Create the BoardInputBundle.
    let bundle = BoardInputBundle {
        name: name.to_string(),
        include_in: include_in.clone(),
        drivers: collected_drivers,
        packages,
        kernel_boot_args: kernel_boot_args.clone().into_iter().collect(),
        configuration,
        release_info: ReleaseInfo {
            name: name.to_string(),
            repository: get_release_repository(repo, repo_file)?,
            version: get_release_version(version, version_file)?,
        },
    };
    bundle.write_to_dir(output, depfile.as_ref())?;

    Ok(())
}

/// Helper struct for deserializing the driver information file that's passed to
/// the creation of a board input bundle.  This is its own type so that it
/// can be exactly matched to the CLI arguments, and separately versioned from
/// internal types used by assembly.
// LINT.IfChange
#[derive(Default, Debug, Serialize, Deserialize)]
struct DriversInformationHelper {
    drivers: Vec<DriverInformation>,
}

/// Each packaged driver's information
#[derive(Debug, Serialize, Deserialize)]
struct DriverInformation {
    /// The path (relative to the current working dir) of the package manifest
    package: Utf8PathBuf,

    /// Which set this package belongs to.
    set: PackageSet,

    /// The driver components within the package, e.g. meta/foo.cm.
    components: Vec<Utf8PathBuf>,
}
// LINT.ThenChange(//build/assembly/board_input_bundle.gni)
