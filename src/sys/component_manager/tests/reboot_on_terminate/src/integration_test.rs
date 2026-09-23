// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use futures::channel::mpsc;
use futures::prelude::*;
use log::*;
use {fidl_fidl_test_components as ftest, fuchsia_async as fasync};

#[fuchsia::test]
async fn reboot_on_terminate_success() {
    let (send_trigger_called, mut receive_trigger_called) = mpsc::unbounded();
    let _instance = build_reboot_on_terminate_realm(
        "#meta/reboot_on_terminate_success.cm",
        send_trigger_called,
    )
    .await;

    // Wait for the test to signal that it received the shutdown request.
    info!("waiting for shutdown request");
    let _ = receive_trigger_called.next().await.expect("failed to receive results");
}

#[fuchsia::test]
async fn reboot_on_terminate_policy() {
    let (send_trigger_called, mut receive_trigger_called) = mpsc::unbounded();
    let _instance =
        build_reboot_on_terminate_realm("#meta/reboot_on_terminate_policy.cm", send_trigger_called)
            .await;

    // Wait for the test to signal that the security policy was correctly applied.
    info!("waiting for policy error");
    let _ = receive_trigger_called.next().await.expect("failed to receive results");
}

async fn build_reboot_on_terminate_realm(
    url: &str,
    send_trigger_called: mpsc::UnboundedSender<()>,
) -> RealmInstance {
    // Define the realm inside component manager.
    let builder = RealmBuilder::new().await.unwrap();
    let realm = builder
        .add_child_realm_from_relative_url("realm", url, ChildOptions::new().eager())
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .capability(Capability::protocol_by_name("fuchsia.component.resolver.RealmBuilder"))
                .capability(Capability::protocol_by_name("fuchsia.component.runner.RealmBuilder"))
                .capability(Capability::protocol_by_name("fuchsia.sys2.SystemController"))
                .from(Ref::parent())
                .to(&realm),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.hardware.power.statecontrol.Admin",
                ))
                .from(&realm)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    realm.with_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    // The realm under test will call the Trigger protocol when it's confirmed it's getting shut
    // down.
    let trigger = builder
        .add_local_child(
            "trigger",
            move |handles| Box::pin(trigger_mock(send_trigger_called.clone(), handles)),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fidl.test.components.Trigger"))
                .from(&trigger)
                .to(&realm),
        )
        .await
        .unwrap();

    builder.build().await.unwrap()
}

async fn trigger_mock(
    send_trigger_called: mpsc::UnboundedSender<()>,
    handles: LocalComponentHandles,
) -> Result<(), anyhow::Error> {
    let mut fs = ServiceFs::new();
    let mut tasks = vec![];
    fs.dir("svc").add_fidl_service(move |mut stream: ftest::TriggerRequestStream| {
        let mut send_trigger_called = send_trigger_called.clone();
        tasks.push(fasync::Task::local(async move {
            while let Some(ftest::TriggerRequest::Run { responder }) =
                stream.try_next().await.expect("failed to serve trigger service")
            {
                responder.send("received").expect("failed to send trigger response");
                send_trigger_called.send(()).await.expect("failed to send results");
            }
        }));
    });
    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}
