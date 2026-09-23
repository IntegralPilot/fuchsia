// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::triggers::{Trigger, TriggerAction, TriggersWatcher};
use crate::{TracingError, trace_shutdown};
use async_lock::Mutex;
use flex_client::{AsyncSocket, ProxyHasDomain, socket_to_async};
use flex_fuchsia_tracing_controller::{self as trace, StopResult, TraceConfig};
use fuchsia_async::Task;
use futures::io::AsyncWrite;
use futures::prelude::*;
use futures::task::{Context as FutContext, Poll};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::{Duration, Instant};
use zstd::stream::raw::Operation;

static SERIAL: AtomicU64 = AtomicU64::new(100);

pub struct TraceTask {
    /// Domain for FDomain client handles. It is here to prevent the domain
    /// being dropped while it could still be used.
    _domain: flex_client::ClientArg,
    /// Unique identifier for this task. The value of this id monotonicallly increases.
    task_id: u64,
    /// Tag used to identify this task in the log.
    debug_tag: String,
    /// Trace configuration.
    config: trace::TraceConfig,
    /// Requested categories. These are unexpanded from the user.
    requested_categories: Vec<String>,
    /// Duration to capture trace. None indicates capture until canceled.
    duration: Option<Duration>,
    /// Triggers for terminating the trace.
    triggers: Vec<Trigger>,
    /// True when the task is cleaning up.
    terminating: Arc<AtomicBool>,
    /// Start time of the task.
    start_time: Instant,
    /// Channel used to shutdown this task.
    shutdown_sender: async_channel::Sender<()>,
    /// The task.
    task: Task<Option<trace::StopResult>>,
    /// The socket to read the trace data from when tracing is completed.
    read_socket: Option<AsyncSocket>,
    /// The compression algorithm to use.
    compression: trace::CompressionType,
    /// True when the task was cancelled (aborted).
    cancelled: Arc<AtomicBool>,
}

/// Implement Debug explicitly since `flex_client::ClientArg` does not implement Debug,
/// so we skip it in this implementation.
impl std::fmt::Debug for TraceTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceTask")
            .field("task_id", &self.task_id)
            .field("debug_tag", &self.debug_tag)
            .field("config", &self.config)
            .field("requested_categories", &self.requested_categories)
            .field("duration", &self.duration)
            .field("triggers", &self.triggers)
            .field("terminating", &self.terminating)
            .field("start_time", &self.start_time)
            .field("shutdown_sender", &self.shutdown_sender)
            .field("read_socket", &self.read_socket)
            .field("compression", &self.compression)
            .field("cancelled", &self.cancelled)
            .finish()
    }
}

// This is just implemented for convenience so the wrapper is await-able.
impl Future for TraceTask {
    type Output = Option<trace::StopResult>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut FutContext<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.task).poll(cx)
    }
}

impl TraceTask {
    pub async fn new(
        debug_tag: String,
        config: trace::TraceConfig,
        duration: Option<Duration>,
        triggers: Vec<Trigger>,
        requested_categories: Option<Vec<String>>,
        compression: trace::CompressionType,
        provisioner: trace::ProvisionerProxy,
    ) -> Result<Self, TracingError> {
        // Start the tracing session immediately. Maybe we should consider separating the creating
        // of the session and the actual starting of it. This seems like a side-effect.
        log::info!("TraceTask::new called with compression: {:?}", compression);
        let task_id = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let domain = provisioner.domain();
        let (client, server) = domain.create_stream_socket();
        let (client_end, server_end) = domain.create_proxy::<trace::SessionMarker>();
        provisioner.initialize_tracing(server_end, &config, server)?;

        client_end
            .start_tracing(&trace::StartOptions::default())
            .await?
            .map_err(Into::<TracingError>::into)?;

        let logging_prefix_og = format!("Task {task_id} ({debug_tag})");
        let terminate_result = Arc::new(Mutex::new(None));
        let (shutdown_sender, shutdown_receiver) = async_channel::bounded::<()>(1);

        let controller = client_end.clone();
        let shutdown_controller = client_end.clone();
        let triggers_watcher =
            TriggersWatcher::new(controller, triggers.clone(), shutdown_receiver);
        let terminating = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let terminating_clone = terminating.clone();
        let cancelled_clone = cancelled.clone();
        let terminate_result_clone = terminate_result.clone();

        let shutdown_fut = {
            let logging_prefix = logging_prefix_og.clone();
            async move {
                if terminating_clone
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    log::info!("{logging_prefix} Running shutdown future.");
                    let is_cancelled = cancelled_clone.load(std::sync::atomic::Ordering::Relaxed);
                    let result = trace_shutdown(&shutdown_controller, is_cancelled).await;

                    let mut done = terminate_result_clone.lock().await;
                    if done.is_none() {
                        match result {
                            Ok(stop) => {
                                log::info!("{logging_prefix} call to trace_shutdown successful.");
                                *done = Some(stop)
                            }
                            Err(e) => {
                                log::error!(
                                    "{logging_prefix} call to trace_shutdown failed: {e:?}"
                                );
                            }
                        }
                    }
                } else {
                    log::debug!("Shutdown already triggered");
                }
                "shutdown future completed"
            }
        };

