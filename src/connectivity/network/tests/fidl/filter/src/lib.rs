// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

mod actions;
mod ip_hooks;
mod matchers;
mod nat;
mod root;

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU16;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_matches::assert_matches;
use fidl::endpoints::Proxy as _;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext::{
    self as fnet_filter_ext, Action, Change, ChangeCommitError, CommitError, Controller,
    ControllerId, Domain, Event, InstalledIpRoutine, InstalledNatRoutine, IpHook, Matchers,
    Namespace, NamespaceId, NatHook, PortRange, PushChangesError, RegisterEbpfProgramError,
    Resource, ResourceId, Routine, RoutineId, RoutineType, Rule, RuleId, TransparentProxy,
};
use fidl_fuchsia_net_interfaces_ext::PortClass;
use fidl_fuchsia_net_matchers as fnet_matchers;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_matchers_ext::TransportProtocol;
use fuchsia_async::{DurationExt as _, OnSignals, TimeoutExt as _};
use futures::{FutureExt as _, StreamExt as _, TryFutureExt as _};
use itertools::Itertools as _;
use net_declare::{fidl_ip, fidl_subnet};
use net_types::ip::IpVersion;
use netemul::{RealmTcpListener as _, RealmTcpStream as _};
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
};
use netstack_testing_macros::netstack_test;
use test_case::{test_case, test_matrix};

trait TestValue {
    fn test_value() -> Self;
}

impl TestValue for ResourceId {
    fn test_value() -> Self {
        Self::Namespace(NamespaceId::test_value())
    }
}

impl TestValue for Resource {
    fn test_value() -> Self {
        Self::Namespace(Namespace::test_value())
    }
}

impl TestValue for NamespaceId {
    fn test_value() -> Self {
        Self("NAMESPACE_ID".to_owned())
    }
}

impl TestValue for Namespace {
    fn test_value() -> Self {
        Self { id: NamespaceId::test_value(), domain: Domain::AllIp }
    }
}

impl TestValue for RoutineId {
    fn test_value() -> Self {
        Self { namespace: NamespaceId::test_value(), name: String::from("ingress") }
    }
}

impl TestValue for Routine {
    fn test_value() -> Self {
        Self {
            id: RoutineId::test_value(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }
    }
}

impl TestValue for RuleId {
    fn test_value() -> Self {
        Self { routine: RoutineId::test_value(), index: 0 }
    }
}

impl TestValue for Rule {
    fn test_value() -> Self {
        Rule {
            id: RuleId { routine: RoutineId::test_value(), index: 0 },
            matchers: Matchers {
                transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                    src_port: None,
                    dst_port: Some(
                        fnet_matchers_ext::Port::new(22, 22, /* invert */ false)
                            .expect("valid port range"),
                    ),
                }),
                ..Default::default()
            },
            action: Action::Drop,
        }
    }
}

#[netstack_test]
async fn watcher_existing(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");

    // There should be no resources on startup.
    {
        let stream = fnet_filter_ext::event_stream_from_state(state.clone())
            .expect("get filter event stream");
        let mut stream = pin!(stream);
        let observed: HashMap<_, _> =
            fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
        assert_eq!(observed, HashMap::new());
    }

    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    let observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
    assert_eq!(
        observed,
        HashMap::from([(
            controller.id().clone(),
            resources
                .into_iter()
                .map(|resource| (resource.id(), resource))
                .collect::<HashMap<_, _>>(),
        )])
    );
}

#[netstack_test]
async fn watcher_observe_updates(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    assert_eq!(stream.next().await.expect("wait for idle").expect("wait for idle"), Event::Idle);

    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");

    assert_matches!(
        stream.next().on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || None).await,
        None,
        "changes should not be broadcast until committed"
    );

    controller.commit().await.expect("commit pending changes");
    for resource in &resources {
        let (controller_id, added_resource) = assert_matches!(
            stream.next().await,
            Some(Ok(Event::Added(id, resource))) => (id, resource),
            "added resources should be broadcast to watcher"
        );
        assert_eq!(&controller_id, controller.id());
        assert_eq!(&added_resource, resource);
    }
    assert_matches!(
        stream.next().await,
        Some(Ok(Event::EndOfUpdate)),
        "transactional updates should be demarcated with EndOfUpdate event"
    );

    // Removing a containing resource (e.g. a namespace) also removes all its
    // contents.
    let to_remove = Resource::Namespace(Namespace::test_value());
    controller.push_changes(vec![Change::Remove(to_remove.id())]).await.expect("push changes");
    controller.commit().await.expect("commit pending changes");
    let mut expected = resources.into_iter().map(|resource| resource.id()).collect::<HashSet<_>>();
    while !expected.is_empty() {
        let (controller_id, removed_resource) = assert_matches!(
            stream.next().await,
            Some(Ok(Event::Removed(id, resource))) => (id, resource),
            "removed resources should be broadcast to watcher"
        );
        assert_eq!(&controller_id, controller.id());
        assert_eq!(expected.remove(&removed_resource), true);
    }
    assert_matches!(
        stream.next().await,
        Some(Ok(Event::EndOfUpdate)),
        "transactional updates should be demarcated with EndOfUpdate event"
    );
}

#[netstack_test]
async fn resources_and_events_scoped_to_controllers(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    assert_eq!(stream.next().await.expect("wait for idle").expect("wait for idle"), Event::Idle);

    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let create_controller_and_commit_updates = |name: &'static str| async {
        let mut controller = Controller::new(&control, &ControllerId(name.to_owned()))
            .await
            .expect("create controller");
        controller
            .push_changes(vec![Change::Create(Resource::test_value())])
            .await
            .expect("push changes");
        controller.commit().await.expect("commit pending changes");
        controller
    };

    // Add two identical resources under different controllers. Note that we
    // retain the controllers as dropping them would cause their resources to be
    // removed.
    let _controllers = futures::future::join_all([
        create_controller_and_commit_updates("controller-a"),
        create_controller_and_commit_updates("controller-b"),
    ])
    .await;

    let mut expected_controllers = HashSet::from(["controller-a", "controller-b"]);
    while !expected_controllers.is_empty() {
        let (ControllerId(id), added_resource) = assert_matches!(
            stream.next().await,
            Some(Ok(Event::Added(id, resource))) => (id, resource),
            "added resources should be broadcast to watcher"
        );
        assert!(expected_controllers.remove(id.as_str()));
        assert_eq!(added_resource, Resource::test_value());
        assert_matches!(
            stream.next().await,
            Some(Ok(Event::EndOfUpdate)),
            "transactional updates should be demarcated with EndOfUpdate event"
        );
    }
}

#[netstack_test]
async fn watcher_already_pending(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let (watcher, server_end) = fidl::endpoints::create_proxy::<fnet_filter::WatcherMarker>();
    state.get_watcher(&fnet_filter::WatcherOptions::default(), server_end).expect("get watcher");

    let events = watcher.watch().await.expect("get existing resources");
    assert_eq!(events, &[fnet_filter::Event::Idle(fnet_filter::Empty {})]);

    // Call `Watch` twice and the netstack should close the channel.
    assert_matches!(
        futures::future::join(watcher.watch(), watcher.watch()).await,
        (
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. }),
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. }),
        )
    );
    assert!(watcher.is_closed());
}

