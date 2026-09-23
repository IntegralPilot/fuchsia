// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
//! # Inspect stats node.
//!
//! Stats installs a lazy node (or a node with a snapshot of the current state of things)
//! reporting stats about the Inspect being served by the component (such as size, number of
//! dynamic children, etc) at the root of the hierarchy.
//!
//! Stats nodes are always at "fuchsia.inspect.Stats".
//!
//! # Examples
//!
//! ```
//! /* This example shows automatically updated stats */
//!
//! use fuchsia_inspect::stats::InspectorExt;
//!
//! let inspector = /* the inspector of your choice */
//! inspector.record_lazy_stats();  // A lazy node containing stats has been installed
//! ```
//!
//! ```
//! /* This example shows stats which must be manually updated */
//! use fuchsia_inspect::stats;
//!
//! let inspector = ...;  // the inspector you want instrumented
//! let stats = stats::StatsNode::new(&inspector);
//! /* do stuff to inspector */
//! /* ... */
//! stats.update();  // update the stats node
//! stats.record_data_to(root);  /* consume the stats node and persist the data */
//! ```

use super::private::InspectTypeInternal;
use super::{Inspector, Node, Property, UintProperty};
use futures::FutureExt;

// The metric node name, as exposed by the stats node.
const FUCHSIA_INSPECT_STATS: &str = "fuchsia.inspect.Stats";
const CURRENT_SIZE_KEY: &str = "current_size";
const MAXIMUM_SIZE_KEY: &str = "maximum_size";
const UTILIZATION_PER_TEN_K_KEY: &str = "utilization_per_ten_k";
const TOTAL_DYNAMIC_CHILDREN_KEY: &str = "total_dynamic_children";
const ALLOCATED_BLOCKS_KEY: &str = "allocated_blocks";
const DEALLOCATED_BLOCKS_KEY: &str = "deallocated_blocks";
const FAILED_ALLOCATIONS_KEY: &str = "failed_allocations";

/// InspectorExt provides a method for installing a "fuchsia.inspect.Stats" lazy node at the root
/// of the Inspector's hierarchy.
pub trait InspectorExt {
    /// Creates a new stats node  that will expose the given `Inspector` stats.
    fn record_lazy_stats(&self);
}

impl InspectorExt for Inspector {
    fn record_lazy_stats(&self) {
        let weak_root_node = self.root().clone_weak();
        self.root().record_lazy_child(FUCHSIA_INSPECT_STATS, move || {
            let weak_root_node = weak_root_node.clone_weak();
            async move {
                let local_inspector = Inspector::default();
                let stats =
                    StatsNode::from_nodes(weak_root_node, local_inspector.root().clone_weak());
                stats.record_data_to(local_inspector.root());
                Ok(local_inspector)
            }
            .boxed()
        });
    }
}

/// Contains information about inspect such as size and number of dynamic children.
#[derive(Default)]
pub struct StatsNode {
    root_of_instrumented_inspector: Node,
    stats_root: Node,
    current_size: UintProperty,
    maximum_size: UintProperty,
    utilization_per_ten_k: UintProperty,
    total_dynamic_children: UintProperty,
    allocated_blocks: UintProperty,
    deallocated_blocks: UintProperty,
    failed_allocations: UintProperty,
}

impl StatsNode {
    /// Takes a snapshot of the stats and writes them to the given parent.
    ///
    /// The returned `StatsNode` is RAII.
    pub fn new(inspector: &Inspector) -> Self {
        let stats_root = inspector.root().create_child(FUCHSIA_INSPECT_STATS);
        Self::from_nodes(inspector.root().clone_weak(), stats_root)
    }

    /// Update the stats with the current state of the Inspector being instrumented.
    pub fn update(&self) {
        if let Some(stats) = self
            .root_of_instrumented_inspector
            .state()
            .and_then(|outer_state| outer_state.try_lock().ok().map(|state| state.stats()))
        {
            // just piggy-backing on current_size; it doesn't matter how the atomic_update
            // is triggered
            self.current_size.atomic_update(|_| {
                self.current_size.set(stats.current_size as u64);
                self.maximum_size.set(stats.maximum_size as u64);
                self.utilization_per_ten_k.set(
                    ((stats.current_size as f64) / (stats.maximum_size as f64) * 10000.0) as u64,
                );
                self.total_dynamic_children.set(stats.total_dynamic_children as u64);
                self.allocated_blocks.set(stats.allocated_blocks as u64);
                self.deallocated_blocks.set(stats.deallocated_blocks as u64);
                self.failed_allocations.set(stats.failed_allocations as u64);
            });
        }
    }

    /// Tie the lifetime of the statistics to the provided `fuchsia_inspect::Node`.
    pub fn record_data_to(self, lifetime: &Node) {
        lifetime.record(self.stats_root);
        lifetime.record(self.current_size);
        lifetime.record(self.maximum_size);
        lifetime.record(self.utilization_per_ten_k);
        lifetime.record(self.total_dynamic_children);
        lifetime.record(self.allocated_blocks);
        lifetime.record(self.deallocated_blocks);
        lifetime.record(self.failed_allocations);
    }

