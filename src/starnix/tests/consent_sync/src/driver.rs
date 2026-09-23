// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_settings::{PrivacyRequest, PrivacyRequestStream};
use fidl_fuchsia_sys2 as fsys2;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use std::sync::Arc;

async fn build_realm() -> (RealmInstance, mpsc::Receiver<bool>) {
    let builder =
        RealmBuilder::with_params(RealmBuilderParams::new().from_relative_url("#meta/realm.cm"))
            .await
            .expect("created");

    let (sender, receiver) = mpsc::channel(10);
    let sender = Arc::new(Mutex::new(sender));

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |mut stream: PrivacyRequestStream| {
        let sender = sender.clone();
        fasync::Task::spawn(async move {
            while let Some(request) = stream.try_next().await.expect("failed to serve Privacy") {
                match request {
                    PrivacyRequest::Set { settings, responder } => {
                        if let Some(consent) = settings.user_data_sharing_consent {
                            let mut sender_guard = sender.lock();
                            sender_guard.try_send(consent).expect("failed to send consent");
                        }
                        responder.send(Ok(())).expect("failed to send response");
                    }
                    _ => panic!("Unexpected Privacy request"),
                }
            }
        })
        .detach();
    });

    let fs_holder = Mutex::new(Some(fs));
    let mock_privacy = builder
        .add_local_child(
            "mock_privacy",
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
                .capability(Capability::protocol::<fidl_fuchsia_settings::PrivacyMarker>())
                .from(&mock_privacy)
                .to(Ref::child("container")),
        )
        .await
        .unwrap();

    // Route LogSink to Starnix components for logs
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fidl_fuchsia_logger::LogSinkMarker>())
                .from(Ref::parent())
                .to(Ref::child("kernel"))
                .to(Ref::child("container"))
                .to(Ref::child("consent_sync_test")),
        )
        .await
        .unwrap();

    (builder.build().await.unwrap(), receiver)
}

#[fuchsia::test]
async fn test_consent_sync() -> Result<()> {
    let (realm_instance, mut consent_receiver) = build_realm().await;
    let lifecycle_controller: fsys2::LifecycleControllerProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let (_, binder_server) = fidl::endpoints::create_endpoints();
    lifecycle_controller
        .start_instance("./consent_sync_test", binder_server)
        .await
        .unwrap()
        .unwrap();

    // Expect "1" (true)
    assert_eq!(consent_receiver.next().await, Some(true));

    // Expect "0" (false)
    assert_eq!(consent_receiver.next().await, Some(false));

    Ok(())
}