#[netstack_test]
async fn watcher_channel_closed_if_not_polled(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let (watcher, server_end) = fidl::endpoints::create_proxy::<fnet_filter::WatcherMarker>();
    state.get_watcher(&fnet_filter::WatcherOptions::default(), server_end).expect("get watcher");

    let events = watcher.watch().await.expect("get existing resources");
    assert_eq!(events, &[fnet_filter::Event::Idle(fnet_filter::Empty {})]);

    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller = Controller::new(&control, &ControllerId(String::from("test")))
        .await
        .expect("create controller");

    async fn create_and_remove_namespace(controller: &mut Controller) {
        controller
            .push_changes(vec![Change::Create(Resource::test_value())])
            .await
            .expect("push changes");
        controller.commit().await.expect("commit pending changes");

        controller
            .push_changes(vec![Change::Remove(ResourceId::test_value())])
            .await
            .expect("push changes");
        controller.commit().await.expect("commit pending changes");
    }

    // Repeatedly add and remove resources, causing events to be queued
    // server-side for the watcher.
    let perform_updates = async {
        loop {
            create_and_remove_namespace(&mut controller).await
        }
    }
    .fuse();
    let mut perform_updates = pin!(perform_updates);

    // Wait for the watcher channel to be closed as a result.
    let mut event_stream = watcher.take_event_stream();
    futures::select! {
        event = event_stream.next() => assert_matches!(event, None),
        _ = perform_updates => unreachable!(),
    }
}

#[netstack_test]
async fn on_id_assigned(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    let controller_id = ControllerId(String::from("test"));
    let open_new_controller =
        || async { Controller::new(&control, &controller_id).await.expect("create controller") };

    let mut controller = open_new_controller().await;
    assert_eq!(controller.id(), &controller_id);

    // The netstack should deduplicate IDs if there are conflicts.
    let other_controller = open_new_controller().await;
    assert_ne!(other_controller.id(), &controller_id);

    // Add a resource with the first controller and initialize a watcher so that
    // we'll be able to observe its removal.
    let resource = Resource::test_value();
    controller.push_changes(vec![Change::Create(resource.clone())]).await.expect("push changes");
    controller.commit().await.expect("commit pending changes");
    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    let mut observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
    assert_eq!(
        observed,
        HashMap::from([(
            controller.id().clone(),
            HashMap::from([(resource.id(), resource.clone())])
        )])
    );

    // If the first controller is closed, its ID can be reused.
    //
    // NB: to avoid a race between the server-side handling of the channel
    // closure and opening a new controller with the same ID, we wait to observe
    // removal of the controller's resources.
    drop(controller);
    fnet_filter_ext::wait_for_condition(&mut stream, &mut observed, |state| {
        state.get(&controller_id).unwrap().is_empty()
    })
    .await
    .expect("controller's resources should be removed on drop");

    let controller = open_new_controller().await;
    assert_eq!(controller.id(), &controller_id);
}

#[netstack_test]
async fn drop_controller_removes_resources(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let controller_id = ControllerId(String::from("test"));
    let mut controller =
        Controller::new(&control, &controller_id).await.expect("create controller");

    // Create some resources with the controller.
    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);

    // Observe existing resources and ensure we see what was added.
    let observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get existing");
    assert_eq!(
        observed,
        HashMap::from([(
            controller.id().clone(),
            resources
                .iter()
                .cloned()
                .map(|resource| (resource.id(), resource))
                .collect::<HashMap<_, _>>(),
        )])
    );

    // Set up a listening socket and try to connect to it so we can verify that the
    // filtering resources *actually* got dropped in Core, and not just in Bindings.
    //
    // NB: the default test values above create a configuration that drops incoming
    // TCP traffic to port 22.
    let listen_addr =
        std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 22);
    let listener = fuchsia_async::net::TcpListener::listen_in_realm(&realm, listen_addr)
        .await
        .expect("listen on port 22");

    let mut server_fut = Box::pin(async {
        let (_, _stream, _from) = listener.accept().await.expect("accept incoming connection");
    })
    .fuse();
    let client_fut = async {
        match fuchsia_async::net::TcpStream::connect_in_realm(&realm, listen_addr)
            .map_ok(Some)
            .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || Ok(None))
            .await
            .expect("call connect")
        {
            Some(_stream) => panic!("unexpectedly connected successfully"),
            None => {}
        }
    };
    futures::select! {
        () = client_fut.fuse() => {},
        () = server_fut => panic!("listener should not get incoming connection"),
    }

    // Drop the controller and ensure that the resources it owned are removed.
    drop(controller);

    let mut resources =
        resources.into_iter().map(|resource| (resource.id(), resource)).collect::<HashMap<_, _>>();
    while !resources.is_empty() {
        let (id, resource) = assert_matches!(
            stream.next().await,
            Some(Ok(Event::Removed(id, resource))) => (id, resource),
            "resource lifetime should be scoped to controller handle"
        );
        assert_eq!(id, controller_id);
        assert_matches!(resources.remove(&resource), Some(_));
    }
    assert_matches!(
        stream.next().await,
        Some(Ok(Event::EndOfUpdate)),
        "transactional updates should be demarcated with EndOfUpdate event"
    );

    // Now that the controller has been dropped, we should be able to connect to the
    // listening socket.
    let client_fut = async {
        let _stream = fuchsia_async::net::TcpStream::connect_in_realm(&realm, listen_addr)
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
                panic!("timed out waiting to connect to server after dropping filtering rules")
            })
            .await
            .expect("connect to server should succeed");
    };
    let ((), ()) = futures::future::join(client_fut, server_fut).await;
}

#[netstack_test]
async fn push_too_many_changes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    let mut controller = Controller::new(&control, &ControllerId(String::from("test")))
        .await
        .expect("create controller");

    let changes =
        [Change::Create(Resource::test_value()), Change::Remove(ResourceId::test_value())]
            .into_iter()
            .cycle();

    // Commit a change of the maximum size.
    for batch in &changes
        .clone()
        .take(fnet_filter::MAX_COMMIT_SIZE.into())
        .chunks(fnet_filter::MAX_BATCH_SIZE.into())
    {
        controller.push_changes(batch.collect()).await.expect("push changes");
    }
    controller.commit().await.expect("commit changes");

    // Push one more change than `MAX_COMMIT_SIZE` and ensure we get the
    // expected error.
    for batch in &changes
        .clone()
        .take(fnet_filter::MAX_COMMIT_SIZE.into())
        .chunks(fnet_filter::MAX_BATCH_SIZE.into())
    {
        controller.push_changes(batch.collect()).await.expect("push changes");
    }
    assert_matches!(
        controller.push_changes(changes.take(1).collect()).await,
        Err(PushChangesError::TooManyChanges)
    );
    // Committing should still succeed because the final change was not pushed
    // to the server.
    controller.commit().await.expect("commit changes");
}

#[netstack_test]
async fn push_commit_zero_changes_is_valid(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    let mut controller = Controller::new(&control, &ControllerId(String::from("test")))
        .await
        .expect("create controller");

    controller.push_changes(Vec::new()).await.expect("push zero changes");
    controller.commit().await.expect("commit changes");
}

#[netstack_test]
async fn push_change_missing_required_field(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    // Use the FIDL bindings directly rather than going through the extension
    // library, because it intentionally does not allow us to express the
    // invalid types that we are testing.
    let (controller, server_end) = fidl::endpoints::create_proxy();
    control.open_controller("test", server_end).expect("open controller");
    assert_eq!(
        controller
            .push_changes(&[fnet_filter::Change::Create(fnet_filter::Resource::Namespace(
                fnet_filter::Namespace {
                    id: None,
                    domain: Some(fnet_filter::Domain::AllIp),
                    ..Default::default()
                }
            ))])
            .await
            .expect("call push changes"),
        fnet_filter::ChangeValidationResult::ErrorOnChange(vec![
            fnet_filter::ChangeValidationError::MissingRequiredField
        ])
    );
}

