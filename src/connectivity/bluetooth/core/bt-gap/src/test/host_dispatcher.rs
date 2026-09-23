// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use assert_matches::assert_matches;
use async_helpers::hanging_get::asynchronous as hanging_get;
use diagnostics_assertions::assert_data_tree;
use fidl_fuchsia_bluetooth::Appearance;
use fidl_fuchsia_bluetooth_host::{BondingDelegateRequest, HostRequest};
use fidl_fuchsia_bluetooth_sys::{self as sys, TechnologyType};
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_bluetooth::types::bonding_data::example;
use fuchsia_bluetooth::types::{Address, HostData, HostId, Identity, Peer, PeerId};
use fuchsia_inspect as inspect;
use futures::channel::mpsc;
use futures::future::join;
use futures::stream::TryStreamExt;
use futures::{FutureExt, StreamExt, TryFutureExt};
use log::info;
use std::collections::{HashMap, HashSet};

use crate::build_config::{BrEdrConfig, Config};
use crate::host_dispatcher::{DEFAULT_DEVICE_NAME, HostDispatcher, NameReplace, test as hd_test};
use crate::store::stash::Stash;
use crate::types;

fn peer(id: PeerId) -> Peer {
    Peer {
        id: id.into(),
        address: Address::Public([1, 2, 3, 4, 5, 6]),
        technology: TechnologyType::LowEnergy,
        name: Some("Peer Name".into()),
        appearance: Some(Appearance::Watch),
        device_class: None,
        rssi: None,
        tx_power: None,
        connected: false,
        bonded: false,
        le_services: vec![],
        bredr_services: vec![],
    }
}

#[fuchsia::test]
async fn on_device_changed_inspect_state() {
    // test setup
    let stash = Stash::in_memory_mock();
    let inspector = inspect::Inspector::default();
    let system_inspect = inspector.root().create_child("system");
    let (gas_channel_sender, _generic_access_req_stream) = mpsc::channel(0);
    let watch_peers_broker = hanging_get::HangingGetBroker::new(
        HashMap::new(),
        |_, _| true,
        hanging_get::DEFAULT_CHANNEL_SIZE,
    );
    let watch_hosts_broker = hanging_get::HangingGetBroker::new(
        Vec::new(),
        |_, _| true,
        hanging_get::DEFAULT_CHANNEL_SIZE,
    );
    let dispatcher = HostDispatcher::new(
        Appearance::Display,
        Default::default(),
        stash,
        system_inspect,
        gas_channel_sender,
        watch_peers_broker.new_publisher(),
        watch_peers_broker.new_registrar(),
        watch_hosts_broker.new_publisher(),
        watch_hosts_broker.new_registrar(),
    );
    let peer_id = PeerId(1);

    // assert inspect tree is in clean state
    assert_data_tree!(inspector, root: {
        system: contains {
            peer_count: 0u64,
            peers: {}
        }
    });

    // add new peer and assert inspect tree is updated
    dispatcher.on_device_updated(peer(peer_id)).await;
    assert_data_tree!(inspector, root: {
        system: contains {
            peer_count: 1u64,
            peers: {
                "peer_0": contains {
                    peer_id: peer_id.to_string(),
                    technology: "LowEnergy"
                }
            }
        }
    });

    // remove peer and assert inspect tree is updated
    dispatcher.on_device_removed(peer_id).await;
    assert_data_tree!(inspector, root: {
        system: contains {
            peer_count: 0u64,
            peers: { },
            recently_removed: contains {
                "0": contains {
                    peer: contains {
                        peer_id: peer_id.to_string(),
                    }
                }
            }
        }
    });
}

#[fuchsia::test]
async fn test_change_name_no_deadlock() {
    let dispatcher = hd_test::make_simple_test_dispatcher();
    // Call a function that used to use the self.state.write().gas_channel_sender.send().await
    // pattern, which caused a deadlock by yielding to the executor while holding onto a write
    // lock to the mutable gas_channel. We expect an error here because there's no active host
    // in the dispatcher - we don't need to go through the trouble of setting up an emulated
    // host to test whether or not we can send messages to the GAS task. We just want to make
    // sure that the function actually returns and doesn't deadlock.
    assert_matches!(
        dispatcher.set_name("test-change".to_string(), NameReplace::Replace).await.unwrap_err(),
        types::Error::SysError(sys::Error::Failed)
    );
}

fn host_is_in_dispatcher(id: &HostId, dispatcher: &HostDispatcher) -> bool {
    dispatcher.get_adapters().iter().map(|i| i.id).collect::<HashSet<_>>().contains(id)
}

