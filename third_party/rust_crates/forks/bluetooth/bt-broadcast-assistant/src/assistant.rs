// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

use bt_bap::types::BroadcastId;
use bt_bass::client::error::Error as BassClientError;
use bt_bass::client::BroadcastAudioScanServiceClient;
use bt_common::{core::AdvertisingSetId, PeerId, Uuid};
use bt_gatt::central::*;
use bt_gatt::client::PeerServiceHandle;
use bt_gatt::types::Error as GattError;
use bt_gatt::Client;

pub mod event;
use event::*;
pub mod peer;
pub use peer::Peer;

use crate::types::*;

pub const BROADCAST_AUDIO_SCAN_SERVICE: Uuid = Uuid::from_u16(0x184F);
pub const BASIC_AUDIO_ANNOUNCEMENT_SERVICE: Uuid = Uuid::from_u16(0x1851);
pub const BROADCAST_AUDIO_ANNOUNCEMENT_SERVICE: Uuid = Uuid::from_u16(0x1852);

pub type BroadcastSourceKey = (PeerId, AdvertisingSetId);

#[derive(Debug, Error)]
pub enum Error {
    #[error("GATT operation error: {0:?}")]
    Gatt(#[from] GattError),

    #[error("Broadcast Audio Scan Service client error at peer ({0}): {1:?}")]
    BassClient(PeerId, BassClientError),

    #[error("Not connected to Broadcast Audio Scan Service at peer ({0})")]
    NotConnectedToBass(PeerId),

    #[error("Central scanning terminated unexpectedly")]
    CentralScanTerminated,

    #[error("Failed to connect to service ({1}) at peer ({0})")]
    ConnectionFailure(PeerId, Uuid),

    #[error("Broadcast Assistant was already started previously. It cannot be started twice")]
    AlreadyStarted,

    #[error("Failed due to error: {0}")]
    Generic(String),
}

/// Contains information about the currently-known broadcast
/// sources and the peers they were found on
#[derive(Debug)]
pub(crate) struct DiscoveredBroadcastSources(Mutex<HashMap<BroadcastSourceKey, BroadcastSource>>);

impl DiscoveredBroadcastSources {
    /// Creates a shareable instance of `DiscoveredBroadcastSources`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(HashMap::new())))
    }

    /// Merges the broadcast source data with existing broadcast source data.
    /// Returns the copy of the broadcast source data after the merge and
    /// indicates whether it has changed from before or not.
    pub(crate) fn merge_broadcast_source_data(
        &self,
        key: &BroadcastSourceKey,
        data: &BroadcastSource,
    ) -> (BroadcastSource, bool) {
        let mut lock = self.0.lock();
        let source = lock.entry(*key).or_default();
        let before = source.clone();

        source.merge(data);

        let after = source.clone();
        let changed = before != after;

        (after, changed)
    }

    /// Get a BroadcastSource from a peer id and advertising SID.
    pub(crate) fn get_by_key(
        &self,
        peer_id: PeerId,
        advertising_sid: AdvertisingSetId,
    ) -> Option<BroadcastSource> {
        let lock = self.0.lock();
        lock.get(&(peer_id, advertising_sid)).cloned()
    }

    /// Get a BroadcastSource from associated broadcast id.
    fn get_by_broadcast_id(&self, broadcast_id: &BroadcastId) -> Option<BroadcastSource> {
        let lock = self.0.lock();
        let info = lock.values().find(|v| v.broadcast_id == Some(*broadcast_id));
        info.cloned()
    }
}

pub struct BroadcastAssistant<T: bt_gatt::GattTypes> {
    central: T::Central,
    broadcast_sources: Arc<DiscoveredBroadcastSources>,
    broadcast_source_scan_started: Arc<AtomicBool>,
}