#[netstack_test]
async fn push_change_invalid_interface_matcher(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    // Use the FIDL bindings directly rather than going through the extension
    // library, because it intentionally does not allow us to express the
    // invalid types that we are testing.
    let (controller, server_end) = fidl::endpoints::create_proxy();
    control.open_controller("test", server_end).expect("open controller");
    assert_eq!(
        controller
            .push_changes(&[fnet_filter::Change::Create(fnet_filter::Resource::Rule(
                fnet_filter::Rule {
                    id: fnet_filter::RuleId {
                        routine: fnet_filter::RoutineId {
                            namespace: String::from("namespace"),
                            name: String::from("routine"),
                        },
                        index: 0,
                    },
                    matchers: fnet_filter::Matchers {
                        in_interface: Some(fnet_matchers::Interface::Id(0)),
                        ..Default::default()
                    },
                    action: fnet_filter::Action::Drop(fnet_filter::Empty {}),
                }
            ))])
            .await
            .expect("call push changes"),
        fnet_filter::ChangeValidationResult::ErrorOnChange(vec![
            fnet_filter::ChangeValidationError::InvalidInterfaceMatcher
        ])
    );
}

#[netstack_test]
#[test_case(
    fnet_matchers::Address {
        matcher: fnet_matchers::AddressMatcherType::Range(fnet_matchers::AddressRange {
            start: fidl_ip!("192.0.2.1"),
            end: fidl_ip!("2001:db8::1"),
        }),
        invert: false,
    };
    "address family mismatch"
)]
#[test_case(
    fnet_matchers::Address {
        matcher: fnet_matchers::AddressMatcherType::Range(fnet_matchers::AddressRange {
            start: fidl_ip!("192.0.2.2"),
            end: fidl_ip!("192.0.2.1"),
        }),
        invert: false,
    };
    "start > end"
)]
#[test_case(
    fnet_matchers::Address {
        matcher: fnet_matchers::AddressMatcherType::Subnet(fnet::Subnet {
            addr: fidl_ip!("192.0.2.1"),
            prefix_len: 33,
        }),
        invert: false,
    };
    "subnet prefix too long"
)]
#[test_case(
    fnet_matchers::Address {
        matcher: fnet_matchers::AddressMatcherType::Subnet(fidl_subnet!("192.0.2.1/24")),
        invert: false,
    };
    "subnet host bits set"
)]
async fn push_change_invalid_address_matcher(name: &str, matcher: fnet_matchers::Address) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    // Use the FIDL bindings directly rather than going through the extension
    // library, because it intentionally does not allow us to express the
    // invalid types that we are testing.
    let (controller, server_end) = fidl::endpoints::create_proxy();
    control.open_controller("test", server_end).expect("open controller");
    assert_eq!(
        controller
            .push_changes(&[fnet_filter::Change::Create(fnet_filter::Resource::Rule(
                fnet_filter::Rule {
                    id: fnet_filter::RuleId {
                        routine: fnet_filter::RoutineId {
                            namespace: String::from("namespace"),
                            name: String::from("routine"),
                        },
                        index: 0,
                    },
                    matchers: fnet_filter::Matchers {
                        src_addr: Some(matcher),
                        ..Default::default()
                    },
                    action: fnet_filter::Action::Drop(fnet_filter::Empty {}),
                }
            ))])
            .await
            .expect("call push changes"),
        fnet_filter::ChangeValidationResult::ErrorOnChange(vec![
            fnet_filter::ChangeValidationError::InvalidAddressMatcher
        ])
    );
}

#[netstack_test]
async fn push_change_invalid_port_matcher(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    // Use the FIDL bindings directly rather than going through the extension
    // library, because it intentionally does not allow us to express the
    // invalid types that we are testing.
    let (controller, server_end) = fidl::endpoints::create_proxy();
    control.open_controller("test", server_end).expect("open controller");
    assert_eq!(
        controller
            .push_changes(&[fnet_filter::Change::Create(fnet_filter::Resource::Rule(
                fnet_filter::Rule {
                    id: fnet_filter::RuleId {
                        routine: fnet_filter::RoutineId {
                            namespace: String::from("namespace"),
                            name: String::from("routine"),
                        },
                        index: 0,
                    },
                    matchers: fnet_filter::Matchers {
                        transport_protocol: Some(fnet_matchers::PacketTransportProtocol::Tcp(
                            fnet_matchers::TcpPacket {
                                src_port: Some(fnet_matchers::Port {
                                    start: 1,
                                    end: 0,
                                    invert: false,
                                }),
                                ..Default::default()
                            }
                        )),
                        ..Default::default()
                    },
                    action: fnet_filter::Action::Drop(fnet_filter::Empty {}),
                }
            ))])
            .await
            .expect("call push changes"),
        fnet_filter::ChangeValidationResult::ErrorOnChange(vec![
            fnet_filter::ChangeValidationError::InvalidPortMatcher
        ])
    );
}

#[netstack_test]
#[test_case(
    fnet_filter::Action::TransparentProxy(fnet_filter::TransparentProxy_::LocalPort(0)),
    fnet_filter::ChangeValidationError::InvalidTransparentProxyAction;
    "TPROXY zero local port"
)]
#[test_case(
    fnet_filter::Action::TransparentProxy(fnet_filter::TransparentProxy_::LocalAddrAndPort(
        fnet_filter::SocketAddr { addr: fidl_ip!("192.0.2.1"), port: 0 }
    )),
    fnet_filter::ChangeValidationError::InvalidTransparentProxyAction;
    "TPROXY addr with zero local port"
)]
#[test_case(
    fnet_filter::Action::Redirect(fnet_filter::Redirect {
        dst_port: Some(fnet_filter::PortRange { start: 0, end: u16::MAX }),
        ..Default::default()
    }),
    fnet_filter::ChangeValidationError::InvalidNatAction;
    "redirect zero destination port range start"
)]
#[test_case(
    fnet_filter::Action::Redirect(fnet_filter::Redirect {
        dst_port: Some(fnet_filter::PortRange { start: 0, end: 0 }),
        ..Default::default()
    }),
    fnet_filter::ChangeValidationError::InvalidNatAction;
    "redirect zero destination port range"
)]
#[test_case(
    fnet_filter::Action::Masquerade(fnet_filter::Masquerade {
        src_port: Some(fnet_filter::PortRange { start: 0, end: u16::MAX }),
        ..Default::default()
    }),
    fnet_filter::ChangeValidationError::InvalidNatAction;
    "masquerade zero source port range start"
)]
#[test_case(
    fnet_filter::Action::Masquerade(fnet_filter::Masquerade {
        src_port: Some(fnet_filter::PortRange { start: 0, end: 0 }),
        ..Default::default()
    }),
    fnet_filter::ChangeValidationError::InvalidNatAction;
    "masquerade zero source port"
)]
#[test_case(
    fnet_filter::Action::Redirect(fnet_filter::Redirect {
        dst_port: Some(fnet_filter::PortRange { start: 33333, end: 22222 }),
        ..Default::default()
    }),
    fnet_filter::ChangeValidationError::InvalidPortRange;
    "redirect invalid port range"
)]
#[test_case(
    fnet_filter::Action::Masquerade(fnet_filter::Masquerade {
        src_port: Some(fnet_filter::PortRange { start: 33333, end: 22222 }),
        ..Default::default()
    }),
    fnet_filter::ChangeValidationError::InvalidPortRange;
    "masquerade invalid port range"
)]
async fn push_change_invalid_action(
    name: &str,
    action: fnet_filter::Action,
    expected_err: fnet_filter::ChangeValidationError,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    // Use the FIDL bindings directly rather than going through the extension
    // library, because it intentionally does not allow us to express the
    // invalid types that we are testing.
    let (controller, server_end) = fidl::endpoints::create_proxy();
    control.open_controller("test", server_end).expect("open controller");
    assert_eq!(
        controller
            .push_changes(&[fnet_filter::Change::Create(fnet_filter::Resource::Rule(
                fnet_filter::Rule {
                    id: fnet_filter::RuleId {
                        routine: fnet_filter::RoutineId {
                            namespace: String::from("namespace"),
                            name: String::from("routine"),
                        },
                        index: 0,
                    },
                    matchers: fnet_filter::Matchers::default(),
                    action,
                }
            ))])
            .await
            .expect("call push changes"),
        fnet_filter::ChangeValidationResult::ErrorOnChange(vec![expected_err])
    );
}

