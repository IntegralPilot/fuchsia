// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "capability",
    description = "Lists component instances that reference a capability",
    example = "To show all components that reference a capability:

    $ ffx component capability fuchsia.net.routes"
)]
pub struct ComponentCapabilityCommand {
    #[argh(positional)]
    /// name of a capability. Partial matches allowed.
    pub capability: String,
}
