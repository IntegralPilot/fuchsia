// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, format_err};
use component_events::events::{Destroyed, Event, EventStream, Started};
use component_events::matcher::EventMatcher;
use component_events::sequence::*;
use fidl::endpoints::{ServiceMarker, ServiceProxy, create_proxy};
use fidl_fidl_test_components as ftest;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_examples as fecho;
use fidl_fuchsia_examples_services as fexamples;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys2;
use fuchsia_component::client;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route, ScopedInstance,
};
use fuchsia_fs::directory::{WatchEvent, WatchMessage, Watcher};
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{FutureExt, SinkExt, StreamExt, TryFutureExt, TryStreamExt};
use log::*;
use moniker::ChildName;
use std::collections::HashSet;
use std::sync::Arc;
use test_case::test_case;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::simple::Simple;
use vfs::execution_scope::ExecutionScope;
use vfs::pseudo_directory;

const BRANCHES_COLLECTION: &str = "branches";
const BRANCH_ONECOLL_COMPONENT_URL: &str = "#meta/service-routing-branch-onecoll.cm";
const BRANCH_TWOCOLL_COMPONENT_URL: &str = "#meta/service-routing-branch-twocoll.cm";
const A_ONECOLL_MONIKER: &str = "account_providers:a";
const B_ONECOLL_MONIKER: &str = "account_providers:b";
const A_TWOCOLL_MONIKER: &str = "account_providers_1:a";
const B_TWOCOLL_MONIKER: &str = "account_providers_2:b";
const ECHO_URL: &str = "#meta/multi-instance-echo-provider.cm";

struct TestInput {
    url: &'static str,
    provider_a_moniker: &'static str,
    provider_b_moniker: &'static str,
}

