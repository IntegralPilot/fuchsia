// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{format_err, Error};
use bt_test_harness::access::{expectation, AccessHarness};
use bt_test_harness::core_realm::DEFAULT_TEST_DEVICE_NAME;
use bt_test_harness::host_watcher::{activate_fake_host, HostWatcherHarness};
use fidl_fuchsia_bluetooth_sys::ProcedureTokenProxy;
use fidl_fuchsia_hardware_bluetooth::{
    AdvertisingData, PeerParameters, PeerProxy, PeerSetLeAdvertisementRequest,
};
use fuchsia_bluetooth::constants::INTEGRATION_TIMEOUT;
use fuchsia_bluetooth::expectation::asynchronous::{ExpectableExt, ExpectableStateExt};
use fuchsia_bluetooth::types::Address;
use hci_emulator_client::Emulator;

async fn create_le_peer(hci: &Emulator, address: Address) -> Result<PeerProxy, Error> {
    let (peer, remote) = fidl::endpoints::create_proxy();
    let peer_params = PeerParameters {
        address: Some(address.into()),
        connectable: Some(true),
        channel: Some(remote),
        ..Default::default()
    };
    let _ = hci
        .emulator()
        .add_low_energy_peer(peer_params)
        .await?
        .map_err(|e| format_err!("Failed to register fake peer: {:#?}", e))?;

    let request = PeerSetLeAdvertisementRequest {
        le_address: Some(address.into()),
        advertisement: Some(AdvertisingData {
            data: Some(
                vec![0x02, 0x01, 0x02], // Flags field set to "general discoverable"
            ),
            __source_breaking: fidl::marker::SourceBreaking,
        }),
        scan_response: Some(AdvertisingData {
            data: None,
            __source_breaking: fidl::marker::SourceBreaking,
        }),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    let _ = peer.set_le_advertisement(&request).await.unwrap();

    Ok(peer)
}

async fn start_discovery(access: &AccessHarness) -> Result<ProcedureTokenProxy, Error> {
    // We create a capability to capture the discovery token, and pass it to the access provider
    // Discovery will stop once we drop this token
    let (token, token_server) = fidl::endpoints::create_proxy();
    let fidl_response = access.aux().start_discovery(token_server);
    fidl_response
        .await?
        .map_err(|sys_err| format_err!("Error calling StartDiscovery(): {:?}", sys_err))?;
    Ok(token)
}

async fn make_discoverable(access: &AccessHarness) -> Result<ProcedureTokenProxy, Error> {
    // We create a capability to capture the discoverable token, and pass it to the access provider
    // Discoverable will stop once we drop this token
    let (token, token_server) = fidl::endpoints::create_proxy();
    let fidl_response = access.aux().make_discoverable(token_server);
    fidl_response
        .await?
        .map_err(|sys_err| format_err!("Error calling StartDiscoverable(): {:?}", sys_err))?;
    Ok(token)
}

// Test that we can
//  * Enable discovery via fuchsia.bluetooth.sys.Access.StartDiscovery()
//  * Receive peer information via fuchsia.bluetooth.sys.Access.WatchPeers()
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-access-integration-tests#meta/bt-access-integration-tests-component.cm"
)]
async fn test_watch_peers((access, host_watcher): (AccessHarness, HostWatcherHarness)) {
    let (_host, mut hci) = activate_fake_host(host_watcher).await.unwrap();

    let first_address = Address::Random([1, 0, 0, 0, 0, 0]);
    let second_address = Address::Public([2, 0, 0, 0, 0, 0]);
    let _first_peer = create_le_peer(&hci, first_address).await.unwrap();
    let _discovery_token = start_discovery(&access).await.unwrap();

    // We should be notified of the first peer
    let state = access
        .when_satisfied(expectation::peer_with_address(first_address), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    // We should not have seen the second peer yet
    assert!(!expectation::peer_with_address(second_address).satisfied(&state));

    // Once the second peer is added, we should see it
    let _second_peer = create_le_peer(&hci, second_address).await.unwrap();
    let _state = access
        .when_satisfied(expectation::peer_with_address(second_address), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    hci.destroy_and_wait().await.unwrap();
}

#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-access-integration-tests#meta/bt-access-integration-tests-component.cm"
)]
async fn test_disconnect((access, host_watcher): (AccessHarness, HostWatcherHarness)) {
    let (_host, mut hci) = activate_fake_host(host_watcher.clone()).await.unwrap();

    let peer_address = Address::Random([6, 5, 0, 0, 0, 0]);
    let _peer = create_le_peer(&hci, peer_address).await.unwrap();
    let _discovery = start_discovery(&access).await.unwrap();

    let state = access
        .when_satisfied(expectation::peer_with_address(peer_address), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    // We can safely unwrap here as this is guarded by the previous expectation
    let peer_id = state.peers.values().find(|p| p.address == peer_address).unwrap().id;

    let fidl_response = access.aux().connect(&mut peer_id.into());
    fidl_response
        .await
        .unwrap()
        .map_err(|sys_err| format_err!("Error calling Connect(): {:?}", sys_err))
        .unwrap();

    let _ = access
        .when_satisfied(expectation::peer_connected(peer_id, true), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    let fidl_response = access.aux().disconnect(&mut peer_id.into());
    fidl_response
        .await
        .unwrap()
        .map_err(|sys_err| format_err!("Error calling Disconnect(): {:?}", sys_err))
        .unwrap();

    let _ = access
        .when_satisfied(expectation::peer_connected(peer_id, false), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    hci.destroy_and_wait().await.unwrap();
}

// Test that we can
//  * Set local name via fuchsia.bluetooth.sys.Access.SetLocalName()
//  * Receive host information via fuchsia.bluetooth.sys.HostWatcher.Watch()
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-access-integration-tests#meta/bt-access-integration-tests-component.cm"
)]
async fn test_set_local_name((access, host_watcher): (AccessHarness, HostWatcherHarness)) {
    let (_host, mut hci) = activate_fake_host(host_watcher.clone()).await.unwrap();

    let _ = host_watcher
        .when_satisfied(expectation::host_with_name(DEFAULT_TEST_DEVICE_NAME), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    let expected_name = "bt-integration-test";
    access.aux().set_local_name(expected_name).unwrap();

    let _ = host_watcher
        .when_satisfied(expectation::host_with_name(expected_name), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    hci.destroy_and_wait().await.unwrap();
}

// Test that we can
//  * Enable discovery via fuchsia.bluetooth.sys.Access.StartDiscovery()
//  * Disable discovery by dropping our token
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-access-integration-tests#meta/bt-access-integration-tests-component.cm"
)]
async fn test_discovery((access, host_watcher): (AccessHarness, HostWatcherHarness)) {
    let (host, mut hci) = activate_fake_host(host_watcher.clone()).await.unwrap();
    let discovery_token = start_discovery(&access).await.unwrap();

    // We should now be discovering
    let _ = host_watcher
        .when_satisfied(expectation::host_discovering(host, true), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    // Drop our end of the token channel
    std::mem::drop(discovery_token);

    // Since no-one else has requested discovery, we should cease discovery
    let _ = host_watcher
        .when_satisfied(expectation::host_discovering(host, false), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    hci.destroy_and_wait().await.unwrap();
}

// Test that we can
//  * Enable discoverable via fuchsia.bluetooth.sys.Access.StartDiscoverable()
//  * Disable discoverable by dropping our token
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-access-integration-tests#meta/bt-access-integration-tests-component.cm"
)]
async fn test_discoverable((access, host_watcher): (AccessHarness, HostWatcherHarness)) {
    let (host, mut hci) = activate_fake_host(host_watcher.clone()).await.unwrap();
    let discoverable_token = make_discoverable(&access).await.unwrap();

    // We should now be discoverable
    let _ = host_watcher
        .when_satisfied(expectation::host_discoverable(host, true), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    // Drop our end of the token channel
    std::mem::drop(discoverable_token);

    // Since no-one else has requested discoverable, we should cease discoverable
    let _ = host_watcher
        .when_satisfied(expectation::host_discoverable(host, false), INTEGRATION_TIMEOUT)
        .await
        .unwrap();

    hci.destroy_and_wait().await.unwrap();
}
