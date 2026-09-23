// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use attribution_data::AttributionDataProviderImpl;
use attribution_processing::digest::BucketDefinition;
use attribution_processing::{AttributionDataProvider, PrincipalDescription};
use cobalt::{collect_stalls_forever, create_metric_event_logger};
use fidl::endpoints::RequestStream;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_path};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::Property;
use fuchsia_inspect::health::Reporter;
use fuchsia_sync::Mutex;
use fuchsia_trace::duration;
use futures::{FutureExt, StreamExt, TryFutureExt, select};
use log::{error, info, warn};
use periodic_monitoring::periodic_monitoring;
use resources::Job;
use snapshot::AttributionSnapshot;
use stalls::StallProvider;
use stalls::refaults::RefaultProvider;
use std::sync::Arc;
use traces::CATEGORY_MEMORY_CAPTURE;
use zx::{BootInstant, MonotonicInstant};

use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_kernel as fkernel;
use fidl_fuchsia_memory_attribution as fattribution;
use fidl_fuchsia_memory_attribution_plugin as fattribution_plugin;
use fidl_fuchsia_metrics as fmetrics;

mod attribution_client;
mod attribution_data;
mod common;
mod resources;
mod snapshot;
mod thrashing;

/// All FIDL services that are exposed by this component's ServiceFs.
enum Service {
    /// The `fuchsia.memory.attribution.plugin.MemoryMonitor` protocol.
    MemoryMonitor(fattribution_plugin::MemoryMonitorRequestStream),
    /// The `fuchsia.memory.attribution.PageRefaultSink` protocol.
    PageRefaultSink(fattribution::PageRefaultSinkRequestStream),
}

const INTROSPECTOR_PATH: &str = "/svc/fuchsia.component.Introspector.root";

// Lower this thread priority to avoid affecting the system.
fn run_with_lower_priority() -> Result<()> {
    fuchsia_scheduler::set_role_for_this_thread("fuchsia.memory-monitor.main").into()
}

