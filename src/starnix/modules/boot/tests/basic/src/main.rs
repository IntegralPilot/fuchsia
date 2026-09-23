// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use futures::StreamExt;
use log::info;

#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("boot_controller")
            .from_relative_url("#meta/container.cm"),
    )
    .await
    .unwrap();

    let (reporter_send, mut reporter_requests) = futures::channel::mpsc::unbounded();
    let boot_controller_mock = builder
        .add_local_child(
            "boot_controller",
            move |handles| {
                let reporter_send = reporter_send.clone();
                Box::pin(async move {
                    let mut fs = ServiceFs::new();
                    fs.serve_connection(handles.outgoing_dir).unwrap();
                    fs.dir("svc").add_fidl_service(|h: fsys::BootControllerRequestStream| Ok(h));
                    fs.forward(reporter_send).await.unwrap();
                    Ok(())
                })
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys::BootControllerMarker>())
                .from(&boot_controller_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    info!("starting realm");
    let kernel_with_container = builder.build().await.unwrap();
    let realm_moniker = format!("realm_builder:{}", kernel_with_container.root.child_name());
    info!(realm_moniker:%; "started");
    let boot_reader_moniker = format!("{realm_moniker}/boot_reader");

    info!(boot_reader_moniker:%; "waiting for boot_reader to exit");
    let stopped = EventMatcher::ok()
        .moniker(&boot_reader_moniker)
        .wait::<Stopped>(&mut events)
        .await
        .unwrap();
    let status = stopped.result().unwrap().status;
    info!(status:?; "boot reader stopped");
    assert_eq!(status, ExitStatus::Clean);

    info!("waiting for boot notification");
    let mut reporter_client = reporter_requests.next().await.unwrap();
    let request = reporter_client.next().await.unwrap().unwrap();
    match request {
        fsys::BootControllerRequest::Notify { responder } => {
            responder.send().unwrap();
        }
        _ => panic!(),
    };
}
