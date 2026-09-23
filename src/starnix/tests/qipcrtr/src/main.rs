// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fake_qrtr_connector::mock_qrtr_client_service;
use fidl_fuchsia_hardware_qualcomm_router as fqrtr;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use log::info;

mod fake_qrtr_connector;

#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("qipcrtr_test")
            .from_relative_url("#meta/qipcrtr_client_container.cm"),
    )
    .await
    .unwrap();

    let qrtr_client_service_mock = builder
        .add_local_child(
            "fake_qrtr_client_service",
            move |handles: LocalComponentHandles| Box::pin(mock_qrtr_client_service(handles)),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fqrtr::ClientServiceMarker>())
                .from(&qrtr_client_service_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    info!("starting realm");
    let kernel_with_container_and_client = builder.build().await.unwrap();
    let realm_moniker =
        format!("realm_builder:{}", kernel_with_container_and_client.root.child_name());
    info!(realm_moniker:%; "started");
    let client_moniker = format!("{realm_moniker}/qipcrtr_client");

    info!(client_moniker:%; "waiting for client to exit");
    let stopped =
        EventMatcher::ok().moniker(&client_moniker).wait::<Stopped>(&mut events).await.unwrap();
    let status = stopped.result().unwrap().status;
    info!(status:?; "client stopped");
    assert_eq!(status, ExitStatus::Clean);
}
