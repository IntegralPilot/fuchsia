// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements the Bluetooth Battery Service (BAS) client role.
//!
//! Use the toplevel `BatteryMonitor` to construct a new battery monitoring
//! client. The client will scan for compatible peers that support the Battery
//! Service. When a compatible peer is found, use `BatteryMonitor::connect` to
//! establish a GATT connection to the peer's service. The battery level and
//! other characteristics are available in the returned `BatteryMonitorClient`.
//!
//! For example:
//!
//! // Set up a GATT Central and construct the battery monitor.
//! let central = ...;
//! let monitor = BatteryMonitor::new(central);
//!
//! // Start scanning for compatible peers and find the next available one.
//! let mut scan_results = monitor.start()?;
//! let compatible_peer_id = scan_results.next().await?;
//!
//! // Connect to the peer's Battery Service.
//! let connected_service = monitor.connect(compatible_peer_id).await?;
//!
//! // Process battery events (notifications/indications) from the peer.
//! let battery_events = connected_service.take_event_stream()?;
//! while let Some(battery_event) = battery_events.next().await? {
//!     // Do something with `battery_event`
//! }

use bt_common::PeerId;
use bt_gatt::central::{Filter, ScanFilter};
use bt_gatt::client::PeerServiceHandle;
use bt_gatt::{Central, Client, GattTypes};

use crate::error::Error;
use crate::types::BATTERY_SERVICE_UUID;

mod client;
pub use client::{BatteryMonitorClient, BatteryMonitorEventStream};

/// Monitors the battery properties on a remote peer's Battery Service (BAS).
///
/// Use `BatteryMonitor::start` to scan for compatible peers that provide the
/// battery service. Once a suitable peer has been found, use
/// `BatteryMonitor::connect` to initiate an outbound connection to the peer's
/// Battery GATT service. Use the returned `BatteryMonitorClient` to
/// interact with the GATT service (e.g. read battery level, receive battery
/// level updates, etc.).
pub struct BatteryMonitor<T: GattTypes> {
    central: T::Central,
    scan_stream: Option<T::ScanResultStream>,
}

impl<T: GattTypes> BatteryMonitor<T> {
    pub fn new(central: T::Central) -> Self {
        let scan_stream = central.scan(&Self::scan_filter());
        Self { central, scan_stream: Some(scan_stream) }
    }

    fn scan_filter() -> Vec<ScanFilter> {
        vec![ScanFilter {
            filters: vec![Filter::ServiceUuid(BATTERY_SERVICE_UUID), Filter::IsConnectable],
        }]
    }

    /// Start scanning for compatible Battery peers.
    /// Returns a stream of scan results on success, Error if the scan couldn't
    /// complete for any reason.
    /// Can only be called once, returns `Error::ScanAlreadyStarted` on
    /// subsequent attempts.
    pub fn start(&mut self) -> Result<T::ScanResultStream, Error>
    where
        <T as bt_gatt::GattTypes>::ScanResultStream: std::marker::Send,
    {
        let Some(scan_stream) = self.scan_stream.take() else {
            return Err(Error::ScanAlreadyStarted);
        };

        Ok(scan_stream)
    }

    /// Attempts to connect to the remote peer's Battery service.
    /// Returns a battery monitor which can be used to interact with the peer's
    /// battery service on success, Error if the connection couldn't be made
    /// or if the peer's Battery service is invalid.
    pub async fn connect(&mut self, id: PeerId) -> Result<BatteryMonitorClient<T>, Error>
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        let client = self.central.connect(id).await.map_err(Error::GattLibrary)?;
        let peer_service_handles =
            client.find_service(BATTERY_SERVICE_UUID).await.map_err(Error::GattLibrary)?;

        for handle in peer_service_handles {
            if handle.uuid() != BATTERY_SERVICE_UUID || !handle.is_primary() {
                return Err(Error::ServiceNotFound);
            }
            let service = handle.connect().await.map_err(Error::GattLibrary)?;
            let monitor = BatteryMonitorClient::<T>::create(client, service).await?;
            // TODO(b/335246946): This short circuits after the first valid service is
            // found. Expand this to read all of the battery services to provide
            // an aggregated view of the peer.
            return Ok(monitor);
        }
        Err(Error::ServiceNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bt_common::Uuid;
    use bt_gatt::test_utils::{FakeCentral, FakeClient, FakeTypes};
    use futures::{pin_mut, FutureExt};
    use std::task::Poll;

    use crate::monitor::client::tests::fake_battery_service;

    #[test]
    fn battery_monitor_start_scan() {
        let fake_central = FakeCentral::new();
        let mut monitor = BatteryMonitor::<FakeTypes>::new(fake_central);
        let _monitor_scan_stream = monitor.start().expect("can start scanning");

        // The scan stream can only be acquired once.
        assert!(monitor.start().is_err());
    }

    #[test]
    fn battery_monitor_connect_success() {
        // Instantiate a fake client with a battery service.
        let id = PeerId(1);
        let mut fake_central = FakeCentral::new();
        let mut fake_client = FakeClient::new();
        fake_central.add_client(id, fake_client.clone());
        let fake_battery_service = fake_battery_service(50);
        fake_client.add_service(BATTERY_SERVICE_UUID, true, fake_battery_service);

        let mut monitor = BatteryMonitor::<FakeTypes>::new(fake_central);

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let connect_fut = monitor.connect(id);
        pin_mut!(connect_fut);
        let polled = connect_fut.poll_unpin(&mut noop_cx);
        let Poll::Ready(Ok(_monitor_service)) = polled else {
            panic!("Expected connect success");
        };
    }

    #[test]
    fn connect_no_services_is_error() {
        let id = PeerId(1);
        let mut fake_central = FakeCentral::new();
        let fake_client = FakeClient::new();
        fake_central.add_client(id, fake_client.clone());
        // No battery service.

        let mut monitor = BatteryMonitor::<FakeTypes>::new(fake_central);

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let connect_fut = monitor.connect(id);
        pin_mut!(connect_fut);
        let polled = connect_fut.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(Error::ServiceNotFound)) = polled else {
            panic!("Expected connect failure");
        };
    }

    #[test]
    fn connect_invalid_battery_service_is_error() {
        let id = PeerId(1);
        let mut fake_central = FakeCentral::new();
        let mut fake_client = FakeClient::new();
        fake_central.add_client(id, fake_client.clone());
        let fake_battery_service = fake_battery_service(50);
        let random_uuid = Uuid::from_u16(0x1234);
        fake_client.add_service(random_uuid, true, fake_battery_service);

        let mut monitor = BatteryMonitor::<FakeTypes>::new(fake_central);

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let connect_fut = monitor.connect(id);
        pin_mut!(connect_fut);
        let polled = connect_fut.poll_unpin(&mut noop_cx);
        let Poll::Ready(Err(Error::ServiceNotFound)) = polled else {
            panic!("Expected connect failure");
        };
    }
}
