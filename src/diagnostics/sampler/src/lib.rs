// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::SamplerConfig;
use crate::project::Project;
use anyhow::Error as AnyhowError;
use argh::FromArgs;
use diagnostics_reader::drain_batch_iterator;
use fidl::endpoints::{ControlHandle, RequestStream, create_endpoints};
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_hardware_power_statecontrol::{
    ShutdownWatcherMarker, ShutdownWatcherRegisterMarker, ShutdownWatcherRequest,
};
use fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker;
use fuchsia_component_client::connect_to_protocol;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use futures::future::{Either, select};
use futures::stream::{self, StreamExt};
use inspect_runtime::publish;
use itertools::Itertools;
use log::{info, warn};
use sampler_component_config::Config;
use std::sync::Arc;

mod config;
mod error;
mod project;

/// Arguments used to configure sampler.
#[derive(Debug, Default, FromArgs, PartialEq)]
#[argh(subcommand, name = "sampler")]
pub struct Args {}

pub const PROGRAM_NAME: &str = "sampler";

pub async fn main() -> Result<(), AnyhowError> {
    info!("Sampler starting up");
    component::health().set_starting_up();

    let _inspect = publish(component::inspector(), Default::default());

    let execution_stats = component::inspector().root().create_child("sampler_executor_stats");
    let config = SamplerConfig::new(Config::take_from_startup_handle(), &execution_stats)?;

    let sampler = connect_to_protocol::<fdiagnostics::SampleMarker>()?;

    for chunk in &config
        .sample_data()
        .into_iter()
        .chunks(fdiagnostics::MAX_SAMPLE_PARAMETERS_PER_SET as usize)
    {
        sampler.set(&fdiagnostics::SampleParameters {
            data: Some(chunk.collect()),
            ..Default::default()
        })?;
    }

    let (sample_sink_client, sample_sink_server) =
        create_endpoints::<fdiagnostics::SampleSinkMarker>();

    if let Err(e) = sampler.commit(sample_sink_client).await? {
        match e {
            fdiagnostics::ConfigurationError::SamplePeriodTooSmall => {
                return Err(anyhow::anyhow!(
                    "Configured sample period was too small, indicating a config bug. Exiting."
                ));
            }
            err => warn!(err:?; "Sampler encountered non-fatal error. Review Archivist's logs."),
        }
    }

    let metric_logger_factory = connect_to_protocol::<MetricEventLoggerFactoryMarker>()?;

    let mut projects = futures::stream::iter(config.project_configs)
        .filter_map(|project_config| async {
            let project_id = *project_config.project_id;
            let stats = config.stats.projects.get(&project_config.project_id);
            match Project::new(&metric_logger_factory, project_config, stats).await {
                Ok(project) => Some(project),
                Err(e) => {
                    warn!(
                        e:?,
                        project_id;
                        "Sampler failed to configure a project",
                    );
                    None
                }
            }
        })
        .collect::<Vec<_>>()
        .await;

    let (shutdown_watcher_client, shutdown_watcher_request_stream) =
        fidl::endpoints::create_request_stream::<ShutdownWatcherMarker>();
    let shutdown_watcher_register = connect_to_protocol::<ShutdownWatcherRegisterMarker>()?;
    shutdown_watcher_register.register_watcher(shutdown_watcher_client).await?;

    let sink_stream = sample_sink_server.into_stream();
    let sample_sink_control = sink_stream.control_handle();
    let mut sink_stream = sink_stream.fuse();
    let mut shutdown_stream = Either::Left(shutdown_watcher_request_stream);
    let mut shutdown = false;
    let mut shutdown_responder = None;

    component::health().set_ok();

    loop {
        match select(shutdown_stream.next(), sink_stream.next()).await {
            Either::Left((shutdown_request, _)) => match shutdown_request {
                Some(Ok(ShutdownWatcherRequest::OnShutdown { responder, .. })) => {
                    shutdown = true;
                    shutdown_responder = Some(responder);
                    shutdown_stream = Either::Right(stream::pending());
                    if let Err(e) = sample_sink_control.send_on_now_or_never() {
                        warn!(e:?; "Failed to send on_now_or_never to sample sink");
                        break;
                    }
                }
                Some(Ok(ShutdownWatcherRequest::_UnknownMethod { .. })) => {
                    warn!("Sampler encountered unknown method on ShutdownWatcher");
                }
                Some(Err(err)) => {
                    warn!(err:?; "Sampler encountered error on ShutdownWatcher, data may be missing");
                    if shutdown {
                        break;
                    }
                }
                None => {
                    if shutdown {
                        break;
                    }
                    shutdown_stream = Either::Right(stream::pending());
                    continue;
                }
            },
            Either::Right((event, _)) => {
                let event = match event {
                    Some(Ok(event)) => event,
                    Some(Err(err)) => {
                        warn!(err:?; "Sample sink stream encountered an error");
                        break;
                    }
                    None => break,
                };

                handle_sample_sink_request(event, shutdown, &mut projects).await;

                if shutdown {
                    break;
                }
            }
        }
    }

    if let Some(responder) = shutdown_responder
        && let Err(err) = responder.send()
    {
        warn!(err:?; "Failed to send ShutdownWatcher response; shutdown coordinator may have timed out");
    }

    Ok(())
}

