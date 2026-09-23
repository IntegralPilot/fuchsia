// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fidl_fuchsia_hardware_power_statecontrol::{
    self as fpower, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use fidl_fuchsia_sys2 as fsys2;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, ChildRef, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::prelude::*;
use mock_reboot::MockRebootService;
use std::sync::Arc;

async fn build_realm() -> (RealmInstance, oneshot::Receiver<ShutdownOptions>) {
    let builder =
        RealmBuilder::with_params(RealmBuilderParams::new().from_relative_url("#meta/realm.cm"))
            .await
            .expect("created");

    let (sender, shutdown_reason_receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    let reboot_service = Arc::new(MockRebootService::new(Box::new(move |reason| {
        sender.lock().take().unwrap().send(reason).unwrap();
        Ok(())
    })));

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::spawn(
            Arc::clone(&reboot_service)
                .run_reboot_service(stream)
                .unwrap_or_else(|e| panic!("error running reboot service: {:#}", anyhow!(e))),
        )
        .detach()
    });

    let fs_holder = Mutex::new(Some(fs));
    let fake_reboot = builder
        .add_local_child(
            "fake_reboot",
            move |handles| {
                let mut rfs =
                    fs_holder.lock().take().expect("mock component should only be launched once");
                async {
                    rfs.serve_connection(handles.outgoing_dir).unwrap();
                    Ok(rfs.collect().await)
                }
                .boxed()
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fpower::AdminMarker>())
                .from(&fake_reboot)
                .to(&ChildRef::from("kernel")),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys2::LifecycleControllerMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    (builder.build().await.unwrap(), shutdown_reason_receiver)
}

#[fuchsia::test]
async fn test_reboot_ota_update() {
    let (realm_instance, shutdown_options_receiver) = build_realm().await;
    let lifecycle_controller: fsys2::LifecycleControllerProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let (_, binder_server) = fidl::endpoints::create_endpoints();
    lifecycle_controller
        .start_instance("./reboot_ota_update", binder_server)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        shutdown_options_receiver.await.unwrap(),
        ShutdownOptions {
            action: Some(ShutdownAction::Reboot),
            reasons: Some(vec![ShutdownReason::SystemUpdate]),
            ..Default::default()
        }
    );
}

#[fuchsia::test]
async fn test_reboot_no_args() {
    let (realm_instance, shutdown_options_receiver) = build_realm().await;
    let lifecycle_controller: fsys2::LifecycleControllerProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let (_, binder_server) = fidl::endpoints::create_endpoints();
    lifecycle_controller.start_instance("./reboot_no_args", binder_server).await.unwrap().unwrap();

    fasync::Timer::new(std::time::Duration::from_secs(1)).await;
    assert_eq!(
        shutdown_options_receiver.await.unwrap(),
        ShutdownOptions {
            action: Some(ShutdownAction::Reboot),
            reasons: Some(vec![ShutdownReason::StarnixContainerNoReason]),
            ..Default::default()
        }
    );
}
