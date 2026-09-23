// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "add",
    description = "Add a node with a given bind property",
    example = "To add a node to the driver framework:

    $ driver node add my-node my-key-string=my-value-string",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct AddNodeCommand {
    /// the name of the node.
    #[argh(positional)]
    pub name: String,

    /// the node's property. Should be in the format key=value.
    #[argh(positional)]
    pub property: String,
}