enum InvalidChangePosition {
    First,
    Middle,
    Last,
}

#[netstack_test]
#[test_case(InvalidChangePosition::First)]
#[test_case(InvalidChangePosition::Middle)]
#[test_case(InvalidChangePosition::Last)]
async fn push_changes_index_based_error_return(name: &str, pos: InvalidChangePosition) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    // Use the FIDL bindings directly rather than going through the extension
    // library, because it intentionally does not allow us to express the
    // invalid types that we are testing.
    let (controller, server_end) = fidl::endpoints::create_proxy();
    control.open_controller("test", server_end).expect("open controller");

    // Create a batch of valid changes, and insert an invalid change somewhere in the batch.
    let mut changes = vec![fnet_filter::Change::Create(Resource::test_value().into()); 10];
    let index = match pos {
        InvalidChangePosition::First => 0,
        InvalidChangePosition::Middle => changes.len() / 2,
        InvalidChangePosition::Last => changes.len() - 1,
    };
    changes[index] =
        fnet_filter::Change::Create(fnet_filter::Resource::Namespace(fnet_filter::Namespace {
            id: None,
            domain: Some(fnet_filter::Domain::AllIp),
            ..Default::default()
        }));
    let errors = assert_matches!(
        controller.push_changes(&changes).await.expect("call push changes"),
        fnet_filter::ChangeValidationResult::ErrorOnChange(errors) => errors
    );
    let expected = std::iter::repeat(fnet_filter::ChangeValidationError::Ok)
        .take(index)
        .chain(std::iter::once(fnet_filter::ChangeValidationError::MissingRequiredField))
        .chain(
            std::iter::repeat(fnet_filter::ChangeValidationError::NotReached)
                .take(changes.len() - index - 1),
        )
        .collect::<Vec<_>>();
    assert_eq!(errors, expected);
}

#[netstack_test]
#[test_case(
    vec![
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ],
    ResourceId::Namespace(NamespaceId::test_value()),
    &[];
    "removing a namespace removes constituent routines and rules"
)]
#[test_case(
    vec![
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ],
    ResourceId::Routine(RoutineId::test_value()),
    &[Resource::Namespace(Namespace::test_value())];
    "removing a routine removes constituent rules"
)]
async fn remove_resource_removes_contents(
    name: &str,
    resources: Vec<Resource>,
    container: ResourceId,
    expect_remaining: &[Resource],
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    // Add a resource along with some contents.
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    let mut observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
    assert_eq!(
        observed,
        HashMap::from([(
            controller.id().clone(),
            resources
                .into_iter()
                .map(|resource| (resource.id(), resource))
                .collect::<HashMap<_, _>>()
        )])
    );

    // Remove the containing resource.
    controller.push_changes(vec![Change::Remove(container)]).await.expect("push changes");
    controller.commit().await.expect("commit pending changes");

    fnet_filter_ext::wait_for_condition(&mut stream, &mut observed, |resources| {
        resources
            .get(controller.id())
            .expect("get resources owned by controller")
            .values()
            .eq(expect_remaining.iter())
    })
    .await
    .expect("wait for constituent resources to be removed");
}

#[derive(Clone, Copy)]
enum Idempotence {
    Idempotent,
    NonIdempotent,
}

async fn commit_change_expect_result(
    controller: &mut Controller,
    change: Change,
    idempotence: Idempotence,
    expected_result: Result<(), ChangeCommitError>,
) {
    controller.push_changes(vec![change.clone()]).await.expect("push changes");
    let result = match idempotence {
        Idempotence::Idempotent => controller.commit_idempotent().await,
        Idempotence::NonIdempotent => controller.commit().await,
    };
    match expected_result {
        Ok(()) => result.expect("commit should succeed"),
        Err(expected_error) => {
            let errors = assert_matches!(
                result,
                Err(CommitError::ErrorOnChange(errors)) => errors
            );
            let (invalid_change, error) = assert_matches!(
                &errors[..],
                [result] => result,
                "should observe one error on commit"
            );
            assert_eq!(invalid_change, &change);
            assert_eq!(error, &expected_error);
        }
    }
}

#[netstack_test]
#[test_case(
    Resource::Namespace(Namespace::test_value()),
    Resource::Namespace(
        Namespace {
            domain: Domain::Ipv6,
            ..Namespace::test_value()
        },
    );
    "namespace"
)]
#[test_case(
    Resource::Routine(Routine::test_value()),
    Resource::Routine(
        Routine {
            routine_type: RoutineType::Nat(None),
            ..Routine::test_value()
        },
    );
    "routine"
)]
#[test_case(
    Resource::Rule(Rule::test_value()),
    Resource::Rule(
        Rule {
            matchers: Matchers::default(),
            ..Rule::test_value()
        },
    );
    "rule"
)]
async fn add_existing_resource_idempotent(
    name: &str,
    duplicate_resource: Resource,
    resource_with_same_id_different_property: Resource,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");

    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");
    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    let observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
    assert_eq!(
        observed,
        HashMap::from([(
            controller.id().clone(),
            resources
                .into_iter()
                .map(|resource| (resource.id(), resource))
                .collect::<HashMap<_, _>>(),
        )])
    );

    // Add a resource with the same ID as the existing one, but with one of its
    // properties changed. This should always fail, even if `idempotent` is set.
    commit_change_expect_result(
        &mut controller,
        Change::Create(resource_with_same_id_different_property.clone()),
        Idempotence::Idempotent,
        Err(ChangeCommitError::AlreadyExists),
    )
    .await;
    commit_change_expect_result(
        &mut controller,
        Change::Create(resource_with_same_id_different_property),
        Idempotence::NonIdempotent,
        Err(ChangeCommitError::AlreadyExists),
    )
    .await;

    // Add a resource that exactly matches the existing one. If the commit is
    // idempotent, this should succeed; if not, it should fail.
    commit_change_expect_result(
        &mut controller,
        Change::Create(duplicate_resource.clone()),
        Idempotence::Idempotent,
        Ok(()),
    )
    .await;
    commit_change_expect_result(
        &mut controller,
        Change::Create(duplicate_resource),
        Idempotence::NonIdempotent,
        Err(ChangeCommitError::AlreadyExists),
    )
    .await;

    // Whether the attempted changes fail, or succeed idempotently, either way
    // the watcher should observe no changes. Contained resources should also
    // remain installed (routines for a namespace and rules for a routine).
    assert_matches!(
        stream.next().on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || None).await,
        None,
        "no changes should be observed"
    );
}

