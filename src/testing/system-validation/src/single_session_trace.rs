// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use fidl_fuchsia_tracing_controller::{
    ProvisionerMarker, ProvisionerProxy, SessionMarker, SessionProxy, StartError, StartOptions,
    StopOptions, TerminateOptions, TraceConfig,
};
use fuchsia_async;
use fuchsia_component::{self as app};
use fuchsia_sync::RwLock;
use futures::io::AsyncReadExt;
use futures::{future, TryFutureExt};
use std::fs;
use std::io::Write;

const TRACE_FILE: &'static str = "/custom_artifacts/trace.fxt";
const BUFFER_SIZE_MB: u32 = 36;

struct Status {
    controller: Option<SessionProxy>,
    data_socket: Option<zx::Socket>,
}

pub struct SingleSessionTrace {
    status: RwLock<Status>,
}

/// Provides controls to capture a single trace session
///
/// To use:
/// ```
/// let trace = SingleSessionTrace::new();
/// trace.initialize(args.trace_config).await?;
/// trace.start().await?;
/// ...
/// trace.stop().await?;
/// trace.terminate().await?
/// ```
impl SingleSessionTrace {
    pub fn new() -> SingleSessionTrace {
        SingleSessionTrace { status: RwLock::new(Status { controller: None, data_socket: None }) }
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
    pub async fn initialize(&self, categories: Vec<String>) -> Result<(), Error> {
        let trace_provisioner = app::client::connect_to_protocol::<ProvisionerMarker>()?;
        let (write_socket, read_socket) = zx::Socket::create_stream();
        let (trace_controller, server) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_tracing_controller::SessionMarker>();
        let config = TraceConfig {
            buffer_size_megabytes_hint: Some(BUFFER_SIZE_MB),
            categories: Some(categories),
            ..Default::default()
        };

        trace_provisioner.initialize_tracing(server, config, write_socket)?;
        {
            let mut status = self.status.write();
            status.data_socket = Some(read_socket);
            status.controller = Some(trace_controller);
        }
        Ok(())
    }

    /// Start tracing.
    ///
    /// There must be a trace session initialized first, otherwise an error is returned.
    /// Within a trace session, tracing may be started and stopped multiple times.
    pub async fn start(&self) -> Result<(), Error> {
        let status = self.status.read();
        let trace_controller = status
            .controller
            .as_ref()
            .ok_or_else(|| format_err!("No trace session has been initialized"))?;
        match trace_controller.start_tracing(StartOptions::default()).await? {
            Ok(_) => Ok(()),
            Err(e) => match e {
                StartError::NotInitialized => {
                    Err(format_err!("trace_manager reports trace not initialized"))
                }
                StartError::AlreadyStarted => Err(format_err!("Trace already started")),
                StartError::Stopping => Err(format_err!("Trace is stopping")),
                StartError::Terminating => Err(format_err!("Trace is terminating")),
            },
        }
    }

    /// Stop tracing.
    ///
    /// There must be a trace session initialized first, otherwise an error is returned.
    /// Within a trace session, tracing may be started and stopped multiple times.
    pub async fn stop(&self) -> Result<(), Error> {
        let status = self.status.read();
        let trace_controller = status
            .controller
            .as_ref()
            .ok_or_else(|| format_err!("No trace session has been initialized"))?;
        trace_controller.stop_tracing(StopOptions::default()).await?;
        Ok(())
    }

    /// Terminate tracing and convert data to json using `trace2json`.
    ///
    /// Both raw trace file and converted trace json file will be stored in test's custom artifact.
    pub async fn terminate(&self) -> Result<(), Error> {
        // Tracing gets terminated when the controller is closed. Drain the socket to get
        // everything that was written until the last stop and destroy the existing controller.
        let controller = self.status.write().controller.take();
        let data_socket = self.status.write().data_socket.take();
        let drain_result = drain_socket(data_socket).await?;

        // write this to file
        let mut trace_file = fs::File::create(TRACE_FILE)?;
        trace_file.write_all(&drain_result)?;

        Ok(())
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
