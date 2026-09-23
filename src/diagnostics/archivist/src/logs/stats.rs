// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::identity::ComponentIdentity;
use diagnostics_data::Severity;
use flyweights::FlyStr;
use fuchsia_inspect::{
    ArrayProperty, Inspector, IntArrayProperty, LazyNode, Node, UintArrayProperty,
};
use fuchsia_sync::Mutex;
use futures::FutureExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Debug, Default)]
struct LogCounter {
    number: AtomicU64,
    bytes: AtomicU64,
}

impl LogCounter {
    fn count(&self, bytes: usize) {
        self.number.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    fn increment_bytes(&self, bytes: usize) {
        self.number.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
struct InnerStats {
    sockets_opened: AtomicU64,
    sockets_closed: AtomicU64,
    last_timestamp: AtomicI64,
    total: LogCounter,
    rolled_out: LogCounter,
    fatal: LogCounter,
    error: LogCounter,
    warn: LogCounter,
    info: LogCounter,
    debug: LogCounter,
    trace: LogCounter,
    url: FlyStr,
    invalid: LogCounter,
}

#[derive(Debug)]
pub struct LogStreamStats {
    inner: Arc<InnerStats>,
    _lazy_node: LazyNode,
}

impl LogStreamStats {
    pub fn new(parent: &Node, identity: &ComponentIdentity) -> Self {
        let inner = Arc::new(InnerStats { url: identity.url.clone(), ..Default::default() });
        let inner_clone = Arc::clone(&inner);
        let lazy_node = parent.create_lazy_child(identity.moniker.to_string(), move || {
            let inner = Arc::clone(&inner_clone);
            async move {
                let inspector = Inspector::default();
                let root = inspector.root();
                root.record_uint("sockets_opened", inner.sockets_opened.load(Ordering::Relaxed));
                root.record_uint("sockets_closed", inner.sockets_closed.load(Ordering::Relaxed));
                root.record_int("last_timestamp", inner.last_timestamp.load(Ordering::Relaxed));
                root.record_string("url", inner.url.as_str());

                let record_counter = |name: &str, counter: &LogCounter| {
                    let child = root.create_child(name);
                    child.record_uint("number", counter.number.load(Ordering::Relaxed));
                    child.record_uint("bytes", counter.bytes.load(Ordering::Relaxed));
                    root.record(child);
                };

                record_counter("total", &inner.total);
                record_counter("rolled_out", &inner.rolled_out);
                record_counter("fatal", &inner.fatal);
                record_counter("error", &inner.error);
                record_counter("warn", &inner.warn);
                record_counter("info", &inner.info);
                record_counter("debug", &inner.debug);
                record_counter("trace", &inner.trace);
                record_counter("invalid", &inner.invalid);

                Ok(inspector)
            }
            .boxed()
        });
        Self { inner, _lazy_node: lazy_node }
    }

    pub fn open_socket(&self) {
        self.inner.sockets_opened.fetch_add(1, Ordering::Relaxed);
    }

    pub fn close_socket(&self) {
        self.inner.sockets_closed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_rolled_out(&self, msg_len: usize) {
        self.inner.rolled_out.increment_bytes(msg_len);
    }

    pub fn increment_invalid(&self, bytes: usize) {
        self.inner.invalid.increment_bytes(bytes);
    }

    pub fn ingest_message(&self, bytes: usize, severity: Severity) {
        self.inner.total.count(bytes);
        match severity {
            Severity::Trace => self.inner.trace.count(bytes),
            Severity::Debug => self.inner.debug.count(bytes),
            Severity::Info => self.inner.info.count(bytes),
            Severity::Warn => self.inner.warn.count(bytes),
            Severity::Error => self.inner.error.count(bytes),
            Severity::Fatal => self.inner.fatal.count(bytes),
        }
    }
}

pub struct GlobalAnalytics {
    _node: Node,
    logs_node: Node,
}

impl GlobalAnalytics {
    pub fn new(parent: &Node) -> Self {
        let node = parent.create_child("global_analytics");
        let logs_node = node.create_child("logs");
        Self { _node: node, logs_node }
    }

    pub fn logs_node(&self) -> &Node {
        &self.logs_node
    }
}

pub struct SaturationCurve {
    boot_times: IntArrayProperty,
    message_counts: UintArrayProperty,
    cursor: Mutex<usize>,
    size: usize,
    _node: Node,
}

impl SaturationCurve {
    pub fn new(parent: &Node, size: usize) -> Self {
        let node = parent.create_child("saturation_curve");
        let boot_times = node.create_int_array("boot_times", size);
        let message_counts = node.create_uint_array("message_counts", size);
        Self { boot_times, message_counts, cursor: Mutex::new(0), size, _node: node }
    }

    pub fn record(&self, boot_time: i64, message_count: u64) {
        let mut cursor = self.cursor.lock();
        self.boot_times.set(*cursor, boot_time);
        self.message_counts.set(*cursor, message_count);
        *cursor = (*cursor + 1) % self.size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn saturation_curve_circular() {
        let inspector = Inspector::default();
        let curve = SaturationCurve::new(inspector.root(), 3);

        curve.record(1, 10);
        curve.record(2, 20);
        curve.record(3, 30);

        assert_data_tree!(inspector,
        root: {
            saturation_curve: {
                boot_times: vec![1i64, 2, 3],
                message_counts: vec![10u64, 20, 30],
            }
        });

        // Should wrap around
        curve.record(4, 40);

        assert_data_tree!(inspector,
        root: {
            saturation_curve: {
                boot_times: vec![4i64, 2, 3],
                message_counts: vec![40u64, 20, 30],
            }
        });
    }
}
