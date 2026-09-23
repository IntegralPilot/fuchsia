// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements the Broadcast Audio Scan Service (BASS) server role.

use bt_gatt::server::{LocalService, Server as _, ServiceDefinition, ServiceId};
use bt_gatt::types::{
    AttributePermissions, CharacteristicProperty, Handle, SecurityLevels, ServiceKind,
};
use bt_gatt::Characteristic;
use futures::Future;
use pin_project::pin_project;
use std::collections::HashMap;

use crate::types::{
    BroadcastReceiveState, SourceId, BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID,
    BROADCAST_AUDIO_SCAN_SERVICE_UUID, BROADCAST_RECEIVE_STATE_UUID,
};

pub mod error;
pub use error::Error;

/// Service identifier assigned to the published BASS GATT service instance.
const BASS_SERVICE_ID: ServiceId = ServiceId::new(1);

/// Handle assigned to the Broadcast Audio Scan Control Point characteristic.
const CONTROL_POINT_HANDLE: Handle = Handle(1);

/// Maximum number of Broadcast Receive State characteristics that can be hosted
/// by the server. See BASS v1.0 Section 3.2.1.
const MAX_RECEIVE_STATES: usize = 255;

/// Internal representation of a Broadcast Receive State characteristic slot.
/// See BASS v1.0 Section 3.2.
#[derive(Debug)]
pub(crate) struct PublishedReceiveStateCharacteristic {
    /// 1-indexed source identifier.
    /// See BASS v1.0 Section 3.2.1.
    source_id: SourceId,
    /// Handle assigned to this GATT characteristic.
    handle: Handle,
    /// Current Broadcast Receive State value.
    state: BroadcastReceiveState,
}

/// Internal publication state lifecycle of the BASS GATT service.
#[pin_project(project = LocalServiceProj)]
enum LocalServiceState<T: bt_gatt::ServerTypes> {
    /// Service definition has not been registered in the GATT database.
    NotPublished,
    /// Service registration is in progress.
    Preparing {
        #[pin]
        fut: T::LocalServiceFut,
    },
    /// Service registration is complete and active in the GATT database.
    Published { service: T::LocalService },
}

impl<T: bt_gatt::ServerTypes> Default for LocalServiceState<T> {
    fn default() -> Self {
        Self::NotPublished
    }
}

impl<T: bt_gatt::ServerTypes> LocalServiceState<T> {
    fn is_published(&self) -> bool {
        matches!(self, LocalServiceState::Published { .. })
    }

    fn poll_publish(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Error>> {
        match self.as_mut().project() {
            LocalServiceProj::NotPublished => std::task::Poll::Pending,
            LocalServiceProj::Preparing { fut } => {
                let service_result = futures::ready!(fut.poll(cx));
                match service_result {
                    Ok(service) => {
                        let _ = service.publish();
                        self.set(LocalServiceState::Published { service });
                        std::task::Poll::Ready(Ok(()))
                    }
                    Err(e) => {
                        self.set(LocalServiceState::NotPublished);
                        std::task::Poll::Ready(Err(Error::Gatt(e)))
                    }
                }
            }
            LocalServiceProj::Published { .. } => std::task::Poll::Ready(Ok(())),
        }
    }
}

/// Builder for constructing a BASS GATT [`Server`].
#[derive(Default)]
pub struct ServerBuilder {
    /// Staged Broadcast Receive State characteristics.
    /// There must be at least one such staged characteristic.
    receive_states: Vec<BroadcastReceiveState>,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a Broadcast Receive State characteristic slot to the builder.
    pub fn add_receive_state_characteristic(mut self, state: BroadcastReceiveState) -> Self {
        self.receive_states.push(state);
        self
    }

