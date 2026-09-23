// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use cm_rust::{ComponentDecl, FidlIntoNative, push_box};
use fidl::endpoints::{ProtocolMarker, Proxy, ServerEnd, create_proxy};
use fidl_fidl_examples_routing_echo as fecho;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fcdecl;
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use fuchsia_component::runtime;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use fuchsia_runtime::{HandleInfo, HandleType};
use fuchsia_sync::Mutex;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::{FutureExt, SinkExt, StreamExt, TryStreamExt};
use std::sync::Arc;
use test_case::test_case;
use vfs::file::vmo::read_only;
use vfs::pseudo_directory;

const COLLECTION_NAME: &'static str = "col";
const STATIC_CHILD_NAME: &'static str = "static_child";

/// Marks the `component_status` as not running when dropped.
struct DropMarksComponentAsStopped {
    component_status: ComponentStatus,
}

impl Drop for DropMarksComponentAsStopped {
    fn drop(&mut self) {
        let mut is_running_guard = self.component_status.is_running.lock();
        if !*is_running_guard {
            panic!("component was already stopped");
        }
        assert!(*is_running_guard, "component is already stopped");
        *is_running_guard = false;
    }
}

/// Tracks if a local component is running. The local component calls `new_run` when it starts
/// which marks it as running, and when the struct returned by that function is dropped the
/// component is marked as not running.
#[derive(Clone, Default)]
struct ComponentStatus {
    // We use fuchsia_sync::Mutex because we need to unlock it in a drop function, which is not async
    is_running: Arc<Mutex<bool>>,
}

impl ComponentStatus {
    fn is_running(&self) -> bool {
        *self.is_running.lock()
    }

    fn new_run(&self) -> DropMarksComponentAsStopped {
        let mut is_running_guard = self.is_running.lock();
        assert!(!*is_running_guard, "component is already running");
        *is_running_guard = true;
        DropMarksComponentAsStopped { component_status: self.clone() }
    }
}

/// Creates a nested component manager instance with a collection, and invokes the `create_child`
/// call to create a child within that collection.
async fn launch_child_in_a_collection_in_nested_component_manager(
    collection_ref: fcdecl::CollectionRef,
    child_decl: fcdecl::Child,
    child_args: fcomponent::CreateChildArgs,
) -> RealmInstance {
    let builder = RealmBuilder::new().await.unwrap();
    let child_args = Arc::new(Mutex::new(Some(child_args)));
    let (child_creation_tx, child_creation_rx) = oneshot::channel::<()>();
    let child_creation_tx = Arc::new(Mutex::new(Some(child_creation_tx)));
    let realm_user = builder
        .add_local_child(
            "realm_user",
            move |handles| {
                let collection_ref = collection_ref.clone();
                let child_decl = child_decl.clone();
                let child_args = child_args.clone();
                let child_creation_tx = child_creation_tx.clone();
                async move {
                    let realm_proxy: fcomponent::RealmProxy =
                        handles.connect_to_protocol().unwrap();
                    let child_args = {
                        let mut child_args_guard = child_args.lock();
                        child_args_guard.take().unwrap()
                    };
                    realm_proxy
                        .create_child(&collection_ref, &child_decl, child_args)
                        .await
                        .unwrap()
                        .unwrap();
                    child_creation_tx.lock().take().unwrap().send(()).unwrap();
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    let static_child = builder
        .add_child(STATIC_CHILD_NAME, "#meta/echo_server.cm", ChildOptions::new())
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fcomponent::RealmMarker>())
                .from(Ref::framework())
                .to(&realm_user)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fruntime::CapabilitiesMarker>())
                .from(Ref::framework())
                .to(&realm_user)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fruntime::CapabilitiesMarker>())
                .from(Ref::framework())
                .to(&realm_user)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .from(Ref::parent())
                .to(Ref::collection(COLLECTION_NAME))
                .to(&static_child),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(Ref::collection(COLLECTION_NAME))
                .to(&static_child),
        )
        .await
        .unwrap();
    let mut realm_decl = builder.get_realm_decl().await.unwrap();
    push_box(
        &mut realm_decl.collections,
        fcdecl::Collection {
            name: Some(COLLECTION_NAME.to_string()),
            durability: Some(fcdecl::Durability::Transient),
            ..Default::default()
        }
        .fidl_into_native(),
    );
    builder.replace_realm_decl(realm_decl).await.unwrap();
    let cm_realm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    child_creation_rx.await.unwrap();
    cm_realm_instance
}

