// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # Dynamic IO Stressor Harness
//!
//! A lightweight configurable storage stress workload generator.

use anyhow::Result;
use io_stress::stress::{MemoryPressureStats, watch_memory_pressure};
use io_stress::workloads::run_workloads;
use std::sync::Arc;

#[fuchsia::main(logging_tags = ["io_stress"])]
async fn main() -> Result<()> {
    // 1. Initialize tracing provider
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    // 2. Parse and Validate CLI Arguments
    let args_vec: Vec<String> = std::env::args().collect();
    let (duration_secs, subcommands) = io_stress::parser::parse_chained_workloads(&args_vec)?;
    io_stress::parser::validate_workloads(&subcommands)?;

    // 4. Monitor memory pressure level changes concurrently in the background
    let mem_stats = Arc::new(MemoryPressureStats::new());
    let mem_stats_clone = mem_stats.clone();
    let _watcher_task = fuchsia_async::Task::spawn(async move {
        if let Err(e) = watch_memory_pressure(mem_stats_clone).await {
            log::warn!("Memory pressure watcher task failed: {:?}", e);
        }
    });

    // 5. Run the configured workloads
    let _ = run_workloads("manual", subcommands, duration_secs, mem_stats).await?;

    Ok(())
}
