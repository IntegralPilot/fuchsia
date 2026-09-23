// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This entire library is actually only used to create a test binary, so mark the entire module to
// only build in test configuration
#![cfg(test)]

use anyhow::{Context, Error, format_err};
use bt_test_harness::emulator::{
    self, add_bredr_peer, add_le_peer, default_bredr_peer, default_le_peer,
};
use bt_test_harness::host::{HostHarness, expectation as host_expectation};
use bt_test_harness::host_realm::HostRealm;
use fidl::endpoints::Proxy;
use fidl_fuchsia_bluetooth::{self as fbt, DeviceClass, MAJOR_DEVICE_CLASS_TOY};
use fidl_fuchsia_bluetooth_host::{self as _, HostStartDiscoveryRequest};
use fidl_fuchsia_bluetooth_sys::{self as fsys, TechnologyType};
use fidl_fuchsia_hardware_bluetooth::{HciError, PeerProxy};
use fuchsia_async::TimeoutExt;
use fuchsia_bluetooth::constants::INTEGRATION_TIMEOUT;
use fuchsia_bluetooth::expectation::asynchronous::{ExpectableExt, ExpectableStateExt};
use fuchsia_bluetooth::expectation::{self, peer};
use fuchsia_bluetooth::types::{Address, HostInfo, PeerId};
use hci_emulator_client::Emulator;
use std::sync::Arc;

static ADV_DATA: &'static [u8] = &[
    // Flags field set to "general discoverable"
    0x02, 0x01, 0x02, // Complete local name set to "Fake"
    0x05, 0x09, 'F' as u8, 'a' as u8, 'k' as u8, 'e' as u8,
];

async fn wait_for_test_peer(
    harness: HostHarness,
    address: &Address,
) -> Result<(PeerId, PeerProxy), Error> {
    let fut =
        add_le_peer(harness.aux().as_ref(), default_le_peer(&address), Some(ADV_DATA.to_vec()));
    let proxy = fut.await.unwrap();

    // Start discovery and let bt-host process the fake LE peer.
    let host = harness.aux().host.clone();
    let (_discovery_proxy, token) = fidl::endpoints::create_proxy();
    host.start_discovery(HostStartDiscoveryRequest { token: Some(token), ..Default::default() })
        .unwrap();

    let le_dev = peer::address(address.clone());
    let _ = host_expectation::peer(&harness, le_dev).await.unwrap();

    let peer_id = harness
        .write_state()
        .peers()
        .iter()
        .find(|(_, p)| p.address == *address)
        .ok_or_else(|| format_err!("could not find peer with address: {}", address))
        .unwrap()
        .0
        .clone();
    Ok((peer_id, proxy))
}

/// Tests that creating and destroying a fake HCI device creates and destroys a bt-host component.
#[test_harness::run_singlethreaded_test]
async fn test_lifecycle(_: ()) {
    let test_components = [
        "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm",
        "fuchsia-pkg://fuchsia.com/sunstone-host-integration-tests#meta/bt-host-integration-tests-component.cm",
    ];
    let realm = Arc::new(HostRealm::create_with_candidates(test_components).await.unwrap());

    // Create and publish an HCI device after HostRealm::create
    let dev_dir = realm.dev().unwrap();
    let mut emulator = Emulator::create(dev_dir).await.unwrap();
    let device_path =
        emulator.publish_and_wait_for_device_path(Emulator::default_settings()).await.unwrap();

    // Create bt-host component in HostRealm after device is published
    let host =
        HostRealm::create_bt_host_in_collection(&realm, &device_path).await.unwrap().into_proxy();
    let info: HostInfo = host
        .watch_state()
        .await
        .context("Is bt-gap running? If so, try stopping it and re-running these tests")
        .unwrap()
        .try_into()
        .unwrap();

    // Default address
    let address = Address::Public([0, 0, 0, 0, 0, 0]);
    assert_eq!(info.addresses.as_slice(), &[address]);

    // Remove the bt-hci device and check that the test device is also destroyed
    emulator.destroy_and_wait().await.unwrap();

    // Check that the bt-host component is also destroyed
    let _: (fidl::Signals, fidl::Signals) =
        futures::future::try_join(host.as_channel().on_closed(), host.as_channel().on_closed())
            .on_timeout(INTEGRATION_TIMEOUT, || panic!("timed out waiting for device to close"))
            .await
            .unwrap();
}

