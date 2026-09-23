// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::driver_host::DriverHost;
use anyhow::{Context, Result};
use fidl_fuchsia_driver_host::DriverHostRequestStream;
use fuchsia_component::server::ServiceFs;
use futures::channel::oneshot;
use futures::{FutureExt, StreamExt};
use std::rc::Rc;

mod conversion;
mod driver;
mod driver_host;
mod loader;
mod modules;
mod utils;

enum ExposedProtocols {
    DriverHost(DriverHostRequestStream),
}

#[fuchsia::main(logging = true, logging_tags = ["driver_host", "driver"])]
async fn main() -> Result<(), anyhow::Error> {
    // Redirect standard out to debuglog.
    if stdout_to_debuglog::init().await.is_err() {
        log::warn!(
            "Failed to redirect stdout to debuglog, assuming test environment and continuing"
        );
    }

    let options = fdf_env::Environment::ENFORCE_ALLOWED_SCHEDULER_ROLES;
    let env = fdf_env::Environment::start(options)?;

    let mut service_fs = ServiceFs::new_local();

    // Initialize inspect
    let _inspect_server_task = inspect_runtime::publish(
        fuchsia_inspect::component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );

    // Initialize tracing
    let scope_dispatcher = libasync_scope_dispatcher::ScopeDispatcher::new();
    let trace_provider =
        fuchsia_trace_provider::TraceProvider::new_with_fdio(&scope_dispatcher, None);

    service_fs.dir("svc").add_fidl_service(ExposedProtocols::DriverHost);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    // TODO: add inspect instrumentation to driver host.
    let (signaler, no_more_drivers_event) = oneshot::channel();
    let driver_host = Rc::new(DriverHost::new(env, signaler));

    log::debug!("Initialized");
    let service_fs_fut = service_fs.for_each_concurrent(None, |request: ExposedProtocols| async {
        match request {
            ExposedProtocols::DriverHost(stream) => {
                driver_host.clone().run_driver_host_server(stream).await
            }
        }
    });
    driver_host.clone().run_exception_listener();
    driver_host.clone().run_exception_cleanup_task();
    driver_host.run_thread_monitor_task();

    // Exit if the servicefs exits or all drivers are shutdown.
    futures::select! {
        _ = service_fs_fut.fuse() => {},
        _ = no_more_drivers_event.fuse() => {},
    };

    drop(trace_provider);
    scope_dispatcher.shutdown().await;

    Ok(())
}