#[netstack_test]
#[test_case(
    ResourceId::Namespace(NamespaceId::test_value());
    "namespace"
)]
#[test_case(
    ResourceId::Routine(RoutineId::test_value());
    "routine"
)]
#[test_case(ResourceId::Rule(RuleId::test_value()); "rule")]
async fn remove_unknown_resource_idempotent(name: &str, resource: ResourceId) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    let observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
    assert!(observed.is_empty());

    // Remove a resource that doesn't exist. If the commit is idempotent, this
    // should succeed; if not, it should fail.
    commit_change_expect_result(
        &mut controller,
        Change::Remove(resource.clone()),
        Idempotence::Idempotent,
        Ok(()),
    )
    .await;
    commit_change_expect_result(
        &mut controller,
        Change::Remove(resource),
        Idempotence::NonIdempotent,
        Err(ChangeCommitError::NamespaceNotFound),
    )
    .await;

    // Whether the attempted change fails or succeeds, either way the watcher
    // should observe no actual changes.
    assert_matches!(
        stream.next().on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || None).await,
        None,
        "no changes should be observed"
    );
}

#[netstack_test]
#[test_case(Idempotence::Idempotent)]
#[test_case(Idempotence::NonIdempotent)]
async fn reference_unknown_resource(name: &str, idempotence: Idempotence) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    // Adding a routine that refers to an unknown namespace should fail.
    commit_change_expect_result(
        &mut controller,
        Change::Create(Resource::Routine(Routine::test_value())),
        idempotence,
        Err(ChangeCommitError::NamespaceNotFound),
    )
    .await;

    // Adding a rule that refers to an unknown namespace should fail.
    commit_change_expect_result(
        &mut controller,
        Change::Create(Resource::Rule(Rule::test_value())),
        idempotence,
        Err(ChangeCommitError::NamespaceNotFound),
    )
    .await;

    // Add the namespace the rule is referring to. The rule creation should
    // still fail because it refers to an unknown routine.
    controller
        .push_changes(vec![Change::Create(Resource::Namespace(Namespace::test_value()))])
        .await
        .expect("push change");
    controller.commit().await.expect("commit pending change");
    commit_change_expect_result(
        &mut controller,
        Change::Create(Resource::Rule(Rule::test_value())),
        idempotence,
        Err(ChangeCommitError::RoutineNotFound),
    )
    .await;

    // Add the routine the rule is referring to. The rule creation should still
    // fail because it contains a `Jump` action that refers to an unknown
    // routine.
    controller
        .push_changes(vec![Change::Create(Resource::Routine(Routine::test_value()))])
        .await
        .expect("push change");
    commit_change_expect_result(
        &mut controller,
        Change::Create(Resource::Rule(Rule {
            action: Action::Jump(String::from("does-not-exist")),
            ..Rule::test_value()
        })),
        idempotence,
        Err(ChangeCommitError::RoutineNotFound),
    )
    .await;
}

#[netstack_test]
async fn commit_failure_clears_pending_changes_and_does_not_change_state(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    // Add some state and observe its addition.
    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    let observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
    assert_eq!(
        observed,
        HashMap::from([(
            controller.id().clone(),
            resources
                .into_iter()
                .map(|resource| (resource.id(), resource))
                .collect::<HashMap<_, _>>(),
        )])
    );

    // Commit an invalid change and ensure we get an error.
    commit_change_expect_result(
        &mut controller,
        Change::Remove(ResourceId::Namespace(NamespaceId(String::from("does-not-exist")))),
        Idempotence::NonIdempotent,
        Err(ChangeCommitError::NamespaceNotFound),
    )
    .await;

    // Committing again should succeed because the pending change should have
    // been cleared on commit failure.
    controller.commit().await.expect("commit with no pending changes should succeed");

    // State should be unchanged, so the watcher should not observe any events.
    assert_matches!(
        stream.next().on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || None).await,
        None,
        "no changes should be observed"
    );
}

enum DomainType {
    IpAgnostic,
    IpSpecific,
}

enum InvalidMatcher {
    AddressRange,
    Subnet,
    TransportProtocol,
}

fn invalid_address_range_matcher<I: net_types::ip::Ip>() -> Matchers {
    Matchers {
        src_addr: Some(fnet_matchers_ext::Address {
            matcher: fnet_matchers_ext::AddressMatcherType::Range(
                fnet_matchers_ext::AddressRange::try_from({
                    let (start, end) = match I::VERSION {
                        // IPv6 range in an IPv4 namespace
                        IpVersion::V4 => (fidl_ip!("2001:db8::1"), fidl_ip!("2001:db8::2")),
                        // IPv4 range in an IPv6 namespace
                        IpVersion::V6 => (fidl_ip!("192.0.2.1"), fidl_ip!("192.0.2.2")),
                    };
                    fnet_matchers::AddressRange { start, end }
                })
                .unwrap(),
            ),
            invert: false,
        }),
        ..Default::default()
    }
}

fn invalid_subnet_matcher<I: net_types::ip::Ip>() -> Matchers {
    Matchers {
        src_addr: Some(fnet_matchers_ext::Address {
            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                fnet_matchers_ext::Subnet::try_from({
                    match I::VERSION {
                        // IPv6 subnet in an IPv4 namespace
                        IpVersion::V4 => fidl_subnet!("2001:db8::/64"),
                        // IPv4 subnet in an IPv6 namespace
                        IpVersion::V6 => fidl_subnet!("192.0.2.0/24"),
                    }
                })
                .unwrap(),
            ),
            invert: false,
        }),
        ..Default::default()
    }
}

