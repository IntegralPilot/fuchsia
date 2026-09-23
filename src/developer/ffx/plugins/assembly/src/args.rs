// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use assembly_cli_args::{CreateSystemArgs, ProductArgs};
use assembly_images_config::BlobfsLayout;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "assembly", description = "Assemble images")]
pub struct AssemblyCommand {
    /// the assembly operation to perform
    #[argh(subcommand)]
    pub op_class: OperationClass,
}

/// This is the set of top-level operations within the `ffx assembly` plugin
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand)]
pub enum OperationClass {
    CreateSystem(CreateSystemArgs),
    CreateUpdate(CreateUpdateArgs),
    Product(ProductArgs),
    SizeCheck(SizeCheckArgs),
}

/// construct an UpdatePackage using images and package.
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "create-update")]
pub struct CreateUpdateArgs {
    /// path to a partitions config, which specifies where in the partition
    /// table the images are put.
    #[argh(option)]
    pub partitions: Utf8PathBuf,

    /// path to an images manifest, which specifies images to put in slot A.
    #[argh(option)]
    pub system_a: Option<Utf8PathBuf>,

    /// path to an images manifest, which specifies images to put in slot R.
    #[argh(option)]
    pub system_r: Option<Utf8PathBuf>,

    /// backstop OTA version.
    /// Fuchsia will reject updates with a lower epoch.
    #[argh(option)]
    pub epoch: u64,

    /// rewrite package repository URLs to use this package URL hostname.
    #[argh(option)]
    pub rewrite_default_repo: Option<String>,

    /// name to give the Subpackage Blobs Package.
    /// This is currently only used by OTA tests to allow publishing multiple
    /// subpackage blob packages to the same amber repository without naming
    /// collisions.
    #[argh(option, default = "default_subpackage_blobs_package_name()")]
    pub subpackage_blobs_package_name: String,

    /// name to give the Update Package.
    /// This is currently only used by OTA tests to allow publishing multiple
    /// update packages to the same amber repository without naming collisions.
    #[argh(option)]
    pub update_package_name: Option<String>,

    /// directory to write the UpdatePackage.
    #[argh(option)]
    pub outdir: Utf8PathBuf,

    /// directory to write intermediate files.
    #[argh(option)]
    pub gendir: Option<Utf8PathBuf>,
}

fn default_subpackage_blobs_package_name() -> String {
    "subpackage_blobs".into()
}

/// Perform size checks (on packages or product based on the sub-command).
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "size-check")]
pub struct SizeCheckArgs {
    #[argh(subcommand)]
    pub op_class: SizeCheckOperationClass,
}

/// The set of operations available under `ffx assembly size-check`.
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand)]
pub enum SizeCheckOperationClass {
    /// Check that the set of all blobs included in the product fit in the blobfs capacity.
    Product(ProductSizeCheckArgs),
    /// Check that package sets are not over their allocated budgets.
    Package(PackageSizeCheckArgs),
}

/// Measure package sizes and verify they fit in the specified budgets.
/// Exit status is 2 when one or more budgets are exceeded, and 1 when
/// a failure prevented the budget verification to happen.
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "package")]
pub struct PackageSizeCheckArgs {
    /// path to a JSON file containing the list of size budgets.
    /// Each size budget has a `name`, a `size` which is the maximum
    /// number of bytes, and `packages` a list of path to manifest files.
    #[argh(option)]
    pub budgets: Utf8PathBuf,
    /// path to a `blobs.json` file. It provides the size of each blob
    /// composing the package on device.
    #[argh(option)]
    pub blob_sizes: Vec<Utf8PathBuf>,
    /// the layout of blobs in blobfs.
    #[argh(option, default = "default_blobfs_layout()")]
    pub blobfs_layout: BlobfsLayout,
    /// path where to write the verification report, in JSON format.
    #[argh(option)]
    pub gerrit_output: Option<Utf8PathBuf>,
    /// show the storage consumption of each component broken down by package
    /// regardless of whether the component exceeded its budget.
    #[argh(switch, short = 'v')]
    pub verbose: bool,
    /// path where to write the verbose JSON output.
    #[argh(option)]
    pub verbose_json_output: Option<Utf8PathBuf>,
}

/// (Not implemented yet) Check that the set of all blobs included in the product
/// fit in the blobfs capacity.
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "product")]
pub struct ProductSizeCheckArgs {
    /// use specific auth mode for oauth2 (see examples; default: pkce).
    #[argh(option, default = "AuthMode::Default")]
    pub auth: AuthMode,
    /// path to the images.json.
    #[argh(option)]
    pub assembly_manifest: Utf8PathBuf,
    /// path to the past images.json which will be used to compare with the current
    /// images.json to produce a diff.
    #[argh(option)]
    pub base_assembly_manifest: Option<Utf8PathBuf>,
    /// whether to show the verbose output.
    #[argh(switch, short = 'v')]
    pub verbose: bool,
    /// path to the directory where HTML visualization should be stored.
    #[argh(option)]
    pub visualization_dir: Option<Utf8PathBuf>,
    /// path where to write the gerrit size report.
    #[argh(option)]
    pub gerrit_output: Option<Utf8PathBuf>,
    /// path where to write the size breakdown.
    #[argh(option)]
    pub size_breakdown_output: Option<Utf8PathBuf>,
    /// maximum amount that the size of blobfs can increase in one CL.
    /// This value is propagated to the gerrit size report.
    #[argh(option)]
    pub blobfs_creep_budget: Option<u64>,
    /// maximum amount of bytes the platform resources can consume.
    #[argh(option)]
    pub platform_resources_budget: Option<u64>,
}

/// Select an Oauth2 authorization mode.
#[derive(PartialEq, Debug, Clone, Default)]
pub enum AuthMode {
    #[default]
    /// defaults to Pkce
    Default,
    /// expects a path to a tool which will print an access token to stdout and exit 0.
    Exec(PathBuf),
    /// uses PKCE auth flow to obtain an access token (requires GUI browser)
    Pkce,
}

impl FromStr for AuthMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.as_ref() {
            "default" => Ok(AuthMode::Default),
            "pkce" => Ok(AuthMode::Pkce),
            exec => {
                let path = Path::new(exec);
                if path.is_file() {
                    Ok(AuthMode::Exec(path.to_path_buf()))
                } else {
                    Err("Unknown auth flow choice. Use one of \
                        pkce, default, or a path to an executable \
                        which prints an access token to stdout."
                        .to_string())
                }
            }
        }
    }
}

fn default_blobfs_layout() -> BlobfsLayout {
    BlobfsLayout::Compact
}
