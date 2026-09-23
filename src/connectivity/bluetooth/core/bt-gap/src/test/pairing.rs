// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl::endpoints;
use fidl_fuchsia_bluetooth_sys::{
    InputCapability, OutputCapability, PairingDelegateMarker, PairingMarker,
};
use fuchsia_bluetooth::types::HostId;
use futures::stream::StreamExt;
use futures::{select, FutureExt};
use std::pin::pin;

use crate::host_dispatcher;
use crate::services::pairing;

// Test that a client can request to set the pairing delegate.
#[fuchsia::test]
async fn set_pairing_delegate() {
    // Create mock host dispatcher
    let hd = host_dispatcher::test::make_simple_test_dispatcher();

    // Add Host #1 to dispatcher and make active
    let (_host_server_1, _host_1, _gatt_server_1, _bonding) =
        host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(1), &hd)
            .await
            .unwrap();
    hd.set_active_host(HostId(1)).expect("can set active host");

    // Create the pairing server
    let (pairing_client, pairing_server) = endpoints::create_proxy_and_stream::<PairingMarker>();
    let run_pairing = pin!(pairing::run(hd.clone(), pairing_server));

    // First client to request to set the delegate should be OK.
    let input = InputCapability::None;
    let output = OutputCapability::None;
    let (delegate_client1, mut delegate_server1) =
        endpoints::create_request_stream::<PairingDelegateMarker>();
    let _ = pairing_client
        .set_pairing_delegate(input, output, delegate_client1)
        .expect("FIDL request is OK");

    // Second client can't claim the delegate. The `delegate_server2` end should subsequently close.
    let (delegate_client2, mut delegate_server2) =
        endpoints::create_request_stream::<PairingDelegateMarker>();
    let _ = pairing_client
        .set_pairing_delegate(input, output, delegate_client2)
        .expect("FIDL request is OK");

    select! {
        _pairing_result = run_pairing.fuse() => {
            panic!("`sys.Pairing` server unexpectedly terminated");
        }
        result = delegate_server1.next() => {
            panic!("First delegate should be stay open: {:?}", result);
        }
        result = delegate_server2.next() => {
            assert_matches!(result, None);
        }
    }
}