fn invalid_transport_protocol_matcher<I: net_types::ip::Ip>() -> Matchers {
    Matchers {
        transport_protocol: Some({
            match I::VERSION {
                // ICMPv6 in an IPv4 namespace
                IpVersion::V4 => fnet_matchers_ext::TransportProtocol::Icmpv6,
                // ICMPv4 in an IPv6 namespace
                IpVersion::V6 => fnet_matchers_ext::TransportProtocol::Icmp,
            }
        }),
        ..Default::default()
    }
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(DomainType::IpSpecific, InvalidMatcher::AddressRange)]
#[test_case(DomainType::IpSpecific, InvalidMatcher::Subnet)]
#[test_case(DomainType::IpSpecific, InvalidMatcher::TransportProtocol)]
#[test_case(DomainType::IpAgnostic, InvalidMatcher::AddressRange)]
#[test_case(DomainType::IpAgnostic, InvalidMatcher::Subnet)]
#[test_case(DomainType::IpAgnostic, InvalidMatcher::TransportProtocol)]
async fn ip_specific_matcher_in_namespace<I: net_types::ip::Ip>(
    name: &str,
    domain: DomainType,
    matcher: InvalidMatcher,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let namespace_id = NamespaceId(String::from("ip-specific-namespace"));
    let routine_id = RoutineId { namespace: namespace_id.clone(), name: String::from("routine") };
    let invalid_rule_id = RuleId { routine: routine_id.clone(), index: 0 };
    let resources = [
        Resource::Namespace(Namespace {
            id: namespace_id,
            domain: match domain {
                DomainType::IpAgnostic => Domain::AllIp,
                DomainType::IpSpecific => match I::VERSION {
                    IpVersion::V4 => Domain::Ipv4,
                    IpVersion::V6 => Domain::Ipv6,
                },
            },
        }),
        Resource::Routine(Routine {
            id: routine_id,
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }),
        Resource::Rule(Rule {
            id: invalid_rule_id.clone(),
            matchers: match matcher {
                InvalidMatcher::AddressRange => invalid_address_range_matcher::<I>(),
                InvalidMatcher::Subnet => invalid_subnet_matcher::<I>(),
                InvalidMatcher::TransportProtocol => invalid_transport_protocol_matcher::<I>(),
            },
            action: Action::Drop,
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");

    let result = controller.commit().await;
    match domain {
        DomainType::IpAgnostic => result.expect("commit should succeed"),
        DomainType::IpSpecific => {
            let invalid_rule = assert_matches!(
                result,
                Err(CommitError::RuleWithInvalidMatcher(rule_id)) => rule_id
            );
            assert_eq!(invalid_rule, invalid_rule_id);
        }
    }
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(DomainType::IpSpecific, InvalidMatcher::AddressRange)]
#[test_case(DomainType::IpSpecific, InvalidMatcher::Subnet)]
#[test_case(DomainType::IpSpecific, InvalidMatcher::TransportProtocol)]
#[test_case(DomainType::IpAgnostic, InvalidMatcher::AddressRange)]
#[test_case(DomainType::IpAgnostic, InvalidMatcher::Subnet)]
#[test_case(DomainType::IpAgnostic, InvalidMatcher::TransportProtocol)]
async fn jump_to_routine_with_ip_specific_matcher_in_namespace<I: net_types::ip::Ip>(
    name: &str,
    domain: DomainType,
    matcher: InvalidMatcher,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let namespace_id = NamespaceId(String::from("ip-specific-namespace"));
    let routine_id = RoutineId { namespace: namespace_id.clone(), name: String::from("routine") };
    const TARGET_ROUTINE: &str = "target-routine";
    let target_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: TARGET_ROUTINE.to_owned() };
    let target_rule_id = RuleId { routine: target_routine_id.clone(), index: 0 };
    let resources = [
        Resource::Namespace(Namespace {
            id: namespace_id,
            domain: match domain {
                DomainType::IpAgnostic => Domain::AllIp,
                DomainType::IpSpecific => match I::VERSION {
                    IpVersion::V4 => Domain::Ipv4,
                    IpVersion::V6 => Domain::Ipv6,
                },
            },
        }),
        // Target routine includes a rule with an IP-version-specific matcher.
        Resource::Routine(Routine { id: target_routine_id, routine_type: RoutineType::Ip(None) }),
        Resource::Rule(Rule {
            id: target_rule_id.clone(),
            // Note that the matcher is not invalid per se -- it is invalid only because it
            // is IP-specific *and* is installed in a namespace with a *different* IP-
            // specific domain (when `domain` is `Domain::IpSpecific`).
            matchers: match matcher {
                InvalidMatcher::AddressRange => invalid_address_range_matcher::<I>(),
                InvalidMatcher::Subnet => invalid_subnet_matcher::<I>(),
                InvalidMatcher::TransportProtocol => invalid_transport_protocol_matcher::<I>(),
            },
            action: Action::Drop,
        }),
        // Installed routine jumps to target routine.
        Resource::Routine(Routine {
            id: routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Jump(TARGET_ROUTINE.to_owned()),
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");

    let result = controller.commit().await;
    match domain {
        DomainType::IpAgnostic => result.expect("commit should succeed"),
        DomainType::IpSpecific => {
            // Note that the matcher is invalid only in the context of the namespace in
            // which it is installed.
            let invalid_rule = assert_matches!(
                result,
                Err(CommitError::RuleWithInvalidMatcher(rule_id)) => rule_id
            );
            assert_eq!(invalid_rule, target_rule_id);
        }
    }
}

#[netstack_test]
#[test_case(
    RoutineType::Ip(Some(
        InstalledIpRoutine {
            hook: IpHook::Ingress,
            priority: 0,
        },
    )),
    RoutineType::Nat(None);
    "jump from IP routine to NAT routine"
)]
#[test_case(
    RoutineType::Nat(Some(
        InstalledNatRoutine {
            hook: NatHook::Ingress,
            priority: 0,
        },
    )),
    RoutineType::Ip(None);
    "jump from NAT routine to IP routine"
)]
async fn jump_to_routine_of_different_type(
    name: &str,
    calling_routine: RoutineType,
    target_routine: RoutineType,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let routine_id =
        RoutineId { namespace: NamespaceId::test_value(), name: String::from("routine") };
    const TARGET_ROUTINE: &str = "target-routine";
    let target_routine_id =
        RoutineId { namespace: NamespaceId::test_value(), name: TARGET_ROUTINE.to_owned() };
    let rule_id = RuleId { routine: routine_id.clone(), index: 0 };
    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine { id: target_routine_id, routine_type: target_routine }),
        Resource::Routine(Routine { id: routine_id, routine_type: calling_routine }),
        Resource::Rule(Rule {
            id: rule_id.clone(),
            matchers: Matchers::default(),
            action: Action::Jump(TARGET_ROUTINE.to_owned()),
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");

    let invalid_rule = assert_matches!(
        controller.commit().await,
        Err(CommitError::RuleWithInvalidAction(rule_id)) => rule_id
    );
    assert_eq!(invalid_rule, rule_id);
}

#[netstack_test]
async fn rule_jumps_to_installed_routine(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let routine_id =
        RoutineId { namespace: NamespaceId::test_value(), name: String::from("routine") };
    const TARGET_ROUTINE: &str = "target-routine";
    let target_routine_id =
        RoutineId { namespace: NamespaceId::test_value(), name: TARGET_ROUTINE.to_owned() };
    let rule_id = RuleId { routine: routine_id.clone(), index: 0 };
    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine {
            id: target_routine_id,
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }),
        Resource::Routine(Routine {
            id: routine_id,
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }),
        Resource::Rule(Rule {
            id: rule_id.clone(),
            matchers: Matchers::default(),
            action: Action::Jump(TARGET_ROUTINE.to_owned()),
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");

    let errors = assert_matches!(
        controller.commit().await,
        Err(CommitError::ErrorOnChange(errors)) => errors
    );
    let (invalid_change, error) = assert_matches!(
        &errors[..],
        [error] => error
    );
    assert_eq!(invalid_change, &Change::Create(resources.into_iter().last().unwrap()));
    assert_eq!(error, &ChangeCommitError::TargetRoutineIsInstalled);
}

#[netstack_test]
#[test_case(
    RoutineType::Ip(Some(
        InstalledIpRoutine {
            hook: IpHook::Ingress,
            priority: 0,
        },
    ));
    "cycle is in IP installation hook"
)]
#[test_case(
    RoutineType::Ip(None);
    "cycle is between uninstalled routines only"
)]
async fn routine_cycle(name: &str, calling_routine: RoutineType) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let routine_id =
        RoutineId { namespace: NamespaceId::test_value(), name: String::from("calling-routine") };

    const CIRCULAR_ROUTINE: &str = "circular-routine";
    let circular_routine_id =
        RoutineId { namespace: NamespaceId::test_value(), name: CIRCULAR_ROUTINE.to_owned() };

    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine {
            id: circular_routine_id.clone(),
            routine_type: RoutineType::Ip(None),
        }),
        Resource::Routine(Routine { id: routine_id.clone(), routine_type: calling_routine }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Jump(CIRCULAR_ROUTINE.to_owned()),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: circular_routine_id.clone(), index: 0 },
            matchers: Matchers::default(),
            action: Action::Jump(CIRCULAR_ROUTINE.to_owned()),
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");

    let invalid_routine = assert_matches!(
        controller.commit().await,
        Err(CommitError::CyclicalRoutineGraph(routine_id)) => routine_id
    );
    assert_eq!(invalid_routine, circular_routine_id);
}

