// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::image_assembly_config_builder::ImageAssemblyConfigBuilder;
use assembly_cli_args::{AssemblyMode, ValidationMode};

use anyhow::{Context, Result, bail};
use assembly_config_schema::developer_overrides::{DeveloperOnlyOptions, DeveloperOverrides};
use assembly_config_schema::{
    BoardConfig, BoardInputBundle, BuildType, FeatureSetLevel, ProductConfig,
};
use assembly_platform_artifacts::PlatformArtifacts;
use assembly_release_info::SystemReleaseInfo;

use assembly_config_schema::assembly_input_bundle::{
    CompiledComponentDefinition, CompiledPackageDefinition,
};
use assembly_constants::{BlobfsCompiledPackageDestination, CompiledPackageDestination};
use assembly_container::AssemblyContainer;
use assembly_images_config::{FilesystemImageMode, ImagesConfig};
use assembly_tool::ToolProvider;
use camino::{Utf8Path, Utf8PathBuf};
use image_assembly_config::ImageAssemblyConfig;

pub struct ProductAssembly {
    builder: ImageAssemblyConfigBuilder,
    platform_artifacts: PlatformArtifacts,
    product_config: ProductConfig,
    board_config: BoardConfig,
    developer_only_options: Option<DeveloperOnlyOptions>,
    kernel_aib: Utf8PathBuf,
    boot_shim_aib: Utf8PathBuf,
    validation_mode: ValidationMode,
    builder_forensics_file_path: Option<Utf8PathBuf>,
    board_forensics_file_path: Option<Utf8PathBuf>,
    include_example_aib_for_tests: bool,
    mode: AssemblyMode,
}

impl ProductAssembly {
    pub fn new(
        platform_artifacts: PlatformArtifacts,
        mut product_config: ProductConfig,
        mut board_config: BoardConfig,
        include_example_aib_for_tests: bool,
        mode: AssemblyMode,
        developer_overrides: Option<DeveloperOverrides>,
    ) -> Result<Self> {
        // Apply developer overrides first to ensure builder is constructed with all overrides
        let (developer_only_options, builder_overrides) =
            if let Some(overrides) = developer_overrides {
                // Apply the platform and product overrides.
                let product_config_overrides = serde_json::json!({
                    "platform": overrides.platform,
                    "product": overrides.product,
                });
                product_config = product_config
                    .apply_overrides(product_config_overrides)
                    .context("Merging developer overrides into product configuration")?;

                // Apply the board overrides.
                board_config = board_config
                    .apply_overrides(overrides.board)
                    .context("Merging developer overrides into board configuration")?;

                // Reconstitute the developer overrides struct, but with a null platform and product
                // configs, since they've been used to modify the main platform and product
                // configurations.
                let builder_overrides = DeveloperOverrides {
                    platform: serde_json::Value::Null,
                    product: serde_json::Value::Null,
                    board: serde_json::Value::Null,
                    ..overrides
                };

                // If the developer overrides specifies `netboot_mode`, then we override
                // the image mode to 'ramdisk'.
                if builder_overrides.developer_only_options.netboot_mode {
                    product_config.platform.storage.filesystems.image_mode =
                        FilesystemImageMode::Ramdisk;
                }

                (Some(builder_overrides.developer_only_options.clone()), Some(builder_overrides))
            } else {
                (None, None)
            };

        let image_mode = product_config.platform.storage.filesystems.image_mode;
        let mut builder = ImageAssemblyConfigBuilder::new(
            product_config.platform.build_type,
            product_config.platform.feature_set_level,
            board_config.name.clone(),
            board_config.partitions_config.as_ref().map(|p| p.as_utf8_path_buf().clone()),
            image_mode,
            mode,
            SystemReleaseInfo {
                platform: platform_artifacts.release_info.clone(),
                product: product_config.product.release_info.clone(),
                board: board_config.release_info.clone(),
            },
        );

        // Add remaining overrides to constructed builder
        if let Some(builder_overrides) = builder_overrides {
            builder
                .add_developer_overrides(builder_overrides)
                .context("Setting developer overrides")?;
        }

        let kernel_name = match product_config.platform.build_type {
            BuildType::Eng => "zircon_eng",
            BuildType::User => "zircon_user",
            BuildType::UserDebug => "zircon_userdebug",
        };
        let kernel_aib = platform_artifacts.get_bundle(&kernel_name);
        // The emulator support bundle is always added, even to an empty build.
        // The emulator support bundle contains only a QEMU boot shim.
        // Kernel tests can customize this bundle to provide an alternate boot shim.
        //
        // TODO(https://fxbug.dev/408223995): Determine whether we want to expose alternate boot shims via the platform
        // and refactor this if we decide to.
        let boot_shim_aib = platform_artifacts.get_bundle("emulator_support");
        Ok(Self {
            builder,
            platform_artifacts,
            product_config,
            board_config,
            developer_only_options,
            kernel_aib,
            boot_shim_aib,
            validation_mode: ValidationMode::On,
            builder_forensics_file_path: None,
            board_forensics_file_path: None,
            include_example_aib_for_tests,
            mode,
        })
    }

