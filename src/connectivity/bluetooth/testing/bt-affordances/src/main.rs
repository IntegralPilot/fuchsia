// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl::endpoints::Proxy;
use fidl_fuchsia_bluetooth_affordances::{
    CentralControllerRequest, CentralControllerRequestStream,
    GattClientControllerDiscoverServicesResponse, GattClientControllerRequest,
    GattClientControllerRequestStream, HostControllerRequest, HostControllerRequestStream,
    HostControllerSetConnectabilityRequest, HostControllerSetDiscoverabilityRequest,
    PeerControllerGetPeerIdRequest, PeerControllerGetPeerIdResponse, PeerControllerRequest,
    PeerControllerRequestStream, PeerControllerSetDiscoveryRequest, PeerSelector,
    PeripheralControllerAdvertiseRequest, PeripheralControllerAdvertiseResponse,
    PeripheralControllerRequest, PeripheralControllerRequestStream,
    ScanResultListenerOnPeersDiscoveredRequest,
};
use fuchsia_bt_test_affordances::WorkThread;
use fuchsia_component::server::ServiceFs;
use futures::{FutureExt, StreamExt, TryStreamExt};
use log::error;
use std::sync::Arc;

pub enum Services {
    Peer(PeerControllerRequestStream),
    Host(HostControllerRequestStream),
    Peripheral(PeripheralControllerRequestStream),
    Central(CentralControllerRequestStream),
    GattClient(GattClientControllerRequestStream),
}

macro_rules! selector_to_peer_id {
    ($method:expr, $selector:expr, $responder:expr) => {
        match $selector {
            PeerSelector { id: Some(id), .. } => id,
            _ => {
                $responder
                    .send(Err(fidl_fuchsia_bluetooth_affordances::Error::MissingParameters))?;
                return Ok(());
            }
        }
    };
}

