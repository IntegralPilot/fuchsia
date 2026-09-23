// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This is an integration test of the Introspector protocol.
//!
//! It sets up the following realm:
//!
//! ```
//! realm builder realm
//!   - mock_runner (local component)
//!   - nested component_manager
//!     - test_root (root component of tested component manager)
//!       - main_realm (exposes Introspector from framework)
//!         - main_realm_child
//!         - single_run_collection
//!       - unrelated_realm
//!         - unrelated_realm_child
//! ```
//!
//! The mock_runner exposes a ComponentRunner protocol, which is then offered
//! to component_manager then to test_root. There, it gets turned into a runner
//! capability which is used to run main_realm_child and unrelated_realm_child.
//!
//! Additionally, single_run_collection is used to test the behavior of tokens
//! after the corresponding component instance is destroyed.
//!
//! The test cases make assertions on the behavior of Introspector with the
//! observed tokens.

use component_events::events::*;
use component_events::matcher::*;
use component_events::sequence::{EventSequence, Ordering};
use fcomponent::{IntrospectorProxy, RealmProxy};
use fdecl::StartupMode;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys2;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use fuchsia_sync::Mutex as StdMutex;
use futures::SinkExt;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures_util::{FutureExt, StreamExt, TryStreamExt};
use std::sync::Arc;

struct SenderReceiver {
    sender: Mutex<mpsc::UnboundedSender<fcrunner::ComponentRunnerRequest>>,
    receiver: Mutex<mpsc::UnboundedReceiver<fcrunner::ComponentRunnerRequest>>,
}

impl SenderReceiver {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded();
        Self { sender: Mutex::new(sender), receiver: Mutex::new(receiver) }
    }
}

/// [`MockRunner`] captures requests from component_manager to run the
/// "./main_realm/main_realm_child" and "./unrelated_realm/unrelated_realm_child" components.
struct MockRunner {
    main_realm: SenderReceiver,
    unrelated_realm: SenderReceiver,
}

impl MockRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self { main_realm: SenderReceiver::new(), unrelated_realm: SenderReceiver::new() })
    }

    async fn main_realm_start(&self) -> fcrunner::ComponentRunnerRequest {
        self.main_realm.receiver.lock().await.next().await.unwrap()
    }

    async fn unrelated_realm_start(&self) -> fcrunner::ComponentRunnerRequest {
        self.unrelated_realm.receiver.lock().await.next().await.unwrap()
    }

    async fn serve(self: Arc<Self>, mut stream: fcrunner::ComponentRunnerRequestStream) {
        while let Ok(Some(request)) = stream.try_next().await {
            match &request {
                fcrunner::ComponentRunnerRequest::Start { start_info, .. } => {
                    match start_info.resolved_url.as_ref().map(String::as_str) {
                        Some(url) if url.contains("main_realm_child") => {
                            self.main_realm.sender.lock().await.send(request).await.unwrap()
                        }
                        Some(url) if url.contains("unrelated_realm_child") => {
                            self.unrelated_realm.sender.lock().await.send(request).await.unwrap()
                        }
                        _ => {
                            panic!(
                                "Unexpected resolved URL: {}",
                                start_info.resolved_url.as_ref().unwrap()
                            );
                        }
                    }
                }
                fcrunner::ComponentRunnerRequest::_UnknownMethod { .. } => {
                    panic!("Unknown ComponentRunner request");
                }
            }
        }
    }
}

struct Fixture {
    _realm_instance: RealmInstance,
    event_stream: EventStream,
    introspector: IntrospectorProxy,
    realm: RealmProxy,
}