    /// Constructs the Broadcast Audio Scan Control Point characteristic.
    /// Defined in BASS v1.0 Section 3.1.
    fn build_control_point() -> Characteristic {
        let cp_properties =
            CharacteristicProperty::Write | CharacteristicProperty::WriteWithoutResponse;
        Characteristic {
            handle: CONTROL_POINT_HANDLE,
            uuid: BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID,
            properties: cp_properties.clone(),
            permissions: AttributePermissions::with_levels(
                &cp_properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }

    /// Constructs the Broadcast Receive State characteristic.
    /// Defined in BASS v1.0 Section 3.2.
    fn build_receive_state(handle: Handle) -> Characteristic {
        let properties = CharacteristicProperty::Read | CharacteristicProperty::Notify;
        Characteristic {
            handle,
            uuid: BROADCAST_RECEIVE_STATE_UUID,
            properties: properties.clone(),
            permissions: AttributePermissions::with_levels(
                &properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }

    /// Builds a [`Server`] instance after verifying required service
    /// characteristics.
    pub fn build<T: bt_gatt::ServerTypes>(self) -> Result<Server<T>, Error> {
        // Per BASS v1.0 Section 3.2, there must be [1,255] Broadcast Receive State
        // characteristics.
        if self.receive_states.is_empty() {
            return Err(Error::MissingReceiveState);
        }
        if self.receive_states.len() > MAX_RECEIVE_STATES {
            return Err(Error::ExceedsMaxReceiveStates);
        }

        let mut service_def = ServiceDefinition::new(
            BASS_SERVICE_ID,
            BROADCAST_AUDIO_SCAN_SERVICE_UUID,
            ServiceKind::Primary,
        );

        let _ = service_def.add_characteristic(Self::build_control_point());

        // Broadcast Receive State characteristics (Read, Notify; Encryption Required)
        const FIRST_RECEIVE_STATE_HANDLE: Handle = Handle(2);
        let num_receive_states = self.receive_states.len();
        let mut receive_state_characteristics = HashMap::with_capacity(num_receive_states);
        let mut source_id_to_handle = HashMap::with_capacity(num_receive_states);
        for (i, mut state) in self.receive_states.into_iter().enumerate() {
            let source_id = (i + 1) as u8;
            let handle = Handle(FIRST_RECEIVE_STATE_HANDLE.0 + i as u64);
            let _ = service_def.add_characteristic(Self::build_receive_state(handle));

            // Override Source ID as this is assigned by the server. See BASS v1.0 Section
            // 3.2.1.
            if let BroadcastReceiveState::NonEmpty(ref mut receive_state) = state {
                receive_state.source_id = source_id;
            }

            receive_state_characteristics
                .insert(handle, PublishedReceiveStateCharacteristic { source_id, handle, state });
            source_id_to_handle.insert(source_id, handle);
        }

        let next_source_id = (num_receive_states + 1) as SourceId;

        Ok(Server {
            service_def,
            local_service: Default::default(),
            control_point_handle: CONTROL_POINT_HANDLE,
            receive_state_characteristics,
            source_id_to_handle,
            next_source_id,
        })
    }
}

/// The BASS GATT Server implementation.
///
/// Manages the Control Point characteristic and one or more Broadcast Receive
/// State characteristics.
#[pin_project]
pub struct Server<T: bt_gatt::ServerTypes> {
    service_def: ServiceDefinition,
    #[pin]
    local_service: LocalServiceState<T>,
    control_point_handle: Handle,
    /// Broadcast Receive State characteristics mapped by GATT handle.
    receive_state_characteristics: HashMap<Handle, PublishedReceiveStateCharacteristic>,
    /// Map from Source ID to characteristic handle.
    source_id_to_handle: HashMap<SourceId, Handle>,
    /// Next available Source ID to be assigned to an empty Receive State
    /// characteristic.
    next_source_id: SourceId,
}

impl<T: bt_gatt::ServerTypes> Server<T> {
    /// Returns true if the server has successfully published the GATT service.
    pub fn is_published(&self) -> bool {
        self.local_service.is_published()
    }

    /// Helper for polling publication to transition from Preparing to Published
    /// state.
    // TODO(b/534436439): Remove once the Stream implementation is defined.
    #[cfg(test)]
    pub(crate) fn poll_publish(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Error>> {
        self.project().local_service.poll_publish(cx)
    }

    /// Publishes the service to the GATT database.
    pub fn publish(&mut self, server: T::Server) -> Result<(), Error> {
        if matches!(
            self.local_service,
            LocalServiceState::Preparing { .. } | LocalServiceState::Published { .. }
        ) {
            return Err(Error::AlreadyPublished);
        }

        let LocalServiceState::NotPublished = std::mem::replace(
            &mut self.local_service,
            LocalServiceState::Preparing { fut: server.prepare(self.service_def.clone()) },
        ) else {
            unreachable!();
        };
        Ok(())
    }

    /// Handles a read request for a characteristic on the BASS server.
    // TODO(b/534436439): Remove once the Stream implementation is defined.
    #[cfg(test)]
    fn handle_read(
        &self,
        handle: Handle,
        offset: usize,
        responder: impl bt_gatt::server::ReadResponder,
    ) {
        use bt_common::packet_encoding::Encodable;
        use bt_gatt::types::GattError;

        if handle == self.control_point_handle {
            responder.error(GattError::ReadNotPermitted);
            return;
        }

        let Some(slot) = self.receive_state_characteristics.get(&handle) else {
            responder.error(GattError::InvalidHandle);
            return;
        };

        let mut buf = vec![0u8; slot.state.encoded_len()];
        if slot.state.encode(&mut buf).is_ok() {
            if offset > buf.len() {
                responder.error(GattError::InvalidOffset);
            } else {
                responder.respond(&buf[offset..]);
            }
        } else {
            responder.error(GattError::UnlikelyError);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_bap::types::BroadcastId;
    use bt_common::core::{AddressType, AdvertisingSetId};
    use bt_gatt::server::ReadResponder;
    use bt_gatt::test_utils::{FakeServer, FakeServerEvent, FakeTypes};
    use bt_gatt::types::GattError;
    use futures::FutureExt;
    use parking_lot::Mutex;
    use std::sync::Arc;

    use crate::types::{EncryptionStatus, PaSyncState, ReceiveState};

    struct TestReadResponder {
        result: Arc<Mutex<Option<Result<Vec<u8>, GattError>>>>,
    }

    impl ReadResponder for TestReadResponder {
        fn respond(self, value: &[u8]) {
            *self.result.lock() = Some(Ok(value.to_vec()));
        }

        fn error(self, error: GattError) {
            *self.result.lock() = Some(Err(error));
        }
    }

    fn make_test_receive_state(source_id: SourceId) -> BroadcastReceiveState {
        BroadcastReceiveState::NonEmpty(ReceiveState::new(
            source_id,
            AddressType::Public,
            [0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
            AdvertisingSetId::try_from(1).unwrap(),
            BroadcastId::try_from(0x123456).unwrap(),
            PaSyncState::NotSynced,
            EncryptionStatus::NotEncrypted,
            vec![],
        ))
    }

    #[test]
    fn empty_builder_fails() {
        let builder = ServerBuilder::new();
        assert!(matches!(builder.build::<FakeTypes>(), Err(Error::MissingReceiveState)));
    }

    #[test]
    fn too_many_receive_states_fails() {
        let mut builder = ServerBuilder::new();
        for _ in 0..=MAX_RECEIVE_STATES {
            builder = builder.add_receive_state_characteristic(BroadcastReceiveState::Empty);
        }
        assert!(matches!(builder.build::<FakeTypes>(), Err(Error::ExceedsMaxReceiveStates)));
    }

    #[test]
    fn publish_server() {
        use futures::task::Context;
        use futures::StreamExt;
        use std::task::Poll;

        let mut noop_cx = Context::from_waker(futures::task::noop_waker_ref());

        let mut server = ServerBuilder::new()
            .add_receive_state_characteristic(BroadcastReceiveState::Empty)
            .add_receive_state_characteristic(make_test_receive_state(2))
            .build::<FakeTypes>()
            .expect("building server works");

        assert!(!server.is_published());

        let (fake_gatt_server, mut event_receiver) = FakeServer::new();
        let mut event_stream = event_receiver.next();

        assert!(server.publish(fake_gatt_server.clone()).is_ok());
        assert!(server.publish(fake_gatt_server).is_err());

        // Poll publish to complete preparation and transition to Published state
        assert!(std::pin::Pin::new(&mut server).poll_publish(&mut noop_cx).is_ready());
        assert!(server.is_published());

        let Poll::Ready(Some(FakeServerEvent::Published { id, definition })) =
            event_stream.poll_unpin(&mut noop_cx)
        else {
            panic!("Expected published event");
        };

        assert_eq!(id, BASS_SERVICE_ID);
        assert_eq!(definition.uuid(), BROADCAST_AUDIO_SCAN_SERVICE_UUID);
        assert_eq!(definition.characteristics().count(), 3);
    }

    #[test]
    fn read_receive_state() {
        let server = ServerBuilder::new()
            .add_receive_state_characteristic(BroadcastReceiveState::Empty)
            .add_receive_state_characteristic(make_test_receive_state(2))
            .build::<FakeTypes>()
            .expect("building server works");

        // 1. Read empty Receive State slot (Handle 2)
        let res = Arc::new(Mutex::new(None));
        server.handle_read(Handle(2), 0, TestReadResponder { result: res.clone() });
        assert_eq!(res.lock().take().unwrap().expect("ok"), vec![]);

        // 2. Read populated Receive State slot (Handle 3)
        let res = Arc::new(Mutex::new(None));
        server.handle_read(Handle(3), 0, TestReadResponder { result: res.clone() });
        assert!(!res.lock().take().unwrap().expect("ok").is_empty());

        // 3. Read unknown handle (Handle 99)
        let res = Arc::new(Mutex::new(None));
        server.handle_read(Handle(99), 0, TestReadResponder { result: res.clone() });
        assert_eq!(res.lock().take().unwrap().expect_err("err"), GattError::InvalidHandle);
    }

    #[test]
    fn read_control_point_fails() {
        let server = ServerBuilder::new()
            .add_receive_state_characteristic(BroadcastReceiveState::Empty)
            .build::<FakeTypes>()
            .expect("building server works");

        let res = Arc::new(Mutex::new(None));
        server.handle_read(CONTROL_POINT_HANDLE, 0, TestReadResponder { result: res.clone() });
        assert_eq!(res.lock().take().unwrap().expect_err("err"), GattError::ReadNotPermitted);
    }

    #[test]
    fn read_receive_state_invalid_offset() {
        let server = ServerBuilder::new()
            .add_receive_state_characteristic(BroadcastReceiveState::Empty)
            .build::<FakeTypes>()
            .expect("building server works");

        let res = Arc::new(Mutex::new(None));
        server.handle_read(Handle(2), 10, TestReadResponder { result: res.clone() });
        assert_eq!(res.lock().take().unwrap().expect_err("err"), GattError::InvalidOffset);
    }

    #[test]
    fn poll_publish_failure() {
        use futures::task::Context;
        use std::task::Poll;

        let mut noop_cx = Context::from_waker(futures::task::noop_waker_ref());

        let mut server = ServerBuilder::new()
            .add_receive_state_characteristic(BroadcastReceiveState::Empty)
            .build::<FakeTypes>()
            .expect("building server works");

        let (fake_gatt_server, _event_receiver) = FakeServer::new();
        fake_gatt_server
            .set_next_prepare_result(Err(bt_gatt::types::Error::Gatt(GattError::UnlikelyError)));

        assert!(server.publish(fake_gatt_server.clone()).is_ok());

        assert!(matches!(
            std::pin::Pin::new(&mut server).poll_publish(&mut noop_cx),
            Poll::Ready(Err(Error::Gatt(bt_gatt::types::Error::Gatt(GattError::UnlikelyError))))
        ));

        // Verifying server reset state to NotPublished so it is not published
        assert!(!server.is_published());
    }

    #[test]
    fn builder_normalizes_source_id() {
        // Create state with an arbitrary source_id = 99
        let state = make_test_receive_state(99);
        let server = ServerBuilder::new()
            .add_receive_state_characteristic(state)
            .build::<FakeTypes>()
            .expect("building server works");

        // Verify slot's internal state and encoded read value have source_id = 1
        let res = Arc::new(Mutex::new(None));
        server.handle_read(Handle(2), 0, TestReadResponder { result: res.clone() });
        let buf = res.lock().take().unwrap().expect("ok");
        assert_eq!(buf[0], 1);
    }
}