    pub fn set_kernel_aib(&mut self, path: Utf8PathBuf) {
        self.kernel_aib = path;
    }

    pub fn set_boot_shim_aib(&mut self, path: Utf8PathBuf) -> Result<()> {
        if !self.mode.is_test_kernel() {
            bail!("A custom boot shim can only be set with --mode `test-ramdisk` or `test-zbi`");
        }
        self.boot_shim_aib = path;
        Ok(())
    }

    pub fn set_validation_mode(&mut self, validation_mode: ValidationMode) {
        self.validation_mode = validation_mode;
    }

    pub fn write_forensics_files(
        &mut self,
        builder_forensics_file_path: Utf8PathBuf,
        board_forensics_file_path: Utf8PathBuf,
    ) {
        self.builder_forensics_file_path = Some(builder_forensics_file_path);
        self.board_forensics_file_path = Some(board_forensics_file_path);
    }

    pub fn build(
        self,
        tools: &impl ToolProvider,
        outdir: impl AsRef<Utf8Path>,
    ) -> Result<ImageAssemblyConfig> {
        let platform = self.product_config.platform;
        let product = self.product_config.product;
        let board_config = self.board_config;
        let mut builder = self.builder;
        let include_example_aib_for_tests = self.include_example_aib_for_tests;

        builder
            .add_bundle(&self.kernel_aib)
            .with_context(|| format!("Adding kernel ({})", self.kernel_aib))?;
        builder
            .add_bundle(&self.boot_shim_aib)
            .with_context(|| format!("Adding boot shim ({})", self.boot_shim_aib))?;

        // Parse the board's Board Input Bundles, if it has them, and merge their
        // configuration fields into that of the board_config struct.
        let mut board_input_bundles = Vec::new();
        for bundle_path in board_config.input_bundles.values() {
            let bundle = BoardInputBundle::from_dir(&bundle_path)
                .with_context(|| format!("Reading board input bundle: {bundle_path}"))?;
            if bundle.should_be_included(platform.build_type) {
                board_input_bundles.push((bundle_path.clone(), bundle));
            }
        }
        let board_input_bundles = board_input_bundles;

        // Find the Board Input Bundle that's providing the configuration files, by first finding _all_
        // structs that aren't None, and then verifying that we only have one of them.  This is perhaps
        // more complicated than strictly necessary to get that struct, because it collects all paths to
        // the bundles that are providing a Some() value, and reporting them all in the error.
        let board_configuration_files = board_input_bundles
            .iter()
            .filter_map(|(path, bib)| bib.configuration.as_ref().map(|cfg| (path, cfg)))
            .collect::<Vec<_>>();

        let board_provided_config = if board_configuration_files.len() > 1 {
            let paths = board_configuration_files
                .iter()
                .map(|(path, _)| format!("  - {path}"))
                .collect::<Vec<_>>();
            let paths = paths.join("\n");
            bail!("Only one board input bundle can provide configuration files, found: \n{paths}");
        } else {
            board_configuration_files.first().map(|(_, cfg)| (*cfg).clone()).unwrap_or_default()
        };

        // Replace board_config with a new one that swaps its empty 'configuraton' field
        // for the consolidated one created from the board's input bundles.
        let board_config = BoardConfig { configuration: board_provided_config, ..board_config };

        // Get platform configuration based on the ProductConfig and the BoardConfig.
        let resource_dir = self.platform_artifacts.get_resources();
        let configuration = assembly_platform_configuration::define_configuration(
            self.platform_artifacts.to_owned(),
            &platform,
            &product,
            &board_config,
            &outdir,
            &resource_dir,
            self.developer_only_options.as_ref(),
            include_example_aib_for_tests,
            &self.mode,
        )?;

        // Set the info used for BoardDriver arguments.
        if !self.mode.is_test_kernel() {
            builder
                .set_board_driver_arguments(&board_config)
                .context("Setting arguments for the Board Driver")?;
        }

        // Set the configuration for the rest of the packages.
        for (package, config) in configuration.package_configs {
            builder.set_package_config(package, config)?;
        }

        // Add the kernel cmdline arguments
        builder.add_kernel_args(configuration.kernel_args)?;

        // Add the domain config packages.
        for (package, config) in configuration.domain_configs {
            builder.add_domain_config(package, config)?;
        }

        // Add the configuration capabilities.
        builder.add_configuration_capabilities(configuration.configuration_capabilities)?;

        if !self.mode.is_test_kernel() {
            // Add the board's Board Input Bundles, if it has them.
            for (bundle_path, bundle) in board_input_bundles {
                builder
                    .add_board_input_bundle(
                        bundle,
                        platform.feature_set_level == FeatureSetLevel::Bootstrap
                            || platform.feature_set_level == FeatureSetLevel::Embeddable,
                    )
                    .with_context(|| format!("Adding board input bundle from: {bundle_path}"))?;
            }
            // Add the product's Product Input Bundles, if it has them.
            for bundle in self.product_config.product_input_bundles.values() {
                builder.add_product_input_bundle(bundle).with_context(|| {
                    format!("Adding product input bundle: {}", bundle.release_info.name)
                })?;
            }
        }

        // Add the platform Assembly Input Bundles that were chosen by the configuration.
        for platform_bundle_name in &configuration.bundles {
            let platform_bundle_path = self.platform_artifacts.get_bundle(platform_bundle_name);
            builder.add_bundle(&platform_bundle_path).with_context(|| {
                format!("Adding platform bundle {platform_bundle_name} ({platform_bundle_path})")
            })?;
        }

        // Add the core shards.
        if !configuration.core_shards.is_empty() {
            let compiled_package_def: CompiledPackageDefinition = CompiledPackageDefinition {
                name: CompiledPackageDestination::Blob(BlobfsCompiledPackageDestination::Core),
                components: vec![CompiledComponentDefinition {
                    component_name: "core".to_string(),
                    shards: configuration.core_shards.iter().map(Into::into).collect(),
                    cmc_features: vec![],
                }],
                contents: Default::default(),
                includes: Default::default(),
                bootfs_package: Default::default(),
            };
            builder
                .add_compiled_package(&compiled_package_def, "".into())
                .context("Adding core shards")?;
        }

        // Add the bootfs files.
        builder.add_bootfs_files(&configuration.bootfs.files).context("Adding bootfs files")?;

        // Add product-specified packages and configuration
        if product.bootfs_files_package.is_some() {
            let embeddable_or_bootstrap = matches!(
                platform.feature_set_level,
                FeatureSetLevel::Embeddable | FeatureSetLevel::Bootstrap
            );
            if self.mode.is_default() && !embeddable_or_bootstrap {
                bail!(
                    "bootfs files can only be added to the 'embeddable' or 'bootstrap' feature set levels"
                );
            }
        }

        // Add product-specified bootfs files, if present
        if let Some(bootfs_files_package) = product.bootfs_files_package.as_ref() {
            builder
                .add_bootfs_files_package(bootfs_files_package, true)
                .context("Adding product-specified bootfs files")?;
        }

        // Add product-specified packages
        builder
            .add_product_packages(product.packages)
            .context("Adding product-provided packages")?;

        // Add product-specified memory buckets.
        if let Some(buckets) = platform.diagnostics.memory_monitor.buckets {
            builder.add_memory_buckets(&vec![buckets.into()])?;
        }

        // Add any packages compiled by the assembly process itself
        for package in configuration.compiled_packages.values() {
            builder
                .add_compiled_package(package, "".into())
                .context("adding configuration-generated package")?;
        }

        builder
            .add_product_base_drivers(product.base_drivers)
            .context("Adding product-provided base-drivers")?;

        // Add ZBI extra items.
        if let Some(zbi_extra_items_path) = &board_config.zbi_extra_items {
            builder
                .add_zbi_extra_items(zbi_extra_items_path)
                .context("Adding ZBI extra items image")?;
        }

        // Add devicetree binary
        if let Some(devicetree_path) = &board_config.devicetree {
            builder.add_devicetree(devicetree_path).context("Adding devicetree binary")?;
        }
        if let Some(devicetree_overlay_path) = &board_config.devicetree_overlay {
            builder
                .add_devicetree_overlay(devicetree_overlay_path)
                .context("Adding devicetree binary overlay")?;
        }

        // Construct and set the images config
        builder
            .set_images_config(
                ImagesConfig::from_product_and_board(
                    &platform.storage.filesystems,
                    &board_config.filesystems,
                )
                .context("Constructing images config")?,
            )
            .context("Setting images configuration.")?;

        if let Some(builder_forensics_file_path) = &self.builder_forensics_file_path {
            if let Some(parent_dir) = builder_forensics_file_path.parent() {
                std::fs::create_dir_all(parent_dir)
                    .with_context(|| format!("unable to create outdir: {parent_dir}"))?;
            }
            let builder_forensics_file = std::fs::File::create(&builder_forensics_file_path)
                .with_context(|| {
                    format!(
                        "Failed to create builder forensics files: {builder_forensics_file_path}"
                    )
                })?;
            serde_json::to_writer_pretty(builder_forensics_file, &builder).with_context(|| {
                format!("Writing builder forensics file to: {builder_forensics_file_path}")
            })?;
        }
        if let Some(board_forensics_file_path) = &self.board_forensics_file_path {
            let board_forensics_file = std::fs::File::create(&board_forensics_file_path)
                .with_context(|| {
                    format!("Failed to create builder forensics files: {board_forensics_file_path}")
                })?;
            serde_json::to_writer_pretty(board_forensics_file, &board_config).with_context(
                || format!("Writing board forensics file to: {board_forensics_file_path}"),
            )?;
        }

        let (image_assembly_config, validation_error) =
            builder.build_and_validate(&outdir, tools, self.validation_mode)?;
        if let Some(validation_error) = validation_error {
            return Err(validation_error.into());
        }
        Ok(image_assembly_config)
    }
}
