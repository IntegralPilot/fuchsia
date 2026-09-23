// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod error;
pub mod event;

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{BoxStream, FusedStream, SelectAll, Stream, StreamExt};
use futures::Future;
use log::warn;
use parking_lot::Mutex;

use bt_bap::types::BroadcastId;
use bt_common::core::{AddressType, AdvertisingSetId, PeriodicAdvertisingInterval};
use bt_common::generic_audio::metadata_ltv::Metadata;
use bt_common::packet_encoding::Decodable;
use bt_gatt::client::{CharacteristicNotification, PeerService, ServiceCharacteristic};
use bt_gatt::types::{Handle, WriteMode};

use crate::client::error::{Error, ServiceError};
use crate::client::event::*;
use crate::types::*;

const READ_CHARACTERISTIC_BUFFER_SIZE: usize = 255;

/// Keeps track of Source_ID and Broadcast_ID that are associated together.
/// Source_ID is assigned by the BASS server to a Broadcast Receive State
/// characteristic. If the remote peer with the BASS server autonomously
/// synchronized to a PA or accepted the Add Source operation, the server
/// selects an empty Broadcast Receive State characteristic to update or deletes
/// one of the existing one to update. However, because the concept of Source_ID
/// is unqiue to BASS, we track the Broadcast_ID that a Source_ID is associated
/// so that it can be used by upper layers.
#[derive(Default)]
pub(crate) struct KnownBroadcastSources(HashMap<Handle, BroadcastReceiveState>);

impl KnownBroadcastSources {
    fn new(receive_states: HashMap<Handle, BroadcastReceiveState>) -> Self {
        KnownBroadcastSources(receive_states)
    }

    /// Updates the value of the specified broadcast receive state
    /// characteristic. Returns the old value if it existed.
    fn update_state(
        &mut self,
        key: Handle,
        value: BroadcastReceiveState,
    ) -> Option<BroadcastReceiveState> {
        self.0.insert(key, value)
    }

    /// Given the broadcast ID, find the corresponding source ID.
    /// Returns none if the server doesn't know the specified broadcast source
    /// because a) the broadcast source was never added or discovered; or,
    /// b) the broadcast source was removed from remove operation; or,
    /// c) the broadcast source was removed by the server to add a different
    ///    broadcast source.
    fn source_id(&self, broadcast_id: &BroadcastId) -> Option<SourceId> {
        let Some(state) = self.state(broadcast_id) else {
            return None;
        };
        Some(state.source_id)
    }

    /// Gets the last updated broadcast receive state value.
    /// Returns none if the server doesn't know the specified broadcast source.
    fn state(&self, broadcast_id: &BroadcastId) -> Option<&ReceiveState> {
        self.0.iter().find_map(|(&_k, &ref v)| match v {
            BroadcastReceiveState::Empty => None,
            BroadcastReceiveState::NonEmpty(rs) => {
                if rs.broadcast_id() == *broadcast_id {
                    return Some(rs);
                }
                None
            }
        })
    }
}

/// Manages connection to the Broadcast Audio Scan Service at the
/// remote Scan Delegator and writes/reads characteristics to/from it.
pub struct BroadcastAudioScanServiceClient<T: bt_gatt::GattTypes> {
    gatt_client: T::PeerService,
    /// Broadcast Audio Scan Service only has one Broadcast Audio Scan Control
    /// Point characteristic according to BASS Section 3. There shall
    /// be one or more Broadcast Receive State characteristics.
    audio_scan_control_point: Handle,
    /// Broadcast Receive State characteristics can be used to determine the
    /// BASS status.
    broadcast_sources: Arc<Mutex<KnownBroadcastSources>>,
    /// Keeps track of the broadcast codes that were sent to the remote BASS
    /// server.
    broadcast_codes: Arc<Mutex<HashMap<SourceId, [u8; 16]>>>,
    // GATT notification streams for BRS characteristic value changes.
    notification_streams: Option<
        SelectAll<BoxStream<'static, Result<CharacteristicNotification, bt_gatt::types::Error>>>,
    >,
}

