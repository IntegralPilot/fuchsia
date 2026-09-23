// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// core
mod error;
mod message;
mod node;
mod power_manager;
mod thermal_sensor_manager;
mod timer;
mod utils;

#[path = "../../common/lib/common_utils.rs"]
mod common_utils;
#[path = "../../common/lib/types.rs"]
mod types;

// nodes
mod activity_handler;
mod crash_report_handler;
mod debug_service;
mod input_settings_handler;
mod platform_metrics;
mod system_power_mode_handler;
mod system_profile_handler;
mod system_shutdown_handler;
mod temperature_handler;
mod thermal_load_driver;
mod thermal_policy;
mod thermal_shutdown;
mod thermal_state_handler;

#[cfg(test)]
mod test;

use crate::power_manager::PowerManager;
use anyhow::Error;
use fidl_fuchsia_process_lifecycle as flifecycle;
use fuchsia_inspect::health::Reporter;
use futures::FutureExt;
use futures::stream::StreamExt;

async fn run_stop_watcher(mut stream: flifecycle::LifecycleRequestStream) {
    let Some(Ok(request)) = stream.next().await else {
        return std::future::pending::<()>().await;
    };
    match request {
        flifecycle::LifecycleRequest::Stop { .. } => {
            return;
        }
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    log::info!("started");

    // Setup tracing
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    // Set up the PowerManager
    let mut pm = PowerManager::new();

    let lifecycle =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::Lifecycle.into())
            .expect("Expected to have a lifecycle startup handle.");
    let lifecycle = fidl::endpoints::ServerEnd::<flifecycle::LifecycleMarker>::new(
        zx::Channel::from(lifecycle),
    );
    let lifecycle_stream = lifecycle.into_stream();

    futures::select! {
        result = pm.run().fuse() => {
            // This future should never complete
            log::error!("Unexpected exit with result: {:?}", result);
            fuchsia_inspect::component::health().set_unhealthy("Unexpected exit");
            result
        },
        () = run_stop_watcher(lifecycle_stream).fuse() => {
            log::info!("Recieved stop request, exiting gracefully");
            Ok(())
        },
    }
}
