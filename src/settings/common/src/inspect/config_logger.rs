// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{clock, config};
use fuchsia_inspect::{self as inspect, Node, NumericProperty, Property};
use fuchsia_inspect_derive::Inspect;
use settings_inspect_utils::managed_inspect_map::ManagedInspectMap;

const CONFIG_INSPECT_NODE_NAME: &str = "config_loads";

pub struct InspectConfigLogger {
    /// The saved information about each load.
    config_load_values: ManagedInspectMap<ConfigInspectInfo>,
}

/// Information about a config file load to be written to inspect.
///
/// Inspect nodes are not used, but need to be held as they're deleted from inspect once they go
/// out of scope.
#[derive(Default, Inspect)]
struct ConfigInspectInfo {
    /// Node of this info.
    inspect_node: inspect::Node,

    /// Nanoseconds since boot that this config was loaded.
    timestamp: inspect::StringProperty,

    /// Number of times the config was loaded.
    count: inspect::UintProperty,

    /// Debug string representation of the value of this config load info.
    value: inspect::StringProperty,

    /// Counts of different results for this config's load attempts.
    result_counts: ManagedInspectMap<inspect::UintProperty>,
}

impl InspectConfigLogger {
    /// Creates a new [InspectConfigLogger] that writes to the default
    /// [fuchsia_inspect::component::inspector()].
    pub fn new(node: &Node) -> Self {
        let config_inspect_node = node.create_child(CONFIG_INSPECT_NODE_NAME);
        Self {
            config_load_values: ManagedInspectMap::<ConfigInspectInfo>::with_node(
                config_inspect_node,
            ),
        }
    }

    pub fn write_config_load_to_inspect(
        &mut self,
        path: String,
        config_load_info: config::ConfigLoadInfo,
    ) {
        let timestamp = clock::inspect_format_now();
        let config::ConfigLoadInfo { status, contents } = config_load_info;
        let status_clone = status.clone();

        let config_inspect_info =
            self.config_load_values.get_or_insert_with(path, ConfigInspectInfo::default);

        config_inspect_info.timestamp.set(&timestamp);
        config_inspect_info
            .value
            .set(&format!("{:#?}", config::ConfigLoadInfo { status, contents }));
        let _ = config_inspect_info.count.add(1u64);
        let _ = config_inspect_info
            .result_counts
            .get_or_insert_with(status_clone.into(), inspect::UintProperty::default)
            .add(1u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLoadStatus;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::component;

    #[fuchsia::test]
    async fn test_listener_logger() {
        // Set clock for consistent timestamps.
        clock::mock::set(zx::MonotonicInstant::from_nanos(0));

        let inspector = component::inspector();
        let mut logger = InspectConfigLogger::new(inspector.root());

        logger.write_config_load_to_inspect(
            "test_path".to_string(),
            config::ConfigLoadInfo {
                status: ConfigLoadStatus::Success,
                contents: Some("test".to_string()),
            },
        );

        assert_data_tree!(inspector, root: {
            config_loads: {
                "test_path": {
                    "count": 1u64,
                    "result_counts": {
                        "Success": 1u64,
                    },
                    "timestamp": "0.000000000",
                    "value": "ConfigLoadInfo {\n    status: Success,\n    contents: Some(\n        \"test\",\n    ),\n}"
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_response_counts() {
        // Set clock for consistent timestamps.
        clock::mock::set(zx::MonotonicInstant::from_nanos(0));

        let inspector = component::inspector();
        let mut logger = InspectConfigLogger::new(inspector.root());

        logger.write_config_load_to_inspect(
            "test_path".to_string(),
            config::ConfigLoadInfo {
                status: ConfigLoadStatus::Success,
                contents: Some("test".to_string()),
            },
        );
        logger.write_config_load_to_inspect(
            "test_path".to_string(),
            config::ConfigLoadInfo {
                status: ConfigLoadStatus::ParseFailure("Fake parse failure".to_string()),
                contents: Some("test".to_string()),
            },
        );
        logger.write_config_load_to_inspect(
            "test_path".to_string(),
            config::ConfigLoadInfo {
                status: ConfigLoadStatus::ParseFailure("Fake parse failure".to_string()),
                contents: Some("test".to_string()),
            },
        );
        logger.write_config_load_to_inspect(
            "test_path".to_string(),
            config::ConfigLoadInfo {
                status: ConfigLoadStatus::UsingDefaults("default".to_string()),
                contents: Some("test".to_string()),
            },
        );

        assert_data_tree!(inspector, root: {
            config_loads: {
                "test_path": {
                    "count": 4u64,
                    "result_counts": {
                        "Success": 1u64,
                        "ParseFailure": 2u64,
                        "UsingDefaults": 1u64,
                    },
                    "timestamp": "0.000000000",
                    "value": "ConfigLoadInfo {\n    status: UsingDefaults(\n        \"default\",\n    ),\n    contents: Some(\n        \"test\",\n    ),\n}"
                }
            }
        });
    }
}