async fn handle_sample_sink_request(
    event: fdiagnostics::SampleSinkRequest,
    shutdown: bool,
    projects: &mut [Project<'_>],
) {
    match event {
        fdiagnostics::SampleSinkRequest::OnSampleReadied {
            event:
                fdiagnostics::SampleSinkResult::Ready(fdiagnostics::SampleReady {
                    batch_iter: Some(batch_iter),
                    seconds_since_start: Some(seconds_since_start),
                    ..
                }),
            control_handle: _control_handle,
        } => {
            let data = drain_batch_iterator::<diagnostics_data::InspectData>(Arc::new(
                batch_iter.into_proxy(),
            ))
            .filter_map(|v| async {
                match v {
                    Ok(v) => Some(v),
                    Err(e) => {
                        warn!(e:?; "Failed to read some Inspect data; skipping");
                        None
                    }
                }
            })
            .collect::<Vec<_>>()
            .await;

            let seconds_since_start = if shutdown {
                None
            } else {
                Some(zx::MonotonicDuration::from_seconds(seconds_since_start))
            };

            for project in projects {
                if let Err(e) = project.log(&data, seconds_since_start).await {
                    warn!(e:?; "Project failed to log");
                }

                // TODO: b/440153294 - Update SampleSink to allow removing selectors during
                // runtime. These selectors are returned by Project::log. That will reduce
                // the load on Archivist, but is not required for correctness of Sampler.
            }
        }
        fdiagnostics::SampleSinkRequest::OnSampleReadied {
            event:
                fdiagnostics::SampleSinkResult::Ready(fdiagnostics::SampleReady {
                    batch_iter,
                    seconds_since_start,
                    ..
                }),
            control_handle,
        } => {
            control_handle.shutdown();
            warn!(
                batch_iter:?, seconds_since_start:?;
                "Sample server sent Ready but crucial fields were None"
            );
        }
        fdiagnostics::SampleSinkRequest::OnSampleReadied {
            event: fdiagnostics::SampleSinkResult::Error(e),
            ..
        } => {
            warn!(e:?; "Sample server sent an error, data may be missing");
        }
        fdiagnostics::SampleSinkRequest::OnSampleReadied {
            event: fdiagnostics::SampleSinkResult::__SourceBreaking { .. },
            control_handle,
        }
        | fdiagnostics::SampleSinkRequest::_UnknownMethod { control_handle, .. } => {
            control_handle.shutdown();
            warn!("Sample server sent a source-breaking or unknown event")
        }
    }
}
