// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::atomic_box::AtomicBox;
use crate::tracing::{Hermeticity, HermeticityParameters};
use anyhow::format_err;
use fidl_fuchsia_tracing_controller::SessionProxy;
use flex_fuchsia_io as fio;
use fuchsia_async::Timer;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at};
use futures::AsyncReadExt;
use futures::channel::oneshot::{Canceled, Receiver, Sender};
use log::{error, info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{JoinHandle, ThreadId};
use std::time::Duration;

pub struct TerminationResult {
    pub termination_signal: Option<Result<(), anyhow::Error>>,
    pub trace_writer: Option<Result<(), anyhow::Error>>,
}

#[derive(Clone)]
pub struct TraceRunner {
    terminate_tracing_sender: Arc<AtomicBox<Sender<String>>>,
    id: ThreadId,
    trace_writer: Arc<AtomicBox<JoinHandle<Result<(), anyhow::Error>>>>,
}

impl TraceRunner {
    pub async fn start(
        hermeticity: Hermeticity,
        output_trace_path: PathBuf,
        trace_timeout: Duration,
        trace_file_max_bytes: usize,
    ) -> Result<Self, anyhow::Error> {
        let (tracing_started_sender, tracing_started_receiver) =
            futures::channel::oneshot::channel();
        let (terminate_tracing_sender, terminate_tracing_receiver) =
            futures::channel::oneshot::channel::<String>();

        // Keep a reference to this Sender so TraceRunner::terminate_trace can terminate a trace,
        // e.g., when dropping an instance of Tracing or upon a panic.
        let terminate_tracing_sender = Arc::new(AtomicBox::new(terminate_tracing_sender));

        // Spawn a thread that will start a trace and stream the trace into a file until receiving
        // a reason to stop the trace on `terminate_tracing_receiver`.
        let trace_writer = std::thread::spawn({
            let terminate_tracing_sender = Arc::clone(&terminate_tracing_sender);
            move || {
                let mut executor = fuchsia_async::LocalExecutor::default();

                // Make the initial connection to Provisioner on the executor that will run the
                // trace-writer scope. This ensures future port notifications associated with
                // Provisioner and Session will arrive on this executor.
                let (controller, tracing_stream) = executor.run_singlethreaded(async move {
                    let launcher = match hermeticity {
                        Hermeticity::NonHermetic => connect_to_protocol::<
                            fidl_fuchsia_tracing_controller::ProvisionerMarker,
                        >(),
                        Hermeticity::Hermetic(HermeticityParameters { service_prefix }) => {
                            connect_to_protocol_at::<
                                fidl_fuchsia_tracing_controller::ProvisionerMarker,
                            >(service_prefix)
                        }
                    }
                    .map_err(|e| format_err!("Failed to get tracing controller: {e:?}"))?;

                    let (socket_read, socket_write) = fidl::Socket::create_stream();
                    let tracing_stream = fuchsia_async::Socket::from_socket(socket_read);
                    let (controller, controller_server) = fidl::endpoints::create_proxy::<
                        fidl_fuchsia_tracing_controller::SessionMarker,
                    >();

                    launcher
                        .initialize_tracing(
                            controller_server,
                            &fidl_fuchsia_tracing_controller::TraceConfig {
                                categories: Some(vec!["*".to_string()]),
                                buffer_size_megabytes_hint: Some(64),
                                buffering_mode: Some(
                                    fidl_fuchsia_tracing::BufferingMode::Streaming,
                                ),
                                ..Default::default()
                            },
                            socket_write,
                        )
                        .map_err(|e| format_err!("Failed to initialize tracing: {e:?}"))?;

                    controller
                        .start_tracing(&fidl_fuchsia_tracing_controller::StartOptions::default())
                        .await
                        .map_err(|e| {
                            format_err!("Encountered FIDL error when starting trace: {e:?}")
                        })?
                        .map_err(|e| format_err!("Failed to start tracing: {e:?}"))?;

                    info!("Trace started.");
                    let _ = tracing_started_sender
                        .send(())
                        .map_err(|_| format_err!("Failed to send tracing started"))?;

                    Ok::<_, anyhow::Error>((controller, tracing_stream))
                })?;

                executor.run_singlethreaded(Self::trace_writer_entrypoint(
                    controller,
                    tracing_stream,
                    output_trace_path,
                    terminate_tracing_sender,
                    terminate_tracing_receiver,
                    trace_timeout,
                    trace_file_max_bytes,
                ))
            }
        });

        tracing_started_receiver
            .await
            .map_err(|_| format_err!("Background thread exited before tracing started"))?;

        let tracer = TraceRunner {
            id: trace_writer.thread().id(),
            terminate_tracing_sender,
            trace_writer: Arc::new(AtomicBox::new(trace_writer)),
        };

        // Stop and write a trace upon panic. This is required because Fuchsia uses the abort panic
        // strategy. If the unwind strategy were used, then the Tracing destructor would run and
        // this hook would not be necessary.
        //
        // This panic hook skips the attempt to terminate the trace when run from the same thread
        // running `trace_writer`.
        let panic_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new({
            let tracer = tracer.clone();
            move |panic_info| {
                if std::thread::current().id() != tracer.id {
                    match tracer.terminate_trace(format!("panic")) {
                        TerminationResult { termination_signal: Some(Err(e)), .. } => {
                            warn!("Failed to signal termination of trace-writer: {e:?}");
                        }
                        TerminationResult { trace_writer: Some(Err(e)), .. } => {
                            warn!("Failed to terminate trace-writer: {e:?}");
                        }
                        _ => (),
                    }
                }
                panic_hook(panic_info);
            }
        }));

        Ok(tracer)
    }

    async fn trace_writer_entrypoint(
        controller: SessionProxy,
        mut tracing_stream: fuchsia_async::Socket,
        output_trace_path: PathBuf,
        terminate_tracing_sender: Arc<AtomicBox<Sender<String>>>,
        terminate_tracing_receiver: Receiver<String>,
        trace_timeout: Duration,
        trace_file_max_bytes: usize,
    ) -> Result<(), anyhow::Error> {
        let scope = fuchsia_async::Scope::new_with_name("trace-writer");

        // Spawn a microservice whose role is to call Session.StopTracing upon receiving
        // a signal from the `terminate_tracing_receiver` oneshot channel.
        drop(scope.spawn_local(async move {
            let reason = match terminate_tracing_receiver.await {
                Err(Canceled) => format!("Receiver to terminate tracing canceled."),
                Ok(reason) => reason,
            };
            info!("Stopping trace: {reason}");
            match controller
                .stop_tracing(&fidl_fuchsia_tracing_controller::StopOptions {
                    write_results: Some(true),
                    ..Default::default()
                })
                .await
            {
                Err(e) => error!("Failed to stop tracing: {e:?}"),
                Ok(Err(e)) => error!("Failed to stop tracing: {e:?}"),
                Ok(Ok(stop_result)) => info!("Trace stopped: {stop_result:#?}"),
            };
        }));

        // Stop and write a trace if it runs for longer than `trace_timeout`. This ensures a
        // trace will be written even for tests that exceed some outer timeout that
        // in the test runner being aborted and preempting the panic hook.
        drop(scope.spawn_local({
            let terminate_tracing_sender = Arc::clone(&terminate_tracing_sender);
            async move {
                Timer::new(trace_timeout).await;
                let _: Option<Result<(), ()>> = terminate_tracing_sender
                    .send(format!("timeout ({trace_timeout:#?})"))
                    .map(|r| r.map_err(|e| warn!("{e:?}")));
            }
        }));

        let trace_writer = scope.compute_local(async move {
            let custom_artifacts = fuchsia_fs::directory::open_in_namespace(
                &output_trace_path
                    .parent()
                    .ok_or_else(|| {
                        format_err!(
                            "Failed to retrieve parent from {}",
                            output_trace_path.display()
                        )
                    })?
                    .to_string_lossy(),
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            )?;
            let fxt_file = fuchsia_fs::directory::open_file(
                &custom_artifacts,
                &output_trace_path
                    .file_name()
                    .ok_or_else(|| {
                        format_err!(
                            "Failed to retrieve file_name from {}",
                            output_trace_path.display()
                        )
                    })?
                    .to_string_lossy(),
                fio::Flags::FLAG_MUST_CREATE | fio::PERM_WRITABLE | fio::Flags::FILE_APPEND,
            )
            .await?;

            info!("Writing trace to file: {output_trace_path:?}");

            let mut buf = [0; fio::MAX_TRANSFER_SIZE as usize];
            let mut trace_file_size: usize = 0;
            let mut terminate_tracing_sender = Some(terminate_tracing_sender);
            loop {
                // Stop and write a trace when it already exceeds `trace_file_max_bytes` as a
                // safeguard against polluting infra with large trace files. We intend this
                // limit to only be encountered for tests that use a non-hermetic trace and run
                // for an unusually long time.
                //
                // In practice, stopping a non-hermetic trace after writing 5MB to a file may
                // still result in a final trace file size of more than 50MB. This is because
                // trace-manager generally avoids draining provider buffers until a request to
                // stop tracing.
                if trace_file_size >= trace_file_max_bytes {
                    match terminate_tracing_sender.take() {
                        Some(s) => {
                            let _: Option<Result<(), ()>> = s
                                .send(format!(
                                    "Exceeded max number of bytes streamed to trace file: \
                                     {trace_file_size} bytes already written."
                                ))
                                .map(|r| r.map_err(|e| warn!("{e:?}")));
                        }
                        None => (),
                    };
                }

                let read_result = tracing_stream.read(&mut buf).await;
                let bytes_read = match read_result {
                    Err(e) => {
                        error!("Error reading from socket: {:?}", e);
                        break;
                    }
                    Ok(bytes_read) => bytes_read,
                };
                if bytes_read == 0 {
                    break;
                }

                let bytes_written = match fxt_file.write(&buf[..bytes_read]).await {
                    Err(fidl_error) => {
                        error!("Failed writing to file: {fidl_error:?}");
                        break;
                    }
                    Ok(Err(raw_status)) => {
                        error!("Failed writing to file: {}", zx::Status::err_from_raw(raw_status));
                        break;
                    }
                    Ok(Ok(bytes_written)) => {
                        let bytes_written = bytes_written as usize;
                        if bytes_written != bytes_read {
                            error!(
                                "Partial write: expected {bytes_read} bytes, \
                                 wrote {bytes_written}"
                            );
                            break;
                        }
                        bytes_written
                    }
                };
                trace_file_size += bytes_written;
            }

            fxt_file.sync().await?.map_err(|raw_status| {
                format_err!(
                    "Failed to write to {output_trace_path:#?}: {}.",
                    zx::Status::err_from_raw(raw_status)
                )
            })?;
            info!("Trace written to file: {output_trace_path:#?} ({trace_file_size} bytes)");

            // TODO: Re-enable this assertion once trace files are guaranteed to meet the size threshold again.

            Ok(())
        });

        trace_writer.await
    }

    pub fn terminate_trace(&self, reason: String) -> TerminationResult {
        let termination_signal_result = match self.terminate_tracing_sender.send(reason) {
            Some(Err(reason)) => {
                return TerminationResult {
                    termination_signal: Some(Err(format_err!(
                        "Failed to send signal to terminate tracing: {reason}"
                    ))),
                    trace_writer: None,
                };
            }
            Some(Ok(())) => Some(Ok(())),
            // Continue even in this case because there may have been a race to send a signal for
            // termination from a source that cannot wait on `trace_writer`.
            None => None,
        };

        // It's possible concurrent calls could race to this `AtomicBox::take`. Consequently, the
        // caller that sent the signal to terminate the trace may not be the caller that waits for
        // `trace_writer`. This is okay since the goal is for some caller to wait for `trace_writer`
        // before the process exits, so the trace will be written regardless.
        let trace_writer = match self.trace_writer.take() {
            None => {
                return TerminationResult {
                    termination_signal: termination_signal_result,
                    trace_writer: None,
                };
            }
            Some(trace_writer) => trace_writer,
        };

        // Join `trace_writer` so the trace file is written before this function returns.
        //
        // This std::thread::join is risky because the current executor will block on the executor
        // running in trace-writer. The trace-writer executor does not block awaiting any execution
        // in the current executor since the only shared resource is `terminate_tracing_sender`
        // which must have been consumed by this point.
        //
        // NOTE: Modifications to trace-writer must avoid consuming any FIDL channels created
        // on the current executor since port notifications associated with those channels would be
        // sent to the current executor and therefore not wakeup the future in trace-writer. This
        // caution is generally applicable to all FIDL channels as they're not designed to be sent
        // between executors.
        match trace_writer.join() {
            Err(e) => TerminationResult {
                termination_signal: termination_signal_result,
                trace_writer: Some(Err(format_err!("Failed to join trace writer thread: {e:?}"))),
            },
            Ok(Err(e)) => TerminationResult {
                termination_signal: termination_signal_result,
                trace_writer: Some(Err(format_err!(
                    "Trace writer thread exited with error: {e:?}"
                ))),
            },
            Ok(Ok(())) => TerminationResult {
                termination_signal: termination_signal_result,
                trace_writer: Some(Ok(())),
            },
        }
    }
}
