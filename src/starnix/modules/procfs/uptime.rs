// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::{CurrentTask, KernelStats};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::sync::Arc;

#[derive(Clone)]
pub struct UptimeFile {
    kernel_stats: Arc<KernelStats>,
}

impl UptimeFile {
    pub fn new_node(kernel_stats: &Arc<KernelStats>) -> impl FsNodeOps {
        DynamicFile::new_node(Self { kernel_stats: kernel_stats.clone() })
    }
}

impl DynamicFileSource for UptimeFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        let uptime = (zx::BootInstant::get() - zx::BootInstant::ZERO).into_seconds_f64();

        // Fetch CPU stats from `fuchsia.kernel.Stats` to calculate idle time.
        let cpu_stats = self
            .kernel_stats
            .get()
            .get_cpu_stats(zx::MonotonicInstant::INFINITE)
            .map_err(|_| errno!(EIO))?;
        let per_cpu_stats = cpu_stats.per_cpu_stats.unwrap_or(vec![]);
        let idle_time = per_cpu_stats.iter().map(|s| s.idle_time.unwrap_or(0)).sum();
        let idle_time = zx::MonotonicDuration::from_nanos(idle_time).into_seconds_f64();

        writeln!(sink, "{:.2} {:.2}", uptime, idle_time)?;

        Ok(())
    }
}
