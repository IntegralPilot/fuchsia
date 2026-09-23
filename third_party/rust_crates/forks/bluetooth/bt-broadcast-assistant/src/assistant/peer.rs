// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_gatt::pii::GetPeerAddr;
use futures::stream::FusedStream;
use futures::Stream;
use std::sync::Arc;
use thiserror::Error;

use bt_bap::types::BroadcastId;
use bt_bass::client::error::Error as BassClientError;
use bt_bass::client::event::Event as BassEvent;
use bt_bass::client::BroadcastAudioScanServiceClient;
#[cfg(any(test, feature = "debug"))]
use bt_bass::types::BroadcastReceiveState;
use bt_bass::types::{BisSync, PaSync};
use bt_common::core::{AdvertisingSetId, PeriodicAdvertisingInterval};
use bt_common::packet_encoding::Error as PacketError;
use bt_common::PeerId;
#[cfg(any(test, feature = "debug"))]
use bt_gatt::types::Handle;
use std::collections::HashMap;

use crate::assistant::DiscoveredBroadcastSources;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to take event stream at Broadcast Audio Scan Service client")]
    UnavailableBassEventStream,

    #[error("Broadcast Audio Scan Service client error: {0:?}")]
    BassClient(#[from] BassClientError),

    #[error("Incomplete information for broadcast source with peer id ({0})")]
    NotEnoughInfo(PeerId),

    #[error("Broadcast source with peer id ({0}) does not exist")]
    DoesNotExist(PeerId),

    #[error("Failed to lookup address for peer id ({0}): {1:?}")]
    AddressLookupError(PeerId, bt_gatt::types::Error),

    #[error("Packet error: {0}")]
    PacketError(#[from] PacketError),
}

/// Connected scan delegator peer. Clients can use this
/// object to perform Broadcast Audio Scan Service operations on the
/// scan delegator peer.
/// Not thread-safe and only one operation must be done at a time.
pub struct Peer<T: bt_gatt::GattTypes> {
    peer_id: PeerId,
    // Keep for peer connection.
    _client: T::Client,
    bass: BroadcastAudioScanServiceClient<T>,
    // TODO(b/309015071): add a field for pacs.
    broadcast_sources: Arc<DiscoveredBroadcastSources>,
}

impl<T: bt_gatt::GattTypes> Peer<T> {
    pub(crate) fn new(
        peer_id: PeerId,
        client: T::Client,
        bass: BroadcastAudioScanServiceClient<T>,
        broadcast_sources: Arc<DiscoveredBroadcastSources>,
    ) -> Self {
        Peer { peer_id, _client: client, bass, broadcast_sources }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Takes event stream for BASS events from this scan delegator peer.
    /// Clients can call this method to start subscribing to BASS events.
    pub fn take_event_stream(
        &mut self,
    ) -> Result<impl Stream<Item = Result<BassEvent, BassClientError>> + FusedStream, Error> {
        self.bass.take_event_stream().ok_or(Error::UnavailableBassEventStream)
    }

    /// Send broadcast code for a particular broadcast.
    pub async fn send_broadcast_code(
        &self,
        broadcast_id: BroadcastId,
        broadcast_code: [u8; 16],
    ) -> Result<(), Error> {
        self.bass.set_broadcast_code(broadcast_id, broadcast_code).await.map_err(Into::into)
    }

    /// Sends a command to add a particular broadcast source.
    ///
    /// # Arguments
    ///
    /// * `broadcast_source_pid` - peer id of the braodcast source that's to be
    ///   added to this scan delegator peer
    /// * `address_lookup` - An implementation of [`GetPeerAddr`] that will be
    ///   used to look up the peer's address.
    /// * `pa_sync` - pa sync mode the peer should attempt to be in
    /// * `bis_sync` - desired BIG to BIS synchronization information. If the
    ///   set is empty, no preference value is used for all the BIGs
    pub async fn add_broadcast_source(
        &self,
        source_peer_id: PeerId,
        advertising_sid: AdvertisingSetId,
        address_lookup: &impl GetPeerAddr,
        pa_sync: PaSync,
        bis_sync: HashMap<u8, BisSync>,
    ) -> Result<(), Error> {
        let mut broadcast_source = self
            .broadcast_sources
            .get_by_key(source_peer_id, advertising_sid)
            .ok_or(Error::DoesNotExist(source_peer_id))?;

        let (broadcast_addr, broadcast_addr_type) = address_lookup
            .get_peer_address(source_peer_id)
            .await
            .map_err(|err| Error::AddressLookupError(source_peer_id, err))?;
        broadcast_source.with_address(broadcast_addr).with_address_type(broadcast_addr_type);

        if !broadcast_source.is_ready_to_add() {
            return Err(Error::NotEnoughInfo(source_peer_id));
        }

        self.bass
            .add_broadcast_source(
                broadcast_source.broadcast_id.unwrap(),
                broadcast_source.address_type.unwrap(),
                broadcast_source.address.unwrap(),
                advertising_sid,
                pa_sync,
                broadcast_source
                    .periodic_advertising_interval
                    .unwrap_or(PeriodicAdvertisingInterval::unknown()),
                broadcast_source.endpoint_to_big_subgroups(bis_sync).map_err(Error::PacketError)?,
            )
            .await
            .map_err(Into::into)
    }

    /// Sends a command to to update a particular broadcast source's PA sync.
    ///
    /// # Arguments
    ///
    /// * `broadcast_id` - broadcast id of the broadcast source that's to be
    ///   updated
    /// * `pa_sync` - pa sync mode the scan delegator peer should attempt to be
    ///   in.
    /// * `bis_sync` - desired BIG to BIS synchronization information
    pub async fn update_broadcast_source_sync(
        &self,
        broadcast_id: BroadcastId,
        pa_sync: PaSync,
        bis_sync: HashMap<u8, BisSync>,
    ) -> Result<(), Error> {
        let pa_interval = self
            .broadcast_sources
            .get_by_broadcast_id(&broadcast_id)
            .map(|bs| bs.periodic_advertising_interval)
            .unwrap_or(None);

        self.bass
            .modify_broadcast_source(broadcast_id, pa_sync, pa_interval, bis_sync, None)
            .await
            .map_err(Into::into)
    }

    /// Sends a command to remove a particular broadcast source.
    ///
    /// # Arguments
    ///
    /// * `broadcast_id` - broadcast id of the braodcast source that's to be
    ///   removed from the scan delegator
    pub async fn remove_broadcast_source(&self, broadcast_id: BroadcastId) -> Result<(), Error> {
        self.bass.remove_broadcast_source(broadcast_id).await.map_err(Into::into)
    }

    /// Sends a command to inform the scan delegator peer that we have
    /// started scanning for broadcast sources on behalf of it.
    pub async fn inform_remote_scan_started(&self) -> Result<(), Error> {
        self.bass.remote_scan_started().await.map_err(Into::into)
    }

    /// Sends a command to inform the scan delegator peer that we have
    /// stopped scanning for broadcast sources on behalf of it.
    pub async fn inform_remote_scan_stopped(&self) -> Result<(), Error> {
        self.bass.remote_scan_stopped().await.map_err(Into::into)
    }

    /// Returns a list of BRS characteristics' latest values the scan delegator
    /// has received.
    #[cfg(any(test, feature = "debug"))]
    pub fn get_broadcast_receive_states(&self) -> Vec<(Handle, BroadcastReceiveState)> {
        self.bass.known_broadcast_sources()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use bt_gatt::pii::StaticPeerAddr;
    use futures::{pin_mut, FutureExt};
    use std::collections::HashMap;
    use std::task::Poll;

    use bt_common::core::{AddressType, AdvertisingSetId};
    use bt_gatt::test_utils::{FakeClient, FakeGetPeerAddr, FakePeerService, FakeTypes};
    use bt_gatt::types::{
        AttributePermissions, CharacteristicProperties, CharacteristicProperty, Handle,
    };

    use bt_gatt::Characteristic;

    use crate::types::BroadcastSource;

    const RECEIVE_STATE_HANDLE: Handle = Handle(0x11);
    const AUDIO_SCAN_CONTROL_POINT_HANDLE: Handle = Handle(0x12);

    pub(crate) fn fake_bass_service() -> FakePeerService {
        let mut peer_service = FakePeerService::new();
        // One broadcast receive state and one broadcast audio scan control
        // point characteristic handles.
        peer_service.add_characteristic(
            Characteristic {
                handle: RECEIVE_STATE_HANDLE,
                uuid: bt_bass::types::BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        peer_service.add_characteristic(
            Characteristic {
                handle: AUDIO_SCAN_CONTROL_POINT_HANDLE,
                uuid: bt_bass::types::BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID,
                properties: CharacteristicProperties(vec![CharacteristicProperty::Broadcast]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        peer_service
    }

    fn setup() -> (Peer<FakeTypes>, FakePeerService, Arc<DiscoveredBroadcastSources>) {
        let peer_service = fake_bass_service();

        let broadcast_sources = DiscoveredBroadcastSources::new();
        (
            Peer {
                peer_id: PeerId(0x1),
                _client: FakeClient::new(),
                bass: BroadcastAudioScanServiceClient::<FakeTypes>::create_for_test(
                    peer_service.clone(),
                    Handle(0x1),
                ),
                broadcast_sources: broadcast_sources.clone(),
            },
            peer_service,
            broadcast_sources,
        )
    }

    #[test]
    fn take_event_stream() {
        let (mut peer, _peer_service, _broadcast_source) = setup();
        let _event_stream = peer.take_event_stream().expect("should succeed");

        // If we try to take the event stream the second time, it should fail.
        assert!(peer.take_event_stream().is_err());
    }

    #[test]
    fn add_broadcast_source_fail() {
        let (peer, _peer_service, broadcast_source) = setup();

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

        // Should fail because broadcast source doesn't exist.
        {
            let fut = peer.add_broadcast_source(
                PeerId(1001),
                AdvertisingSetId::try_from(1).unwrap(),
                &FakeGetPeerAddr,
                PaSync::SyncPastUnavailable,
                HashMap::new(),
            );
            pin_mut!(fut);
            let polled = fut.poll_unpin(&mut noop_cx);
            assert_matches!(polled, Poll::Ready(Err(Error::DoesNotExist(_))));
        }

        let _ = broadcast_source.merge_broadcast_source_data(
            &(PeerId(1001), AdvertisingSetId::try_from(1).unwrap()),
            &BroadcastSource::default().with_broadcast_id(BroadcastId::try_from(1001).unwrap()),
        );

        // Should fail because peer address couldn't be looked up.
        {
            let address_lookup =
                StaticPeerAddr::new_for_peer(PeerId(1002), [1, 2, 3, 4, 5, 6], AddressType::Public);
            let fut = peer.add_broadcast_source(
                PeerId(1001),
                AdvertisingSetId::try_from(1).unwrap(),
                &address_lookup,
                PaSync::SyncPastUnavailable,
                HashMap::new(),
            );
            pin_mut!(fut);
            let polled = fut.poll_unpin(&mut noop_cx);
            assert_matches!(polled, Poll::Ready(Err(Error::AddressLookupError(_, _))));
        }

        // Should fail because not enough information.
        {
            let address_lookup =
                StaticPeerAddr::new_for_peer(PeerId(1001), [1, 2, 3, 4, 5, 6], AddressType::Public);
            let fut = peer.add_broadcast_source(
                PeerId(1001),
                AdvertisingSetId::try_from(1).unwrap(),
                &address_lookup,
                PaSync::SyncPastUnavailable,
                HashMap::new(),
            );
            pin_mut!(fut);
            let polled = fut.poll_unpin(&mut noop_cx);
            assert_matches!(polled, Poll::Ready(Err(Error::NotEnoughInfo(_))));
        }
    }
}
