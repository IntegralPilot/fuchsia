// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "disable",
    description = "Disables the given driver, and restart its nodes with rematching.",
    note = "After this call, nodes that were bound to the requested driver will either have
another driver (specifically a fallback driver) bound to them, or the node becomes an unbound node
ready to bind to a driver later, which is generally done with a register call. If there is a
fallback driver that might take the place of the driver being disabled, and you want to register
your own driver for the node, the fallback driver should be disabled as well.",
    example = "To disable a driver

    $ driver disable 'fuchsia-pkg://fuchsia.com/example_driver#meta/example_driver.cm'

This can also be used with boot drivers, but keep in mind if the driver being disabled is
critical to the system, the system will become unstable.

    $ driver disable 'fuchsia-boot:///#meta/example_driver.cm'",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct DisableCommand {
    #[argh(positional, description = "component URL of the driver to be disabled.")]
    pub url: String,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,
}
