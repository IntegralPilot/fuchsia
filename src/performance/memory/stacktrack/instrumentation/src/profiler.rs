// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_runtime;
use stacktrack_vmo::threads_table_v1::{Frame, MAX_FRAMES, NODE_INVALID, StacktrackWriter};
use std::sync::Mutex;

use crate::unwind::unwind_if_deeper;

/// We cap the size of our backing VMO at 2 GiB, then preallocate it and map it entirely.
/// Actual memory for each page will only be committed when we first write to that page.
const VMO_SIZE: usize = 1 << 31;

const VMO_NAME: zx::Name = zx::Name::new_lossy("stacktrack");

pub struct PerThreadData {
    /// The index of the node assigned to this thread, or `NODE_INVALID` if none.
    node_index: u32,
    /// Whether the thread is in the process of shutting down / exiting.
    is_shutting_down: bool,
    /// The base frame pointer of the most recent stack capture.
    peak_fp: u64,
}

impl PerThreadData {
    pub const fn new() -> Self {
        Self { node_index: NODE_INVALID, is_shutting_down: false, peak_fp: 0 }
    }
}

pub struct Profiler {
    vmo: zx::Vmo,
    writer: Mutex<StacktrackWriter>,
}

unsafe impl Sync for Profiler {}
unsafe impl Send for Profiler {}

impl Default for Profiler {
    fn default() -> Profiler {
        let vmo = zx::Vmo::create(VMO_SIZE as u64).expect("failed to create stacktrack VMO");
        vmo.set_name(&VMO_NAME).expect("failed to set VMO name");

        // SAFETY: Nobody else will directly access this VMO. Even when we share it with the
        // collector, it will always take a snapshot first and then read the snapshot instead of the
        // original one.
        let writer = unsafe { StacktrackWriter::new(&vmo).expect("failed to create writer") };

        Profiler { vmo, writer: Mutex::new(writer) }
    }
}

impl Profiler {
    pub fn update_thread(&self, td: &mut PerThreadData) {
        if td.is_shutting_down {
            return;
        }

        // Capture frames.
        let mut frames = [Frame::default(); MAX_FRAMES];

        // Unwind only if deeper than previous peak.
        if let Some(count) = unwind_if_deeper(td.peak_fp, &mut frames) {
            let koid =
                fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap().raw_koid();
            let count = count.get();

            let mut writer = self.writer.lock().unwrap();
            let Ok(new_node_index) = writer.insert_at_head(koid, &frames[..count]) else {
                return; // Silently ignore the out-of-memory error.
            };

            let old_node_index = td.node_index;
            if old_node_index != NODE_INVALID {
                writer.remove(old_node_index);
            }

            td.node_index = new_node_index;
            td.peak_fp = frames[0].fp;
        }
    }

    pub fn remove_thread(&self, td: &mut PerThreadData) {
        td.is_shutting_down = true;

        let old_node_index = td.node_index;
        if old_node_index != NODE_INVALID {
            let mut writer = self.writer.lock().unwrap();
            writer.remove(old_node_index);
            td.node_index = NODE_INVALID;
        }
    }

    pub fn get_vmo(&self) -> Result<zx::Vmo, zx::Status> {
        self.vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
    }
}
