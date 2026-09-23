// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::product::{AssemblyMode, ProductAssemblyOutputs};

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;

/// Create the system images.
#[derive(Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "create-system")]
pub struct CreateSystemArgs {
    /// the platform artifacts directory.
    /// this is needed in order to retrieve the assembly binary.
    #[argh(option)]
    pub platform: Utf8PathBuf,

    /// the configuration file that specifies the packages, binaries, and
    /// settings specific to the product being assembled.
    #[argh(option)]
    pub image_assembly_config: Utf8PathBuf,

    /// the directory to write the assembled system to.
    #[argh(option)]
    pub outdir: Utf8PathBuf,

    /// the directory to write generated intermediate files to.
    #[argh(option)]
    pub gendir: Utf8PathBuf,

    /// whether to include an account partition in the FVMs.
    #[argh(switch)]
    pub include_account: Option<bool>,

    /// name to give the Base Package. this is useful if you must publish multiple
    /// base packages to the same TUF repository.
    #[argh(option)]
    pub base_package_name: Option<String>,

    /// change the default mode assembly runs in to produce test images.
    #[argh(option, default = "crate::product::default_mode()")]
    pub mode: AssemblyMode,
}

impl CreateSystemArgs {
    /// convert arg struct to vector
    pub fn to_vec(&self) -> Vec<String> {
        let mut args = vec![
            "create-system".to_string(),
            "--platform".to_string(),
            self.platform.to_string(),
            "--image-assembly-config".to_string(),
            self.image_assembly_config.to_string(),
            "--outdir".to_string(),
            self.outdir.to_string(),
            "--gendir".to_string(),
            self.gendir.to_string(),
        ];

        if let Some(true) = &self.include_account {
            args.push("--include-account".to_string());
        }
        if let Some(name) = &self.base_package_name {
            args.push("--base-package-name".to_string());
            args.push(name.clone());
        }
        if !self.mode.is_default() {
            args.push("--mode".to_string());
            args.push(self.mode.to_string());
        }

        args
    }
}

impl From<ProductAssemblyOutputs> for CreateSystemArgs {
    fn from(outs: ProductAssemblyOutputs) -> Self {
        CreateSystemArgs {
            platform: outs.platform.clone(),
            image_assembly_config: outs.image_assembly_config.clone(),
            outdir: outs.outdir.clone(),
            gendir: outs.gendir,
            base_package_name: None,
            include_account: None,
            mode: outs.mode,
        }
    }
}

/// Create output struct for CreateSystem operation.
pub struct CreateSystemOutputs {
    /// The directory where the generated assembled system is written.
    pub outdir: Utf8PathBuf,
}

impl From<CreateSystemArgs> for CreateSystemOutputs {
    fn from(args: CreateSystemArgs) -> Self {
        CreateSystemOutputs { outdir: args.outdir }
    }
}