impl TestInput {
    fn new(test_type: TestType) -> Self {
        match test_type {
            TestType::OneCollection => Self {
                url: BRANCH_ONECOLL_COMPONENT_URL,
                provider_a_moniker: A_ONECOLL_MONIKER,
                provider_b_moniker: B_ONECOLL_MONIKER,
            },
            TestType::TwoCollections => Self {
                url: BRANCH_TWOCOLL_COMPONENT_URL,
                provider_a_moniker: A_TWOCOLL_MONIKER,
                provider_b_moniker: B_TWOCOLL_MONIKER,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TestType {
    OneCollection,
    TwoCollections,
}

#[test_case(TestType::OneCollection)]
#[test_case(TestType::TwoCollections)]
#[fuchsia::test]
async fn list_instances_test(test_type: TestType) {
    let event_stream = EventStream::open_at_path("/events/started")
        .await
        .context("failed to subscribe to EventSource")
        .unwrap();
    let event_stream_2 = EventStream::open_at_path("/events/started")
        .await
        .context("failed to subscribe to EventSource")
        .unwrap();
    let input = TestInput::new(test_type);
    let branch = start_branch(&input).await.expect("failed to start branch component");
    start_provider(event_stream, &branch, input.provider_a_moniker)
        .await
        .expect("failed to start provider a");
    start_provider(event_stream_2, &branch, input.provider_b_moniker)
        .await
        .expect("failed to start provider b");

    // List the instances in the BankAccount service.
    let service_dir = fuchsia_fs::directory::open_directory(
        branch.get_exposed_dir(),
        fexamples::BankAccountMarker::SERVICE_NAME,
        fio::PERM_READABLE,
    )
    .await
    .expect("failed to open service dir");

    let instances: Vec<String> = fuchsia_fs::directory::readdir(&service_dir)
        .await
        .expect("failed to read entries from service dir")
        .into_iter()
        .map(|dirent| dirent.name)
        .collect();
    verify_instances(instances, 2);
}

#[test_case(TestType::OneCollection)]
#[test_case(TestType::TwoCollections)]
#[fuchsia::test]
async fn connect_to_instances_test(test_type: TestType) {
    let event_stream = EventStream::open_at_path("/events/started")
        .await
        .context("failed to subscribe to EventSource")
        .unwrap();
    let event_stream_2 = EventStream::open_at_path("/events/started")
        .await
        .context("failed to subscribe to EventSource")
        .unwrap();

    let input = TestInput::new(test_type);
    let branch = start_branch(&input).await.expect("failed to start branch component");
    start_provider(event_stream, &branch, input.provider_a_moniker)
        .await
        .expect("failed to start provider a");
    start_provider(event_stream_2, &branch, input.provider_b_moniker)
        .await
        .expect("failed to start provider b");

    // List the instances in the BankAccount service.
    let service =
        client::Service::open_from_dir(branch.get_exposed_dir(), fexamples::BankAccountMarker)
            .expect("failed to open service");
    let instances = service.enumerate().await.expect("failed to read entries from service dir");

    // Connect to every instance and ensure the protocols are functional.
    for proxy in instances {
        let read_only_account = proxy.connect_to_read_only().expect("read_only protocol");
        let owner = read_only_account.get_owner().await.expect("failed to get owner");
        let initial_balance = read_only_account.get_balance().await.expect("failed to get_balance");
        info!("retrieved account for owner '{}' with balance ${}", &owner, &initial_balance);

        let read_write_account = proxy.connect_to_read_write().expect("read_write protocol");
        assert_eq!(read_write_account.get_owner().await.expect("failed to get_owner"), owner);
        assert_eq!(
            read_write_account.get_balance().await.expect("failed to get_balance"),
            initial_balance
        );
    }
}

#[test_case(TestType::OneCollection)]
#[test_case(TestType::TwoCollections)]
#[fuchsia::test]
async fn create_destroy_instance_test(test_type: TestType) {
    let event_stream = EventStream::open_at_path("/events/started")
        .await
        .context("failed to subscribe to EventSource")
        .unwrap();
    let event_stream_2 = EventStream::open_at_path("/events/started")
        .await
        .context("failed to subscribe to EventSource")
        .unwrap();

    let input = TestInput::new(test_type);
    let branch = start_branch(&input).await.expect("failed to start branch component");
    start_provider(event_stream, &branch, input.provider_a_moniker)
        .await
        .expect("failed to start provider a");
    start_provider(event_stream_2, &branch, input.provider_b_moniker)
        .await
        .expect("failed to start provider b");

    // List the instances in the BankAccount service.
    let service_dir = fuchsia_fs::directory::open_directory(
        branch.get_exposed_dir(),
        fexamples::BankAccountMarker::SERVICE_NAME,
        fio::PERM_READABLE,
    )
    .await
    .expect("failed to open service dir");

    let instances: Vec<String> = fuchsia_fs::directory::readdir(&service_dir)
        .await
        .expect("failed to read entries from service dir")
        .into_iter()
        .map(|dirent| dirent.name)
        .collect();

    // The aggregated service directory should contain instances from the provider.
    verify_instances(instances, 2);

    // Destroy provider a.
    destroy_provider(&branch, input.provider_a_moniker)
        .await
        .expect("failed to destroy provider a");

    let instances: Vec<String> = fuchsia_fs::directory::readdir(&service_dir)
        .await
        .expect("failed to read entries from service dir")
        .into_iter()
        .map(|dirent| dirent.name)
        .collect();

    // The provider's instances should be removed from the aggregated service directory.
    verify_instances(instances, 1);
}

#[fuchsia::test]
async fn static_aggregate_offer() {
    let builder = RealmBuilder::new().await.unwrap();
    // Add subrealm to test offer from parent.
    let parent_echo = builder.add_child("echo", ECHO_URL, ChildOptions::new()).await.unwrap();
    let subrealm = builder.add_child_realm("realm", ChildOptions::new().eager()).await.unwrap();
    // Initialize subrealm from echo component to test "offer from self"
    let echo_decl = builder.get_component_decl(&parent_echo).await.unwrap();
    subrealm.replace_realm_decl(echo_decl).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&parent_echo)
                .to(&subrealm),
        )
        .await
        .unwrap();
    let echo = subrealm.add_child("echo", ECHO_URL, ChildOptions::new()).await.unwrap();
    let (handles_sender, mut handles_receiver) = mpsc::unbounded();
    let consumer = subrealm
        .add_local_child(
            "consumer",
            move |handles| {
                let mut handles_sender = handles_sender.clone();
                async move {
                    handles_sender.send(handles).await.unwrap();
                    // Keep the component running so that component_manager keeps the namespace alive
                    futures::pending!();
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    subrealm
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&echo)
                .to(&consumer),
        )
        .await
        .unwrap();
    subrealm
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(Ref::parent())
                .to(&consumer),
        )
        .await
        .unwrap();
    subrealm
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(Ref::self_())
                .to(&consumer),
        )
        .await
        .unwrap();
    let _realm = builder.build().await.unwrap();

    let handles = handles_receiver.next().await.unwrap();
    let echo_svc = handles.open_service::<fecho::EchoServiceMarker>().unwrap();
    let instances = fuchsia_fs::directory::readdir(&echo_svc)
        .await
        .unwrap()
        .into_iter()
        .map(|dirent| dirent.name);
    // parent, self, child in the aggregate x 3 instances each
    assert_eq!(instances.len(), 3 * 3);
    drop(echo_svc);

    // Connect to every instance and ensure the protocols are functional.
    for instance in instances {
        let proxy =
            handles.connect_to_service_instance::<fecho::EchoServiceMarker>(&instance).unwrap();

        let echo_proxy = proxy.connect_to_regular_echo().unwrap();
        let res = echo_proxy.echo_string("hello").await.unwrap();
        assert!(res.ends_with("hello"));

        let echo_proxy = proxy.connect_to_reversed_echo().unwrap();
        let res = echo_proxy.echo_string("hello").await.unwrap();
        assert!(res.ends_with("olleh"));
    }
}

#[fuchsia::test]
async fn static_aggregate_expose() {
    let builder = RealmBuilder::new().await.unwrap();
    // Add placeholder echo component so we can get its decl. Then initialize a subrealm with this
    // decl to test "expose from self".
    let placeholder_echo =
        builder.add_child("placeholder_echo", ECHO_URL, ChildOptions::new()).await.unwrap();
    let subrealm = builder.add_child_realm("realm", ChildOptions::new()).await.unwrap();
    let echo_decl = builder.get_component_decl(&placeholder_echo).await.unwrap();
    subrealm.replace_realm_decl(echo_decl).await.unwrap();
    let echo = subrealm.add_child("echo", ECHO_URL, ChildOptions::new()).await.unwrap();
    subrealm
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&echo)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    subrealm
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(Ref::self_())
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&subrealm)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let realm = builder.build().await.unwrap();

    let exposed_dir = realm.root.get_exposed_dir();
    let instances = client::Service::open_from_dir(exposed_dir, fecho::EchoServiceMarker)
        .unwrap()
        .enumerate()
        .await
        .unwrap();
    // self, child in the aggregate x 3 instances each
    assert_eq!(instances.len(), 3 * 2);

    // Connect to every instance and ensure the protocols are functional.
    for proxy in instances {
        let echo_proxy = proxy.connect_to_regular_echo().unwrap();
        let res = echo_proxy.echo_string("hello").await.unwrap();
        assert!(res.ends_with("hello"));

        let echo_proxy = proxy.connect_to_reversed_echo().unwrap();
        let res = echo_proxy.echo_string("hello").await.unwrap();
        assert!(res.ends_with("olleh"));
    }
}

