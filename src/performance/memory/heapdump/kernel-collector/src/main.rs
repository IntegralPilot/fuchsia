// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::path::Path;

use anyhow::Context;
use fidl_fuchsia_memory_heapdump_client as fheapdump_client;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
// use fuchsia_zircon::{self as zx};
use futures::prelude::*;
use log::{debug, warn};

mod kernel_collector;
use kernel_collector::KernelCollector;
mod region;
use region::Region;

const KERNEL_HEAP_PROFILE_PATH: &str = "/boot/kernel/i/memory-profile/d/heap.bin";

/// Wraps all hosted protocols into a single type that can be matched against
/// and dispatched.
enum IncomingRequest {
    // Add a variant for each protocol being served.
    Client(fheapdump_client::CollectorRequestStream),
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), anyhow::Error> {
    let mut service_fs = ServiceFs::new_local();

    // Initialize inspect
    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    component::health().set_starting_up();

    service_fs.dir("svc").add_fidl_service(IncomingRequest::Client);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    component::health().set_ok();
    debug!("Initialized.");

    // Map the heap profile VMO in memory once for all.
    let file = File::open(KERNEL_HEAP_PROFILE_PATH)?;
    let region = Region::new(&fdio::get_vmo_exact_from_file(&file)?)?;
    let symbol_path = Path::new("/boot/kernel/i/logs/physboot");
    let kernel_collector = KernelCollector::new(&region, &symbol_path);
    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| async {
            match request {
                IncomingRequest::Client(stream) => {
                    if let Err(error) = kernel_collector.serve_client_stream(stream).await {
                        let s = std::backtrace::Backtrace::capture().status();
                        warn!(error:?; "Error while serving client stream. {s:?}");
                    }
                }
            }
        })
        .await;

    Ok(())
}
