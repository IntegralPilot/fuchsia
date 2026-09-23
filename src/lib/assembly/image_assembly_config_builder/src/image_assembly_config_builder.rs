// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::compiled_package::CompiledPackageBuilder;

use anyhow::{Context, Result, anyhow, bail};
use assembly_cli_args::{AssemblyMode, ValidationMode};
use assembly_config_data::ConfigDataBuilder;
use assembly_config_schema::assembly_input_bundle::{
    AssemblyInputBundle, CompiledPackageDefinition, ShellCommands,
};
use assembly_config_schema::board_config::HardwareInfo;
use assembly_config_schema::board_input_bundle::BoardInputBundle;
use assembly_config_schema::common::PackagedDriverDetails;
use assembly_config_schema::developer_overrides::{DeveloperOnlyOptions, DeveloperOverrides};
use assembly_config_schema::platform_settings::BuildType;
use assembly_config_schema::product_settings::{
    ProductConfigData, ProductPackageDetails, ProductPackagesConfig,
};
use assembly_config_schema::{
    BoardConfig, DriverDetails, FeatureSetLevel, PackageDetails, PackageSet,
};
use assembly_constants::{
    BootfsDestination, BootfsPackageDestination, FileEntry, PackageDestination,
    PackageSetDestination,
};
use assembly_container::AssemblyContainer;
use assembly_domain_config::DomainConfigPackage;
use assembly_driver_manifest::{DriverManifestBuilder, DriverPackageType};
use assembly_images_config::{FilesystemImageMode, ImagesConfig};
use assembly_memory_buckets::MemoryBuckets;
use assembly_named_file_map::NamedFileMap;
use assembly_package_utils::PackageInternalPathBuf;
use assembly_platform_configuration::{
    DomainConfig, DomainConfigs, PackageConfigs, PackageConfiguration,
};
use assembly_release_info::SystemReleaseInfo;
use assembly_shell_commands::ShellCommandsBuilder;
use assembly_structured_config::Repackager;
use assembly_tool::ToolProvider;
use assembly_util as util;
use assembly_util::{DuplicateKeyError, InsertAllUniqueExt, InsertUniqueExt, NamedMap};
use assembly_validate_package::{PackageValidationError, validate_component, validate_package};
use assembly_validate_util::{BootfsContents, PkgNamespace};
use camino::{Utf8Path, Utf8PathBuf};
use fidl::unpersist;
use fidl_fuchsia_component_decl::Component;
use fuchsia_archive::Utf8Reader;
use fuchsia_pkg::PackageManifest;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use image_assembly_config::{BoardDriverArguments, ImageAssemblyConfig, KernelConfig};
use itertools::Itertools;
use product_input_bundle::ProductInputBundle;
use rayon::iter::{ParallelBridge, ParallelIterator};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use util::MapEntry;

#[derive(Debug, Serialize)]
pub struct ImageAssemblyConfigBuilder {
    /// The RFC-0115 Build Type of the assembled product + platform.
    build_type: BuildType,

    /// The feature set to include in this product by default.
    feature_set_level: FeatureSetLevel,

    /// The name of the board that these images can be OTA'd to.
    board_name: String,

    /// The partitions that this assembly can be flashed to.
    partitions_config: Option<Utf8PathBuf>,

    /// How to generate the filesystem image.
    image_mode: FilesystemImageMode,

    /// The mode to run assembly in.
    mode: AssemblyMode,

    /// All of the packages, in all package sets.
    packages: Packages,

    /// The base driver packages from the AssemblyInputBundles
    base_drivers: NamedMap<String, DriverDetails>,

    /// The boot driver packages from the AssemblyInputBundles
    boot_drivers: NamedMap<String, DriverDetails>,

    /// The boot_args from the AssemblyInputBundles
    boot_args: BTreeSet<String>,

    /// The bootfs_files from the AssemblyInputBundles
    bootfs_files: NamedFileMap<BootfsDestination>,

    /// Modifications that must be made to configuration for packages.
    package_configs: PackageConfigs,

    /// Domain config packages to create.
    domain_configs: DomainConfigs,

    kernel_path: Option<Utf8PathBuf>,
    kernel_args: BTreeMap<String, KernelArg>,
    kernel_arg_overrides: BTreeMap<String, KernelArg>,

    qemu_kernel: Option<Utf8PathBuf>,
    bootfs_shell_commands: ShellCommands,
    shell_commands: ShellCommands,

    /// The packages for assembly to create specified by AIBs
    packages_to_compile: BTreeMap<String, CompiledPackageBuilder>,

    /// The memory buckets to merge and make available to memory monitor.
    memory_buckets: MemoryBuckets,

    /// Data passed to the board's Board Driver, if provided.
    board_driver_arguments: Option<BoardDriverArguments>,

    /// Configuration capabilities to add to a configuration component/package.
    configuration_capabilities: Option<assembly_config_capabilities::CapabilityNamedMap>,

    /// ZBI container with extra items to be added to zbi
    zbi_extra_items: Option<Utf8PathBuf>,

    /// Devicetree binary to be added to zbi
    devicetree: Option<Utf8PathBuf>,

    /// Devicetree binary overlay to be added to the update package and flashed to a specific
    /// partition.
    devicetree_overlay: Option<Utf8PathBuf>,

    /// Developer override options
    developer_only_options: Option<DeveloperOnlyOptions>,

    /// Optional ImagesConfig to add to the constructed ImageAssembly config
    images_config: Option<ImagesConfig>,

    /// Optional version information for all input artifacts.
    /// TODO(https://fxbug.dev/416239346): Make this a required field.
    system_release_info: SystemReleaseInfo,
}

impl ImageAssemblyConfigBuilder {
    pub fn new(
        build_type: BuildType,
        feature_set_level: FeatureSetLevel,
        board_name: String,
        partitions_config: Option<Utf8PathBuf>,
        image_mode: FilesystemImageMode,
        mode: AssemblyMode,
        system_release_info: SystemReleaseInfo,
    ) -> Self {
        Self {
            build_type,
            feature_set_level,
            board_name,
            partitions_config,
            image_mode,
            mode,
            packages: Packages::default(),
            base_drivers: NamedMap::new("base_drivers"),
            boot_drivers: NamedMap::new("boot_drivers"),
            boot_args: BTreeSet::default(),
            bootfs_shell_commands: ShellCommands::default(),
            shell_commands: ShellCommands::default(),
            bootfs_files: NamedFileMap::new("bootfs files"),
            package_configs: PackageConfigs::new("package configs"),
            domain_configs: DomainConfigs::new("domain configs"),
            kernel_path: None,
            kernel_args: BTreeMap::default(),
            kernel_arg_overrides: BTreeMap::default(),
            qemu_kernel: None,
            packages_to_compile: BTreeMap::default(),
            memory_buckets: MemoryBuckets::default(),
            board_driver_arguments: None,
            configuration_capabilities: None,
            zbi_extra_items: None,
            devicetree: None,
            devicetree_overlay: None,
            developer_only_options: None,
            images_config: None,
            system_release_info,
        }
    }

    /// Add developer overrides to the builder
    pub fn add_developer_overrides(
        &mut self,
        developer_overrides: DeveloperOverrides,
    ) -> Result<()> {
        let DeveloperOverrides {
            target_name: _,
            developer_only_options,
            kernel,
            platform: _,
            product: _,
            board: _,
            packages,
            drivers,
            packages_to_compile,
            shell_commands,
            developer_provided_files: _,
            bootfs_files_package,
        } = developer_overrides;

        // Set the developer-only options for the buidler to use.
        self.developer_only_options = Some(developer_only_options);

        // Add the kernel command line args from the developer into the separate "overrides" map, as
        // these will be applied after all other command-line args have been added.
        for arg in kernel.command_line_args {
            let kernel_arg = KernelArg::from(&arg);
            self.kernel_arg_overrides.insert(kernel_arg.key.clone(), kernel_arg);
        }

        // Add packages specified by the developer
        for package_details in packages {
            self.add_package_from_path(
                package_details.package,
                PackageOrigin::Developer,
                &package_details.set,
            )
            .context("Adding developer-specified package")?;
        }

        // Add drivers specified by the developer
        for driver_details in drivers {
            self.add_driver_package(
                DriverDetails {
                    package: driver_details.package,
                    components: driver_details.components,
                },
                driver_details.set,
                PackageOrigin::Developer,
                None,
            )?;
        }

        for compiled_package_def in packages_to_compile {
            self.add_compiled_package(&compiled_package_def, "".into()).with_context(|| {
                format!(
                    "Adding developer-specified compiled package: {}",
                    compiled_package_def.name
                )
            })?;
        }

        for (package, binaries) in shell_commands {
            for binary in binaries {
                self.add_pkg_shell_command_entry(&package, binary)?;
            }
        }

        if let Some(path) = bootfs_files_package {
            self.add_bootfs_files_package(path, true)?;
        }

        Ok(())
    }

    /// Add an Assembly Input Bundle to the builder, via the path to its
    /// manifest.
    ///
    /// If any of the items it's trying to add are duplicates (either of itself
    /// or others, this will return an error.)
    pub fn add_bundle(&mut self, bundle_path: impl AsRef<Utf8Path>) -> Result<()> {
        // Strip filename from bundle path if it's the config file.
        let mut bundle_dir = bundle_path.as_ref();
        if bundle_dir.file_name() == Some(AssemblyInputBundle::get_config_filename()) {
            bundle_dir = bundle_dir.parent().unwrap();
        }

        let bundle = AssemblyInputBundle::from_dir(bundle_dir)?;

        let bundle_path = Utf8PathBuf::from(bundle_dir);

        // Now add the parsed bundle
        self.add_parsed_bundle(bundle_path, bundle)
    }

    /// Add an Assembly Input Bundle to the builder, using a parsed
    /// AssemblyInputBundle, and the path to the folder that contains it.
    ///
    /// If any of the items it's trying to add are duplicates (either of itself
    /// or others, this will return an error.)
    pub fn add_parsed_bundle(
        &mut self,
        bundle_path: impl AsRef<Utf8Path>,
        bundle: AssemblyInputBundle,
    ) -> Result<()> {
        let bundle_path = bundle_path.as_ref();
        // Assert that this AIB is supported on this feature set level and build type.
        if !bundle.is_allowed_in(&self.feature_set_level, &self.build_type) {
            bail!(
                "Product is using Fuchsia '{:?}' feature set level and '{:?}' build type, but attempted to include a feature that is only allowed on: {:?}",
                self.feature_set_level,
                self.build_type,
                bundle.allowed_in
            );
        }

        let AssemblyInputBundle {
            allowed_in: _,
            scrutiny_required: _,
            auto_include_in: _,
            experimental: _,
            kernel,
            qemu_kernel,
            boot_args,
            bootfs_files: _,
            packages,
            config_data,
            blobs: _,
            drivers,
            bootfs_shell_commands,
            shell_commands,
            packages_to_compile,
            bootfs_files_package,
            memory_buckets,
        } = bundle;

        self.add_bundle_packages(bundle_path, &packages)?;

        if let Some(path) = bootfs_files_package {
            let path = bundle_path.join(path);
            self.add_bootfs_files_package(path, false).context("Adding bootfs files package")?;
        }

        // Drivers are added to the correct driver map based on their set
        for driver_details in drivers {
            self.add_driver_package(
                DriverDetails {
                    package: driver_details.package,
                    components: driver_details.components,
                },
                driver_details.set,
                PackageOrigin::AIB,
                Some(bundle_path),
            )?;
        }

        self.boot_args
            .try_insert_all_unique(boot_args)
            .map_err(|arg| anyhow!("duplicate boot_arg found: {}", arg))?;

        if let Some(kernel) = kernel {
            assembly_util::set_option_once_or(
                &mut self.kernel_path,
                kernel.path.map(|p| bundle_path.join(p)),
                anyhow!("Only one input bundle can specify a kernel path"),
            )?;

            self.add_kernel_args(kernel.args)?;
        }

        for (package, entries) in config_data {
            for entry in Self::file_entry_paths_from_bundle(bundle_path, entries) {
                self.add_config_data_entry(&package, entry)?;
            }
        }

        for (package, binaries) in bootfs_shell_commands {
            for binary in binaries {
                self.add_bootfs_shell_command_entry(&package, binary)?;
            }
        }
        for (package, binaries) in shell_commands {
            for binary in binaries {
                self.add_pkg_shell_command_entry(&package, binary)?;
            }
        }

        for compiled_package in packages_to_compile {
            self.add_compiled_package(&compiled_package, bundle_path)?;
        }

        self.add_memory_buckets(&memory_buckets)?;

        assembly_util::set_option_once_or(
            &mut self.qemu_kernel,
            qemu_kernel.map(|p| bundle_path.join(p)),
            anyhow!("Only one input bundle can specify a qemu kernel path"),
        )?;

        Ok(())
    }

