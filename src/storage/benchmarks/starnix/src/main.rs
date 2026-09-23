// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl::endpoints::Proxy;
use fidl_fuchsia_component::BinderMarker;
use fidl_fuchsia_power_cpu_manager as fcpumanager;
use fuchsia_component::client::connect_to_protocol;
use log::{info, warn};

#[fuchsia::main(logging_tags = ["storage_starnix_benchmarks"])]
async fn main() -> Result<(), anyhow::Error> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    fuchsia_trace_provider::trace_provider_wait_for_init();
    fuchsia_trace::duration!("benchmark", "starnix_storage_benchmark");

    // Attempt to boost CPU performance during the benchmark run to mitigate DVFS.
    let booster = connect_to_protocol::<fcpumanager::BoostMarker>();
    let _boost_token = match booster {
        Ok(proxy) => match proxy.boost().await {
            Ok(Ok(token)) => {
                info!("CPU performance boost active for Starnix storage benchmarks.");
                Some(token)
            }
            Ok(Err(e)) => {
                warn!("CPU boost protocol returned error: {:?}", e);
                None
            }
            Err(e) => {
                warn!("Failed to call CPU boost: {:?}", e);
                None
            }
        },
        Err(e) => {
            warn!("CPU boost protocol not available: {:?}", e);
            None
        }
    };

    info!("Starting Starnix storage benchmark workload...");
    // Connecting to the Binder protocol starts the benchmark child component.
    let binder_proxy = connect_to_protocol::<BinderMarker>()
        .context("Failed to connect to benchmark app Binder")?;

    binder_proxy.on_closed().await.context("Failed waiting for benchmark app to complete")?;

    info!("Starnix storage benchmark workload completed successfully.");
    Ok(())
}
