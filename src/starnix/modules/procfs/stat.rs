// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::{CurrentTask, KernelStats};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_logging::log_error;
use starnix_types::time::duration_to_scheduler_clock;
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone)]
pub struct StatFile {
    kernel_stats: Arc<KernelStats>,
}

impl StatFile {
    pub fn new_node(kernel_stats: &Arc<KernelStats>) -> impl FsNodeOps {
        DynamicFile::new_node(Self { kernel_stats: kernel_stats.clone() })
    }
}

impl DynamicFileSource for StatFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        let uptime = zx::MonotonicInstant::get() - zx::MonotonicInstant::ZERO;

        let cpu_stats =
            self.kernel_stats.get().get_cpu_stats(zx::MonotonicInstant::INFINITE).map_err(|e| {
                log_error!("FIDL error getting cpu stats: {e}");
                errno!(EIO)
            })?;

        // Number of values reported per CPU. See `get_cpu_stats_row` below for the list of values.
        const NUM_CPU_STATS: usize = 10;

        let get_cpu_stats_row = |cpu_stats: &fidl_fuchsia_kernel::PerCpuStats| {
            let idle = zx::MonotonicDuration::from_nanos(cpu_stats.idle_time.unwrap_or(0));

            // Assume that all non-idle time is spent in user mode.
            let user = uptime - idle;

            // Zircon currently reports only number of various interrupts, but not the time spent
            // handling them. Return zeros.
            let nice: u64 = 0;
            let system: u64 = 0;
            let iowait: u64 = 0;
            let irq: u64 = 0;
            let softirq: u64 = 0;
            let steal: u64 = 0;
            let quest: u64 = 0;
            let quest_nice: u64 = 0;

            [
                duration_to_scheduler_clock(user) as u64,
                nice,
                system,
                duration_to_scheduler_clock(idle) as u64,
                iowait,
                irq,
                softirq,
                steal,
                quest,
                quest_nice,
            ]
        };
        let per_cpu_stats = cpu_stats.per_cpu_stats.unwrap_or(vec![]);
        let mut cpu_total_row = [0u64; NUM_CPU_STATS];
        for row in per_cpu_stats.iter().map(get_cpu_stats_row) {
            for (i, value) in row.iter().enumerate() {
                cpu_total_row[i] += value
            }
        }

        writeln!(sink, "cpu {}", cpu_total_row.map(|n| n.to_string()).join(" "))?;
        for (i, row) in per_cpu_stats.iter().map(get_cpu_stats_row).enumerate() {
            writeln!(sink, "cpu{} {}", i, row.map(|n| n.to_string()).join(" "))?;
        }

        let context_switches: u64 =
            per_cpu_stats.iter().map(|s| s.context_switches.unwrap_or(0)).sum();
        writeln!(sink, "ctxt {}", context_switches)?;

        let num_interrupts: u64 = per_cpu_stats.iter().map(|s| s.ints.unwrap_or(0)).sum();
        writeln!(sink, "intr {}", num_interrupts)?;

        let epoch_time = zx::MonotonicDuration::from(
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default(),
        );
        let boot_time_epoch = epoch_time - uptime;
        writeln!(sink, "btime {}", boot_time_epoch.into_seconds())?;

        Ok(())
    }
}