        let task = Self::make_task(
            task_id,
            debug_tag,
            duration,
            shutdown_fut,
            triggers_watcher,
            terminate_result,
        );
        Ok(Self {
            _domain: domain,
            task_id,
            debug_tag: logging_prefix_og,
            config,
            duration,
            triggers: triggers.clone(),
            terminating,
            requested_categories: requested_categories.unwrap_or_default(),
            start_time: Instant::now(),
            shutdown_sender,
            read_socket: Some(socket_to_async(client)),
            compression,
            task,
            cancelled,
        })
    }

    /// Shutdown the tracing task.
    async fn shutdown(self) -> Result<trace::StopResult, TracingError> {
        if !self.terminating.load(std::sync::atomic::Ordering::SeqCst) {
            log::info!("{} Sending shutdown message.", self.debug_tag);
            if self.shutdown_sender.send(()).await.is_err() {
                log::warn!(
                    "{} Shutdown channel was closed. Task may have already completed.",
                    self.debug_tag
                );
            }
        } else {
            log::debug!("{} Shutdown already in progress.", self.debug_tag);
        }

        self.await
            .map(|r| Ok(r))
            .unwrap_or_else(|| Err(TracingError::RecordingStop("Error awaiting".into())))
    }

    /// Abort the tracing task without writing results.
    pub async fn abort(mut self) -> Result<trace::StopResult, TracingError> {
        if !self.terminating.load(std::sync::atomic::Ordering::SeqCst) {
            log::info!("{} Sending cancel message for task", self.debug_tag);
            self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
            if self.shutdown_sender.send(()).await.is_err() {
                log::warn!(
                    "{} Shutdown channel was closed. Task may have already completed.",
                    self.debug_tag
                );
            }
        } else {
            log::debug!("{} Shutdown already in progress.", self.debug_tag);
        }

        // Close the socket without reading.
        if let Some(mut socket) = self.read_socket.take() {
            let res = socket.close().await;
            if res.is_err() {
                log::warn!("{} Failed to close socket: {:?}", self.debug_tag, res);
            }
            drop(socket);
        }
        self.shutdown().await
    }

    fn make_task(
        task_id: u64,
        debug_tag: String,
        duration: Option<Duration>,
        shutdown_fut: impl Future<Output = &'static str> + 'static + std::marker::Send,
        trigger_watcher: TriggersWatcher<'static>,
        terminate_result: Arc<Mutex<Option<StopResult>>>,
    ) -> Task<Option<trace::StopResult>> {
        Task::local(async move {
            let mut timeout_fut = Box::pin(async move {
                if let Some(duration) = duration {
                    fuchsia_async::Timer::new(duration).await;
                } else {
                    std::future::pending::<()>().await;
                }
            })
            .fuse();
            let mut trigger_fut = trigger_watcher.fuse();

            futures::select! {
                // Timeout, clean up and wait for copying to finish.
                _ = timeout_fut => {
                    log::info!("Trace {task_id} (debug_tag): timeout of {} successfully completed. Stopping and cleaning up.",
                     duration.map(|d| format!("{} secs", d.as_secs())).unwrap_or_else(|| "infinite?".into()));

                    shutdown_fut.await;
                     log::debug!("done with timeout!");

                }

                // Trigger hit, shutdown and copy the trace.
                action = trigger_fut => {
                    if let Some(action) = action {
                        match action {
                            TriggerAction::Terminate => {
                                log::info!("Task {task_id} ({debug_tag}): received terminate trigger");
                            }
                        }
                    } else {
                        // This usually means the proxy was closed.
                        log::debug!("Task {task_id} ({debug_tag}): Trigger future completed without an action!");
                    }
                    shutdown_fut.await;
                     log::debug!("done with trigger future!");
                }
            };
            log::debug!("end of task waiting for terminate_result lock");
            let res = terminate_result.lock().await.clone();
            log::debug!("got res in task is some: {}", res.is_some());
            res
        })
    }

    pub fn triggers(&self) -> Vec<Trigger> {
        self.triggers.clone()
    }
    pub fn config(&self) -> TraceConfig {
        self.config.clone()
    }

    pub fn start_time(&self) -> Instant {
        self.start_time
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration.clone()
    }

    pub fn requested_categories(&self) -> Vec<String> {
        self.requested_categories.clone()
    }

    pub fn task_id(&self) -> u64 {
        self.task_id
    }

    /// Signals the trace session to stop, copies all trace data to the
    /// provided writer, and awaits task completion.
    pub async fn stop_and_receive_data<W>(
        mut self,
        mut writer: W,
    ) -> Result<trace::StopResult, TracingError>
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        if !self.terminating.load(std::sync::atomic::Ordering::SeqCst) {
            log::info!("{} Sending shutdown message for task", self.debug_tag);
            if self.shutdown_sender.send(()).await.is_err() {
                log::warn!(
                    "{} Shutdown channel was closed. Task may have already completed.",
                    self.debug_tag
                );
            }
        } else {
            log::debug!("{} Shutdown already in progress.", self.debug_tag);
        }

        let mut read_socket = self.read_socket.take().unwrap();
        let res = match self.compression {
            trace::CompressionType::Zstd => compress_zstd(&mut read_socket, &mut writer).await,
            _ => futures::io::copy(&mut read_socket, &mut writer)
                .await
                .map(|_| ())
                .map_err(|e| TracingError::GeneralError(format!("{e:?}"))),
        };

        if res.is_ok() { self.shutdown().await } else { Err(res.err().unwrap()) }
    }

    /// Waits for the tracing task to complete and copies the trace data to the writer.
    /// If the tracing should be stopped vs. waiting, call |stop_and_receive_data|.
    pub async fn await_completion_and_receive_data<W>(
        mut self,
        mut writer: W,
    ) -> Result<StopResult, TracingError>
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let mut read_socket = self.read_socket.take().unwrap();
        let res = match self.compression {
            trace::CompressionType::Zstd => compress_zstd(&mut read_socket, &mut writer).await,
            _ => futures::io::copy(&mut read_socket, &mut writer)
                .await
                .map(|_| ())
                .map_err(|e| TracingError::RecordingStop(e.to_string())),
        };

        match res {
            Ok(_) => match self.await {
                Some(r) => Ok(r),
                None => Err(TracingError::RecordingStop("could not await task".into())),
            },
            Err(e) => Err(e),
        }
    }
}