// Enable debug trace:
// 1. set `logging_minimum_severity = "debug"`
// 2. run `fx log --severity trace --moniker core/memory_monitor2`
#[fuchsia::main(logging_minimum_severity = "info")]
async fn main() -> Result<()> {
    info!("Starting memory_monitor 2");

    if let Err(e) = run_with_lower_priority() {
        error!("Failed to set scheduler role: {:?}", e);
    }
    fuchsia_inspect::component::health().set_starting_up();
    let kernel_stats = connect_to_protocol::<fkernel::StatsMarker>()
        .context("Failed to connect to the kernel stats provider")?;

    let stall_provider = stalls::StallProviderImpl::new(Arc::new(
        connect_to_protocol::<fkernel::StallResourceMarker>()?.get().await?,
    ))?;

    let page_refault_tracker = stalls::refaults::RefaultProviderImpl::default();

    let root_node = fuchsia_inspect::component::inspector().root();
    let task_health_node = root_node.create_child("task health");

    // Serves Fuchsia performance trace system.
    // https://fuchsia.dev/fuchsia-src/concepts/kernel/tracing-system
    // Watch trace category and trace kernel memory stats, until this variable goes out of scope.
    let mut kernel_trace_service = fuchsia_async::Task::spawn(traces::kernel::serve_forever(
        kernel_stats.clone(),
        stall_provider.clone(),
        page_refault_tracker.clone(),
    ))
    .fuse();
    let kernel_trace_health = task_health_node.create_string("kernel_trace_service", "ok");

    let attribution_data_provider = {
        let attribution_provider = connect_to_protocol::<fattribution::ProviderMarker>()
            .context("Failed to connect to the memory attribution provider")?;
        let introspector =
            connect_to_protocol_at_path::<fcomponent::IntrospectorMarker>(&INTROSPECTOR_PATH)
                .context("Failed to connect to the memory attribution provider")?;
        let root_job: Arc<Mutex<dyn Job>> = Arc::new(Mutex::new(
            connect_to_protocol::<fkernel::RootJobForInspectMarker>()
                .context("error connecting to the root job")?
                .get()
                .await?,
        ));
        let attribution_client = attribution_client::AttributionClientImpl::new(
            attribution_provider,
            introspector,
            root_job.lock().get_koid().context("Unable to get the root job's koid")?,
        );
        AttributionDataProviderImpl::new(attribution_client, root_job)
    };
    let fast_attribution_data_provider = attribution_data_provider.clone().with_muted_principal(
        Some(PrincipalDescription::Component(
            "core/session-manager/session:session/container".to_string(),
        )),
    );

    let bucket_definitions: Arc<[BucketDefinition]> = read_bucket_definitions().into();
    // Serves Fuchsia component inspection protocol
    // https://fuchsia.dev/fuchsia-src/development/diagnostics/inspect
    let mut inspect_nodes_service = fuchsia_async::Task::spawn(inspect_nodes::serve(
        kernel_stats.clone(),
        stall_provider.clone(),
        page_refault_tracker.clone(),
    )?)
    .fuse();
    let metric_event_logger = create_metric_event_logger(connect_to_protocol::<
        fmetrics::MetricEventLoggerFactoryMarker,
    >()?)
    .await?;
    let mut periodic_collection = fuchsia_async::Task::local({
        let attribution_data_provider = fast_attribution_data_provider.clone();
        let stall_provider = stall_provider.clone();
        let kernel_stats = kernel_stats.clone();
        let metric_event_logger = metric_event_logger.clone();
        let bucket_definitions = bucket_definitions.clone();
        async move {
            periodic_monitoring(
                kernel_stats,
                &*attribution_data_provider,
                &stall_provider,
                &metric_event_logger,
                &*bucket_definitions,
                root_node.create_child("logger"),
            )
            .await
        }
        .inspect_ok(|_| error!("Periodic collection unexpectedly exited without error"))
        .inspect_err(|e| error!("Periodic collection unexpectedly failed: {:?}", e))
    })
    .fuse();
    let periodic_collection_health =
        task_health_node.create_string("periodic_collection_health", "ok");

    let mut collect_stalls_task = fuchsia_async::Task::spawn({
        let stall_provider = stall_provider.clone();
        let metric_event_logger = metric_event_logger.clone();
        collect_stalls_forever(stall_provider, metric_event_logger)
    })
    .fuse();
    let collect_stalls_health = task_health_node.create_string("collect_stalls_health", "ok");

    let thrashing_config = thrashing::read_thrashing_config();
    let mut thrashing_detector = thrashing::ThrashingDetector::new(
        thrashing_config.clone(),
        root_node.create_child("thrashing"),
        page_refault_tracker.clone(),
        metric_event_logger.clone(),
    );
    let mut thrashing_loop = fuchsia_async::Task::spawn(async move {
        // Default to configured interval, or the default constant if 0/invalid.
        let seconds = if thrashing_config.polling_interval_seconds > 0 {
            thrashing_config.polling_interval_seconds as i64
        } else {
            thrashing::DEFAULT_POLLING_INTERVAL_SECONDS as i64
        };
        let mut interval = fuchsia_async::Interval::new(zx::Duration::from_seconds(seconds));
        while let Some(_) = interval.next().await {
            if let Err(e) = thrashing_detector.run_one_iteration().await {
                warn!("Thrashing detector failure: {:?}", e);
            }
        }
    })
    .fuse();
    let thrashing_health = task_health_node.create_string("thrashing_loop", "ok");

    let mut services = {
        let mut service_fs = ServiceFs::new();
        service_fs
            .dir("svc")
            .add_fidl_service(Service::MemoryMonitor)
            .add_fidl_service(Service::PageRefaultSink);
        service_fs.take_and_serve_directory_handle()?;
        service_fs.for_each_concurrent(None, |stream| async {
            let _ = match stream {
                Service::MemoryMonitor(stream) => {
                    serve_client_stream(
                        stream,
                        &bucket_definitions,
                        &*attribution_data_provider.clone(),
                        &*fast_attribution_data_provider.clone(),
                        kernel_stats.clone(),
                        stall_provider.clone(),
                        page_refault_tracker.clone(),
                    )
                    .inspect_err(|error| warn!(error:%; ""))
                    .await
                }
                Service::PageRefaultSink(stream) => {
                    page_refault_tracker
                        .listen_to_page_refaults(stream)
                        .inspect_err(|e| warn!("PageRefaultSink disconnected: {:?}", e))
                        .await
                }
            };
        })
    };
    let servicefs_health = task_health_node.create_string("servicefs_health", "ok");
    fuchsia_inspect::component::health().set_ok();
    loop {
        select! {
            _ = services => {
                servicefs_health.set("stopped");
                error!("Stopped serving requests");
            },
            _ = kernel_trace_service => {
                kernel_trace_health.set("stopped");
                error!("Stopped providing traces");
            },
            _ = inspect_nodes_service => error!("No longer serving inspect!"),
            result = periodic_collection => {
                periodic_collection_health.set(&result.err().map_or_else(||"stopped".to_string(), |err| format!("{:?}", err)));
                error!("Stopped periodic collection");
            },
            result = collect_stalls_task => {
                collect_stalls_health.set(&(result.err().map_or_else(||"stopped".to_string(), |err| format!("{:?}", err))));
                error!("Stopped collecting stalls");
            },
            _ = thrashing_loop => {
                 thrashing_health.set("stopped");
                 error!("Stopped thrashing loop");
            },
            complete => break,
        };
        fuchsia_inspect::component::health().set_unhealthy("One or more tasks unhealthy");
    }
    error!("Stopping memory_monitor 2");
    Ok(())
}

