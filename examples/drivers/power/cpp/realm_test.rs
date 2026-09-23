// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_driver_test::RealmArgs;
use fidl_fuchsia_examples as fex;
use fidl_fuchsia_power_system as fps;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{ChildOptions, LocalComponentHandles, RealmBuilder};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};

async fn sag_serve(
    mut stream: fps::ActivityGovernorRequestStream,
    mut sender: mpsc::Sender<ClientEnd<fps::SuspendBlockerMarker>>,
) {
    while let Some(fps::ActivityGovernorRequest::RegisterSuspendBlocker { payload, responder }) =
        stream.try_next().await.expect("Stream failed")
    {
        sender.try_send(payload.suspend_blocker.unwrap()).expect("Sender failed");
        let (fake_lease, _) = zx::EventPair::create();
        let _ = responder.send(Ok(fake_lease));
    }
}

async fn echo_server(mut stream: fex::EchoRequestStream, mut sender: mpsc::Sender<()>) {
    while let Some(fex::EchoRequest::EchoString { value, responder }) =
        stream.try_next().await.expect("echo stream failed")
    {
        let _ = responder.send(&value);
        sender.try_send(()).expect("send failed");
    }
}

async fn capability_provider(
    handles: LocalComponentHandles,
    sender: mpsc::Sender<ClientEnd<fps::SuspendBlockerMarker>>,
    echo_sender: mpsc::Sender<()>,
) -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream: fps::ActivityGovernorRequestStream| {
        fasync::Task::spawn(sag_serve(stream, sender.clone())).detach()
    });
    fs.dir("svc").add_fidl_service(move |stream: fex::EchoRequestStream| {
        fasync::Task::spawn(echo_server(stream, echo_sender.clone())).detach();
    });
    fs.serve_connection(handles.outgoing_dir)?;
    Ok(fs.collect::<()>().await)
}

async fn setup_capability_provider(
    builder: &RealmBuilder,
    offers: &Vec<ftest::Capability>,
) -> Result<(mpsc::Receiver<ClientEnd<fps::SuspendBlockerMarker>>, mpsc::Receiver<()>)> {
    let (sender, receiver) = mpsc::channel(1);

    let (echo_sender, echo_receiver) = mpsc::channel::<()>(1);

    builder.driver_test_realm_setup().await?;

    let waiter = builder
        .add_local_child(
            "capability-provider",
            move |handles: LocalComponentHandles| {
                Box::pin(capability_provider(handles, sender.clone(), echo_sender.clone()))
            },
            ChildOptions::new(),
        )
        .await?;

    builder.driver_test_realm_add_dtr_offers(offers, (&waiter).into()).await?;

    Ok((receiver, echo_receiver))
}

// TODO(https://fxbug.dev/552000162) Re-enable after tests are based on power
// elements instead of `fuchsia.power.system/SuspendBlocker`.
#[fuchsia::test]
async fn test_power_driver() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let dtr_offers = vec![
        fuchsia_component_test::Capability::protocol::<fps::ActivityGovernorMarker>().into(),
        fuchsia_component_test::Capability::protocol::<fex::EchoMarker>().into(),
    ];
    let (mut receiver, mut echo_receiver) =
        setup_capability_provider(&builder, &dtr_offers).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance
        .driver_test_realm_start(RealmArgs {
            root_driver: Some("#meta/power_driver.cm".to_owned()),
            dtr_offers: Some(dtr_offers),
            ..Default::default()
        })
        .await?;

    let proxy = receiver.try_next()?.ok_or_else(|| anyhow::anyhow!("missing proxy"))?.into_proxy();
    // Invoke suspend
    proxy.before_suspend().await?;
    // Invoke resume
    proxy.after_resume().await?;

    echo_receiver.try_next()?.ok_or_else(|| anyhow::anyhow!("echo not called"))?;

    Ok(())
}

/// This test expects that the driver will make not attempt to connect to SAG since suspend is
/// disabled. It does expect that the driver uses the Echo protocol, indicating it started
/// successfully.
#[fuchsia::test]
async fn test_power_driver_suspend_disabled() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let dtr_offers = vec![
        fuchsia_component_test::Capability::protocol::<fps::ActivityGovernorMarker>().into(),
        fuchsia_component_test::Capability::protocol::<fex::EchoMarker>().into(),
    ];
    let (mut receiver, mut echo_receiver) =
        setup_capability_provider(&builder, &dtr_offers).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance
        .driver_test_realm_start(RealmArgs {
            root_driver: Some("#meta/power_driver_suspend_disabled.cm".to_owned()),
            dtr_offers: Some(dtr_offers),
            ..Default::default()
        })
        .await?;

    assert!(receiver.try_next().is_err());
    echo_receiver.try_next()?.ok_or_else(|| anyhow::anyhow!("echo not called"))?;

    Ok(())
}

/// Expect that we don't see activity on the Echo protocol because we expect the driver will crash
/// since suspend is enabled, but it fails to connect to SAG, which is not offered to the driver
/// under test.
#[fuchsia::test]
async fn test_suspend_enabled_but_no_sag() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    let dtr_offers = vec![fuchsia_component_test::Capability::protocol::<fex::EchoMarker>().into()];

    let (_receiver, mut echo_receiver) = setup_capability_provider(&builder, &dtr_offers).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance
        .driver_test_realm_start(RealmArgs {
            root_driver: Some("#meta/power_driver.cm".to_owned()),
            dtr_offers: Some(dtr_offers),
            ..Default::default()
        })
        .await?;

    assert!(echo_receiver.try_next().is_err());

    Ok(())
}
