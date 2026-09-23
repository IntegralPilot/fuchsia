// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "show",
    description = "Show a specific node in the driver framework",
    example = "To show a node:

    $ driver node show dev.sys.my_node
    ",
    note = "This command supports partial matches over the node moniker and driver URL.
    If the query matches more than one node, all matching nodes will be shown.
    ",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ShowNodeCommand {
    #[argh(positional)]
    /// driver URL or moniker. Partial matches allowed.
    pub query: String,
}
