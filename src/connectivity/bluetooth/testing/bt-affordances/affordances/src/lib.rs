// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_bluetooth::{PeerId, Uuid};
use fidl_fuchsia_bluetooth_gatt2::{Characteristic, ServiceHandle, ServiceInfo};
use fidl_fuchsia_bluetooth_le::{AdvertisingParameters, ConnectionMarker};
use fidl_fuchsia_bluetooth_sys::{HostInfo, Peer};
use fuchsia_async::LocalExecutor;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use std::sync::Arc;
use std::thread;

mod gatt;
mod le;
mod proxies;
mod sys;

use proxies::Proxies;

// TODO(https://fxbug.dev/414848887): Return fidl_fuchsia_bluetooth_affordances::Error instead of
// anyhow::Error.
enum Request {
    GetHosts(oneshot::Sender<Result<Vec<HostInfo>, anyhow::Error>>),
    GetKnownPeers(oneshot::Sender<Result<Vec<Peer>, anyhow::Error>>),
    GetPeerId([u8; 6], oneshot::Sender<Result<Option<PeerId>, anyhow::Error>>),
    SetDiscovery(bool, oneshot::Sender<Result<(), anyhow::Error>>),
    SetDiscoverability(bool, oneshot::Sender<Result<(), anyhow::Error>>),
    SetConnectability(bool, oneshot::Sender<Result<(), anyhow::Error>>),
    StartLeScan(
        futures::channel::mpsc::UnboundedSender<
            Vec<fidl_fuchsia_bluetooth_affordances::ScannedPeer>,
        >,
        oneshot::Sender<Result<(), anyhow::Error>>,
    ),
    StopLeScan(oneshot::Sender<Result<(), anyhow::Error>>),
    ConnectLe(PeerId, oneshot::Sender<Result<(), anyhow::Error>>),
    AdvertisePeripheral(
        Box<AdvertisingParameters>,
        std::time::Duration,
        oneshot::Sender<Result<Option<PeerId>, anyhow::Error>>,
    ),
    PublishService(
        Uuid,
        ServiceHandle,
        Vec<Characteristic>,
        oneshot::Sender<Result<(), anyhow::Error>>,
    ),
    DiscoverServices(oneshot::Sender<Result<Vec<ServiceInfo>, anyhow::Error>>),
    ReadCharacteristic(
        ServiceHandle,
        fidl_fuchsia_bluetooth_gatt2::Handle,
        oneshot::Sender<Result<fidl_fuchsia_bluetooth_gatt2::ReadValue, anyhow::Error>>,
    ),
    RegisterCharacteristicNotifier(
        ServiceHandle,
        fidl_fuchsia_bluetooth_gatt2::Handle,
        oneshot::Sender<Result<(), anyhow::Error>>,
    ),
    Stop,
}

pub struct WorkThread {
    thread_handle: Mutex<Option<thread::JoinHandle<Result<(), anyhow::Error>>>>,
    sender: mpsc::UnboundedSender<Request>,
}

impl WorkThread {
    pub fn spawn() -> Self {
        let (sender, receiver) = mpsc::unbounded::<Request>();

        let thread_handle = thread::spawn(move || {
            LocalExecutor::default().run_singlethreaded(Self::handle_requests(receiver))?;
            Ok(())
        });

        Self { thread_handle: Mutex::new(Some(thread_handle)), sender }
    }

