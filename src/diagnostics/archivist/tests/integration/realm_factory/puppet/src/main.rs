// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This program serves the fuchsia.archivist.test.Puppet protocol.
//
// It is meant to be controlled by a test suite and will emit log messages
// and inspect data as requested. This output can be retrieved from the
// archivist under test using fuchsia.diagnostics.ArchiveAccessor.
//
// For full documentation, see //src/diagnostics/archivist/testing/realm-factory/README.md

use anyhow::{Context, Error, Result};
use diagnostics_hierarchy::Property;
use diagnostics_log::{OnInterestChanged, Publisher, PublisherOptions, TestRecord};
use diagnostics_log_encoding::Argument;
use fidl::endpoints::{ClientEnd, create_request_stream};
use fidl_table_validation::ValidFidlTable;
use fuchsia_async::{Scope, Timer};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::health::Reporter;
use fuchsia_inspect::{Inspector, component};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::lock::Mutex;
use futures::{FutureExt, StreamExt, TryStreamExt};
use inspect_runtime::{EscrowOptions, EscrowToken, PublishedInspectController};
use inspect_testing::ExampleInspectData;
use log::{debug, error, info, trace, warn};
use std::sync::Arc;
use {fidl_fuchsia_archivist_test as fpuppet, fidl_fuchsia_diagnostics_types as fdiagnostics};

enum IncomingServices {
    Puppet(fpuppet::PuppetRequestStream),
    InspectPuppet(fpuppet::InspectPuppetRequestStream),
}

// `logging = false` allows us to set the global default trace dispatcher
// ourselves. This can only be done once and is usually handled by fuchsia::main.
#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    // Listen for log interest change events.
    let (interest_send, interest_recv) = unbounded::<InterestChangedEvent>();
    let logger = subscribe_to_log_interest_changes(InterestChangedNotifier(interest_send)).await?;

    let mut fs = ServiceFs::new();

    let puppet_server = Arc::new(PuppetServer::new(interest_recv));

    fs.dir("svc")
        .add_fidl_service(IncomingServices::Puppet)
        .add_fidl_service(IncomingServices::InspectPuppet);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, |service| async {
        match service {
            IncomingServices::Puppet(s) => {
                serve_puppet(puppet_server.clone(), s, &logger).await;
            }
            IncomingServices::InspectPuppet(s) => {
                serve_inspect_puppet(puppet_server.clone(), s).await;
            }
        }
    })
    .await;

    Ok(())
}

async fn subscribe_to_log_interest_changes(
    notifier: InterestChangedNotifier,
) -> Result<Publisher, Error> {
    let publisher = Publisher::new_async(
        PublisherOptions::default().register_global_logger(true).listen_for_interest_updates(true),
    )
    .await?;
    publisher.set_interest_listener(notifier);

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error!(info:%; "PANIC");
        previous_hook(info);
    }));
    Ok(publisher)
}

#[derive(Clone)]
struct InterestChangedEvent {
    severity: fdiagnostics::Severity,
}

struct PuppetServer {
    // A stream of noifications about interest changed events.
    interest_changed: Mutex<UnboundedReceiver<InterestChangedEvent>>,
    // Scope for waiters on interest changed events.
    waiter_scope: Scope,
    // Scope for published Inspectors
    inspectors_scope: Scope,
}

impl PuppetServer {
    fn new(receiver: UnboundedReceiver<InterestChangedEvent>) -> Self {
        Self {
            interest_changed: Mutex::new(receiver),
            waiter_scope: Scope::new_with_name("waiters-for-interest-changed-events"),
            inspectors_scope: Scope::new_with_name("published-inspectors"),
        }
    }
}

// Notifies the puppet when log interest changes.
//
// Together, `PuppetServer` and `InterestChangeNotifier` must guarantee delivery
// of all interest change notifications to clients (test cases) regardless of
// whether a test case begins waiting for the interest change notification
// before or after it is received by this component. Failure to deliver will
// cause the test case to hang.
#[derive(Clone)]
struct InterestChangedNotifier(UnboundedSender<InterestChangedEvent>);

