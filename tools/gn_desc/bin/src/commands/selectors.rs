// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use argh::FromArgs;
use gn_graph::{Graph, Target};
use regex::Regex;

use crate::commands::CommandsArg;

/// How to select the package(s) to do the operation on.
#[derive(Debug, FromArgs)]
#[argh(subcommand)]
pub enum NodeSelector {
    Exact(Exact),
    Match(ByLabelRegex),
    MatchFile(ByFileRegex),
}

/// an exact GN label
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "label")]
pub struct Exact {
    /// a list of exact target labels
    #[argh(positional)]
    pub labels: Vec<String>,

    /// the command to run on the selected labels.
    #[argh(subcommand)]
    pub command: CommandsArg,
}

/// find by substring or regex on gn target labels
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "match")]
pub struct ByLabelRegex {
    /// the regex to match labels against
    #[argh(positional)]
    pub target_spec: String,

    /// the command to run on the selected labels.
    #[argh(subcommand)]
    pub command: CommandsArg,
}

/// find by substring or regex on files
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "match-file")]
pub struct ByFileRegex {
    /// match against all files (inputs, outputs, sources, scripts), not just outputs
    #[argh(switch, short = 'a')]
    all: bool,

    /// match against input files
    #[argh(switch, short = 'i')]
    inputs: bool,

    /// match against output files
    #[argh(switch, short = 'o')]
    outputs: bool,

    /// match against source files
    #[argh(switch, short = 's')]
    sources: bool,

    /// match against script files
    #[argh(switch, short = 'x')]
    scripts: bool,

    /// the regex to match files against
    #[argh(positional)]
    pub target_spec: String,

    /// the command to run on the selected labels.
    #[argh(subcommand)]
    pub command: CommandsArg,
}

impl NodeSelector {
    /// Select TargetNodes in the graph that match this NodeSelector
    pub fn select_from<'a>(&self, graph: &'a Graph) -> Result<Vec<&'a Target>, anyhow::Error> {
        match self {
            // Find targets that have an exact match for the complete GN label name
            NodeSelector::Exact(exact) => Ok(filter_targets_by_label(graph, |target_label| {
                for label in &exact.labels {
                    if label == target_label {
                        return true;
                    }
                }
                return false;
            })),
            // Find targets that have a regex match against the GN label
            NodeSelector::Match(by) => {
                let filter =
                    Regex::new(by.target_spec.as_str()).context("Unable to parse regex filter")?;
                Ok(filter_targets_by_label(graph, |label| filter.is_match(label)))
            }
            // Find targets that have a regex match against the files involved in a GN target
            NodeSelector::MatchFile(by) => {
                let filter =
                    Regex::new(by.target_spec.as_str()).context("Unable to parse regex filter")?;

                // Check all by default.
                let by_all = by.all || !by.inputs && !by.outputs && !by.scripts;

                // Check the inputs if inputs or all was selected.
                let check_inputs = by.inputs || by_all;

                // Check the outputs if the switches for checking inputs or scripts hasn't been set,
                // or if the switch for all sources or the outputs is set.  This allows teh default
                // case to act as if outputs was selected.
                let check_outputs = by.outputs || by_all;

                let check_sources = by.sources || by_all;

                // Check the scripts if scripts or all was selected.
                let check_scripts = by.scripts || by.all;

                Ok(filter_targets(graph, |t| {
                    (check_outputs
                        && t.description.outputs.iter().any(|file| filter.is_match(file)))
                        || (check_inputs
                            && t.description.inputs.iter().any(|file| filter.is_match(file)))
                        || (check_sources
                            && t.description.sources.iter().any(|file| filter.is_match(file)))
                        || (check_scripts
                            && t.description.script.as_ref().map_or(false, |s| filter.is_match(s)))
                }))
            }
        }
    }

    /// Perform the command specified in the arguments on the selected target.
    pub fn perform_command(&self, target: &Target, graph: &Graph) -> Result<(), anyhow::Error> {
        match self {
            NodeSelector::Exact(selector) => selector.command.perform(target, graph),
            NodeSelector::Match(selector) => selector.command.perform(target, graph),
            NodeSelector::MatchFile(selector) => selector.command.perform(target, graph),
        }
    }
}

/// Return a set of Target references from the graph, based on filtering the label using criteria
/// provided by 'f'
fn filter_targets_by_label<'a, F>(graph: &'a Graph, mut f: F) -> Vec<&'a Target>
where
    F: FnMut(&'a String) -> bool,
{
    filter_targets(graph, |t| (f)(&t.label))
}

/// Return a set of Target references from the graph, based on filtering criteria provided by 'f'
fn filter_targets<'a, F>(graph: &'a Graph, mut f: F) -> Vec<&'a Target>
where
    F: FnMut(&'a Target) -> bool,
{
    graph.targets().iter().by_ref().filter(|(_, v)| (f)(v)).map(|(_, v)| v).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gn_json::target::{TargetDescription, TargetDescriptionBuilder};
    use std::collections::HashMap;

    fn make_graph() -> Graph {
        let mut descs = HashMap::<String, TargetDescription>::new();
        let default_description = TargetDescriptionBuilder::default()
            .target_type("group")
            .deps(vec!["//:a", "//:b"])
            .build();

        descs.insert("//:default".to_owned(), default_description.clone());
        descs.insert(
            "//:a".to_owned(),
            TargetDescriptionBuilder::default().target_type("group").build(),
        );
        descs.insert(
            "//:b".to_owned(),
            TargetDescriptionBuilder::default().target_type("group").build(),
        );
        descs.insert(
            "//:c".to_owned(),
            TargetDescriptionBuilder::default().target_type("group").build(),
        );
        let descs = descs;

        Graph::create_from(descs).unwrap()
    }

    #[test]
    pub fn test_target_label_filtering_exact_single() {
        let graph = make_graph();

        let selector = NodeSelector::Exact(Exact {
            labels: vec!["//:a".to_owned()],
            command: CommandsArg::List(crate::commands::list::ListArgs {}),
        });
        let targets = selector.select_from(&graph).unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].label, "//:a");
    }

    #[test]
    pub fn test_target_label_filtering_exact_multiple() {
        let graph = make_graph();

        let selector = NodeSelector::Exact(Exact {
            labels: vec!["//:a".to_owned(), "//:default".to_owned()],
            command: CommandsArg::List(crate::commands::list::ListArgs {}),
        });
        let targets = selector.select_from(&graph).unwrap();

        assert_eq!(targets.len(), 2);

        let labels: Vec<&String> = targets.iter().map(|t| &t.label).collect();

        assert!(labels.contains(&&"//:a".to_owned()));
        assert!(labels.contains(&&"//:default".to_owned()));
    }
}
