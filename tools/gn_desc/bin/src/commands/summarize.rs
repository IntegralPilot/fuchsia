// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::{self, Display};

use crate::display::{self};
use argh::FromArgs;
use display::{OptionalTitledLine, TitledList};
use gn_graph::{Graph, Target};
use serde_json::to_string_pretty;

/// display a summary of each target
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "summarize")]
pub struct SummarizeArgs {
    /// print the target's dependencies
    #[argh(switch, short = 'd')]
    pub deps: bool,

    /// print the target's inputs
    #[argh(switch, short = 'i')]
    pub inputs: bool,

    /// print the target's outputs
    #[argh(switch, short = 'o')]
    pub outputs: bool,

    /// print the target's sources
    #[argh(switch, short = 's')]
    pub sources: bool,

    /// print the targets arguments
    #[argh(switch, short = 'a')]
    pub arguments: bool,

    /// print the target's toolchain
    #[argh(switch, short = 't')]
    pub toolchain: bool,

    /// print the target's metadata
    #[argh(switch, short = 'm')]
    pub metadata: bool,

    /// verbose mode (print everything above this)
    #[argh(switch, short = 'v')]
    pub verbose: bool,

    /// debug-print the target's complete description data
    #[argh(switch)]
    pub debug_description: bool,
}

impl SummarizeArgs {
    /// Display a summary of the given target.
    pub fn perform(&self, target: &Target, graph: &Graph) -> Result<(), anyhow::Error> {
        let summary = if self.verbose {
            Self {
                verbose: false,
                deps: true,
                inputs: true,
                outputs: true,
                sources: true,
                arguments: true,
                toolchain: true,
                metadata: true,
                ..*self
            }
            .summarize_target(graph, target)?
        } else {
            self.summarize_target(graph, target)?
        };

        println!("\n{}", summary);
        Ok(())
    }

    fn summarize_target<'a>(
        &self,
        graph: &'a Graph,
        target: &'a Target,
    ) -> Result<TargetSummary<'a>, anyhow::Error> {
        let Target { label, target_type, description } = target;

        Ok(TargetSummary {
            label: label.as_ref(),
            target_type: target_type.as_ref(),
            toolchain: self.toolchain.then(|| description.toolchain.as_ref()),
            script: description.script.as_ref().map(|s| s.as_ref()),
            arguments: if self.arguments { description.args.iter().collect() } else { Vec::new() },
            metadata: if self.metadata && description.metadata.len() > 0 {
                Some(format!("{}", to_string_pretty(&description.metadata)?))
            } else {
                None
            },
            inputs: if self.inputs { description.inputs.iter().collect() } else { Vec::new() },
            outputs: if self.outputs { description.outputs.iter().collect() } else { Vec::new() },
            sources: if self.sources { description.sources.iter().collect() } else { Vec::new() },
            deps: if self.deps { description.deps.iter().collect() } else { Vec::new() },
            dep_ofs: if self.deps {
                graph.targets_dependent_on(label).unwrap_or_default()
            } else {
                Vec::new()
            },
            description: if self.debug_description {
                Some(format!("{:#?}", description))
            } else {
                None
            },
        })
    }
}

#[derive(Debug)]
struct TargetSummary<'a> {
    label: &'a str,
    target_type: &'a str,
    script: Option<&'a str>,
    toolchain: Option<&'a str>,
    arguments: Vec<&'a String>,
    metadata: Option<String>,
    inputs: Vec<&'a String>,
    outputs: Vec<&'a String>,
    sources: Vec<&'a String>,
    deps: Vec<&'a String>,
    dep_ofs: Vec<&'a String>,
    description: Option<String>,
}

const DISPLAY_INDENT: usize = 4;

impl<'a> Display for TargetSummary<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.label)?;
        writeln!(f, "  Type: {}", self.target_type)?;

        OptionalTitledLine::format(f, "    Toolchain:", &self.toolchain)?;
        OptionalTitledLine::format(f, "    Script:", &self.script)?;
        OptionalTitledLine::format(f, "    Metadata:\n", &self.metadata)?;

        TitledList::new("Arguments:", &self.arguments, DISPLAY_INDENT).fmt(f)?;

        TitledList::new("Inputs:", &self.inputs, DISPLAY_INDENT).fmt(f)?;
        TitledList::new("Outputs:", &self.outputs, DISPLAY_INDENT).fmt(f)?;
        TitledList::new("Sources:", &self.sources, DISPLAY_INDENT).fmt(f)?;
        TitledList::new("Dependencies:", &self.deps, DISPLAY_INDENT).fmt(f)?;
        TitledList::new("Dependency of:", &self.dep_ofs, DISPLAY_INDENT).fmt(f)?;

        OptionalTitledLine::format(f, "    Raw Description:\n", &self.description)?;

        Ok(())
    }
}
