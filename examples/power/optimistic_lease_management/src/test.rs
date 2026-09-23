// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl::endpoints;
use fidl_fuchsia_example_power as fexample;
use fidl_fuchsia_hardware_power_suspend as fhardware_suspend;
use fidl_fuchsia_power_system as fsag;
use fidl_fuchsia_testing_harness as ftest_harness;
use fidl_test_sagcontrol as sagcontrol;
use fidl_test_suspendcontrol as fsuspend_control;
use fidl_test_systemactivitygovernor as sag_test;
use ftest::{ChildOptions, RealmBuilder};
use fuchsia_async as fasync;
use fuchsia_component::client as component;
use fuchsia_component_test as ftest;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::lock::Mutex;
use log::info;
use power_framework_test_realm::PowerFrameworkTestRealmBuilder;
use rand::Rng as _;

#[fuchsia::test]
async fn run_components() -> Result<()> {
    let _sag_realm = component::connect_to_protocol::<sag_test::RealmFactoryMarker>();

    let (_client, _server) = endpoints::create_endpoints::<ftest_harness::RealmProxy_Marker>();

    let realm_builder = RealmBuilder::new().await?;
    realm_builder.power_framework_test_realm_setup().await?;
    let server = realm_builder
        .add_child(
            "server",
            "fuchsia-pkg://fuchsia.com/olm#meta/server.cm",
            ChildOptions::new().eager(),
        )
        .await?;

    let client = realm_builder
        .add_child(
            "client",
            "fuchsia-pkg://fuchsia.com/olm#meta/client.cm",
            ChildOptions::new().eager(),
        )
        .await?;

    // Create a channel that the local, test component uses to signal it
    // has a wake lease and the main test code can set boot complete.
    let (tx, mut rx) = mpsc::unbounded::<()>();
    let test_controller = std::sync::Arc::new(TestController { channel: Mutex::new(tx) });

    let test_controller_ref = test_controller.clone();
    let test_controller_component = realm_builder
        .add_local_child(
            "test-controller",
            move |handles: ftest::LocalComponentHandles| {
                let ref_copy = test_controller_ref.clone();
                Box::pin(async move {
                    let component_run = ref_copy.local_component(handles).await;
                    if let Err(e) = component_run {
                        panic!("local component run failure: {:#?}", e);
                    }
                    Ok(())
                })
            },
            ChildOptions::new().eager(),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(ftest::Capability::protocol::<fexample::FrameControlMarker>())
                .from(&server)
                .to(&test_controller_component),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(ftest::Capability::protocol::<
                    fidl_fuchsia_tracing_provider::RegistryMarker,
                >())
                .from(ftest::Ref::parent())
                .to(&server),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(ftest::Capability::protocol::<
                    fidl_fuchsia_tracing_provider::RegistryMarker,
                >())
                .from(ftest::Ref::parent())
                .to(&client),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(ftest::Capability::protocol::<fexample::MessageSourceMarker>())
                .from(&server)
                .to(&client),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(ftest::Capability::protocol::<fexample::CounterMarker>())
                .from(&server)
                .to(ftest::Ref::parent()),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(
                    ftest::Capability::protocol::<fexample::CounterMarker>()
                        .as_("client_counter_marker"),
                )
                .from(&server)
                .to(ftest::Ref::parent()),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(ftest::Capability::protocol::<fsag::ActivityGovernorMarker>())
                .from(&ftest::ChildRef::from("power_framework_test_realm"))
                .to(&server),
        )
        .await?;

    realm_builder
        .add_route(
            ftest::Route::new()
                .capability(ftest::Capability::protocol::<fsag::ActivityGovernorMarker>())
                .from(&ftest::ChildRef::from("power_framework_test_realm"))
                .to(&test_controller_component),
        )
        .await?;

    let realm = realm_builder.build().await?;

    // Give us one suspend state since this is required
    let suspend_controller: fsuspend_control::DeviceProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;
    suspend_controller
        .set_suspend_states(&fsuspend_control::DeviceSetSuspendStatesRequest {
            suspend_states: Some(vec![fhardware_suspend::SuspendState {
                resume_latency: Some(zx::MonotonicDuration::from_micros(10).into_nanos()),
                ..Default::default()
            }]),
            ..Default::default()
        })
        .await?
        .expect("setting suspend states failed");

    // For informational purposes, monitor the state of SAG
    let sag_controller: sagcontrol::StateProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
    let sag_controller_copy = sag_controller.clone();
    fasync::Task::local(async move {
        loop {
            let status = sag_controller_copy.watch().await.unwrap();
            info!("new SAG state: {:?}", status);
        }
    })
    .detach();

    // Wait for the local test component to signal it talked to the server
    // component. This is important so the server component can be prepared
    // to take a wake lease before we set boot complete on SAG.
    let _ = rx.next().await;
    sag_controller.set_boot_complete().await?;

    // We'll use this to observe when SAG makes an request to suspend the system.
    let suspend_result = suspend_controller.await_suspend().await;

    info!("Got suspend result: {:?}", suspend_result);

    // SAG suspended the system, see what the respective counts are from the
    // server and client.
    let server_count_protocol: fexample::CounterProxy = realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("failed to connect to server's counter protocol");

    let server_count = server_count_protocol.get().await.expect("Call to get SERVER count failed.");
    info!("Message count from server is {}", server_count);

    let client_count_protocol = realm
        .root
        .connect_to_named_protocol_at_exposed_dir::<fexample::CounterMarker>(
            "client_counter_marker",
        )
        .expect("failed to connect to server's counter protocol");

    let client_count = client_count_protocol.get().await.expect("Call to get CLIENT count failed.");
    info!("Message count from client is {}", client_count);

    // Check that _some_ messages were sent and that the counts from client and server match.
    assert_ne!(0, server_count);
    assert_eq!(server_count, client_count);

    Ok(())
}

struct TestController {
    channel: Mutex<mpsc::UnboundedSender<()>>,
}

impl TestController {
    async fn local_component(&self, handles: ftest::LocalComponentHandles) -> Result<()> {
        let duration_ms = 10000u16;

        // Acquire a lease until we can have the server take over managing any wake lease
        let sag: fsag::ActivityGovernorProxy =
            handles.connect_to_protocol().expect("couldn't connec to SAG");

        let lease = sag
            .acquire_wake_lease("initial")
            .await
            .expect("FIDL faile")
            .expect("SAG failed created lease");

        {
            self.channel.lock().await.start_send(()).context("lock failed")?;
        }

        let fraction = {
            let mut rand_src = rand::rng();
            rand_src.next_u32() as f64 / u32::MAX as f64
        };
        info!("fraction is: {}", fraction);

        // Run for at least 500ms before dropping the wake lease and triggering
        // the first suspend attempt.
        let offset =
            std::cmp::max(500, (fraction * Into::<f64>::into(duration_ms)).abs().round() as u64);

        let fctrl: fexample::FrameControlProxy =
            handles.connect_to_protocol().context("couldn't access frame control")?;

        fctrl.start_frame(duration_ms, duration_ms / 2).await.context("start frame failed")?;

        fasync::Timer::new(zx::BootDuration::from_millis(offset.try_into().unwrap())).await;
        info!("dropping lease");
        drop(lease);
        drop(sag);
        Ok(())
    }
}