/// Creates a nested component manager and route the component runner capability
/// from `mock_runner` to `component_manager`, which will then make that available
/// as a namespaced capability to the test realm.
async fn setup_realm(mock_runner: Arc<MockRunner>) -> Fixture {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/test_root.cm"),
    )
    .await
    .unwrap();

    let builder = builder
        .with_nested_component_manager("#meta/component_manager_debug_with_mock_runner.cm")
        .await
        .unwrap();

    let mut fs = ServiceFs::new();
    let mut svc = fs.dir("svc");

    let mock_runner = Arc::clone(&mock_runner);
    svc.add_fidl_service(move |stream| {
        fasync::Task::spawn(mock_runner.clone().serve(stream)).detach()
    });

    let fs = StdMutex::new(Some(fs));
    let mock_runner = builder
        .add_local_child(
            "mock_runner",
            move |handles| {
                let mut fs = fs.lock().take().expect("mock runner should only be started once");
                async {
                    fs.serve_connection(handles.outgoing_dir).unwrap();
                    Ok(fs.collect().await)
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
                .capability(Capability::protocol_by_name(
                    "fuchsia.component.runner.ComponentRunner",
                ))
                .from(&mock_runner)
                .to(Ref::self_()),
        )
        .await
        .unwrap();

    let instance = builder.build().await.unwrap();
    let proxy: fcomponent::EventStreamProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    proxy.wait_for_ready().await.unwrap();
    let event_stream = EventStream::new(proxy);
    instance.start_component_tree().await.unwrap();

    let realm_query: fsys2::RealmQueryProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (exposed_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    realm_query
        .open_directory(".", fsys2::OpenDirType::ExposedDir, server_end)
        .await
        .unwrap()
        .unwrap();

    let (introspector, server_end) =
        fidl::endpoints::create_proxy::<fcomponent::IntrospectorMarker>();
    exposed_dir
        .open(
            fcomponent::IntrospectorMarker::PROTOCOL_NAME,
            fio::Flags::PROTOCOL_SERVICE,
            &Default::default(),
            server_end.into_channel(),
        )
        .unwrap();

    let (realm, server_end) = fidl::endpoints::create_proxy::<fcomponent::RealmMarker>();
    exposed_dir
        .open(
            fcomponent::RealmMarker::PROTOCOL_NAME,
            fio::Flags::PROTOCOL_SERVICE,
            &Default::default(),
            server_end.into_channel(),
        )
        .unwrap();

    Fixture { _realm_instance: instance, event_stream, introspector, realm }
}

#[fuchsia::test]
async fn moniker_relative_to_scope() {
    let mock_runner = MockRunner::new();
    let fixture = setup_realm(mock_runner.clone()).await;
    let Fixture { introspector, .. } = fixture;
    let mut main_start_request = mock_runner.main_realm_start().await;

    let fcrunner::ComponentRunnerRequest::Start { start_info, .. } = &mut main_start_request else {
        panic!("unexpected runner request");
    };
    let component_instance = start_info.component_instance.take().unwrap();

    let moniker = introspector.get_moniker(component_instance).await.unwrap().unwrap();
    // The token corresponds to "./main_realm/main_realm_child". However, the framework
    // capability is scoped at "./main_realm". Therefore, the relative moniker should be
    // "main_realm_child".
    assert_eq!(&moniker, "main_realm_child");
}

#[fuchsia::test]
async fn moniker_out_of_scope() {
    let mock_runner = MockRunner::new();
    let fixture = setup_realm(mock_runner.clone()).await;
    let Fixture { introspector, .. } = fixture;
    let mut unrelated_start_request = mock_runner.unrelated_realm_start().await;

    let fcrunner::ComponentRunnerRequest::Start { start_info, .. } = &mut unrelated_start_request
    else {
        panic!("unexpected runner request");
    };
    let component_instance = start_info.component_instance.take().unwrap();

    // The token corresponds to "./unrelated_realm/unrelated_realm_child". However, the framework
    // capability is scoped at "./main_realm". Therefore, the relative moniker cannot be retrieved.
    let moniker_error = introspector.get_moniker(component_instance).await.unwrap().unwrap_err();
    assert_eq!(moniker_error, fcomponent::Error::InstanceNotFound);
}

#[fuchsia::test]
async fn invalid_token() {
    let mock_runner = MockRunner::new();
    let fixture = setup_realm(mock_runner.clone()).await;
    let Fixture { introspector, .. } = fixture;

    let component_instance = zx::Event::create();
    let moniker_error = introspector.get_moniker(component_instance).await.unwrap().unwrap_err();
    assert_eq!(moniker_error, fcomponent::Error::InstanceNotFound);
}

#[fuchsia::test]
async fn valid_when_stopped() {
    let mock_runner = MockRunner::new();
    let fixture = setup_realm(mock_runner.clone()).await;
    let Fixture { event_stream, introspector, .. } = fixture;

    let component_instance = {
        let mut main_start_request = mock_runner.main_realm_start().await;
        let fcrunner::ComponentRunnerRequest::Start { start_info, .. } = &mut main_start_request
        else {
            panic!("unexpected runner request");
        };
        let component_instance = start_info.component_instance.take().unwrap();

        let moniker = introspector
            .get_moniker(component_instance.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&moniker, "main_realm_child");

        component_instance
        // Dropping the controller channel will notify component manager that the component has stopped.
    };

    // Wait until component manager knows the component is stopped.
    EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok().r#type(Started::TYPE).moniker("./main_realm/main_realm_child"),
                EventMatcher::ok().r#type(Stopped::TYPE).moniker("./main_realm/main_realm_child"),
            ],
            Ordering::Ordered,
        )
        .expect(event_stream)
        .await
        .unwrap();

    // The token is still valid after the component is stopped.
    let moniker = introspector.get_moniker(component_instance).await.unwrap().unwrap();
    assert_eq!(&moniker, "main_realm_child");
}

