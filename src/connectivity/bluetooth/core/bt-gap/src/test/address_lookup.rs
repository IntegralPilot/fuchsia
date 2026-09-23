// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints;
use fidl_fuchsia_bluetooth::Appearance;
use fidl_fuchsia_bluetooth_sys::{
    AddressLookupLookupRequest, AddressLookupMarker, LookupError, TechnologyType,
};
use fuchsia_bluetooth::types::{Address, Peer, PeerId};

use crate::host_dispatcher;
use crate::services::address_lookup;

fn peer(id: PeerId, address: Address) -> Peer {
    Peer {
        id,
        address,
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

fn make_request(peer_id: Option<PeerId>) -> AddressLookupLookupRequest {
    AddressLookupLookupRequest { peer_id: peer_id.map(Into::into), ..Default::default() }
}

#[fuchsia::test]
async fn get_address_of_peer() {
    // Create mock host dispatcher
    let hd = host_dispatcher::test::make_simple_test_dispatcher();
    hd.on_device_updated(peer(PeerId(1), Address::Public([1, 2, 3, 4, 5, 6]))).await;
    hd.on_device_updated(peer(PeerId(2), Address::Random([6, 5, 4, 3, 2, 1]))).await;

    let (client, stream) = endpoints::create_proxy_and_stream::<AddressLookupMarker>();
    let server_task = address_lookup::run(hd.clone(), stream);

    let client_task = async move {
        let addr = client.lookup(&make_request(Some(PeerId(1)))).await.unwrap().unwrap();
        assert_eq!(addr, Address::Public([1, 2, 3, 4, 5, 6]).into());
        let addr = client.lookup(&make_request(Some(PeerId(2)))).await.unwrap().unwrap();
        assert_eq!(addr, Address::Random([6, 5, 4, 3, 2, 1]).into());
        let err = client.lookup(&make_request(Some(PeerId(3)))).await.unwrap().unwrap_err();
        assert_eq!(err, LookupError::NotFound);
        let err = client.lookup(&make_request(None)).await.unwrap().unwrap_err();
        assert_eq!(err, LookupError::MissingArgument);
        Ok(())
    };

    futures::try_join!(server_task, client_task).expect("Client or server failure");
}
