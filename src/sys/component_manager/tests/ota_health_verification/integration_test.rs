// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use cm_rust::push_box;
use cm_types::{Name, RelativePath};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_update_verify as fupdate;
use fuchsia_async as fasync;
use fuchsia_component::server as fserver;
use fuchsia_component_test::{
    Capability, ChildOptions, ChildRef, LocalComponentHandles, RealmBuilder, Ref, Route,
};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt, TryStreamExt};

async fn verifier_client(
    handles: LocalComponentHandles,
    expected_res: i32,
    mut success_reporter: mpsc::Sender<()>,
) -> Result<(), Error> {
    let verifier: fupdate::HealthVerificationProxy =
        handles.connect_to_protocol().expect("error connecting to health check verification");
    let res = verifier.query_health_checks().await.unwrap();

    assert_eq!(res, expected_res);

    success_reporter.send(()).await.expect("failed to report success from verifier client");
    Ok(())
}

async fn mock_server(
    handles: LocalComponentHandles,
    health_status: fupdate::HealthStatus,
    success_reporter: mpsc::Sender<()>,
) -> Result<(), Error> {
    let mut fs = fserver::ServiceFs::new();
    let mut tasks = vec![];

    fs.dir("svc").add_fidl_service(
        move |mut stream: fupdate::ComponentOtaHealthCheckRequestStream| {
            let mut success_reporter = success_reporter.clone();
            tasks.push(fasync::Task::local(async move {
                while let Some(fupdate::ComponentOtaHealthCheckRequest::GetHealthStatus {
                    responder,
                }) = stream.try_next().await.expect("error running health check service")
                {
                    success_reporter.send(()).await.expect("failed to report success");
                    responder.send(health_status).expect("failed to send health status");
                }
            }));
        },
    );

    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    panic!("component should not exit on its own");
}

