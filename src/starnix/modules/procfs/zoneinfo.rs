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
pub struct ZoneInfoFile {
    kernel_stats: Arc<KernelStats>,
}

impl ZoneInfoFile {
    pub fn new_node(kernel_stats: &Arc<KernelStats>) -> impl FsNodeOps {
        DynamicFile::new_node(Self { kernel_stats: kernel_stats.clone() })
    }
}

impl DynamicFileSource for ZoneInfoFile {
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
        let free = mem_stats.free_bytes.unwrap_or_default() / *PAGE_SIZE;
        let present = mem_stats.total_bytes.unwrap_or_default() / *PAGE_SIZE;

        // Pages min: minimum number of free pages the kernel tries to maintain in this memory zone.
        // Can be set by writing to `/proc/sys/vm/min_free_kbytes`. It is observed to be ~3% of the
        // total memory.
        let pages_min = present * 3 / 100;
        // Pages low: more aggressive memory reclaimation when free pages fall below this level.
        // Typically ~4% of the total memory.
        let pages_low = present * 4 / 100;
        // Pages high: page reclamation begins when free pages drop below this level.
        // Typically ~4% of the total memory.
        let pages_high = present * 6 / 100;

        // Only required fields are written. Add more fields as needed.
        writeln!(sink, "Node 0, zone   Normal")?;
        writeln!(sink, "  per-node stats")?;
        writeln!(sink, "      nr_inactive_file {}", nr_inactive_file)?;
        writeln!(sink, "      nr_active_file {}", nr_active_file)?;
        writeln!(sink, "  pages free     {}", free)?;
        writeln!(sink, "        min      {}", pages_min)?;
        writeln!(sink, "        low      {}", pages_low)?;
        writeln!(sink, "        high     {}", pages_high)?;
        writeln!(sink, "        present  {}", present)?;
        writeln!(sink, "  pagesets")?;

        Ok(())
    }
}
