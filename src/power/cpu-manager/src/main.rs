// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// core
mod cpu_manager;
mod domain_controller;
mod error;
mod message;
mod node;

#[path = "../../common/lib/common_utils.rs"]
mod common_utils;
#[path = "../../common/lib/types.rs"]
mod types;

// nodes
mod cpu_control_handler;
mod cpu_device_handler;
mod cpu_manager_main;
mod cpu_stats_handler;
mod cpu_stats_recorder;
mod syscall_handler;
mod thermal_watcher;
mod trippoint_watcher;
mod utils;

#[cfg(test)]
mod test;

use crate::cpu_manager::CpuManager;
use anyhow::Error;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    log::info!("started");

    // Setup tracing
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    // Set up the CpuManager
    let mut cm = CpuManager::new();

    // This future should never complete
    let result = cm.run().await;
    log::error!("Unexpected exit with result: {:?}", result);
    result
}
