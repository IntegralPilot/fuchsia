// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;

use bt_common::{PeerId, Uuid};

use crate::types::*;

/// ServiceId is used to identify a local service when publishing.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ServiceId(u64);

impl ServiceId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
}

impl From<ServiceId> for u64 {
    fn from(value: ServiceId) -> Self {
        value.0
    }
}

/// Defines a service to be added to a Server
#[derive(Debug, Clone)]
pub struct ServiceDefinition {
    /// Local identifier used to include services within each other.
    id: ServiceId,
    /// UUID identifying the type of service
    uuid: Uuid,
    /// Whether the service is marked as Primary in the GATT server.
    kind: ServiceKind,
    /// Characteristics in the service. Add with
    /// [ServiceDefinition::add_characteristic]
    characteristics: Vec<Characteristic>,
    included_services: HashSet<ServiceId>,
    /// Set of handles (characteristic or descriptor) used, to verify uniquness
    /// of new handles added.
    handles: HashSet<Handle>,
}

impl ServiceDefinition {
    /// Make a new, empty service with a given id.
    pub fn new(id: ServiceId, uuid: Uuid, kind: ServiceKind) -> Self {
        Self {
            id,
            uuid,
            kind,
            characteristics: Default::default(),
            included_services: Default::default(),
            handles: HashSet::new(),
        }
    }

    /// Add a characteristic to the definition.  If any duplicate handles or an
    /// invalid configuration of descriptors are detected, an error is
    /// returned.
    pub fn add_characteristic(&mut self, characteristic: Characteristic) -> Result<()> {
        let new_handles = characteristic.handles().collect();
        if !self.handles.is_disjoint(&new_handles) {
            return Err(Error::DuplicateHandle(
                self.handles.intersection(&new_handles).copied().collect(),
            ));
        }
        self.handles.extend(new_handles);
        // TODO: check for more errors
        self.characteristics.push(characteristic);
        Ok(())
    }

    pub fn id(&self) -> ServiceId {
        self.id
    }

    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    pub fn kind(&self) -> ServiceKind {
        self.kind
    }

    pub fn characteristics(&self) -> impl Iterator<Item = &Characteristic> {
        self.characteristics.iter()
    }

    pub fn included(&self) -> impl Iterator<Item = ServiceId> + '_ {
        self.included_services.iter().cloned()
    }

    /// Add a service to the definition.
    pub fn add_service(&mut self, id: ServiceId) {
        self.included_services.insert(id);
    }
}

/// Services can be included in other services, and are included in the database
/// when they are published.  All included services should be prepared before
/// the service including them.  Publishing a service that includes other
/// services will publish the included services, although the events associated
/// with the included service will not be returned until the
/// [LocalService::publish] is called.
pub trait Server<T: crate::ServerTypes> {
    /// Prepare to publish a service.
    /// This service is not immediately visible in the local GATT server.
    /// It will be published when the LocalService::publish is called.
    /// If the returned LocalService is dropped, the service will be removed
    /// from the Server.
    fn prepare(&self, service: ServiceDefinition) -> T::LocalServiceFut;
}

pub trait LocalService<T: crate::ServerTypes> {
    /// Publish the service.
    /// Returns an EventStream providing Events to be processed by the local
    /// service implementation.
    /// Events will only be delivered to one ServiceEventStream at a time.
    /// Calling publish while a previous ServiceEventStream is still active
    /// will return a stream with only Err(AlreadyPublished).
    fn publish(&self) -> T::ServiceEventStream;

    /// Notify a characteristic.
    /// Leave `peers` empty to notify all peers who have configured
    /// notifications. Peers that have not configured for notifications will
    /// not be notified.
    fn notify(&self, characteristic: &Handle, data: &[u8], peers: &[PeerId]);

    /// Indicate on a characteristic.
    /// Leave `peers` empty to notify all peers who have configured
    /// indications. Peers that have not configured for indications will
    /// be skipped. Returns a stream which has items for each peer that
    /// confirms the notification, and terminates when all peers have either
    /// timed out or confirmed.
    fn indicate(
        &self,
        characteristic: &Handle,
        data: &[u8],
        peers: &[PeerId],
    ) -> T::IndicateConfirmationStream;
}

pub struct ConfirmationEvent {
    peer_id: PeerId,
    result: Result<()>,
}

impl ConfirmationEvent {
    pub fn create_ack(peer_id: PeerId) -> Self {
        Self { peer_id, result: Ok(()) }
    }

    pub fn create_error(peer_id: PeerId, error: Error) -> Self {
        Self { peer_id, result: Err(error) }
    }
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn error(&self) -> Option<&Error> {
        self.result.as_ref().err()
    }

    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    pub fn is_err(&self) -> bool {
        self.result.is_err()
    }
}

/// Responder that can send data that has been read from a characteristic.
pub trait ReadResponder {
    /// Respond with the data requested.  `value` may be shorter than requested.
    fn respond(self, value: &[u8]);
    /// Respond with an error.
    fn error(self, error: GattError);
}

/// Responder that can acknowledge a write to a characteristic.
pub trait WriteResponder {
    /// Acknowledge the write. Will only send an acknowledgement if allowed by
    /// the GATT protocol.
    fn acknowledge(self);
    /// Respond with an error.
    fn error(self, error: GattError);
}

#[derive(Debug)]
pub enum NotificationType {
    Disable,
    Notify,
    Indicate,
}

#[non_exhaustive]
pub enum ServiceEvent<T: crate::ServerTypes> {
    /// Peer requests to read from a handle (characteritic or descriptor) at the
    /// given offset.
    Read { peer_id: PeerId, handle: Handle, offset: u32, responder: T::ReadResponder },
    /// Peer has written a value to a handle (characteristic or descriptor) at
    /// the given offset.
    Write {
        peer_id: PeerId,
        handle: Handle,
        offset: u32,
        value: T::ServiceWriteType,
        responder: T::WriteResponder,
    },
    /// Notification that a peer has configured a characteristic for indication
    /// or notification.
    ClientConfiguration { peer_id: PeerId, handle: Handle, notification_type: NotificationType },
    /// Extra information about a peer is provided. This event may not be sent
    /// by all implementations.
    #[non_exhaustive]
    PeerInfo { peer_id: PeerId, mtu: Option<u16>, connected: Option<bool> },
}

impl<T: crate::ServerTypes> ServiceEvent<T> {
    pub fn peer_id(&self) -> PeerId {
        match self {
            Self::Read { peer_id, .. } => *peer_id,
            Self::Write { peer_id, .. } => *peer_id,
            Self::ClientConfiguration { peer_id, .. } => *peer_id,
            Self::PeerInfo { peer_id, .. } => *peer_id,
        }
    }

    pub fn peer_info(peer_id: PeerId, mtu: Option<u16>, connected: Option<bool>) -> Self {
        Self::PeerInfo { peer_id, mtu, connected }
    }
}