#[fuchsia::test]
async fn apply_settings_fails_host_removed() {
    let dispatcher = hd_test::make_simple_test_dispatcher();
    let host_id = HostId(42);
    let (mut host_server, _, _gatt_server, _bonding) =
        hd_test::create_and_add_test_host_to_dispatcher(host_id, &dispatcher).await.unwrap();
    assert!(host_is_in_dispatcher(&host_id, &dispatcher));
    let run_host = async move {
        match host_server.try_next().await {
            Ok(Some(HostRequest::SetConnectable { responder, .. })) => {
                responder.send(Err(sys::Error::Failed)).unwrap()
            }
            x => panic!("Unexpected request: {:?}", x),
        }
    };
    let disable_connectable_fut = async {
        let updated_config = dispatcher
            .apply_sys_settings(sys::Settings {
                bredr_connectable_mode: Some(false),
                ..Default::default()
            })
            .await;
        assert_matches!(
            updated_config,
            Config { bredr: BrEdrConfig { connectable: false, .. }, .. }
        );
    };
    futures::future::join(run_host, disable_connectable_fut).await;
    assert!(!host_is_in_dispatcher(&host_id, &dispatcher));
}

#[fuchsia::test]
async fn default_name_behavior() {
    let dispatcher = hd_test::make_simple_test_dispatcher();
    let host_id = HostId(42);
    let (mut host_server, _, _gatt_server, _bonding) =
        hd_test::create_and_add_test_host_to_dispatcher(host_id, &dispatcher).await.unwrap();

    let _host_server_answers_set_name = fasync::Task::spawn(async move {
        while let Some(request) = host_server.next().await {
            if let Err(e) = request {
                panic!("Unexpected error from host server: {:?}", e);
            }
            use fidl_fuchsia_bluetooth_host::HostRequest;
            match request.unwrap() {
                HostRequest::SetLocalName { local_name, responder } => {
                    info!("Host Local name was set to {}", local_name);
                    responder.send(Ok(())).unwrap();
                }
                x => panic!("Unexpected request to host server: {:?}", x),
            }
        }
        info!("Ended host server stream");
    });

    assert_eq!(dispatcher.get_name(), DEFAULT_DEVICE_NAME);
    // TODO: assert that host_server gets a the default name set
    // No name is set, so this should set the name.
    let test_name = "Sapphire💖";
    dispatcher.set_name(test_name.to_string(), NameReplace::Keep).await.unwrap();
    info!("Name is set");
    assert_eq!(dispatcher.get_name(), test_name);
    // Replacing with ::Replace should replace the name
    let replace_name = "Fuchsia Sapphire";
    dispatcher.set_name(replace_name.to_string(), NameReplace::Replace).await.unwrap();
    info!("Name is set 2");
    assert_eq!(dispatcher.get_name(), replace_name);
    // Replacing again with Keep should not change the name
    dispatcher.set_name(test_name.to_string(), NameReplace::Keep).await.unwrap();
    info!("Name is set 3");
    assert_eq!(dispatcher.get_name(), replace_name);
}

#[fuchsia::test]
async fn test_commit_bootstrap_doesnt_fail_from_host_failure() {
    // initiate test host-dispatcher
    let host_dispatcher = hd_test::make_simple_test_dispatcher();

    // add a test host with a channel we provide, which fails on restore_bonds()
    let host_id = HostId(1);
    let (_host_server, _, _gatt_server, mut bonding_delegate_stream) =
        hd_test::create_and_add_test_host_to_dispatcher(host_id, &host_dispatcher).await.unwrap();
    assert!(host_is_in_dispatcher(&host_id, &host_dispatcher));

    let run_delegate = async move {
        match bonding_delegate_stream.try_next().await {
            Ok(Some(BondingDelegateRequest::RestoreBonds { bonds, responder })) => {
                // Fail by returning all bonds as errors
                let _ = responder.send(&bonds);
            }
            x => panic!("Expected RestoreBonds Request but got: {:?}", x),
        }
        bonding_delegate_stream
    };

    let identity = Identity {
        host: HostData { irk: None },
        bonds: vec![example::bond(
            Address::Public([1, 1, 1, 1, 1, 1]),
            Address::Public([0, 0, 0, 0, 0, 0]),
        )],
    };
    // Call dispatcher.commit_bootstrap() & assert that the result is success
    let (result, _bonding_delegate_stream) =
        futures::future::join(host_dispatcher.commit_bootstrap(vec![identity]), run_delegate).await;
    assert_matches!(result, Ok(()));
}

