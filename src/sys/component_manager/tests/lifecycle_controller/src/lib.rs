// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::client::connect_to_protocol;
use fuchsia_component_test::{ChildOptions, RealmBuilder};
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt};
use zx::Event;
use {
    fidl_fuchsia_component as fcomp, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_process as fprocess, fidl_fuchsia_sys2 as fsys,
};

#[fuchsia::test]
async fn static_child() {
    let lifecycle_controller = connect_to_protocol::<fsys::LifecycleControllerMarker>().unwrap();
    let realm_query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    // echo server is unresolved
    let instance = realm_query.get_instance("./echo_server").await.unwrap().unwrap();
    assert!(instance.resolved_info.is_none());

    lifecycle_controller.resolve_instance("./echo_server").await.unwrap().unwrap();

    // echo server is resolved
    let instance = realm_query.get_instance("./echo_server").await.unwrap().unwrap();
    let resolved_info = instance.resolved_info.unwrap();
    assert!(resolved_info.execution_info.is_none());

    let (binder, server) = fidl::endpoints::create_proxy();
    lifecycle_controller.start_instance("./echo_server", server).await.unwrap().unwrap();

    // echo server is running
    let instance = realm_query.get_instance("./echo_server").await.unwrap().unwrap();
    let resolved_info = instance.resolved_info.unwrap();
    assert!(resolved_info.execution_info.is_some());

    // Check that the binder protocol is still alive
    let mut event_stream = binder.take_event_stream();
    let fut = event_stream.next();
    futures::pin_mut!(fut);
    assert!(fut.as_mut().now_or_never().is_none());

    lifecycle_controller.stop_instance("./echo_server").await.unwrap().unwrap();

    // echo server is not running
    let instance = realm_query.get_instance("./echo_server").await.unwrap().unwrap();
    let resolved_info = instance.resolved_info.unwrap();
    assert!(resolved_info.execution_info.is_none());

    // the binder of the component should be closed by now
    assert!(fut.await.is_none());
}

#[fuchsia::test]
async fn dynamic_child() {
    let lifecycle_controller = connect_to_protocol::<fsys::LifecycleControllerMarker>().unwrap();
    let realm_query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    // dynamic echo server doesn't exist
    let error =
        realm_query.get_instance("./servers:dynamic_echo_server").await.unwrap().unwrap_err();
    assert_eq!(error, fsys::GetInstanceError::InstanceNotFound);

    lifecycle_controller
        .create_instance(
            ".",
            &fdecl::CollectionRef { name: "servers".to_string() },
            &fdecl::Child {
                name: Some("dynamic_echo_server".to_string()),
                url: Some("#meta/echo_server.cm".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                ..Default::default()
            },
            fcomp::CreateChildArgs::default(),
        )
        .await
        .unwrap()
        .unwrap();

    // dynamic echo server is unresolved
    let instance =
        realm_query.get_instance("./servers:dynamic_echo_server").await.unwrap().unwrap();
    assert!(instance.resolved_info.is_none());

    lifecycle_controller.resolve_instance("./servers:dynamic_echo_server").await.unwrap().unwrap();

    // dynamic echo server is resolved
    let instance =
        realm_query.get_instance("./servers:dynamic_echo_server").await.unwrap().unwrap();
    let resolved_info = instance.resolved_info.unwrap();
    assert!(resolved_info.execution_info.is_none());

    let (binder, server) = fidl::endpoints::create_proxy();
    lifecycle_controller
        .start_instance("./servers:dynamic_echo_server", server)
        .await
        .unwrap()
        .unwrap();

    // dynamic echo server is running
    let instance =
        realm_query.get_instance("./servers:dynamic_echo_server").await.unwrap().unwrap();
    let resolved_info = instance.resolved_info.unwrap();
    assert!(resolved_info.execution_info.is_some());

    // Check that the binder protocol is still alive
    let mut event_stream = binder.take_event_stream();
    let fut = event_stream.next();
    futures::pin_mut!(fut);
    assert!(fut.as_mut().now_or_never().is_none());

    lifecycle_controller.stop_instance("./servers:dynamic_echo_server").await.unwrap().unwrap();

    // dynamic echo server is not running
    let instance =
        realm_query.get_instance("./servers:dynamic_echo_server").await.unwrap().unwrap();
    let resolved_info = instance.resolved_info.unwrap();
    assert!(resolved_info.execution_info.is_none());

    // the binder of the component should be closed by now
    assert!(fut.await.is_none());

    lifecycle_controller
        .destroy_instance(
            ".",
            &fdecl::ChildRef {
                name: "dynamic_echo_server".to_string(),
                collection: Some("servers".to_string()),
            },
        )
        .await
        .unwrap()
        .unwrap();

    // dynamic echo server doesn't exist
    let error =
        realm_query.get_instance("./servers:dynamic_echo_server").await.unwrap().unwrap_err();
    assert_eq!(error, fsys::GetInstanceError::InstanceNotFound);
}

#[fuchsia::test]
async fn dynamic_child_with_arguments() {
    let builder = RealmBuilder::new().await.unwrap();
    let (sender, mut receiver) = mpsc::channel(1);
    let _child =
        builder
            .add_local_child(
                "numbered_handles_child",
                move |mut handles| {
                    let mut sender = sender.clone();
                    async move {
                        sender
                            .send(handles.take_numbered_handle(
                                HandleInfo::new(HandleType::User0, 0).as_raw(),
                            ))
                            .await
                            .expect("failed to send handle");
                        Ok(())
                    }
                    .boxed()
                },
                ChildOptions::new().eager(),
            )
            .await
            .unwrap();
    let (url, _local_component_task) = builder.initialize().await.unwrap();

    let lifecycle_controller = connect_to_protocol::<fsys::LifecycleControllerMarker>().unwrap();

    lifecycle_controller
        .create_instance(
            ".",
            &fdecl::CollectionRef { name: "servers".to_string() },
            &fdecl::Child {
                name: Some("dynamic_child_with_arguments".to_string()),
                url: Some(url),
                startup: Some(fdecl::StartupMode::Lazy),
                ..Default::default()
            },
            fcomp::CreateChildArgs::default(),
        )
        .await
        .unwrap()
        .unwrap();

    lifecycle_controller
        .resolve_instance("./servers:dynamic_child_with_arguments")
        .await
        .unwrap()
        .unwrap();

    let handle = Event::create();
    let koid = handle.basic_info().unwrap().koid;

    let (_binder, server) = fidl::endpoints::create_proxy();
    lifecycle_controller
        .start_instance_with_args(
            "./servers:dynamic_child_with_arguments/numbered_handles_child",
            server,
            fcomp::StartChildArgs {
                numbered_handles: Some(vec![fprocess::HandleInfo {
                    handle: handle.into(),
                    id: HandleInfo::new(HandleType::User0, 0).as_raw(),
                }]),
                ..Default::default()
            },
        )
        .await
        .expect("failed to send start_instance_with_args")
        .expect("failed to start instance");

    let option_handle = receiver.next().await.expect("failed to receive");
    let handle = option_handle.expect("local component did not receive handle");
    assert_eq!(koid, handle.basic_info().unwrap().koid, "child received unexpected handle");

    lifecycle_controller
        .destroy_instance(
            ".",
            &fdecl::ChildRef {
                name: "dynamic_child_with_arguments".to_string(),
                collection: Some("servers".to_string()),
            },
        )
        .await
        .unwrap()
        .unwrap();
}
