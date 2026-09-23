// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_resourceboundcomposite_test as ft;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{ChildOptions, LocalComponentHandles, RealmBuilder};
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use log::info;

async fn run_waiter_server(mut stream: ft::WaiterRequestStream, mut sender: mpsc::Sender<()>) {
    while let Some(ft::WaiterRequest::Ack { status, .. }) =
        stream.try_next().await.expect("Stream failed")
    {
        assert_eq!(zx::Status::ok(status), Ok(()));
        info!("Received Ack request");
        sender.try_send(()).expect("Sender failed")
    }
}

enum IncomingService {
    Waiter(ft::WaiterRequestStream),
}

async fn waiter_component(handles: LocalComponentHandles, sender: mpsc::Sender<()>) -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(IncomingService::Waiter);
    fs.serve_connection(handles.outgoing_dir)?;

    fs.for_each_concurrent(None, |IncomingService::Waiter(stream)| {
        let sender = sender.clone();
        async move {
            run_waiter_server(stream, sender).await;
        }
    })
    .await;

    Ok(())
}

#[fuchsia::test]
async fn test_resourceboundcomposite() -> Result<()> {
    let (sender, mut receiver) = mpsc::channel(1);

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    let waiter = builder
        .add_local_child(
            "waiter",
            move |handles: LocalComponentHandles| {
                Box::pin(waiter_component(handles, sender.clone()))
            },
            ChildOptions::new(),
        )
        .await?;

    let offer = fuchsia_component_test::Capability::protocol::<ft::WaiterMarker>().into();
    let offers = vec![offer];
    let args = fdt::RealmArgs::default();

    builder
        .driver_test_realm_setup(Options2::new().driver_offers((&waiter).into(), offers), args)
        .await?;

    // Build the Realm.
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    info!("connected to the test realm!");

    // Wait for the driver to call Waiter.Ack.
    receiver.next().await.ok_or_else(|| anyhow!("Receiver failed"))?;
    instance.destroy().await?;

    Ok(())
}
