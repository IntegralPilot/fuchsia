// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::accessor::{ArchiveAccessorServer, BatchRetrievalTimeout};
use crate::component_lifecycle;
use crate::error::Error;
use crate::events::router::{ConsumerConfig, EventRouter, ProducerConfig};
use crate::events::sources::EventSource;
use crate::events::types::*;
use crate::identity::ComponentIdentity;
use crate::inspect::container::InspectHandle;
use crate::inspect::repository::InspectRepository;
use crate::inspect::servers::*;
use crate::logs::debuglog::KernelDebugLog;
use crate::logs::repository::{ComponentInitialInterest, LogsRepository};
use crate::logs::serial::{SerialSink, launch_serial};
use crate::logs::servers::*;
use crate::pipeline::PipelineManager;
use archivist_config::Config;
use fidl_fuchsia_process_lifecycle::LifecycleRequestStream;
use fuchsia_component::server::{ServiceFs, ServiceObj};
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc::unbounded;
use futures::prelude::*;
use log::{debug, error, info, warn};
use moniker::ExtendedMoniker;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use {fidl_fuchsia_component_sandbox as fsandbox, fidl_fuchsia_io as fio, fuchsia_async as fasync};

/// Responsible for initializing an `Archivist` instance. Supports multiple configurations by
/// either calling or not calling methods on the builder like `serve_test_controller_protocol`.
pub struct Archivist {
    /// Handles event routing between archivist parts.
    event_router: EventRouter,

    /// The diagnostics pipelines that have been installed.
    pipeline_manager: PipelineManager,

    /// The repository holding Inspect data.
    _inspect_repository: Arc<InspectRepository>,

    /// The repository holding active log connections.
    logs_repository: Arc<LogsRepository>,

    /// The server handling fuchsia.diagnostics.ArchiveAccessor
    accessor_server: Arc<ArchiveAccessorServer>,

    /// The server handling fuchsia.logger.Log
    log_server: Arc<LogServer>,

    /// The server handling fuchsia.diagnostics.system.LogFlush
    flush_server: Arc<LogFlushServer>,

    /// The server handling fuchsia.diagnostics.LogStream
    log_stream_server: Arc<LogStreamServer>,

    /// The server handling fuchsia.inspect.InspectSink
    _inspect_sink_server: Arc<InspectSinkServer>,

    /// Top level scope.
    general_scope: fasync::Scope,

    /// Tasks receiving external events from component manager.
    incoming_events_scope: fasync::Scope,

    /// All tasks for FIDL servers that ingest data into the Archivist must run in this scope.
    servers_scope: fasync::Scope,

    /// Freeze server.
    freeze_server: LogFreezeServer,

    /// The server handling fuchsia.diagnostics.Sample
    sample_server: Arc<SampleServer>,
}

