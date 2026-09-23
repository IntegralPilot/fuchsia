// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;

/// Display a list of details from within a product bundle.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "show")]
pub struct ShowCommand {
    /// list the virtual devices linked to this product bundle.
    #[argh(switch)]
    pub devices: bool,

    /// print the details of a virtual device linked to this product bundle.
    #[argh(option)]
    pub device: Option<String>,

    /// the path to the product bundle to inspect.
    #[argh(positional)]
    pub product_bundle_path: Utf8PathBuf,
}