#[fuchsia::test]
async fn test_notify_host_watcher_of_active_hosts() {
    let watch_peers_broker = hanging_get::HangingGetBroker::new(
        HashMap::new(),
        |_, _| true,
        hanging_get::DEFAULT_CHANNEL_SIZE,
    );

    let watch_hosts_broker = hanging_get::HangingGetBroker::new(
        Vec::new(),
        crate::host_watcher::observe_hosts,
        hanging_get::DEFAULT_CHANNEL_SIZE,
    );

    let host_dispatcher = hd_test::make_test_dispatcher(
        watch_peers_broker.new_publisher(),
        watch_peers_broker.new_registrar(),
        watch_hosts_broker.new_publisher(),
        watch_hosts_broker.new_registrar(),
    );

    let watchers_fut = join(watch_peers_broker.run(), watch_hosts_broker.run()).map(|_| ());
    fasync::Task::spawn(watchers_fut).detach();

    // Start HostWatcher client/server.
    let (host_watcher_proxy, host_watcher_stream) =
        fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_bluetooth_sys::HostWatcherMarker>();
    let host_watcher_fut = crate::host_watcher::run(host_dispatcher.clone(), host_watcher_stream);
    let _host_watcher_task = fasync::Task::spawn(host_watcher_fut);

    // New host should be active by default.
    let host_id_0 = HostId(0);
    let _host_server_0 =
        hd_test::create_and_add_test_host_to_dispatcher(host_id_0, &host_dispatcher)
            .await
            .expect("add test host succeeds");
    assert!(host_is_in_dispatcher(&host_id_0, &host_dispatcher));

    // The future has a timeout so that tests consistently terminate.
    const HOST_WATCHER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
    let hosts = host_watcher_proxy
        .watch()
        .map_err(|e| Error::from(e))
        .on_timeout(HOST_WATCHER_TIMEOUT, move || Err(format_err!("watch timed out")))
        .await
        .expect("watch timed out");
    assert_eq!(hosts.len(), 1);
    assert_matches!(hosts[0].active, Some(true));

    // Add a second host that should start inactive.
    let host_id_1 = HostId(1);
    let _host_server_1 =
        hd_test::create_and_add_test_host_to_dispatcher(host_id_1, &host_dispatcher)
            .await
            .expect("add test host succeeds");
    assert!(host_is_in_dispatcher(&host_id_1, &host_dispatcher));

    // The future has a timeout so that tests consistently terminate.
    let mut hosts = host_watcher_proxy
        .watch()
        .map_err(|e| Error::from(e))
        .on_timeout(HOST_WATCHER_TIMEOUT, move || Err(format_err!("watch timed out")))
        .await
        .expect("watch timed out");
    assert_eq!(hosts.len(), 2);
    hosts.sort_by(|a, b| a.id.unwrap().value.cmp(&b.id.unwrap().value));
    assert_eq!(hosts[0].id, Some(host_id_0.into()));
    assert_eq!(hosts[0].active, Some(true));
    assert_eq!(hosts[1].id, Some(host_id_1.into()));
    assert_eq!(hosts[1].active, Some(false));

    // Setting host 1 to active should set host 0 to inactive.
    assert_matches!(host_watcher_proxy.set_active(&host_id_1.into()).await, Ok(_));
    let mut hosts = host_watcher_proxy
        .watch()
        .map_err(|e| Error::from(e))
        .on_timeout(HOST_WATCHER_TIMEOUT, move || Err(format_err!("watch timed out")))
        .await
        .expect("watch timed out");
    assert_eq!(hosts.len(), 2);
    hosts.sort_by(|a, b| a.id.unwrap().value.cmp(&b.id.unwrap().value));
    assert_eq!(hosts[0].id, Some(host_id_0.into()));
    assert_eq!(hosts[0].active, Some(false));
    assert_eq!(hosts[1].id, Some(host_id_1.into()));
    assert_eq!(hosts[1].active, Some(true));

    // Removing active host 1 should mark host 0 as active.
    host_dispatcher.rm_device(&format!("/dev/host{}", host_id_1.0)).await;
    // The future has a timeout so that tests consistently terminate.
    let hosts = host_watcher_proxy
        .watch()
        .map_err(|e| Error::from(e))
        .on_timeout(HOST_WATCHER_TIMEOUT, move || Err(format_err!("watch timed out")))
        .await
        .expect("watch timed out");
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, Some(host_id_0.into()));
    assert_eq!(hosts[0].active, Some(true));
}

#[fuchsia::test]
async fn test_set_active_host_invalid_id_does_not_shutdown_active_host() {
    let host_dispatcher = hd_test::make_simple_test_dispatcher();
    let host_id = HostId(42);
    let (mut host_server, _, _gatt_server, _bonding) =
        hd_test::create_and_add_test_host_to_dispatcher(host_id, &host_dispatcher).await.unwrap();
    assert!(host_is_in_dispatcher(&host_id, &host_dispatcher));

    // The host should be active by default.
    let active_host = host_dispatcher.active_host().await;
    assert!(active_host.is_some());
    assert_eq!(active_host.unwrap().id(), host_id);

    // Try to set active host to an invalid ID.
    let invalid_id = HostId(0);
    let result = host_dispatcher.set_active_host(invalid_id);
    assert!(result.is_err());

    // Verify that the active host is still the original host.
    let active_host = host_dispatcher.active_host().await;
    assert!(active_host.is_some());
    assert_eq!(active_host.unwrap().id(), host_id);

    // Verify that no shutdown request was sent to the active host.
    let next_request = host_server.next().now_or_never();
    assert!(next_request.is_none(), "Expected no request to host, but got: {:?}", next_request);
}