    /// Add a Board input Bundle to the builder, using the path to the
    /// folder that contains it.
    ///
    /// If any of the items it's trying to add are duplicates (either of itself
    /// or others, this will return an error).
    pub fn add_board_input_bundle(
        &mut self,
        bundle: BoardInputBundle,
        bootstrap_only: bool,
    ) -> Result<()> {
        for PackagedDriverDetails { package, set, components } in bundle.drivers {
            let set = match (bootstrap_only, set) {
                (true, PackageSet::BootfsOrBase) => PackageSet::Bootfs,
                (false, PackageSet::BootfsOrBase) => PackageSet::Base,
                (true, PackageSet::Base) => continue,
                (_, set) => set,
            };
            self.add_driver_package(
                DriverDetails { package, components },
                set,
                PackageOrigin::Board,
                None,
            )?;
        }

        for PackageDetails { package, set } in bundle.packages {
            let set = match (bootstrap_only, set) {
                (true, PackageSet::BootfsOrBase) => PackageSet::Bootfs,
                (false, PackageSet::BootfsOrBase) => PackageSet::Base,
                (_, set) => set,
            };

            // Always add the package if bootfs, and only add non-bootfs packages
            // if this is not a bootstrap_only build.
            if set == PackageSet::Bootfs || !bootstrap_only {
                self.add_package_from_path(package, PackageOrigin::Board, &set)?;
            }
        }

        self.add_kernel_args(bundle.kernel_boot_args)?;

        Ok(())
    }

    /// Add a Product input Bundle to the builder, using the path to the
    /// folder that contains it.
    ///
    /// If any of the items it's trying to add are duplicates (either of itself
    /// or others, this will return an error).
    pub fn add_product_input_bundle(&mut self, bundle: &ProductInputBundle) -> Result<()> {
        for package in bundle.packages.base.values() {
            self.add_package_from_path(
                &package.manifest,
                PackageOrigin::Product,
                &PackageSet::Base,
            )?;
        }
        for package in bundle.packages.cache.values() {
            self.add_package_from_path(
                &package.manifest,
                PackageOrigin::Product,
                &PackageSet::Cache,
            )?;
        }
        for package in bundle.packages.flexible.values() {
            self.add_package_from_path(
                &package.manifest,
                PackageOrigin::Product,
                &PackageSet::Flexible,
            )?;
        }
        Ok(())
    }

    /// Set the (optional) arguments for the Board Driver.
    pub fn set_board_driver_arguments(&mut self, board_config: &BoardConfig) -> Result<()> {
        if self.board_driver_arguments.is_some() {
            bail!("Board driver arguments have already been set");
        }
        self.board_driver_arguments = match &board_config.hardware_info {
            HardwareInfo {
                name,
                vendor_id: Some(vendor_id),
                product_id: Some(product_id),
                revision: Some(revision),
            } => Some(BoardDriverArguments {
                vendor_id: *vendor_id,
                product_id: *product_id,
                revision: *revision,
                name: name.as_ref().unwrap_or(&board_config.name).clone(),
            }),
            HardwareInfo { name: _, vendor_id: None, product_id: None, revision: None } => None,
            _ => {
                bail!(
                    "If any of 'vendor_id', 'product_id', or 'revision' are set, all must be provided: {:?}",
                    board_config.hardware_info
                );
            }
        };
        Ok(())
    }

    /// Add all the bootfs file entries to the builder.
    pub fn add_bootfs_files(&mut self, files: &NamedFileMap<BootfsDestination>) -> Result<()> {
        for entry in files.clone().into_file_entries() {
            self.bootfs_files.add_entry(entry)?;
        }
        Ok(())
    }

    /// Add kernel args to the builder
    pub fn add_kernel_args(&mut self, args: impl IntoIterator<Item = String>) -> Result<()> {
        self.kernel_args
            .try_insert_all_unique(args.into_iter().map(|arg: String| {
                let kernel_arg = KernelArg::from(&arg);
                (kernel_arg.key.clone(), kernel_arg).into()
            }))
            .map_err(|error| {
                let previous = error.previous_value().to_string();
                let new = error.new_value().to_string();
                if previous == new {
                    anyhow!("duplicate kernel command line arg found: {}", previous)
                } else {
                    anyhow!("duplicate kernel command line arg found: {} was {}", new, previous)
                }
            })?;
        Ok(())
    }

    /// Add the blobs of a package as bootfs files
    pub fn add_bootfs_files_package(
        &mut self,
        path: impl AsRef<Utf8Path>,
        from_product: bool,
    ) -> Result<()> {
        let blobs = assembly_package_utils::bootfs_files_from_package(&path)?;
        for blob in blobs {
            let destination = if from_product {
                BootfsDestination::FromProduct(blob.path.clone())
            } else {
                BootfsDestination::FromAIB(blob.path.clone())
            };
            self.bootfs_files
                .add_blob(destination, blob)
                .with_context(|| format!("adding bootfs file from {}", path.as_ref()))?;
        }
        Ok(())
    }

    pub fn add_package_from_path(
        &mut self,
        path: impl AsRef<Utf8Path>,
        origin: PackageOrigin,
        to_package_set: &PackageSet,
    ) -> Result<PackageSet> {
        let is_platform = matches!(origin, PackageOrigin::AIB);
        let mapped_package_set = self.map_package_set(to_package_set, is_platform)?;

        // Create PackageEntry
        let (d, package_entry) =
            PackageEntry::parse_from(origin, mapped_package_set.clone(), &path)?;

        // Now store the package and its destination.
        self.packages
            .try_insert_unique(d, package_entry)
            .with_context(|| format!("Adding packages to {to_package_set}"))?;

        Ok(mapped_package_set)
    }

    /// Remap the package sets based on the build type (for the flexible set)
    /// and the developer_only_overrides (for the 'all_packages_in_base' flag)
    fn map_package_set(&self, set: &PackageSet, is_platform: bool) -> Result<PackageSet> {
        // Ensure that the developer didn't set `all_packages_in_base` for
        // configurations that do not support it.
        if let Some(DeveloperOnlyOptions { all_packages_in_base: true, .. }) =
            self.developer_only_options
        {
            if self.image_mode == FilesystemImageMode::NoImage {
                bail!(
                    "all_packages_in_base cannot be enabled for products without a filesystem (image_mode = no_image)"
                );
            }
            if self.build_type == BuildType::User {
                bail!("all_packages_in_base cannot be enabled for user products");
            }
        }

        Ok(
            match (
                set,
                &self.build_type,
                &self.developer_only_options,
                &self.image_mode,
                is_platform,
            ) {
                // BootFS packages are always in BootFS.
                (&PackageSet::Bootfs, _, _, _, _) => PackageSet::Bootfs,
                // System packages are always system packages
                (&PackageSet::System, _, _, _, _) => PackageSet::System,

                (&PackageSet::BootfsOrBase, _, _, FilesystemImageMode::NoImage, _) => {
                    PackageSet::Bootfs
                }
                (
                    &PackageSet::BootfsOrBase,
                    _,
                    _,
                    FilesystemImageMode::Partition | FilesystemImageMode::Ramdisk,
                    _,
                ) => PackageSet::Base,

                // When the all_packages_in_base developer override option is
                // enabled, that takes precedence over all the rest on eng and userdebug
                // build-types.
                (_, _, Some(DeveloperOnlyOptions { all_packages_in_base: true, .. }), _, _) => {
                    PackageSet::Base
                }

                // The Flexible package set is in Cache for eng builds, and base
                // for user/userdebug.
                (&PackageSet::Flexible, BuildType::Eng, _, _, _) => PackageSet::Cache,
                (&PackageSet::Flexible, _, _, _, _) => PackageSet::Base,

                // For product packages with no filesystem, we move the packages to bootfs.
                (_, _, _, &FilesystemImageMode::NoImage, /*is_platform=*/ false) => {
                    PackageSet::Bootfs
                }

                // In all other cases, packages are just in their original
                // package set.
                (ps, _, _, _, _) => ps.clone(),
            },
        )
    }

    /// Add a set of packages from a bundle, resolving each path to a package
    /// manifest from the bundle's path to locate it.
    fn add_bundle_packages(
        &mut self,
        bundle_path: impl AsRef<Utf8Path>,
        packages: &Vec<PackageDetails>,
    ) -> Result<()> {
        for entry in packages {
            let manifest_path: Utf8PathBuf = bundle_path.as_ref().join(&entry.package);
            self.add_package_from_path(manifest_path, PackageOrigin::AIB, &entry.set)?;
        }

        Ok(())
    }

    fn file_entry_paths_from_bundle(
        base: &Utf8Path,
        entries: impl IntoIterator<Item = FileEntry<String>>,
    ) -> Vec<FileEntry<String>> {
        entries
            .into_iter()
            .map(|entry| FileEntry {
                destination: entry.destination,
                source: base.join(entry.source),
            })
            .collect()
    }

    /// Add all the product-provided packages to the assembly configuration.
    ///
    /// This should be performed after the platform's bundles have been added,
    /// so that any packages that are in conflict with the platform bundles are
    /// flagged as being the issue (and not the platform being the issue).
    pub fn add_product_packages(&mut self, packages: ProductPackagesConfig) -> Result<()> {
        self.add_product_packages_to_set(packages.base, PackageSet::Base)?;
        self.add_product_packages_to_set(packages.cache, PackageSet::Cache)?;
        self.add_product_packages_to_set(packages.bootfs, PackageSet::Bootfs)?;
        Ok(())
    }

    /// Add a vector of product packages to a specific package set.
    fn add_product_packages_to_set(
        &mut self,
        entries: BTreeMap<String, ProductPackageDetails>,
        to_package_set: PackageSet,
    ) -> Result<()> {
        for entry in entries.into_values() {
            // Load the PackageManifest from the given path, in order to get the
            // package name.
            let manifest = PackageManifest::try_load_from(&entry.manifest)
                .with_context(|| format!("parsing {} as a package manifest", entry.manifest))?;

            // Add the package to the set of packages in the assembly.
            let mapped_to_package_set = self.add_package_from_path(
                entry.manifest,
                PackageOrigin::Product,
                &to_package_set,
            )?;

            // Config data cannot be added for packages destinated to bootfs.
            if mapped_to_package_set == PackageSet::Bootfs && !entry.config_data.is_empty() {
                bail!(
                    "Config data cannot be added to {} because it is destined for bootfs",
                    manifest.name()
                );
            }

            // Add the config data entries to the map
            for ProductConfigData { source, destination } in entry.config_data {
                // If there are config_data entries, convert the TypedPathBuf pairs into
                // FileEntry objects.  From this point on, they are handled as FileEntry
                // TODO(tbd): Switch FileEntry to use TypedPathBuf instead of String and
                // PathBuf.
                self.add_config_data_entry(
                    manifest.name(),
                    FileEntry { source: source.into(), destination: destination.to_string() },
                )?;
            }
        }
        Ok(())
    }

