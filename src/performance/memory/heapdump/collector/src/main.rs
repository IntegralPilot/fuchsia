// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::warn;
use {
    fidl_fuchsia_memory_heapdump_client as fheapdump_client,
    fidl_fuchsia_memory_heapdump_process as fheapdump_process,
};

mod process;
mod process_v1;
mod registry;
mod snapshot_storage;
mod utils;

use registry::Registry;
use std::sync::Arc;

/// All FIDL services that are exposed by this component's ServiceFs.
enum Service {
    /// The `fuchsia.memory.heapdump.client.Collector` protocol.
    Client(fheapdump_client::CollectorRequestStream),
    /// The `fuchsia.memory.heapdump.process.Registry` protocol.
    Process(fheapdump_process::RegistryRequestStream),
}

#[fuchsia::main]
async fn main() -> Result<(), anyhow::Error> {
    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let registry = Arc::new(Registry::new());
    registry.register_inspect(inspector.root());

    let mut service_fs = ServiceFs::new();
    service_fs.dir("svc").add_fidl_service(Service::Client);
    service_fs.dir("svc").add_fidl_service(Service::Process);
    service_fs.take_and_serve_directory_handle()?;

    service_fs
        .for_each_concurrent(None, |stream| async {
            match stream {
                Service::Client(stream) => {
                    if let Err(error) = registry.serve_client_stream(stream).await {
                        warn!(error:%; "Error while serving client");
                    }
                }
                Service::Process(stream) => {
                    if let Err(error) = registry.serve_process_stream(stream).await {
                        warn!(error:%; "Error while serving process");
                    }
                }
            }
        })
        .await;

    Ok(())
}