/// Tests that the bt-host component assigns the local name to "fuchsia" when initialized.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_default_local_name(harness: HostHarness) {
    const NAME: &str = "fuchsia";

    let _ = harness
        .when_satisfied(emulator::expectation::local_name_is(NAME), INTEGRATION_TIMEOUT)
        .await
        .unwrap();
    let _ =
        host_expectation::host_state(&harness, expectation::host_driver::name(NAME)).await.unwrap();
}

/// Tests that the local name assigned to a bt-host is reflected in `AdapterState` and propagated
/// down to the controller.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_set_local_name(harness: HostHarness) {
    const NAME: &str = "test1234";
    let proxy = harness.aux().host.clone();
    let result = proxy.set_local_name(NAME).await.unwrap();
    assert_eq!(Ok(()), result);

    let _ = harness
        .when_satisfied(emulator::expectation::local_name_is(NAME), INTEGRATION_TIMEOUT)
        .await
        .unwrap();
    let _ =
        host_expectation::host_state(&harness, expectation::host_driver::name(NAME)).await.unwrap();
}

/// Tests that the device class assigned to a bt-host gets propagated down to the controller.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_set_device_class(harness: HostHarness) {
    let device_class = DeviceClass { value: MAJOR_DEVICE_CLASS_TOY + 4 };
    let proxy = harness.aux().host.clone();
    let result = proxy.set_device_class(&device_class).await.unwrap();
    assert_eq!(Ok(()), result);

    let _ = harness
        .when_satisfied(emulator::expectation::device_class_is(device_class), INTEGRATION_TIMEOUT)
        .await
        .unwrap();
}

/// Tests that Host state updates when discoverable mode is turned on.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_discoverable(harness: HostHarness) {
    let proxy = harness.aux().host.clone();

    // Disabling discoverable mode when not discoverable should succeed.
    let result = proxy.set_discoverable(false).await.unwrap();
    assert_eq!(Ok(()), result);

    // Enable discoverable mode.
    let result = proxy.set_discoverable(true).await.unwrap();
    assert_eq!(Ok(()), result);
    let _ = host_expectation::host_state(&harness, expectation::host_driver::discoverable(true))
        .await
        .unwrap();

    // Disable discoverable mode
    let result = proxy.set_discoverable(false).await.unwrap();
    assert_eq!(Ok(()), result);
    let _ = host_expectation::host_state(&harness, expectation::host_driver::discoverable(false))
        .await
        .unwrap();

    // Disabling discoverable mode when not discoverable should succeed.
    let result = proxy.set_discoverable(false).await.unwrap();
    assert_eq!(Ok(()), result);
}

/// Tests that Host state updates when discovery is started and stopped.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_discovery(harness: HostHarness) {
    let proxy = harness.aux().host.clone();

    // Start discovery. "discovering" should get set to true.
    let (discovery_proxy, token) = fidl::endpoints::create_proxy();
    proxy
        .start_discovery(HostStartDiscoveryRequest { token: Some(token), ..Default::default() })
        .unwrap();
    let _ = host_expectation::host_state(&harness, expectation::host_driver::discovering(true))
        .await
        .unwrap();

    let address = Address::Random([1, 0, 0, 0, 0, 0]);
    let fut =
        add_le_peer(harness.aux().as_ref(), default_le_peer(&address), Some(ADV_DATA.to_vec()));
    let _peer = fut.await.unwrap();

    // The host should discover a fake peer.
    let _ = host_expectation::peer(&harness, peer::name("Fake").and(peer::address(address)))
        .await
        .unwrap();

    // Stop discovery. "discovering" should get set to false.
    discovery_proxy.stop().unwrap();
    let _ = discovery_proxy.on_closed().await.unwrap();
    let _ = host_expectation::host_state(&harness, expectation::host_driver::discovering(false))
        .await
        .unwrap();
}