    /// Add the product-provided base-drivers to the assembly configuration.
    ///
    /// This should be performed after all the platform bundles have
    /// been added as it is for packages. Packages specified as
    /// base driver packages should not be in the base package set and
    /// are added automatically.
    pub fn add_product_base_drivers(&mut self, drivers: Vec<DriverDetails>) -> Result<()> {
        // Base drivers are added to the base packages
        // Config data is not supported for driver packages since it is deprecated.
        for driver_details in drivers {
            self.add_driver_package(
                driver_details,
                PackageSet::Base,
                PackageOrigin::Product,
                None,
            )?;
        }
        Ok(())
    }

    /// Helper to add a driver package, handling path resolution and insertion into the correct
    /// driver set.
    fn add_driver_package(
        &mut self,
        driver_details: DriverDetails,
        set: PackageSet,
        origin: PackageOrigin,
        bundle_path: Option<&Utf8Path>,
    ) -> Result<()> {
        let driver_package_path =
            bundle_path.map(|p| p.join(&driver_details.package)).unwrap_or_else(|| {
                // If no bundle_path is provided, assume driver_details.package
                // is absolute.
                driver_details.package.clone()
            });

        self.add_package_from_path(&driver_package_path, origin, &set)?;

        let driver_package_type = match set {
            PackageSet::Base => DriverPackageType::Base,
            PackageSet::Bootfs => DriverPackageType::Boot,
            set => bail!("Unsupported package set '{set}' for drivers"),
        };

        let package_url =
            DriverManifestBuilder::get_package_url(driver_package_type, &driver_package_path)?;

        let driver_set = match &driver_package_type {
            DriverPackageType::Base => &mut self.base_drivers,
            DriverPackageType::Boot => &mut self.boot_drivers,
        };
        driver_set.try_insert_unique(package_url, driver_details)?;
        Ok(())
    }

    /// Add an entry to `config_data` for the given package.  If the entry
    /// duplicates an existing entry, return an error.
    fn add_config_data_entry(
        &mut self,
        package: impl AsRef<str>,
        entry: FileEntry<String>,
    ) -> Result<()> {
        let config_data = &mut self
            .package_configs
            .entry(package.as_ref().into())
            .or_insert_with(|| PackageConfiguration::new(package.as_ref()))
            .config_data;
        config_data.add_entry(entry).map_err(|dup| {
            anyhow!(
                "duplicate config data file found for package: {}\n  error: {}",
                package.as_ref(),
                dup,
            )
        })
    }

    fn add_bootfs_shell_command_entry(
        &mut self,
        package_name: impl AsRef<str>,
        binary: PackageInternalPathBuf,
    ) -> Result<()> {
        self.bootfs_shell_commands
            .entry(package_name.as_ref().into())
            .or_default()
            .try_insert_unique(binary)
            .map_err(|dup| {
                anyhow!(
                    "duplicate shell command found in package: {} = {}",
                    package_name.as_ref(),
                    dup
                )
            })
    }

    fn add_pkg_shell_command_entry(
        &mut self,
        package_name: impl AsRef<str>,
        binary: PackageInternalPathBuf,
    ) -> Result<()> {
        self.shell_commands
            .entry(package_name.as_ref().into())
            .or_default()
            .try_insert_unique(binary)
            .map_err(|dup| {
                anyhow!(
                    "duplicate shell command found in package: {} = {}",
                    package_name.as_ref(),
                    dup
                )
            })
    }

    /// Set the configuration updates for a package. Can only be called once per
    /// package.
    pub fn set_package_config(
        &mut self,
        package: impl AsRef<str>,
        config: PackageConfiguration,
    ) -> Result<()> {
        if self.package_configs.insert(package.as_ref().to_owned(), config).is_none() {
            Ok(())
        } else {
            Err(anyhow!("duplicate config patch"))
        }
    }

    /// Add a domain config package.
    pub fn add_domain_config(
        &mut self,
        package: PackageSetDestination,
        config: DomainConfig,
    ) -> Result<()> {
        if self.domain_configs.insert(package, config).is_none() {
            Ok(())
        } else {
            Err(anyhow!("duplicate domain config"))
        }
    }

    pub fn add_configuration_capabilities(
        &mut self,
        config: assembly_config_capabilities::CapabilityNamedMap,
    ) -> Result<()> {
        if self.configuration_capabilities.is_some() {
            return Err(anyhow!("duplicate configuration capabilities"));
        }
        self.configuration_capabilities = Some(config);
        Ok(())
    }

    pub fn add_compiled_package(
        &mut self,
        compiled_package_def: &CompiledPackageDefinition,
        bundle_path: &Utf8Path,
    ) -> Result<()> {
        let name = compiled_package_def.name.to_string();

        self.packages_to_compile
            .entry(name.clone())
            .or_insert_with(|| CompiledPackageBuilder::new(name.clone()))
            .add_package_def(compiled_package_def, bundle_path)
            .with_context(|| format!("Adding compiled-package definition: {}", name))?;
        Ok(())
    }

    pub fn add_zbi_extra_items(&mut self, zbi_extra_items_path: &Utf8Path) -> Result<()> {
        if self.zbi_extra_items.is_some() {
            bail!("duplicate zbi_extra_items image");
        }
        self.zbi_extra_items = Some(zbi_extra_items_path.into());
        Ok(())
    }

    pub fn add_devicetree(&mut self, devicetree_path: &Utf8Path) -> Result<()> {
        if self.devicetree.is_some() {
            bail!("duplicate devicetree binary");
        }
        self.devicetree = Some(devicetree_path.into());
        Ok(())
    }

    pub fn add_devicetree_overlay(&mut self, devicetree_overlay_path: &Utf8Path) -> Result<()> {
        if self.devicetree_overlay.is_some() {
            bail!("duplicate devicetree binary overlay");
        }
        self.devicetree_overlay = Some(devicetree_overlay_path.into());
        Ok(())
    }

    pub fn set_images_config(&mut self, images_config: ImagesConfig) -> Result<()> {
        if self.images_config.is_some() {
            bail!("image_config has already been set.");
        }
        self.images_config = Some(images_config);
        Ok(())
    }

    pub fn add_memory_buckets(&mut self, buckets: &Vec<Utf8PathBuf>) -> Result<()> {
        self.memory_buckets.add_buckets(buckets)
    }

    /// Construct an ImageAssembly ImageAssemblyConfig from the collected items in the
    /// builder, and report any validation issues.
    ///
    /// If there are config_data entries, the config_data package will be
    /// created in the outdir, and it will be added to the returned
    /// ImageAssemblyConfig.
    ///
    /// If there are compiled packages specified, the compiled packages will
    /// also be created in the outdir and added to the ImageAssemblyConfig.
    ///
    /// If this cannot create a completed ImageAssemblyConfig, it will return an error
    /// instead.
    ///
    /// If this _can_ create a completed ImageAssemblyConfig, but that
    /// ImageAssemblyConfig does not pass validation checks, it will return
    /// those validation errors via its second return value.
    pub fn build_and_validate(
        self,
        outdir: impl AsRef<Utf8Path>,
        tools: &impl ToolProvider,
        validation_mode: ValidationMode,
    ) -> Result<(ImageAssemblyConfig, Option<ProductValidationError>)> {
        let (config, validator) = self.build(outdir, tools)?;
        let error = validator.validate_product(&config, validation_mode);
        Ok((config, error.err()))
    }