impl<T: bt_gatt::GattTypes> BroadcastAudioScanServiceClient<T> {
    #[cfg(any(test, feature = "test-utils"))]
    pub fn create_for_test(gatt_client: T::PeerService, audio_scan_control_point: Handle) -> Self {
        Self {
            gatt_client,
            audio_scan_control_point,
            broadcast_sources: Default::default(),
            broadcast_codes: Arc::new(Mutex::new(HashMap::new())),
            notification_streams: Some(SelectAll::new()),
        }
    }

    pub async fn create(gatt_client: T::PeerService) -> Result<Self, Error>
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        // BASS server should have a single Broadcast Audio Scan Control Point
        // Characteristic.
        let bascp =
            ServiceCharacteristic::<T>::find(&gatt_client, BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID)
                .await
                .map_err(|e| Error::Gatt(e))?;
        if bascp.len() != 1 {
            let err = if bascp.len() == 0 {
                Error::Service(ServiceError::MissingCharacteristic)
            } else {
                Error::Service(ServiceError::ExtraScanControlPointCharacteristic)
            };
            return Err(err);
        }
        let bascp_handle = *bascp[0].handle();
        let brs_chars = Self::discover_brs_characteristics(&gatt_client).await?;
        let mut c = Self {
            gatt_client,
            audio_scan_control_point: bascp_handle,
            broadcast_sources: Arc::new(Mutex::new(KnownBroadcastSources::new(brs_chars))),
            broadcast_codes: Arc::new(Mutex::new(HashMap::new())),
            notification_streams: None,
        };
        c.register_notifications();
        Ok(c)
    }

    // Discover all the Broadcast Receive State characteristics.
    // On success, returns the HashMap of all Broadcast Received State
    // Characteristics.
    async fn discover_brs_characteristics(
        gatt_client: &T::PeerService,
    ) -> Result<HashMap<Handle, BroadcastReceiveState>, Error> {
        let brs = ServiceCharacteristic::<T>::find(gatt_client, BROADCAST_RECEIVE_STATE_UUID)
            .await
            .map_err(|e| Error::Gatt(e))?;
        if brs.len() == 0 {
            return Err(Error::Service(ServiceError::MissingCharacteristic));
        }
        let mut brs_map = HashMap::new();
        for c in brs {
            // Read the value of the Broadcast Recieve State at the time of discovery for
            // record.
            let mut buf = vec![0; READ_CHARACTERISTIC_BUFFER_SIZE];
            match c.read(&mut buf[..]).await {
                Ok(read_bytes) => match BroadcastReceiveState::decode(&buf[0..read_bytes]).0 {
                    Ok(decoded) => {
                        brs_map.insert(*c.handle(), decoded);
                        continue;
                    }
                    Err(e) => warn!(
                        "Failed to decode characteristic ({:?}) to Broadcast Receive State value: {:?}",
                        *c.handle(),
                        e
                    ),
                },
                Err(e) => warn!("Failed to read characteristic ({:?}) value: {:?}", *c.handle(), e),
            }
            brs_map.insert(*c.handle(), BroadcastReceiveState::Empty);
        }
        Ok(brs_map)
    }

    fn register_notifications(&mut self)
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        let mut notification_streams = SelectAll::new();
        {
            let lock = self.broadcast_sources.lock();
            for handle in lock.0.keys() {
                let stream = self.gatt_client.subscribe(&handle);
                notification_streams.push(stream.boxed());
            }
        }
        self.notification_streams = Some(notification_streams);
    }

    /// Returns a stream that can be used by the upper layer to poll for
    /// BroadcastAudioScanServiceEvent. BroadcastAudioScanServiceEvents are
    /// generated based on BRS characteristic change received from GATT
    /// notification that are processed by BroadcastAudioScanServiceClient.
    /// This method should only be called once.
    /// Returns an error if the method is called for a second time.
    pub fn take_event_stream(
        &mut self,
    ) -> Option<impl Stream<Item = Result<Event, Error>> + FusedStream> {
        let notification_streams = self.notification_streams.take();
        let Some(streams) = notification_streams else {
            return None;
        };
        let event_stream = EventStream::new(streams, self.broadcast_sources.clone());
        Some(event_stream)
    }

    /// Write to the Broadcast Audio Scan Control Point characteristic in
    /// without response mode.
    fn write_to_bascp(
        &self,
        op: impl ControlPointOperation,
    ) -> impl Future<Output = Result<(), Error>> + '_ {
        let handle = self.audio_scan_control_point;
        let mut buf = vec![0; op.encoded_len()];
        let encode_res = op.encode(&mut buf[..]);
        async move {
            match encode_res {
                Err(e) => Err(Error::Packet(e)),
                Ok(_) => self
                    .gatt_client
                    .write_characteristic(&handle, WriteMode::WithoutResponse, 0, buf.as_slice())
                    .await
                    .map_err(|e| Error::Gatt(e)),
            }
        }
    }

    fn get_source_id(&self, broadcast_id: &BroadcastId) -> Result<SourceId, Error> {
        self.broadcast_sources
            .lock()
            .source_id(broadcast_id)
            .ok_or(Error::UnknownBroadcastSource(*broadcast_id))
    }

    /// Returns a clone of the latest known broadcast audio receive state of the
    /// specified broadcast source given its broadcast id.
    fn get_broadcast_source_state(&self, broadcast_id: &BroadcastId) -> Option<ReceiveState> {
        let lock = self.broadcast_sources.lock();
        lock.state(broadcast_id).clone().map(|rs| rs.clone())
    }

    /// Indicates to the remote BASS server that we have started scanning for
    /// broadcast sources on behalf of it. If the scan delegator that serves
    /// the BASS server is collocated with a broadcast sink, this may or may
    /// not change the scanning behaviour of the the broadcast sink.
    pub async fn remote_scan_started(&self) -> Result<(), Error> {
        let op = RemoteScanStartedOperation;
        self.write_to_bascp(op).await
    }

    /// Indicates to the remote BASS server that we have stopped scanning for
    /// broadcast sources on behalf of it.
    pub async fn remote_scan_stopped(&self) -> Result<(), Error> {
        let op = RemoteScanStoppedOperation;
        self.write_to_bascp(op).await
    }

    /// Provides the BASS server with information regarding a Broadcast Source.
    pub async fn add_broadcast_source(
        &self,
        broadcast_id: BroadcastId,
        address_type: AddressType,
        advertiser_address: [u8; ADDRESS_BYTE_SIZE],
        sid: AdvertisingSetId,
        pa_sync: PaSync,
        pa_interval: PeriodicAdvertisingInterval,
        subgroups: Vec<BigSubgroup>,
    ) -> Result<(), Error> {
        let op = AddSourceOperation::new(
            address_type,
            advertiser_address,
            sid,
            broadcast_id,
            pa_sync,
            pa_interval,
            subgroups,
        );
        self.write_to_bascp(op).await
    }

    /// Requests the Scan Delegator to modify a Broadcast Source's
    /// synchronization state and/or metadata.
    ///
    /// This method writes a **Modify Source** operation to the Broadcast Audio
    /// Scan Control Point. It reads the current state of the Broadcast
    /// Source from the local cache (the BRS characteristic value), applies
    /// the requested modifications, and sends the complete updated subgroup
    /// list to the server.
    ///
    /// # Arguments
    ///
    /// * `broadcast_id` - id of the broadcast source to modify
    /// * `pa_sync` - pa sync mode the scan delegator peer should attempt to be
    ///   in
    /// * `pa_interval` - updated PA interval value. If none, unknown value is
    ///   used
    /// * `bis_map` - desired BIG to BIS synchronization update information. If
    ///   a BIG does not exist as a key, sync for that BIG is not updated.
    /// * `metadata_map` - map of updated metadata for BIGs. If a mapping does
    ///   not exist for a BIG, that BIG's metadata is not updated
    pub async fn modify_broadcast_source(
        &self,
        broadcast_id: BroadcastId,
        pa_sync: PaSync,
        pa_interval: Option<PeriodicAdvertisingInterval>,
        bis_map: HashMap<SubgroupIndex, BisSync>,
        metadata_map: Option<HashMap<SubgroupIndex, Vec<Metadata>>>,
    ) -> Result<(), Error> {
        let mut state = self
            .get_broadcast_source_state(&broadcast_id)
            .ok_or(Error::UnknownBroadcastSource(broadcast_id))?;

        // Update metadata for BIGs if applicable.
        if let Some(mut m) = metadata_map {
            for (big_index, group) in state.subgroups.iter_mut().enumerate() {
                if let Some(metadata) = m.remove(&(big_index as u8)) {
                    group.metadata = metadata;
                }
            }

            // Left over metadata values are new subgroups that are to be added. New
            // subgroups can only be added if the subgroup index is
            // contiguous to existing subgroups.
            let mut new_big_indices: Vec<&u8> = m.keys().collect();
            new_big_indices.sort();
            for big_index in new_big_indices {
                if (*big_index as usize) != state.subgroups.len() {
                    warn!("cannot add new [{big_index}th] subgroup");
                    break;
                }
                let new_subgroup = BigSubgroup::new(None).with_metadata(m[big_index].clone());
                state.subgroups.push(new_subgroup);
            }
        }

        // Update BIS_Sync param for BIGs if applicable.
        for (big_index, group) in state.subgroups.iter_mut().enumerate() {
            if let Some(bis_sync) = bis_map.get(&(big_index as u8)) {
                group.bis_sync = bis_sync.clone();
            }
        }

        let op = ModifySourceOperation::new(
            state.source_id,
            pa_sync,
            pa_interval.unwrap_or(PeriodicAdvertisingInterval::unknown()),
            state.subgroups,
        );
        self.write_to_bascp(op).await
    }

    pub async fn remove_broadcast_source(&self, broadcast_id: BroadcastId) -> Result<(), Error> {
        let source_id = self.get_source_id(&broadcast_id)?;

        let op = RemoveSourceOperation::new(source_id);
        self.write_to_bascp(op).await
    }

    /// Sets the broadcast code for a particular broadcast stream.
    pub async fn set_broadcast_code(
        &self,
        broadcast_id: BroadcastId,
        broadcast_code: [u8; 16],
    ) -> Result<(), Error> {
        let source_id = self.get_source_id(&broadcast_id)?;

        let op = SetBroadcastCodeOperation::new(source_id, broadcast_code.clone());
        self.write_to_bascp(op).await?;

        // Save the broadcast code we sent.
        self.broadcast_codes.lock().insert(source_id, broadcast_code);
        Ok(())
    }

    /// Returns a list of currently known broadcast sources at the time
    /// this method was called.
    pub fn known_broadcast_sources(&self) -> Vec<(Handle, BroadcastReceiveState)> {
        let lock = self.broadcast_sources.lock();
        let mut brs = Vec::new();
        for (k, v) in lock.0.iter() {
            brs.push((*k, v.clone()));
        }
        brs
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn insert_broadcast_receive_state(&mut self, handle: Handle, brs: BroadcastReceiveState) {
        self.broadcast_sources.lock().update_state(handle, brs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::task::Poll;

    use assert_matches::assert_matches;
    use futures::executor::block_on;
    use futures::{pin_mut, FutureExt};

    use bt_common::core::AdvertisingSetId;
    use bt_common::Uuid;
    use bt_gatt::test_utils::*;
    use bt_gatt::types::{
        AttributePermissions, CharacteristicProperties, CharacteristicProperty, Handle,
    };
    use bt_gatt::Characteristic;

    const RECEIVE_STATE_1_HANDLE: Handle = Handle(1);
    const RECEIVE_STATE_2_HANDLE: Handle = Handle(2);
    const RECEIVE_STATE_3_HANDLE: Handle = Handle(3);
    const RANDOME_CHAR_HANDLE: Handle = Handle(4);
    const AUDIO_SCAN_CONTROL_POINT_HANDLE: Handle = Handle(5);

    fn setup_client() -> (BroadcastAudioScanServiceClient<FakeTypes>, FakePeerService) {
        let mut fake_peer_service = FakePeerService::new();
        // Add 3 Broadcast Receive State Characteristics, 1 Broadcast Audio Scan Control
        // Point Characteristic, and 1 random one.
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: RECEIVE_STATE_1_HANDLE,
                uuid: BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: RECEIVE_STATE_2_HANDLE,
                uuid: BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: RECEIVE_STATE_3_HANDLE,
                uuid: BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: RANDOME_CHAR_HANDLE,
                uuid: Uuid::from_u16(0x1234),
                properties: CharacteristicProperties(vec![CharacteristicProperty::Notify]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: AUDIO_SCAN_CONTROL_POINT_HANDLE,
                uuid: BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID,
                properties: CharacteristicProperties(vec![CharacteristicProperty::Broadcast]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BroadcastAudioScanServiceClient::<FakeTypes>::create(fake_peer_service.clone());
        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Ok(client)) = polled else {
            panic!("Expected BroadcastAudioScanServiceClient to be succesfully created");
        };

        (client, fake_peer_service)
    }

    #[test]
    fn create_client() {
        let (client, _) = setup_client();

        // Check that all the characteristics have been discovered.
        assert_eq!(client.audio_scan_control_point, AUDIO_SCAN_CONTROL_POINT_HANDLE);
        let broadcast_sources = client.known_broadcast_sources();
        assert_eq!(broadcast_sources.len(), 3);
        assert!(broadcast_sources.iter().find(|v| v.0 == RECEIVE_STATE_1_HANDLE).is_some());
        assert!(broadcast_sources.iter().find(|v| v.0 == RECEIVE_STATE_2_HANDLE).is_some());
        assert!(broadcast_sources.iter().find(|v| v.0 == RECEIVE_STATE_3_HANDLE).is_some());
    }

    #[test]
    fn create_client_fails_missing_characteristics() {
        // Missing scan control point characteristic.
        let mut fake_peer_service = FakePeerService::new();
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: Handle(1),
                uuid: BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BroadcastAudioScanServiceClient::<FakeTypes>::create(fake_peer_service.clone());

        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(_)) = polled else {
            panic!("Expected BroadcastAudioScanServiceClient to have failed");
        };

        // Missing receive state characteristic.
        let mut fake_peer_service: FakePeerService = FakePeerService::new();
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: Handle(1),
                uuid: BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID,
                properties: CharacteristicProperties(vec![CharacteristicProperty::Broadcast]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BroadcastAudioScanServiceClient::<FakeTypes>::create(fake_peer_service.clone());
        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(_)) = polled else {
            panic!("Expected BroadcastAudioScanServiceClient to have failed");
        };
    }

    #[test]
    fn create_client_fails_duplicate_characteristics() {
        // More than one scan control point characteristics.
        let mut fake_peer_service = FakePeerService::new();
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: Handle(1),
                uuid: BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: Handle(2),
                uuid: BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID,
                properties: CharacteristicProperties(vec![CharacteristicProperty::Broadcast]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: Handle(3),
                uuid: BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID,
                properties: CharacteristicProperties(vec![CharacteristicProperty::Broadcast]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let create_result =
            BroadcastAudioScanServiceClient::<FakeTypes>::create(fake_peer_service.clone());
        pin_mut!(create_result);
        let polled = create_result.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(_)) = polled else {
            panic!("Expected BroadcastAudioScanServiceClient to have failed");
        };
    }

    #[test]
    fn start_event_stream() {
        let (mut client, mut fake_peer_service) = setup_client();
        let mut event_stream = client.take_event_stream().expect("stream was created");

        // Send notification for updating BRS characteristic to indicate it's synced and
        // requires broadcast code.
        #[rustfmt::skip]
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: RECEIVE_STATE_2_HANDLE,
                uuid: BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![
                0x02, AddressType::Public as u8,                      // source id and address type
                0x02, 0x03, 0x04, 0x05, 0x06, 0x07,                   // address
                0x01, 0x02, 0x03, 0x04,                               // ad set id and broadcast id
                PaSyncState::Synced as u8,
                EncryptionStatus::BroadcastCodeRequired.raw_value(),
                0x00,                                                 // no subgroups
            ],
        );

        // Check that synced and broadcast code required events were sent out.
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

        let recv_fut = event_stream.select_next_some();
        let event = block_on(recv_fut).expect("should receive event");
        assert_eq!(
            event,
            Event::AddedBroadcastSource(
                BroadcastId::try_from(0x040302).unwrap(),
                PaSyncState::Synced,
                EncryptionStatus::BroadcastCodeRequired
            )
        );

        // Stream should be pending since no more notifications.
        assert!(event_stream.poll_next_unpin(&mut noop_cx).is_pending());

        // Send notification for updating BRS characteristic to indicate it requires
        // sync info. Notification for updating the BRS characteristic value for
        // characteristic with handle 3.
        #[rustfmt::skip]
        fake_peer_service.add_characteristic(
            Characteristic {
                handle: RECEIVE_STATE_3_HANDLE,
                uuid: BROADCAST_RECEIVE_STATE_UUID,
                properties: CharacteristicProperties(vec![
                    CharacteristicProperty::Broadcast,
                    CharacteristicProperty::Notify,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![
                0x03, AddressType::Public as u8,             // source id and address type
                0x03, 0x04, 0x05, 0x06, 0x07, 0x08,          // address
                0x01, 0x03, 0x04, 0x05,                      // ad set id and broadcast id
                PaSyncState::SyncInfoRequest as u8,
                EncryptionStatus::NotEncrypted.raw_value(),
                0x00,                                        // no subgroups
            ],
        );

        let recv_fut = event_stream.select_next_some();
        let event = block_on(recv_fut).expect("should receive event");
        assert_eq!(
            event,
            Event::AddedBroadcastSource(
                BroadcastId::try_from(0x050403).unwrap(),
                PaSyncState::SyncInfoRequest,
                EncryptionStatus::NotEncrypted
            )
        );

        // Stream should be pending since no more notifications.
        assert!(event_stream.poll_next_unpin(&mut noop_cx).is_pending());
    }

    #[test]
    fn remote_scan_started() {
        let (client, mut fake_peer_service) = setup_client();

        fake_peer_service.expect_characteristic_value(&AUDIO_SCAN_CONTROL_POINT_HANDLE, vec![0x01]);

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let op_fut = client.remote_scan_started();
        pin_mut!(op_fut);
        let polled = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn remote_scan_stopped() {
        let (client, mut fake_peer_service) = setup_client();

        fake_peer_service.expect_characteristic_value(&AUDIO_SCAN_CONTROL_POINT_HANDLE, vec![0x00]);

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let op_fut = client.remote_scan_stopped();
        pin_mut!(op_fut);
        let polled = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn add_broadcast_source() {
        let (client, mut fake_peer_service) = setup_client();

        fake_peer_service.expect_characteristic_value(
            &AUDIO_SCAN_CONTROL_POINT_HANDLE,
            vec![
                0x02, 0x00, 0x04, 0x10, 0x00, 0x00, 0x00, 0x00, 0x01, 0x11, 0x00, 0x00, 0x00, 0xFF,
                0xFF, 0x00,
            ],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let op_fut = client.add_broadcast_source(
            BroadcastId::try_from(0x11).unwrap(),
            AddressType::Public,
            [0x04, 0x10, 0x00, 0x00, 0x00, 0x00],
            AdvertisingSetId::try_from(1).unwrap(),
            PaSync::DoNotSync,
            PeriodicAdvertisingInterval::unknown(),
            vec![],
        );
        pin_mut!(op_fut);
        let polled = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn modify_broadcast_source() {
        let (client, mut fake_peer_service) = setup_client();

        // Manually update the broadcast source tracker for testing purposes.
        // In practice, this would have been updated from BRS value change notification.
        client.broadcast_sources.lock().update_state(
            RECEIVE_STATE_1_HANDLE,
            BroadcastReceiveState::NonEmpty(ReceiveState {
                source_id: 0x11,
                source_address_type: AddressType::Public,
                source_address: [1, 2, 3, 4, 5, 6],
                source_adv_sid: AdvertisingSetId::try_from(1).unwrap(),
                broadcast_id: BroadcastId::try_from(0x11).unwrap(),
                pa_sync_state: PaSyncState::Synced,
                big_encryption: EncryptionStatus::BroadcastCodeRequired,
                subgroups: vec![],
            }),
        );

        #[rustfmt::skip]
        fake_peer_service.expect_characteristic_value(
            &AUDIO_SCAN_CONTROL_POINT_HANDLE,
            vec![
                0x03, 0x11, 0x00,  // opcode, source id, pa sync
                0xAA, 0xAA, 0x00,  // pa sync, pa interval, num of subgroups
            ],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let op_fut = client.modify_broadcast_source(
            BroadcastId::try_from(0x11).unwrap(),
            PaSync::DoNotSync,
            Some(PeriodicAdvertisingInterval(0xAAAA)),
            HashMap::new(),
            None,
        );
        pin_mut!(op_fut);
        let polled = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn modify_broadcast_source_updates_groups() {
        let (client, mut fake_peer_service) = setup_client();

        // Manually update the broadcast source tracker for testing purposes.
        // In practice, this would have been updated from BRS value change notification.
        client.broadcast_sources.lock().update_state(
            RECEIVE_STATE_1_HANDLE,
            BroadcastReceiveState::NonEmpty(ReceiveState {
                source_id: 0x11,
                source_address_type: AddressType::Public,
                source_address: [1, 2, 3, 4, 5, 6],
                source_adv_sid: AdvertisingSetId::try_from(1).unwrap(),
                broadcast_id: BroadcastId::try_from(0x11).unwrap(),
                pa_sync_state: PaSyncState::Synced,
                big_encryption: EncryptionStatus::BroadcastCodeRequired,
                subgroups: vec![BigSubgroup::new(None)],
            }),
        );

        // Default PA interval value and subgroups value read from the BRS
        // characteristic are used.
        #[rustfmt::skip]
        fake_peer_service.expect_characteristic_value(
            &AUDIO_SCAN_CONTROL_POINT_HANDLE,
            vec![
                0x03, 0x11, 0x00,                    // opcode, source id, pa sync
                0xFF, 0xFF, 0x02,                    // pa sync, pa interval, num of subgroups
                0x15, 0x00, 0x00, 0x00,              // bis sync (0th subgroup)
                0x02, 0x01, 0x09,                    // metadata len, metadata
                0xFF, 0xFF, 0xFF, 0xFF,              // bis sync (1st subgroup)
                0x05, 0x04, 0x04, 0x65, 0x6E, 0x67,  // metadata len, metadata
            ],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let op_fut = client.modify_broadcast_source(
            BroadcastId::try_from(0x11).unwrap(),
            PaSync::DoNotSync,
            None,
            HashMap::from([(0, BisSync::sync(vec![1, 3, 5]).unwrap())]),
            Some(HashMap::from([
                (0, vec![Metadata::BroadcastAudioImmediateRenderingFlag]),
                (1, vec![Metadata::Language("eng".to_string())]),
                (5, vec![Metadata::ProgramInfoURI("this subgroup shouldn't be added".to_string())]),
            ])),
        );
        pin_mut!(op_fut);
        let polled: Poll<Result<(), Error>> = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn modify_broadcast_source_applies_bis_sync_to_new_subgroups() {
        let (client, mut fake_peer_service) = setup_client();

        client.broadcast_sources.lock().update_state(
            RECEIVE_STATE_1_HANDLE,
            BroadcastReceiveState::NonEmpty(ReceiveState {
                source_id: 0x11,
                source_address_type: AddressType::Public,
                source_address: [1, 2, 3, 4, 5, 6],
                source_adv_sid: AdvertisingSetId::try_from(1).unwrap(),
                broadcast_id: BroadcastId::try_from(0x11).unwrap(),
                pa_sync_state: PaSyncState::Synced,
                big_encryption: EncryptionStatus::BroadcastCodeRequired,
                subgroups: vec![BigSubgroup::new(None)],
            }),
        );

        #[rustfmt::skip]
        fake_peer_service.expect_characteristic_value(
            &AUDIO_SCAN_CONTROL_POINT_HANDLE,
            vec![
                0x03, 0x11, 0x00,                    // opcode, source id, pa sync
                0xFF, 0xFF, 0x02,                    // pa sync, pa interval, num of subgroups
                0x15, 0x00, 0x00, 0x00,              // bis sync (0th subgroup)
                0x02, 0x01, 0x09,                    // metadata len, metadata
                0x0A, 0x00, 0x00, 0x00,              // bis sync for 1st subgroup (BIS index 2 and 4 = 0b00001010)
                0x05, 0x04, 0x04, 0x65, 0x6E, 0x67,  // metadata len, metadata
            ],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let op_fut = client.modify_broadcast_source(
            BroadcastId::try_from(0x11).unwrap(),
            PaSync::DoNotSync,
            None,
            HashMap::from([
                (0, BisSync::sync(vec![1, 3, 5]).unwrap()),
                (1, BisSync::sync(vec![2, 4]).unwrap()),
            ]),
            Some(HashMap::from([
                (0, vec![Metadata::BroadcastAudioImmediateRenderingFlag]),
                (1, vec![Metadata::Language("eng".to_string())]),
            ])),
        );
        pin_mut!(op_fut);
        let polled: Poll<Result<(), Error>> = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn modify_broadcast_source_fail() {
        let (client, _fake_peer_service) = setup_client();

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        // Broadcast source wasn't previously added.
        let op_fut = client.modify_broadcast_source(
            BroadcastId::try_from(0x11).unwrap(),
            PaSync::DoNotSync,
            None,
            HashMap::new(),
            None,
        );
        pin_mut!(op_fut);
        let polled = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Err(_)));
    }

    #[test]
    fn remove_broadcast_source() {
        let (client, mut fake_peer_service) = setup_client();
        let bid = BroadcastId::try_from(0x11).expect("should not fail");

        // Manually update the broadcast source tracker for testing purposes.
        // In practice, this would have been updated from BRS value change notification.
        client.broadcast_sources.lock().update_state(
            RECEIVE_STATE_1_HANDLE,
            BroadcastReceiveState::NonEmpty(ReceiveState {
                source_id: 0x11,
                source_address_type: AddressType::Public,
                source_address: [1, 2, 3, 4, 5, 6],
                source_adv_sid: AdvertisingSetId::try_from(1).unwrap(),
                broadcast_id: bid,
                pa_sync_state: PaSyncState::Synced,
                big_encryption: EncryptionStatus::BroadcastCodeRequired,
                subgroups: vec![],
            }),
        );

        fake_peer_service
            .expect_characteristic_value(&AUDIO_SCAN_CONTROL_POINT_HANDLE, vec![0x05, 0x11]);

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        // Broadcast source wasn't previously added.
        let op_fut = client.remove_broadcast_source(bid);
        pin_mut!(op_fut);
        let polled = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn remove_broadcast_source_fail() {
        let (client, _fake_peer_service) = setup_client();

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        // Broadcast source wasn't previously added.
        let op_fut = client.remove_broadcast_source(BroadcastId::try_from(0x11).unwrap());
        pin_mut!(op_fut);
        let polled = op_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Err(_)));
    }

    #[test]
    fn set_broadcast_code() {
        let (client, mut fake_peer_service) = setup_client();

        // Manually update the broadcast source tracker for testing purposes.
        // In practice, this would have been updated from BRS value change notification.
        client.broadcast_sources.lock().update_state(
            RECEIVE_STATE_1_HANDLE,
            BroadcastReceiveState::NonEmpty(ReceiveState {
                source_id: 0x01,
                source_address_type: AddressType::Public,
                source_address: [1, 2, 3, 4, 5, 6],
                source_adv_sid: AdvertisingSetId::try_from(1).unwrap(),
                broadcast_id: BroadcastId::try_from(0x030201).unwrap(),
                pa_sync_state: PaSyncState::Synced,
                big_encryption: EncryptionStatus::BroadcastCodeRequired,
                subgroups: vec![],
            }),
        );

        fake_peer_service.expect_characteristic_value(
            &AUDIO_SCAN_CONTROL_POINT_HANDLE,
            vec![0x04, 0x01, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
        );

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let set_code_fut =
            client.set_broadcast_code(BroadcastId::try_from(0x030201).unwrap(), [1; 16]);
        pin_mut!(set_code_fut);
        let polled = set_code_fut.poll_unpin(&mut noop_cx);
        assert_matches!(polled, Poll::Ready(Ok(_)));
    }

    #[test]
    fn set_broadcast_code_fails() {
        let (client, _) = setup_client();

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let set_code_fut =
            client.set_broadcast_code(BroadcastId::try_from(0x030201).unwrap(), [1; 16]);
        pin_mut!(set_code_fut);
        let polled = set_code_fut.poll_unpin(&mut noop_cx);

        // Should fail because we cannot get source id for the broadcast id since BRS
        // Characteristic value wasn't updated.
        assert_matches!(polled, Poll::Ready(Err(_)));
    }
}
