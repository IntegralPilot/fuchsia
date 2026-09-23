// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use async_lock::RwLock;
use fidl_fuchsia_tracing_controller::{
    CompressionType, ProvisionerMarker, ProvisionerProxy, RecordingError, SessionManagerRequest,
    SessionManagerRequestStream, TraceConfig, TraceOptions, TraceStatus,
};
use futures::TryStreamExt;
use log::error;
use std::fs;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use trace_task::{TraceTask, TracingError};

type Result<T> = std::result::Result<T, TracingError>;

use crate::data::OnBootTraceConfig;
/// Struct to hold on to an instance of TraceTask.
#[derive(Debug)]
struct TraceTaskEntry {
    pub task: TraceTask,
    pub options: TraceOptions,
}

#[derive(Default, Clone)]
pub(crate) struct TracingProtocol {
    task_entry: Arc<RwLock<Option<TraceTaskEntry>>>,
}

const ON_BOOT_CONFIG_FILE: &str = "/traces/on_boot_trace.json";

// Based on ffx protocol of the same name.
impl TracingProtocol {
    /// Dictates how the handle function is invoked across the lifetime of a
    /// single FIDL request stream. The default is to handle each request in
    /// serial. This can be changed as needed, but will likely only ever need
    /// to remain the default implementation.
    pub(crate) async fn serve<'a>(&'a self, mut stream: SessionManagerRequestStream) -> Result<()> {
        while let Ok(Some(req)) = stream.try_next().await {
            self.handle(req).await?
        }
        Ok(())
    }

    /// Handles each individual request coming from a FIDL request stream.
    async fn handle(&self, req: SessionManagerRequest) -> Result<()> {
        match req {
            SessionManagerRequest::StartTraceSession { config, options, responder, .. } => {
                log::info!("StartTraceSession called");

                let res = self.start_recording(options, config).await;
                let result: std::result::Result<u64, RecordingError> = res.map_err(Into::into);
                if let Err(e) = responder.send(result) {
                    error!("Error sending start trace session response: {:?}", e);
                    return Err(TracingError::FidlError(e));
                }
                Ok(())
            }
            SessionManagerRequest::EndTraceSession { output, responder, .. } => {
                log::info!("EndTraceSession called");
                if let Some(TraceTaskEntry { task, options }) = self.task_entry.write().await.take()
                {
                    let async_output = fidl::AsyncSocket::from_socket(output);
                    let task_result = if options.duration_ns.is_none() {
                        // shutdown and copy
                        task.stop_and_receive_data(async_output).await
                    } else {
                        task.await_completion_and_receive_data(async_output).await
                    };
                    responder
                        .send(match task_result {
                            Ok(ref result) => Ok((&options, result)),
                            Err(e) => Err(e.into()),
                        })
                        .map_err(Into::into)
                } else {
                    log::warn!("no trace task found");
                    return responder
                        .send(Err(RecordingError::NoSuchTraceFile))
                        .map_err(Into::into);
                }
            }
            SessionManagerRequest::AbortTraceSession { responder, .. } => {
                log::info!("AbortTraceSession called");
                if let Some(TraceTaskEntry { task, .. }) = self.task_entry.write().await.take() {
                    let task_result = task.abort().await;
                    responder
                        .send(match task_result {
                            Ok(_) => Ok(()),
                            Err(e) => Err(e.into()),
                        })
                        .map_err(Into::into)
                } else {
                    log::warn!("no trace task found");
                    return responder
                        .send(Err(RecordingError::NoSuchTraceFile))
                        .map_err(Into::into);
                }
            }
            SessionManagerRequest::Status { responder } => {
                log::info!("Status called");
                if let Some(ref entry) = *self.task_entry.read().await {
                    let remaining_runtime = entry.task.duration().map(|d| {
                        d.saturating_sub(entry.task.start_time().elapsed()).as_nanos() as i64
                    });
                    responder
                        .send(Ok(&TraceStatus {
                            duration: entry.task.duration().map(|d| d.as_nanos() as i64),
                            remaining_runtime,
                            config: Some(entry.task.config()),
                            task_id: Some(entry.task.task_id()),
                            ..Default::default()
                        }))
                        .map_err(Into::into)
                } else {
                    responder.send(Err(RecordingError::NoSuchTraceFile)).map_err(Into::into)
                }
            }
            SessionManagerRequest::GetKnownCategories { responder } => {
                let provisioner = provisioner_proxy()?;

                match provisioner.get_known_categories().await {
                    Ok(categories) => {
                        if let Err(e) = responder.send(&categories) {
                            error!("Error sending categories: {:?}", e);
                            Err(e.into())
                        } else {
                            Ok(())
                        }
                    }
                    Err(e) => {
                        error!("Error getting known categories: {:?}", e);
                        Err(e.into())
                    }
                }
            }
            SessionManagerRequest::GetProviders { responder } => {
                let provisioner = provisioner_proxy()?;

                match provisioner.get_providers().await {
                    Ok(providers) => {
                        if let Err(e) = responder.send(&providers) {
                            error!("Error sending providers: {:?}", e);
                            Err(e.into())
                        } else {
                            Ok(())
                        }
                    }
                    Err(e) => {
                        error!("Error getting providers: {:?}", e);
                        Err(e.into())
                    }
                }
            }
            SessionManagerRequest::StartTraceSessionOnBoot {
                config, options, responder, ..
            } => {
                log::info!("StartTraceSessionOnBoot called");

                let res = self.save_start_on_boot_config(options, config).await;
                log::debug!("save_start_on_boot_config returned {res:?}");
                let result: std::result::Result<(), RecordingError> = res.map_err(Into::into);
                if let Err(e) = responder.send(result) {
                    error!("Error sending start_on_boot response: {:?}", e);
                    return Err(TracingError::FidlError(e));
                }
                Ok(())
            }
            SessionManagerRequest::_UnknownMethod { ordinal, .. } => {
                log::error!("Received unknown SessionManager method with ordinal: {}", ordinal);
                Ok(())
            }
        }
    }

    // StartRecording a new trace session when the component starts up.
    async fn save_start_on_boot_config(
        &self,
        options: TraceOptions,
        trace_config: TraceConfig,
    ) -> Result<()> {
        let on_boot_config =
            OnBootTraceConfig { options: options.into(), config: trace_config.into() };
        let serialized = serde_json::to_string_pretty(&on_boot_config)
            .map_err(|e| TracingError::GeneralError(format!("serialization failed: {e:?}")))?;

        // Explicitly open, write, and sync to ensure data is written.
        let mut boot_config_file = fs::File::create(ON_BOOT_CONFIG_FILE).map_err(|e| {
            TracingError::GeneralError(format!("unable to create {ON_BOOT_CONFIG_FILE}: {e:?}"))
        })?;
        boot_config_file.write_all(serialized.as_bytes()).map_err(|e| {
            TracingError::GeneralError(format!("unable to write {ON_BOOT_CONFIG_FILE}: {e:?}"))
        })?;
        boot_config_file.sync_data().map_err(|e| {
            TracingError::GeneralError(format!("unable to sync {ON_BOOT_CONFIG_FILE}: {e:?}"))
        })?;
        Ok(())
    }

    // Reads the on boot configuration from storage, the deletes it.
    fn get_on_boot_config(&self) -> Result<Option<OnBootTraceConfig>> {
        let data = match fs::read_to_string(ON_BOOT_CONFIG_FILE) {
            Ok(contents) => Ok(Some(contents)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("{ON_BOOT_CONFIG_FILE} not found, skipping on-boot trace check");
                Ok(None)
            }
            Err(e) => {
                log::error!("Failed to read {ON_BOOT_CONFIG_FILE}: {e:?}");
                let error = TracingError::GeneralError(format!(
                    "failed to deserialize on_boot_trace.json: {e:?}"
                ));
                Err(error)
            }
        };
        // Remove the on boot config file if present.
        let config_path = std::path::Path::new(ON_BOOT_CONFIG_FILE);
        if config_path.exists() {
            if let Err(e) = std::fs::remove_file(ON_BOOT_CONFIG_FILE) {
                log::error!("Could not remove {ON_BOOT_CONFIG_FILE}: {e:?}");
            } else {
                log::debug!("Removed {ON_BOOT_CONFIG_FILE} after reading");
            }
        }

        if let Some(contents) = data? {
            let on_boot_config: OnBootTraceConfig =
                serde_json::from_str(&contents).map_err(|e| {
                    TracingError::GeneralError(format!(
                        "failed to deserialize on_boot_trace.json: {e:?}"
                    ))
                })?;
            Ok(Some(on_boot_config))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn check_for_on_boot_tracing(&self) -> Result<()> {
        if let Some(on_boot_config) = self.get_on_boot_config()? {
            log::info!("Starting on-boot trace session from config");
            let res = self
                .start_recording(on_boot_config.options.into(), on_boot_config.config.into())
                .await;

            if let Err(e) = res {
                log::error!("Failed to start on-boot trace: {e:?}");
            }
        }
        Ok(())
    }

    // StartRecording handler for the task protocol. The return
    // is a unique task id for the trace. The ids are reset on component restart.
    async fn start_recording<'a>(
        &self,
        options: TraceOptions,
        trace_config: TraceConfig,
    ) -> Result<u64> {
        let provisioner = provisioner_proxy()?;

        // Check for existing trace task. Currently we only support one at a time.
        // Get the write lock and check. It is not ideal to hold a lock across async functions,
        // but in this case we want to make sure there is only one trace task running at a time.
        let mut task_entry = self.task_entry.write().await;

        if task_entry.is_some() {
            log::error!("Trace task already running!!");
            return Err(TracingError::RecordingAlreadyStarted);
        }

        let task = match TraceTask::new(
            // Use the target info as the task name
            "trace_task".into(),
            trace_config,
            options.duration_ns.map(|d| Duration::from_nanos(d as u64)),
            options
                .triggers
                .clone()
                .map(|tv| tv.iter().map(Into::into).collect())
                .unwrap_or(vec![]),
            options.requested_categories.clone(),
            options.compression.unwrap_or(CompressionType::None),
            provisioner,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                log::warn!("unable to start trace: {:?}", e);
                return Err(e.into());
            }
        };
        let task_id = task.task_id();

        *task_entry = Some(TraceTaskEntry { task, options });
        Ok(task_id)
    }
}

fn provisioner_proxy() -> Result<ProvisionerProxy> {
    match fuchsia_component::client::connect_to_protocol::<ProvisionerMarker>() {
        Ok(p) => Ok(p),
        Err(e) => {
            log::error!("getting target controller proxy: {:?}", e);
            Err(TracingError::GeneralError(format!("{e:?}")))
        }
    }
}