impl Archivist {
    /// Creates new instance, sets up inspect and adds 'archive' directory to output folder.
    /// Also installs `fuchsia.diagnostics.Archive` service.
    /// Call `install_log_services`
    pub async fn new(ring_buffer: ring_buffer::Reader, config: Config) -> Self {
        let general_scope = fasync::Scope::new_with_name("general");
        let servers_scope = fasync::Scope::new_with_name("servers");

        // Initialize the pipelines that the archivist will expose.
        let pipeline_manager = PipelineManager::new(
            PathBuf::from(&config.pipelines_path),
            component::inspector().root().create_child("pipelines"),
            component::inspector().root().create_child("archive_accessor_stats"),
            general_scope.new_child_with_name("pipeline_manager"),
        )
        .await;

        // Initialize the core event router
        let mut event_router =
            EventRouter::new(component::inspector().root().create_child("events"));
        let incoming_events_scope = general_scope.new_child_with_name("incoming_events");
        Self::initialize_external_event_sources(&mut event_router, &incoming_events_scope).await;

        let initial_interests =
            config.component_initial_interests.into_iter().filter_map(|interest| {
                ComponentInitialInterest::from_str(&interest)
                    .map_err(|err| {
                        warn!(err:?, invalid:% = interest; "Failed to load initial interest");
                    })
                    .ok()
            });
        let logs_repo = LogsRepository::new(
            ring_buffer,
            initial_interests,
            component::inspector().root(),
            general_scope.new_child_with_name("logs_repository"),
        );
        let (freeze_server, freeze_receiver) = LogFreezeServer::new();
        let (flush_sender, flush_receiver) = unbounded();
        if !config.allow_serial_logs.is_empty() {
            let logs_repo_clone = Arc::clone(&logs_repo);
            general_scope.spawn(async move {
                launch_serial(
                    config.allow_serial_logs,
                    config.deny_serial_log_tags,
                    logs_repo_clone,
                    SerialSink,
                    freeze_receiver,
                    flush_receiver,
                )
                .await;
            });
        }
        let inspect_repo = Arc::new(InspectRepository::new(
            pipeline_manager.weak_pipelines(),
            general_scope.new_child_with_name("inspect_repository"),
        ));

        let inspect_sink_server = Arc::new(InspectSinkServer::new(
            Arc::clone(&inspect_repo),
            servers_scope.new_child_with_name("InspectSink"),
        ));

        let sample_server = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            zx::MonotonicDuration::from_seconds(
                config.fuchsia_diagnostics_sample_min_period_seconds,
            ),
            servers_scope.new_child_with_name("SampleServer"),
        ));

        // Initialize our FIDL servers. This doesn't start serving yet.
        let accessor_server = Arc::new(ArchiveAccessorServer::new(
            Arc::clone(&inspect_repo),
            Arc::clone(&logs_repo),
            config.maximum_concurrent_snapshots_per_reader,
            BatchRetrievalTimeout::from_seconds(config.per_component_batch_timeout_seconds),
            servers_scope.new_child_with_name("ArchiveAccessor"),
        ));

        let flush_server = Arc::new(LogFlushServer::new(
            servers_scope.new_child_with_name("LogFlush"),
            flush_sender,
        ));

        let log_server = Arc::new(LogServer::new(
            Arc::clone(&logs_repo),
            servers_scope.new_child_with_name("Log"),
        ));
        let log_stream_server = Arc::new(LogStreamServer::new(
            Arc::clone(&logs_repo),
            servers_scope.new_child_with_name("LogStream"),
        ));

        // Initialize the external event providers containing incoming diagnostics directories and
        // log sink connections.
        event_router.add_consumer(ConsumerConfig {
            consumer: &logs_repo,
            events: vec![EventType::LogSinkRequested],
        });
        event_router.add_consumer(ConsumerConfig {
            consumer: &inspect_sink_server,
            events: vec![EventType::InspectSinkRequested],
        });

        // Drain klog and publish it to syslog.
        if config.enable_klog {
            match KernelDebugLog::new().await {
                Ok(klog) => logs_repo.drain_debuglog(klog),
                Err(err) => warn!(
                    err:?;
                    "Failed to start the kernel debug log reader. Klog won't be in syslog"
                ),
            };
        }

        // Start related services that should start once the Archivist has started.
        for name in &config.bind_services {
            info!("Connecting to service {}", name);
            let (_local, remote) = zx::Channel::create();
            if let Err(e) = fdio::service_connect(&format!("/svc/{name}"), remote) {
                error!("Couldn't connect to service {}: {:?}", name, e);
            }
        }

        // TODO(https://fxbug.dev/324494668): remove this when Netstack2 is gone.
        if let Ok(dir) =
            fuchsia_fs::directory::open_in_namespace("/netstack-diagnostics", fio::PERM_READABLE)
        {
            inspect_repo.add_inspect_handle(
                Arc::new(ComponentIdentity::new(
                    ExtendedMoniker::parse_str("core/network/netstack").unwrap(),
                    "fuchsia-pkg://fuchsia.com/netstack#meta/netstack2.cm",
                )),
                InspectHandle::directory(dir),
            );
        }

        Self {
            accessor_server,
            log_server,
            flush_server,
            log_stream_server,
            event_router,
            _inspect_sink_server: inspect_sink_server,
            pipeline_manager,
            _inspect_repository: inspect_repo,
            logs_repository: logs_repo,
            general_scope,
            servers_scope,
            freeze_server,
            incoming_events_scope,
            sample_server,
        }
    }

    pub async fn initialize_external_event_sources(
        event_router: &mut EventRouter,
        scope: &fasync::Scope,
    ) {
        match EventSource::new("/events/log_sink_requested_event_stream").await {
            Err(err) => warn!(err:?; "Failed to create event source for log sink requests"),
            Ok(mut event_source) => {
                event_router.add_producer(ProducerConfig {
                    producer: &mut event_source,
                    events: vec![EventType::LogSinkRequested],
                });
                scope.spawn(async move {
                    // This should never exit.
                    let _ = event_source.spawn().await;
                });
            }
        }

        match EventSource::new("/events/inspect_sink_requested_event_stream").await {
            Err(err) => {
                warn!(err:?; "Failed to create event source for InspectSink requests");
            }
            Ok(mut event_source) => {
                event_router.add_producer(ProducerConfig {
                    producer: &mut event_source,
                    events: vec![EventType::InspectSinkRequested],
                });
                scope.spawn(async move {
                    // This should never exit.
                    let _ = event_source.spawn().await;
                });
            }
        }
    }

    /// Run archivist to completion.
    /// # Arguments:
    /// * `outgoing_channel`- channel to serve outgoing directory on.
    pub async fn run(
        mut self,
        mut fs: ServiceFs<ServiceObj<'static, ()>>,
        is_embedded: bool,
        store: fsandbox::CapabilityStoreProxy,
        request_stream: LifecycleRequestStream,
    ) -> Result<(), Error> {
        debug!("Running Archivist.");

        // Start servicing all outgoing services.
        self.serve_protocols(&mut fs, store).await;
        let svc_task = self.general_scope.spawn(fs.collect::<()>());

        let _inspect_server_task = inspect_runtime::publish(
            component::inspector(),
            inspect_runtime::PublishOptions::default(),
        );

        let Self {
            _inspect_repository,
            mut pipeline_manager,
            logs_repository,
            accessor_server: _accessor_server,
            log_server: _log_server,
            log_stream_server: _log_stream_server,
            _inspect_sink_server,
            general_scope,
            incoming_events_scope,
            freeze_server: _freeze_server,
            servers_scope,
            event_router,
            flush_server: _flush_server,
            sample_server: _sample_server,
        } = self;

        // Start ingesting events.
        let (terminate_handle, drain_events_fut) = event_router
            .start()
            // panic: can only panic if we didn't register event producers and consumers correctly.
            .expect("Failed to start event router");
        general_scope.spawn(drain_events_fut);

        let servers_scope_handle = servers_scope.to_handle();
        general_scope.spawn(component_lifecycle::on_stop_request(request_stream, || async move {
            terminate_handle.terminate().await;
            debug!("Stopped ingesting new CapabilityRequested events");
            incoming_events_scope.cancel().await;
            debug!("Cancel all tasks currently executing in our event router");
            servers_scope_handle.close();
            logs_repository.stop_accepting_new_log_sinks();
            debug!("Close any new connections to FIDL servers");
            svc_task.abort().await;
            pipeline_manager.cancel().await;
            debug!("Stop allowing new connections through the incoming namespace.");
            logs_repository.wait_for_termination().await;
            debug!("All LogSink connections have finished");
            servers_scope.join().await;
            debug!("All servers stopped.");
        }));
        if is_embedded {
            debug!("Entering core loop.");
        } else {
            info!("archivist: Entering core loop.");
        }

        component::health().set_ok();
        general_scope.await;

        Ok(())
    }

    async fn serve_protocols(
        &mut self,
        fs: &mut ServiceFs<ServiceObj<'static, ()>>,
        mut store: fsandbox::CapabilityStoreProxy,
    ) {
        component::serve_inspect_stats();
        let mut svc_dir = fs.dir("svc");

        let id_gen = sandbox::CapabilityIdGenerator::new();

        // Serve fuchsia.diagnostics.ArchiveAccessors backed by a pipeline.
        let accessors_dict_id = self
            .pipeline_manager
            .serve_pipelines(Arc::clone(&self.accessor_server), &id_gen, &mut store)
            .await;

        // Serve fuchsia.logger.Log
        let log_server = Arc::clone(&self.log_server);
        svc_dir.add_fidl_service(move |stream| {
            debug!("fuchsia.logger.Log connection");
            log_server.spawn(stream);
        });

        // Serve fuchsia.diagnostics.system.LogFlush
        let flush_server = Arc::clone(&self.flush_server);
        svc_dir.add_fidl_service(move |stream| {
            debug!("fuchsia.diagnostics.system.LogFlush connection");
            flush_server.spawn(stream);
        });

        // Install sample server to the accessors dictionary and start serving.
        // Note that the await is not awaiting on the request stream or anything like that;
        // the server spawns onto its owned Scope.
        self.sample_server.install_and_serve(*accessors_dict_id, &id_gen, &mut store).await;

        let server = self.freeze_server.clone();
        let scope = self.general_scope.clone();

        svc_dir.add_fidl_service(move |stream| {
            debug!("fuchsia.diagnostics.system.SerialLogControl connection");
            let server = server.clone();
            scope.spawn(async move {
                let _ = server.handle_requests(stream).await;
            });
        });

        // Server fuchsia.logger.LogStream
        let log_stream_server = Arc::clone(&self.log_stream_server);
        svc_dir.add_fidl_service(move |stream| {
            debug!("fuchsia.logger.LogStream connection");
            log_stream_server.spawn(stream);
        });

        // Server fuchsia.diagnostics.LogSettings
        let log_settings_server = LogSettingsServer::new(
            Arc::clone(&self.logs_repository),
            // Don't create this in the servers scope. We don't care about this protocol for
            // shutdown purposes.
            self.general_scope.new_child_with_name("LogSettings"),
        );
        svc_dir.add_fidl_service(move |stream| {
            debug!("fuchsia.diagnostics.LogSettings connection");
            log_settings_server.spawn(stream);
        });

        // Serve fuchsia.component.sandbox.Router
        let router_scope =
            self.servers_scope.new_child_with_name("fuchsia.component.sandbox.Router");
        svc_dir.add_fidl_service(move |mut stream: fsandbox::DictionaryRouterRequestStream| {
            let id_gen = Clone::clone(&id_gen);
            let store = Clone::clone(&store);
            router_scope.spawn(async move {
                while let Ok(Some(request)) = stream.try_next().await {
                    match request {
                        fsandbox::DictionaryRouterRequest::Route { payload: _, responder } => {
                            debug!("Got route request for the dynamic accessors dictionary");
                            let dup_dict_id = id_gen.next();
                            store
                                .duplicate(*accessors_dict_id, dup_dict_id)
                                .await
                                .unwrap()
                                .unwrap();
                            let capability = store.export(dup_dict_id).await.unwrap().unwrap();
                            let fsandbox::Capability::Dictionary(dict) = capability else {
                                let _ = responder.send(Err(fsandbox::RouterError::Internal));
                                continue;
                            };
                            let _ = responder.send(Ok(
                                fsandbox::DictionaryRouterRouteResponse::Dictionary(dict),
                            ));
                        }
                        fsandbox::DictionaryRouterRequest::_UnknownMethod {
                            method_type,
                            ordinal,
                            ..
                        } => {
                            warn!(method_type:?, ordinal; "Got unknown interaction on Router");
                        }
                    }
                }
            });
        });
    }
}
