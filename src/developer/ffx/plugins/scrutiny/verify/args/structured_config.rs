// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "structured-config",
    description = "Verifies component configuration according to configured assertions.",
    example = r#"To verify structured config on your current build:

    $ ffx scrutiny verify structured-config \
        --product-bundle $(fx get-build-dir)/obj/build/images/fuchsia/product_bundle \
        --policy path/to/policy
        --component-tree-config path/to/platform
        --component-tree-config path/to/product"#
)]
pub struct Command {
    /// absolute or working directory-relative path to a policy file for structured configuration
    #[argh(option)]
    pub policy: PathBuf,

    /// path to a product bundle.
    #[argh(option)]
    pub product_bundle: PathBuf,

    /// absolute or working path-relative path to component tree configuration file that affects
    /// how component tree data is gathered.
    #[argh(option)]
    pub component_tree_config: Vec<PathBuf>,
}
