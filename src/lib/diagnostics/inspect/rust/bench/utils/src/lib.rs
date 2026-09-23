// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_inspect::{TreeMarker, TreeProxy};
use fuchsia_async::{Scope, Task};
use fuchsia_criterion::{FuchsiaCriterion, criterion};
use fuchsia_inspect::Inspector;
use fuchsia_inspect::hierarchy::{DiagnosticsHierarchy, DiagnosticsHierarchyGetter};
use futures::{FutureExt, future};
use inspect_runtime::TreeServerSendPreference;
use inspect_runtime::service::handle_request_stream;
use rand::distr::Uniform;
use rand::rngs::StdRng;
use rand::{Rng, RngExt as _, SeedableRng};
use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::future::IntoFuture;
use std::time::Duration;

/// Spawns a tree server for the test purposes.
pub fn spawn_server(
    inspector: Inspector,
) -> Result<(TreeProxy, Task<Result<(), anyhow::Error>>), anyhow::Error> {
    let (tree, request_stream) = fidl::endpoints::create_proxy_and_stream::<TreeMarker>();
    let scope = Scope::new_with_name("tree_server_scope");
    let tree_server_task = scope.compute(handle_request_stream(
        inspector,
        TreeServerSendPreference::default(),
        request_stream,
        scope.new_child_with_name("tree_server"),
    ));
    Ok((tree, Task::spawn(future::join(tree_server_task, scope.into_future()).map(|(r, _)| r))))
}

pub struct CriterionConfig {
    pub warm_up_time: Duration,
    pub measurement_time: Duration,
    pub sample_size: usize,
}

impl Default for CriterionConfig {
    fn default() -> CriterionConfig {
        CriterionConfig {
            warm_up_time: Duration::from_millis(1),
            measurement_time: Duration::from_millis(100),
            sample_size: 10,
        }
    }
}

pub fn configured_criterion(config: CriterionConfig) -> FuchsiaCriterion {
    let CriterionConfig { warm_up_time, measurement_time, sample_size } = config;
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut criterion::Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(warm_up_time)
        .measurement_time(measurement_time)
        // We must reduce the sample size from the default of 100, otherwise
        // Criterion will sometimes override the 1ms + 500ms suggested times
        // and run for much longer.
        .sample_size(sample_size);
    c
}

pub struct InspectHierarchyGenerator<R: SeedableRng + Rng> {
    rng: R,
    pub inspector: Inspector,
}

impl<R: SeedableRng + Rng> InspectHierarchyGenerator<R> {
    pub fn new(rng: R, inspector: Inspector) -> Self {
        Self { rng, inspector }
    }

    /// Generates prufer sequence by sampling uniformly from the rng.
    fn generate_prufer_sequence(&mut self, n: usize) -> Vec<usize> {
        (0..n - 2).map(|_| self.rng.sample(Uniform::new(0, n).unwrap())).collect()
    }

    /// Generate hierarchy from prufer sequence
    fn generate_hierarchy_from_prufer(&mut self, sequence: &[usize]) {
        let n = sequence.len() + 2;
        let mut degree = vec![1; n];

        for node in sequence {
            degree[*node] += 1;
        }

        let mut ptr = 0;
        while ptr < n && degree[ptr] != 1 {
            ptr += 1;
        }
        let mut leaf = ptr;

        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        for v in sequence {
            edges[leaf].push(*v);
            edges[*v].push(leaf);

            degree[*v] -= 1;
            if degree[*v] == 1 && *v < ptr {
                leaf = *v;
            } else {
                ptr += 1;
                while ptr < n && degree[ptr] != 1 {
                    ptr += 1;
                }
                leaf = ptr;
            }
        }
        edges[leaf].push(n - 1);
        edges[n - 1].push(leaf);

        // Now we have the tree in undirectred form
        // we will construct the inspect hierarchy assuming 0 to be
        // inspector root
        let mut unprocessed_nodes = VecDeque::new();
        unprocessed_nodes.push_back((0, self.inspector.root().clone_weak()));

        let mut processed_edges: HashSet<(usize, usize)> = HashSet::with_capacity(n - 1);

        while let Some((u, node)) = unprocessed_nodes.pop_front() {
            for v in &edges[u] {
                // Already processed v -> u
                if processed_edges.contains(&(*v, u)) {
                    continue;
                }
                // Processed edge from u -> v
                processed_edges.insert((u, *v));

                let child_node = node.create_child(format!("child_{}", *v));

                // Add IntProperty to the child node
                child_node.record_uint("value", *v as u64);
                unprocessed_nodes.push_back((*v, child_node.clone_weak()));
                node.record(child_node);
            }
        }
    }

    /// Generate random inspect hierarchy with n nodes.
    pub fn generate_hierarchy(&mut self, n: usize) {
        let prufer_sequence = self.generate_prufer_sequence(n);
        self.generate_hierarchy_from_prufer(&prufer_sequence);
    }
}

impl<R: SeedableRng + Rng> DiagnosticsHierarchyGetter<String> for InspectHierarchyGenerator<R> {
    async fn get_diagnostics_hierarchy<'a>(&'a self) -> Cow<'_, DiagnosticsHierarchy>
    where
        String: 'a,
    {
        self.inspector.get_diagnostics_hierarchy().await
    }
}

/// [size] must be >1.
pub fn filled_hierarchy_generator(seed: u64, size: usize) -> InspectHierarchyGenerator<StdRng> {
    assert!(size > 1, "Generator requires a size greater than 1");
    let inspector = Inspector::default();
    let mut hierarchy_generator =
        InspectHierarchyGenerator::new(StdRng::seed_from_u64(seed), inspector);
    hierarchy_generator.generate_hierarchy(size);
    hierarchy_generator
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_json_diff;

    #[fuchsia::test]
    async fn random_generated_hierarchy_is_reproducible() {
        let hierarchy_generator = filled_hierarchy_generator(0, 10);
        assert_json_diff!(
        hierarchy_generator,
        root: {
          child_6: {
            value: 6,
            child_5: {
              value: 5,
              child_3: {
                value: 3
              },
              child_9: {
                value: 9
              }
            }
          },
          child_8: {
            value: 8,
            child_1: {
              value: 1
            },
            child_7: {
              value: 7,
              child_2: {
                value: 2
              },
              child_4: {
                value: 4
              }
            }
          }
        });
    }
}
