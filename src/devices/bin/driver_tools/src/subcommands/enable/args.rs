// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "enable",
    description = "Enables the given driver, reverting a disable action.",
    note = "This is only meant to revert a DisableDriver action. After this call, the driver will be considered for matching to nodes again.",
    example = "To enable a driver

    $ driver enable 'fuchsia-pkg://fuchsia.com/example_driver#meta/example_driver.cm'",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct EnableCommand {
    #[argh(positional, description = "component URL of the driver to be enabled.")]
    pub url: String,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,
}
