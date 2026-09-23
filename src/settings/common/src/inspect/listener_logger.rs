// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::{self as inspect, component, NumericProperty};
use fuchsia_inspect_derive::Inspect;
use settings_inspect_utils::managed_inspect_map::ManagedInspectMap;
use std::cell::RefCell;

const LISTENER_INSPECT_NODE_NAME: &str = "active_listeners";

pub struct ListenerInspectLogger {
    /// The saved information about each setting type's active listeners.
    listener_counts: RefCell<ManagedInspectMap<ListenerInspectInfo>>,
}

impl Default for ListenerInspectLogger {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about active listeners to be written to inspect.
///
/// Inspect nodes are not used, but need to be held as they're deleted from inspect once they go
/// out of scope.
#[derive(Default, Inspect)]
struct ListenerInspectInfo {
    /// Node of this info.
    inspect_node: inspect::Node,

    /// Number of active listeners.
    count: inspect::UintProperty,
}

impl ListenerInspectLogger {
    /// Creates a new [ListenerInspectLogger] that writes to the default
    /// [fuchsia_inspect::component::inspector()].
    pub fn new() -> Self {
        Self::with_inspector(component::inspector())
    }

    pub fn with_inspector(inspector: &inspect::Inspector) -> Self {
        let listener_counts_node = inspector.root().create_child(LISTENER_INSPECT_NODE_NAME);
        Self {
            listener_counts: RefCell::new(ManagedInspectMap::<ListenerInspectInfo>::with_node(
                listener_counts_node,
            )),
        }
    }

    /// Adds a listener to the count for [setting_type].
    pub fn add_listener(&self, setting_type: String) {
        let mut listener_counts = self.listener_counts.borrow_mut();
        let inspect_info =
            listener_counts.get_or_insert_with(setting_type, ListenerInspectInfo::default);
        let _ = inspect_info.count.add(1u64);
    }

    /// Removes a listener from the count for [setting_type].
    pub fn remove_listener(&self, setting_type: String) {
        let mut listener_counts = self.listener_counts.borrow_mut();
        match listener_counts.map_mut().get_mut(&setting_type) {
            Some(listener_inspect_info) => {
                let _ = listener_inspect_info.count.subtract(1u64);
            }
            None => log::error!("Tried to subtract from nonexistent listener count"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;

    #[fuchsia::test]
    async fn test_listener_logger() {
        let inspector = inspect::Inspector::default();

        let logger = ListenerInspectLogger::with_inspector(&inspector);

        logger.add_listener("Unknown".into());
        logger.add_listener("Unknown".into());
        logger.add_listener("Unknown".into());

        logger.remove_listener("Unknown".into());

        // Since listeners were added thrice and removed once, the count at the end is 2.
        assert_data_tree!(inspector, root: {
            active_listeners: {
                Unknown: {
                    "count": 2u64,
                }
            }
        });
    }
}