#[netstack_test]
#[variant(I, Ip)]
async fn uninstalled_routine_validated_even_if_unreachable<I: net_types::ip::Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let namespace_id = NamespaceId(String::from("ip-specific-namespace"));
    let routine_id = RoutineId { namespace: namespace_id.clone(), name: String::from("routine") };
    let rule_id = RuleId { routine: routine_id.clone(), index: 0 };
    let resources = [
        Resource::Namespace(Namespace {
            id: namespace_id,
            domain: match I::VERSION {
                IpVersion::V4 => Domain::Ipv4,
                IpVersion::V6 => Domain::Ipv6,
            },
        }),
        // Uninstalled routine includes a rule with an IP-version-specific matcher.
        Resource::Routine(Routine { id: routine_id, routine_type: RoutineType::Ip(None) }),
        Resource::Rule(Rule {
            id: rule_id.clone(),
            matchers: invalid_address_range_matcher::<I>(),
            action: Action::Drop,
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");

    let result = controller.commit().await;
    let invalid_rule = assert_matches!(
        result,
        Err(CommitError::RuleWithInvalidMatcher(rule_id)) => rule_id
    );
    assert_eq!(invalid_rule, rule_id);
}

enum WhichInterface {
    In,
    Out,
}

#[netstack_test]
#[test_case(
    RoutineType::Ip(None),
    RoutineType::Ip(Some(InstalledIpRoutine {
        hook: IpHook::Ingress,
        priority: 0,
    })),
    WhichInterface::Out;
    "match on out interface in IP ingress"
)]
#[test_case(
    RoutineType::Ip(None),
    RoutineType::Ip(Some(InstalledIpRoutine {
        hook: IpHook::LocalIngress,
        priority: 0,
    })),
    WhichInterface::Out;
    "match on out interface in IP local ingress"
)]
#[test_case(
    RoutineType::Ip(None),
    RoutineType::Ip(Some(InstalledIpRoutine {
        hook: IpHook::Egress,
        priority: 0,
    })),
    WhichInterface::In;
    "match on in interface in IP egress"
)]
#[test_case(
    RoutineType::Ip(None),
    RoutineType::Ip(Some(InstalledIpRoutine {
        hook: IpHook::LocalEgress,
        priority: 0,
    })),
    WhichInterface::In;
    "match on in interface in IP local egress"
)]
#[test_case(
    RoutineType::Nat(None),
    RoutineType::Nat(Some(InstalledNatRoutine {
        hook: NatHook::Ingress,
        priority: 0,
    })),
    WhichInterface::Out;
    "match on out interface in NAT ingress"
)]
#[test_case(
    RoutineType::Nat(None),
    RoutineType::Nat(Some(InstalledNatRoutine {
        hook: NatHook::LocalIngress,
        priority: 0,
    })),
    WhichInterface::Out;
    "match on out interface in NAT local ingress"
)]
#[test_case(
    RoutineType::Nat(None),
    RoutineType::Nat(Some(InstalledNatRoutine {
        hook: NatHook::Egress,
        priority: 0,
    })),
    WhichInterface::In;
    "match on in interface in NAT egress"
)]
#[test_case(
    RoutineType::Nat(None),
    RoutineType::Nat(Some(InstalledNatRoutine {
        hook: NatHook::LocalEgress,
        priority: 0,
    })),
    WhichInterface::In;
    "match on in interface in NAT local egress"
)]
async fn invalid_matcher_for_hook(
    name: &str,
    uninstalled_routine: RoutineType,
    installed_routine: RoutineType,
    interface_matcher: WhichInterface,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let namespace_id = NamespaceId(String::from("namespace"));
    const TARGET_ROUTINE: &str = "target-routine";
    let target_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: TARGET_ROUTINE.to_owned() };
    let rule_id = RuleId { routine: target_routine_id.clone(), index: 0 };
    let resources = [
        Resource::Namespace(Namespace { id: namespace_id.clone(), domain: Domain::AllIp }),
        // Uninstalled routine includes a rule with a matcher that is only valid in
        // particular hooks.
        Resource::Routine(Routine { id: target_routine_id, routine_type: uninstalled_routine }),
        Resource::Rule(Rule {
            id: rule_id.clone(),
            matchers: match interface_matcher {
                WhichInterface::In => Matchers {
                    in_interface: Some(fnet_matchers_ext::Interface::PortClass(
                        PortClass::WlanClient,
                    )),
                    ..Default::default()
                },
                WhichInterface::Out => Matchers {
                    out_interface: Some(fnet_matchers_ext::Interface::PortClass(
                        PortClass::WlanClient,
                    )),
                    ..Default::default()
                },
            },
            action: Action::Drop,
        }),
    ];
    // Adding the uninstalled routine should succeed since it is not reachable from
    // any hook yet.
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let installed_routine_id =
        RoutineId { namespace: namespace_id, name: String::from("installed") };
    let resources = [
        // Routine is installed in a hook, and it jumps to a routine with a rule that
        // matches on a property that is unavailable in that hook.
        Resource::Routine(Routine {
            id: installed_routine_id.clone(),
            routine_type: installed_routine,
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: installed_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Jump(TARGET_ROUTINE.to_owned()),
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    let result = controller.commit().await;
    let invalid_rule = assert_matches!(
        result,
        Err(CommitError::RuleWithInvalidMatcher(rule_id)) => rule_id
    );
    assert_eq!(invalid_rule, rule_id);
}

const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(8080).unwrap();

#[netstack_test]
#[test_case(
    Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT)),
    &[
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Ingress, priority: 0})),
    ],
    &[
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalIngress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Forwarding, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalEgress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Egress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Ingress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalIngress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalEgress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Egress, priority: 0})),
    ];
    "TPROXY valid in IP INGRESS"
)]
#[test_case(
    Action::Redirect { dst_port: None },
    &[
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Ingress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalEgress, priority: 0})),
    ],
    &[
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Ingress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalIngress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Forwarding, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalEgress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Egress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalIngress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Egress, priority: 0})),
    ];
    "redirect valid in NAT INGRESS and LOCAL_EGRESS"
)]
#[test_case(
    Action::Masquerade { src_port: None },
    &[
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Egress, priority: 0})),
    ],
    &[
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Ingress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalIngress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Forwarding, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalEgress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Egress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Ingress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalIngress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalEgress, priority: 0})),
    ];
    "masquerade valid in NAT EGRESS"
)]
#[test_case(
    Action::Reject(fnet_filter_ext::RejectType::HostUnreachable),
    &[
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalIngress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Forwarding, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::LocalEgress, priority: 0})),
    ],
    &[
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Ingress, priority: 0})),
        RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Egress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Ingress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalIngress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::LocalEgress, priority: 0})),
        RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Egress, priority: 0})),
    ];
    "reject valid in IP LOCAL_INGRESS, FORWARDING, LOCAL_EGRESS"
)]
async fn invalid_action_for_hook(
    name: &str,
    action: fnet_filter_ext::Action,
    valid_hooks: &[RoutineType],
    invalid_hooks: &[RoutineType],
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");
    const NAMESPACE: &str = "namespace";
    controller
        .push_changes(vec![Change::Create(Resource::Namespace(Namespace {
            id: NamespaceId(NAMESPACE.to_owned()),
            domain: Domain::AllIp,
        }))])
        .await
        .expect("push changes");

    async fn create_routine_and_rule(
        controller: &mut Controller,
        routine_type: &RoutineType,
        action: &fnet_filter_ext::Action,
    ) -> (RuleId, Result<(), fnet_filter_ext::CommitError>) {
        // Give each routine a unique ID since some of them will be added successfully.
        static ID: AtomicUsize = AtomicUsize::new(0);

        let routine_id = RoutineId {
            namespace: NamespaceId(NAMESPACE.to_owned()),
            name: format!("routine-{}", ID.fetch_add(1, Ordering::Relaxed)),
        };
        let rule_id = RuleId { routine: routine_id.clone(), index: 0 };
        let resources = [
            Resource::Routine(Routine { id: routine_id, routine_type: routine_type.clone() }),
            Resource::Rule(Rule {
                id: rule_id.clone(),
                matchers: Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: action.clone(),
            }),
        ];
        controller
            .push_changes(resources.iter().cloned().map(Change::Create).collect())
            .await
            .expect("push changes");
        (rule_id, controller.commit().await)
    }

    for routine_type in valid_hooks {
        let (_rule_id, result) =
            create_routine_and_rule(&mut controller, routine_type, &action).await;
        result.expect("commit should succeed");
    }
    for routine_type in invalid_hooks {
        let (rule_id, result) =
            create_routine_and_rule(&mut controller, routine_type, &action).await;
        let invalid_rule = assert_matches!(
            result,
            Err(CommitError::RuleWithInvalidAction(rule_id)) => rule_id
        );
        assert_eq!(invalid_rule, rule_id);
    }
}

