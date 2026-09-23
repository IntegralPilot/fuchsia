// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::subcommands::add::args::AddNodeCommand;
use super::subcommands::graph::args::GraphNodeCommand;
use super::subcommands::list::args::ListNodeCommand;
use super::subcommands::remove::args::RemoveNodeCommand;
use super::subcommands::show::args::ShowNodeCommand;
use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "node",
    description = "Commands to interact with driver framework nodes."
)]
pub struct NodeCommand {
    #[argh(subcommand)]
    pub subcommand: NodeSubcommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum NodeSubcommand {
    List(ListNodeCommand),
    Show(ShowNodeCommand),
    Add(AddNodeCommand),
    Remove(RemoveNodeCommand),
    Graph(GraphNodeCommand),
}