/// Represents a local child that was created in a nested realm builder.
struct SpawnedChild {
    /// The task which executes the local child.
    _local_child_task: Option<fasync::Task<()>>,
    /// The nested component manager instance that is hosting the local child.
    cm_realm_instance: RealmInstance,
    /// When the local child is started it will send its handles over this mpsc.
    handles_receiver: mpsc::UnboundedReceiver<LocalComponentHandles>,
    /// The `fuchsia.component.Controller` channel that was created for the child.
    controller_proxy: fcomponent::ControllerProxy,
    /// The `ChildRef` used to create the child.
    child_ref: fcdecl::ChildRef,
    /// Tracks when the component is and is not running.
    component_status: ComponentStatus,
    /// A oneshot that can be used to instruct the local component to exit.
    cancel_sender: Option<oneshot::Sender<()>>,
}

type CancelSender = oneshot::Sender<()>;

async fn build_local_child() -> (
    RealmBuilder,
    ComponentDecl,
    mpsc::UnboundedReceiver<LocalComponentHandles>,
    ComponentStatus,
    CancelSender,
) {
    let builder = RealmBuilder::new().await.unwrap();
    let (handles_sender, handles_receiver) = mpsc::unbounded();
    let (cancel_sender, cancel_receiver) = oneshot::channel::<()>();
    let component_status = ComponentStatus::default();
    let component_status_clone = component_status.clone();
    let cancel_receiver = Arc::new(Mutex::new(Some(cancel_receiver)));
    let child_ref = builder
        .add_local_child(
            "child",
            move |handles| {
                let mut handles_sender = handles_sender.clone();
                let component_status_clone = component_status_clone.clone();
                let cancel_receiver = cancel_receiver.clone();
                async move {
                    let _drop_marks_component_as_stopped = component_status_clone.new_run();
                    handles_sender.send(handles).await.unwrap();
                    let cancel_receiver = cancel_receiver.lock().take().unwrap();
                    let _ = cancel_receiver.await;
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();

    let child_decl = builder.get_component_decl(&child_ref).await.unwrap();
    (builder, child_decl, handles_receiver, component_status, cancel_sender)
}

/// Spawns a local child and populates the `SpawnedChild` struct.
async fn spawn_local_child_controller_from_create_child() -> SpawnedChild {
    let (builder, child_decl, handles_receiver, component_status, cancel_sender) =
        build_local_child().await;
    builder.replace_realm_decl(child_decl).await.unwrap();

    let (url, local_child_task) = builder.initialize().await.unwrap();
    let (controller_proxy, child_ref, cm_realm_instance) = spawn_child_with_url(&url).await;
    SpawnedChild {
        _local_child_task: Some(local_child_task),
        cm_realm_instance,
        handles_receiver,
        controller_proxy,
        child_ref,
        component_status,
        cancel_sender: Some(cancel_sender),
    }
}

async fn spawn_local_child_controller_from_open_controller() -> SpawnedChild {
    let (builder, child_decl, handles_receiver, component_status, cancel_sender) =
        build_local_child().await;
    builder.replace_realm_decl(child_decl).await.unwrap();

    let (url, local_child_task) = builder.initialize().await.unwrap();
    let (child_ref, cm_realm_instance) =
        spawn_child_with_url_with_args(&url, fcomponent::CreateChildArgs::default()).await;

    let realm_proxy: fcomponent::RealmProxy =
        cm_realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (controller_proxy, server_end) = create_proxy::<fcomponent::ControllerMarker>();
    realm_proxy.open_controller(&child_ref, server_end).await.unwrap().unwrap();

    SpawnedChild {
        _local_child_task: Some(local_child_task),
        cm_realm_instance,
        handles_receiver,
        controller_proxy,
        child_ref,
        component_status,
        cancel_sender: Some(cancel_sender),
    }
}

async fn spawn_child_with_url(
    url: &str,
) -> (fcomponent::ControllerProxy, fcdecl::ChildRef, RealmInstance) {
    let (controller_proxy, server_end) = create_proxy::<fcomponent::ControllerMarker>();
    let (child_ref, cm_realm_instance) = spawn_child_with_url_with_args(
        url,
        fcomponent::CreateChildArgs { controller: Some(server_end), ..Default::default() },
    )
    .await;
    (controller_proxy, child_ref, cm_realm_instance)
}

async fn spawn_child_with_url_with_args(
    url: &str,
    child_args: fcomponent::CreateChildArgs,
) -> (fcdecl::ChildRef, RealmInstance) {
    let collection_ref = fcdecl::CollectionRef { name: COLLECTION_NAME.to_string() };
    let child_decl = fcdecl::Child {
        name: Some("local_child".into()),
        url: Some(url.into()),
        startup: Some(fcdecl::StartupMode::Lazy),
        ..Default::default()
    };
    let cm_realm_instance = launch_child_in_a_collection_in_nested_component_manager(
        collection_ref,
        child_decl,
        child_args,
    )
    .await;
    let child_ref = fcdecl::ChildRef {
        name: "local_child".into(),
        collection: Some(COLLECTION_NAME.to_string()),
    };
    (child_ref, cm_realm_instance)
}

#[test_case(spawn_local_child_controller_from_create_child().boxed())]
#[test_case(spawn_local_child_controller_from_open_controller().boxed())]
#[fuchsia::test]
async fn destroy(spawn_child_future: BoxFuture<'static, SpawnedChild>) {
    let spawned_child = spawn_child_future.await;
    assert_matches!(spawned_child.controller_proxy.destroy().await.unwrap(), Ok(()));
    assert_matches!(spawned_child.controller_proxy.take_event_stream().try_next().await, Ok(None));

    let realm_proxy: fcomponent::RealmProxy =
        spawned_child.cm_realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (_controller, server) = create_proxy::<fcomponent::ControllerMarker>();
    assert_matches!(
        realm_proxy.open_controller(&spawned_child.child_ref, server).await.unwrap(),
        Err(fcomponent::Error::InstanceNotFound)
    );
}

#[fuchsia::test]
async fn destroy_err() {
    let spawned_child = spawn_local_child_controller_from_create_child().await;
    let realm_proxy: fcomponent::RealmProxy =
        spawned_child.cm_realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (controller, server) = create_proxy::<fcomponent::ControllerMarker>();
    let static_child_ref = fcdecl::ChildRef { name: STATIC_CHILD_NAME.into(), collection: None };
    realm_proxy.open_controller(&static_child_ref, server).await.unwrap().unwrap();
    assert_matches!(controller.destroy().await.unwrap(), Err(fcomponent::Error::AccessDenied));
}

#[test_case(spawn_local_child_controller_from_create_child().boxed())]
#[test_case(spawn_local_child_controller_from_open_controller().boxed())]
#[fuchsia::test]
async fn start(spawn_child_future: BoxFuture<'static, SpawnedChild>) {
    let mut spawned_child = spawn_child_future.await;
    let (_execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(Default::default(), execution_controller_server_end)
        .await
        .unwrap()
        .unwrap();
    assert!(
        spawned_child.handles_receiver.next().await.is_some(),
        "failed to observe the local child be started"
    );
    assert!(spawned_child.component_status.is_running());
}

#[fuchsia::test]
async fn start_with_namespace_entries() {
    let namespace_entries = pseudo_directory! {
        "file.txt" => read_only("hippos"),
    };

    let ns_proxy = vfs::directory::serve_read_only(
        namespace_entries,
        vfs::execution_scope::ExecutionScope::new(),
    );

    let mut spawned_child = spawn_local_child_controller_from_create_child().await;
    let (_execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(
            fcomponent::StartChildArgs {
                namespace_entries: Some(vec![fcomponent::NamespaceEntry {
                    path: Some("/test".to_string()),
                    directory: Some(ns_proxy.into_client_end().unwrap()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            execution_controller_server_end,
        )
        .await
        .unwrap()
        .unwrap();

    let local_component_handles = spawned_child.handles_receiver.next().await.unwrap();
    let test_dir_proxy = local_component_handles.clone_from_namespace("test").unwrap();
    let file_proxy =
        fuchsia_fs::directory::open_file(&test_dir_proxy, "file.txt", fio::PERM_READABLE)
            .await
            .unwrap();
    let file_contents = fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap();
    assert_eq!(file_contents, "hippos".to_string());
}

#[fuchsia::test]
async fn start_with_numbered_handles() {
    let mut spawned_child = spawn_local_child_controller_from_create_child().await;
    let handle_id = HandleInfo::new(HandleType::User0, 0).as_raw();
    let (s1, s2) = zx::Socket::create_stream();
    let (_execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(
            fcomponent::StartChildArgs {
                numbered_handles: Some(vec![fprocess::HandleInfo {
                    id: handle_id,
                    handle: s2.into_handle(),
                }]),
                ..Default::default()
            },
            execution_controller_server_end,
        )
        .await
        .unwrap()
        .unwrap();

    let mut local_component_handles = spawned_child.handles_receiver.next().await.unwrap();
    let s2_from_runner = local_component_handles
        .take_numbered_handle(handle_id)
        .expect("child was not given the numbered handle");
    assert_eq!(s1.basic_info().unwrap().related_koid, s2_from_runner.koid().unwrap());
}

#[fuchsia::test]
async fn start_with_dict() {
    let mut spawned_child = spawn_local_child_controller_from_create_child().await;

    let capabilities_proxy: fruntime::CapabilitiesProxy =
        spawned_child.cm_realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    // StartChild dictionary entries must be Sender capabilities.
    let (connector, mut receiver) =
        runtime::Connector::new_with_proxy(capabilities_proxy.clone()).await;

    let scope = fasync::Scope::new();
    let _task = fasync::Task::spawn(async move {
        loop {
            match receiver.next().await {
                Some(channel) => {
                    scope.spawn(async move {
                        let server_end = ServerEnd::<fecho::EchoMarker>::new(channel);
                        let mut stream = server_end.into_stream();
                        while let Some(fecho::EchoRequest::EchoString { value, responder }) =
                            stream.try_next().await.unwrap()
                        {
                            responder.send(value.as_ref().map(|s| &**s)).unwrap();
                        }
                    });
                }
                _ => panic!(),
            }
        }
    });

    // Create a sender from our receiver.

    let svc_dictionary = runtime::Dictionary::new_with_proxy(capabilities_proxy.clone()).await;
    svc_dictionary.insert("fidl.examples.routing.echo.Echo", connector).await;
    let dictionary = runtime::Dictionary::new_with_proxy(capabilities_proxy.clone()).await;
    dictionary.insert("svc", svc_dictionary).await;

    let (_execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(
            fcomponent::StartChildArgs {
                additional_inputs: Some(dictionary.handle),
                ..Default::default()
            },
            execution_controller_server_end,
        )
        .await
        .unwrap()
        .unwrap();

    let local_component_handles = spawned_child.handles_receiver.next().await.unwrap();

    let echo_proxy: fecho::EchoProxy =
        local_component_handles.connect_to_protocol().expect("failed to connect to Echo");

    let response = echo_proxy.echo_string(Some("hello")).await.expect("failed to call EchoString");
    assert!(response.is_some());
    assert_eq!(response.unwrap(), "hello".to_string());
}

#[test_case(spawn_local_child_controller_from_create_child().boxed())]
#[test_case(spawn_local_child_controller_from_open_controller().boxed())]
#[fuchsia::test]
async fn channel_is_closed_on_stop(spawn_child_future: BoxFuture<'static, SpawnedChild>) {
    let mut spawned_child = spawn_child_future.await;
    let (execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(Default::default(), execution_controller_server_end)
        .await
        .unwrap()
        .unwrap();
    assert!(
        spawned_child.handles_receiver.next().await.is_some(),
        "failed to observe the local child be started"
    );
    assert!(spawned_child.component_status.is_running());
    execution_controller_proxy.stop().unwrap();
    let execution_controller_channel = execution_controller_proxy.into_channel().unwrap();
    fasync::OnSignals::new(&execution_controller_channel, zx::Signals::CHANNEL_PEER_CLOSED)
        .await
        .unwrap();
    assert!(!spawned_child.component_status.is_running());
}

#[test_case(spawn_local_child_controller_from_create_child().boxed())]
#[test_case(spawn_local_child_controller_from_open_controller().boxed())]
#[fuchsia::test]
async fn on_stop_is_called(spawn_child_future: BoxFuture<'static, SpawnedChild>) {
    let mut spawned_child = spawn_child_future.await;
    let (execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(Default::default(), execution_controller_server_end)
        .await
        .unwrap()
        .unwrap();
    assert!(
        spawned_child.handles_receiver.next().await.is_some(),
        "failed to observe the local child be started"
    );
    assert!(spawned_child.component_status.is_running());
    execution_controller_proxy.stop().unwrap();
    if let Ok(Some(fcomponent::ExecutionControllerEvent::OnStop { stopped_payload })) =
        execution_controller_proxy.take_event_stream().try_next().await
    {
        assert_eq!(
            stopped_payload.status,
            Some(fcomponent::Error::InstanceDied.into_primitive() as i32)
        );
    } else {
        panic!("expected OnStop to be called");
    }
    assert!(!spawned_child.component_status.is_running());
}

#[test_case(spawn_local_child_controller_from_create_child().boxed())]
#[test_case(spawn_local_child_controller_from_open_controller().boxed())]
#[fuchsia::test]
async fn wait_for_exit(spawn_child_future: BoxFuture<'static, SpawnedChild>) {
    let mut spawned_child = spawn_child_future.await;
    let (execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(Default::default(), execution_controller_server_end)
        .await
        .unwrap()
        .unwrap();
    assert!(
        spawned_child.handles_receiver.next().await.is_some(),
        "failed to observe the local child be started"
    );
    assert!(spawned_child.component_status.is_running());
    spawned_child.cancel_sender.take().unwrap().send(()).unwrap();
    if let Ok(Some(fcomponent::ExecutionControllerEvent::OnStop { stopped_payload })) =
        execution_controller_proxy.take_event_stream().try_next().await
    {
        assert_eq!(stopped_payload.status, Some(zx::sys::ZX_OK));
    } else {
        panic!("expected OnStop to be called");
    }
    assert!(!spawned_child.component_status.is_running());
    let execution_controller_channel = execution_controller_proxy.into_channel().unwrap();
    fasync::OnSignals::new(&execution_controller_channel, zx::Signals::CHANNEL_PEER_CLOSED)
        .await
        .unwrap();
}

#[test_case(spawn_local_child_controller_from_create_child().boxed())]
#[test_case(spawn_local_child_controller_from_open_controller().boxed())]
#[fuchsia::test]
async fn start_when_already_started(spawn_child_future: BoxFuture<'static, SpawnedChild>) {
    let mut spawned_child = spawn_child_future.await;
    let (_execution_controller_proxy, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    spawned_child
        .controller_proxy
        .start(Default::default(), execution_controller_server_end)
        .await
        .unwrap()
        .unwrap();
    assert!(
        spawned_child.handles_receiver.next().await.is_some(),
        "failed to observe the local child be started"
    );
    assert!(spawned_child.component_status.is_running());
    let (execution_controller_proxy_2, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    let err = spawned_child
        .controller_proxy
        .start(Default::default(), execution_controller_server_end)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err, fcomponent::Error::InstanceAlreadyStarted);
    let execution_controller_channel = execution_controller_proxy_2.into_channel().unwrap();
    fasync::OnSignals::new(&execution_controller_channel, zx::Signals::CHANNEL_PEER_CLOSED)
        .await
        .unwrap();
}

#[fuchsia::test]
async fn get_exposed_dictionary() {
    let (controller_proxy, _child_ref, instance) =
        spawn_child_with_url("#meta/echo_server.cm").await;
    let handle = controller_proxy.get_output_dictionary().await.unwrap().unwrap();
    let capabilities_proxy: fruntime::CapabilitiesProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let output_dictionary = runtime::Dictionary { handle, capabilities_proxy };
    assert_matches!(
        output_dictionary.get(fecho::EchoMarker::DEBUG_NAME).await,
        Some(runtime::Capability::ConnectorRouter(_))
    );
}

#[fuchsia::test]
async fn get_output_dictionary() {
    let (controller_proxy, _child_ref, instance) =
        spawn_child_with_url("#meta/echo_server.cm").await;

    let exposed_dict = controller_proxy.get_output_dictionary().await.unwrap().unwrap();
    let capabilities_proxy: fruntime::CapabilitiesProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let dictionary = runtime::Dictionary { handle: exposed_dict, capabilities_proxy };
    assert_matches!(
        dictionary.get(fecho::EchoMarker::DEBUG_NAME).await,
        Some(runtime::Capability::ConnectorRouter(_))
    );
}

#[fuchsia::test]
async fn open_exposed_dir() {
    let (controller_proxy, _child_ref, _instance) =
        spawn_child_with_url("#meta/echo_server.cm").await;
    let (exposed_dir_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
    controller_proxy.open_exposed_dir(server_end).await.unwrap().unwrap();
    let echo_proxy =
        connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&exposed_dir_proxy).unwrap();

    assert_eq!(
        "Hello, World!".to_string(),
        echo_proxy.echo_string(Some("Hello, World!")).await.unwrap().unwrap()
    );
}