    fn build(
        mut self,
        outdir: impl AsRef<Utf8Path>,
        tools: &impl ToolProvider,
    ) -> Result<(ImageAssemblyConfig, Validator)> {
        let outdir = outdir.as_ref();

        // Merge the memory buckets into a single file and make it available
        // to memory monitor. This has to happen during `build()` to ensure that
        // all the AIBs have been added, but before we decompose `self` below.
        let memory_buckets_path = outdir.join("memory_buckets.json");
        self.memory_buckets.write(&memory_buckets_path)?;
        self.add_config_data_entry(
            "memory_monitor",
            FileEntry { source: memory_buckets_path.clone(), destination: "buckets.json".into() },
        )?;
        self.add_config_data_entry(
            "memory_monitor2",
            FileEntry { source: memory_buckets_path, destination: "buckets.json".into() },
        )?;

        // Decompose the fields in self, so that they can be recomposed into the generated
        // image assembly configuration.
        let Self {
            build_type: _,
            feature_set_level: _,
            board_name,
            partitions_config,
            image_mode,
            mode,
            package_configs,
            domain_configs,
            mut packages,
            base_drivers,
            boot_drivers,
            boot_args,
            bootfs_files,
            kernel_path,
            mut kernel_args,
            mut kernel_arg_overrides,
            qemu_kernel,
            bootfs_shell_commands,
            shell_commands,
            packages_to_compile,
            memory_buckets: _,
            board_driver_arguments,
            configuration_capabilities,
            zbi_extra_items,
            devicetree,
            devicetree_overlay,
            developer_only_options: _,
            images_config,
            system_release_info,
        } = self;

        if !boot_args.is_empty() {
            bail!("Found additional boot args")
        }

        let cmc_tool = tools.get_tool("cmc")?;

        // Add dynamically compiled packages first so they are all present
        // and can be repackaged and configured
        for (_, package_builder) in packages_to_compile {
            let package_name = package_builder.name.to_owned();
            let package_manifest_path = package_builder
                .build(cmc_tool.as_ref(), outdir)
                .with_context(|| format!("building compiled package {}", package_name))?;

            match package_manifest_path {
                (p, PackageSet::Base) => {
                    let d = PackageSetDestination::Blob(PackageDestination::FromProduct(
                        package_name.clone(),
                    ));
                    let (_d, package_entry) =
                        PackageEntry::parse_from(PackageOrigin::AIB, PackageSet::Base, p)?;

                    packages
                        .try_insert_unique(d, package_entry)
                        .with_context(|| format!("Adding compiled package {package_name}"))?;
                }
                (p, PackageSet::Bootfs) => {
                    let d = PackageSetDestination::Boot(BootfsPackageDestination::FromAIB(
                        package_name.clone(),
                    ));
                    let (_d, package_entry) =
                        PackageEntry::parse_from(PackageOrigin::AIB, PackageSet::Bootfs, p)?;
                    packages
                        .try_insert_unique(d, package_entry)
                        .with_context(|| format!("Adding compiled package {package_name}"))?;
                }
                (_, package_set) => panic!("Unexpected package set {package_set}"),
            };
        }

        // Repackage any matching packages
        for (package, config) in &package_configs {
            // Only process configs that have component entries for structured config.
            if !config.components.is_empty() {
                // Get the manifest for this package name, removing it from the set.  There are two
                // different potential destinations for this, so try each in order, removing the
                // first entry that's found, so that we can replace it with the
                if let Some((destination, entry)) = vec![
                    PackageSetDestination::Blob(PackageDestination::FromAIB(package.clone())),
                    PackageSetDestination::Boot(BootfsPackageDestination::FromAIB(package.clone())),
                ]
                .into_iter()
                .find_map(|d| packages.remove(&d).map(|entry| (d, entry)))
                {
                    // Create the new package
                    let outdir = outdir.join("repackaged").join(package);
                    let mut repackager = Repackager::new(entry.manifest, outdir)
                        .with_context(|| format!("reading existing manifest for {package}"))?;

                    // Iterate over the components to get their structured config values
                    for (component, values) in &config.components {
                        repackager
                            .set_component_config(component, values.fields.clone().into())
                            .with_context(|| format!("setting new config for {component}"))?;
                    }
                    let new_path = repackager
                        .build()
                        .with_context(|| format!("building repackaged {package}"))?;
                    let (_, new_entry) =
                        PackageEntry::parse_from(PackageOrigin::AIB, entry.package_set, new_path)
                            .with_context(
                            || "Parsing new package manifest for repackaged package: {package}",
                        )?;

                    // Re-add the package now that it's been repackaged.
                    packages.try_insert_unique(destination, new_entry).with_context(|| {
                        format!("Re-adding package after repackaging with new config: {package}")
                    })?;
                } else {
                    // TODO(https://fxbug.dev/42052394) return an error here
                }
            }
        }

        // Construct the domain config packages
        for (destination, config) in domain_configs {
            let outdir = outdir.join(destination.to_string());
            std::fs::create_dir_all(&outdir)
                .with_context(|| format!("creating directory {outdir}"))?;
            let package = DomainConfigPackage::new(config);
            let (path, manifest) = package
                .build(outdir)
                .with_context(|| format!("building domain config package {destination}"))?;

            let package_set = match &destination {
                PackageSetDestination::Blob(_) => PackageSet::Base,
                PackageSetDestination::Boot(_) => PackageSet::Bootfs,
            };
            packages
                .try_insert_unique(destination, PackageEntry { path, manifest, package_set })
                .context("Adding domain config package")?;
        }

        // Generate the driver manifests and add to the configuration capability package
        let mut configuration_capabilities =
            configuration_capabilities.unwrap_or_else(|| NamedMap::new("config capabilities"));
        {
            let mut driver_manifest_builder = DriverManifestBuilder::default();
            for (package_url, driver_details) in boot_drivers.entries {
                driver_manifest_builder
                    .add_driver(driver_details, &package_url)
                    .with_context(|| format!("adding driver {}", package_url))?;
            }

            configuration_capabilities.try_insert_unique(
                "fuchsia.driver.BootDrivers".to_string(),
                driver_manifest_builder.create_config(),
            )?;
        }
        {
            let mut driver_manifest_builder = DriverManifestBuilder::default();
            for (package_url, driver_details) in base_drivers.entries {
                driver_manifest_builder
                    .add_driver(driver_details, &package_url)
                    .with_context(|| format!("adding driver {}", package_url))?;
            }

            configuration_capabilities.try_insert_unique(
                "fuchsia.driver.BaseDrivers".to_string(),
                driver_manifest_builder.create_config(),
            )?;
        }

        // Construct the config capability package.
        if !mode.is_test_kernel() {
            let package_name = "config";
            let outdir = outdir.join(package_name);
            std::fs::create_dir_all(&outdir)
                .with_context(|| format!("creating directory {outdir}"))?;

            let (path, manifest) = assembly_config_capabilities::build_config_capability_package(
                configuration_capabilities,
                &outdir,
            )
            .with_context(|| format!("building config capabilties package {package_name}"))?;
            packages
                .try_insert_unique(
                    PackageSetDestination::Boot(BootfsPackageDestination::Config),
                    PackageEntry { path, manifest, package_set: PackageSet::Bootfs },
                )
                .context("Adding config capabilities package")?;
        }

        // Build the config_data package if we have any blobfs packages.
        if packages.has_blobs() {
            let mut config_data_builder = ConfigDataBuilder::default();
            for (package_name, config) in &package_configs {
                for (destination, source_merkle_pair) in config.config_data.iter() {
                    config_data_builder.add_entry(
                        package_name,
                        destination.clone().into(),
                        source_merkle_pair.source.clone(),
                    )?;
                }
            }
            let manifest_path = config_data_builder
                .build(outdir)
                .context("writing the 'config_data' package metafar.")?;
            let (_, entry) =
                PackageEntry::parse_from(PackageOrigin::AIB, PackageSet::Base, manifest_path)?;
            packages
                .try_insert_unique(
                    PackageSetDestination::Blob(PackageDestination::ConfigData),
                    entry,
                )
                .context("Adding generated config-data package")?;
        }

        if !shell_commands.is_empty() {
            let mut shell_commands_builder = ShellCommandsBuilder::new_pkg();
            shell_commands_builder.add_shell_commands(shell_commands, "fuchsia.com".to_string());
            let manifest_path =
                shell_commands_builder.build(outdir).context("building shell commands package")?;
            let (_, entry) =
                PackageEntry::parse_from(PackageOrigin::AIB, PackageSet::Base, manifest_path)?;
            packages
                .try_insert_unique(
                    PackageSetDestination::Blob(PackageDestination::ShellCommands),
                    entry,
                )
                .context("Adding shell commands package to base")?;
        }

        if !bootfs_shell_commands.is_empty() {
            let mut shell_commands_builder = ShellCommandsBuilder::new_bootfs();
            shell_commands_builder.add_bootfs_shell_commands(bootfs_shell_commands);
            let manifest_path = shell_commands_builder
                .build(outdir)
                .context("building bootfs shell commands package")?;
            let (_, entry) =
                PackageEntry::parse_from(PackageOrigin::AIB, PackageSet::Bootfs, manifest_path)?;
            packages
                .try_insert_unique(
                    PackageSetDestination::Boot(BootfsPackageDestination::ShellCommands),
                    entry,
                )
                .context("Adding shell commands package to bootfs")?;
        }

        let bootfs_files = bootfs_files
            .into_file_entries()
            .iter()
            .map(|e| FileEntry { source: e.source.clone(), destination: e.destination.to_string() })
            .collect();

        // merge all the kernel_args overrides into the kernel_args map, letting them replace any
        // existing values
        kernel_args.append(&mut kernel_arg_overrides);

        // Construct a single "partial" config from the combined fields, and
        // then pass this to the ImageAssemblyConfig::try_from_partials() to get the
        // final validation that it's complete.
        let image_assembly_config = ImageAssemblyConfig {
            base: packages.package_manifest_paths(PackageSet::Base),
            cache: packages.package_manifest_paths(PackageSet::Cache),
            anchored_automatic: packages.package_manifest_paths(PackageSet::AnchoredAutomatic),
            anchored_on_demand: packages.package_manifest_paths(PackageSet::AnchoredOnDemand),
            system: packages.package_manifest_paths(PackageSet::System),
            bootfs_packages: packages.package_manifest_paths(PackageSet::Bootfs),
            on_demand: packages.package_manifest_paths(PackageSet::OnDemand),
            kernel: KernelConfig {
                path: kernel_path.context("A kernel path must be specified")?,
                args: kernel_args.values().map(ToString::to_string).collect(),
            },
            qemu_kernel: qemu_kernel.context("A qemu kernel configuration must be specified")?,
            boot_args: boot_args.into_iter().collect(),
            bootfs_files,
            images_config: images_config.unwrap_or_default(),
            board_name,
            partitions_config,
            board_driver_arguments,
            zbi_extra_items,
            devicetree,
            devicetree_overlay,
            system_release_info,
            image_mode,
            build_type: self.build_type,
        };

        if mode.is_test_kernel() {
            anyhow::ensure!(
                image_mode == FilesystemImageMode::NoImage,
                "Products using a test kernel must use image_mode=no_image"
            );
            anyhow::ensure!(
                image_assembly_config.bootfs_packages.is_empty(),
                format!(
                    "Bootfs packages are not allowed on products with a test kernel. Found: {:?}",
                    image_assembly_config.bootfs_packages
                )
            );
            anyhow::ensure!(
                image_assembly_config.bootfs_files.is_empty(),
                format!(
                    "Bootfs files are not allowed on products with a test kernel. Found: {:?}",
                    image_assembly_config.bootfs_files
                )
            );
        }

        if image_mode == FilesystemImageMode::NoImage {
            anyhow::ensure!(
                image_assembly_config.base.is_empty(),
                format!(
                    "Base packages are not allowed on 'no_image' products. Found: {:?}",
                    image_assembly_config.base
                )
            );
            anyhow::ensure!(
                image_assembly_config.cache.is_empty(),
                format!(
                    "Cache packages are not allowed on 'no_image' products. Found: {:?}",
                    image_assembly_config.cache
                )
            );
        }
        Ok((image_assembly_config, Validator))
    }
}

/// A wrapper around all the packages
#[derive(Debug, Default, Serialize)]
struct Packages {
    inner: BTreeMap<PackageSetDestination, PackageEntry>,
}

impl Packages {
    /// Insert a package entry by its destination.  This enforces that there no duplicate package
    /// names, regardless of destination. *Note*: the `Display` impl on PackageSetDestination and
    /// PackageDestination is used as input to the `cmp` functions for those types, so a package
    /// from the board named "foo" and a package from the product named "foo" evaluate to the same
    /// for the purposes of this function.
    fn try_insert_unique(
        &mut self,
        destination: PackageSetDestination,
        entry: PackageEntry,
    ) -> Result<()> {
        self.inner
            .try_insert_unique(MapEntry(destination, entry))
            .map_err(|e| anyhow!("Duplicate package found {}", e.existing_entry.key()))
    }

    /// Remove a package entry by its destination.
    fn remove(&mut self, destination: &PackageSetDestination) -> Option<PackageEntry> {
        self.inner.remove(destination)
    }

    /// Returns true if there are any packages destined for BlobFS.
    fn has_blobs(&self) -> bool {
        self.inner.iter().any(|(d, _)| matches!(d, PackageSetDestination::Blob(_)))
    }

    /// Returns a sorted list of the package manifests for a given package set.
    fn package_manifest_paths(&self, package_set: PackageSet) -> Vec<Utf8PathBuf> {
        self.inner
            .values()
            .filter_map(|e| if e.package_set == package_set { Some(e.path.clone()) } else { None })
            .sorted()
            .collect()
    }
}

/// Information about a single package in the set.
#[derive(Debug, Serialize)]
struct PackageEntry {
    /// Path to the package manifest.
    path: Utf8PathBuf,

    /// Parsed package manifest.
    manifest: PackageManifest,

    /// Which package set that the package belongs to.
    package_set: PackageSet,
}

impl PackageEntry {
    /// Construct a PackageEntry from a path to a package manifest, and a given
    /// destination package set.
    pub fn parse_from(
        origin: PackageOrigin,
        package_set: PackageSet,
        path: impl AsRef<Utf8Path>,
    ) -> Result<(PackageSetDestination, Self)> {
        let path = path.as_ref();
        let manifest = PackageManifest::try_load_from(path)
            .with_context(|| format!("parsing {path} as a package manifest"))?;

        let name = manifest.name().to_string();
        if name.is_empty() {
            bail!("Package with no name {path}");
        }

        let destination = match &package_set {
            PackageSet::Base
            | PackageSet::BootfsOrBase
            | PackageSet::Cache
            | PackageSet::System
            | PackageSet::Flexible
            | PackageSet::OnDemand
            | PackageSet::AnchoredAutomatic
            | PackageSet::AnchoredOnDemand => PackageSetDestination::Blob(match &origin {
                PackageOrigin::AIB => PackageDestination::FromAIB(name),
                PackageOrigin::Board => PackageDestination::FromBoard(name),
                PackageOrigin::Product => PackageDestination::FromProduct(name),
                PackageOrigin::Developer => PackageDestination::FromDeveloper(name),
            }),
            PackageSet::Bootfs => PackageSetDestination::Boot(match &origin {
                PackageOrigin::AIB => BootfsPackageDestination::FromAIB(name),
                PackageOrigin::Board => BootfsPackageDestination::FromBoard(name),
                PackageOrigin::Developer => BootfsPackageDestination::FromDeveloper(name),
                PackageOrigin::Product => BootfsPackageDestination::FromProduct(name),
            }),
        };

        Ok((destination, Self { path: path.to_path_buf(), manifest, package_set }))
    }
}

#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum PackageOrigin {
    /// The package is from a platform AIB
    AIB,

    /// The package is from the board
    Board,

    /// The package is from the product
    Product,

    /// The package is from the developer
    Developer,
}

impl std::fmt::Display for PackageOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PackageOrigin::AIB => "platform",
            PackageOrigin::Board => "board",
            PackageOrigin::Product => "product",
            PackageOrigin::Developer => "developer",
        })
    }
}

/// A key=value kernel commandline argument
///
/// This parses an argument like "arg1=value1" into a key-value pair.
/// For args with no value (vs. empty value), such as "arg2", they are represented as a key with
/// a None value.
#[derive(Debug, Serialize)]
struct KernelArg {
    key: String,
    value: Option<String>,
}

impl From<&str> for KernelArg {
    fn from(arg: &str) -> KernelArg {
        if let Some((key, value)) = arg.split_once("=") {
            Self { key: key.to_owned(), value: Some(value.to_owned()) }
        } else {
            Self { key: arg.to_owned(), value: None }
        }
    }
}

