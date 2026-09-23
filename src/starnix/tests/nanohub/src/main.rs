// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fake_datachannel::FakeDataChannel;
use fake_display_server::{DisplayRequests, mock_display_server};
use fake_nanohub_server::mock_nanohub_server;
use fake_socket_tunnel::mock_socket_tunnel;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use log::info;
use {
    fidl_fuchsia_hardware_google_nanohub as fnanohub,
    fidl_fuchsia_hardware_sockettunnel as fsockettunnel,
};
mod fake_datachannel;
mod fake_display_server;
mod fake_nanohub_server;
mod fake_socket_tunnel;

#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("nanohub_test")
            .from_relative_url("#meta/container_with_endpoint_reader.cm"),
    )
    .await
    .unwrap();

    let display_requests = DisplayRequests::default();
    let display_server_mock = builder
        .add_local_child(
            "fake_display_server",
            {
                let display_requests = display_requests.clone();
                move |handles: LocalComponentHandles| {
                    Box::pin(mock_display_server(handles, display_requests.clone()))
                }
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fnanohub::DisplayDeviceMarker>())
                .from(&display_server_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    let nanohub_server_mock = builder
        .add_local_child(
            "fake_nanohub_server",
            move |handles: LocalComponentHandles| Box::pin(mock_nanohub_server(handles)),
            ChildOptions::new(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fnanohub::DeviceMarker>())
                .from(&nanohub_server_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    let socket_tunnel_mock = builder
        .add_local_child(
            "fake_socket_tunnel",
            move |handles: LocalComponentHandles| Box::pin(mock_socket_tunnel(handles)),
            ChildOptions::new(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsockettunnel::DeviceMarker>())
                .from(&socket_tunnel_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    let mock1 = builder
        .add_local_child(
            "mock1",
            move |handles: LocalComponentHandles| {
                Box::pin(FakeDataChannel::fake_driverservice(
                    "test_endpoint1".to_string(),
                    "test_endpoint1_data".to_string(),
                    handles,
                ))
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();

    let mock2 = builder
        .add_local_child(
            "mock2",
            move |handles: LocalComponentHandles| {
                Box::pin(FakeDataChannel::fake_driverservice(
                    "test_endpoint2".to_string(),
                    "test_endpoint2_data".to_string(),
                    handles,
                ))
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fnanohub::StarnixDataChannelServiceMarker>())
                .from(&mock1)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fnanohub::StarnixDataChannelServiceMarker>())
                .from(&mock2)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    info!("starting realm");
    let kernel_with_container_and_endpoint_reader = builder.build().await.unwrap();
    let realm_moniker =
        format!("realm_builder:{}", kernel_with_container_and_endpoint_reader.root.child_name());
    info!(realm_moniker:%; "started");
    let endpoint_reader_moniker = format!("{realm_moniker}/nanohub_user");

    // Wait for the endpoint_reader program to run...
    info!(endpoint_reader_moniker:%; "waiting for endpoint_reader to exit");
    let stopped = EventMatcher::ok()
        .moniker(&endpoint_reader_moniker)
        .wait::<Stopped>(&mut events)
        .await
        .unwrap();
    let status = stopped.result().unwrap().status;
    info!(status:?; "endpoint_reader stopped");
    assert_eq!(status, ExitStatus::Clean);

    let display_requests = display_requests.lock();
    assert_eq!(*display_requests, vec!["GetDisplayState", "GetDisplayInfo", "GetDisplaySelect"]);
}
