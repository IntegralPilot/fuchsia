// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::PeerId;
use bt_common::core::{Address, AddressType};
use bt_gatt::pii::GetPeerAddr;
use bt_gatt::{Result, types};
use fidl_fuchsia_bluetooth_sys::{AddressLookupLookupRequest, AddressLookupProxy, LookupError};
use fuchsia_sync::Mutex;
use std::collections::HashMap;

use crate::to_fidl_peer_id;

/// An implementation of `GetPeerAddr` that uses the `fuchsia.bluetooth.sys.AddressLookup`
/// FIDL service.
pub struct FuchsiaPeerAddr {
    proxy: AddressLookupProxy,
    cache: Mutex<HashMap<PeerId, (Address, AddressType)>>,
}

fn from_fidl_address(addr: fidl_fuchsia_bluetooth::Address) -> (Address, AddressType) {
    let addr_type = match addr.type_ {
        fidl_fuchsia_bluetooth::AddressType::Public => AddressType::Public,
        fidl_fuchsia_bluetooth::AddressType::Random => AddressType::Random,
    };
    (addr.bytes.into(), addr_type)
}

impl FuchsiaPeerAddr {
    pub fn new(proxy: AddressLookupProxy) -> Self {
        Self { proxy, cache: Mutex::new(HashMap::new()) }
    }
}

impl GetPeerAddr for FuchsiaPeerAddr {
    async fn get_peer_address(&self, peer_id: PeerId) -> Result<(Address, AddressType)> {
        // Check for the address in the cache. The lock is released after this scope.
        if let Some(addr) = self.cache.lock().get(&peer_id) {
            return Ok(*addr);
        }

        // If not in the cache, perform the FIDL call. The lock is not held.
        let result = self
            .proxy
            .lookup(&AddressLookupLookupRequest {
                peer_id: Some(to_fidl_peer_id(&peer_id)),
                ..Default::default()
            })
            .await;

        match result {
            Ok(Ok(addr)) => {
                let addr = from_fidl_address(addr);
                // Lock again to insert the new address. The lock is released after this statement.
                let _ = self.cache.lock().insert(peer_id, addr);
                Ok(addr)
            }
            Ok(Err(LookupError::NotFound)) => Err(types::Error::PeerNotRecognized(peer_id)),
            Ok(Err(e)) => Err(types::Error::from(format!("AddressLookup error: {:?}", e))),
            Err(fidl_err) => Err(types::Error::other(fidl_err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth_sys::AddressLookupRequest;
    use fuchsia_async as fasync;
    use futures::pin_mut;
    use futures::stream::StreamExt;
    use futures::task::Poll;

    #[test]
    fn test_get_peer_address_success() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, mut stream) =
            create_proxy_and_stream::<fidl_fuchsia_bluetooth_sys::AddressLookupMarker>();

        let peer_id = PeerId(123);
        let addr = fidl_fuchsia_bluetooth::Address {
            type_: fidl_fuchsia_bluetooth::AddressType::Random,
            bytes: [1, 2, 3, 4, 5, 6],
        };

        let lookup = FuchsiaPeerAddr::new(proxy);

        // First call - should be a cache miss and result in a FIDL call.
        let client_fut = lookup.get_peer_address(peer_id);
        pin_mut!(client_fut);
        assert!(exec.run_until_stalled(&mut client_fut).is_pending());

        // Handle the FIDL request from the stream.
        let stream_fut = stream.next();
        pin_mut!(stream_fut);
        let stream_result = exec.run_until_stalled(&mut stream_fut);
        let Poll::Ready(Some(Ok(AddressLookupRequest::Lookup { payload, responder }))) =
            stream_result
        else {
            panic!("Expected Lookup request, got {:?}", stream_result);
        };
        assert_eq!(peer_id.0, payload.peer_id.unwrap().value);
        responder.send(Ok(&addr.into())).unwrap();

        // Now the client future should complete.
        let client_result = exec.run_until_stalled(&mut client_fut);
        let Poll::Ready(Ok((found_addr, found_type))) = client_result else {
            panic!("Expected future to complete with Ok, got {:?}", client_result);
        };
        let (expected_addr, expected_type) = from_fidl_address(addr);
        assert_eq!(found_addr, expected_addr);
        assert_eq!(found_type, expected_type);

        // Second call - should be a cache hit.
        let client_fut2 = lookup.get_peer_address(peer_id);
        pin_mut!(client_fut2);
        let client_result2 = exec.run_until_stalled(&mut client_fut2);
        let Poll::Ready(Ok((found_addr, found_type))) = client_result2 else {
            panic!("Expected future to complete immediately from cache, got {:?}", client_result2);
        };
        assert_eq!(found_addr, expected_addr);
        assert_eq!(found_type, expected_type);

        // Verify that no FIDL request was made on the second call.
        let stream_fut = stream.next();
        pin_mut!(stream_fut);
        assert!(exec.run_until_stalled(&mut stream_fut).is_pending());
    }

    #[test]
    fn test_get_peer_address_not_found() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, mut stream) =
            create_proxy_and_stream::<fidl_fuchsia_bluetooth_sys::AddressLookupMarker>();

        let peer_id = PeerId(123);

        let lookup = FuchsiaPeerAddr::new(proxy);

        let client_fut = lookup.get_peer_address(peer_id);
        pin_mut!(client_fut);
        assert!(exec.run_until_stalled(&mut client_fut).is_pending());

        // Handle the FIDL request from the stream.
        let stream_fut = stream.next();
        pin_mut!(stream_fut);
        let stream_result = exec.run_until_stalled(&mut stream_fut);
        let Poll::Ready(Some(Ok(AddressLookupRequest::Lookup { payload, responder }))) =
            stream_result
        else {
            panic!("Expected Lookup request, got {:?}", stream_result);
        };
        assert_eq!(peer_id.0, payload.peer_id.unwrap().value);
        responder.send(Err(LookupError::NotFound)).unwrap();

        // Now the client future should complete with an error.
        let client_result = exec.run_until_stalled(&mut client_fut);
        let Poll::Ready(Err(e)) = client_result else {
            panic!("Expected future to complete with Err, got {:?}", client_result);
        };
        assert!(matches!(e, types::Error::PeerNotRecognized(id) if id == peer_id));
    }
}
