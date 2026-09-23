// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_memory_stacktrack_client as fstacktrack_client;
use fidl_fuchsia_memory_stacktrack_process as fstacktrack_process;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::warn;
use std::sync::Arc;

mod process;
mod process_v1;
mod registry;
mod utils;
use registry::Registry;

/// All FIDL services that are exposed by this component's ServiceFs.
enum Service {
    /// The `fuchsia.memory.stacktrack.client.Collector` protocol.
    Client(fstacktrack_client::CollectorRequestStream),
    /// The `fuchsia.memory.stacktrack.process.Registry` protocol.
    Process(fstacktrack_process::RegistryRequestStream),
}

#[fuchsia::main]
async fn main() -> Result<(), anyhow::Error> {
    let registry = Arc::new(Registry::new());

    let mut service_fs = ServiceFs::new();
    service_fs.dir("svc").add_fidl_service(Service::Client);
    service_fs.dir("svc").add_fidl_service(Service::Process);
    service_fs.take_and_serve_directory_handle()?;

    service_fs
        .for_each_concurrent(None, |stream| async {
            match stream {
                Service::Client(stream) => {
                    if let Err(error) = registry.serve_client_stream(stream).await {
                        warn!("Error while serving client: {:?}", error);
                    }
                }
                Service::Process(stream) => {
                    if let Err(error) = registry.serve_process_stream(stream).await {
                        warn!("Error while serving process: {:?}", error);
                    }
                }
            }
        })
        .await;

    Ok(())
}
