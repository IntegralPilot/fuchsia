// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "package",
    description = "Extracts a Fuchsia package from a Url",
    example = "To extract a Fuchsia package from a url:

        $ ffx scrutiny extract package \
            --product-bundle $(fx get-build-dir)/obj/build/images/fuchsia/product_bundle \
            --url fuchsia-pkg://fuchsia.com/foo \
            --output /tmp/foo",
    note = "Extracts a package to a specific directory."
)]
pub struct ScrutinyPackageCommand {
    /// a path to a product bundle that contains the package.
    #[argh(option)]
    pub product_bundle: PathBuf,
    /// the package url.
    #[argh(option)]
    pub url: AbsolutePackageUrl,
    /// the location to write the output artifacts.
    #[argh(option)]
    pub output: String,
    /// build scrutiny model based on recovery-mode build artifacts.
    #[argh(switch)]
    pub recovery: bool,
}
