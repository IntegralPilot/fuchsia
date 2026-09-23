// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use gn_graph::Target;

pub mod all_deps;
pub mod list;
pub mod selectors;
pub mod summarize;

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
pub enum CommandsArg {
    List(list::ListArgs),
    Summarize(summarize::SummarizeArgs),
    AllDeps(all_deps::AllDepsArgs),
}

impl CommandsArg {
    pub fn perform(&self, target: &Target, graph: &gn_graph::Graph) -> Result<(), anyhow::Error> {
        match self {
            CommandsArg::List(args) => args.perform(target, graph),
            CommandsArg::Summarize(args) => args.perform(target, graph),
            CommandsArg::AllDeps(args) => args.perform(target, graph),
        }
    }
}