    async fn handle_requests(
        mut receiver: mpsc::UnboundedReceiver<Request>,
    ) -> Result<(), anyhow::Error> {
        let mut proxies = Proxies::connect()?;
        let mut host_cache: Vec<HostInfo> = Vec::new();
        // TODO(https://fxbug.dev/396500079): Consider HashMap<PeerId, Peer> instead.
        let peer_cache: Arc<Mutex<Vec<Peer>>> = Arc::new(Mutex::new(Vec::new()));
        let mut _peripheral_connection: ClientEnd<ConnectionMarker>;

        while let Some(request) = receiver.next().await {
            match request {
                Request::GetHosts(result_sender) => {
                    if let Err(err) = sys::refresh_host_cache(&mut proxies, &mut host_cache).await {
                        result_sender
                            .send(Err(anyhow!("refresh_host_cache() error: {err}")))
                            .unwrap();
                        continue;
                    }
                    result_sender.send(Ok(host_cache.clone())).unwrap();
                }
                Request::GetKnownPeers(result_sender) => {
                    if let Err(err) = sys::refresh_peer_cache(
                        &mut proxies,
                        std::time::Duration::from_millis(10),
                        peer_cache.clone(),
                    )
                    .await
                    {
                        result_sender
                            .send(Err(anyhow!("refresh_peer_cache() error: {err}")))
                            .unwrap();
                        continue;
                    }
                    result_sender.send(Ok(peer_cache.lock().clone())).unwrap();
                }
                Request::GetPeerId(address, result_sender) => {
                    let result = sys::get_peer(
                        &mut proxies,
                        address,
                        std::time::Duration::from_secs(2),
                        peer_cache.clone(),
                    )
                    .await
                    .map(|opt_peer| opt_peer.map(|peer| peer.id.unwrap()));
                    result_sender.send(result).unwrap();
                }
                Request::SetDiscovery(discovery, result_sender) => {
                    result_sender.send(sys::set_discovery(&mut proxies, discovery).await).unwrap();
                }
                Request::SetDiscoverability(discoverable, result_sender) => {
                    result_sender
                        .send(sys::set_discoverability(&mut proxies, discoverable).await)
                        .unwrap();
                }
                Request::SetConnectability(connectable, result_sender) => {
                    result_sender
                        .send(sys::set_connectability(&proxies, connectable).await)
                        .unwrap();
                }
                Request::StartLeScan(sender, result_sender) => {
                    result_sender.send(le::start_le_scan(&mut proxies, sender).await).unwrap();
                }
                Request::StopLeScan(result_sender) => {
                    let stopped = le::stop_scan(&proxies);
                    if stopped {
                        result_sender.send(Ok(())).unwrap();
                    } else {
                        result_sender.send(Err(anyhow!("No scan ongoing"))).unwrap();
                    }
                }
                Request::ConnectLe(peer_id, result_sender) => {
                    result_sender.send(le::connect_le(&mut proxies, &peer_id).await).unwrap();
                }
                Request::AdvertisePeripheral(parameters, timeout, result_sender) => {
                    match le::advertise_peripheral(&proxies, *parameters, timeout).await {
                        Ok(Some((peer_id, connection))) => {
                            _peripheral_connection = connection;
                            result_sender.send(Ok(Some(peer_id))).unwrap();
                        }
                        result => {
                            result_sender.send(result.map(|_| None)).unwrap();
                        }
                    }
                }
                Request::PublishService(uuid, service_handle, characteristics, result_sender) => {
                    match gatt::publish_service(&proxies, uuid, service_handle, characteristics)
                        .await
                    {
                        Ok(mut local_service_request_stream) => {
                            fuchsia_async::Task::spawn(async move {
                                while let Some(Ok(request)) =
                                    local_service_request_stream.next().await
                                {
                                    // Just log the request for now.
                                    println!("Received LocalService request: {:?}", request);
                                }
                            })
                            .detach();
                            result_sender.send(Ok(())).unwrap();
                        }
                        Err(err) => {
                            result_sender.send(Err(err)).unwrap();
                        }
                    }
                }
                Request::DiscoverServices(result_sender) => {
                    result_sender.send(gatt::discover_services(&mut proxies).await).unwrap();
                }
                Request::ReadCharacteristic(
                    service_handle,
                    characteristic_handle,
                    result_sender,
                ) => {
                    result_sender
                        .send(
                            gatt::read_characteristic(
                                &proxies,
                                service_handle,
                                characteristic_handle,
                            )
                            .await,
                        )
                        .unwrap();
                }
                Request::RegisterCharacteristicNotifier(
                    service_handle,
                    characteristic_handle,
                    result_sender,
                ) => {
                    result_sender
                        .send(
                            gatt::register_characteristic_notifier(
                                &proxies,
                                service_handle,
                                characteristic_handle,
                            )
                            .await,
                        )
                        .unwrap();
                }
                Request::Stop => break,
            }
        }

        Ok(())
    }

    pub fn join(&self) -> Result<(), anyhow::Error> {
        self.sender.clone().unbounded_send(Request::Stop).unwrap();
        if let Err(err) =
            self.thread_handle.lock().take().unwrap().join().expect("Failed to join work thread")
        {
            return Err(anyhow!("Work thread exited with error: {err}"));
        }
        Ok(())
    }

