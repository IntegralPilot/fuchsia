// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::log_debug;
use fuchsia_inspect::Node;
use fuchsia_inspect_contrib::nodes::BoundedListNode;

use starnix_sync::Mutex;

/// The maximum number of failed tasks to record.
///
/// This number is arbitrary and we may want to make it configurable in the future.
const MAX_NUM_COREDUMPS: usize = 64;

/// The maximum length of an argv string to record.
///
/// This number is arbitrary and we may wand to make it configurable in the future.
const MAX_ARGV_LENGTH: usize = 128;

/// A list of recently coredumped tasks in Inspect.
pub struct CoreDumpList {
    list: Mutex<BoundedListNode>,
}

#[derive(Debug)]
pub struct CoreDumpInfo {
    pub process_koid: zx::Koid,
    pub thread_koid: zx::Koid,
    pub linux_pid: i64,
    pub argv: Vec<String>,
    pub uptime: i64,
    pub thread_name: String,
    pub signal: String,
}

impl CoreDumpList {
    pub fn new(node: Node) -> Self {
        Self { list: Mutex::new(BoundedListNode::new(node, MAX_NUM_COREDUMPS)) }
    }

    pub fn record_core_dump(&self, core_dump_info: CoreDumpInfo) {
        let mut argv = core_dump_info.argv.join(" ");
        let original_len = argv.len();
        argv.truncate(MAX_ARGV_LENGTH - 3);
        if argv.len() < original_len {
            argv.push_str("...");
        }
        let mut list = self.list.lock();
        list.add_entry(|crash_node| {
            log_debug!(linux_pid = core_dump_info.linux_pid, argv:%; "Recording task with a coredump.");
            crash_node.record_uint("thread_koid", core_dump_info.thread_koid.raw_koid());
            crash_node.record_uint("process_koid", core_dump_info.process_koid.raw_koid());
            crash_node.record_int("pid", core_dump_info.linux_pid);
            crash_node.record_int("uptime", core_dump_info.uptime);
            crash_node.record_string("argv", argv);
            crash_node.record_string("thread_name", &core_dump_info.thread_name);
            crash_node.record_string("signal", &core_dump_info.signal);
        });
    }
}
