// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_power_cpu_manager as fcpumanager;
use fidl_fuchsia_testing as ftesting;
use fidl_fuchsia_testing_harness::RealmProxy_Marker;
use fidl_test_systemactivitygovernor::*;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, DEFAULT_COLLECTION_NAME, RealmBuilder, RealmInstance, Ref, Route,
};
use fuchsia_inspect::Node as INode;
use futures::{FutureExt, StreamExt, TryStreamExt};
use log::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
const ACTIVITY_GOVERNOR_CHILD_NAME: &str = "system-activity-governor";
const FAKE_BOOST_CHILD_NAME: &str = "fake-boost";

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: RealmFactoryRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_realm_factory).await;
    Ok(())
}

async fn serve_realm_factory(stream: RealmFactoryRequestStream) {
    if let Err(err) = handle_request_stream(stream).await {
        error!("{:?}", err);
    }
}

async fn handle_request_stream(mut stream: RealmFactoryRequestStream) -> Result<()> {
    let scope = fasync::Scope::new();
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            RealmFactoryRequest::CreateRealm { realm_server, responder } => {
                let realm = create_realm(RealmOptions::default()).await?;
                responder.send(Ok(&realm.moniker()))?;
                scope.spawn(realm.serve(realm_server));
            }
            RealmFactoryRequest::CreateRealmExt { options, realm_server, responder } => {
                let realm = create_realm(options).await?;
                responder.send(Ok(&realm.moniker()))?;
                scope.spawn(realm.serve(realm_server));
            }
            RealmFactoryRequest::_UnknownMethod { .. } => unreachable!(),
        }
    }

    scope.join().await;
    Ok(())
}

struct SagRealm {
    realm: RealmInstance,
}

impl SagRealm {
    fn moniker(&self) -> String {
        format!(
            "{}:{}/{}",
            DEFAULT_COLLECTION_NAME,
            self.realm.root.child_name(),
            ACTIVITY_GOVERNOR_CHILD_NAME
        )
    }

    async fn serve(self, server_end: ServerEnd<RealmProxy_Marker>) {
        realm_proxy::service::serve(self.realm, server_end.into_stream()).await.unwrap()
    }
}