/// Tests that closing Host cancels all operations.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_close(harness: HostHarness) {
    // Enable all procedures.
    let proxy = harness.aux().host.clone();
    let (_discovery_proxy, token) = fidl::endpoints::create_proxy();
    proxy
        .start_discovery(HostStartDiscoveryRequest { token: Some(token), ..Default::default() })
        .unwrap();
    let result = proxy.set_discoverable(true).await.unwrap();
    assert_eq!(Ok(()), result);

    let active_state = expectation::host_driver::discoverable(true)
        .and(expectation::host_driver::discovering(true));
    let _ = host_expectation::host_state(&harness, active_state).await.unwrap();

    // Shutdown should cancel these procedures.
    proxy.shutdown().unwrap();

    let closed_state_update = expectation::host_driver::discoverable(false)
        .and(expectation::host_driver::discovering(false));

    let _ = host_expectation::host_state(&harness, closed_state_update).await.unwrap();
}

/// Tests that bt-host discovers BR/EDR and LE peers.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_watch_peers(harness: HostHarness) {
    // `HostHarness` internally calls `Host.WatchPeers()` to monitor peers and satisfy peer
    // expectations. `harness.peers()` represents the local cache monitored using this method.
    // Peers should be initially empty.
    assert_eq!(0, harness.write_state().peers().len());

    // Calling `Host.WatchPeers()` directly will hang since the harness already calls this
    // internally. We issue our own request and verify that it gets satisfied later.

    // Add a LE and a BR/EDR peer with the given addresses.
    let le_peer_address = Address::Random([1, 0, 0, 0, 0, 0]);
    let bredr_peer_address = Address::Public([2, 0, 0, 0, 0, 0]);

    let fut = add_le_peer(
        harness.aux().as_ref(),
        default_le_peer(&le_peer_address),
        Some(ADV_DATA.to_vec()),
    );
    let _le_peer = fut.await.unwrap();

    let fut = add_bredr_peer(harness.aux().as_ref(), default_bredr_peer(&bredr_peer_address));
    let bredr_peer = fut.await.unwrap();
    let _ = bredr_peer.set_device_class(MAJOR_DEVICE_CLASS_TOY).await.unwrap();

    // At this stage the fake peers are registered with the emulator but bt-host does not know about
    // them yet. Check that `watch_fut` is still unsatisfied.
    assert_eq!(0, harness.write_state().peers().len());

    // Wait for all fake devices to be discovered.
    let proxy = harness.aux().host.clone();
    let (_discovery_proxy, token) = fidl::endpoints::create_proxy();
    proxy
        .start_discovery(HostStartDiscoveryRequest { token: Some(token), ..Default::default() })
        .unwrap();
    let expected_le =
        peer::address(le_peer_address).and(peer::technology(TechnologyType::LowEnergy));
    let expected_bredr =
        peer::address(bredr_peer_address).and(peer::technology(TechnologyType::Classic));

    let _ = host_expectation::peer(&harness, expected_le).await.unwrap();
    let _ = host_expectation::peer(&harness, expected_bredr).await.unwrap();
    assert_eq!(2, harness.write_state().peers().len());
}

// Tests that bt-host discovers two LE peers. One connects successfully while the other fails.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_connect(harness: HostHarness) {
    let address1 = Address::Random([1, 0, 0, 0, 0, 0]);
    let address2 = Address::Random([2, 0, 0, 0, 0, 0]);
    let fut =
        add_le_peer(harness.aux().as_ref(), default_le_peer(&address1), Some(ADV_DATA.to_vec()));
    let _peer1 = fut.await.unwrap();
    let fut =
        add_le_peer(harness.aux().as_ref(), default_le_peer(&address2), Some(ADV_DATA.to_vec()));
    let peer2 = fut.await.unwrap();

    // Configure `peer2` to return an error for the connection attempt.
    peer2.assign_connection_status(HciError::ConnectionTimeout).await.unwrap();

    let proxy = harness.aux().host.clone();

    // Start discovery and let bt-host process the fake devices.
    let (_discovery_proxy, token) = fidl::endpoints::create_proxy();
    proxy
        .start_discovery(HostStartDiscoveryRequest { token: Some(token), ..Default::default() })
        .unwrap();

    let _ = host_expectation::peer(&harness, peer::address(address1)).await.unwrap();
    let _ = host_expectation::peer(&harness, peer::address(address2)).await.unwrap();

    let peers = harness.write_state().peers().clone();
    assert_eq!(2, peers.len());

    // Obtain bt-host assigned IDs of the devices.
    let success_id = peers
        .iter()
        .find(|x| x.1.address == address1)
        .ok_or_else(|| format_err!("success peer not found"))
        .unwrap()
        .0
        .clone();
    let failure_id = peers
        .iter()
        .find(|x| x.1.address == address2)
        .ok_or_else(|| format_err!("error peer not found"))
        .unwrap()
        .0
        .clone();
    let success_id: fbt::PeerId = success_id.into();
    let failure_id: fbt::PeerId = failure_id.into();

    // Connecting to the failure peer should result in an error.
    let status = proxy.connect(&failure_id).await.unwrap();
    assert_eq!(Err(fsys::Error::Failed), status);

    // Connecting to the success peer should return success and the peer should become connected.
    let status = proxy.connect(&success_id).await.unwrap();
    assert_eq!(Ok(()), status);

    let connected = peer::identifier(success_id.into()).and(peer::connected(true));
    let _ = host_expectation::peer(&harness, connected).await.unwrap();
}