impl OnInterestChanged for InterestChangedNotifier {
    fn on_changed(&self, severity: diagnostics_log::Severity) {
        let sender = self.0.clone();
        // Panic on failure since undelivered notifications may hang clients.
        sender.unbounded_send(InterestChangedEvent { severity: severity.into() }).unwrap();
    }
}

async fn serve_puppet(
    server: Arc<PuppetServer>,
    mut stream: fpuppet::PuppetRequestStream,
    logger: &Publisher,
) {
    while let Ok(Some(request)) = stream.try_next().await {
        handle_puppet_request(server.clone(), request, logger)
            .await
            .unwrap_or_else(|e| error!(e:?; "handle_puppet_request"));
    }
}

async fn serve_inspect_puppet(
    server: Arc<PuppetServer>,
    mut stream: fpuppet::InspectPuppetRequestStream,
) {
    while let Ok(Some(request)) = stream.try_next().await {
        handle_inspect_puppet_request(server.clone(), request)
            .await
            .unwrap_or_else(|e| error!(e:?; "handle_puppet_request"));
    }
}

async fn handle_inspect_writer(
    inspector: &Inspector,
    inspect_controller: PublishedInspectController,
    mut stream: fpuppet::InspectWriterRequestStream,
) -> Result<(), Error> {
    let mut example_inspect = ExampleInspectData::default();

    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fpuppet::InspectWriterRequest::EmitExampleInspectData { responder } => {
                example_inspect.write_to(inspector.root());
                responder.send().expect("response succeeds")
            }
            fpuppet::InspectWriterRequest::RecordString { key, value, responder } => {
                inspector.root().record_string(key, value);
                responder.send().expect("response succeeds")
            }
            fpuppet::InspectWriterRequest::RecordInt { key, value, responder } => {
                inspector.root().record_int(key, value);
                responder.send().expect("response succeeds")
            }
            fpuppet::InspectWriterRequest::SetHealthOk { responder } => {
                component::health().set_ok();
                responder.send().expect("response succeeds")
            }
            #[cfg(fuchsia_api_level_at_least = "29")]
            fpuppet::InspectWriterRequest::SetHealthStartingUp { responder } => {
                component::health().set_starting_up();
                responder.send().expect("response succeeds")
            }
            fpuppet::InspectWriterRequest::EscrowAndExit {
                payload: fpuppet::InspectWriterEscrowAndExitRequest { name, .. },
                responder,
            } => {
                let mut options = EscrowOptions::default();
                if let Some(name) = name {
                    options = options.name(name);
                }
                let token = inspect_controller.escrow_frozen(options).await.unwrap();
                responder
                    .send(fpuppet::InspectWriterEscrowAndExitResponse {
                        token: Some(token),
                        ..Default::default()
                    })
                    .expect("response succeeds");
                std::process::exit(0);
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fpuppet::InspectWriterRequest::RecordLazyValues { key, responder } => {
                let (client, requests) = create_request_stream();
                responder.send(client).expect("response succeeds");
                record_lazy_values(key, requests, inspector).await?;
            }
            fpuppet::InspectWriterRequest::_UnknownMethod { .. } => unreachable!(),
        }
    }
    Ok(())
}

async fn handle_create_inspector(
    server: Arc<PuppetServer>,
    name: Option<String>,
) -> ClientEnd<fpuppet::InspectWriterMarker> {
    let (inspector, controller) = match name {
        Some(ref name) => {
            let inspector = Inspector::default();
            let controller = inspect_runtime::publish(
                &inspector,
                inspect_runtime::PublishOptions::default().inspect_tree_name(name),
            )
            .unwrap();
            (inspector, controller)
        }
        None => {
            let inspector = component::inspector().clone();
            let controller =
                inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default())
                    .unwrap();
            (inspector, controller)
        }
    };

    let (client_end, server_end) = fidl::endpoints::create_endpoints();
    server.inspectors_scope.spawn(async move {
        handle_inspect_writer(&inspector, controller, server_end.into_stream()).await.unwrap()
    });
    client_end
}

