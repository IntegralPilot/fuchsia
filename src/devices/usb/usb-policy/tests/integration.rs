// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_hardware_usb_policy as fpolicy;
use fidl_fuchsia_usb_policy as usb_policy;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route,
};
use futures::StreamExt;

enum IncomingRequest {
    Controller(fpolicy::ControllerRequestStream),
}

/// Handle a stream of `ControllerRequest`s for the mock USB policy service.
///
/// If `rx_opt` is `Some`, it will block responding to the first `WatchDeviceState`
/// until the receiver receives a signal. If `rx_opt` is `None`, it responds immediately.
async fn handle_controller_stream_generic(
    mut stream: fpolicy::ControllerRequestStream,
    rx_opt: Option<
        std::sync::Arc<std::sync::Mutex<Option<futures::channel::oneshot::Receiver<()>>>>,
    >,
) -> Result<(), Error> {
    let mut state_changed = true;
    while let Some(request) = stream.next().await {
        match request {
            Ok(fpolicy::ControllerRequest::WatchDeviceState { responder }) => {
                if state_changed {
                    if let Some(ref rx) = rx_opt {
                        let rx_lock = rx.lock().unwrap().take();
                        if let Some(r) = rx_lock {
                            let _ = r.await;
                        }
                    }
                    let update = fpolicy::DeviceStateUpdate {
                        state: Some(fpolicy::DeviceState::Configured),
                        address: Some(42),
                        ..Default::default()
                    };
                    let _ = responder.send(Ok(&update));
                    state_changed = false;
                } else {
                    let () = std::future::pending().await;
                }
            }
            Ok(fpolicy::ControllerRequest::_UnknownMethod { .. }) => todo!(),
            Err(_) => break, // Client disconnected
        }
    }
    Ok(())
}

/// Runs a generic mock service server for testing `usb-policy`.
///
/// Dispatches incoming requests to `handle_controller_stream_generic`.
async fn mock_service_server_generic(
    handles: LocalComponentHandles,
    rx_opt: Option<
        std::sync::Arc<std::sync::Mutex<Option<futures::channel::oneshot::Receiver<()>>>>,
    >,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();

    fs.dir("svc").add_fidl_service_instance("default", |req: fpolicy::ServiceRequest| match req {
        fpolicy::ServiceRequest::Controller(stream) => IncomingRequest::Controller(stream),
    });

    fs.serve_connection(handles.outgoing_dir)?;
    fs.for_each_concurrent(None, |req| {
        let rx_clone = rx_opt.clone();
        async move {
            match req {
                IncomingRequest::Controller(stream) => {
                    let _ = handle_controller_stream_generic(stream, rx_clone).await;
                }
            }
        }
    })
    .await;
    Ok(())
}

/// Add standard capability routes between `mock_service`, `policy`, and the test parent.
///
/// Routes:
/// - `fuchsia.hardware.usb.policy.Service` from `mock_service` to `policy`
/// - `fuchsia.logger.LogSink` from parent to both
/// - `fuchsia.usb.policy.Health` and `fuchsia.usb.policy.PolicyProvider` from `policy` to parent
async fn add_standard_routes(builder: &RealmBuilder) -> Result<(), Error> {
    builder
        .add_route(
            Route::new()
                .capability(Capability::service_by_name("fuchsia.hardware.usb.policy.Service"))
                .from(Ref::child("mock_service"))
                .to(Ref::child("policy")),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(Ref::child("mock_service"))
                .to(Ref::child("policy")),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.usb.policy.Health"))
                .capability(Capability::protocol_by_name("fuchsia.usb.policy.PolicyProvider"))
                .from(Ref::child("policy"))
                .to(Ref::parent()),
        )
        .await?;

    Ok(())
}

#[fuchsia::test]
async fn test_usb_policy() -> Result<(), Error> {
    let builder = RealmBuilder::new().await?;

    builder
        .add_local_child(
            "mock_service",
            move |handles| Box::pin(mock_service_server_generic(handles, None)),
            ChildOptions::new(),
        )
        .await?;

    builder.add_child("policy", "#meta/usb-policy.cm", ChildOptions::new()).await?;

    add_standard_routes(&builder).await?;

    let instance = builder.build().await?;

    let (provider_proxy, server_end) = create_proxy::<usb_policy::PolicyProviderMarker>();
    instance.root.connect_request_to_named_protocol_at_exposed_dir(
        "fuchsia.usb.policy.PolicyProvider",
        server_end.into_channel(),
    )?;
    let mut update =
        provider_proxy.watch_device_state().await?.expect("Failed to watch device state");
    while update.state != Some(fpolicy::DeviceState::Configured) {
        update = provider_proxy.watch_device_state().await?.expect("Failed to watch device state");
    }
    assert_eq!(update.state, Some(fpolicy::DeviceState::Configured));
    assert_eq!(update.address, Some(42));

    let health_proxy: usb_policy::HealthProxy =
        instance.root.connect_to_protocol_at_exposed_dir()?;
    let report = health_proxy.get_report().await?.expect("Failed to get report");
    assert_eq!(report.state, Some(fpolicy::DeviceState::Configured));
    assert_eq!(report.address, Some(42));

    Ok(())
}

#[fuchsia::test]
async fn test_usb_policy_delayed() -> Result<(), Error> {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    let rx_opt = std::sync::Arc::new(std::sync::Mutex::new(Some(rx)));

    let builder = RealmBuilder::new().await?;

    builder
        .add_local_child(
            "mock_service",
            move |handles| {
                let rx_clone = rx_opt.clone();
                Box::pin(mock_service_server_generic(handles, Some(rx_clone)))
            },
            ChildOptions::new(),
        )
        .await?;

    builder.add_child("policy", "#meta/usb-policy.cm", ChildOptions::new()).await?;

    add_standard_routes(&builder).await?;

    let instance = builder.build().await?;

    // Verify Health is initially None (unblocked startup)
    let health_proxy: usb_policy::HealthProxy =
        instance.root.connect_to_protocol_at_exposed_dir()?;
    let report = health_proxy.get_report().await?.expect("Failed to get report");
    assert_eq!(report.state, None);

    // Unblock mock service
    let _ = tx.send(());

    // Wait for PolicyProvider to become ready using watch (blocks until driver answers)
    let (provider_proxy, server_end) = create_proxy::<usb_policy::PolicyProviderMarker>();
    instance.root.connect_request_to_named_protocol_at_exposed_dir(
        "fuchsia.usb.policy.PolicyProvider",
        server_end.into_channel(),
    )?;
    let mut update =
        provider_proxy.watch_device_state().await?.expect("Failed to watch device state");
    while update.state != Some(fpolicy::DeviceState::Configured) {
        update = provider_proxy.watch_device_state().await?.expect("Failed to watch device state");
    }
    assert_eq!(update.state, Some(fpolicy::DeviceState::Configured));

    // Verify Health is now ready
    let report = health_proxy.get_report().await?.expect("Failed to get report");
    assert_eq!(report.state, Some(fpolicy::DeviceState::Configured));

    Ok(())
}
