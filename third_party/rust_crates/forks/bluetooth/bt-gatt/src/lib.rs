// Copyright 2023 The Sapphire Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod types;
pub use types::{Characteristic, Descriptor, Result};

pub mod server;
pub use server::Server;

pub mod client;
pub use client::Client;

pub mod central;
pub use central::Central;

pub mod periodic_advertising;
pub mod pii;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

#[cfg(test)]
mod tests;

use futures::{Future, Stream};

use crate::periodic_advertising::PeriodicAdvertising;

/// Implementors implement traits with respect to GattTypes.
/// Implementation crates provide an object which relates a constellation of
/// types to each other, used to provide concrete types for library crates to
/// paramaterize on.
pub trait GattTypes: Sized {
    // Types related to finding and connecting to peers
    type Central: Central<Self>;
    type ScanResultStream: Stream<Item = Result<central::ScanResult>> + 'static;
    type Client: Client<Self>;
    type ConnectFuture: Future<Output = Result<Self::Client>>;

    // Types related to finding and connecting to services
    type PeerServiceHandle: client::PeerServiceHandle<Self>;
    type FindServicesFut: Future<Output = Result<Vec<Self::PeerServiceHandle>>> + 'static;
    type PeerService: client::PeerService<Self>;
    type ServiceConnectFut: Future<Output = Result<Self::PeerService>>;

    // Types related to interacting with services
    /// Future returned by PeerService::discover_characteristics,
    /// delivering the set of characteristics discovered within a service
    type CharacteristicDiscoveryFut: Future<Output = Result<Vec<Characteristic>>>;
    /// A stream of notifications, delivering updates to a characteristic value
    /// that has been subscribed to.  See [`client::PeerService::subscribe`]
    type NotificationStream: Stream<Item = Result<client::CharacteristicNotification>> + 'static;
    /// Future resolving when a characteristic or descriptor has been read.
    /// Resolves to a number of bytes read along with a boolean indicating if
    /// the value was possibly truncated, or an Error if the value could not
    /// be read.
    type ReadFut<'a>: Future<Output = Result<(usize, bool)>> + 'a;
    /// Future resolving when a characteristic or descriptor has been written.
    /// Returns an error if the value could not be written.
    type WriteFut<'a>: Future<Output = Result<()>> + 'a;
    /// The implementation of periodic advertising for this GATT implementation.
    type PeriodicAdvertising: PeriodicAdvertising;
}

/// Servers and services are defined with respect to ServerTypes.
/// Implementation crates provide an object which relates this constellation of
/// types to each other, used to provide concrete types for library crates to
/// paramaterize on.
pub trait ServerTypes: Sized {
    type Server: Server<Self>;
    type LocalService: server::LocalService<Self>;
    type LocalServiceFut: Future<Output = Result<Self::LocalService>>;
    type ServiceEventStream: Stream<Item = Result<server::ServiceEvent<Self>>>;
    type ServiceWriteType: ToOwned<Owned = Vec<u8>>;
    type ReadResponder: server::ReadResponder;
    type WriteResponder: server::WriteResponder;

    type IndicateConfirmationStream: Stream<Item = Result<server::ConfirmationEvent>>;
}