async fn handle_create_inspector_from_escrow(
    server: Arc<PuppetServer>,
    token: Option<EscrowToken>,
) -> (ClientEnd<fpuppet::InspectWriterMarker>, zx::Vmo) {
    let escrow_token = token.expect("Escrow token is required");
    let inspect_runtime::FetchEscrowResult { vmo, server: tree } = inspect_runtime::fetch_escrow(
        escrow_token,
        inspect_runtime::FetchEscrowOptions::new().replace_with_tree(),
    )
    .await
    .unwrap();

    let controller = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default().on_tree_server(tree.unwrap()),
    )
    .unwrap();

    let (client_end, server_end) = fidl::endpoints::create_endpoints();
    server.inspectors_scope.spawn(async move {
        handle_inspect_writer(component::inspector(), controller, server_end.into_stream())
            .await
            .unwrap()
    });

    (client_end, vmo)
}

async fn handle_inspect_puppet_request(
    server: Arc<PuppetServer>,
    request: fpuppet::InspectPuppetRequest,
) -> Result<(), Error> {
    match request {
        fpuppet::InspectPuppetRequest::CreateInspector {
            payload: fpuppet::InspectPuppetCreateInspectorRequest { name, .. },
            responder,
        } => {
            let inspect_writer = handle_create_inspector(server, name).await;
            responder.send(inspect_writer).context("response succeeds")
        }
        fpuppet::InspectPuppetRequest::CreateInspectorFromEscrow {
            payload: fpuppet::InspectPuppetCreateInspectorFromEscrowRequest { token, .. },
            responder,
        } => {
            let (inspect_writer, vmo) = handle_create_inspector_from_escrow(server, token).await;
            responder.send(inspect_writer, vmo).context("response succeeds")
        }
        fpuppet::InspectPuppetRequest::_UnknownMethod { .. } => unreachable!(),
    }
}

async fn handle_puppet_request(
    server: Arc<PuppetServer>,
    request: fpuppet::PuppetRequest,
    logger: &Publisher,
) -> Result<(), Error> {
    match request {
        fpuppet::PuppetRequest::CreateInspector {
            payload: fpuppet::InspectPuppetCreateInspectorRequest { name, .. },
            responder,
        } => {
            let inspect_writer = handle_create_inspector(server, name).await;
            responder.send(inspect_writer).context("response succeeds")
        }
        fpuppet::PuppetRequest::CreateInspectorFromEscrow {
            payload: fpuppet::InspectPuppetCreateInspectorFromEscrowRequest { token, .. },
            responder,
        } => {
            let (inspect_writer, vmo) = handle_create_inspector_from_escrow(server, token).await;
            responder.send(inspect_writer, vmo).context("response succeeds")
        }
        fpuppet::PuppetRequest::Crash { message, .. } => {
            panic!("{message}");
        }
        fpuppet::PuppetRequest::RecordLazyValues { key, responder } => {
            let (client, requests) = create_request_stream();
            responder.send(client).expect("response succeeds");
            record_lazy_values(key, requests, component::inspector()).await
        }
        fpuppet::PuppetRequest::Println { message, responder } => {
            println!("{message}");
            responder.send().context("response succeeds")
        }
        fpuppet::PuppetRequest::Eprintln { message, responder } => {
            eprintln!("{message}");
            responder.send().context("response succeeds")
        }
        fpuppet::PuppetRequest::Log { payload, responder, .. } => {
            let request = LogRequest::try_from(payload).context("Invalid log")?;
            let LogRequest { message, severity, time, .. } = request;

            match time {
                None => match severity {
                    fdiagnostics::Severity::Trace => trace!("{message}"),
                    fdiagnostics::Severity::Debug => debug!("{message}"),
                    fdiagnostics::Severity::Info => info!("{message}"),
                    fdiagnostics::Severity::Warn => warn!("{message}"),
                    fdiagnostics::Severity::Error => error!("{message}"),
                    _ => unimplemented!("Logging with severity: {severity:?}"),
                },
                Some(time) => {
                    let record = TestRecord {
                        severity: severity.into_primitive(),
                        timestamp: zx::BootInstant::from_nanos(time),
                        file: None,
                        line: None,
                        record_arguments: vec![Argument::message(message.as_str())],
                    };
                    logger.event_for_testing(record);
                }
            }
            responder.send().context("response succeeds")
        }
        fpuppet::PuppetRequest::WaitForInterestChange { responder } => {
            let s = server.clone();
            server.waiter_scope.spawn(async move {
                let event = s.interest_changed.lock().await.next().await.unwrap();
                let response = &fpuppet::LogPuppetWaitForInterestChangeResponse {
                    severity: Some(event.severity),
                    ..Default::default()
                };
                responder.send(response).expect("response succeeds");
            });
            Ok(())
        }
        fpuppet::PuppetRequest::_UnknownMethod { .. } => unreachable!(),
    }
}

