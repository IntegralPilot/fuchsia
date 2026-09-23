// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A tool for generating assembly configs.

#![deny(missing_docs)]

mod board_config;
mod board_input_bundle;
mod board_input_bundle_set;
mod partitions_config;
mod platform_artifacts;
mod product_config;
mod product_input_bundle;

use anyhow::Result;
use argh::FromArgs;
use assembly_config_schema::IncludeInBuildType;
use camino::Utf8PathBuf;

/// Arguments to construct an assembly config.
#[derive(FromArgs)]
struct Args {
    /// which assembly config to generate.
    #[argh(subcommand)]
    command: Subcommand,
}

#[derive(FromArgs)]
#[argh(subcommand)]
#[allow(clippy::large_enum_variant)]
enum Subcommand {
    Generate(GenerateCommand),
    Extract(ExtractCommand),
}

/// An intermediate pass-through struct to manage the `generate` subcommand
#[derive(FromArgs)]
#[argh(subcommand, name = "generate")]
struct GenerateCommand {
    #[argh(subcommand)]
    pub subcommand: GenerateSubcommand,
}

/// Subcommands to generate a specific assembly config.
#[derive(FromArgs)]
#[argh(subcommand)]
#[allow(clippy::large_enum_variant)]
enum GenerateSubcommand {
    /// generate a product config.
    Product(ProductArgs),

    /// generate a product input bundle.
    ProductInputBundle(ProductInputBundleArgs),

    /// generate a product config using an input product config as a template.
    HybridProduct(HybridProductArgs),

    /// generate a board input bundle.
    BoardInputBundle(BoardInputBundleArgs),

    /// generate a board input bundle set.
    BoardInputBundleSet(BoardInputBundleSetArgs),

    /// generate a board config.
    Board(BoardArgs),

    /// generate a board config using an input board config as a template.
    HybridBoard(HybridBoardArgs),

    /// generate a partitions config.
    Partitions(PartitionsArgs),

    /// generate a platform artifacts directory.
    PlatformArtifacts(PlatformArtifactsArgs),
}

/// An intermediate pass-through struct to manage the `extract` subcommand
#[derive(FromArgs)]
#[argh(subcommand, name = "extract")]
struct ExtractCommand {
    #[argh(subcommand)]
    pub subcommand: ExtractSubcommand,
}

/// Subcommands to extract artifacts from an assembly config.
#[derive(FromArgs)]
#[argh(subcommand)]
#[allow(dead_code)]
enum ExtractSubcommand {
    /// extract a package from an existing product config.
    ProductPackage(ExtractProductPackageArgs),
}

/// Arguments to generate a product config.
#[derive(FromArgs)]
#[argh(subcommand, name = "product")]
struct ProductArgs {
    /// the input product config with absolute paths.
    #[argh(option)]
    config: Utf8PathBuf,

    /// release version that this product config corresponds to.
    #[argh(option)]
    version: Option<String>,

    /// path to a file containing the release version that this
    /// product config matches.
    #[argh(option)]
    version_file: Option<Utf8PathBuf>,

    /// name of repository where this product is released.
    #[argh(option)]
    repo: Option<String>,

    /// path to a file containing the repository information.
    #[argh(option)]
    repo_file: Option<Utf8PathBuf>,

    /// paths to product input bundles to include.
    #[argh(option)]
    product_input_bundles: Vec<Utf8PathBuf>,

    /// the directory to write the product config to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a product input bundle.
#[derive(FromArgs)]
#[argh(subcommand, name = "product-input-bundle")]
struct ProductInputBundleArgs {
    /// the name of the product input bundle.
    #[argh(option)]
    name: String,

    /// a set of base package manifests to include.
    #[argh(option)]
    base_packages: Vec<Utf8PathBuf>,

    /// a set of cache package manifests to include.
    #[argh(option)]
    cache_packages: Vec<Utf8PathBuf>,

    /// a set of flexible package manifests to include.
    #[argh(option)]
    flexible_packages: Vec<Utf8PathBuf>,

    /// a set of package manifests to include that will only be included in the
    /// final images if a product config references it.
    #[argh(option)]
    packages_for_product_config: Vec<Utf8PathBuf>,

    /// the directory to write the product input bundle to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// release version that this BIB corresponds to.
    #[argh(option)]
    version: Option<String>,

    /// path to a file containing the release version that this BIB matches.
    #[argh(option)]
    version_file: Option<Utf8PathBuf>,

    /// name of repository where this board is released.
    #[argh(option)]
    repo: Option<String>,

