// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl::endpoints;
use fidl_fuchsia_bluetooth_host::HostRequest;
use fidl_fuchsia_bluetooth_sys::{
    AccessMarker, AccessSetConnectionPolicyRequest, ProcedureTokenMarker,
};
use fuchsia_bluetooth::types::pairing_options::{BondableMode, PairingOptions, SecurityLevel};
use fuchsia_bluetooth::types::{HostId, PeerId, Technology};
use fuchsia_sync::RwLock;
use futures::future;
use futures::stream::{StreamExt, TryStreamExt};
use std::sync::Arc;

use crate::services::access;
use crate::{host_device, host_dispatcher};

#[fuchsia::test]
async fn initiate_pairing() {
    let dispatcher = host_dispatcher::test::make_simple_test_dispatcher();

    let (host_server, _, _gatt_server, _bonding) =
        host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(42), &dispatcher)
            .await
            .unwrap();

    let (client, server) = endpoints::create_proxy_and_stream::<AccessMarker>();
    let run_access = access::run(dispatcher, server);

    // The parameters to send to access.Pair()
    let req_id = PeerId(128);
    let req_opts = PairingOptions {
        transport: Technology::LE,
        le_security_level: SecurityLevel::Authenticated,
        bondable: BondableMode::NonBondable,
    };

    let req_opts_ = req_opts.clone();

    let make_request = async move {
        let response = client.pair(&req_id.into(), &req_opts_.into()).await;
        assert_matches!(response, Ok(Ok(())));
        // This terminating will drop the access client, which causes the access stream to
        // terminate. This will cause run_access to terminate which drops the host dispatcher, which
        // closes the host channel and will cause run_host to terminate
        Ok(())
    };

    let run_host = async move {
        host_server.try_for_each(move |req| {
            assert_matches!(&req, HostRequest::Pair { id, options, responder: _ } if *id == req_id.into() && PairingOptions::from(options) == req_opts);
            if let HostRequest::Pair { id: _, options: _, responder } = req {
                assert_matches!(responder.send(Ok(())), Ok(()));
            }
            future::ok(())
        }).await.map_err(|e| e.into())
    };

    future::try_join3(make_request, run_host, run_access)
        .await
        .map(|_: ((), (), ())| ())
        .expect("successful execution")
}

// Test that we can start discovery on a host then migrate that discovery session onto a different
// host when the original host is deactivated
#[fuchsia::test]
async fn discovery_over_adapter_change() {
    // Create mock host dispatcher
    let hd = host_dispatcher::test::make_simple_test_dispatcher();

    // Add Host #1 to dispatcher and make active
    let (host_server_1, host_1, _gatt_server_1, _bonding) =
        host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(1), &hd)
            .await
            .unwrap();
    let host_info_1 = Arc::new(RwLock::new(host_1.info()));

    hd.set_active_host(HostId(1)).expect("can set active host");

    // Add Host #2 to dispatcher
    let (host_server_2, host_2, _gatt_server_2, _bonding) =
        host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(2), &hd)
            .await
            .unwrap();
    let host_info_2 = Arc::new(RwLock::new(host_2.info()));

    // Create access server future
    let (access_client, access_server) = endpoints::create_proxy_and_stream::<AccessMarker>();
    let run_access = access::run(hd.clone(), access_server);

    // Create access client future
    let (discovery_session, discovery_session_server) = endpoints::create_proxy();
    let run_client = async move {
        // Request discovery on active Host #1
        let response = access_client.start_discovery(discovery_session_server).await;
        assert_matches!(response, Ok(Ok(())));

        // Assert that Host #1 is now marked as discovering
        let _ = host_1
            .clone()
            .refresh_test_host_info()
            .await
            .expect("did not receive Host #1 info update");
        let is_discovering = host_1.info().discovering.clone();
        assert!(is_discovering);

        // Deactivate Host #1
        hd.rm_device(host_1.path()).await;

        // Assert that Host #2 is now marked as discovering
        let _ = host_2
            .clone()
            .refresh_test_host_info()
            .await
            .expect("did not receive Host #2 info update");
        let is_discovering = host_2.info().discovering.clone();
        assert!(is_discovering);

        // Drop discovery session, which contains an internal reference to the dispatcher state,
        // so that the other futures may terminate. Then, assert Host #2 stops discovering.
        drop(discovery_session);

        let _ = host_2
            .clone()
            .refresh_test_host_info()
            .await
            .expect("did not receive Host #2 info update");
        let is_discovering = host_2.info().discovering.clone();
        assert!(!is_discovering);

        Ok(())
    };

    future::try_join4(
        run_client,
        run_access,
        host_device::test::run_discovery_host_server(host_server_1, host_info_1),
        host_device::test::run_discovery_host_server(host_server_2, host_info_2),
    )
    .await
    .map(|_: ((), (), (), ())| ())
    .expect("successful execution")
}

