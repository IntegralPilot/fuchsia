// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::*;

use bt_common::{PeerId, Uuid};

/// GATT Client connected to a particular peer.
/// Holding a struct that implements this should attempt to maintain a LE
/// connection to the peer.
pub trait Client<T: crate::GattTypes> {
    /// The ID of the peer this is connected to.
    fn peer_id(&self) -> PeerId;

    /// Find services by UUID on the peer.
    /// This may cause as much as a full discovery of all services on the peer
    /// if the stack deems it appropriate.
    /// Service information should be up to date at the time returned.
    fn find_service(&self, uuid: Uuid) -> T::FindServicesFut;
}

pub trait PeerServiceHandle<T: crate::GattTypes> {
    fn uuid(&self) -> Uuid;
    fn is_primary(&self) -> bool;
    fn connect(&self) -> T::ServiceConnectFut;
}

/// Implement when a type can be deserialized from a characteristic value.
pub trait FromCharacteristic: Sized {
    const UUID: Uuid;

    /// Create this type from a Characteristic and an initial value.
    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> ::core::result::Result<Self, bt_common::packet_encoding::Error>;

    /// Attempt to update the type when supplied with the `new_value`, which may
    /// or may not be the complete value.
    fn update(
        &mut self,
        new_value: &[u8],
    ) -> ::core::result::Result<&mut Self, bt_common::packet_encoding::Error>;

    /// Attempt to read a characteristic if it matches the provided
    /// characteristic UUID.
    fn try_read<T: crate::GattTypes>(
        characteristic: Characteristic,
        service: &T::PeerService,
    ) -> impl futures::Future<Output = ::core::result::Result<Self, Error>> {
        async move {
            if characteristic.uuid != Self::UUID {
                return Err(Error::ScanFailed("Wrong UUID".to_owned()));
            }
            let mut buf = [0; 128];
            let (bytes, mut truncated) =
                service.read_characteristic(&characteristic.handle, 0, &mut buf).await?;
            let mut vec;
            let buf_ptr = if truncated {
                vec = Vec::with_capacity(bytes);
                vec.copy_from_slice(&buf[..bytes]);
                while truncated {
                    let (bytes, still_truncated) = service
                        .read_characteristic(&characteristic.handle, vec.len() as u16, &mut buf)
                        .await?;
                    vec.extend_from_slice(&buf[..bytes]);
                    truncated = still_truncated;
                }
                &vec[..]
            } else {
                &buf[..bytes]
            };
            Self::from_chr(characteristic, buf_ptr).map_err(Into::into)
        }
    }
}

#[derive(Debug, Clone)]
pub struct CharacteristicNotification {
    pub handle: Handle,
    pub value: Vec<u8>,
    pub maybe_truncated: bool,
}

/// A connection to a GATT Service on a Peer.
/// All operations are done synchronously.
pub trait PeerService<T: crate::GattTypes> {
    /// Discover characteristics on this service.
    /// If `uuid` is provided, only the characteristics matching `uuid` will be
    /// returned. This operation may use either the Discovery All
    /// Characteristics of a Service or Discovery Characteristic by UUID
    /// procedures, regardless of `uuid`.
    fn discover_characteristics(&self, uuid: Option<Uuid>) -> T::CharacteristicDiscoveryFut;

    /// Read a characteristic into a buffer, given the handle within the
    /// service. On success, returns the size read and whether the value may
    /// have been truncated. By default this will try to use a long read if
    /// the `buf` is larger than a normal read will allow (22 bytes) or if
    /// the offset is non-zero.
    fn read_characteristic<'a>(
        &self,
        handle: &Handle,
        offset: u16,
        buf: &'a mut [u8],
    ) -> T::ReadFut<'a>;

    fn write_characteristic<'a>(
        &self,
        handle: &Handle,
        mode: WriteMode,
        offset: u16,
        buf: &'a [u8],
    ) -> T::WriteFut<'a>;

    fn read_descriptor<'a>(
        &self,
        handle: &Handle,
        offset: u16,
        buf: &'a mut [u8],
    ) -> T::ReadFut<'a>;

    fn write_descriptor<'a>(&self, handle: &Handle, offset: u16, buf: &'a [u8]) -> T::WriteFut<'a>;

    /// Subscribe to updates on a Characteristic.
    /// Either notifications or indications will be enabled depending on the
    /// properties available, with indications preferred if they are
    /// supported. Fails if the Characteristic doesn't support indications
    /// or notifications. Errors are delivered through an Err item in the
    /// stream. This will often write to the Client Characteristic
    /// Configuration descriptor for the Characteristic subscribed to.
    /// Updates sent from the peer will be delivered to the Stream returned.
    fn subscribe(&self, handle: &Handle) -> T::NotificationStream;

    // TODO: Find included services
}

/// Convenience class for communicating with characteristics on a remote peer.
pub struct ServiceCharacteristic<'a, T: crate::GattTypes> {
    service: &'a T::PeerService,
    characteristic: Characteristic,
    uuid: Uuid,
}

impl<'a, T: crate::GattTypes> ServiceCharacteristic<'a, T> {
    pub async fn find(
        service: &'a T::PeerService,
        uuid: Uuid,
    ) -> Result<Vec<ServiceCharacteristic<'a, T>>> {
        let chrs = service.discover_characteristics(Some(uuid)).await?;
        Ok(chrs.into_iter().map(|characteristic| Self { service, characteristic, uuid }).collect())
    }

    pub async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        self.service.read_characteristic(self.handle(), 0, buf).await.map(|(bytes, _)| bytes)
    }

    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    pub fn handle(&self) -> &Handle {
        &self.characteristic.handle
    }

    pub fn characteristic(&self) -> &Characteristic {
        &self.characteristic
    }
}
