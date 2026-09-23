// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::VecDeque;

use fuchsia_inspect::{Node, NumericProperty, UintProperty};
use fuchsia_inspect_derive::Unit;

use crate::nodes::{BootTimeProperty, NodeTimeExt};

/// The name of the property that holds the time at which the log entry was created.
/// This property is used by toolings and should not be changed without considering
/// how it affects toolings.
const CREATED_AT_PROPERTY_NAME: &str = "Created@time";
/// The name of the property that holds the time at which the log entry was last seen.
/// This property is used by toolings and should not be changed without considering
/// how it affects toolings.
const LAST_SEEN_AT_PROPERTY_NAME: &str = "LastSeen@time";

struct EntryData<T: Unit> {
    _node: Node,
    count: UintProperty,
    last_seen: Option<BootTimeProperty>,
    _time: BootTimeProperty,
    _log: T::Data,
}

/// A Inspect node that holds an ordered, bounded list of log entries, with
/// the ability to dedupe consecutive logs.
pub struct DedupeLogNode<T: Unit + PartialEq> {
    node: Node,
    last_log: Option<T>,
    entries: VecDeque<EntryData<T>>,
    next_index: u64,
    capacity: usize,
}

impl<T: Unit + PartialEq> DedupeLogNode<T> {
    pub fn new(node: Node, capacity: usize) -> Self {
        let capacity = std::cmp::max(capacity, 1);
        Self {
            node,
            last_log: None,
            entries: VecDeque::with_capacity(capacity),
            next_index: 0,
            capacity,
        }
    }

    /// Insert |log| into `DedupeLogNode`.
    ///
    /// If |log| is equivalent to the last inserted log, the last inserted log
    /// entry's count is incremented and last seen time is updated. If the new
    /// log is not equivalent to the last inserted log, a new log entry is
    /// inserted, and the oldest log entry is removed if capacity is exceeded.
    pub fn insert(&mut self, log: T) {
        if self.last_log.as_ref() == Some(&log) {
            if let Some(entry) = self.entries.back_mut() {
                entry.count.add(1);
                if let Some(time_prop) = &entry.last_seen {
                    time_prop.update();
                } else {
                    let now = zx::BootInstant::get();
                    entry.last_seen =
                        Some(entry._node.create_time_at(LAST_SEEN_AT_PROPERTY_NAME, now));
                }
            }
            return;
        }

        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }

        let index_str = self.next_index.to_string();
        self.next_index += 1;

        let child_node = self.node.create_child(&index_str);

        let now = zx::BootInstant::get();
        let time_prop = child_node.create_time_at(CREATED_AT_PROPERTY_NAME, now);
        let count_prop = child_node.create_uint("count", 1);
        let log_data = log.inspect_create(&child_node, "log");

        self.entries.push_back(EntryData {
            _node: child_node,
            count: count_prop,
            last_seen: None,
            _log: log_data,
            _time: time_prop,
        });

        self.last_log = Some(log);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyNumericProperty, assert_data_tree};
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn test_insert_unique_items() {
        let inspector = Inspector::default();
        let log_node = inspector.root().create_child("log");
        let mut dedupe_log = DedupeLogNode::new(log_node, 3);

        dedupe_log.insert(111);
        dedupe_log.insert(222);
        dedupe_log.insert(333);

        assert_data_tree!(inspector, root: {
            log: {
                "0": {
                    "Created@time": AnyNumericProperty,
                    count: 1u64,
                    log: 111i64,
                },
                "1": {
                    "Created@time": AnyNumericProperty,
                    count: 1u64,
                    log: 222i64,
                },
                "2": {
                    "Created@time": AnyNumericProperty,
                    count: 1u64,
                    log: 333i64,
                },
            }
        });
    }

    #[fuchsia::test]
    async fn test_deduplication() {
        let inspector = Inspector::default();
        let log_node = inspector.root().create_child("log");
        let mut dedupe_log = DedupeLogNode::new(log_node, 3);

        dedupe_log.insert(111);
        dedupe_log.insert(111);
        dedupe_log.insert(111);

        assert_data_tree!(inspector, root: {
            log: {
                "0": {
                    "Created@time": AnyNumericProperty,
                    "LastSeen@time": AnyNumericProperty,
                    count: 3u64,
                    log: 111i64,
                },
            }
        });

        // Insert a new item, then another dedupe
        dedupe_log.insert(222);
        dedupe_log.insert(222);

        assert_data_tree!(inspector, root: {
            log: {
                "0": {
                    "Created@time": AnyNumericProperty,
                    "LastSeen@time": AnyNumericProperty,
                    count: 3u64,
                    log: 111i64,
                },
                "1": {
                    "Created@time": AnyNumericProperty,
                    "LastSeen@time": AnyNumericProperty,
                    count: 2u64,
                    log: 222i64,
                },
            }
        });
    }

    #[fuchsia::test]
    async fn test_eviction() {
        let inspector = Inspector::default();
        let log_node = inspector.root().create_child("log");
        let mut dedupe_log = DedupeLogNode::new(log_node, 2);

        dedupe_log.insert(111);
        dedupe_log.insert(222);
        dedupe_log.insert(333); // This will evict 111

        assert_data_tree!(inspector, root: {
            log: {
                "1": {
                    "Created@time": AnyNumericProperty,
                    count: 1u64,
                    log: 222i64,
                },
                "2": {
                    "Created@time": AnyNumericProperty,
                    count: 1u64,
                    log: 333i64,
                },
            }
        });
    }

    #[derive(PartialEq, Unit)]
    struct Item {
        num: u64,
        string: String,
    }

    #[fuchsia::test]
    async fn test_insert_custom_struct() {
        let inspector = Inspector::default();
        let log_node = inspector.root().create_child("log");
        let mut dedupe_log = DedupeLogNode::new(log_node, 3);

        dedupe_log.insert(Item { num: 1337u64, string: "42".to_string() });
        dedupe_log.insert(Item { num: 1337u64, string: "42".to_string() }); // Deduplicates

        assert_data_tree!(inspector, root: {
            log: {
                "0": {
                    "Created@time": AnyNumericProperty,
                    "LastSeen@time": AnyNumericProperty,
                    count: 2u64,
                    log: { num: 1337u64, string: "42".to_string() }
                },
            }
        });
    }
}
