// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use component_debug::cli::{GraphFilter, GraphOrientation};
use component_debug_fdomain as component_debug;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "graph",
    description = "Outputs a Graphviz dot graph for the components in the component topology. Children of unresolved components are not included in this list.",
    example = "To graph components in the topology:

    $ ffx component graph

    To graph all running components in the topology:

    $ ffx component graph --only running

    To graph all stopped components in the topology:

    $ ffx component graph --only stopped

    To graph the ancestors of a component named `foo`:

    $ ffx component graph --only ancestor:foo

    To graph the descendants of a component named `foo`:

    $ ffx component graph --only descendant:foo

    To graph both the ancestors and descendants of a component named `foo`:

    $ ffx component graph --only relatives:foo

    To order the graph's nodes from left-to-right (instead of top-to-bottom):

    $ ffx component graph --orientation left_to_right"
)]

pub struct ComponentGraphCommand {
    #[argh(option, long = "only", short = 'o')]
    /// filter the instance list by a criteria: ancestor, descendant, relative
    pub filter: Option<GraphFilter>,

    #[argh(option, long = "orientation", short = 'r', default = "GraphOrientation::TopToBottom")]
    /// changes the visual orientation of the graph's nodes.
    /// Allowed values are "lefttoright"/"lr" and "toptobottom"/"tb".
    pub orientation: GraphOrientation,
}
