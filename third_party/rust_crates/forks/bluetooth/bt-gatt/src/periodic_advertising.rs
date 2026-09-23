// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Contains traits that are used to synchronize to Periodic Advertisements,
//! i.e. the GAP Central role role defined in the Bluetooth Core
//! Specification (5.4, Volume 3 Part C Section 2.2.2)
//!
//! These traits should be implemented outside this crate, conforming to the
//! types and structs here when necessary.

use bt_common::core::Phy;
use futures::{Future, Stream};

use crate::central::AdvertisingDatum;
use thiserror::Error;

use bt_common::PeerId;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("Periodic Advertising Sync failed to establish")]
    SyncEstablishFailed,
    #[error("Periodic Advertising Sync lost")]
    SyncLost,
    #[error("I/O error")]
    Io,
}

/// A trait for managing a periodic advertising.
pub trait PeriodicAdvertising {
    type SyncFut: Future<Output = crate::Result<Self::SyncStream>>;
    type SyncStream: Stream<Item = crate::Result<SyncReport>>;

    /// Request to sync to periodic advertising resports.
    /// On success, returns the SyncStream which can be used to receive
    /// SyncReports.
    fn sync_to_advertising_reports(
        &self,
        peer_id: PeerId,
        advertising_sid: u8,
        config: SyncConfiguration,
    ) -> Self::SyncFut;

    // TODO(b/340885203): Add a method to sync to subevents.
}

#[derive(Debug, Clone)]
pub struct SyncConfiguration {
    /// Filter out duplicate advertising reports.
    /// Optional.
    /// Default: true
    pub filter_duplicates: bool,
}

#[derive(Debug, Clone)]
pub struct PeriodicAdvertisingReport {
    pub rssi: i8,
    pub data: Vec<AdvertisingDatum>,
    /// The event counter of the event that the advertising packet was received
    /// in.
    pub event_counter: Option<u16>,
    /// The subevent number of the report. Only present if the packet was
    /// received in a subevent.
    pub subevent: Option<u8>,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct BroadcastIsochronousGroupInfo {
    /// The number of Broadcast Isochronous Streams in this group.
    /// The specification calls this "num_bis".
    pub streams_count: u8,
    /// The time interval of the periodic SDUs.
    pub sdu_interval: i64,
    /// The maximum size of an SDU.
    pub max_sdu_size: u16,
    /// The PHY used for transmission of data.
    pub phy: Phy,
    /// Indicates whether the BIG is encrypted.
    pub encryption: bool,
}

#[derive(Debug, Clone)]
pub struct BroadcastIsochronousGroupInfoReport {
    pub info: BroadcastIsochronousGroupInfo,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub enum SyncReport {
    PeriodicAdvertisingReport(PeriodicAdvertisingReport),
    BroadcastIsochronousGroupInfoReport(BroadcastIsochronousGroupInfoReport),
}