#[fuchsia::test]
async fn make_active_host_discoverable() {
    let dispatcher = host_dispatcher::test::make_simple_test_dispatcher();

    let (host_server, _, _gatt_server, _bonding) =
        host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(49), &dispatcher)
            .await
            .unwrap();

    let (client, server) = endpoints::create_proxy_and_stream::<AccessMarker>();
    let run_access = access::run(dispatcher, server);

    let make_request = async move {
        let (token, server) = fidl::endpoints::create_sync_proxy::<ProcedureTokenMarker>();
        let response = client.make_discoverable(server).await;
        assert_matches!(response, Ok(Ok(())));
        // Immediately dropping the token should trigger a SetDiscoverable request to turn it off.
        drop(token);
        Ok(())
    };

    let mut expected_enabled = true;
    let run_host = async move {
        host_server
            .try_for_each(move |req| {
                let HostRequest::SetDiscoverable { enabled, responder } = req else {
                    panic!("Expected HostRequest::SetDiscoverable");
                };
                assert_matches!(responder.send(Ok(())), Ok(()));
                assert_eq!(enabled, expected_enabled);
                // We first expect the Discoverable request with `enabled=true`. Then when the token
                // is dropped, we expect it to be false.
                expected_enabled = !expected_enabled;
                future::ok(())
            })
            .await
            .map_err(|e| e.into())
    };

    future::try_join3(make_request, run_host, run_access)
        .await
        .map(|_: ((), (), ())| ())
        .expect("successful execution");
}

#[fuchsia::test]
async fn toggle_connectable_of_active_host() {
    let dispatcher = host_dispatcher::test::make_simple_test_dispatcher();

    let (mut host_server, _, _gatt_server, _bonding) =
        host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(49), &dispatcher)
            .await
            .unwrap();

    let (client, server) = endpoints::create_proxy_and_stream::<AccessMarker>();
    let _run_access_task = fuchsia_async::Task::spawn(access::run(dispatcher, server));

    let (token, server) = fidl::endpoints::create_sync_proxy::<ProcedureTokenMarker>();
    let policy = AccessSetConnectionPolicyRequest {
        suppress_bredr_connections: Some(server),
        ..Default::default()
    };
    let connection_policy_fut = client.set_connection_policy(policy);
    // Expect the Host server to receive a request to disable `connectable'.
    match host_server.next().await.expect("valid_request") {
        Ok(HostRequest::SetConnectable { enabled: false, responder }) => {
            responder.send(Ok(())).expect("valid response");
        }
        x => panic!("Expected SetConnectable, got: {x:?}"),
    }

    // Expect the connection policy request to resolve successfully.
    let result = connection_policy_fut.await;
    assert_matches!(result, Ok(Ok(_)));

    // Immediately dropping the token should result in a request to re-enable connectable.
    drop(token);
    match host_server.next().await.expect("valid_request") {
        Ok(HostRequest::SetConnectable { enabled: true, responder }) => {
            responder.send(Ok(())).expect("valid response");
        }
        x => panic!("Expected SetConnectable, got: {x:?}"),
    }
}