async fn create_realm(options: RealmOptions) -> Result<SagRealm, Error> {
    info!("building the realm");

    let use_fake_sag = options.use_fake_sag.unwrap_or(false);
    let use_fake_clock = options.use_fake_clock.unwrap_or(false);
    let wait_for_suspending_token = options.wait_for_suspending_token.unwrap_or(false);
    let use_suspender = options.use_suspender.unwrap_or(true);
    let stuck_warning_timeout_seconds = options.stuck_warning_timeout_seconds.unwrap_or(60);
    let reboot_on_stalled_suspend_blocker =
        options.reboot_on_stalled_suspend_blocker.unwrap_or(false);
    let long_wake_lease_timeout_seconds = options.long_wake_lease_timeout_seconds.unwrap_or(60);

    let builder = RealmBuilder::new().await?;

    let component_url = if use_fake_sag {
        "fake-system-activity-governor#meta/fake-system-activity-governor.cm"
    } else if use_fake_clock {
        "#meta/system-activity-governor-fake-time.cm"
    } else {
        "#meta/system-activity-governor.cm"
    };

    let component_ref =
        builder.add_child(ACTIVITY_GOVERNOR_CHILD_NAME, component_url, ChildOptions::new()).await?;

    if use_fake_clock {
        let fake_clock =
            builder.add_child("fake_clock", "#meta/fake_clock.cm", ChildOptions::new()).await?;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<ftesting::FakeClockMarker>())
                    .from(&fake_clock)
                    .to(&component_ref),
            )
            .await?;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<ftesting::FakeClockControlMarker>())
                    .from(&fake_clock)
                    .to(Ref::parent()),
            )
            .await?;
    }

    let power_broker_ref =
        builder.add_child("power-broker", "#meta/power-broker.cm", ChildOptions::new()).await?;

    let fake_suspend_ref =
        builder.add_child("fake-suspend", "#meta/fake-suspend.cm", ChildOptions::new()).await?;

    let fake_shutdown_shim = builder
        .add_child("fake-shutdown-shim", "#meta/fake-shutdown-shim.cm", ChildOptions::new())
        .await?;

    let fake_crash_reporter = builder
        .add_child("fake-crash-reporter", "#meta/fake_crash_reporter.cm", ChildOptions::new())
        .await?;

    // Expose capabilities from power-broker.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.broker.Topology"))
                .from(&power_broker_ref)
                .to(Ref::parent()),
        )
        .await?;

    // Expose capabilities from fake-suspend.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("test.suspendcontrol.Device"))
                .from(&fake_suspend_ref)
                .to(Ref::parent()),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.feedback.testing.FakeCrashReporterQuerier",
                ))
                .from(&fake_crash_reporter)
                .to(Ref::parent()),
        )
        .await?;

    // Expose capabilities from fake-shutdown-shim.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.hardware.power.statecontrol.ShutdownWatcherRegister",
                ))
                .from(&fake_shutdown_shim)
                .to(Ref::parent()),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.hardware.power.statecontrol.Admin",
                ))
                .from(&fake_shutdown_shim)
                .to(Ref::parent()),
        )
        .await?;

    // Expose capabilities from fake-shutdown-shim to system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.hardware.power.statecontrol.ShutdownWatcherRegister",
                ))
                .from(&fake_shutdown_shim)
                .to(&component_ref),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.hardware.power.statecontrol.Admin",
                ))
                .from(&fake_shutdown_shim)
                .to(&component_ref),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.feedback.CrashReporter"))
                .from(&fake_crash_reporter)
                .to(&component_ref),
        )
        .await?;

    // Expose capabilities from power-broker to system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.broker.Topology"))
                .from(&power_broker_ref)
                .to(&component_ref),
        )
        .await?;

    // Expose capabilities from fake-suspend to system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::service_by_name(
                    "fuchsia.hardware.power.suspend.SuspendService",
                ))
                .from(&fake_suspend_ref)
                .to(&component_ref),
        )
        .await?;

    // Expose config capabilities to system-activity-governor.
    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.UseSuspender".parse()?,
            value: use_suspender.into(),
        }))
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.UseSuspender"))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await?;

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.SuspendResumeStuckWarningTimeout".parse()?,
            value: stuck_warning_timeout_seconds.into(),
        }))
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.power.SuspendResumeStuckWarningTimeout",
                ))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await?;

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.RebootOnStalledSuspendBlocker".parse()?,
            value: reboot_on_stalled_suspend_blocker.into(),
        }))
        .await?;

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.LongWakeLeaseTimeout".parse()?,
            value: long_wake_lease_timeout_seconds.into(),
        }))
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.power.RebootOnStalledSuspendBlocker",
                ))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.LongWakeLeaseTimeout"))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await?;

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.WaitForSuspendingToken".parse()?,
            value: wait_for_suspending_token.into(),
        }))
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.WaitForSuspendingToken"))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await?;

    // Expose capabilities from system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.suspend.Stats"))
                .capability(Capability::protocol_by_name("fuchsia.power.system.ActivityGovernor"))
                .capability(Capability::protocol_by_name(
                    "fuchsia.power.system.ExecutionStateManager",
                ))
                .capability(Capability::protocol_by_name("fuchsia.power.system.BootControl"))
                .capability(Capability::protocol_by_name("fuchsia.power.system.CpuElementManager"))
                .capability(Capability::service_by_name(
                    "fuchsia.power.broker.ElementInfoProviderService",
                ))
                .from(&component_ref)
                .to(Ref::parent()),
        )
        .await?;

    if use_fake_sag {
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("test.sagcontrol.State"))
                    .from(&component_ref)
                    .to(Ref::parent()),
            )
            .await?;
    }

    let realm_id = Arc::new(OnceLock::<String>::new());
    let realm_id_clone = realm_id.clone();
    let fake_boost_ref = builder
        .add_local_child(
            FAKE_BOOST_CHILD_NAME,
            move |handles| {
                let realm_id = realm_id_clone.clone();
                async move {
                    let mut fs = ServiceFs::new();
                    fs.dir("svc").add_fidl_service(
                        move |stream: fcpumanager::BoostRequestStream| {
                            let realm_id = realm_id.clone();
                            fasync::Task::local(async move {
                                let inspector = fuchsia_inspect::component::inspector();
                                let id_str = realm_id.get().expect("realm_id not set");
                                let realm_id_str = id_str.to_string();
                                if let Err(e) = run_fake_boost(
                                    inspector.root().clone_weak(),
                                    realm_id_str,
                                    stream,
                                )
                                .await
                                {
                                    warn!("FakeBoost failed: {:?}", e);
                                }
                            })
                            .detach();
                        },
                    );
                    fs.serve_connection(handles.outgoing_dir)?;
                    fs.collect::<()>().await;
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new(),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.cpu.manager.Boost"))
                .from(&fake_boost_ref)
                .to(&component_ref),
        )
        .await?;

    let realm = builder.build().await?;
    realm_id.set(realm.root.child_name().to_string()).unwrap();
    Ok(SagRealm { realm })
}

async fn run_fake_boost(
    node: INode,
    realm_id: String,
    mut stream: fcpumanager::BoostRequestStream,
) -> Result<()> {
    let active = Arc::new(AtomicBool::new(false));
    let active_clone = active.clone();

    let realm_node = node.create_child(realm_id);
    let _node = realm_node.create_lazy_child_with_thread_local("fake-boost", move || {
        let active = active_clone.clone();
        async move {
            let inspector = fuchsia_inspect::Inspector::default();
            inspector.root().record_bool("active", active.load(Ordering::Relaxed));
            Ok(inspector)
        }
        .boxed_local()
    });

    while let Some(request) = stream.try_next().await? {
        match request {
            fcpumanager::BoostRequest::Boost { responder } => {
                log::info!("FakeBoost: Received Boost request");
                let (server_token, client_token) = zx::EventPair::create();
                let active = active.clone();
                active.store(true, Ordering::Relaxed);

                fasync::Task::local(async move {
                    let _ =
                        fasync::OnSignals::new(server_token, zx::Signals::EVENTPAIR_PEER_CLOSED)
                            .await;
                    log::info!("FakeBoost: Client closed token, ending boost");
                    active.store(false, Ordering::Relaxed);
                })
                .detach();

                responder.send(Ok(client_token))?;
            }
            fcpumanager::BoostRequest::_UnknownMethod { .. } => {
                log::warn!("FakeBoost: Unknown method called");
            }
        }
    }
    Ok(())
}
