// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Contains traits that are used to find and connect to Low Energy Peers, i.e.
//! the GAP Central and Observer roles role defined in the Bluetooth Core
//! Specification (5.4, Volume 3 Part C Section 2.2.2)
//!
//! These traits should be implemented outside this crate, conforming to the
//! types and structs here when necessary.

use bt_common::{PeerId, Uuid};

#[derive(Debug, Clone)]
pub enum AdvertisingDatum {
    Services(Vec<Uuid>),
    ServiceData(Uuid, Vec<u8>),
    ManufacturerData(u16, Vec<u8>),
    // TODO: Update to a more structured Appearance
    Appearance(u16),
    TxPowerLevel(i8),
    Uri(String),
    BroadcastName(String),
    ResolvableSetIdentifier([u8; 6]),
}

/// Matches a single advertised attribute or condition from a Bluetooth Low
/// Energy peer.
#[derive(Clone, Debug)]
pub enum Filter {
    /// Advertised Service UUID
    ServiceUuid(Uuid),
    /// ServiceData is included which is associated with the UUID
    HasServiceData(Uuid),
    /// ManufacturerData is provided with the Company Identifier Code given
    HasManufacturerData(u16),
    /// Connectable flag is set
    IsConnectable,
    /// String provided is included in the peer's name
    MatchesName(String),
    /// Path loss from the peer (RSSI - Advertised TX Power) is below the given
    /// dB value
    MaxPathLoss(i8),
}

/// A ScanFilter must match all of its combined filters and conditions to
/// provide a result. Currently can only include zero or more Filters.
/// The Default ScanFilter will match everything and should be avoided.
#[derive(Default, Clone, Debug)]
pub struct ScanFilter {
    pub filters: Vec<Filter>,
}

impl From<Filter> for ScanFilter {
    fn from(value: Filter) -> Self {
        ScanFilter { filters: vec![value] }
    }
}

impl ScanFilter {
    pub fn add(&mut self, filter: Filter) -> &mut Self {
        self.filters.push(filter);
        self
    }
}

#[derive(Debug, Clone)]
pub enum PeerName {
    Unknown,
    PartialName(String),
    CompleteName(String),
}

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub id: PeerId,
    pub connectable: bool,
    pub name: PeerName,
    pub advertised: Vec<AdvertisingDatum>,
    pub advertising_sid: Option<u8>,
    pub periodic_advertising_interval: Option<u16>,
}

pub trait Central<T: crate::GattTypes> {
    /// Scan for peers.
    /// If any of the filters match, the results will be returned in the Stream.
    fn scan(&self, filters: &[ScanFilter]) -> T::ScanResultStream;

    /// Connect to a specific peer.
    fn connect(&self, peer_id: PeerId) -> T::ConnectFuture;

    /// Get API for periodic advertising.
    fn periodic_advertising(&self) -> crate::Result<T::PeriodicAdvertising>;
}