#[fuchsia::test]
async fn invalidate_when_destroyed() {
    let mock_runner = MockRunner::new();
    let fixture = setup_realm(mock_runner.clone()).await;
    let Fixture { event_stream, introspector, realm, .. } = fixture;
    // Ignore the start request from the eager child.
    let _ = mock_runner.main_realm_start().await;
    // Use the Realm protocol to start another main_realm_child in the single-run collection.
    realm
        .create_child(
            &fdecl::CollectionRef { name: "single_run_collection".to_string() },
            &fdecl::Child {
                name: Some("main_realm_child".to_string()),
                url: Some("#meta/main_realm_child.cm".to_string()),
                startup: Some(StartupMode::Lazy),
                ..Default::default()
            },
            fcomponent::CreateChildArgs { ..Default::default() },
        )
        .await
        .unwrap()
        .unwrap();

    let component_instance = {
        let mut main_start_request = mock_runner.main_realm_start().await;
        let fcrunner::ComponentRunnerRequest::Start { start_info, .. } = &mut main_start_request
        else {
            panic!("unexpected runner request");
        };
        let component_instance = start_info.component_instance.take().unwrap();

        let moniker = introspector
            .get_moniker(component_instance.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&moniker, "single_run_collection:main_realm_child");

        component_instance
        // Dropping the controller channel will notify component manager that the component has stopped.
        // Because it is in a single-run collection, the component will also be destroyed.
    };

    // Wait until component manager knows the component is destroyed.
    EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok()
                    .r#type(Started::TYPE)
                    .moniker("./main_realm/single_run_collection:main_realm_child"),
                EventMatcher::ok()
                    .r#type(Stopped::TYPE)
                    .moniker("./main_realm/single_run_collection:main_realm_child"),
                EventMatcher::ok()
                    .r#type(Destroyed::TYPE)
                    .moniker("./main_realm/single_run_collection:main_realm_child"),
            ],
            Ordering::Ordered,
        )
        .expect(event_stream)
        .await
        .unwrap();

    // The component instance was destroyed so we cannot get the moniker.
    let moniker_error = introspector.get_moniker(component_instance).await.unwrap().unwrap_err();
    assert_eq!(moniker_error, fcomponent::Error::InstanceNotFound);
}
