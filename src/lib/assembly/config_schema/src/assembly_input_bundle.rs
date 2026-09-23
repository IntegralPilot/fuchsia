// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::{PackageDetails, PackageName, PackagedDriverDetails};
use crate::{BuildType, FeatureSetLevel};
use assembly_constants::{CompiledPackageDestination, FileEntry};
use assembly_container::{AssemblyContainer, WalkPaths, assembly_container};
use assembly_package_utils::PackageInternalPathBuf;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A bundle of inputs to be used in the assembly of a product.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, WalkPaths)]
#[assembly_container(assembly_config.json)]
#[serde(default, deny_unknown_fields)]
pub struct AssemblyInputBundle {
    /// The feature set level and build type combinations that this AIB is allowed
    /// to be included in.
    pub allowed_in: BTreeMap<FeatureSetLevel, Vec<BuildType>>,

    /// The feature set level and build type combinations that we should expect
    /// to find the contents of this AIB. This helps us determine which scrutiny
    /// entries should be 'required' (no ? prefix).
    pub scrutiny_required: BTreeMap<FeatureSetLevel, Vec<BuildType>>,

    /// The feature set level and build type combinations that should
    /// automatically include this AIB.
    pub auto_include_in: BTreeMap<FeatureSetLevel, Vec<BuildType>>,

    /// Whether this AIB is part of a feature that is actively being developed,
    /// and we want to delay a security review. Running scrutiny on products
    /// that use experimental AIBs will fail.
    pub experimental: bool,

    /// The parameters that specify which kernel to put into the ZBI.
    pub kernel: Option<PartialKernelConfig>,

    /// The qemu kernel to use when starting the emulator.
    pub qemu_kernel: Option<Utf8PathBuf>,

    /// The list of additional boot args to add.
    pub boot_args: Vec<String>,

    /// The set of files to be placed in BOOTFS in the ZBI.
    pub bootfs_files: Vec<FileEntry<String>>,

    /// Package entries that internally specify their package set, instead of being grouped
    /// separately.
    pub packages: Vec<PackageDetails>,

    /// Entries for the `config_data` package.
    pub config_data: BTreeMap<String, Vec<FileEntry<String>>>,

    /// The blobs index of the AIB.  This currently isn't used by product
    /// assembly, as the package manifests contain the same information.
    pub blobs: Vec<Utf8PathBuf>,

    /// Configuration of driver packages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drivers: Vec<PackagedDriverDetails>,

    /// Map of the names of packages that contain shell commands to the list of
    /// commands within each.
    pub bootfs_shell_commands: ShellCommands,

    /// Map of the names of packages that contain shell commands to the list of
    /// commands within each.
    pub shell_commands: ShellCommands,

    /// Packages to create dynamically as part of the Assembly process.
    #[walk_paths]
    pub packages_to_compile: Vec<CompiledPackageDefinition>,

    /// A package that includes files to include in bootfs.
    pub bootfs_files_package: Option<Utf8PathBuf>,

    /// A list of memory buckets to pass to memory monitor.
    #[walk_paths]
    pub memory_buckets: Vec<Utf8PathBuf>,
}

impl AssemblyInputBundle {
    /// Returns whether this AIB is allowed to be included in the specified
    /// feature set level and build type.
    ///
    /// If `allowed_in` is empty, then it is allowed in all build types and
    /// feature set levels.
    pub fn is_allowed_in(
        &self,
        feature_set_level: &FeatureSetLevel,
        build_type: &BuildType,
    ) -> bool {
        if !self.auto_include_in.is_empty() {
            return self.should_be_auto_included_in(feature_set_level, build_type);
        }

        // Default to allowed if no restrictions are present.
        if self.allowed_in.is_empty() {
            return true;
        }

        // If specific allowances are set, check them.
        if let Some(build_types) = self.allowed_in.get(feature_set_level) {
            return build_types.contains(build_type);
        }
        false
    }

    /// Returns whether the contents of the AIB should be expected in the
    /// specified feature set level and build type.
    pub fn required_to_be_in(
        &self,
        feature_set_level: &FeatureSetLevel,
        build_type: &BuildType,
    ) -> bool {
        if !self.auto_include_in.is_empty() {
            return self.should_be_auto_included_in(feature_set_level, build_type);
        }

        // If specific requirements are set, check them.
        if let Some(build_types) = self.scrutiny_required.get(feature_set_level) {
            return build_types.contains(build_type);
        }

        // Default to not required if no restrictions are present.
        false
    }

    /// Returns whether this AIB should be automatically included in the
    /// specified feature set level and build type.
    pub fn should_be_auto_included_in(
        &self,
        feature_set_level: &FeatureSetLevel,
        build_type: &BuildType,
    ) -> bool {
        if let Some(build_types) = self.auto_include_in.get(feature_set_level) {
            return build_types.contains(build_type);
        }
        false
    }
}

/// The information required to specify a kernel and its arguments, all optional
/// to allow for the partial specification
#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialKernelConfig {
    /// The path to the prebuilt kernel.
    pub path: Option<Utf8PathBuf>,

    /// The list of command line arguments to pass to the kernel on startup.
    #[serde(default)]
    pub args: Vec<String>,
}

/// A typename to represent a package that contains shell command binaries,
/// and the paths to those binaries
pub type ShellCommands = BTreeMap<PackageName, BTreeSet<PackageInternalPathBuf>>;

/// Contents of a compiled package. The contents provided by all
/// selected AIBs are merged by `name` into a single package
/// at assembly time.
#[derive(Debug, Deserialize, Serialize, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct CompiledPackageDefinition {
    /// Name of the package to compile.
    pub name: CompiledPackageDestination,

    /// Components to compile and add to the package.
    #[walk_paths]
    #[serde(default)]
    pub components: Vec<CompiledComponentDefinition>,

    /// Non-component files to add to the package.
    #[serde(default)]
    pub contents: Vec<FileEntry<String>>,

    /// CML files included by the component cml.
    #[serde(default)]
    pub includes: Vec<Utf8PathBuf>,

    /// Whether the contents of this package should go into bootfs.
    /// Gated by allowlist -- please use this as a base package if possible.
    #[serde(default)]
    pub bootfs_package: bool,
}

/// Contents of a compiled component. The contents provided by all
/// selected AIBs are merged by `name` into a single package
/// at assembly time.
#[derive(Debug, Deserialize, Serialize, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct CompiledComponentDefinition {
    /// The name of the component to compile.
    pub component_name: String,

    /// CML file shards to include in the compiled component manifest.

    #[walk_paths]
    pub shards: Vec<Utf8PathBuf>,

    /// List of CMC features to use during component compilation.
    pub cmc_features: Vec<String>,
}