impl From<&String> for KernelArg {
    fn from(arg: &String) -> KernelArg {
        KernelArg::from(arg.as_str())
    }
}

impl std::fmt::Display for KernelArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(value) = &self.value {
            f.write_fmt(format_args!("{}={}", self.key, value))
        } else {
            f.write_str(&self.key)
        }
    }
}

struct Validator;

impl Validator {
    /// Validate a product config.
    fn validate_product(
        &self,
        product: &ImageAssemblyConfig,
        validation_mode: ValidationMode,
    ) -> Result<(), ProductValidationError> {
        // validate the packages in the system/base/cache package sets
        let manifests =
            product.system.iter().chain(product.base.iter()).chain(product.cache.iter());
        let packages: BTreeMap<_, _> = manifests
        .par_bridge()
        .filter_map(|package_manifest_path| {
            match PackageManifest::try_load_from(&package_manifest_path) {
                Ok(manifest) => {
                    // After loading the manifest, validate it
                    match validate_package(&manifest) {
                        Ok(()) => None,
                        Err(e) => {
                            // If validation issues have been downgraded to warnings, then print
                            // a warning instead of returning an error.
                            let warning_eligible = match e {
                                PackageValidationError::MissingAbiRevisionFile(_) => true,
                                PackageValidationError::InvalidAbiRevisionFile(_) => true,
                                PackageValidationError::UnsupportedAbiRevision (_) => true,
                                _ => false,
                            };
                            match (&validation_mode, warning_eligible) {
                                (&ValidationMode::Off, _) => None,
                                (&ValidationMode::WarnOnly, false) | (&ValidationMode::On, _) => Some((package_manifest_path.to_owned(), e)),
                                (&ValidationMode::WarnOnly, true) => {
                                    eprintln!("WARNING: The package named '{}', with manifest at {} failed validation:\n{}", manifest.name(), package_manifest_path, e);
                                    None
                                },
                            }
                        }
                    }
                }
                // Convert any error loading the manifest into the appropriate
                // error type.
                Err(e) => Some((package_manifest_path.to_owned(), PackageValidationError::LoadPackageManifest(e)))
            }
        })
        .collect();

        // validate the contents of bootfs
        let bootfs = match validate_bootfs(&product.bootfs_files) {
            Ok(()) => None,
            Err(e) => Some(e),
        };

        let cache_package_environments =
            validate_cache_package_environments(product).err().unwrap_or_default();

        if packages.is_empty() && bootfs.is_none() && cache_package_environments.is_empty() {
            Ok(())
        } else {
            Err(ProductValidationError { bootfs, packages, cache_package_environments })
        }
    }
}

/// Read the `core.cm` component declaration from the `core` package in `base_packages` if present.
fn read_core_component_decl(base_packages: &[Utf8PathBuf]) -> Result<Option<Component>> {
    for manifest_path in base_packages {
        let Ok(manifest) = PackageManifest::try_load_from(manifest_path) else {
            continue;
        };
        if manifest.name().as_ref() != "core" {
            continue;
        }
        let Some(meta_far_info) = manifest.blobs().iter().find(|b| b.path == "meta/") else {
            continue;
        };
        let meta_far = File::open(&meta_far_info.source_path)?;
        let mut reader = Utf8Reader::new(meta_far)?;
        let bytes = reader.read_file("meta/core.cm")?;
        let decl = unpersist::<Component>(&bytes)?;
        return Ok(Some(decl));
    }
    Ok(None)
}

