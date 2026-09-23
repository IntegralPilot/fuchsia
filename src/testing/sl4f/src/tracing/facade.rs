// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::tracing::types::{
    InitializeRequest, ResultsDestination, TerminateRequest, TerminateResponse,
};
use anyhow::Error;
use base64::engine::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use fidl_fuchsia_tracing_controller::{
    ProvisionerMarker, SessionMarker, SessionProxy, StartError, StartOptions, StopOptions,
    TraceConfig,
};
use fuchsia_component::{self as app};
use fuchsia_sync::RwLock;

use futures::io::AsyncReadExt;
use serde_json::{Value, from_value, to_value};

// This list should be kept in sync with //src/developer/ffx/plugins/trace/data/config.json which
// is the source of truth for default categories.
// LINT.IfChange
const DEFAULT_CATEGORIES: &[&'static str] = &[
    "app",
    "audio",
    "benchmark",
    "blobfs",
    "fxfs",
    "gfx",
    "input",
    "kernel:meta",
    "kernel:sched",
    "magma",
    "memory:kernel",
    "minfs",
    "modular",
    "net",
    "storage",
    "system_metrics",
    "view",
    "flutter",
    "dart",
    "dart:compiler",
    "dart:dart",
    "dart:debugger",
    "dart:embedder",
    "dart:gc",
    "dart:isolate",
    "dart:profiler",
    "dart:vm",
];
// LINT.ThenChange(/src/developer/ffx/plugins/trace/data/config.json)

/// Perform tracing operations.
///
/// Note this object is shared among all threads created by server.
///
/// This facade does not hold onto a Tracing proxy as the server may be
/// long-running while individual tests set up and tear down Tracing.
#[derive(Debug)]
pub struct TracingFacade {
    status: RwLock<Status>,
}

#[derive(Debug)]
pub struct Status {
    controller: Option<SessionProxy>,
    data_socket: Option<zx::Socket>,
}

impl TracingFacade {
    pub fn new() -> TracingFacade {
        TracingFacade { status: RwLock::new(Status::new()) }
    }

    /// Initialize a trace session.
    ///
    /// A trace session allows for starting and stopping the collection of trace data. Trace data
    /// is then returned all at once when [terminate] is called.
    ///
    /// For documentation on the args parameter, see
    /// [InitializeRequest](crate::tracing::types::InitializeRequest).
    ///
    /// There can only be one trace session active on the system at a time. If there is a trace
    /// session from another controller active on the system, initialize may still return
    /// success, as trace_manager accepts the initialize_tracing call as a no-op. If needed,
    /// [terminate] may be used to ensure that no trace session is active on the system.
    pub async fn initialize(&self, args: Value) -> Result<Value, Error> {
        let request: InitializeRequest = parse_args(args)?;

        let trace_provisioner = app::client::connect_to_protocol::<ProvisionerMarker>()?;
        let (trace_controller, server_end) = fidl::endpoints::create_proxy::<SessionMarker>();
        let (write_socket, read_socket) = zx::Socket::create_stream();
        let mut config = TraceConfig::default();
        match request.categories {
            Some(cats) => {
                config.categories = Some(cats);
            }
            None => {
                config.categories =
                    Some(DEFAULT_CATEGORIES.iter().map(|&s| s.to_owned()).collect());
            }
        }
        config.buffer_size_megabytes_hint = request.buffer_size;

        trace_provisioner.initialize_tracing(server_end, &config, write_socket)?;

        {
            let mut status = self.status.write();
            status.data_socket = Some(read_socket);
            status.controller = Some(trace_controller);
        }

        Ok(to_value(())?)
    }

    /// Start tracing.
    ///
    /// There must be a trace session initialized through this facade, otherwise an error is
    /// returned. Within a trace session, tracing may be started and stopped multiple times.
    pub async fn start(&self) -> Result<Value, Error> {
        let trace_controller = self
            .status
            .read()
            .controller
            .clone()
            .ok_or_else(|| format_err!("No trace session has been initialized"))?;
        let options = StartOptions::default();
        let response = trace_controller.start_tracing(&options).await?;
        match response {
            Ok(_) => Ok(to_value(())?),
            Err(e) => match e {
                StartError::NotInitialized => {
                    Err(format_err!("trace_manager reports trace not initialized"))
                }
                StartError::AlreadyStarted => Err(format_err!("Trace already started")),
                StartError::Stopping => Err(format_err!("Trace is stopping")),
                StartError::Terminating => Err(format_err!("Trace is terminating")),
                _ => Err(format_err!("Unhandled error code during trace start")),
            },
        }
    }

    /// Stop tracing.
    ///
    /// There must be a trace session initialized through this facade, otherwise an error is
    /// returned. Within a trace session, tracing may be started and stopped multiple times.
    pub async fn stop(&self) -> Result<Value, Error> {
        let trace_controller = self
            .status
            .read()
            .controller
            .clone()
            .ok_or_else(|| format_err!("No trace session has been initialized"))?;
        let options = StopOptions::default();
        let _ = trace_controller.stop_tracing(&options).await?;
        Ok(to_value(())?)
    }

    /// Terminate tracing and optionally download the trace data.
    ///
    /// Any trace session on the system will be terminated whether initialized through this facade
    /// or through other means. Downloading trace data will only work for sessions initialized
    /// through this facade. Attempting to download trace data from another trace session will
    /// likely result in the request hanging as trace_manager will attempt to write the data to a
    /// socket with no readers.
    ///
    /// For documentation on the args parameter, see
    /// [TerminateRequest](crate::tracing::types::TerminateRequest).
    pub async fn terminate(&self, args: Value) -> Result<Value, Error> {
        let request: TerminateRequest = parse_args(args)?;

        // Tracing gets terminated when the controller is closed. By default, traces are written
        // to the socket when the controller is dropped.
        let _ = self.status.write().controller.take();

        let result = match request.results_destination {
            ResultsDestination::Ignore => TerminateResponse { data: None },
            ResultsDestination::WriteAndReturn => {
                let data_socket = self.status.write().data_socket.take();
                let drain_result = drain_socket(data_socket).await?;

                TerminateResponse { data: Some(BASE64_STANDARD.encode(&drain_result)) }
            }
        };

        Ok(to_value(result)?)
    }
}

// Helper function that wraps from_value, attempting to deserialize null as if it were an empty
// object.
fn parse_args<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, serde_json::error::Error> {
    if args.is_null() {
        from_value(serde_json::value::Value::Object(serde_json::map::Map::new()))
    } else {
        from_value(args)
    }
}

async fn drain_socket(socket: Option<zx::Socket>) -> Result<Vec<u8>, Error> {
    let mut ret = Vec::new();
    if let Some(socket) = socket {
        let mut socket = fuchsia_async::Socket::from_socket(socket);
        socket.read_to_end(&mut ret).await?;
    }
    Ok(ret)
}

impl Status {
    fn new() -> Status {
        Status { controller: None, data_socket: None }
    }
}
