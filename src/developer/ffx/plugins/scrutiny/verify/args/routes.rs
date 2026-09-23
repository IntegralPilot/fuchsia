// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use cm_rust::CapabilityTypeName;
use ffx_core::ffx_command;
use scrutiny_frontend::verify::capability_routing::ResponseLevel;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "routes",
    description = "Verifies capability routes in the component tree",
    example = r#"To verify routes on your current build:

    $ ffx scrutiny verify routes \
        --product-bundle $(fx get-build-dir)/obj/build/images/fuchsia/product_bundle"#
)]
pub struct Command {
    /// capability types to verify.
    #[argh(option)]
    pub capability_type: Vec<CapabilityTypeName>,
    /// response level to report from routes scrutiny plugin.
    #[argh(option, default = "ResponseLevel::Error")]
    pub response_level: ResponseLevel,
    /// absolute or working directory-relative path to a product bundle.
    #[argh(option)]
    pub product_bundle: PathBuf,
    /// absolute or working path-relative path to component tree configuration file that affects
    /// how component tree data is gathered.
    #[argh(option)]
    pub component_tree_config: Vec<PathBuf>,
}
