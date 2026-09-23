// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_netemul_test::{CounterRequest, CounterRequestStream};
use fuchsia_component::client;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use fuchsia_sync::Mutex;
use futures::prelude::*;
use log::{error, info};
use std::pin::pin;
use std::sync::Arc;

struct CounterData {
    value: u32,
    abort_on_shutdown: bool,
}

const SVC_DIR: &str = "/svc";

async fn handle_counter(
    stream: CounterRequestStream,
    data: Arc<Mutex<CounterData>>,
) -> Result<(), fidl::Error> {
    stream
        .try_for_each(|request| async {
            match request {
                CounterRequest::Increment { responder } => {
                    let mut d = data.lock();
                    d.value += 1;
                    info!("incrementing counter to {}", d.value);
                    responder
                        .send(d.value)
                        .unwrap_or_else(|e| error!("error sending response: {:?}", e));
                }
                CounterRequest::ConnectToProtocol { protocol_name, request, control_handle: _ } => {
                    info!("connecting to protocol '{}'", protocol_name);
                    client::connect_channel_to_protocol_at_path(
                        request,
                        &format!("{}/{}", SVC_DIR, protocol_name),
                    )
                    .unwrap_or_else(|e| {
                        error!(
                            "error connecting request to protocol '{}' in '{}' directory: {:?}",
                            protocol_name, SVC_DIR, e,
                        )
                    });
                }
                CounterRequest::OpenInNamespace { path, flags, request, control_handle: _ } => {
                    info!("connecting to node at '{}'", path);
                    fdio::open(&path, flags, request).unwrap_or_else(|e| {
                        error!("error connecting request to node at path '{}': {}", path, e)
                    });
                }
                CounterRequest::TryOpenDirectory { path, responder } => {
                    info!("opening directory at '{}'", path);
                    match std::fs::read_dir(&path) {
                        Ok(std::fs::ReadDir { .. }) => responder
                            .send(Ok(()))
                            .unwrap_or_else(|e| error!("error sending response: {:?}", e)),
                        Err(e) => {
                            let status = match e.kind() {
                                std::io::ErrorKind::NotFound | std::io::ErrorKind::BrokenPipe => {
                                    info!("failed to open directory at '{}': {}", path, e);
                                    zx::Status::NOT_FOUND
                                }
                                _ => {
                                    error!("failed to open directory at '{}': {}", path, e);
                                    zx::Status::IO
                                }
                            };
                            responder
                                .send(Err(status.into_raw()))
                                .unwrap_or_else(|e| error!("error sending response: {:?}", e));
                        }
                    }
                }
                CounterRequest::SetAbortOnShutdown { abort, responder } => {
                    data.lock().abort_on_shutdown = abort;
                    responder.send().unwrap_or_else(|e| error!("error sending response: {:?}", e));
                }
            }
            Ok(())
        })
        .await
}

fn get_lifecycle_stop_fut() -> impl Future<Output = ()> {
    // Lifecycle handle takes no args, must be set to zero.
    // See zircon/processargs.h.
    const LIFECYCLE_HANDLE_ARG: u16 = 0;
    let Some(handle) = fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
        fuchsia_runtime::HandleType::Lifecycle,
        LIFECYCLE_HANDLE_ARG,
    )) else {
        // Allow some configurations to not have a lifecycle arg. Pend forever.
        return futures::future::pending::<()>().right_future();
    };

    async move {
        let mut request_stream =
            fidl_fuchsia_process_lifecycle::LifecycleRequestStream::from_channel(
                fuchsia_async::Channel::from_channel(handle.into()).into(),
            );

        match request_stream
            .next()
            .await
            .expect("lifecycle stream ended unexpectedly")
            .expect("error in lifecycle request stream")
        {
            fidl_fuchsia_process_lifecycle::LifecycleRequest::Stop { control_handle } => {
                info!("received shutdown request");
                // Shutdown request is acknowledged by the lifecycle
                // channel shutting down. Intentionally leak the channel
                // so it'll only be closed on process termination,
                // allowing clean process termination to always be
                // observed.

                // Must drop the control_handle to unwrap the lifecycle
                // channel.
                std::mem::drop(control_handle);
                let (inner, _terminated): (_, bool) = request_stream.into_inner();
                let inner = std::sync::Arc::try_unwrap(inner)
                    .expect("failed to retrieve lifecycle channel");
                let inner: zx::Channel = inner.into_channel().into_zx_channel();
                std::mem::forget(inner);
            }
        }
    }
    .left_future()
}

/// Command line arguments for the counter service.
#[derive(argh::FromArgs)]
struct Args {
    /// the value at which to start the counter.
    #[argh(option, default = "0")]
    starting_value: u32,
    /// read the starting value from structured config.
    ///
    /// starting_value is ignored if provided.
    #[argh(switch)]
    starting_value_from_config: bool,
}

#[fuchsia::main()]
async fn main() -> Result<(), Error> {
    let Args { starting_value, starting_value_from_config } = argh::from_env();
    let starting_value = if starting_value_from_config {
        // We only try to read this if the program arg is passed because it
        // makes it easier to handle different CMLs for the same binary, some of
        // which may not contain configs.
        let counter_config::Config { routed_config: _, starting_value } =
            counter_config::Config::take_from_startup_handle();
        starting_value
    } else {
        starting_value
    };

    let mut fs = ServiceFs::new();
    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default())
            .context("publish Inspect task")?;
    let data = {
        let data =
            Arc::new(Mutex::new(CounterData { value: starting_value, abort_on_shutdown: false }));
        let data_clone = data.clone();
        inspector.root().record_lazy_child("counter", move || {
            let srv = fuchsia_inspect::Inspector::default();
            srv.root().record_uint("count", data.lock().value.into());
            futures::future::ok(srv).boxed()
        });
        data_clone
    };

    let _: &mut ServiceFsDir<'_, _> = fs.dir("svc").add_fidl_service(|s: CounterRequestStream| s);
    let _: &mut ServiceFs<_> =
        fs.take_and_serve_directory_handle().context("error serving directory handle")?;

    let mut stop_fut = pin!(get_lifecycle_stop_fut().fuse());
    let mut serve_fut = fs.for_each_concurrent(None, |stream| async {
        handle_counter(stream, data.clone())
            .await
            .unwrap_or_else(|e| error!("error handling CounterRequestStream: {:?}", e))
    });

    futures::select! {
        () = serve_fut => panic!("services future ended unexpectedly"),
        () = stop_fut => {}
    }

    if data.lock().abort_on_shutdown {
        std::process::abort();
    }

    Ok(())
}