enum ActionWithPort {
    TransparentProxy,
    Redirect,
    Masquerade,
}

impl ActionWithPort {
    fn action(&self) -> Action {
        match self {
            Self::TransparentProxy => {
                Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT))
            }
            Self::Redirect => {
                Action::Redirect { dst_port: Some(PortRange(LOCAL_PORT..=LOCAL_PORT)) }
            }
            Self::Masquerade => {
                Action::Masquerade { src_port: Some(PortRange(LOCAL_PORT..=LOCAL_PORT)) }
            }
        }
    }

    fn routine_type(&self) -> RoutineType {
        match self {
            Self::TransparentProxy => {
                RoutineType::Ip(Some(InstalledIpRoutine { hook: IpHook::Ingress, priority: 0 }))
            }
            Self::Redirect => {
                RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Ingress, priority: 0 }))
            }
            Self::Masquerade => {
                RoutineType::Nat(Some(InstalledNatRoutine { hook: NatHook::Egress, priority: 0 }))
            }
        }
    }

    #[track_caller]
    fn assert_expected_error(&self, error: CommitError) -> RuleId {
        match self {
            Self::TransparentProxy => {
                assert_matches!(error, CommitError::TransparentProxyWithInvalidMatcher(rule) => rule)
            }
            Self::Redirect => {
                assert_matches!(error, CommitError::RedirectWithInvalidMatcher(rule) => rule)
            }
            Self::Masquerade => {
                assert_matches!(error, CommitError::MasqueradeWithInvalidMatcher(rule) => rule)
            }
        }
    }
}

#[netstack_test]
#[test_matrix(
    [
        ActionWithPort::TransparentProxy,
        ActionWithPort::Redirect,
        ActionWithPort::Masquerade,
    ],
    [
        (Some(fnet_matchers_ext::TransportProtocol::Tcp { src_port: None, dst_port: None }), Ok(())),
        (Some(fnet_matchers_ext::TransportProtocol::Udp { src_port: None, dst_port: None }), Ok(())),
        (Some(fnet_matchers_ext::TransportProtocol::Icmp), Err(())),
        (Some(fnet_matchers_ext::TransportProtocol::Icmpv6), Err(())),
        (None, Err(())),
    ]
)]
async fn invalid_matcher_for_action_with_specified_port(
    name: &str,
    action: ActionWithPort,
    matcher_and_expected_result: (Option<fnet_matchers_ext::TransportProtocol>, Result<(), ()>),
) {
    let (matcher, expected_result) = matcher_and_expected_result;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");
    let namespace_id = NamespaceId(String::from("namespace"));
    let routine_id = RoutineId { namespace: namespace_id.clone(), name: String::from("routine") };
    let rule_id = RuleId { routine: routine_id.clone(), index: 0 };
    let resources = [
        Resource::Namespace(Namespace { id: namespace_id.clone(), domain: Domain::AllIp }),
        Resource::Routine(Routine { id: routine_id, routine_type: action.routine_type() }),
        Resource::Rule(Rule {
            id: rule_id.clone(),
            matchers: Matchers { transport_protocol: matcher, ..Default::default() },
            action: action.action(),
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    let result = controller.commit().await;
    match expected_result {
        Ok(()) => result.expect("commit should succeed"),
        Err(()) => {
            let error = result.expect_err("commit should fail");
            let invalid_rule = action.assert_expected_error(error);
            assert_eq!(invalid_rule, rule_id);
        }
    }
}

#[netstack_test]
async fn ebpf_program_registration(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let program = ebpf_test_util::TestProgramDefinition::load(ebpf_api::ProgramType::SocketFilter)
        .instantiate();
    controller
        .register_ebpf_program(program.get_program_handle(), program.get_fidl_program())
        .await
        .expect("register ebpf program");

    // Repeated attempt to register the same program should fail.
    assert_matches!(
        controller
            .register_ebpf_program(program.get_program_handle(), program.get_fidl_program())
            .await
            .expect_err("duplicate program registration should fail"),
        RegisterEbpfProgramError::AlreadyRegistered
    );

    // Register the same program from another controller.
    let mut controller2 =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");
    controller2
        .register_ebpf_program(program.get_program_handle(), program.get_fidl_program())
        .await
        .expect("register ebpf program");

    // Raising `PROGRAM_DEFUNCT_SIGNAL` should result in the program being dropped
    // by the netstack.
    let server_handle = program.mark_defunct();

    // Wait for the program to be dropped by the netstack.
    let _: Result<zx::Signals, zx::Status> =
        OnSignals::new(&server_handle, zx::Signals::EVENTPAIR_PEER_CLOSED)
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || panic!("timeout"))
            .await;
}

#[netstack_test]
async fn ebpf_matcher(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller =
        Controller::new(&control, &ControllerId(name.to_owned())).await.expect("create controller");

    let program = ebpf_test_util::TestProgramDefinition::load(ebpf_api::ProgramType::SocketFilter)
        .instantiate();
    let program_id = program.get_program_id();
    controller
        .register_ebpf_program(program.get_program_handle(), program.get_fidl_program())
        .await
        .expect("register ebpf program");

    let namespace_id = NamespaceId(String::from("namespace"));
    const TARGET_ROUTINE: &str = "target-routine";
    let target_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: TARGET_ROUTINE.to_owned() };
    let resources = [
        Resource::Namespace(Namespace { id: namespace_id.clone(), domain: Domain::AllIp }),
        Resource::Routine(Routine {
            id: target_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: target_routine_id.clone(), index: 0 },
            matchers: Matchers { ebpf_program: Some(program_id), ..Default::default() },
            action: Action::Drop,
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    // Raising `PROGRAM_DEFUNCT_SIGNAL` should result in the program being dropped
    // by the netstack.
    let server_handle = program.mark_defunct();

    // Wait for the program to be dropped by the netstack.
    let _: Result<zx::Signals, zx::Status> =
        OnSignals::new(&server_handle, zx::Signals::EVENTPAIR_PEER_CLOSED)
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || panic!("timeout"))
            .await;

    // Try adding another rule to the same routine. This
    // should not fail even though the `program_id` is no longer registered.
    let changes = vec![Change::Create(Resource::Rule(Rule {
        id: RuleId { routine: target_routine_id.clone(), index: 2 },
        matchers: Matchers {
            transport_protocol: Some(TransportProtocol::Icmp),
            ..Default::default()
        },
        action: Action::Drop,
    }))];
    controller.push_changes(changes).await.expect("push changes");
    controller.commit().await.expect("commit pending changes");

    // Try registering another rule using the same `program_id`. This is
    // expected to fail since the program is no longer registered.
    let changes = vec![Change::Create(Resource::Rule(Rule {
        id: RuleId { routine: target_routine_id.clone(), index: 3 },
        matchers: Matchers { ebpf_program: Some(program_id), ..Default::default() },
        action: Action::Drop,
    }))];
    controller.push_changes(changes.clone()).await.expect("push changes");
    let error = controller.commit().await.expect_err("commit should fail");
    let errors = assert_matches!(
        error,
        CommitError::ErrorOnChange(errors) => errors
    );
    assert_eq!(errors, vec![(changes[0].clone(), ChangeCommitError::InvalidEbpfProgramId)])
}