async fn serve_client_stream(
    mut stream: fattribution_plugin::MemoryMonitorRequestStream,
    bucket_definitions: &[BucketDefinition],
    attribution_data_provider: &impl AttributionDataProvider,
    fast_attribution_data_provider: &impl AttributionDataProvider,
    kernel_stats_proxy: fkernel::StatsProxy,
    stall_provider: impl StallProvider,
    refault_tracker: impl RefaultProvider,
) -> Result<()> {
    while let Some(request) = stream.next().await.transpose()? {
        match request {
            fattribution_plugin::MemoryMonitorRequest::GetSnapshot { snapshot, control_handle } => {
                if let Err(err) = provide_snapshot(
                    attribution_data_provider,
                    kernel_stats_proxy.clone(),
                    stall_provider.clone(),
                    refault_tracker.clone(),
                    bucket_definitions,
                    snapshot,
                )
                .await
                {
                    // Errors from `serve_snapshot` are all internal errors, not client-induced.
                    error!(err:%; "");
                    control_handle.shutdown_with_epitaph(zx::Status::INTERNAL);
                }
            }
            fattribution_plugin::MemoryMonitorRequest::GetSystemStatistics { responder } => {
                if let Err(err) = provide_statistics(
                    kernel_stats_proxy.clone(),
                    stall_provider.clone(),
                    refault_tracker.clone(),
                    responder,
                )
                .await
                {
                    error!(err:%; "");
                }
            }
            fattribution_plugin::MemoryMonitorRequest::GetAbridgedSnapshot {
                snapshot,
                control_handle,
            } => {
                if let Err(err) = provide_snapshot(
                    fast_attribution_data_provider,
                    kernel_stats_proxy.clone(),
                    stall_provider.clone(),
                    refault_tracker.clone(),
                    bucket_definitions,
                    snapshot,
                )
                .await
                {
                    // Errors from `serve_snapshot` are all internal errors, not client-induced.
                    error!(err:%; "");
                    control_handle.shutdown_with_epitaph(zx::Status::INTERNAL);
                }
            }
            fattribution_plugin::MemoryMonitorRequest::_UnknownMethod { .. } => {
                stream.control_handle().shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
            }
        }
    }
    Ok(())
}

/// Constructs a [Snapshot] and sends it, serialized, through the `snapshot` socket.
async fn provide_snapshot(
    attribution_data_provider: &impl AttributionDataProvider,
    kernel_stats_proxy: fkernel::StatsProxy,
    stall_provider: impl StallProvider,
    refault_tracker: impl RefaultProvider,
    bucket_definitions: &[BucketDefinition],
    snapshot: zx::Socket,
) -> Result<()> {
    duration!(CATEGORY_MEMORY_CAPTURE, c"provide_snapshot");
    let attribution_data = attribution_data_provider.get_attribution_data()?;

    let kernel_stats = fattribution_plugin::KernelStatistics {
        memory_stats: Some(kernel_stats_proxy.get_memory_stats().await?),
        compression_stats: Some(kernel_stats_proxy.get_memory_stats_compression().await?),
        ..Default::default()
    };

    let memory_stalls = stall_provider.get_stall_info()?;
    let attribution_snapshot = AttributionSnapshot::new(
        attribution_data,
        kernel_stats,
        memory_stalls,
        refault_tracker,
        bucket_definitions,
    );
    attribution_snapshot.serve(snapshot).await
}

/// Looks for a bucket definitions configuration, to perform memory
/// aggregations for reporting purposes. Returns an empty list if no
/// such configuration was found.
fn read_bucket_definitions() -> Vec<BucketDefinition> {
    std::fs::File::open("/config/data/buckets.json")
        .inspect_err(|err| warn!(err:%; "Could not access the bucket definitions configuration"))
        .ok()
        .and_then(|file| {
            serde_json::from_reader(file)
                .inspect_err(
                    |err| warn!(err:%; "Could not read the bucket definitions configuration"),
                )
                .ok()
        })
        .unwrap_or_default()
}

async fn provide_statistics(
    kernel_stats_proxy: fkernel::StatsProxy,
    stall_provider: impl StallProvider,
    refault_tracker: impl RefaultProvider,
    responder: fattribution_plugin::MemoryMonitorGetSystemStatisticsResponder,
) -> Result<()> {
    let kernel_stats = fattribution_plugin::KernelStatistics {
        memory_stats: Some(kernel_stats_proxy.get_memory_stats().await?),
        compression_stats: Some(kernel_stats_proxy.get_memory_stats_compression().await?),
        ..Default::default()
    };

    let memory_stalls = stall_provider.get_stall_info()?;
    let refaults = refault_tracker.get_count();

    responder.send(&fattribution_plugin::MemoryStatistics {
        time: Some(fattribution_plugin::Time {
            boot_time: Some(BootInstant::get()),
            monotonic_time: Some(MonotonicInstant::get()),
            ..Default::default()
        }),
        kernel_statistics: Some(kernel_stats),
        performance_metrics: Some(fattribution_plugin::PerformanceImpactMetrics {
            some_memory_stalls_ns: Some(memory_stalls.some.as_nanos().try_into()?),
            full_memory_stalls_ns: Some(memory_stalls.full.as_nanos().try_into()?),
            page_refaults: Some(refaults),
            ..Default::default()
        }),
        ..Default::default()
    })?;
    Ok(())
}