#[derive(Debug, Clone, ValidFidlTable)]
#[fidl_table_src(fpuppet::LogPuppetLogRequest)]
#[fidl_table_strict]
pub struct LogRequest {
    pub message: String,
    pub severity: fdiagnostics::Severity,
    #[fidl_field_type(optional)]
    pub time: Option<i64>,
}

// Converts InspectPuppet requests into callbacks that report inspect values lazily.
// The values aren't truly lazy since they're computed in the client before the inspect
// data is fetched. They're just lazily reported.
async fn record_lazy_values(
    key: String,
    mut stream: fpuppet::LazyInspectPuppetRequestStream,
    inspector: &Inspector,
) -> Result<(), Error> {
    let mut properties = vec![];
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fpuppet::LazyInspectPuppetRequest::RecordString { key, value, responder } => {
                properties.push(Property::String(key, value));
                responder.send().expect("response succeeds")
            }
            fpuppet::LazyInspectPuppetRequest::RecordInt { key, value, responder } => {
                properties.push(Property::Int(key, value));
                responder.send().expect("response succeeds")
            }
            fpuppet::LazyInspectPuppetRequest::Commit { options, responder } => {
                inspector.root().record_lazy_values(key, move || {
                    let properties = properties.clone();
                    async move {
                        if options.hang.unwrap_or_default() {
                            Timer::new(zx::MonotonicDuration::from_minutes(60)).await;
                        }
                        let inspector = Inspector::default();
                        let node = inspector.root();
                        for property in properties.iter() {
                            match property {
                                Property::String(k, v) => node.record_string(k, v),
                                Property::Int(k, v) => node.record_int(k, *v),
                                _ => unimplemented!(),
                            }
                        }
                        Ok(inspector)
                    }
                    .boxed()
                });
                responder.send().expect("response succeeds");
                return Ok(()); // drop the connection.
            }
            fpuppet::LazyInspectPuppetRequest::_UnknownMethod { .. } => unreachable!(),
            fpuppet::LazyInspectPuppetRequest::EmitExampleInspectData { .. }
            | fpuppet::LazyInspectPuppetRequest::SetHealthOk { .. }
            | fpuppet::LazyInspectPuppetRequest::EscrowAndExit { .. } => {
                unimplemented!()
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fpuppet::LazyInspectPuppetRequest::RecordLazyValues { .. } => unimplemented!(),
            #[cfg(fuchsia_api_level_at_least = "29")]
            fpuppet::LazyInspectPuppetRequest::SetHealthStartingUp { .. } => unimplemented!(),
        };
    }

    Ok(())
}
