// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::host_dispatcher::test as hd_test;
use crate::{host_dispatcher, run_receiver_server};
use assert_matches::assert_matches;
use async_helpers::hanging_get::asynchronous as hanging_get;
use fidl::endpoints::{self, Responder};
use fidl_fuchsia_bluetooth_gatt2::Server_Request;
use fidl_fuchsia_bluetooth_host::{
    BondingDelegateMarker, HostMarker, HostRequest, HostRequestStream, PeerWatcherMarker,
    ProtocolRequest, ReceiverMarker,
};
use fidl_fuchsia_bluetooth_sys::{HostInfo as FidlHostInfo, TechnologyType};
use fuchsia_async as fasync;
use fuchsia_bluetooth::types::{Address, HostId, HostInfo};
use futures::future::join;
use futures::{FutureExt, StreamExt, TryStreamExt};
use log::info;
use std::collections::{HashMap, HashSet};

async fn handle_host_requests(id: HostId, mut stream: HostRequestStream) {
    let mut first_state_req = true;
    let mut peer_watcher_server: Option<fidl::endpoints::ServerEnd<PeerWatcherMarker>> = None;
    let mut bonding_delegate_server: Option<fidl::endpoints::ServerEnd<BondingDelegateMarker>> =
        None;
    while let Some(request) = stream.try_next().await.expect("Invalid Host request") {
        match request {
            HostRequest::WatchState { responder } => {
                if first_state_req {
                    info!("WatchState request");
                    let id_val = id.0 as u8;
                    let address = Address::Public([id_val; 6]);
                    let info = HostInfo {
                        id,
                        technology: TechnologyType::DualMode,
                        active: false,
                        local_name: None,
                        discoverable: false,
                        discovering: false,
                        addresses: vec![address],
                    };
                    assert_matches::assert_matches!(
                        responder.send(&FidlHostInfo::from(info)),
                        Ok(())
                    );
                    first_state_req = false;
                    continue;
                }
                responder.drop_without_shutdown();
            }
            HostRequest::SetLocalData { .. } => {
                info!("SetLocalData request");
            }
            HostRequest::EnablePrivacy { .. } => {
                info!("EnablePrivacy request");
            }
            HostRequest::EnableBackgroundScan { .. } => {
                info!("EnableBackgroundScan request");
            }
            HostRequest::SetBrEdrSecurityMode { .. } => {
                info!("SetBrEdrSecurityMode request");
            }
            HostRequest::SetLeSecurityMode { .. } => {
                info!("SetLeSecurityMode request");
            }
            HostRequest::SetConnectable { responder, .. } => {
                info!("SetConnectable request");
                assert_matches::assert_matches!(responder.send(Ok(())), Ok(()));
            }
            HostRequest::SetLocalName { responder, .. } => {
                info!("SetLocalName request");
                assert_matches::assert_matches!(responder.send(Ok(())), Ok(()));
            }
            HostRequest::RequestProtocol {
                payload: ProtocolRequest::Gatt2Server(server), ..
            } => {
                info!("RequestGatt2Server request");
                let mut gatt_server = server.into_stream();
                match gatt_server.next().await {
                    Some(Ok(Server_Request::PublishService { responder, .. })) => {
                        info!("PublishService request");
                        assert_matches::assert_matches!(responder.send(Ok(())), Ok(()));
                    }
                    x => panic!("Unexpected GATT server request: {:?}", x),
                }
            }
            HostRequest::SetPeerWatcher { peer_watcher, .. } => {
                info!("SetPeerWatcher");
                assert!(!peer_watcher_server.is_some());
                peer_watcher_server = Some(peer_watcher);
            }
            HostRequest::SetBondingDelegate { delegate, .. } => {
                info!("SetBondingDelegate");
                assert!(!bonding_delegate_server.is_some());
                bonding_delegate_server = Some(delegate);
            }
            x => panic!("Unexpected host server request: {:?}", x),
        }
    }
}

