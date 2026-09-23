// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeSet;
use std::fmt::Display;

use argh::FromArgs;
use gn_graph::{Graph, Target};

/// display a list of found targets
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "all_deps")]
pub struct AllDepsArgs {
    /// the depth of dependencies to traverse
    #[argh(option)]
    pub depth: Option<u32>,
}

impl AllDepsArgs {
    /// List all deps of the target, to the given depth
    pub fn perform(&self, target: &Target, graph: &Graph) -> Result<(), anyhow::Error> {
        let deps = AllDeps::gather_deps(target, graph, self.depth);
        println!("{}", deps);
        Ok(())
    }
}

struct AllDeps {
    deps: BTreeSet<String>,
}

impl AllDeps {
    fn gather_deps(target: &Target, graph: &Graph, depth: Option<u32>) -> Self {
        let mut deps = Self { deps: BTreeSet::default() };
        deps.gather_deps_impl(target, graph, depth);
        deps
    }

    fn gather_deps_impl(&mut self, target: &Target, graph: &Graph, depth: Option<u32>) {
        let keep_going = match depth {
            None => true,
            Some(v) => v > 1,
        };

        for dep in &target.description.deps {
            if !self.deps.contains(dep) {
                self.deps.insert(dep.clone());
                if keep_going {
                    if let Some(dep_target) = graph.targets().get(dep) {
                        self.gather_deps_impl(dep_target, graph, depth.map(|v| v - 1))
                    }
                }
            }
        }
    }
}

impl Display for AllDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for dep in &self.deps {
            writeln!(f, "{}", dep)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use gn_json::target::{AllTargets, TargetDescriptionBuilder};

    fn make_test_graph() -> Graph {
        let mut all_target_descs = AllTargets::new();

        all_target_descs.insert(
            "//:default".to_string(),
            TargetDescriptionBuilder::group().dep("//:foo").dep("//:bar").build(),
        );

        // Add a diamond-dep edge to make sure that's not in twice.
        all_target_descs
            .insert("//:foo".to_string(), TargetDescriptionBuilder::group().dep("//:bar").build());

        all_target_descs
            .insert("//:bar".to_string(), TargetDescriptionBuilder::group().dep("//:baz").build());

        all_target_descs.insert(
            "//:baz".to_string(),
            TargetDescriptionBuilder::action("scripts/baz.py").dep("//:baz").build(),
        );

        Graph::create_from(all_target_descs).unwrap()
    }

    #[test]
    fn test_all_deps() {
        let graph = make_test_graph();

        let default_target = graph.targets().get("//:default").unwrap();
        let all_deps = AllDeps::gather_deps(default_target, &graph, None);

        let output = format!("{}", all_deps);

        assert_eq!(output, "//:bar\n//:baz\n//:foo\n");
    }

    #[test]
    fn test_depth_limit() {
        let graph = make_test_graph();

        let default_target = graph.targets().get("//:default").unwrap();
        let all_deps = AllDeps::gather_deps(default_target, &graph, Some(2));

        let output = format!("{}", all_deps);

        assert_eq!(output, "//:bar\n//:foo\n");
    }

    #[test]
    fn test_start_not_default() {
        let graph = make_test_graph();

        let default_target = graph.targets().get("//:foo").unwrap();
        let all_deps = AllDeps::gather_deps(default_target, &graph, None);

        let output = format!("{}", all_deps);

        assert_eq!(output, "//:bar\n//:baz\n");
    }
}