// TODO(https://fxbug.dev/42088425): Add a test for disconnect failure when a connection attempt is
// outgoing, provided that we can provide a manner of doing so that will not flake.

/// Disconnecting from an unknown device should succeed.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_disconnect_unknown_device(harness: HostHarness) {
    let unknown_id = PeerId(0).into();
    let fut = harness.aux().host.disconnect(&unknown_id);
    let status = fut.await.unwrap();
    assert_eq!(Ok(()), status);
}

/// Disconnecting from a known, unconnected device should succeed.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_disconnect_unconnected_device(harness: HostHarness) {
    let address = Address::Random([1, 0, 0, 0, 0, 0]);
    let (id, _proxy) = wait_for_test_peer(harness.clone(), &address).await.unwrap();
    let id = id.into();
    let fut = harness.aux().host.disconnect(&id);
    let status = fut.await.unwrap();
    assert_eq!(Ok(()), status);
}

/// Disconnecting from a connected device should succeed and result in the device being disconnected
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_disconnect_connected_device(harness: HostHarness) {
    let address = Address::Random([1, 0, 0, 0, 0, 0]);
    let (id, _proxy) = wait_for_test_peer(harness.clone(), &address).await.unwrap();
    let id = id.into();
    let proxy = harness.aux().host.clone();

    let status = proxy.connect(&id).await.unwrap();
    assert_eq!(Ok(()), status);

    let connected = peer::address(address).and(peer::connected(true));
    let disconnected = peer::address(address).and(peer::connected(false));

    let _ = host_expectation::peer(&harness, connected).await.unwrap();
    let status = proxy.disconnect(&id).await.unwrap();
    assert_eq!(Ok(()), status);

    let _ = host_expectation::peer(&harness, disconnected).await.unwrap();
}

/// Forgetting a connected device should succeed and result in the device being removed.
#[test_harness::run_singlethreaded_test(
    test_component = "fuchsia-pkg://fuchsia.com/bt-host-integration-tests#meta/bt-host-integration-tests-component.cm"
)]
async fn test_forget(harness: HostHarness) {
    let address = Address::Random([1, 0, 0, 0, 0, 0]);
    let (id, _proxy) = wait_for_test_peer(harness.clone(), &address).await.unwrap();
    let id = id.into();
    let proxy = harness.aux().host.clone();

    // Wait for fake peer to be discovered (`wait_for_test_peer` starts discovery).
    let expected_peer = expectation::peer::address(address);
    let _ = host_expectation::peer(&harness, expected_peer.clone()).await.unwrap();

    // Connecting to the peer should return success and the peer should become connected.
    let status = proxy.connect(&id).await.unwrap();
    assert_eq!(Ok(()), status);
    let _ =
        host_expectation::peer(&harness, expected_peer.and(peer::connected(true))).await.unwrap();

    // Forgetting the peer should result in its removal.
    let status = proxy.forget(&id).await.unwrap();
    assert_eq!(Ok(()), status);
    host_expectation::no_peer(&harness, id.into()).await.unwrap();

    // TODO(https://fxbug.dev/42087836): Test that the link closes by querying fake HCI.
}
