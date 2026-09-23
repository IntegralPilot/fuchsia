// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fidl_fuchsia_hardware_cpu_ctrl as fcpuctrl;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use log::info;

mod fake_cpu_ctrl;

#[fuchsia::main]
async fn main() {
    test_cpu_control_service().await;
    test_kernel_fallback().await;
}

async fn test_cpu_control_service() {
    let mut events = EventStream::open().await.unwrap();
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("cpufreq_test_cpu_control")
            .from_relative_url("#meta/container_with_cpu_control_client.cm"),
    )
    .await
    .unwrap();

    let cpu_ctrl_mock = builder
        .add_local_child(
            "cpu_ctrl",
            move |handles: LocalComponentHandles| {
                Box::pin(fake_cpu_ctrl::mock_cpu_ctrl_service(handles))
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fcpuctrl::ServiceMarker>())
                .from(&cpu_ctrl_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    info!("starting cpu_control realm");
    let instance = builder.build().await.unwrap();

    let realm_moniker = format!("realm_builder:{}", instance.root.child_name());
    info!(realm_moniker:%; "started");
    let client_moniker = format!("{realm_moniker}/cpu_control_client");

    info!(client_moniker:%; "waiting for cpu_control_client to exit");
    let stopped =
        EventMatcher::ok().moniker(&client_moniker).wait::<Stopped>(&mut events).await.unwrap();
    let status = stopped.result().unwrap().status;
    info!(status:?; "cpu_control_client stopped");
    assert_eq!(status, ExitStatus::Clean);
}

async fn test_kernel_fallback() {
    let mut events = EventStream::open().await.unwrap();
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("cpufreq_test_kernel_fallback")
            .from_relative_url("#meta/container_with_kernel_client.cm"),
    )
    .await
    .unwrap();

    info!("starting kernel fallback realm");
    let instance = builder.build().await.unwrap();

    let realm_moniker = format!("realm_builder:{}", instance.root.child_name());
    info!(realm_moniker:%; "started");
    let client_moniker = format!("{realm_moniker}/kernel_client");

    info!(client_moniker:%; "waiting for kernel_client to exit");
    let stopped =
        EventMatcher::ok().moniker(&client_moniker).wait::<Stopped>(&mut events).await.unwrap();
    let status = stopped.result().unwrap().status;
    info!(status:?; "kernel_client stopped");
    assert_eq!(status, ExitStatus::Clean);
}
