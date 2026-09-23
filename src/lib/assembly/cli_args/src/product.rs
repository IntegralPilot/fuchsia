// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_config::EnvironmentContext;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;

/// Arguments for performing a high-level product assembly operation.
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "product")]
pub struct ProductArgs {
    /// the product configuration directory.
    #[argh(option)]
    pub product: Utf8PathBuf,

    /// the board configuration directory.
    #[argh(option)]
    pub board_config: Utf8PathBuf,

    /// the directory to write assembled outputs to.
    #[argh(option)]
    pub outdir: Utf8PathBuf,

    /// the directory to write generated intermediate files to.
    #[argh(option)]
    pub gendir: Utf8PathBuf,

    /// [DEPRECATED] the directory in which to find the platform artifacts
    #[argh(option)]
    pub input_bundles_dir: Option<Utf8PathBuf>,

    /// the directory in which to find the platform artifacts
    #[argh(option)]
    pub platform_artifacts: Option<Utf8PathBuf>,

    /// disable validation of the assembly's packages
    #[argh(option)]
    pub package_validation: Option<ValidationMode>,

    /// path to an AIB containing a customized kernel zbi to use instead of the
    /// one in the platform AIBs.
    #[argh(option)]
    pub custom_kernel_aib: Option<Utf8PathBuf>,

    /// path to an AIB containing a customized qemu_kernel boot shim to use
    /// instead of the in the platform AIBs.
    #[argh(option)]
    pub custom_boot_shim_aib: Option<Utf8PathBuf>,

    /// whether to hide the warning that shows the overrides that are enabled.
    /// This can be helpful to disable for test assemblies.
    #[argh(switch)]
    pub suppress_overrides_warning: bool,

    /// path to a file specifying developer-level overrides for assembly.
    #[argh(option)]
    pub developer_overrides: Option<Utf8PathBuf>,

    /// flag stating whether the example AIB should be included.
    #[argh(option)]
    pub include_example_aib_for_tests: Option<bool>,

    /// change the default mode assembly runs in to produce test images.
    #[argh(option, default = "default_mode()")]
    pub mode: AssemblyMode,

    /// a list of board input bundle sets to override with.
    #[argh(option)]
    pub board_input_bundle_sets: Vec<Utf8PathBuf>,

    /// a list of product input bundles to override with.
    #[argh(option)]
    pub product_input_bundles: Vec<Utf8PathBuf>,
}

impl ProductArgs {
    /// convert args struct to string vector
    pub fn to_vec(&self, context: &EnvironmentContext) -> Vec<String> {
        let mut args = vec![
            "product".to_string(),
            "--product".to_string(),
            self.product.to_string(),
            "--board-config".to_string(),
            self.board_config.to_string(),
            "--outdir".to_string(),
            self.outdir.to_string(),
            "--gendir".to_string(),
            self.gendir.to_string(),
            "--input-bundles-dir".to_string(),
            self.get_platform_artifacts().to_string(),
        ];

        if let Some(val) = &self.package_validation {
            args.push("--package-validation".to_string());
            args.push(val.to_string());
        }
        if let Some(path) = &self.custom_kernel_aib {
            args.push("--custom-kernel-aib".to_string());
            args.push(path.to_string());
        }
        if let Some(path) = &self.custom_boot_shim_aib {
            args.push("--custom-boot-shim-aib".to_string());
            args.push(path.to_string());
        }
        if self.suppress_overrides_warning {
            args.push("--suppress-overrides-warning".to_string());
        }
        if let Some(path) = &self.developer_overrides {
            args.push("--developer-overrides".to_string());
            args.push(path.to_string());
        }

        if context.get::<bool, _>("assembly_example_enabled").unwrap_or_default() {
            args.push("--include-example-aib-for-tests".to_string());
            args.push(true.to_string());
        }
        if !self.mode.is_default() {
            args.push("--mode".to_string());
            args.push(self.mode.to_string());
        }
        for bib_set in &self.board_input_bundle_sets {
            args.push("--board-input-bundle-sets".to_string());
            args.push(bib_set.to_string());
        }
        for pib in &self.product_input_bundles {
            args.push("--product-input-bundles".to_string());
            args.push(pib.to_string());
        }
        args
    }

    /// Get the path to platform artifacts, falling back to input bundles dir.
    pub fn get_platform_artifacts(&self) -> Utf8PathBuf {
        self.platform_artifacts
            .clone()
            .or_else(|| self.input_bundles_dir.clone())
            .expect("At least one of --platform-artifacts or --input-bundles-dir must be provided")
    }
}