impl<T: bt_gatt::GattTypes + 'static> BroadcastAssistant<T> {
    // Creates a broadcast assistant and sets it up to be ready
    // for broadcast source scanning. Clients must use the `start`
    // method to poll the event stream for scan results.
    pub fn new(central: T::Central) -> Self {
        Self {
            central,
            broadcast_sources: DiscoveredBroadcastSources::new(),
            broadcast_source_scan_started: Arc::new(AtomicBool::new(false)),
        }
    }

    /// List of scan filters for advertisement data Broadcast Assistant should
    /// look for, which are:
    /// - Service data with Broadcast Audio Announcement Service UUID from
    ///   Broadcast Sources (see BAP spec v1.0.1 Section 3.7.2.1 for details)
    // TODO(b/308481381): define filter for finding broadcast sink.
    fn scan_filters() -> Vec<ScanFilter> {
        vec![Filter::HasServiceData(BROADCAST_AUDIO_ANNOUNCEMENT_SERVICE).into()]
    }

    /// Start broadcast assistant. Returns EventStream that the upper layer can
    /// poll. Upper layer can call methods on BroadcastAssistant based on the
    /// events it sees.
    pub fn start(&mut self) -> Result<EventStream<T>, Error> {
        if self.is_started() {
            return Err(Error::AlreadyStarted);
        }
        let scan_result_stream = self.central.scan(&Self::scan_filters());
        self.broadcast_source_scan_started.store(true, Ordering::Relaxed);

        let periodic_advertising = self
            .central
            .periodic_advertising()
            .inspect_err(|e| {
                // TODO(b/433285146): This should eventually fail the start operation.
                // For now, log a warning and fallback to EA-only.
                log::warn!(
                    "Periodic Advertising not supported by platform: {:?}. Falling back to EA-only scanning.",
                    e
                );
            })
            .ok();

        Ok(EventStream::<T>::new(
            scan_result_stream,
            periodic_advertising,
            self.broadcast_sources.clone(),
            self.broadcast_source_scan_started.clone(),
        ))
    }

    /// Returns whether or not Broadcast Assistant has started.
    fn is_started(&self) -> bool {
        self.broadcast_source_scan_started.load(Ordering::Relaxed)
    }

    pub fn scan_for_scan_delegators(&mut self) -> Result<T::ScanResultStream, Error> {
        if self.is_started() {
            return Err(Error::Generic(format!(
                "Cannot scan for scan delegators while scanning for broadcast sources"
            )));
        }
        // Scan for service data with Broadcast Audio Scan Service UUID to look
        // for Broadcast Sink collocated with the Scan Delegator (see BAP spec v1.0.1
        // Section 3.9.2 for details).
        Ok(self.central.scan(&vec![Filter::HasServiceData(BROADCAST_AUDIO_SCAN_SERVICE).into()]))
    }

    pub async fn connect_to_scan_delegator(&self, peer_id: PeerId) -> Result<Peer<T>, Error>
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        let client = self.central.connect(peer_id).await?;
        let service_handles = client.find_service(BROADCAST_AUDIO_SCAN_SERVICE).await?;

        for handle in service_handles {
            if handle.uuid() != BROADCAST_AUDIO_SCAN_SERVICE || !handle.is_primary() {
                continue;
            }
            let service = handle.connect().await?;
            let bass = BroadcastAudioScanServiceClient::<T>::create(service)
                .await
                .map_err(|e| Error::BassClient(peer_id, e))?;

            let connected_peer =
                Peer::<T>::new(peer_id, client, bass, self.broadcast_sources.clone());
            return Ok(connected_peer);
        }
        Err(Error::ConnectionFailure(peer_id, BROADCAST_AUDIO_SCAN_SERVICE))
    }

    // Manually adds broadcast source information for debugging purposes.
    #[cfg(any(test, feature = "debug"))]
    pub fn force_discover_broadcast_source(
        &self,
        peer_id: PeerId,
        address: [u8; 6],
        address_type: bt_common::core::AddressType,
        advertising_sid: bt_common::core::AdvertisingSetId,
    ) -> Result<BroadcastSource, Error> {
        let broadcast_source = BroadcastSource {
            address: Some(address),
            address_type: Some(address_type),
            broadcast_id: None,
            periodic_advertising_interval: None,
            endpoint: None,
            broadcast_name: None,
        };

        Ok(self
            .broadcast_sources
            .merge_broadcast_source_data(&(peer_id, advertising_sid), &broadcast_source)
            .0)
    }

    // Manually adds broadcast source information for debugging purposes.
    #[cfg(any(test, feature = "debug"))]
    pub fn force_discover_broadcast_source_metadata(
        &self,
        peer_id: PeerId,
        advertising_sid: bt_common::core::AdvertisingSetId,
        big_metadata: Vec<Vec<bt_common::generic_audio::metadata_ltv::Metadata>>,
    ) -> Result<BroadcastSource, Error> {
        use bt_bap::types::{BroadcastAudioSourceEndpoint, BroadcastIsochronousGroup};
        use bt_common::core::CodecId;

        let mut big = Vec::new();
        for metadata in big_metadata {
            let group = BroadcastIsochronousGroup {
                codec_id: CodecId::Assigned(bt_common::core::CodingFormat::ALawLog), // mock.
                codec_specific_configs: vec![],
                metadata,
                bis: vec![],
            };
            big.push(group);
        }

        let endpoint = BroadcastAudioSourceEndpoint { presentation_delay_ms: 0, big };

        let broadcast_source = BroadcastSource {
            address: None,
            address_type: None,
            broadcast_id: None,
            periodic_advertising_interval: None,
            endpoint: Some(endpoint),
            broadcast_name: None,
        };

        Ok(self
            .broadcast_sources
            .merge_broadcast_source_data(&(peer_id, advertising_sid), &broadcast_source)
            .0)
    }

    // Gets the broadcast sources currently known by the broadcast
    // assistant.
    pub fn known_broadcast_sources(
        &self,
    ) -> std::collections::HashMap<BroadcastSourceKey, BroadcastSource> {
        self.broadcast_sources.0.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::{pin_mut, FutureExt};
    use std::task::Poll;

    use bt_bap::types::*;
    use bt_common::core::{AddressType, AdvertisingSetId, PeriodicAdvertisingInterval};
    use bt_common::generic_audio::metadata_ltv::Metadata;
    use bt_gatt::test_utils::{FakeCentral, FakeClient, FakeTypes};

    use crate::assistant::peer::tests::fake_bass_service;

    #[test]
    fn merge_broadcast_source() {
        let discovered = DiscoveredBroadcastSources::new();
        let bid1 = BroadcastId::try_from(1001).unwrap();
        let key1 = (PeerId(1001), AdvertisingSetId::try_from(1).unwrap());

        // 1. Merge initial source data for SID 1.
        let (bs, changed) = discovered.merge_broadcast_source_data(
            &key1,
            &BroadcastSource::default()
                .with_address([1, 2, 3, 4, 5, 6])
                .with_address_type(AddressType::Public)
                .with_broadcast_id(bid1),
        );
        assert!(changed);
        assert_eq!(
            bs,
            BroadcastSource {
                address: Some([1, 2, 3, 4, 5, 6]),
                address_type: Some(AddressType::Public),
                broadcast_id: Some(bid1),
                periodic_advertising_interval: None,
                endpoint: None,
                broadcast_name: None,
            }
        );

        // 2. Merge endpoint and PA interval for SID 1.
        let (bs, changed) = discovered.merge_broadcast_source_data(
            &key1,
            &BroadcastSource::default()
                .with_address_type(AddressType::Random)
                .with_endpoint(BroadcastAudioSourceEndpoint {
                    presentation_delay_ms: 32,
                    big: vec![],
                })
                .with_periodic_advertising_interval(PeriodicAdvertisingInterval(0x0100)),
        );
        assert!(changed);
        assert_eq!(
            bs,
            BroadcastSource {
                address: Some([1, 2, 3, 4, 5, 6]),
                address_type: Some(AddressType::Random),
                broadcast_id: Some(bid1),
                periodic_advertising_interval: Some(PeriodicAdvertisingInterval(0x0100)),
                endpoint: Some(BroadcastAudioSourceEndpoint {
                    presentation_delay_ms: 32,
                    big: vec![]
                }),
                broadcast_name: None,
            }
        );

        // 3. Merge duplicate endpoint for SID 1 (no change).
        let (_, changed) = discovered.merge_broadcast_source_data(
            &key1,
            &BroadcastSource::default().with_address_type(AddressType::Random).with_endpoint(
                BroadcastAudioSourceEndpoint { presentation_delay_ms: 32, big: vec![] },
            ),
        );
        assert!(!changed);

        // 4. Merge a new broadcast source with a different SID (SID 2) for the same
        //    peer.
        let bid2 = BroadcastId::try_from(1002).unwrap();
        let key2 = (PeerId(1001), AdvertisingSetId::try_from(2).unwrap());
        let (bs, changed) = discovered.merge_broadcast_source_data(
            &key2,
            &BroadcastSource::default()
                .with_address([1, 2, 3, 4, 5, 6])
                .with_address_type(AddressType::Public)
                .with_broadcast_id(bid2),
        );
        assert!(changed);
        assert_eq!(
            bs,
            BroadcastSource {
                address: Some([1, 2, 3, 4, 5, 6]),
                address_type: Some(AddressType::Public),
                broadcast_id: Some(bid2),
                periodic_advertising_interval: None,
                endpoint: None,
                broadcast_name: None,
            }
        );

        // Verify both entries coexist in the registry
        assert!(discovered.get_by_key(key1.0, key1.1).is_some());
        assert!(discovered.get_by_key(key2.0, key2.1).is_some());

        // Verify get_by_broadcast_id works for both and maps to correct keys
        let lock = discovered.0.lock();
        let entry1 = lock.iter().find(|(_, v)| v.broadcast_id == Some(bid1)).unwrap();
        assert_eq!(entry1.0 .1, AdvertisingSetId::try_from(1).unwrap());
        let entry2 = lock.iter().find(|(_, v)| v.broadcast_id == Some(bid2)).unwrap();
        assert_eq!(entry2.0 .1, AdvertisingSetId::try_from(2).unwrap());
    }

    #[test]
    fn start_stream() {
        let mut assistant = BroadcastAssistant::<FakeTypes>::new(FakeCentral::new());
        let stream = assistant.start().expect("can start stream");

        // Stream can only be started once.
        assert!(assistant.is_started());
        assert!(assistant.start().is_err());

        // After the stream is dropped, it can be started again.
        drop(stream);
        assert!(!assistant.is_started());
        assert!(assistant.start().is_ok());
    }

    #[test]
    fn connect_to_scan_delegator() {
        // Set up fake GATT related objects.
        let mut central = FakeCentral::new();
        let mut client = FakeClient::new();
        central.add_client(PeerId(1004), client.clone());
        let service = fake_bass_service();
        client.add_service(BROADCAST_AUDIO_SCAN_SERVICE, true, service.clone());

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let assistant = BroadcastAssistant::<FakeTypes>::new(central);
        let conn_fut = assistant.connect_to_scan_delegator(PeerId(1004));
        pin_mut!(conn_fut);
        let polled = conn_fut.poll_unpin(&mut noop_cx);
        let Poll::Ready(res) = polled else {
            panic!("should be ready");
        };
        let _ = res.expect("should be ok");
    }

    #[test]
    fn force_discover_broadcast_source_test() {
        let assistant = BroadcastAssistant::<FakeTypes>::new(FakeCentral::new());
        let peer_id = PeerId(1);
        let address = [1, 2, 3, 4, 5, 6];
        let address_type = AddressType::Public;
        let sid = AdvertisingSetId::try_from(1).unwrap();

        let source =
            assistant.force_discover_broadcast_source(peer_id, address, address_type, sid).unwrap();

        assert_eq!(source.address, Some(address));
        assert_eq!(source.address_type, Some(address_type));

        // Verify it is registered in the assistant under the correct key
        let known = assistant.known_broadcast_sources();
        assert_eq!(known.get(&(peer_id, sid)).unwrap().address, Some(address));
    }

    #[test]
    fn force_discover_broadcast_source_metadata_test() {
        let assistant = BroadcastAssistant::<FakeTypes>::new(FakeCentral::new());
        let peer_id = PeerId(1);
        let metadata = vec![vec![Metadata::BroadcastAudioImmediateRenderingFlag]];
        let sid = AdvertisingSetId::try_from(1).unwrap();

        let source = assistant
            .force_discover_broadcast_source_metadata(peer_id, sid, metadata.clone())
            .unwrap();

        let endpoint = source.endpoint.unwrap();
        assert_eq!(endpoint.big.len(), 1);
        assert_eq!(endpoint.big[0].metadata, metadata[0]);

        // Verify it is registered in the assistant under the correct key
        let known = assistant.known_broadcast_sources();
        assert!(known.contains_key(&(peer_id, sid)));
    }
}
