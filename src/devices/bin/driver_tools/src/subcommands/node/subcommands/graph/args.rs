// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subcommands::node::common::NodeFilter;
use argh::{ArgsInfo, FromArgs};
use std::str::FromStr;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "graph",
    description = "Outputs a Graphviz dot graph for the nodes in the node topology.",
    example = "To graph all nodes:

    $ driver node graph
    ",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct GraphNodeCommand {
    #[argh(option, long = "only", short = 'o')]
    /// filter the nodes by a criteria:
    /// bound, unbound,
    /// ancestors:<node_name>, primary_ancestors:<node_name>,
    /// descendants:<node_name>, relatives:<node_name>, primary_relatives:<node_name>,
    /// siblings:<node_name>, or primary_siblings:<node_name>.
    /// the primary variants indicate to only traverse primary parents when encountering composites
    pub filter: Option<NodeFilter>,

    #[argh(option, long = "orientation", short = 'r', default = "GraphOrientation::TopToBottom")]
    /// changes the visual orientation of the graph's nodes.
    /// Allowed values are "lefttoright"/"lr" and "toptobottom"/"tb".
    pub orientation: GraphOrientation,

    /// whether to show an edge for service routes between nodes. It is highly recommended to
    /// narrow down the graph by using filters to just the nodes you need, otherwise the graph
    /// is extremely crowded and can cause timeouts on render engines.
    #[argh(switch, long = "services")]
    pub services: bool,

    /// generate an html to display a generated svg with interactivity like highlighting. Reads
    /// from stdin or from the given svg file.
    #[argh(switch, long = "html")]
    pub generate_html: bool,

    /// the svg file to read for processing with the html generation.
    #[argh(option, long = "svg", short = 's')]
    pub svg_path: Option<String>,
}

/// Determines the visual orientation of the graph's nodes.
#[derive(Debug, PartialEq)]
pub enum GraphOrientation {
    /// The graph's nodes should be ordered from top to bottom.
    TopToBottom,
    /// The graph's nodes should be ordered from left to right.
    LeftToRight,
}

impl FromStr for GraphOrientation {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace("_", "").replace("-", "").as_str() {
            "tb" | "toptobottom" => Ok(GraphOrientation::TopToBottom),
            "lr" | "lefttoright" => Ok(GraphOrientation::LeftToRight),
            _ => Err("graph orientation should be 'toptobottom' or 'lefttoright'."),
        }
    }
}