/// How to validate the product.
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum ValidationMode {
    /// Do not validate.
    Off,

    /// Validate everything, but print warnings instead of exiting.
    WarnOnly,

    /// Validate everything.
    #[default]
    On,
}

impl fmt::Display for ValidationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationMode::Off => write!(f, "off"),
            ValidationMode::WarnOnly => write!(f, "warning"),
            ValidationMode::On => write!(f, "error"),
        }
    }
}

impl FromStr for ValidationMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "off" => Ok(ValidationMode::Off),
            "warning" => Ok(ValidationMode::WarnOnly),
            "error" => Ok(ValidationMode::On),
            _ => Err(format!(
                "Unknown handling for package validation, valid values are 'off', 'warning' and 'error' (the default): {}",
                s
            )),
        }
    }
}

/// outputs for product assembly operation
pub struct ProductAssemblyOutputs {
    /// path to platform artifacts
    pub platform: Utf8PathBuf,
    /// path to output directory
    pub outdir: Utf8PathBuf,
    /// path to gen directory
    pub gendir: Utf8PathBuf,
    /// path to image assembly config output file
    pub image_assembly_config: Utf8PathBuf,
    /// the mode to run assembly in.
    pub mode: AssemblyMode,
}

impl From<ProductArgs> for ProductAssemblyOutputs {
    fn from(args: ProductArgs) -> Self {
        let mut image_assembly_config = args.outdir.clone();
        image_assembly_config.push("image_assembly.json");

        ProductAssemblyOutputs {
            platform: args.get_platform_artifacts(),
            outdir: args.outdir,
            gendir: args.gendir,
            image_assembly_config,
            mode: args.mode,
        }
    }
}

/// A mode for Assembly to run in.
#[derive(Debug, Default, PartialEq, Clone, Copy, Serialize)]
pub enum AssemblyMode {
    /// Passes a prebuilt image through to the "ZBI" position in the PB.
    /// This image may not be a ZBI.
    /// Accepts a custom qemu kernel.
    TestRamdisk,

    /// Adds a real ZBI, but possibly no kernel, and definitely no fvm/fxfs.
    /// Uses the board to add a dtbo and run the postprocessing script.
    /// Accepts a custom qemu kernel.
    /// Used for
    /// * Test ZBI (no zircon)
    /// * Bootfs test program
    TestZBI,

    /// Adds a board and product, but skips the platform.
    /// This is often used for testing Assembly itself.
    TestNoPlatform,

    /// Skips the filesystems (fvm/fxfs) generation.
    SkipFilesystems,

    /// The normal mode of operation for assembly is to build everything.
    #[default]
    BuildEverything,
}

impl AssemblyMode {
    /// Returns whether this mode produces a test kernel.
    /// Assembly should not modify this kernel in any way.
    pub fn is_test_kernel(&self) -> bool {
        matches!(self, Self::TestRamdisk | Self::TestZBI)
    }

    /// Returns whether this mode is the default mode.
    pub fn is_default(&self) -> bool {
        matches!(self, Self::BuildEverything)
    }

    /// Whether to configure subsystems based on the mode.
    pub fn should_configure_subsystems(&self) -> bool {
        return self.is_default() || *self == AssemblyMode::SkipFilesystems;
    }
}

impl FromStr for AssemblyMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "test-ramdisk" => Ok(Self::TestRamdisk),
            "test-zbi" => Ok(Self::TestZBI),
            "test-no-platform" => Ok(Self::TestNoPlatform),
            "skip-filesystems" => Ok(Self::SkipFilesystems),
            _ => Err(format!(
                "Unknown option for 'mode', valid values are 'test-ramdisk', 'test-zbi' and 'test-no-platform': {}",
                s
            )),
        }
    }
}

impl fmt::Display for AssemblyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssemblyMode::TestRamdisk => write!(f, "test-ramdisk"),
            AssemblyMode::TestZBI => write!(f, "test-zbi"),
            AssemblyMode::TestNoPlatform => write!(f, "test-no-platform"),
            AssemblyMode::BuildEverything => write!(f, "build-everything"),
            AssemblyMode::SkipFilesystems => write!(f, "skip-filesystems"),
        }
    }
}

/// Get the default assembly mode.
pub fn default_mode() -> AssemblyMode {
    AssemblyMode::default()
}