async fn test_builder(
    expected_verify_result: zx::sys::zx_status_t,
    verify_success_sender: mpsc::Sender<()>,
) -> Result<RealmBuilder, Error> {
    let builder = RealmBuilder::new().await?;

    let verify_component = builder
        .add_local_child(
            "verifier",
            move |mh| {
                Box::pin(verifier_client(mh, expected_verify_result, verify_success_sender.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.update.verify.HealthVerification",
                ))
                .from(Ref::parent())
                .to(&verify_component),
        )
        .await?;

    Ok(builder)
}

async fn add_health_check_expose(
    builder: &RealmBuilder,
    component: &ChildRef,
) -> Result<(), Error> {
    let mut decl = builder.get_component_decl(component).await?;
    push_box(
        &mut decl.capabilities,
        cm_rust::CapabilityDecl::Protocol(cm_rust::ProtocolDecl {
            name: Name::new(fupdate::ComponentOtaHealthCheckMarker::PROTOCOL_NAME).unwrap(),
            source_path: Some(
                cm_types::Path::new(format!(
                    "/svc/{}",
                    fupdate::ComponentOtaHealthCheckMarker::PROTOCOL_NAME
                ))
                .unwrap(),
            ),
            delivery: cm_rust::DeliveryType::Immediate,
        }),
    );
    push_box(
        &mut decl.exposes,
        cm_rust::ExposeDecl::Protocol(cm_rust::ExposeProtocolDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: Name::new(fupdate::ComponentOtaHealthCheckMarker::PROTOCOL_NAME).unwrap(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::ExposeTarget::Framework,
            target_name: Name::new(fupdate::ComponentOtaHealthCheckMarker::PROTOCOL_NAME).unwrap(),
            availability: cm_rust::Availability::Required,
        }),
    );
    builder.replace_component_decl(component, decl).await?;
    Ok(())
}

/// Asserts that all component monikers listed in the configuration implement the server side of the
/// `ComponentOtaHealthCheck` protocol and report a healthy status.
#[fuchsia::test]
async fn healthy_components_returns_healthy() -> Result<(), Error> {
    let (verify_success_sender, mut verify_success_receiver) = mpsc::channel(1);
    let (success_sender_1, mut success_receiver_1) = mpsc::channel(1);
    let (success_sender_2, mut success_receiver_2) = mpsc::channel(1);
    let expected_verify_result = zx::sys::ZX_OK;

    let builder = test_builder(expected_verify_result, verify_success_sender.clone()).await?;
    let healthy_component_one = builder
        .add_local_child(
            "thing_one",
            move |mh| {
                Box::pin(mock_server(mh, fupdate::HealthStatus::Healthy, success_sender_1.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;
    let healthy_component_two = builder
        .add_local_child(
            "thing_two",
            move |mh| {
                Box::pin(mock_server(mh, fupdate::HealthStatus::Healthy, success_sender_2.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;

    add_health_check_expose(&builder, &healthy_component_one).await?;
    add_health_check_expose(&builder, &healthy_component_two).await?;

    let _realm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await?;

    assert!(verify_success_receiver.next().await.is_some());
    assert!(success_receiver_1.next().await.is_some());
    assert!(success_receiver_2.next().await.is_some());

    Ok(())
}

/// Asserts that the components corresponding to the monikers in the configuration
/// must be present in the component tree. Although `thing_one` reports a healthy
/// status, `thing_two` is missing so we treat that as a failing case.
#[fuchsia::test]
async fn missing_component_fails_to_verify() -> Result<(), Error> {
    let (verify_success_sender, mut verify_success_receiver) = mpsc::channel(1);
    let (success_sender_1, mut success_receiver_1) = mpsc::channel(1);
    let expected_verify_result = zx::Status::BAD_STATE.into_raw();
    let builder = test_builder(expected_verify_result, verify_success_sender.clone()).await?;

    let healthy_component_one = builder
        .add_local_child(
            "thing_one",
            move |mh| {
                Box::pin(mock_server(mh, fupdate::HealthStatus::Healthy, success_sender_1.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;
    add_health_check_expose(&builder, &healthy_component_one).await?;

    let _realm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await?;

    assert!(verify_success_receiver.next().await.is_some());
    assert!(success_receiver_1.next().await.is_some());

    Ok(())
}

/// Assert that a component reporting an Unhealthy status results in the verifier reporting
/// an error status.
#[fuchsia::test]
async fn unhealthy_components_short_circuits_verifier() -> Result<(), Error> {
    let (verify_success_sender, mut verify_success_receiver) = mpsc::channel(1);
    let (success_sender_1, mut success_receiver_1) = mpsc::channel(1);
    let (success_sender_2, mut success_receiver_2) = mpsc::channel(1);

    let expected_verify_result = zx::Status::BAD_STATE.into_raw();

    let builder = test_builder(expected_verify_result, verify_success_sender.clone()).await?;

    let component_one = builder
        .add_local_child(
            "thing_one",
            move |mh| {
                Box::pin(mock_server(
                    mh,
                    fupdate::HealthStatus::Unhealthy,
                    success_sender_1.clone(),
                ))
            },
            ChildOptions::new().eager(),
        )
        .await?;
    let component_two = builder
        .add_local_child(
            "thing_two",
            move |mh| {
                Box::pin(mock_server(mh, fupdate::HealthStatus::Healthy, success_sender_2.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;
    add_health_check_expose(&builder, &component_one).await?;
    add_health_check_expose(&builder, &component_two).await?;

    let _realm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await?;

    assert!(verify_success_receiver.next().await.is_some());
    assert!(success_receiver_1.next().await.is_some());
    // Verifier should still call thing 2.
    assert!(success_receiver_2.next().await.is_some());

    Ok(())
}

/// Assert that a component reporting an Unhealthy status does not affect the verifier
/// if that component's moniker is not listed in the configuration.
#[fuchsia::test]
async fn extra_component_doesnt_affect_result() -> Result<(), Error> {
    let (verify_success_sender, mut verify_success_receiver) = mpsc::channel(1);
    let (success_sender_1, mut success_receiver_1) = mpsc::channel(1);
    let (success_sender_2, mut success_receiver_2) = mpsc::channel(1);
    let (success_sender_3, mut success_receiver_3) = mpsc::channel(1);
    let expected_verify_result = zx::sys::ZX_OK;

    let builder = test_builder(expected_verify_result, verify_success_sender.clone()).await?;
    let healthy_component_one = builder
        .add_local_child(
            "thing_one",
            move |mh| {
                Box::pin(mock_server(mh, fupdate::HealthStatus::Healthy, success_sender_1.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;
    let healthy_component_two = builder
        .add_local_child(
            "thing_two",
            move |mh| {
                Box::pin(mock_server(mh, fupdate::HealthStatus::Healthy, success_sender_2.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;
    let unhealthy_component_three = builder
        .add_local_child(
            "thing_three",
            move |mh| {
                Box::pin(mock_server(
                    mh,
                    fupdate::HealthStatus::Unhealthy,
                    success_sender_3.clone(),
                ))
            },
            ChildOptions::new().eager(),
        )
        .await?;
    add_health_check_expose(&builder, &healthy_component_one).await?;
    add_health_check_expose(&builder, &healthy_component_two).await?;
    add_health_check_expose(&builder, &unhealthy_component_three).await?;

    let _realm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await?;

    assert!(verify_success_receiver.next().await.is_some());
    assert!(success_receiver_1.next().await.is_some());
    assert!(success_receiver_2.next().await.is_some());
    assert!(success_receiver_3.next().now_or_never().is_none());

    Ok(())
}
