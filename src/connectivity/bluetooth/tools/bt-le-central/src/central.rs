// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fuchsia_sync::Mutex;
use futures::future::FutureExt;
use futures::{StreamExt, pin_mut};
use std::sync::Arc;

use bt_common::{PeerId, Uuid};
use bt_gatt::central::Central;

use crate::gatt::repl::start_gatt_loop;

pub type CentralStatePtr<T> = Arc<Mutex<CentralState<T>>>;

pub struct CentralState<T: bt_gatt::GattTypes> {
    // If `Some(n)`, stop scanning and close the delegate handle after n more scan results.
    pub remaining_scan_results: Option<u64>,

    // If true, attempt to connect to the first scan result.
    pub connect: bool,

    // The central that we use to perform requests.
    svc: Arc<T::Central>,
}

impl<T: bt_gatt::GattTypes> CentralState<T> {
    pub fn new(central: T::Central) -> CentralStatePtr<T> {
        Arc::new(Mutex::new(CentralState {
            remaining_scan_results: None,
            connect: false,
            svc: Arc::new(central),
        }))
    }

    pub fn get_central(&self) -> Arc<T::Central> {
        self.svc.clone()
    }

    /// If the remaining_scan_results is specified, decrement until it reaches 0.
    /// Scanning should continue if the user is not attempted to connect to a device
    /// and there are remaining scans left to run.
    ///
    /// return: true if scanning should continue
    ///         false if scanning should stop
    pub fn decrement_scan_count(&mut self) -> bool {
        self.remaining_scan_results = self.remaining_scan_results.map(
            // decrement until n is 0
            |n| if n > 0 { n - 1 } else { n },
        );
        // scanning should continue if connection will not be attempted and
        // there are remaining scan results
        !(self.connect || self.remaining_scan_results.iter().any(|&n| n == 0))
    }
}

/// Watch for scan results from the given `result_watcher`. If `state.connect`, then try to connect
/// to the first connectable peer and stop scanning. Returns when `state.remaining_scan_results`
/// results have been received, or after the connected peer disconnects (whichever happens first).
pub async fn watch_scan_results<T: bt_gatt::GattTypes>(
    state: CentralStatePtr<T>,
    result_stream: T::ScanResultStream,
) -> Result<(), Error>
where
    <T as bt_gatt::GattTypes>::Client: Clone,
{
    eprintln!("Starting Scan");
    let mut pinned_stream = Box::pin(result_stream);
    let connect_id = loop {
        let next = pinned_stream.next().await;

        let Some(peer) = next else {
            eprintln!("No scan results, scan finished.");
            return Ok(());
        };

        let peer = peer.map_err(|e| format_err!("Scan error: {e}"))?;

        eprintln!(" {:?}", peer);

        if state.lock().decrement_scan_count() {
            continue;
        }

        if state.lock().connect && peer.connectable {
            break peer.id;
            // connect_peripheral will log errors, so the result can be ignored.
            // TODO(https://fxbug.dev/42060216): Use Central.Connect instead of deprecated Central.ConnectPeripheral.
        }

        return Ok(());
    };

    drop(pinned_stream);
    let central = state.lock().get_central();
    let _ = connect::<T>(central, connect_id, None).await;
    Ok(())
}

/// Attempts to connect to the peripheral with the given `peer_id` and begins the GATT REPL if this succeeds.
/// If `service_uuid` is specified, limit GATT service discovery to services with the indicated UUID.
pub async fn connect<T: bt_gatt::GattTypes>(
    central: Arc<T::Central>,
    peer_id: PeerId,
    service_uuid: Option<Uuid>,
) -> Result<(), Error>
where
    <T as bt_gatt::GattTypes>::Client: Clone,
{
    let client = central.connect(peer_id).await?;

    println!("Connecting to {}..", peer_id);

    let gatt_loop_fut = start_gatt_loop::<T>(client, service_uuid).fuse();
    pin_mut!(gatt_loop_fut);
    gatt_loop_fut.await
}
