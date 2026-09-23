// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::{PackageDetails, PackagedDriverDetails};
use anyhow::{Result, anyhow};
use camino::Utf8PathBuf;
use std::str::FromStr;

use assembly_container::{AssemblyContainer, WalkPaths, assembly_container};
use assembly_release_info::ReleaseInfo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// This struct defines a bundle of artifacts that can be included by the board
/// in the assembled image.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, WalkPaths)]
#[assembly_container(board_input_bundle.json)]
#[serde(default, deny_unknown_fields)]
pub struct BoardInputBundle {
    /// The name of the board input bundle.
    pub name: String,

    /// Which builds types to include this BIB.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_in: IncludeInBuildType,

    /// These are the drivers that are included by this bundle.
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drivers: Vec<PackagedDriverDetails>,

    /// These are the packages to include with this bundle.
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageDetails>,

    /// These are kernel boot arguments that are to be passed to the kernel when
    /// this bundle is included in the assembled system.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub kernel_boot_args: BTreeSet<String>,

    /// Board-provided configuration for platform services.  Each field of this
    /// structure can only be provided by one of the BoardInputBundles that a
    /// BoardConfig uses.
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<BoardProvidedConfig>,

    /// Release information about this assembly container artifact.
    pub release_info: ReleaseInfo,
}

/// This struct defines board-provided configuration for platform services and
/// features, used if those services are included by the product's supplied
/// platform configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct BoardProvidedConfig {
    /// Configuration for the cpu-manager service
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_manager: Option<Utf8PathBuf>,

    /// Energy model configuration for processor power management
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy_model: Option<Utf8PathBuf>,

    /// Configuration for the battery-manager service
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_manager: Option<Utf8PathBuf>,

    /// Configuration for the power-manager service
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_manager: Option<Utf8PathBuf>,

    /// Configuration for the power metrics recorder service
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_metrics_recorder: Option<Utf8PathBuf>,

    /// System power modes configuration
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_power_mode: Option<Utf8PathBuf>,

    /// Thermal configuration for the power-manager service
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thermal: Option<Utf8PathBuf>,

    /// These files describe performance "roles" that threads can take.  These roles translate to
    /// Zircon profiles that change the runtime properties of the thread
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub thread_roles: Vec<Utf8PathBuf>,

    /// Sysmem format costs configuration for the board. The file content bytes
    /// are a persistent fidl fuchsia.sysmem2.FormatCosts. Normally json[5]
    /// would be preferable for config, but we generate this config in rust
    /// using FIDL types (to avoid repetition and to take advantage of FIDL rust
    /// codegen), and there's no json schema for FIDL types.
    ///
    /// See BoardConfig.platform.sysmem_defaults for other board-level
    /// sysmem config.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sysmem_format_costs: Vec<Utf8PathBuf>,

    /// Triage-detect configs for board-specific drivers.
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub triage_detect_configs: Vec<Utf8PathBuf>,
}

/// Which build type to include a particular BIB.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IncludeInBuildType {
    /// Include in all build types.
    #[default]
    All,

    /// Only include in eng build types.
    Eng,

    /// Only include in user and userdebug build types.
    UserAndUserdebug,
}

impl FromStr for IncludeInBuildType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "all" => Ok(Self::All),
            "eng" => Ok(Self::Eng),
            "user_and_userdebug" => Ok(Self::UserAndUserdebug),
            _ => Err(anyhow!("Cannot parse --include-in from string: {}", s)),
        }
    }
}
