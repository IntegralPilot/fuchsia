// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subcommands::node::common::NodeFilter;
use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    description = "List nodes in the driver framework",
    example = "To list all nodes:

    $ driver node list

To list nodes that match a name or driver:

    $ driver node list my_node
    ",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ListNodeCommand {
    /// shows the node's state and bound driver url if one exists.
    #[argh(switch, short = 'v', long = "verbose")]
    pub verbose: bool,

    /// return a non-zero exit code if no matching devices are found.
    #[argh(switch, short = 'f', long = "fail-on-missing")]
    pub fail_on_missing: bool,

    #[argh(option, long = "only", short = 'o')]
    /// filter the instance list by a criteria:
    /// bound, unbound,
    /// ancestors:<node_name>, primary_ancestors:<node_name>,
    /// descendants:<node_name>, relatives:<node_name>, primary_relatives:<node_name>,
    /// siblings:<node_name>, or primary_siblings:<node_name>.
    /// the primary variants indicate to only traverse primary parents when encountering composites
    pub filter: Option<NodeFilter>,
}
