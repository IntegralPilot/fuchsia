// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::PeerId;
use bt_common::core::{Address, AddressType};
use bt_gatt::pii::GetPeerAddr;
use bt_gatt::types::{Error, Result};
use bt_gatt_fuchsia::pii::FuchsiaPeerAddr;
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct LocalPeerAddrCache {
    cache: Arc<Mutex<HashMap<PeerId, (Address, AddressType)>>>,
}

impl LocalPeerAddrCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_peer_address(&self, id: PeerId, addr: Address, addr_type: AddressType) {
        let mut cache = self.cache.lock();
        let _ = cache.insert(id, (addr, addr_type));
    }
}

impl GetPeerAddr for LocalPeerAddrCache {
    // Note: this is async because GetPeerAddr/get_peer_address returns a Future.
    // Returns the address in little endian order.
    async fn get_peer_address(&self, id: PeerId) -> Result<(Address, AddressType)> {
        self.cache.lock().get(&id).copied().ok_or(Error::PeerNotRecognized(id))
    }
}

pub struct BroadcastSourceAddressGetter {
    fuchsia: Option<FuchsiaPeerAddr>,
    local: LocalPeerAddrCache,
}

impl BroadcastSourceAddressGetter {
    pub fn new(fuchsia: Option<FuchsiaPeerAddr>, local: LocalPeerAddrCache) -> Self {
        Self { fuchsia, local }
    }
}

impl GetPeerAddr for BroadcastSourceAddressGetter {
    async fn get_peer_address(&self, id: PeerId) -> Result<(Address, AddressType)> {
        if let Some(fuchsia) = &self.fuchsia {
            if let Ok(addr) = fuchsia.get_peer_address(id).await {
                return Ok(addr);
            }
        }
        self.local.get_peer_address(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use futures::pin_mut;
    use futures::stream::StreamExt;
    use futures::task::Poll;

    #[fuchsia::test]
    async fn test_local_peer_addr_cache() {
        let cache = LocalPeerAddrCache::new();
        let peer_id = PeerId(123);
        let address = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let address_type = AddressType::Public;

        // Initially, the address should not be found.
        assert!(cache.get_peer_address(peer_id).await.is_err());

        // Add the peer and verify it can be looked up.
        cache.set_peer_address(peer_id, address, address_type);
        let (found_addr, found_type) = cache.get_peer_address(peer_id).await.unwrap();
        assert_eq!(found_addr, address);
        assert_eq!(found_type, address_type);

        // Check a different peer ID.
        assert!(cache.get_peer_address(PeerId(456)).await.is_err());
    }

    #[test]
    fn test_broadcast_source_address_getter_fuchsia_success() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, mut stream) =
            create_proxy_and_stream::<fidl_fuchsia_bluetooth_sys::AddressLookupMarker>();
        let fuchsia = FuchsiaPeerAddr::new(proxy);
        let local = LocalPeerAddrCache::new();

        let peer_id = PeerId(123);
        let address = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let address_type = AddressType::Public;

        let getter = BroadcastSourceAddressGetter::new(Some(fuchsia), local);

        let lookup_fut = getter.get_peer_address(peer_id);
        pin_mut!(lookup_fut);
        assert!(exec.run_until_stalled(&mut lookup_fut).is_pending());

        // Handle the FIDL request
        let stream_fut = stream.next();
        pin_mut!(stream_fut);
        let stream_result = exec.run_until_stalled(&mut stream_fut);
        let Poll::Ready(Some(Ok(fidl_fuchsia_bluetooth_sys::AddressLookupRequest::Lookup {
            payload: _,
            responder,
        }))) = stream_result
        else {
            panic!("Expected Lookup request, got {:?}", stream_result);
        };

        let fidl_addr = fidl_fuchsia_bluetooth::Address {
            type_: fidl_fuchsia_bluetooth::AddressType::Public,
            bytes: address,
        };
        responder.send(Ok(&fidl_addr.into())).unwrap();

        // The lookup should now succeed with the address from Fuchsia service
        let lookup_result = exec.run_until_stalled(&mut lookup_fut);
        let Poll::Ready(Ok((found_addr, found_type))) = lookup_result else {
            panic!("Expected future to complete with Ok, got {:?}", lookup_result);
        };
        assert_eq!(found_addr, address);
        assert_eq!(found_type, address_type);
    }

    #[test]
    fn test_broadcast_source_address_getter_fallback_to_local() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, mut stream) =
            create_proxy_and_stream::<fidl_fuchsia_bluetooth_sys::AddressLookupMarker>();
        let fuchsia = FuchsiaPeerAddr::new(proxy);
        let local = LocalPeerAddrCache::new();

        let peer_id = PeerId(123);
        let address = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let address_type = AddressType::Public;

        // Seed the local cache
        local.set_peer_address(peer_id, address, address_type);

        let getter = BroadcastSourceAddressGetter::new(Some(fuchsia), local);

        let lookup_fut = getter.get_peer_address(peer_id);
        pin_mut!(lookup_fut);
        assert!(exec.run_until_stalled(&mut lookup_fut).is_pending());

        // Handle the FIDL request, return NotFound error
        let stream_fut = stream.next();
        pin_mut!(stream_fut);
        let stream_result = exec.run_until_stalled(&mut stream_fut);
        let Poll::Ready(Some(Ok(fidl_fuchsia_bluetooth_sys::AddressLookupRequest::Lookup {
            payload: _,
            responder,
        }))) = stream_result
        else {
            panic!("Expected Lookup request, got {:?}", stream_result);
        };

        responder.send(Err(fidl_fuchsia_bluetooth_sys::LookupError::NotFound)).unwrap();

        // The lookup should fall back to the local cache and succeed
        let lookup_result = exec.run_until_stalled(&mut lookup_fut);
        let Poll::Ready(Ok((found_addr, found_type))) = lookup_result else {
            panic!("Expected future to complete with Ok, got {:?}", lookup_result);
        };
        assert_eq!(found_addr, address);
        assert_eq!(found_type, address_type);
    }
}
