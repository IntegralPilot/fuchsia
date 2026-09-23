// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_driver_test::RealmArgs;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{ChildOptions, LocalComponentHandles, RealmBuilder};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use {fidl_fuchsia_power_system as fps, fuchsia_async as fasync};

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

async fn sag_component(
    handles: LocalComponentHandles,
    sender: mpsc::Sender<ClientEnd<fps::SuspendBlockerMarker>>,
) -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream: fps::ActivityGovernorRequestStream| {
        fasync::Task::spawn(sag_serve(stream, sender.clone())).detach()
    });
    fs.serve_connection(handles.outgoing_dir)?;
    Ok(fs.collect::<()>().await)
}

#[fuchsia::test]
async fn test_power_driver() -> Result<()> {
    let (sender, mut receiver) = mpsc::channel(1);

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    let waiter = builder
        .add_local_child(
            "fake-sag",
            move |handles: LocalComponentHandles| Box::pin(sag_component(handles, sender.clone())),
            ChildOptions::new(),
        )
        .await?;
    let offer =
        fuchsia_component_test::Capability::protocol::<fps::ActivityGovernorMarker>().into();
    let dtr_offers = vec![offer];

    builder.driver_test_realm_add_dtr_offers(&dtr_offers, (&waiter).into()).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance
        .driver_test_realm_start(RealmArgs {
            root_driver: Some("#meta/power_rust_driver.cm".to_owned()),
            dtr_offers: Some(dtr_offers),
            ..Default::default()
        })
        .await?;

    let proxy = receiver.try_next()?.ok_or_else(|| anyhow::anyhow!("missing proxy"))?.into_proxy();
    // Invoke suspend
    proxy.before_suspend().await?;
    // Invoke resume
    proxy.after_resume().await?;

    Ok(())
}
