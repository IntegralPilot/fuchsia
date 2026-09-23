// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "pre-signing",
    description = "Runs assorted checks to ensure a build is okay to sign",
    example = r#"To run the pre signing checks against a build:

    $ ffx scrutiny verify pre-signing \
        --product-bundle $(fx get-build-dir)/obj/build/images/fuchsia/product_bundle \
        --policy path/to/policy_file \
        --golden_files_dir path/to/goldens/dir"#
)]
pub struct Command {
    /// path to a signing validation policy file
    #[argh(option)]
    pub policy: PathBuf,

    /// path to the product bundle for the build to validate
    #[argh(option)]
    pub product_bundle: PathBuf,

    /// path to the directory containing golden files for pre_signing
    #[argh(option)]
    pub golden_files_dir: PathBuf,
}
