// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "remove",
    description = "Remove a node that was previously added using driver node add.
    ",
    example = "To remove a node to the driver framework:

    $ driver node remove my-node",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct RemoveNodeCommand {
    /// the name of the node.
    #[argh(positional)]
    pub name: String,
}