/// Validate that static children in base packages (such as `core`) that refer to packages in
/// `cache_packages` explicitly specify an environment (such as `#core-env`). Without an
/// environment, children inherit the parent's environment (base-resolver), which cannot resolve
/// cache packages.
fn validate_cache_package_environments(product: &ImageAssemblyConfig) -> Result<(), Vec<String>> {
    let cache_package_names: BTreeSet<String> = product
        .cache
        .iter()
        .filter_map(|path| PackageManifest::try_load_from(path).ok())
        .map(|manifest| manifest.name().to_string())
        .collect();

    if cache_package_names.is_empty() {
        return Ok(());
    }

    let Ok(Some(decl)) = read_core_component_decl(&product.base) else {
        return Ok(());
    };
    let Some(children) = decl.children else {
        return Ok(());
    };

    let mut errors = Vec::new();
    for child in children {
        let (Some(child_name), Some(url)) = (child.name, child.url) else {
            continue;
        };
        let Ok(component_url) = AbsoluteComponentUrl::parse(&url) else {
            continue;
        };
        let package_name = component_url.package_url().name().as_ref();
        if !cache_package_names.contains(package_name) {
            continue;
        }
        if child.environment.is_some() {
            continue;
        }
        errors.push(format!(
            "Child '{child_name}' with url '{url}' in 'core/meta/core.cm' is in cache_packages, but does not specify an environment (such as '#core-env'). Components in cache packages must use an environment that resolves via full-resolver."
        ));
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Validate the contents of bootfs.
///
/// Assumes that all component manifests have a `.cm` extension within the destination namespace.
fn validate_bootfs(bootfs_files: &[FileEntry<String>]) -> Result<(), BootfsValidationError> {
    let mut bootfs = BootfsContents::from_iter(
        bootfs_files.iter().map(|entry| (entry.destination.to_string(), &entry.source)),
    )
    .map_err(BootfsValidationError::ReadContents)?;

    // validate components
    let mut errors = BTreeMap::new();
    for path in bootfs.paths().into_iter().filter(|p| p.ends_with(".cm")) {
        if let Err(e) = validate_component(&path, &mut bootfs) {
            errors.insert(path, e);
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(BootfsValidationError::InvalidComponents(errors)) }
}

/// Collection of all package validation failures within a product.
#[derive(Debug)]
pub struct ProductValidationError {
    /// Files in bootfs which failed validation.
    bootfs: Option<BootfsValidationError>,
    /// Packages which failed validation.
    packages: BTreeMap<Utf8PathBuf, PackageValidationError>,
    /// Cache packages referenced as children without an environment.
    cache_package_environments: Vec<String>,
}

impl From<ProductValidationError> for anyhow::Error {
    fn from(e: ProductValidationError) -> anyhow::Error {
        anyhow::Error::msg(e)
    }
}

impl std::fmt::Display for ProductValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Validating product assembly failed:")?;
        if let Some(error) = &self.bootfs {
            let error_msg = textwrap::indent(&error.to_string(), "        ");
            write!(f, "    └── Failed to validate bootfs: {}", error_msg)?;
        }
        for (package, error) in &self.packages {
            let error_msg = textwrap::indent(&error.to_string(), "        ");
            write!(f, "    └── {}: {}", package, error_msg)?;
        }
        for err in &self.cache_package_environments {
            let error_msg = textwrap::indent(err, "        ");
            write!(f, "    └── {}", error_msg)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum BootfsValidationError {
    ReadContents(assembly_validate_util::BootfsContentsError),
    InvalidComponents(BTreeMap<String, anyhow::Error>),
}

impl std::fmt::Display for BootfsValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use BootfsValidationError::*;
        match self {
            ReadContents(source) => {
                write!(f, "Unable to read bootfs contents: {}", source)
            }
            InvalidComponents(components) => {
                for (name, error) in components {
                    write!(f, "\n└── {}: {}", name, error)?;
                    let mut source = error.source();
                    while let Some(s) = source {
                        write!(f, "\n    └── {}", s)?;
                        source = s.source();
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assembly_config_schema::assembly_input_bundle::{
        CompiledComponentDefinition, PartialKernelConfig,
    };
    use assembly_config_schema::developer_overrides::KernelOptions;
    use assembly_constants::CompiledPackageDestination;
    use assembly_constants::TestCompiledPackageDestination::ForTest;
    use assembly_named_file_map::SourceMerklePair;
    use assembly_package_utils::PackageManifestPathBuf;
    use assembly_platform_configuration::ComponentConfigs;
    use assembly_release_info::SystemReleaseInfo;
    use assembly_test_util::generate_test_manifest;
    use assembly_tool::ToolCommandLog;
    use assembly_tool::testing::FakeToolProvider;
    use fuchsia_pkg::{BlobInfo, MetaPackage, PackageBuilder, PackageManifestBuilder};
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    struct TempdirPathsForTest {
        _tmp: TempDir,
        pub outdir: Utf8PathBuf,
        pub bundle_path: Utf8PathBuf,
        pub config_data_target_package_name: String,
        pub config_data_target_package_dir: Utf8PathBuf,
        pub config_data_file_path: Utf8PathBuf,
    }

    impl TempdirPathsForTest {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let outdir = Utf8Path::from_path(tmp.path()).unwrap().to_path_buf();
            let bundle_path = outdir.join("bundle");
            let config_data_target_package_name = "base_package0".to_owned();
            let config_data_target_package_dir =
                bundle_path.join("config_data").join(&config_data_target_package_name);
            let config_data_file_path =
                config_data_target_package_dir.join("config_data_source_file");
            Self {
                _tmp: tmp,
                outdir,
                bundle_path,
                config_data_target_package_name,
                config_data_target_package_dir,
                config_data_file_path,
            }
        }
    }

    fn write_empty_pkg(
        path: impl AsRef<Utf8Path>,
        name: &str,
        repo: Option<&str>,
    ) -> PackageManifestPathBuf {
        let path = path.as_ref();
        let mut builder = PackageBuilder::new_platform_internal_package(name);
        let manifest_path = path.join(name);
        builder.manifest_path(&manifest_path);
        if let Some(repo_name) = repo {
            builder.repository(repo_name);
        } else {
            builder.repository("fuchsia.com");
        }
        builder.build(path, path.join(format!("{name}_meta.far"))).unwrap();
        manifest_path.into()
    }

    fn make_test_assembly_bundle_with_base(
        outdir: &Utf8Path,
        bundle_path: &Utf8Path,
    ) -> AssemblyInputBundle {
        make_test_assembly_bundle(outdir, bundle_path, true /* include_base */)
    }

    fn make_test_assembly_bundle(
        outdir: &Utf8Path,
        bundle_path: &Utf8Path,
        include_base: bool,
    ) -> AssemblyInputBundle {
        let test_file_path = outdir.join("bootfs_files_package");
        let mut test_file = File::create(&test_file_path).unwrap();
        let builder = PackageManifestBuilder::new(MetaPackage::from_name_and_variant_zero(
            "bootfs_files_package".parse().unwrap(),
        ));
        let builder = builder.repository("testrepository.com");
        let builder = builder.add_blob(BlobInfo {
            source_path: "source/path/to/file".into(),
            path: "dest/file/path".into(),
            merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                .parse()
                .unwrap(),
            size: 1,
        });
        let manifest = builder.build();
        serde_json::to_writer(&test_file, &manifest).unwrap();
        test_file.flush().unwrap();

        let write_empty_bundle_pkg =
            |name: &str| write_empty_pkg(bundle_path, name, None).to_string().into();
        let mut packages = vec![
            PackageDetails {
                package: write_empty_bundle_pkg("bootfs_package0"),
                set: PackageSet::Bootfs,
            },
            PackageDetails {
                package: write_empty_bundle_pkg("bootfs_or_base_package0"),
                set: PackageSet::BootfsOrBase,
            },
        ];
        if include_base {
            packages.extend(vec![
                PackageDetails {
                    package: write_empty_bundle_pkg("base_package0"),
                    set: PackageSet::Base,
                },
                PackageDetails {
                    package: write_empty_bundle_pkg("cache_package0"),
                    set: PackageSet::Cache,
                },
                PackageDetails {
                    package: write_empty_bundle_pkg("flexible_package0"),
                    set: assembly_config_schema::PackageSet::Flexible,
                },
                PackageDetails {
                    package: write_empty_bundle_pkg("sys_package0"),
                    set: PackageSet::System,
                },
            ]);
        }

        AssemblyInputBundle {
            allowed_in: Default::default(),
            scrutiny_required: Default::default(),
            auto_include_in: Default::default(),
            experimental: false,
            kernel: Some(PartialKernelConfig {
                path: Some("kernel/path".into()),
                args: vec!["kernel_arg0".into()],
            }),
            qemu_kernel: Some("path/to/qemu/kernel".into()),
            boot_args: vec![],
            bootfs_files: vec![],
            packages,
            drivers: Vec::default(),
            config_data: BTreeMap::default(),
            blobs: Vec::default(),
            bootfs_shell_commands: ShellCommands::default(),
            shell_commands: ShellCommands::default(),
            packages_to_compile: Vec::default(),
            bootfs_files_package: Some(test_file_path),
            memory_buckets: Vec::default(),
        }
    }

    fn make_test_driver(package_name: &str, outdir: impl AsRef<Utf8Path>) -> Result<DriverDetails> {
        let driver_package_manifest_file_path = outdir.as_ref().join(package_name);
        let mut driver_package_manifest_file = File::create(&driver_package_manifest_file_path)?;
        let package_manifest = generate_test_manifest(package_name, None);
        serde_json::to_writer(&driver_package_manifest_file, &package_manifest)?;
        driver_package_manifest_file.flush()?;

        Ok(DriverDetails {
            package: driver_package_manifest_file_path.into(),
            components: vec![Utf8PathBuf::from("meta/foobar.cm")],
        })
    }

    /// Create an ImageAssemblyConfigBuilder with a minimal AssemblyInputBundle
    /// for testing product configuration.
    ///
    /// # Arguments
    ///
    /// * `package_names` - names for empty stub packages to create and add to the
    ///    base set.
    fn get_minimum_config_builder(
        outdir: impl AsRef<Utf8Path>,
        package_names: Vec<String>,
    ) -> ImageAssemblyConfigBuilder {
        let minimum_bundle = AssemblyInputBundle {
            kernel: Some(PartialKernelConfig {
                path: Some("kernel/path".into()),
                args: Vec::default(),
            }),
            qemu_kernel: Some("kernel/qemu/path".into()),
            packages: package_names
                .iter()
                .map(|package_name| PackageDetails {
                    package: write_empty_pkg(&outdir, package_name, None).to_string().into(),
                    set: PackageSet::Base,
                })
                .collect(),
            drivers: Vec::default(),
            config_data: BTreeMap::default(),
            blobs: Vec::default(),
            bootfs_shell_commands: ShellCommands::default(),
            shell_commands: ShellCommands::default(),
            packages_to_compile: Vec::default(),
            bootfs_files_package: None,
            ..AssemblyInputBundle::default()
        };
        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder.add_parsed_bundle(outdir.as_ref().join("minimum_bundle"), minimum_bundle).unwrap();
        builder
    }

    #[test]
    fn test_builder() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder
            .add_parsed_bundle(
                &vars.outdir,
                make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path),
            )
            .unwrap();
        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        assert_eq!(
            result.base,
            vec![
                vars.bundle_path.join("base_package0"),
                vars.bundle_path.join("bootfs_or_base_package0"),
                vars.outdir.join("config_data/package_manifest.json")
            ]
        );
        assert_eq!(
            result.cache,
            vec![
                vars.bundle_path.join("cache_package0"),
                vars.bundle_path.join("flexible_package0"),
            ]
        );
        assert_eq!(result.system, vec![vars.bundle_path.join("sys_package0")]);
        assert_eq!(
            result.bootfs_packages,
            vec![
                vars.bundle_path.join("bootfs_package0"),
                vars.outdir.join("config/package_manifest.json"),
            ]
        );
        assert!(result.boot_args.is_empty());
        assert_eq!(
            result
                .bootfs_files
                .iter()
                .map(|f| f.destination.to_owned())
                .sorted()
                .collect::<Vec<_>>(),
            vec!["dest/file/path"],
        );

        assert_eq!(result.kernel.path, vars.outdir.join("kernel/path"));
        assert_eq!(result.kernel.args, vec!("kernel_arg0".to_string()));
        assert_eq!(result.qemu_kernel, vars.outdir.join("path/to/qemu/kernel"));
    }

    #[test]
    fn test_builder_userdebug() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::UserDebug,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder
            .add_parsed_bundle(
                &vars.outdir,
                make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path),
            )
            .unwrap();
        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        assert_eq!(
            result.base,
            vec![
                vars.bundle_path.join("base_package0"),
                vars.bundle_path.join("bootfs_or_base_package0"),
                vars.bundle_path.join("flexible_package0"),
                vars.outdir.join("config_data/package_manifest.json")
            ]
        );
        assert_eq!(result.cache, vec![vars.bundle_path.join("cache_package0"),]);
        assert_eq!(result.system, vec![vars.bundle_path.join("sys_package0")]);
        assert_eq!(
            result.bootfs_packages,
            vec![
                vars.bundle_path.join("bootfs_package0"),
                vars.outdir.join("config/package_manifest.json"),
            ]
        );
        assert!(result.boot_args.is_empty());
        assert_eq!(
            result
                .bootfs_files
                .iter()
                .map(|f| f.destination.to_owned())
                .sorted()
                .collect::<Vec<_>>(),
            vec!["dest/file/path".to_string()],
        );

        assert_eq!(result.kernel.path, vars.outdir.join("kernel/path"));
        assert_eq!(result.kernel.args, vec!("kernel_arg0".to_string()));
        assert_eq!(result.qemu_kernel, vars.outdir.join("path/to/qemu/kernel"));
    }

    #[test]
    fn test_builder_user() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::User,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder
            .add_parsed_bundle(
                &vars.outdir,
                make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path),
            )
            .unwrap();
        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        assert_eq!(
            result.base,
            vec![
                vars.bundle_path.join("base_package0"),
                vars.bundle_path.join("bootfs_or_base_package0"),
                vars.bundle_path.join("flexible_package0"),
                vars.outdir.join("config_data/package_manifest.json")
            ]
        );
        assert_eq!(result.cache, vec![vars.bundle_path.join("cache_package0"),]);
        assert_eq!(result.system, vec![vars.bundle_path.join("sys_package0")]);
        assert_eq!(
            result.bootfs_packages,
            vec![
                vars.bundle_path.join("bootfs_package0"),
                vars.outdir.join("config/package_manifest.json"),
            ]
        );
        assert!(result.boot_args.is_empty());
        assert_eq!(
            result
                .bootfs_files
                .iter()
                .map(|f| f.destination.to_owned())
                .sorted()
                .collect::<Vec<_>>(),
            vec!["dest/file/path".to_string()],
        );

        assert_eq!(result.kernel.path, vars.outdir.join("kernel/path"));
        assert_eq!(result.kernel.args, vec!("kernel_arg0".to_string()));
        assert_eq!(result.qemu_kernel, vars.outdir.join("path/to/qemu/kernel"));
    }

    #[test]
    fn test_builder_no_image() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::UserDebug,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::NoImage,
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder
            .add_parsed_bundle(
                &vars.outdir,
                make_test_assembly_bundle(
                    &vars.outdir,
                    &vars.bundle_path,
                    false, /* !include_base */
                ),
            )
            .unwrap();
        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        assert!(result.base.is_empty());
        assert!(result.cache.is_empty());
        assert!(result.system.is_empty());
        assert_eq!(
            result.bootfs_packages,
            vec![
                vars.bundle_path.join("bootfs_or_base_package0"),
                vars.bundle_path.join("bootfs_package0"),
                vars.outdir.join("config/package_manifest.json"),
            ]
        );
        assert!(result.boot_args.is_empty());
        assert_eq!(
            result
                .bootfs_files
                .iter()
                .map(|f| f.destination.to_owned())
                .sorted()
                .collect::<Vec<_>>(),
            vec!["dest/file/path".to_string()],
        );

        assert_eq!(result.kernel.path, vars.outdir.join("kernel/path"));
        assert_eq!(result.kernel.args, vec!("kernel_arg0".to_string()));
        assert_eq!(result.qemu_kernel, vars.outdir.join("path/to/qemu/kernel"));
    }

    #[test]
    fn test_aib_not_allowed_on_user() {
        let vars = TempdirPathsForTest::new();

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::User,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        let mut bundle = AssemblyInputBundle::default();
        bundle.allowed_in.insert(FeatureSetLevel::Standard, vec![BuildType::Eng]);

        let result = builder.add_parsed_bundle(&vars.outdir, bundle);
        assert!(result.is_err());
    }

    fn setup_builder(
        vars: &TempdirPathsForTest,
        bundles: Vec<AssemblyInputBundle>,
    ) -> ImageAssemblyConfigBuilder {
        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );

        // Write a file to the temp dir for use with config_data.
        std::fs::create_dir_all(&vars.config_data_target_package_dir).unwrap();
        std::fs::write(&vars.config_data_file_path, "configuration data").unwrap();
        for bundle in bundles {
            builder.add_parsed_bundle(&vars.bundle_path, bundle).unwrap();
        }
        builder
    }

    #[test]
    fn test_builder_with_config_data() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        // Create an assembly bundle and add a config_data entry to it.
        let mut bundle = make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path);

        bundle.config_data.insert(
            vars.config_data_target_package_name.clone(),
            vec![FileEntry {
                source: vars.config_data_file_path.clone(),
                destination: "dest/file/path".to_owned(),
            }],
        );

        let mut builder = setup_builder(&vars, vec![]);
        builder
            .set_package_config(
                vars.config_data_target_package_name.clone(),
                PackageConfiguration {
                    components: ComponentConfigs::new("component configs"),
                    name: vars.config_data_target_package_name.clone(),
                    config_data: NamedFileMap {
                        map: NamedMap {
                            name: "config data".into(),
                            entries: [(
                                "dest/platform/configuration".into(),
                                SourceMerklePair {
                                    merkle: None,
                                    source: vars.config_data_file_path,
                                },
                            )]
                            .into(),
                        },
                    },
                },
            )
            .unwrap();
        builder.add_parsed_bundle(&vars.bundle_path, bundle).unwrap();
        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        // config_data's manifest is in outdir
        let expected_config_data_manifest_path =
            vars.outdir.join("config_data").join("package_manifest.json");

        // Validate that the base package set contains config_data.
        assert_eq!(result.base.len(), 3);
        assert!(result.base.contains(&vars.bundle_path.join("base_package0")));
        assert!(result.base.contains(&expected_config_data_manifest_path));

        // Validate the contents of config_data is what is, expected by:
        // 1.  Reading in the package manifest to get the metafar path
        // 2.  Opening the metafar
        // 3.  Reading the config_data entry's file
        // 4.  Validate the contents of the file

        // 1. Read the config_data package manifest
        let config_data_manifest =
            PackageManifest::try_load_from(expected_config_data_manifest_path).unwrap();
        assert_eq!(config_data_manifest.name().as_ref(), "config-data");

        // and get the metafar path.
        let blobs = config_data_manifest.into_blobs();
        let metafar_blobinfo = blobs.first().unwrap();
        assert_eq!(metafar_blobinfo.path, "meta/");

        // 2. Read the metafar.
        let mut config_data_metafar = File::open(&metafar_blobinfo.source_path).unwrap();
        let mut far_reader = fuchsia_archive::Utf8Reader::new(&mut config_data_metafar).unwrap();

        // 3.  Read the configuration file.
        let config_file_data = far_reader
            .read_file(&format!(
                "meta/data/{}/dest/file/path",
                vars.config_data_target_package_name
            ))
            .unwrap();

        // 4.  Validate its contents.
        assert_eq!(config_file_data, "configuration data".as_bytes());

        // 5.  Read the configuration file from the platform configuration.
        let config_file_data = far_reader
            .read_file(&format!(
                "meta/data/{}/dest/platform/configuration",
                vars.config_data_target_package_name
            ))
            .unwrap();

        // 6.  Validate its contents.
        assert_eq!(config_file_data, "configuration data".as_bytes());
    }

    #[test]
    fn test_builder_with_domain_config() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        let bundle = make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path);
        let mut builder = setup_builder(&vars, vec![bundle]);

        let destination = PackageSetDestination::Blob(PackageDestination::ForTest);
        let config = DomainConfig {
            directories: NamedMap::new("test"),
            name: destination.clone(),
            expose_directories: false,
        };
        builder.add_domain_config(destination, config).unwrap();

        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        // The domain config's manifest is in outdir
        let expected_manifest_path = vars.outdir.join("for-test").join("package_manifest.json");

        // Validate that the base package set contains the domain config.
        assert!(result.base.contains(&expected_manifest_path));
    }

    #[test]
    fn test_builder_with_bootfs_domain_config() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        let bundle = make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path);
        let mut builder = setup_builder(&vars, vec![bundle]);

        let destination = PackageSetDestination::Boot(BootfsPackageDestination::ForTest);
        let config = DomainConfig {
            directories: NamedMap::new("test"),
            name: destination.clone(),
            expose_directories: false,
        };
        builder.add_domain_config(destination, config).unwrap();

        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        // The domain config's manifest is in outdir
        let expected_manifest_path = vars.outdir.join("for-test").join("package_manifest.json");

        // Validate that the bootfs package set contains the domain config.
        assert!(result.bootfs_packages.contains(&expected_manifest_path));
    }

    #[test]
    fn test_builder_with_shell_commands() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        // Make an assembly input bundle with Shell Commands in it
        let mut bundle = make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path);
        bundle.shell_commands.insert(
            "package1".to_string(),
            BTreeSet::from([
                PackageInternalPathBuf::from("bin/binary1"),
                PackageInternalPathBuf::from("bin/binary2"),
            ]),
        );
        let builder = setup_builder(&vars, vec![bundle]);

        let (result, _) =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap();

        // config_data's manifest is in outdir
        let expected_manifest_path =
            vars.outdir.join("pkg-shell-commands").join("package_manifest.json");

        // Validate that the base package set contains shell_commands.
        assert_eq!(result.base.len(), 4);
        assert!(result.base.contains(&expected_manifest_path));
    }

    #[test]
    fn test_builder_with_product_packages_and_config() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let tools = FakeToolProvider::default();

        // Create some config_data source files
        let config_data_source_dir = outdir.join("config_data_source");
        let config_data_source_a = config_data_source_dir.join("cfg.txt");
        let config_data_source_b = config_data_source_dir.join("other.json");
        std::fs::create_dir_all(&config_data_source_dir).unwrap();
        std::fs::write(&config_data_source_a, "source a").unwrap();
        std::fs::write(&config_data_source_b, "{}").unwrap();

        let base_b = write_empty_pkg(outdir, "base_b", None);
        let base_b_path: &Utf8Path = base_b.as_ref();
        let base_b_pathbuf: Utf8PathBuf = base_b_path.into();

        let base_c = write_empty_pkg(outdir, "base_c", None);
        let base_c_path: &Utf8Path = base_c.as_ref();
        let base_c_pathbuf: Utf8PathBuf = base_c_path.into();

        let packages = ProductPackagesConfig {
            base: [
                ("base_a".to_string(), write_empty_pkg(outdir, "base_a", None).into()),
                (
                    "base_b".to_string(),
                    ProductPackageDetails { manifest: base_b_pathbuf, config_data: Vec::default() },
                ),
                (
                    "base_c".to_string(),
                    ProductPackageDetails {
                        manifest: base_c_pathbuf,
                        config_data: vec![
                            ProductConfigData {
                                destination: "dest/path/cfg.txt".into(),
                                source: config_data_source_a.into(),
                            },
                            ProductConfigData {
                                destination: "other_data.json".into(),
                                source: config_data_source_b.into(),
                            },
                        ],
                    },
                ),
            ]
            .into(),
            cache: [
                ("cache_a".to_string(), write_empty_pkg(outdir, "cache_a", None).into()),
                ("cache_b".to_string(), write_empty_pkg(outdir, "cache_b", None).into()),
            ]
            .into(),
            bootfs: [].into(),
        };

        let mut builder = get_minimum_config_builder(
            outdir,
            vec!["platform_a".to_owned(), "platform_b".to_owned()],
        );
        builder.add_product_packages(packages).unwrap();
        let (result, _) = builder.build_and_validate(outdir, &tools, ValidationMode::On).unwrap();

        assert_eq!(
            result.base,
            [
                "base_a",
                "base_b",
                "base_c",
                "config_data/package_manifest.json",
                "platform_a",
                "platform_b",
            ]
            .iter()
            .map(|p| outdir.join(p))
            .collect::<Vec<_>>()
        );
        assert_eq!(result.cache, vec![outdir.join("cache_a"), outdir.join("cache_b")]);

        // Validate product-provided config-data is correct
        let config_data_pkg =
            PackageManifest::try_load_from(outdir.join("config_data/package_manifest.json"))
                .unwrap();
        let metafar_blobinfo = config_data_pkg.blobs().iter().find(|b| b.path == "meta/").unwrap();
        let mut far_reader =
            fuchsia_archive::Utf8Reader::new(File::open(&metafar_blobinfo.source_path).unwrap())
                .unwrap();

        // Assert both config_data files match those written above
        let config_data_a_bytes =
            far_reader.read_file("meta/data/base_c/dest/path/cfg.txt").unwrap();
        let config_data_a = std::str::from_utf8(&config_data_a_bytes).unwrap();
        let config_data_b_bytes = far_reader.read_file("meta/data/base_c/other_data.json").unwrap();
        let config_data_b = std::str::from_utf8(&config_data_b_bytes).unwrap();
        assert_eq!(config_data_a, "source a");
        assert_eq!(config_data_b, "{}");
    }

    #[test]
    fn test_builder_with_compiled_packages() -> Result<()> {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();
        // Write the expected output component files since the component
        // compiler is mocked.
        let component1_dir = vars.outdir.join("for-test/component1");
        let component2_dir = vars.outdir.join("for-test/component2");
        std::fs::create_dir_all(&component1_dir).unwrap();
        std::fs::create_dir_all(&component2_dir).unwrap();
        std::fs::write(component1_dir.join("component1.cm"), "component fake contents").unwrap();
        std::fs::write(component2_dir.join("component2.cm"), "component fake contents").unwrap();
        let bundle2_path = vars.outdir.join("bundle2");

        // Create 2 assembly bundle and add a config_data entry to it.
        let mut bundle1 = make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path);
        bundle1.packages_to_compile.push(CompiledPackageDefinition {
            name: CompiledPackageDestination::Test(ForTest),
            components: vec![
                CompiledComponentDefinition {
                    component_name: "component1".into(),
                    shards: vec![vars.outdir.join("compiled_packages/for_test/component1/cml1")],
                    cmc_features: vec![],
                },
                CompiledComponentDefinition {
                    component_name: "component2".into(),
                    shards: vec![vars.outdir.join("compiled_packages/for_test/component2/cml2")],
                    cmc_features: vec![],
                },
            ],
            contents: Default::default(),
            includes: Default::default(),
            bootfs_package: Default::default(),
        });

        let bundle2 = AssemblyInputBundle {
            packages_to_compile: vec![CompiledPackageDefinition {
                name: CompiledPackageDestination::Test(ForTest),
                components: vec![CompiledComponentDefinition {
                    component_name: "component2".into(),
                    shards: vec![bundle2_path.join("compiled_packages/for_test/component2/shard1")],
                    cmc_features: vec![],
                }],
                contents: Default::default(),
                includes: Default::default(),
                bootfs_package: Default::default(),
            }],
            ..Default::default()
        };

        let builder = setup_builder(&vars, vec![bundle1, bundle2]);
        let _: ImageAssemblyConfig =
            builder.build_and_validate(&vars.outdir, &tools, ValidationMode::On).unwrap().0;

        // Make sure all the components and CML shards from the separate bundles
        // are merged.
        let expected_commands: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
                {
                    "tool": "./host_x64/cmc",
                    "args": [
                        "merge",
                         "--output",
                          vars.outdir.join("for-test/component1/component1.cml").as_str(),
                          vars.outdir.join("compiled_packages/for_test/component1/cml1").as_str()
                    ]
                },
                {
                    "tool": "./host_x64/cmc",
                    "args": [
                        "compile",
                        "--features=allow_long_names",
                        "--config-package-path",
                        "meta/component1.cvf",
                        "-o",
                        vars.outdir.join("for-test/component1/component1.cm").as_str(),
                        vars.outdir.join("for-test/component1/component1.cml").as_str()
                    ]
                },
                {
                    "tool": "./host_x64/cmc",
                    "args": [
                        "merge",
                        "--output",
                        vars.outdir.join("for-test/component2/component2.cml").as_str(),
                        vars.outdir.join("compiled_packages/for_test/component2/cml2").as_str(),
                        bundle2_path.join("compiled_packages/for_test/component2/shard1")
                    ]
                },
                {
                    "tool": "./host_x64/cmc",
                    "args": [
                        "compile",
                        "--features=allow_long_names",
                        "--config-package-path",
                        "meta/component2.cvf",
                        "-o",
                        vars.outdir.join("for-test/component2/component2.cm").as_str(),
                        vars.outdir.join("for-test/component2/component2.cml").as_str()
                    ]
                }
            ]
        }))
        .unwrap();
        assert_eq!(&expected_commands, tools.log());

        Ok(())
    }

    #[test]
    fn test_builder_with_product_packages_catches_duplicates() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        let packages = ProductPackagesConfig {
            base: [("base_a".to_string(), write_empty_pkg(outdir, "base_a", None).into())].into(),
            ..ProductPackagesConfig::default()
        };
        let mut builder = get_minimum_config_builder(outdir, vec!["base_a".to_owned()]);

        let result = builder.add_product_packages(packages);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_builder_with_product_drivers_catches_duplicates() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        let base_driver_1 = make_test_driver("driver1", outdir)?;
        let mut builder = get_minimum_config_builder(outdir, vec!["driver1".to_owned()]);

        let result = builder.add_product_base_drivers(vec![base_driver_1]);

        assert!(result.is_err());
        Ok(())
    }

    /// Helper to duplicate the first item in an Vec<T: Clone> and make it also
    /// the last item. This intentionally panics if the Vec is empty.
    fn duplicate_first<T: Clone>(vec: &mut Vec<T>) {
        vec.push(vec.first().unwrap().clone());
    }

    #[test]
    fn test_builder_catches_dupe_pkgs_in_aib() {
        let temp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();

        let mut aib = make_test_assembly_bundle_with_base(root, root);
        duplicate_first(&mut aib.packages);

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        assert!(builder.add_parsed_bundle(root, aib).is_err());
    }

    fn test_duplicates_across_aibs_impl<
        T: Clone,
        F: Fn(&mut AssemblyInputBundle) -> &mut Vec<T>,
    >(
        accessor: F,
    ) {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        let mut aib = make_test_assembly_bundle_with_base(outdir, outdir);
        let mut second_aib = AssemblyInputBundle::default();

        let first_list = (accessor)(&mut aib);
        let second_list = (accessor)(&mut second_aib);

        // Clone the first item in the first AIB into the same list in the
        // second AIB to create a duplicate item across the two AIBs.
        let value = first_list.first().unwrap();
        second_list.push(value.clone());

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder.add_parsed_bundle(outdir, aib).unwrap();
        assert!(builder.add_parsed_bundle(outdir.join("second"), second_aib).is_err());
    }

    #[test]
    fn test_builder_catches_dupe_pkgs_across_aibs() {
        test_duplicates_across_aibs_impl(|a| &mut a.packages);
    }

    fn assert_two_pkgs_same_name_diff_path_errors() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let tmp_path1 = TempDir::new_in(outdir).unwrap();
        let dir_path1 = Utf8Path::from_path(tmp_path1.path()).unwrap();
        let tmp_path2 = TempDir::new_in(outdir).unwrap();
        let dir_path2 = Utf8Path::from_path(tmp_path2.path()).unwrap();
        let aib = AssemblyInputBundle {
            packages: vec![
                PackageDetails {
                    package: write_empty_pkg(dir_path1, "base_package2", None).to_string().into(),
                    set: PackageSet::Base,
                },
                PackageDetails {
                    package: write_empty_pkg(dir_path2, "base_package2", None).to_string().into(),
                    set: PackageSet::Base,
                },
            ],
            ..Default::default()
        };
        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        assert!(builder.add_parsed_bundle(outdir, aib).is_err());
    }

    #[test]
    /// Asserts that attempting to add a package to the base package set with the same
    /// PackageName but a different package manifest path will result in an error if coming
    /// from the same AIB
    fn test_builder_catches_same_name_diff_path_one_aib() {
        assert_two_pkgs_same_name_diff_path_errors();
    }

    fn assert_two_pkgs_same_name_diff_path_across_aibs_errors() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let tmp_path1 = TempDir::new_in(outdir).unwrap();
        let dir_path1 = Utf8Path::from_path(tmp_path1.path()).unwrap();
        let tmp_path2 = TempDir::new_in(outdir).unwrap();
        let dir_path2 = Utf8Path::from_path(tmp_path2.path()).unwrap();
        let tools = FakeToolProvider::default();
        let aib = AssemblyInputBundle {
            packages: vec![PackageDetails {
                package: write_empty_pkg(dir_path1, "base_package2", None).to_string().into(),
                set: PackageSet::Base,
            }],
            ..Default::default()
        };

        let aib2 = AssemblyInputBundle {
            packages: vec![PackageDetails {
                package: write_empty_pkg(dir_path2, "base_package2", None).to_string().into(),
                set: PackageSet::Base,
            }],
            ..Default::default()
        };

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder.add_parsed_bundle(outdir, aib).ok();
        builder.add_parsed_bundle(outdir, aib2).ok();
        assert!(builder.build_and_validate(outdir, &tools, ValidationMode::On).is_err());
    }
    /// Asserts that attempting to add a package to the base package set with the same
    /// PackageName but a different package manifest path will result in an error if coming
    /// from DIFFERENT AIBs
    #[test]
    fn test_builder_catches_same_name_diff_path_multi_aib() {
        assert_two_pkgs_same_name_diff_path_across_aibs_errors();
    }

    #[test]
    fn test_builder_catches_dupes_across_package_sets() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let tmp_path1 = TempDir::new_in(outdir).unwrap();
        let dir_path1 = Utf8Path::from_path(tmp_path1.path()).unwrap();
        let tmp_path2 = TempDir::new_in(outdir).unwrap();
        let dir_path2 = Utf8Path::from_path(tmp_path2.path()).unwrap();
        let aib = AssemblyInputBundle {
            packages: vec![PackageDetails {
                package: write_empty_pkg(dir_path1, "foo", None).to_string().into(),
                set: PackageSet::Base,
            }],
            ..Default::default()
        };

        let aib2 = AssemblyInputBundle {
            packages: vec![PackageDetails {
                package: write_empty_pkg(dir_path2, "foo", None).to_string().into(),
                set: PackageSet::Cache,
            }],
            ..Default::default()
        };
        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder.add_parsed_bundle(outdir, aib).ok();
        assert!(builder.add_parsed_bundle(outdir, aib2).is_err());
    }

    #[test]
    fn test_builder_catches_dupe_config_data_across_aibs() {
        let temp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();

        let mut first_aib = make_test_assembly_bundle_with_base(root, root);
        let mut second_aib = AssemblyInputBundle::default();

        // Write the config data files.
        std::fs::create_dir(root.join("second")).unwrap();

        let config = root.join("config_data");
        let mut f = File::create(&config).unwrap();
        write!(&mut f, "config_data").unwrap();

        let config = root.join("second/config_data");
        let mut f = File::create(&config).unwrap();
        write!(&mut f, "config_data2").unwrap();

        let config_data_file_entry =
            FileEntry { source: "config_data".into(), destination: "dest/file/path".into() };

        first_aib.config_data.insert("base_package0".into(), vec![config_data_file_entry.clone()]);
        second_aib.config_data.insert("base_package0".into(), vec![config_data_file_entry]);

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );
        builder.add_parsed_bundle(root, first_aib).unwrap();
        assert!(builder.add_parsed_bundle(root.join("second"), second_aib).is_err());
    }

    #[test]
    fn test_builder_catches_kernel_arg_duplicates() {
        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );

        builder.add_kernel_args(vec!["arg1=value1".to_owned()]).unwrap();
        let result = builder.add_kernel_args(vec!["arg1=value2".to_owned()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_kernel_arg_overrides() {
        let vars = TempdirPathsForTest::new();
        let tools = FakeToolProvider::default();

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );

        // Provide developer overrides for kernel commandline args
        builder
            .add_developer_overrides(DeveloperOverrides {
                kernel: KernelOptions {
                    command_line_args: vec![
                        "arg1=override_value_1".to_owned(),
                        "arg2=override=value=2".to_owned(),
                        "some_other_arg".to_owned(),
                    ],
                },
                ..Default::default()
            })
            .unwrap();

        // Simulate the addition of kernel args from AIBs and platform_configuration:
        builder
            .add_kernel_args(vec![
                "arg1=original_value1".to_owned(),
                "arg2=original_value2".to_owned(),
                "another_arg".to_owned(),
                "and_another_arg".to_owned(),
            ])
            .unwrap();

        builder
            .add_parsed_bundle(
                &vars.outdir,
                make_test_assembly_bundle_with_base(&vars.outdir, &vars.bundle_path),
            )
            .unwrap();

        let config = builder.build_and_validate(vars.outdir, &tools, ValidationMode::On).unwrap().0;

        assert_eq!(
            config.kernel.args,
            vec![
                "and_another_arg".to_owned(),
                "another_arg".to_owned(),
                "arg1=override_value_1".to_owned(),
                "arg2=override=value=2".to_owned(),
                "kernel_arg0".to_owned(), // from the test assembly bundle
                "some_other_arg".to_owned(),
            ]
        );
    }

    #[test]
    fn test_developer_overrides_drivers() {
        let vars = TempdirPathsForTest::new();

        let mut builder = ImageAssemblyConfigBuilder::new(
            BuildType::Eng,
            FeatureSetLevel::Standard,
            "my_board".into(),
            None::<Utf8PathBuf>,
            FilesystemImageMode::default(),
            AssemblyMode::default(),
            SystemReleaseInfo::new_for_testing(),
        );

        let driver_path_base = write_empty_pkg(&vars.outdir, "my_driver_base", None);
        let driver_path_boot = write_empty_pkg(&vars.outdir, "my_driver_boot", None);
        builder
            .add_developer_overrides(DeveloperOverrides {
                drivers: vec![
                    PackagedDriverDetails {
                        package: driver_path_base.clone().into(),
                        set: PackageSet::Base,
                        components: vec!["meta/my_driver_base.cm".into()],
                    },
                    PackagedDriverDetails {
                        package: driver_path_boot.clone().into(),
                        set: PackageSet::Bootfs,
                        components: vec!["meta/my_driver_boot.cm".into()],
                    },
                ],
                ..Default::default()
            })
            .unwrap();

        assert!(builder.base_drivers.contains_key("fuchsia-pkg://fuchsia.com/my_driver_base"));
        assert!(builder.boot_drivers.contains_key("fuchsia-boot:///my_driver_boot"));
    }

    #[test]
    fn test_validate_cache_package_environments() {
        let vars = TempdirPathsForTest::new();

        // 1. Create a cache package
        let cache_pkg_path = write_empty_pkg(&vars.outdir, "my_cache_pkg", None);

        // 2. Create a core package containing core.cm with a child pointing to my_cache_pkg without environment
        let mut core_pkg_builder = PackageBuilder::new_platform_internal_package("core");
        let core_manifest_path = vars.outdir.join("core");
        core_pkg_builder.manifest_path(&core_manifest_path);
        core_pkg_builder.repository("fuchsia.com");

        let core_cm_decl = Component {
            children: Some(vec![fidl_fuchsia_component_decl::Child {
                name: Some("my_cache_child".into()),
                url: Some("fuchsia-pkg://fuchsia.com/my_cache_pkg#meta/my_cache_pkg.cm".into()),
                startup: Some(fidl_fuchsia_component_decl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            }]),
            ..Default::default()
        };
        let core_cm_bytes = fidl::persist(&core_cm_decl).unwrap();
        let core_cm_path = vars.outdir.join("core.cm");
        std::fs::write(&core_cm_path, &core_cm_bytes).unwrap();
        core_pkg_builder.add_file_to_far("meta/core.cm", &core_cm_path).unwrap();
        core_pkg_builder.build(&vars.outdir, vars.outdir.join("core_meta.far")).unwrap();

        let product_failing = ImageAssemblyConfig {
            base: vec![core_manifest_path.clone().into()],
            cache: vec![cache_pkg_path.clone().into()],
            system: vec![],
            anchored_automatic: vec![],
            anchored_on_demand: vec![],
            bootfs_packages: vec![],
            on_demand: vec![],
            kernel: KernelConfig { path: vars.outdir.join("kernel"), args: vec![] },
            qemu_kernel: vars.outdir.join("qemu_kernel"),
            boot_args: vec![],
            bootfs_files: vec![],
            images_config: Default::default(),
            board_name: "test_board".into(),
            partitions_config: None,
            board_driver_arguments: Default::default(),
            zbi_extra_items: None,
            devicetree: None,
            devicetree_overlay: None,
            system_release_info: Default::default(),
            image_mode: FilesystemImageMode::default(),
            build_type: BuildType::Eng,
        };

        let result = validate_cache_package_environments(&product_failing);
        assert!(result.is_err(), "Expected error for cache package without environment");
        let errs = result.unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("my_cache_child"));
        assert!(errs[0].contains("my_cache_pkg"));

        // 3. Now create a core package where child HAS an environment
        let mut core_pkg_builder_ok = PackageBuilder::new_platform_internal_package("core");
        let core_manifest_path_ok = vars.outdir.join("core_ok");
        core_pkg_builder_ok.manifest_path(&core_manifest_path_ok);
        core_pkg_builder_ok.repository("fuchsia.com");

        let core_cm_decl_ok = Component {
            children: Some(vec![fidl_fuchsia_component_decl::Child {
                name: Some("my_cache_child".into()),
                url: Some("fuchsia-pkg://fuchsia.com/my_cache_pkg#meta/my_cache_pkg.cm".into()),
                startup: Some(fidl_fuchsia_component_decl::StartupMode::Lazy),
                environment: Some("#core-env".into()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let core_cm_bytes_ok = fidl::persist(&core_cm_decl_ok).unwrap();
        let core_cm_path_ok = vars.outdir.join("core_ok.cm");
        std::fs::write(&core_cm_path_ok, &core_cm_bytes_ok).unwrap();
        core_pkg_builder_ok.add_file_to_far("meta/core.cm", &core_cm_path_ok).unwrap();
        core_pkg_builder_ok.build(&vars.outdir, vars.outdir.join("core_ok_meta.far")).unwrap();

        let product_passing = ImageAssemblyConfig {
            base: vec![core_manifest_path_ok.into()],
            cache: vec![cache_pkg_path.into()],
            system: vec![],
            anchored_automatic: vec![],
            anchored_on_demand: vec![],
            bootfs_packages: vec![],
            on_demand: vec![],
            kernel: KernelConfig { path: vars.outdir.join("kernel"), args: vec![] },
            qemu_kernel: vars.outdir.join("qemu_kernel"),
            boot_args: vec![],
            bootfs_files: vec![],
            images_config: Default::default(),
            board_name: "test_board".into(),
            partitions_config: None,
            board_driver_arguments: Default::default(),
            zbi_extra_items: None,
            devicetree: None,
            devicetree_overlay: None,
            system_release_info: Default::default(),
            image_mode: FilesystemImageMode::default(),
            build_type: BuildType::Eng,
        };

        assert!(validate_cache_package_environments(&product_passing).is_ok());
    }
}