async fn handle_single_peer_request(
    worker: Arc<WorkThread>,
    request: PeerControllerRequest,
) -> Result<(), Error> {
    match request {
        PeerControllerRequest::GetKnownPeers { responder } => {
            match worker.get_known_peers().await {
                Ok(peers) => {
                    responder.send(Ok(
                        &fidl_fuchsia_bluetooth_affordances::PeerControllerGetKnownPeersResponse {
                            peers: Some(peers),
                            ..Default::default()
                        },
                    ))?;
                }
                Err(err) => {
                    error!("GetKnownPeers encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        PeerControllerRequest::GetPeerId { payload, responder } => {
            let PeerControllerGetPeerIdRequest { address: Some(address), .. } = payload else {
                responder
                    .send(Err(fidl_fuchsia_bluetooth_affordances::Error::MissingParameters))?;
                return Ok(());
            };
            match worker.get_peer_id(address.bytes).await {
                Ok(Some(peer_id)) => {
                    responder.send(Ok(&PeerControllerGetPeerIdResponse {
                        id: Some(peer_id),
                        ..Default::default()
                    }))?;
                }
                Ok(None) => {
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Timeout))?;
                }
                Err(err) => {
                    error!("GetPeerId encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        PeerControllerRequest::SetDiscovery { payload, responder } => {
            let PeerControllerSetDiscoveryRequest { discovery: Some(discovery), .. } = payload
            else {
                responder
                    .send(Err(fidl_fuchsia_bluetooth_affordances::Error::MissingParameters))?;
                return Ok(());
            };
            match worker.set_discovery(discovery).await {
                Ok(_) => {
                    responder.send(Ok(()))?;
                }
                Err(err) => {
                    error!("SetDiscovery encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        PeerControllerRequest::_UnknownMethod { ordinal, .. } => {
            error!("PeerControllerRequest: unknown method received with ordinal {ordinal}");
        }
    }
    Ok(())
}

async fn handle_peer_requests(
    stream: PeerControllerRequestStream,
    worker: Arc<WorkThread>,
) -> Result<(), Error> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| handle_single_peer_request(worker.clone(), request))
        .await
}

async fn handle_single_host_request(
    worker: Arc<WorkThread>,
    request: HostControllerRequest,
) -> Result<(), Error> {
    match request {
        HostControllerRequest::GetHosts { responder } => match worker.get_hosts().await {
            Ok(hosts) => {
                responder.send(Ok(
                    &fidl_fuchsia_bluetooth_affordances::HostControllerGetHostsResponse {
                        hosts: Some(hosts),
                        ..Default::default()
                    },
                ))?;
            }
            Err(err) => {
                error!("GetHosts encountered error: {err}");
                responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
            }
        },
        HostControllerRequest::SetDiscoverability { payload, responder } => {
            let HostControllerSetDiscoverabilityRequest {
                discoverable: Some(discoverable), ..
            } = payload
            else {
                responder
                    .send(Err(fidl_fuchsia_bluetooth_affordances::Error::MissingParameters))?;
                return Ok(());
            };
            match worker.set_discoverability(discoverable).await {
                Ok(_) => {
                    responder.send(Ok(()))?;
                }
                Err(err) => {
                    error!("SetDiscoverability encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        HostControllerRequest::SetConnectability { payload, responder } => {
            let HostControllerSetConnectabilityRequest { connectable: Some(connectable), .. } =
                payload
            else {
                responder
                    .send(Err(fidl_fuchsia_bluetooth_affordances::Error::MissingParameters))?;
                return Ok(());
            };
            match worker.set_connectability(connectable).await {
                Ok(_) => {
                    responder.send(Ok(()))?;
                }
                Err(err) => {
                    error!("SetConnectability encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        HostControllerRequest::_UnknownMethod { ordinal, .. } => {
            error!("HostControllerRequest: unknown method received with ordinal {ordinal}");
        }
    }
    Ok(())
}

async fn handle_host_requests(
    stream: HostControllerRequestStream,
    worker: Arc<WorkThread>,
) -> Result<(), Error> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| handle_single_host_request(worker.clone(), request))
        .await
}

async fn handle_single_peripheral_request(
    worker: Arc<WorkThread>,
    request: PeripheralControllerRequest,
) -> Result<(), Error> {
    match request {
        PeripheralControllerRequest::Advertise { payload, responder } => {
            let PeripheralControllerAdvertiseRequest {
                parameters: Some(parameters),
                timeout: Some(timeout),
                ..
            } = payload
            else {
                responder
                    .send(Err(fidl_fuchsia_bluetooth_affordances::Error::MissingParameters))?;
                return Ok(());
            };
            let timeout = std::time::Duration::from_secs(timeout);
            match worker.advertise_peripheral(parameters, timeout).await {
                Ok(Some(peer_id)) => {
                    responder.send(Ok(&PeripheralControllerAdvertiseResponse {
                        peer_id: Some(peer_id),
                        ..Default::default()
                    }))?;
                }
                Ok(None) => {
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Timeout))?;
                }
                Err(err) => {
                    error!("Advertise encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }

        PeripheralControllerRequest::_UnknownMethod { ordinal, .. } => {
            error!("PeripheralControllerRequest: unknown method received with ordinal {ordinal}");
        }
    }
    Ok(())
}

async fn handle_peripheral_requests(
    stream: PeripheralControllerRequestStream,
    worker: Arc<WorkThread>,
) -> Result<(), Error> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| handle_single_peripheral_request(worker.clone(), request))
        .await
}

async fn handle_single_central_request(
    worker: Arc<WorkThread>,
    request: CentralControllerRequest,
    scan_task: &mut Option<fuchsia_async::Task<()>>,
) -> Result<(), Error> {
    match request {
        CentralControllerRequest::StartScan { payload, responder } => {
            let fidl_fuchsia_bluetooth_affordances::CentralControllerStartScanRequest {
                listener: Some(listener),
                ..
            } = payload
            else {
                responder
                    .send(Err(fidl_fuchsia_bluetooth_affordances::Error::MissingParameters))?;
                return Ok(());
            };

            let (tx, rx) = futures::channel::mpsc::unbounded::<
                Vec<fidl_fuchsia_bluetooth_affordances::ScannedPeer>,
            >();

            match worker.start_le_scan(tx).await {
                Ok(_) => {
                    responder.send(Ok(()))?;

                    let listener_proxy = listener.into_proxy();
                    let worker = worker.clone();
                    *scan_task = Some(fuchsia_async::Task::spawn(async move {
                        let mut rx = rx.fuse();
                        let on_closed = listener_proxy.on_closed().fuse();
                        futures::pin_mut!(on_closed);
                        loop {
                            futures::select! {
                                peers = rx.next() => {
                                    let Some(peers) = peers else {
                                        break;
                                    };
                                    let request = ScanResultListenerOnPeersDiscoveredRequest {
                                        peers: Some(peers),
                                        ..Default::default()
                                    };
                                    if let Err(e) =
                                        listener_proxy.on_peers_discovered(&request).await
                                    {
                                        eprintln!("Error sending results to listener: {:?}", e);
                                        break;
                                    }
                                },
                                _ = on_closed => {
                                    println!("Client dropped ScanResultListener");
                                    break;
                                }
                            }
                        }
                        let _ = worker.stop_le_scan().await;
                    }));
                }
                Err(err) => {
                    error!("StartScan encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        CentralControllerRequest::ConnectPeripheral { payload, responder } => {
            let id = selector_to_peer_id!("ConnectPeripheral", payload, responder);
            match worker.connect_le(id).await {
                Ok(_) => {
                    responder.send(Ok(()))?;
                }
                Err(err) => {
                    error!("ConnectPeripheral encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        CentralControllerRequest::_UnknownMethod { ordinal, .. } => {
            error!("CentralControllerRequest: unknown method received with ordinal {ordinal}");
        }
    }
    Ok(())
}

async fn handle_central_requests(
    mut stream: CentralControllerRequestStream,
    worker: Arc<WorkThread>,
) -> Result<(), Error> {
    let mut scan_task: Option<fuchsia_async::Task<()>> = None;

    while let Some(request) = stream.next().await {
        let request = request.context("failed request")?;
        handle_single_central_request(worker.clone(), request, &mut scan_task).await?;
    }

    if let Some(task) = scan_task.take() {
        let _ = worker.stop_le_scan().await;
        task.await;
    }
    Ok(())
}

async fn handle_single_gatt_client_request(
    worker: Arc<WorkThread>,
    request: GattClientControllerRequest,
) -> Result<(), Error> {
    match request {
        GattClientControllerRequest::DiscoverServices { responder } => {
            match worker.discover_services().await {
                Ok(services) => {
                    responder.send(Ok(&GattClientControllerDiscoverServicesResponse {
                        services: Some(services),
                        ..Default::default()
                    }))?;
                }
                Err(err) => {
                    error!("DiscoverServices encountered error: {err}");
                    responder.send(Err(fidl_fuchsia_bluetooth_affordances::Error::Internal))?;
                }
            }
        }
        GattClientControllerRequest::_UnknownMethod { ordinal, .. } => {
            error!("GattClientControllerRequest: unknown method received with ordinal {ordinal}");
        }
    }
    Ok(())
}

async fn handle_gatt_client_requests(
    stream: GattClientControllerRequestStream,
    worker: Arc<WorkThread>,
) -> Result<(), Error> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| handle_single_gatt_client_request(worker.clone(), request))
        .await
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    let _ = fs.dir("svc").add_fidl_service(Services::Peer);
    let _ = fs.dir("svc").add_fidl_service(Services::Host);
    let _ = fs.dir("svc").add_fidl_service(Services::Peripheral);
    let _ = fs.dir("svc").add_fidl_service(Services::Central);
    let _ = fs.dir("svc").add_fidl_service(Services::GattClient);
    let _ = fs.take_and_serve_directory_handle()?;

    let worker = Arc::new(WorkThread::spawn());

    fs.for_each_concurrent(None, move |request| {
        let worker = worker.clone();
        async move {
            match request {
                Services::Peer(stream) => {
                    handle_peer_requests(stream, worker).await.unwrap_or_else(|e| error!("{e:?}"))
                }
                Services::Host(stream) => {
                    handle_host_requests(stream, worker).await.unwrap_or_else(|e| error!("{e:?}"))
                }
                Services::Peripheral(stream) => handle_peripheral_requests(stream, worker)
                    .await
                    .unwrap_or_else(|e| error!("{e:?}")),
                Services::Central(stream) => handle_central_requests(stream, worker)
                    .await
                    .unwrap_or_else(|e| error!("{e:?}")),
                Services::GattClient(stream) => handle_gatt_client_requests(stream, worker)
                    .await
                    .unwrap_or_else(|e| error!("{e:?}")),
            }
        }
    })
    .await;

    Ok(())
}