    /// path to a file containing the repository information.
    #[argh(option)]
    repo_file: Option<Utf8PathBuf>,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a hybrid product config.
#[derive(FromArgs)]
#[argh(subcommand, name = "hybrid-product")]
struct HybridProductArgs {
    /// the input product config directory.
    #[argh(option)]
    input: Utf8PathBuf,

    /// a package to replace in the input.
    #[argh(option)]
    replace_package: Vec<Utf8PathBuf>,

    /// a product input bundle to replace in the input.
    #[argh(option)]
    product_input_bundles: Vec<Utf8PathBuf>,

    /// the directory to write the product config to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a board input bundle.
#[derive(FromArgs)]
#[argh(subcommand, name = "board-input-bundle")]
struct BoardInputBundleArgs {
    /// the name of the board input bundle.
    #[argh(option)]
    name: String,

    /// which build types to include this BIB.
    #[argh(option)]
    include_in: IncludeInBuildType,

    /// the directory to write the board input bundle to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// the path to the file that describes all the drivers to add to the bundle.
    /// The format of this file is a json list of dictionaries that specify the
    /// following fields:
    /// 1) 'package': The path to the package manifest
    /// 2) 'set': The package set that it belongs to ("bootfs" or "base")
    /// 3) 'components': A list of the driver components in this package.
    #[argh(option)]
    drivers: Option<Utf8PathBuf>,

    /// the paths to package manifests for all packages to add to the base
    /// package set.
    #[argh(option)]
    base_packages: Vec<Utf8PathBuf>,

    /// the paths to package manifests for all packages to add to the bootfs
    /// package set.
    #[argh(option)]
    bootfs_packages: Vec<Utf8PathBuf>,

    /// the paths to package manifests for all packages to add to the bootfs-or-base package set.
    #[argh(option)]
    bootfs_or_base_packages: Vec<Utf8PathBuf>,

    /// cpu-manager configuration
    #[argh(option)]
    cpu_manager_config: Option<Utf8PathBuf>,

    /// energy model configuration for processor power management
    #[argh(option)]
    energy_model_config: Option<Utf8PathBuf>,

    /// arguments to pass to the kernel on boot
    #[argh(option)]
    kernel_boot_args: Vec<String>,

    /// battery-manager configuration
    #[argh(option)]
    battery_manager_config: Option<Utf8PathBuf>,

    /// power-manager configuration
    #[argh(option)]
    power_manager_config: Option<Utf8PathBuf>,

    /// power metrics recorder configuration
    #[argh(option)]
    power_metrics_recorder_config: Option<Utf8PathBuf>,

    /// system power modes configuration
    #[argh(option)]
    system_power_mode_config: Option<Utf8PathBuf>,

    /// thermal management configuration
    #[argh(option)]
    thermal_config: Option<Utf8PathBuf>,

    /// thread role configuration files
    #[argh(option)]
    thread_roles: Vec<Utf8PathBuf>,

    /// sysmem format costs configuration files
    ///
    /// Each file's content bytes are a persistent fidl
    /// fuchsia.sysmem2.FormatCosts. Normally json[5] would be preferable for
    /// config, but we generate this config in rust using FIDL types (to avoid
    /// repetition and to take advantage of FIDL rust codegen), and there's no
    /// json schema for FIDL types.
    #[argh(option)]
    sysmem_format_costs_config: Vec<Utf8PathBuf>,

    /// triage-detect configs for board-specific drivers.
    #[argh(option)]
    pub triage_detect_configs: Vec<Utf8PathBuf>,

    /// release version that this BIB corresponds to.
    #[argh(option)]
    version: Option<String>,

    /// path to a file containing the release version that this BIB matches.
    #[argh(option)]
    version_file: Option<Utf8PathBuf>,

    /// name of repository where this board is released.
    #[argh(option)]
    repo: Option<String>,

    /// path to a file containing the repository information.
    #[argh(option)]
    repo_file: Option<Utf8PathBuf>,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a board config.
#[derive(FromArgs, Default)]
#[argh(subcommand, name = "board")]
struct BoardArgs {
    /// the input board config with absolute paths.
    #[argh(option)]
    config: Utf8PathBuf,

    /// the partitions config to add to the board.
    #[argh(option)]
    partitions_config: Option<Utf8PathBuf>,

    /// paths to board input bundles to include.
    #[argh(option)]
    board_input_bundles: Vec<Utf8PathBuf>,

    /// paths to board input bundle sets to make available.
    #[argh(option)]
    board_input_bundle_sets: Vec<Utf8PathBuf>,

    /// the directory to write the board config to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// release version that this board config corresponds to.
    #[argh(option)]
    version: Option<String>,

