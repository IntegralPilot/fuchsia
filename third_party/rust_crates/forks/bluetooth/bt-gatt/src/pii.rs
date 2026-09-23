// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::future::Future;

use bt_common::{
    core::{Address, AddressType},
    PeerId,
};

use crate::types::*;

/// Takes a peer address and queries an underlying service for its actual
/// bluetooth address and address type. Used for adding broadcast sources.
pub trait GetPeerAddr {
    /// Resolve peer ID to peer address and address type.
    fn get_peer_address(
        &self,
        peer_id: PeerId,
    ) -> impl Future<Output = Result<(Address, AddressType)>>;
}

/// Helper for when the peer's address is known. Always returns the given
/// address, if any.
pub struct StaticPeerAddr {
    peer_id: Option<PeerId>,
    address: Address,
    address_type: AddressType,
}

impl StaticPeerAddr {
    /// Returns a [`StaticPeerAddr`] that always returns the given address,
    /// regardless of the `peer_id` being looked up.
    pub fn new(address: Address, address_type: AddressType) -> Self {
        Self { peer_id: None, address, address_type }
    }

    /// Returns a [`StaticPeerAddr`] that will only return successfully if the
    /// `peer_id` matches.
    pub fn new_for_peer(peer_id: PeerId, address: Address, address_type: AddressType) -> Self {
        Self { peer_id: Some(peer_id), address, address_type }
    }
}

impl GetPeerAddr for StaticPeerAddr {
    async fn get_peer_address(&self, peer_id: PeerId) -> Result<(Address, AddressType)> {
        if let Some(validated_peer_id) = self.peer_id {
            if peer_id != validated_peer_id {
                return Err(Error::PeerNotRecognized(peer_id));
            }
        }
        return Ok((self.address, self.address_type));
    }
}
