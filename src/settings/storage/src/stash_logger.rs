// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::{self as inspect, Node, NumericProperty};
use fuchsia_inspect_derive::Inspect;
use settings_inspect_utils::managed_inspect_map::ManagedInspectMap;

const STASH_INSPECT_NODE_NAME: &str = "stash_failures";

pub struct StashInspectLogger {
    /// Map from a setting's device storage key to its inspect data.
    flush_failure_counts: ManagedInspectMap<StashInspectInfo>,
}

/// Contains the node and property used to record the number of stash failures for a given
/// setting.
///
/// Inspect nodes are not used, but need to be held as they're deleted from inspect once they go
/// out of scope.
#[derive(Default, Inspect)]
struct StashInspectInfo {
    /// Node of this info.
    inspect_node: inspect::Node,

    /// Number of write failures.
    count: inspect::UintProperty,
}

impl StashInspectLogger {
    pub fn new(node: &Node) -> Self {
        let inspect_node = node.create_child(STASH_INSPECT_NODE_NAME);
        Self {
            flush_failure_counts: ManagedInspectMap::<StashInspectInfo>::with_node(inspect_node),
        }
    }

    /// Records a write failure for the given setting.
    pub fn record_flush_failure(&mut self, key: String) {
        let stash_inspect_info =
            self.flush_failure_counts.get_or_insert_with(key, StashInspectInfo::default);
        let _ = stash_inspect_info.count.add(1u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::component;

    // Verify that the StashInspectLogger accumulates failure counts to inspect.
    #[fuchsia::test]
    async fn test_stash_logger() {
        let inspector = component::inspector();
        let mut logger = StashInspectLogger::new(inspector.root());

        logger.record_flush_failure("test_key".to_string());
        logger.record_flush_failure("test_key2".to_string());
        logger.record_flush_failure("test_key2".to_string());

        assert_data_tree!(inspector, root: {
            stash_failures: {
                "test_key": {
                    "count": 1u64,
                },
                "test_key2": {
                    "count": 2u64,
                }
            }
        });
    }
}