fn host_is_in_dispatcher(id: &HostId, dispatcher: &host_dispatcher::HostDispatcher) -> bool {
    dispatcher.get_adapters().iter().map(|i| i.id).collect::<HashSet<_>>().contains(id)
}

#[fuchsia::test]
async fn add_component_succeeds() {
    // Create mock host dispatcher
    let hd = host_dispatcher::test::make_simple_test_dispatcher();

    // Create the receiver server
    let (receiver_client, receiver_stream) = endpoints::create_proxy_and_stream::<ReceiverMarker>();
    let _receiver_task = fasync::Task::spawn(run_receiver_server(hd.clone(), receiver_stream));

    // Mock the host server
    let (host_client, host_server) = endpoints::create_request_stream::<HostMarker>();
    let _run_host_task = fasync::Task::spawn(handle_host_requests(HostId(1), host_server));

    // Add a host to dispatcher using host.Receiver API
    let add_host_result = receiver_client.add_host(host_client);

    assert_matches!(add_host_result, Ok(()));
    assert_eq!(hd.active_host().await.expect("No active host").id(), HostId(1));
    assert!(host_is_in_dispatcher(&HostId(1), &hd));
}

#[fuchsia::test]
async fn add_multiple_components_succeeds() {
    // Create HostDispatcher
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
    let hd = hd_test::make_test_dispatcher(
        watch_peers_broker.new_publisher(),
        watch_peers_broker.new_registrar(),
        watch_hosts_broker.new_publisher(),
        watch_hosts_broker.new_registrar(),
    );
    let watchers_fut = join(watch_peers_broker.run(), watch_hosts_broker.run()).map(|_| ());
    fasync::Task::spawn(watchers_fut).detach();

    // Start HostWatcher client/server
    let (host_watcher_proxy, host_watcher_stream) =
        fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_bluetooth_sys::HostWatcherMarker>();
    let host_watcher_fut = crate::host_watcher::run(hd.clone(), host_watcher_stream);
    let _host_watcher_task = fasync::Task::spawn(host_watcher_fut);

    // Create Receiver client/server
    let (receiver_client, receiver_stream) = endpoints::create_proxy_and_stream::<ReceiverMarker>();
    let _receiver_task = fasync::Task::spawn(run_receiver_server(hd.clone(), receiver_stream));

    // Create mock Host server
    let (host_client, host_server) = endpoints::create_request_stream::<HostMarker>();
    let _run_host_task = fasync::Task::spawn(handle_host_requests(HostId(1), host_server));

    // Add a host to dispatcher using host.Receiver API
    let add_host_result = receiver_client.add_host(host_client);
    assert_matches!(add_host_result, Ok(()));
    assert_eq!(hd.active_host().await.expect("No active host").id(), HostId(1));
    let mut hosts = host_watcher_proxy.watch().await.expect("Watch timed out");
    assert_eq!(hosts.len(), 1);
    assert_matches!(hosts[0].active, Some(true));

    // Create second mock Host server
    let (host_client2, host_server2) = endpoints::create_request_stream::<HostMarker>();
    let _run_host_task2 = fasync::Task::spawn(handle_host_requests(HostId(2), host_server2));

    // Add another host to dispatcher using host.Receiver API
    let add_host_result2 = receiver_client.add_host(host_client2);
    assert_matches!(add_host_result2, Ok(()));
    hosts = host_watcher_proxy.watch().await.expect("Watch timed out");
    assert_eq!(hosts.len(), 2);
    hosts.sort_by(|a, b| a.id.unwrap().value.cmp(&b.id.unwrap().value));
    assert_eq!(hosts[0].id, Some(HostId(1).into()));
    assert_eq!(hosts[0].active, Some(true));
    assert_eq!(hosts[1].id, Some(HostId(2).into()));
    assert_eq!(hosts[1].active, Some(false));
}
