// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_inspect::{HistogramProperty, Inspector, LazyNode, Node, NumericProperty};
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use std::sync::LazyLock;

/// Holds a histogram of latencies and a counter for the cumulative time.
pub struct DurationMeasure {
    latency: fuchsia_inspect::UintExponentialHistogramProperty,
    time: fuchsia_inspect::UintProperty,
}

impl DurationMeasure {
    pub fn new(node: &Node, name: &str) -> Self {
        Self {
            latency: node.create_uint_exponential_histogram(
                name.to_owned() + "_latency_ns",
                Self::latency_histogram_params(),
            ),
            time: node.create_uint(name.to_owned() + "_time_ns", 0),
        }
    }
    fn latency_histogram_params() -> fuchsia_inspect::ExponentialHistogramParams<u64> {
        fuchsia_inspect::ExponentialHistogramParams {
            floor: 0,
            initial_step: 100_000, // 100us
            step_multiplier: 2,
            buckets: 30,
        }
    }
}

/// A scope that measures the duration of an operation and records it in a `DurationMeasure`.
/// When the scope is dropped, the duration is added to the histogram and the cumulative time is
/// incremented.
pub struct DurationMeasureScope<'a> {
    start_time: std::time::Instant,
    measure: &'a DurationMeasure,
}

impl<'a> DurationMeasureScope<'a> {
    pub fn new(measure: &'a DurationMeasure) -> Self {
        Self { start_time: std::time::Instant::now(), measure }
    }
}

impl<'a> Drop for DurationMeasureScope<'a> {
    fn drop(&mut self) {
        let elapsed = self.start_time.elapsed().as_nanos() as u64;
        self.measure.latency.insert(elapsed);
        self.measure.time.add(elapsed);
    }
}

/// Root node to which the filesystem Inspect tree will be attached.
fn root() -> Node {
    #[cfg(target_os = "fuchsia")]
    static FXFS_ROOT_NODE: LazyLock<Mutex<fuchsia_inspect::Node>> =
        LazyLock::new(|| Mutex::new(fuchsia_inspect::component::inspector().root().clone_weak()));
    #[cfg(not(target_os = "fuchsia"))]
    static FXFS_ROOT_NODE: LazyLock<Mutex<Node>> = LazyLock::new(|| Mutex::new(Node::default()));

    FXFS_ROOT_NODE.lock().clone_weak()
}

/// `fs.detail` node for holding fxfs-specific metrics.
pub fn detail() -> Node {
    static DETAIL_NODE: LazyLock<Mutex<Node>> =
        LazyLock::new(|| Mutex::new(root().create_child("fs.detail")));

    DETAIL_NODE.lock().clone_weak()
}
pub fn register_fs(
    populate_stores_fn: impl Fn() -> BoxFuture<'static, Result<Inspector, Error>>
    + Sync
    + Send
    + 'static,
) -> LazyNode {
    root().create_lazy_child("stores", populate_stores_fn)
}

pub struct LsmTreeMetrics {
    pub find: DurationMeasure,
    pub insert: DurationMeasure,
    pub replace_or_insert: DurationMeasure,
    pub merge_into: DurationMeasure,
    pub compaction_layer_stack_depth: fuchsia_inspect::UintExponentialHistogramProperty,
    pub journal_compactions_total: fuchsia_inspect::UintProperty,
    pub journal_compaction_bytes_written: fuchsia_inspect::UintProperty,
    pub journal_compaction_time: DurationMeasure,
    _node: fuchsia_inspect::Node,
}

pub fn lsm_tree_metrics() -> &'static LsmTreeMetrics {
    static METRICS: std::sync::LazyLock<LsmTreeMetrics> = std::sync::LazyLock::new(|| {
        let node = detail().create_child("lsm_tree");
        LsmTreeMetrics {
            find: DurationMeasure::new(&node, "find"),
            insert: DurationMeasure::new(&node, "insert"),
            replace_or_insert: DurationMeasure::new(&node, "replace_or_insert"),
            merge_into: DurationMeasure::new(&node, "merge_into"),
            compaction_layer_stack_depth: node.create_uint_exponential_histogram(
                "compaction_layer_stack_depth",
                fuchsia_inspect::ExponentialHistogramParams {
                    floor: 0,
                    initial_step: 1,
                    step_multiplier: 2,
                    buckets: 8,
                },
            ),
            journal_compactions_total: node.create_uint("journal_compactions_total", 0),
            journal_compaction_bytes_written: node
                .create_uint("journal_compaction_bytes_written", 0),
            journal_compaction_time: DurationMeasure::new(&node, "journal_compaction"),
            _node: node,
        }
    });
    &METRICS
}

pub struct DirectoryMetrics {
    pub lookup: DurationMeasure,
    _node: fuchsia_inspect::Node,
}

pub fn directory_metrics() -> &'static DirectoryMetrics {
    static METRICS: std::sync::LazyLock<DirectoryMetrics> = std::sync::LazyLock::new(|| {
        let node = detail().create_child("directory");
        DirectoryMetrics { lookup: DurationMeasure::new(&node, "lookup"), _node: node }
    });
    &METRICS
}