async fn compress_zstd<R, W>(mut reader: R, mut writer: W) -> Result<(), TracingError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut encoder = zstd::stream::raw::Encoder::new(0)
        .map_err(|e| TracingError::GeneralError(format!("zstd init: {e:?}")))?;
    // 128KB is the recommended size for Zstd (ZSTD_CStreamInSize/ZSTD_CStreamOutSize)
    let mut input_buf = vec![0u8; 128 * 1024];
    let mut output_buf = vec![0u8; 128 * 1024];

    // Read the stream until EOF, compressing and writing out fully compressed buffers as we go.
    while let n = reader
        .read(&mut input_buf)
        .await
        .map_err(|e| TracingError::GeneralError(format!("read: {e:?}")))?
        && n > 0
    {
        let mut read_offset = 0;
        while read_offset < n {
            let status = encoder
                .run_on_buffers(&input_buf[read_offset..n], &mut output_buf)
                .map_err(|e| TracingError::GeneralError(format!("zstd run: {e:?}")))?;
            read_offset += status.bytes_read;
            if status.bytes_written > 0 {
                writer
                    .write_all(&output_buf[..status.bytes_written])
                    .await
                    .map_err(|e| TracingError::GeneralError(format!("write: {e:?}")))?;
            }
        }
    }

    // Flush remaining of the last compressed buffer.
    loop {
        let mut out_wrapper = zstd::stream::raw::OutBuffer::around(&mut output_buf);
        let remaining = encoder
            .finish(&mut out_wrapper, true)
            .map_err(|e| TracingError::GeneralError(format!("zstd finish: {e:?}")))?;
        let bytes = out_wrapper.as_slice();
        if !bytes.is_empty() {
            writer
                .write_all(bytes)
                .await
                .map_err(|e| TracingError::GeneralError(format!("write: {e:?}")))?;
        }
        if remaining == 0 {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flex_client::fidl::Responder;
    use flex_fuchsia_tracing_controller::StartError;

    const FAKE_CONTROLLER_TRACE_OUTPUT: &'static str = "HOWDY HOWDY HOWDY";
    fn setup_fake_provisioner_proxy_with_payload(
        start_error: Option<StartError>,
        trigger_name: Option<&'static str>,
        expected_write_results: bool,
        payload_bytes: impl AsRef<[u8]>,
    ) -> trace::ProvisionerProxy {
        let payload_bytes = payload_bytes.as_ref().to_vec();
        let client = fdomain_local::local_client_empty();
        let (proxy, mut stream) = client.create_proxy_and_stream::<trace::ProvisionerMarker>();
        fuchsia_async::Task::local(async move {
            let _client = client;
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    trace::ProvisionerRequest::InitializeTracing { controller, output, .. } => {
                        let mut stream = controller.into_stream();
                        let mut async_output = socket_to_async(output);
                        while let Ok(Some(req)) = stream.try_next().await {
                            match req {
                                trace::SessionRequest::StartTracing { responder, .. } => {
                                    let response = match start_error {
                                        Some(e) => Err(e),
                                        None => Ok(()),
                                    };
                                    responder.send(response).expect("Failed to start")
                                }
                                trace::SessionRequest::StopTracing { responder, payload } => {
                                    if start_error.is_some() {
                                        responder
                                            .send(Err(trace::StopError::NotStarted))
                                            .expect("Failed to stop");
                                    } else {
                                        assert_eq!(
                                            payload.write_results.unwrap(),
                                            expected_write_results
                                        );
                                        if expected_write_results && !payload_bytes.is_empty() {
                                            let _ = async_output.write_all(&payload_bytes).await;
                                        }
                                        let _ = async_output.close().await;
                                        let stop_result = trace::StopResult {
                                            provider_stats: Some(vec![]),
                                            ..Default::default()
                                        };
                                        responder.send(Ok(&stop_result)).expect("Failed to stop");
                                    }
                                }
                                trace::SessionRequest::WatchAlert { responder } => {
                                    if let Some(trigger) = trigger_name {
                                        responder.send(trigger).expect("Unable to send alert");
                                    } else {
                                        responder.drop_without_shutdown();
                                    }
                                }
                                r => panic!("unexpected request: {:#?}", r),
                            }
                        }
                    }
                    r => panic!("unexpected request: {:#?}", r),
                }
            }
        })
        .detach();
        proxy
    }

    #[fuchsia::test]
    async fn test_trace_task_start_stop_write_check_with_vec() {
        let provisioner = setup_fake_provisioner_proxy_with_payload(
            None,
            None,
            true,
            FAKE_CONTROLLER_TRACE_OUTPUT,
        );

        let trace_task = TraceTask::new(
            "test_trace_start_stop_write_check".into(),
            trace::TraceConfig::default(),
            None,
            vec![],
            None,
            trace::CompressionType::None,
            provisioner,
        )
        .await
        .expect("tracing task started");

        let shutdown_result = trace_task.shutdown().await.expect("tracing shutdown");
        assert_eq!(
            shutdown_result,
            trace::StopResult { provider_stats: Some(vec![]), ..Default::default() }.into()
        );
    }

    #[cfg(not(target_os = "fuchsia"))]
    #[fuchsia::test]
    async fn test_trace_task_start_stop_write_check_with_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output = temp_dir.path().join("trace-test.fxt");

        let provisioner = setup_fake_provisioner_proxy_with_payload(
            None,
            None,
            true,
            FAKE_CONTROLLER_TRACE_OUTPUT,
        );
        let writer = async_fs::File::create(&output).await.unwrap();

        let trace_task = TraceTask::new(
            "test_trace_start_stop_write_check".into(),
            trace::TraceConfig::default(),
            None,
            vec![],
            None,
            trace::CompressionType::None,
            provisioner,
        )
        .await
        .expect("tracing task started");

        let shutdown_result =
            trace_task.stop_and_receive_data(writer).await.expect("tracing shutdown");

        let res = async_fs::read_to_string(&output).await.unwrap();
        assert_eq!(res, FAKE_CONTROLLER_TRACE_OUTPUT.to_string());
        let expected = trace::StopResult { provider_stats: Some(vec![]), ..Default::default() };
        assert_eq!(shutdown_result, expected);
    }

    #[fuchsia::test]
    async fn test_trace_error_handling_already_started() {
        let provisioner = setup_fake_provisioner_proxy_with_payload(
            Some(StartError::AlreadyStarted),
            None,
            true,
            FAKE_CONTROLLER_TRACE_OUTPUT,
        );

        let trace_task_result = TraceTask::new(
            "test_trace_error_handling_already_started".into(),
            trace::TraceConfig::default(),
            None,
            vec![],
            None,
            trace::CompressionType::None,
            provisioner,
        )
        .await
        .err();

        assert_eq!(trace_task_result, Some(TracingError::RecordingAlreadyStarted));
    }

    #[cfg(not(target_os = "fuchsia"))]
    #[fuchsia::test]
    async fn test_trace_task_start_with_duration() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output = temp_dir.path().join("trace-test.fxt");

        let provisioner = setup_fake_provisioner_proxy_with_payload(
            None,
            None,
            true,
            FAKE_CONTROLLER_TRACE_OUTPUT,
        );
        let writer = async_fs::File::create(&output).await.unwrap();

        let trace_task = TraceTask::new(
            "test_trace_task_start_with_duration".into(),
            trace::TraceConfig::default(),
            Some(Duration::from_millis(100)),
            vec![],
            None,
            trace::CompressionType::None,
            provisioner,
        )
        .await
        .expect("tracing task started");

        let res = trace_task.await_completion_and_receive_data(writer).await;
        if let Some(ref stop_result) = res.as_ref().ok() {
            assert!(stop_result.provider_stats.is_some());
        } else {
            panic!("Expected stop result from trace_task.await: {res:?}");
        }

        let mut f = async_fs::File::open(std::path::PathBuf::from(output)).await.unwrap();
        let mut res = String::new();
        f.read_to_string(&mut res).await.unwrap();
        assert_eq!(res, FAKE_CONTROLLER_TRACE_OUTPUT.to_string());
    }

    #[cfg(not(target_os = "fuchsia"))]
    #[fuchsia::test]
    async fn test_triggers_valid() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output = temp_dir.path().join("trace-test.fxt");
        let alert_name = "some_alert";
        let provisioner = setup_fake_provisioner_proxy_with_payload(
            None,
            Some(alert_name.into()),
            true,
            FAKE_CONTROLLER_TRACE_OUTPUT,
        );
        let writer = async_fs::File::create(output.clone()).await.unwrap();

        let trace_task = TraceTask::new(
            "test_triggers_valid".into(),
            trace::TraceConfig::default(),
            None,
            vec![Trigger {
                alert: Some(alert_name.into()),
                action: Some(TriggerAction::Terminate),
            }],
            None,
            trace::CompressionType::None,
            provisioner,
        )
        .await
        .expect("tracing task started");

        trace_task.await_completion_and_receive_data(writer).await.unwrap();
        let res = async_fs::read_to_string(&output).await.unwrap();
        assert_eq!(res, FAKE_CONTROLLER_TRACE_OUTPUT.to_string());
    }

    #[fuchsia::test]
    async fn test_trace_task_abort() {
        let provisioner = setup_fake_provisioner_proxy_with_payload(
            None,
            None,
            false,
            FAKE_CONTROLLER_TRACE_OUTPUT,
        );

        let trace_task = TraceTask::new(
            "test_trace_task_abort".into(),
            trace::TraceConfig::default(),
            None,
            vec![],
            None,
            trace::CompressionType::None,
            provisioner,
        )
        .await
        .expect("tracing task started");

        let shutdown_result = trace_task.abort().await.expect("tracing abort");
        assert_eq!(
            shutdown_result,
            trace::StopResult { provider_stats: Some(vec![]), ..Default::default() }
        );
    }

    struct FailingAsyncWriter {
        fail_after_bytes: usize,
        written: usize,
    }

    impl AsyncWrite for FailingAsyncWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut FutContext<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if self.written >= self.fail_after_bytes {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "simulated broken pipe in stress test",
                )));
            }
            let allowed = std::cmp::min(buf.len(), self.fail_after_bytes - self.written);
            self.written += allowed;
            Poll::Ready(Ok(allowed))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut FutContext<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut FutContext<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[fuchsia::test]
    async fn test_stress_faulty_writer_error_handling() {
        let payload = b"PAYLOAD_FOR_FAULTY_WRITER_TEST";

        // Failure mode 1: Immediate failure on uncompressed stop_and_receive_data
        {
            let provisioner =
                setup_fake_provisioner_proxy_with_payload(None, None, true, payload.to_vec());
            let trace_task = TraceTask::new(
                "faulty_writer_1".into(),
                trace::TraceConfig::default(),
                None,
                vec![],
                None,
                trace::CompressionType::None,
                provisioner,
            )
            .await
            .unwrap();

            let res = trace_task
                .stop_and_receive_data(FailingAsyncWriter { fail_after_bytes: 0, written: 0 })
                .await;
            assert!(res.is_err(), "Expected error when writer fails immediately");
        }

        // Failure mode 2: Immediate failure on uncompressed await_completion_and_receive_data
        {
            let provisioner =
                setup_fake_provisioner_proxy_with_payload(None, None, true, payload.to_vec());
            let trace_task = TraceTask::new(
                "faulty_writer_2".into(),
                trace::TraceConfig::default(),
                Some(Duration::from_millis(10)),
                vec![],
                None,
                trace::CompressionType::None,
                provisioner,
            )
            .await
            .unwrap();

            let res = trace_task
                .await_completion_and_receive_data(FailingAsyncWriter {
                    fail_after_bytes: 0,
                    written: 0,
                })
                .await;
            assert!(res.is_err());
        }

        // Failure mode 3: Immediate failure on Zstd stop_and_receive_data
        {
            let provisioner =
                setup_fake_provisioner_proxy_with_payload(None, None, true, payload.to_vec());
            let trace_task = TraceTask::new(
                "faulty_writer_3".into(),
                trace::TraceConfig::default(),
                None,
                vec![],
                None,
                trace::CompressionType::Zstd,
                provisioner,
            )
            .await
            .unwrap();

            let res = trace_task
                .stop_and_receive_data(FailingAsyncWriter { fail_after_bytes: 0, written: 0 })
                .await;
            assert!(res.is_err());
        }

        // Failure mode 4: Failure mid-stream (after 10 bytes) on uncompressed
        {
            let provisioner =
                setup_fake_provisioner_proxy_with_payload(None, None, true, payload.to_vec());
            let trace_task = TraceTask::new(
                "faulty_writer_4".into(),
                trace::TraceConfig::default(),
                None,
                vec![],
                None,
                trace::CompressionType::None,
                provisioner,
            )
            .await
            .unwrap();

            let res = trace_task
                .stop_and_receive_data(FailingAsyncWriter { fail_after_bytes: 10, written: 0 })
                .await;
            assert!(res.is_err());
        }

        // Failure mode 5: Failure mid-stream (after 10 bytes) on Zstd
        {
            let provisioner =
                setup_fake_provisioner_proxy_with_payload(None, None, true, payload.to_vec());
            let trace_task = TraceTask::new(
                "faulty_writer_5".into(),
                trace::TraceConfig::default(),
                None,
                vec![],
                None,
                trace::CompressionType::Zstd,
                provisioner,
            )
            .await
            .unwrap();

            let res = trace_task
                .stop_and_receive_data(FailingAsyncWriter { fail_after_bytes: 10, written: 0 })
                .await;
            assert!(res.is_err());
        }
    }
}
