// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::proxies::Proxies;
use anyhow::anyhow;
use async_utils::hanging_get::client::HangingGetStream;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_bluetooth::PeerId;
use fidl_fuchsia_bluetooth_affordances::ScannedPeer;

use fidl_fuchsia_bluetooth_le::{
    AdvertisedPeripheralMarker, AdvertisedPeripheralRequest, AdvertisingParameters,
    ConnectionMarker, ConnectionOptions,
};
use fuchsia_async::{Task, TimeoutExt};
use futures::{FutureExt, StreamExt, select};

fn lookup_peer_addr(
    lookup_proxy: &fidl_fuchsia_bluetooth_sys::AddressLookupProxy,
    peer: fidl_fuchsia_bluetooth_le::Peer,
) -> impl std::future::Future<
    Output = (
        fidl_fuchsia_bluetooth_le::Peer,
        Result<
            Result<fidl_fuchsia_bluetooth::Address, fidl_fuchsia_bluetooth_sys::LookupError>,
            fidl::Error,
        >,
    ),
> {
    let lookup_proxy = lookup_proxy.clone();
    let id = peer.id.unwrap();
    async move {
        let res = lookup_proxy
            .lookup(&fidl_fuchsia_bluetooth_sys::AddressLookupLookupRequest {
                peer_id: Some(id),
                ..Default::default()
            })
            .await;
        (peer, res)
    }
}

async fn scan_result_watcher(
    scan_client: fidl_fuchsia_bluetooth_le::ScanResultWatcherProxy,
    lookup_proxy: fidl_fuchsia_bluetooth_sys::AddressLookupProxy,
    sender: futures::channel::mpsc::UnboundedSender<Vec<ScannedPeer>>,
    _discovery_token: fidl_fuchsia_bluetooth_sys::ProcedureTokenProxy,
) {
    let mut scan_result_watcher_stream = HangingGetStream::new(
        scan_client,
        fidl_fuchsia_bluetooth_le::ScanResultWatcherProxy::watch,
    );

    while let Some(scan_result) = scan_result_watcher_stream.next().await {
        let Ok(updated) = scan_result else {
            let err = scan_result.unwrap_err();
            eprintln!("LE scan encountered error: {err}");
            return;
        };

        let lookup_futures = updated.into_iter().map(|peer| lookup_peer_addr(&lookup_proxy, peer));
        let resolved = futures::future::join_all(lookup_futures).await;

        let send_list: Vec<ScannedPeer> = resolved
            .into_iter()
            .filter_map(|(peer, res)| match res {
                Ok(Ok(addr)) => Some(ScannedPeer {
                    peer: Some(peer),
                    address: Some(addr),
                    ..Default::default()
                }),
                Ok(Err(e)) => {
                    eprintln!("Address lookup failed for {:?}: {:?}", peer.id, e);
                    None
                }
                Err(e) => {
                    eprintln!("FIDL error during address lookup: {:?}", e);
                    None
                }
            })
            .collect();

        if send_list.is_empty() {
            continue;
        }
        if let Err(e) = sender.unbounded_send(send_list) {
            eprintln!("Error sending results to channel: {:?}", e);
            return;
        }
    }
}

pub(crate) async fn start_le_scan(
    proxies: &mut Proxies,
    sender: futures::channel::mpsc::UnboundedSender<Vec<ScannedPeer>>,
) -> Result<(), anyhow::Error> {
    // Enable Discovery as well to find dual mode peers.
    let (discovery_token, discovery_session_server) = fidl::endpoints::create_proxy();
    if let Err(err) = proxies.access_proxy.start_discovery(discovery_session_server).await? {
        return Err(anyhow!("fuchsia.bluetooth.sys.Access/StartDiscovery error: {err:?}"));
    }

    let (scan_client, scan_server) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_bluetooth_le::ScanResultWatcherMarker>();
    let options = fidl_fuchsia_bluetooth_le::ScanOptions {
        // Empty filter matches all LE peripherals and broadcasters.
        filters: Some(vec![fidl_fuchsia_bluetooth_le::Filter { ..Default::default() }]),
        ..Default::default()
    };
    let scan_fut = proxies.central_proxy.scan(&options, scan_server);
    let watcher_fut = scan_result_watcher(
        scan_client,
        proxies.address_lookup_proxy.clone(),
        sender,
        discovery_token,
    );

    *proxies.le_scan_task.lock() = Some(Task::spawn(async move {
        futures::pin_mut!(scan_fut, watcher_fut);
        let _ = futures::future::select(scan_fut, watcher_fut).await;
    }));

    Ok(())
}

pub(crate) fn stop_scan(proxies: &Proxies) -> bool {
    proxies.le_scan_task.lock().take().is_some()
}

pub(crate) async fn connect_le(
    proxies: &mut Proxies,
    peer_id: &PeerId,
) -> Result<(), anyhow::Error> {
    let (le_client, le_server) = fidl::endpoints::create_proxy::<ConnectionMarker>();
    proxies.central_proxy.connect(peer_id, &ConnectionOptions::default(), le_server)?;
    let (client_proxy, client_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_bluetooth_gatt2::ClientMarker>();
    le_client.request_gatt_client(client_server_end)?;
    *proxies.central_connection.lock() = Some(le_client);
    proxies.gatt_client = Some(client_proxy);
    Ok(())
}

pub(crate) async fn advertise_peripheral(
    proxies: &Proxies,
    parameters: AdvertisingParameters,
    timeout: std::time::Duration,
) -> Result<Option<(PeerId, ClientEnd<ConnectionMarker>)>, anyhow::Error> {
    let (client, mut request_stream) =
        fidl::endpoints::create_request_stream::<AdvertisedPeripheralMarker>();

    select! {
        result = proxies.peripheral_proxy.advertise(&parameters, client) => {
            return Err(anyhow!("LE advertisement finished with result: {result:?}"));
        }
        request = request_stream.next().on_timeout(timeout, || None).fuse() => {
            match request {
                Some(Ok(AdvertisedPeripheralRequest::OnConnected {
                    peer,
                    connection,
                    responder,
                })) => {
                    let _ = responder.send();
                    return Ok(Some((peer.id.unwrap(), connection)));
                }
                Some(Err(e)) => {
                    return Err(anyhow!("Error in AdvertisedPeripheral stream: {e:?}"));
                }
                None => {
                    println!("Peripheral advertisement ended without connection");
                    return Ok(None);
                }
            }
        }
    }
}