    /// Write stats from the state of `root_of_instrumented_inspector` to `stats_root`
    fn from_nodes(root_of_instrumented_inspector: Node, stats_root: Node) -> Self {
        if let Some(stats) = root_of_instrumented_inspector
            .state()
            .and_then(|outer_state| outer_state.try_lock().ok().map(|state| state.stats()))
        {
            let mut n = stats_root.atomic_update(|stats_root| StatsNode {
                root_of_instrumented_inspector,
                current_size: stats_root.create_uint(CURRENT_SIZE_KEY, stats.current_size as u64),
                maximum_size: stats_root.create_uint(MAXIMUM_SIZE_KEY, stats.maximum_size as u64),
                utilization_per_ten_k: stats_root.create_uint(
                    UTILIZATION_PER_TEN_K_KEY,
                    ((stats.current_size as f64) / (stats.maximum_size as f64) * 10000.0) as u64,
                ),
                total_dynamic_children: stats_root
                    .create_uint(TOTAL_DYNAMIC_CHILDREN_KEY, stats.total_dynamic_children as u64),
                allocated_blocks: stats_root
                    .create_uint(ALLOCATED_BLOCKS_KEY, stats.allocated_blocks as u64),
                deallocated_blocks: stats_root
                    .create_uint(DEALLOCATED_BLOCKS_KEY, stats.deallocated_blocks as u64),
                failed_allocations: stats_root
                    .create_uint(FAILED_ALLOCATIONS_KEY, stats.failed_allocations as u64),
                ..StatsNode::default()
            });
            n.stats_root = stats_root;
            n
        } else {
            StatsNode { root_of_instrumented_inspector, ..StatsNode::default() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{assert_data_tree, assert_json_diff};
    use inspect_format::constants;

    #[fuchsia::test]
    async fn inspect_stats() {
        let inspector = Inspector::default();
        inspector.record_lazy_stats();

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Stats": {
                current_size: 4096u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 156u64,
                total_dynamic_children: 1u64,  // snapshot was taken before adding any lazy node.
                allocated_blocks: 4u64,
                deallocated_blocks: 0u64,
                failed_allocations: 0u64,
            },
        });

        inspector.root().record_lazy_child("foo", || {
            async move {
                let inspector = Inspector::default();
                inspector.root().record_uint("a", 1);
                Ok(inspector)
            }
            .boxed()
        });
        assert_data_tree!(inspector, root: {
            foo: {
                a: 1u64,
            },
            "fuchsia.inspect.Stats": {
                current_size: 4096u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 156u64,
                total_dynamic_children: 2u64,
                allocated_blocks: 7u64,
                deallocated_blocks: 0u64,
                failed_allocations: 0u64,
            },
        });

        for i in 0..100 {
            inspector.root().record_string(format!("testing-{i}"), "testing".repeat(i + 1));
        }

        {
            let _ = inspector.root().create_int("drop", 1);
        }

        assert_data_tree!(inspector, root: contains {
            "fuchsia.inspect.Stats": {
                current_size: 61440u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 2343u64,
                total_dynamic_children: 2u64,
                allocated_blocks: 309u64,
                // 2 blocks are deallocated because of the "drop" int block and its
                // STRING_REFERENCE
                deallocated_blocks: 2u64,
                failed_allocations: 0u64,
            }
        });

        for i in 101..220 {
            inspector.root().record_string(format!("testing-{i}"), "testing".repeat(i + 1));
        }

        assert_data_tree!(inspector, root: contains {
            "fuchsia.inspect.Stats": {
                current_size: 262144u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 10000u64,
                total_dynamic_children: 2u64,
                allocated_blocks: 664u64,
                deallocated_blocks: 6u64,
                failed_allocations: 2u64,
            }
        });
    }

    #[fuchsia::test]
    async fn stats_are_updated() {
        let inspector = Inspector::default();
        let stats = super::StatsNode::new(&inspector);
        assert_json_diff!(inspector, root: {
            "fuchsia.inspect.Stats": {
                current_size: 4096u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 156u64,
                total_dynamic_children: 0u64,
                allocated_blocks: 3u64,
                deallocated_blocks: 0u64,
                failed_allocations: 0u64,
            }
        });

        inspector.root().record_int("abc", 5);

        // asserting that everything is the same, since we didn't call `stats.update()`
        assert_json_diff!(inspector, root: {
            abc: 5i64,
            "fuchsia.inspect.Stats": {
                current_size: 4096u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 156u64,
                total_dynamic_children: 0u64,
                allocated_blocks: 3u64,
                deallocated_blocks: 0u64,
                failed_allocations: 0u64,
            }
        });

        stats.update();

        assert_json_diff!(inspector, root: {
            abc: 5i64,
            "fuchsia.inspect.Stats": {
                current_size: 4096u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 156u64,
                total_dynamic_children: 0u64,
                allocated_blocks: 19u64,
                deallocated_blocks: 0u64,
                failed_allocations: 0u64,
            }
        });
    }

    #[fuchsia::test]
    async fn recorded_stats_are_persisted() {
        let inspector = Inspector::default();
        {
            let stats = super::StatsNode::new(&inspector);
            stats.record_data_to(inspector.root());
        }

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Stats": {
                current_size: 4096u64,
                maximum_size: constants::DEFAULT_VMO_SIZE_BYTES as u64,
                utilization_per_ten_k: 156u64,
                total_dynamic_children: 0u64,
                allocated_blocks: 3u64,
                deallocated_blocks: 0u64,
                failed_allocations: 0u64,
            }
        });
    }
}
