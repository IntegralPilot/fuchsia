// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A Graph of GN Targets

use gn_json::target::{AllTargets, TargetDescription};
use petgraph::graph::DiGraph;
use petgraph::EdgeDirection;
use std::collections::BTreeMap;
use thiserror::Error;

/// The GN target data
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub label: String,
    pub target_type: String,
    pub description: TargetDescription,
}

impl Target {
    pub fn from_description(label: impl ToString, description: TargetDescription) -> Self {
        Self {
            label: label.to_string(),
            target_type: description.target_type.clone(),
            description: description,
        }
    }
}

#[derive(Error, Debug)]
pub enum GraphInitError {
    #[error("Unable to find label '{0}' in target index while walking node index.  This shouldn't be possible.")]
    MissingInternalTargetIndex(String),

    #[error("The target '{0}' lists a dependency that's not in the target index: '{1}")]
    MissingDependencyForTarget(String, String),
}

#[derive(Debug)]
pub struct Graph {
    /// All targets in the build graph (keyed by target label)
    targets: BTreeMap<String, Target>,

    /// The graph of dependencies of all targets.
    dependencies: DiGraph<String, ()>,

    /// Lookup map to find a graph node by it's GN label
    node_lookup: BTreeMap<String, petgraph::graph::NodeIndex>,
}

impl Graph {
    /// Given the parsed GN `desc` output, create the build graph.
    pub fn create_from(all_target_descs: AllTargets) -> Result<Graph, GraphInitError> {
        let graph = Graph {
            targets: BTreeMap::new(),
            dependencies: DiGraph::new(),
            node_lookup: BTreeMap::new(),
        };
        graph.init_with(all_target_descs)
    }

    fn init_with(mut self, all_target_descs: AllTargets) -> Result<Self, GraphInitError> {
        // Create all the nodes, but not their dependency edges, yet (they can be out of order)
        for (label, desc) in all_target_descs {
            let target = Target { label, target_type: desc.target_type.clone(), description: desc };

            // Add the dependency graph node first, so that we can move target into the map afterwards
            let node_index = self.dependencies.add_node(target.label.clone());
            self.node_lookup.insert(target.label.clone(), node_index);

            self.targets.insert(target.label.clone(), target);
        }

        // Now that all the Targets and graph nodes are created, go through all the deps and create
        // the dependency edges in the graph
        for (label, target) in &self.targets {
            let target_index = self
                .node_lookup
                .get(label)
                .ok_or_else(|| GraphInitError::MissingInternalTargetIndex(label.to_owned()))?;

            for dep in &target.description.deps {
                let dep_index = self.node_lookup.get(dep).ok_or_else(|| {
                    GraphInitError::MissingDependencyForTarget(label.to_owned(), dep.to_owned())
                })?;
                self.dependencies.add_edge(*target_index, *dep_index, ());
            }
        }

        Ok(self)
    }

    /// All targets in the build graph (keyed by target label)
    pub fn targets(&self) -> &BTreeMap<String, Target> {
        &self.targets
    }

    /// Return the labels of the dependencies of a target, given the target's label
    pub fn dependencies_for_target(&self, label: &str) -> Option<Vec<&String>> {
        self.deps(label, EdgeDirection::Outgoing)
    }

    /// Return the labels of the targets that depend on a given target's label
    pub fn targets_dependent_on(&self, label: &str) -> Option<Vec<&String>> {
        self.deps(label, EdgeDirection::Incoming)
    }

    fn deps(&self, label: &str, direction: EdgeDirection) -> Option<Vec<&String>> {
        let target_index = self.node_lookup.get(label)?;
        let dep_of = self.dependencies.neighbors_directed(*target_index, direction);
        dep_of.map(|n| self.dependencies.node_weight(n)).collect()
    }

    pub fn edges_count(&self) -> usize {
        self.dependencies.edge_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gn_json::target::TargetDescriptionBuilder;

    #[test]
    fn test_graph_creation() {
        let mut descs = AllTargets::new();
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

        let graph = Graph::create_from(descs).unwrap();

        assert_eq!(4, graph.targets.len());

        let default_target = graph.targets.get("//:default").unwrap();
        assert_eq!(default_target.description, default_description);

        let default_target_deps = graph
            .dependencies_for_target("//:default")
            .expect("dependencies were not found in the graph for '//:default'");
        assert!(default_target_deps.contains(&&"//:a".to_owned()));
        assert!(default_target_deps.contains(&&"//:b".to_owned()));
    }
}