    // Get hosts.
    pub async fn get_hosts(&self) -> Result<Vec<HostInfo>, anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<Vec<HostInfo>, anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::GetHosts(sender))?;
        receiver.await?
    }

    // Get identifier of peer at `address`.
    pub async fn get_peer_id(&self, address: [u8; 6]) -> Result<Option<PeerId>, anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<Option<PeerId>, anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::GetPeerId(address, sender))?;
        receiver.await?
    }

    pub async fn get_known_peers(&self) -> Result<Vec<Peer>, anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<Vec<Peer>, anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::GetKnownPeers(sender))?;
        receiver.await?
    }

    // Set discovery state.
    pub async fn set_discovery(&self, discovery: bool) -> Result<(), anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::SetDiscovery(discovery, sender))?;
        receiver.await?
    }

    // Set discoverability state.
    pub async fn set_discoverability(&self, discoverable: bool) -> Result<(), anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::SetDiscoverability(discoverable, sender))?;
        receiver.await?
    }

    // Set connection policy.
    pub async fn set_connectability(&self, connectable: bool) -> Result<(), anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::SetConnectability(connectable, sender))?;
        receiver.await?
    }

    // Scan for nearby LE peripherals and broadcasters.
    pub async fn start_le_scan(
        &self,
        sender: futures::channel::mpsc::UnboundedSender<
            Vec<fidl_fuchsia_bluetooth_affordances::ScannedPeer>,
        >,
    ) -> Result<(), anyhow::Error> {
        let (oneshot_sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::StartLeScan(sender, oneshot_sender))?;
        receiver.await?
    }

    // Stop an ongoing LE scan. Returns an error if no scan is ongoing.
    pub async fn stop_le_scan(&self) -> Result<(), anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::StopLeScan(sender))?;
        receiver.await?
    }

    // Connect an LE peer and store the connection.
    pub async fn connect_le(&self, peer_id: PeerId) -> Result<(), anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::ConnectLe(peer_id, sender))?;
        receiver.await?
    }

    // Start advertising as an LE peripheral, accept the first connection, and return the PeerId of
    // its initiator. If `connectable` is false, then advertise and return None.
    pub async fn advertise_peripheral(
        &self,
        parameters: AdvertisingParameters,
        timeout: std::time::Duration,
    ) -> Result<Option<PeerId>, anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<Option<PeerId>, anyhow::Error>>();
        self.sender
            .clone()
            .unbounded_send(Request::AdvertisePeripheral(Box::new(parameters), timeout, sender))
            .unwrap();
        receiver.await?
    }

    // Publish a GATT service with the given parameters. GATT requests are logged.
    pub async fn publish_service(
        &self,
        uuid: Uuid,
        service_handle: ServiceHandle,
        characteristics: Vec<Characteristic>,
    ) -> Result<(), anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::PublishService(
            uuid,
            service_handle,
            characteristics,
            sender,
        ))?;
        receiver.await?
    }

    // Discover the GATT services of the currently connected LE peer.
    pub async fn discover_services(&self) -> Result<Vec<ServiceInfo>, anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<Vec<ServiceInfo>, anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::DiscoverServices(sender))?;
        receiver.await?
    }

    // Perform a short read of the GATT characteristic identified with the given handles.
    pub async fn read_characteristic(
        &self,
        service_handle: ServiceHandle,
        characteristic_handle: fidl_fuchsia_bluetooth_gatt2::Handle,
    ) -> Result<fidl_fuchsia_bluetooth_gatt2::ReadValue, anyhow::Error> {
        let (sender, receiver) =
            oneshot::channel::<Result<fidl_fuchsia_bluetooth_gatt2::ReadValue, anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::ReadCharacteristic(
            service_handle,
            characteristic_handle,
            sender,
        ))?;
        receiver.await?
    }

    // Enable notifications/indications on the GATT characteristic with the given handles.
    //
    // Only one operation on a Remote Service can be pending at a time.
    pub async fn register_characteristic_notifier(
        &self,
        service_handle: ServiceHandle,
        characteristic_handle: fidl_fuchsia_bluetooth_gatt2::Handle,
    ) -> Result<(), anyhow::Error> {
        let (sender, receiver) = oneshot::channel::<Result<(), anyhow::Error>>();
        self.sender.clone().unbounded_send(Request::RegisterCharacteristicNotifier(
            service_handle,
            characteristic_handle,
            sender,
        ))?;
        receiver.await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_update_peer_cache_handles_duplicates_in_input() {
        let peer_cache = Arc::new(Mutex::new(Vec::new()));

        let mut peer1 = Peer::default();
        peer1.id = Some(PeerId { value: 1 });
        peer1.name = Some("Peer 1".to_string());

        let mut peer2 = Peer::default();
        peer2.id = Some(PeerId { value: 1 });
        peer2.name = Some("Peer 2".to_string());

        // List of updated peers includes two entries with the same ID.
        sys::update_peer_cache(peer_cache.clone(), vec![peer1, peer2.clone()], vec![]);

        let cache = peer_cache.lock();

        // The cache should only keep the final entry.
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].name.as_deref(), Some("Peer 2"));
    }

    #[fuchsia::test]
    fn test_update_peer_cache_replaces_existing_entry() {
        let mut peer = Peer::default();
        peer.id = Some(PeerId { value: 1 });
        peer.name = Some("Peer".to_string());
        let peer_cache = Arc::new(Mutex::new(vec![peer.clone()]));

        // Update the peer currently inside the cache with a new name.
        peer.name = Some("Updated peer".to_string());
        sys::update_peer_cache(peer_cache.clone(), vec![peer], vec![]);

        let cache = peer_cache.lock();

        // The cache should only have one entry with the updated name.
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].name.as_deref(), Some("Updated peer"));
    }
}