    /// path to a file containing the release version that this config matches.
    #[argh(option)]
    version_file: Option<Utf8PathBuf>,

    /// name of repository where this board is released.
    #[argh(option)]
    repo: Option<String>,

    /// path to a file containing the repository information.
    #[argh(option)]
    repo_file: Option<Utf8PathBuf>,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a board input bundle set.
#[derive(FromArgs)]
#[argh(subcommand, name = "board-input-bundle-set")]
struct BoardInputBundleSetArgs {
    /// the name of the set.
    #[argh(option)]
    name: String,

    /// paths to board input bundles to include.
    #[argh(option)]
    board_input_bundles: Vec<Utf8PathBuf>,

    /// release version that this BIB corresponds to.
    #[argh(option)]
    version: Option<String>,

    /// path to a file containing the release version that this BIB matches.
    #[argh(option)]
    version_file: Option<Utf8PathBuf>,

    /// name of repository where this board is released.
    #[argh(option)]
    repo: Option<String>,

    /// path to a file containing the repository information.
    #[argh(option)]
    repo_file: Option<Utf8PathBuf>,

    /// the directory to write the board config to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a hybrid board config.
#[derive(FromArgs)]
#[argh(subcommand, name = "hybrid-board")]
struct HybridBoardArgs {
    /// the input board config with absolute paths.
    #[argh(option)]
    config: Utf8PathBuf,

    /// the directory to write the board config to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// a board that contains BIBs that should be added to `config`.
    #[argh(option)]
    replace_bibs_from_board: Option<Utf8PathBuf>,

    /// replace all the bibs from these sets that are found in `config`.
    #[argh(option)]
    replace_bib_sets: Vec<Utf8PathBuf>,

    /// a partitions config to insert into `config`.
    #[argh(option)]
    replace_partitions_config: Option<Utf8PathBuf>,

    /// a new devicetree to add to the board.
    #[argh(option)]
    replace_devicetree: Option<Utf8PathBuf>,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a partitions config.
#[derive(FromArgs)]
#[argh(subcommand, name = "partitions")]
struct PartitionsArgs {
    /// the input partitions config with absolute paths.
    #[argh(option)]
    config: Utf8PathBuf,

    /// the directory to write the partitions config to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

/// Arguments to generate a platform artifacts directory.
#[derive(FromArgs)]
#[argh(subcommand, name = "platform-artifacts")]
pub struct PlatformArtifactsArgs {
    /// the name to give the platform artifacts.
    #[argh(option)]
    name: String,

    /// the list of AIBs to include.
    #[argh(option)]
    aib_list: Utf8PathBuf,

    /// the repository URL.
    #[argh(option)]
    repo: String,

    /// the platform version.
    #[argh(option)]
    version: String,

    /// paths to tool binaries to include.
    #[argh(option)]
    tools: Vec<Utf8PathBuf>,

    /// the directory to write the platform artifacts to.
    #[argh(option)]
    output: Utf8PathBuf,

    /// a depfile to write.
    #[argh(option)]
    depfile: Utf8PathBuf,
}

/// Arguments to extract a package from an assembly product config container.
#[derive(FromArgs)]
#[argh(subcommand, name = "product-package")]
#[allow(dead_code)]
struct ExtractProductPackageArgs {
    /// the input product config container directory.
    #[argh(option)]
    config: Utf8PathBuf,

    /// the name of the package to extract.
    #[argh(option)]
    package_name: String,

    /// the directory to write the package contents to.
    #[argh(option)]
    outdir: Utf8PathBuf,

    /// the filename to write the package manifest to.
    #[argh(option)]
    output_package_manifest: Utf8PathBuf,

    /// a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();
    match args.command {
        Subcommand::Generate(args) => match args.subcommand {
            GenerateSubcommand::Product(args) => product_config::new(&args),
            GenerateSubcommand::ProductInputBundle(args) => product_input_bundle::new(&args),
            GenerateSubcommand::HybridProduct(args) => product_config::hybrid(&args),
            GenerateSubcommand::BoardInputBundle(args) => board_input_bundle::new(&args),
            GenerateSubcommand::BoardInputBundleSet(args) => board_input_bundle_set::new(&args),
            GenerateSubcommand::Board(args) => board_config::new(&args),
            GenerateSubcommand::HybridBoard(args) => board_config::hybrid(&args),
            GenerateSubcommand::Partitions(args) => partitions_config::new(&args),
            GenerateSubcommand::PlatformArtifacts(args) => platform_artifacts::new(&args),
        },
        Subcommand::Extract(args) => match args.subcommand {
            ExtractSubcommand::ProductPackage(args) => product_config::extract_package(&args),
        },
    }
}