/// Starts a branch child component.
async fn start_branch(input: &TestInput) -> Result<ScopedInstance, Error> {
    let branch = ScopedInstance::new(BRANCHES_COLLECTION.to_string(), input.url.to_string())
        .await
        .context("failed to create branch component instance")?;
    let trigger: ftest::TriggerProxy =
        branch.connect_to_protocol_at_exposed_dir().context("failed to connect to trigger")?;
    let _ = trigger.run().await.context("failed to call trigger")?;
    Ok(branch)
}

/// Starts the provider with the name `child_name` in the branch component.
async fn start_provider(
    event_stream: EventStream,
    branch: &ScopedInstance,
    child_moniker: &str,
) -> Result<(), Error> {
    let lifecycle_controller_proxy =
        client::connect_to_protocol::<fsys2::LifecycleControllerMarker>()
            .context("failed to connect to LifecycleController")?;

    let provider_moniker =
        format!("./{}:{}/{}", BRANCHES_COLLECTION, branch.child_name(), child_moniker);

    let (_, binder_server) = fidl::endpoints::create_endpoints();

    // Start the provider child.
    lifecycle_controller_proxy
        .start_instance(&provider_moniker, binder_server)
        .await?
        .map_err(|err| format_err!("failed to start provider component: {:?}", err))?;

    // Wait for the provider to start.
    EventSequence::new()
        .has_subset(
            vec![EventMatcher::ok().r#type(Started::TYPE).moniker(provider_moniker)],
            Ordering::Unordered,
        )
        .expect(event_stream)
        .await
        .context("event sequence did not match expected")?;

    Ok(())
}

/// Destroys a BankAccount provider component with the name `child_name`.
async fn destroy_provider(branch: &ScopedInstance, child_moniker: &str) -> Result<(), Error> {
    info!(child_moniker:%; "destroying BankAccount provider");

    let lifecycle_controller_proxy =
        client::connect_to_protocol::<fsys2::LifecycleControllerMarker>()
            .context("failed to connect to LifecycleController")?;

    let event_stream = EventStream::open_at_path("/events/destroyed")
        .await
        .context("failed to subscribe to EventSource")?;

    let provider_moniker =
        format!("./{}:{}/{}", BRANCHES_COLLECTION, branch.child_name(), child_moniker);
    let parent_moniker = format!("./{}:{}", BRANCHES_COLLECTION, branch.child_name());

    // Destroy the provider child.
    let child_moniker = ChildName::parse(child_moniker).unwrap();
    lifecycle_controller_proxy
        .destroy_instance(
            &parent_moniker,
            &fdecl::ChildRef {
                name: child_moniker.name().to_string(),
                collection: child_moniker.collection().map(|c| c.to_string()),
            },
        )
        .await?
        .map_err(|err| format_err!("failed to destroy provider component: {:?}", err))?;

    // Wait for the provider to be destroyed.
    EventSequence::new()
        .has_subset(
            vec![EventMatcher::ok().r#type(Destroyed::TYPE).moniker(provider_moniker)],
            Ordering::Unordered,
        )
        .expect(event_stream)
        .await
        .context("event sequence did not match expected")?;

    Ok(())
}

fn verify_instances(instances: Vec<String>, expected_len: usize) {
    assert_eq!(instances.len(), expected_len);
    assert!(instances.iter().all(|id| id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())));
}

#[fuchsia::test]
async fn use_from_collection() {
    let builder = RealmBuilder::new().await.unwrap();
    let (service_directory_sender, mut service_directory_receiver) = mpsc::unbounded();
    let (service_access_blocker_sender, service_access_blocker_receiver) = mpsc::unbounded();
    let service_access_blocker_receiver =
        Arc::new(Mutex::new(Some(service_access_blocker_receiver)));
    let service_accessing_child = builder
        .add_local_child(
            "service_accessing_child",
            move |h| {
                let service_directory_sender = service_directory_sender.clone();
                let service_access_blocker_receiver = service_access_blocker_receiver.clone();
                async move {
                    let mut service_access_blocker_receiver =
                        service_access_blocker_receiver.lock().await.take().unwrap();
                    let () = service_access_blocker_receiver.next().await.unwrap();
                    let service_directory = h.open_service::<fecho::EchoServiceMarker>().unwrap();
                    service_directory_sender.unbounded_send(service_directory).unwrap();
                    futures::future::pending().await
                }
                .boxed()
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();
    let service_accessing_child_decl =
        builder.get_component_decl(&service_accessing_child).await.unwrap();
    builder.replace_realm_decl(service_accessing_child_decl).await.unwrap();
    let collection = builder
        .add_collection(cm_rust::CollectionDecl {
            name: "col".parse().unwrap(),
            durability: fdecl::Durability::Transient,
            environment: None,
            allowed_offers: cm_types::AllowedOffers::StaticOnly,
            allow_long_names: false,
            persistent_storage: None,
        })
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&collection)
                .to(Ref::self_()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fcomponent::RealmMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let realm_proxy: fcomponent::RealmProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let publishing_child_builder = RealmBuilder::new().await.unwrap();
    let publishing_child = publishing_child_builder
        .add_local_child(
            "publishing_child",
            move |h| publishing_component_impl(h).boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    publishing_child_builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&publishing_child)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let (url, _publishing_child_task) = publishing_child_builder.initialize().await.unwrap();

    realm_proxy
        .create_child(
            &fdecl::CollectionRef { name: "col".to_string() },
            &fdecl::Child {
                name: Some("publishing_child".to_string()),
                url: Some(url),
                startup: Some(fdecl::StartupMode::Lazy),
                ..Default::default()
            },
            Default::default(),
        )
        .await
        .unwrap()
        .unwrap();
    service_access_blocker_sender.unbounded_send(()).unwrap();

    let service_directory = service_directory_receiver.next().await.unwrap();
    let dir_entries = fuchsia_fs::directory::readdir(&service_directory).await.unwrap();
    assert_eq!(2, dir_entries.len());
    let mut echo_results = HashSet::new();
    for dir_entry in dir_entries {
        let instance_dir = fuchsia_fs::directory::open_directory(
            &service_directory,
            &dir_entry.name,
            fio::Flags::empty(),
        )
        .await
        .expect("failed to open instance dir");
        let echo_service_proxy = fecho::EchoServiceProxy::from_member_opener(Box::new(
            client::ServiceInstanceDirectory(instance_dir, dir_entry.name.clone()),
        ));
        let echo_proxy = echo_service_proxy.connect_to_regular_echo().unwrap();
        let echoed_string = echo_proxy.echo_string("Hello, world!").await.unwrap();
        echo_results.insert(echoed_string);
    }
    assert_eq!(
        HashSet::from(["Hello, world!".to_string(), "Greetings and Hello, world!".to_string()]),
        echo_results
    );
}

/// If a component contributes to a service aggregate and does not publish instances, the instances
/// published by other instances must still be reachable.
///
/// Sets up three statically declared components inside of a nested component manager:
/// - publishing_child: this component publishes two service instances.
/// - non_publishing_child: this component does not publish any service instances.
/// - service_accessing_child: services from the other two children are offered to this component
///
/// After constructing this setup, we assert that the service instances from publishing_child are
/// visible and can be connected to.
#[fuchsia::test]
#[ignore]
async fn not_every_static_component_publishes_service() {
    let builder = RealmBuilder::new().await.unwrap();
    let publishing_child = builder
        .add_local_child(
            "publishing_child",
            move |h| publishing_component_impl(h).boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    let non_publishing_child = builder
        .add_local_child(
            "non_publishing_child",
            move |h| non_publishing_component_impl(h).boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    let (service_directory_sender, mut service_directory_receiver) = mpsc::unbounded();
    let service_accessing_child = builder
        .add_local_child(
            "service_accessing_child",
            move |h| {
                let service_directory_sender = service_directory_sender.clone();
                async move {
                    let service_directory = h.open_service::<fecho::EchoServiceMarker>().unwrap();
                    service_directory_sender.unbounded_send(service_directory).unwrap();
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&publishing_child)
                .to(&service_accessing_child),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&non_publishing_child)
                .to(&service_accessing_child),
        )
        .await
        .unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let service_directory = service_directory_receiver.next().await.unwrap();
    let dir_entries = fuchsia_fs::directory::readdir(&service_directory).await.unwrap();
    assert_eq!(2, dir_entries.len());
    let mut echo_results = HashSet::new();
    for dir_entry in dir_entries {
        let instance_dir = fuchsia_fs::directory::open_directory(
            &service_directory,
            &dir_entry.name,
            fio::Flags::empty(),
        )
        .await
        .expect("failed to open instance dir");
        let echo_service_proxy = fecho::EchoServiceProxy::from_member_opener(Box::new(
            client::ServiceInstanceDirectory(instance_dir, dir_entry.name.clone()),
        ));
        let echo_proxy = echo_service_proxy.connect_to_regular_echo().unwrap();
        let echoed_string = echo_proxy.echo_string("Hello, world!").await.unwrap();
        echo_results.insert(echoed_string);
    }
    assert_eq!(
        HashSet::from(["Hello, world!".to_string(), "Greetings and Hello, world!".to_string()]),
        echo_results
    );
}

/// If a component contributes to a service aggregate and does not publish instances, the instances
/// published by other instances must still be reachable.
///
/// Sets up three components inside of a nested component manager, one statically declared and two
/// dynamic:
/// - publishing_child: this dynamic component is created in collection `col` and publishes two
///   service instances.
/// - non_publishing_child: this dynamic component is created in collection `col` and does not
///   publish any service instances.
/// - service_accessing_child: services from collection `col` are offered to this static component.
///
/// After constructing this setup, we assert that the service instances from publishing_child are
/// visible and can be connected to.
#[fuchsia::test]
async fn not_every_dynamic_component_publishes_service() {
    let builder = RealmBuilder::new().await.unwrap();
    let collection = builder
        .add_collection(cm_rust::CollectionDecl {
            name: "col".parse().unwrap(),
            durability: fdecl::Durability::Transient,
            environment: None,
            allowed_offers: cm_types::AllowedOffers::StaticOnly,
            allow_long_names: false,
            persistent_storage: None,
        })
        .await
        .unwrap();
    let (service_directory_sender, mut service_directory_receiver) = mpsc::unbounded();
    let service_accessing_child = builder
        .add_local_child(
            "service_accessing_child",
            move |h| {
                let service_directory_sender = service_directory_sender.clone();
                async move {
                    let service_directory = h.open_service::<fecho::EchoServiceMarker>().unwrap();
                    service_directory_sender.unbounded_send(service_directory).unwrap();
                    futures::future::pending().await
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
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&collection)
                .to(&service_accessing_child),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fcomponent::RealmMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let realm_proxy: fcomponent::RealmProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let publishing_child_builder = RealmBuilder::new().await.unwrap();
    let publishing_child = publishing_child_builder
        .add_local_child(
            "publishing_child",
            move |h| publishing_component_impl(h).boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    publishing_child_builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&publishing_child)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let (url, _publishing_child_task) = publishing_child_builder.initialize().await.unwrap();

    realm_proxy
        .create_child(
            &fdecl::CollectionRef { name: "col".to_string() },
            &fdecl::Child {
                name: Some("publishing_child".to_string()),
                url: Some(url),
                startup: Some(fdecl::StartupMode::Lazy),
                ..Default::default()
            },
            Default::default(),
        )
        .await
        .unwrap()
        .unwrap();

    let non_publishing_child_builder = RealmBuilder::new().await.unwrap();
    let non_publishing_child = non_publishing_child_builder
        .add_local_child(
            "non_publishing_child",
            move |h| non_publishing_component_impl(h).boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    non_publishing_child_builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&non_publishing_child)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let (url, _non_publishing_child_task) =
        non_publishing_child_builder.initialize().await.unwrap();

    realm_proxy
        .create_child(
            &fdecl::CollectionRef { name: "col".to_string() },
            &fdecl::Child {
                name: Some("non_publishing_child".to_string()),
                url: Some(url),
                startup: Some(fdecl::StartupMode::Lazy),
                ..Default::default()
            },
            Default::default(),
        )
        .await
        .unwrap()
        .unwrap();

    let (controller_proxy, server_end) = create_proxy::<fcomponent::ControllerMarker>();
    realm_proxy
        .open_controller(
            &fdecl::ChildRef { name: "service_accessing_child".to_string(), collection: None },
            server_end,
        )
        .await
        .unwrap()
        .unwrap();
    let (_exec_proxy, server_end) = create_proxy::<fcomponent::ExecutionControllerMarker>();
    controller_proxy.start(Default::default(), server_end).await.unwrap().unwrap();

    let service_directory = service_directory_receiver.next().await.unwrap();
    let dir_entries = fuchsia_fs::directory::readdir(&service_directory).await.unwrap();
    assert_eq!(2, dir_entries.len());
    let mut echo_results = HashSet::new();
    for dir_entry in dir_entries {
        let instance_dir = fuchsia_fs::directory::open_directory(
            &service_directory,
            &dir_entry.name,
            fio::Flags::empty(),
        )
        .await
        .expect("failed to open instance dir");
        let echo_service_proxy = fecho::EchoServiceProxy::from_member_opener(Box::new(
            client::ServiceInstanceDirectory(instance_dir, dir_entry.name.clone()),
        ));
        let echo_proxy = echo_service_proxy.connect_to_regular_echo().unwrap();
        let echoed_string = echo_proxy.echo_string("Hello, world!").await.unwrap();
        echo_results.insert(echoed_string);
    }
    assert_eq!(
        HashSet::from(["Hello, world!".to_string(), "Greetings and Hello, world!".to_string()]),
        echo_results
    );
}

/// If a component is contributing to an anonymizing aggregate, any service instances it publishes
/// well after the service capability is initially routed should appear in the routed directory.
#[fuchsia::test]
#[ignore]
async fn component_adds_service_entries_late() {
    let builder = RealmBuilder::new().await.unwrap();
    let (backing_directory_sender, mut backing_directory_receiver) = mpsc::unbounded();
    let publishing_child = builder
        .add_local_child(
            "publishing_child",
            move |h| {
                let backing_directory_sender = backing_directory_sender.clone();
                async move {
                    let outdir = Simple::new();
                    backing_directory_sender.unbounded_send(outdir.clone()).unwrap();
                    let scope = ExecutionScope::new();
                    vfs::directory::serve_on(
                        outdir,
                        fio::PERM_READABLE,
                        scope.clone(),
                        h.outgoing_dir,
                    );
                    scope.wait().await;
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    let non_publishing_child = builder
        .add_local_child(
            "non_publishing_child",
            move |h| non_publishing_component_impl(h).boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    let (service_directory_sender, mut service_directory_receiver) = mpsc::unbounded();
    let service_accessing_child = builder
        .add_local_child(
            "service_accessing_child",
            move |h| {
                let service_directory_sender = service_directory_sender.clone();
                async move {
                    let service_directory = h.open_service::<fecho::EchoServiceMarker>().unwrap();
                    service_directory_sender.unbounded_send(service_directory).unwrap();
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&publishing_child)
                .to(&service_accessing_child),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fecho::EchoServiceMarker>())
                .from(&non_publishing_child)
                .to(&service_accessing_child),
        )
        .await
        .unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    let service_directory = service_directory_receiver.next().await.unwrap();

    let backing_directory = backing_directory_receiver.next().await.unwrap();
    let backing_service_instance_directory = Simple::new();
    backing_directory
        .add_entry(
            "svc",
            pseudo_directory! {
                "fuchsia.examples.EchoService" => backing_service_instance_directory.clone()
            },
        )
        .unwrap();

    let mut watcher = Watcher::new(&service_directory).await.unwrap();

    assert_eq!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename: ".".into() }))
    );
    assert_eq!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::IDLE, filename: "".into() }))
    );

    let dir_entries = fuchsia_fs::directory::readdir(&service_directory).await.unwrap();
    assert_eq!(0, dir_entries.len());

    backing_service_instance_directory.add_entry("default", pseudo_directory! {}).unwrap();

    let first_watch_event = watcher.next().await.unwrap().unwrap();
    assert_eq!(first_watch_event.event, WatchEvent::ADD_FILE);

    backing_service_instance_directory.add_entry("foo", pseudo_directory! {}).unwrap();

    let second_watch_event = watcher.next().await.unwrap().unwrap();
    assert_eq!(
        second_watch_event.event,
        WatchEvent::ADD_FILE,
        "watch_event: {:?}",
        second_watch_event
    );
}

async fn publishing_component_impl(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = fuchsia_component::server::ServiceFs::new();
    fs.dir("svc").add_fidl_service_instance("default", IncomingService::Default);
    fs.dir("svc").add_fidl_service_instance("hello", IncomingService::Hello);
    fs.serve_connection(handles.outgoing_dir).unwrap();
    fs.for_each_concurrent(None, |request| {
        match request {
            IncomingService::Default(fecho::EchoServiceRequest::RegularEcho(stream)) => {
                run_echo_server(stream, "".to_string(), false)
            }
            IncomingService::Default(fecho::EchoServiceRequest::ReversedEcho(stream)) => {
                run_echo_server(stream, "".to_string(), true)
            }
            IncomingService::Hello(fecho::EchoServiceRequest::RegularEcho(stream)) => {
                run_echo_server(stream, "Greetings and ".to_string(), false)
            }
            IncomingService::Hello(fecho::EchoServiceRequest::ReversedEcho(stream)) => {
                run_echo_server(stream, "Greetings and ".to_string(), true)
            }
        }
        .unwrap_or_else(|e| {
            info!("Error serving multi instance echo service {:?}", e);
            error!("{:?}", e)
        })
    })
    .await;
    Ok(())
}

async fn non_publishing_component_impl(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = fuchsia_component::server::ServiceFs::new();
    fs.dir("svc");
    fs.serve_connection(handles.outgoing_dir).unwrap();
    fs.collect::<()>().await;
    Ok(())
}

enum IncomingService {
    Default(fecho::EchoServiceRequest),
    Hello(fecho::EchoServiceRequest),
}
async fn run_echo_server(
    mut stream: fecho::EchoRequestStream,
    prefix: String,
    reverse: bool,
) -> Result<(), Error> {
    while let Some(fecho::EchoRequest::EchoString { value, responder }) =
        stream.try_next().await.context("error running echo server")?
    {
        println!("Received EchoString request for string {:?}", value);
        let echo_string = if reverse { value.chars().rev().collect() } else { value.clone() };
        let resp = vec![prefix.clone(), echo_string].join("");
        responder.send(&resp).context("error sending response")?;
        println!("Response sent successfully");
    }
    Ok(())
}
