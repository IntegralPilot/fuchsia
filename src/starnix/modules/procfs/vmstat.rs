// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::mm::PAGE_SIZE;
use starnix_core::task::{CurrentTask, KernelStats};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_logging::log_error;
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::sync::Arc;

#[derive(Clone)]
pub struct VmStatFile {
    kernel_stats: Arc<KernelStats>,
}

impl VmStatFile {
    pub fn new_node(kernel_stats: &Arc<KernelStats>) -> impl FsNodeOps {
        DynamicFile::new_node(Self { kernel_stats: kernel_stats.clone() })
    }
}

// Fake an increment, since the current clients only care about the number increasing and not
// what the number is.
static VM_STAT_HACK_COUNTER: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl DynamicFileSource for VmStatFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        let mem_stats = self
            .kernel_stats
            .get()
            .get_memory_stats_extended(zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!("FIDL error getting memory stats: {e}");
                errno!(EIO)
            })?;

        let userpager_total = mem_stats.vmo_pager_total_bytes.unwrap_or_default() / *PAGE_SIZE;
        let userpager_active = mem_stats.vmo_pager_newest_bytes.unwrap_or_default() / *PAGE_SIZE;

        let nr_active_file = userpager_active;
        let nr_inactive_file = userpager_total.saturating_sub(userpager_active);

        // Only fields required so far are written. Add more fields as needed.
        writeln!(sink, "workingset_refault_file {}", 0)?;
        writeln!(sink, "nr_inactive_file {}", nr_inactive_file)?;
        writeln!(sink, "nr_active_file {}", nr_active_file)?;
        writeln!(
            sink,
            "pgscan_kswapd {}",
            VM_STAT_HACK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )?;
        writeln!(sink, "pgscan_direct {}", 0)?;

        Ok(())
    }
}
