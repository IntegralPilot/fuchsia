// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, format_err};
use fidl_fuchsia_bluetooth::{self as fbt, DeviceClass};
use fidl_fuchsia_bluetooth_host::{
    BondingDelegateMarker, BondingDelegateProxy, BondingDelegateWatchBondsResponse,
    DiscoverySessionMarker, DiscoverySessionProxy, HostProxy, HostStartDiscoveryRequest,
    PeerWatcherGetNextResponse, PeerWatcherMarker,
};
use fidl_fuchsia_bluetooth_sys as sys;
use fuchsia_async as fasync;
use fuchsia_bluetooth::inspect::Inspectable;
use fuchsia_bluetooth::types::{Address, BondingData, HostData, HostId, HostInfo, Peer, PeerId};
use fuchsia_sync::RwLock;
use futures::future::try_join_all;
use futures::{Future, FutureExt, TryFutureExt};
use log::{debug, error, info, trace, warn};
use std::pin::pin;
use std::sync::{Arc, Weak};

#[cfg(test)]
use fidl_fuchsia_bluetooth_sys::TechnologyType;

use crate::build_config;
use crate::types::{self, Error, from_fidl_result};

/// When the host dispatcher requests being discoverable on a host device, the host device enables
/// discoverable and returns a HostDiscoverableSession. The discoverable state on the host device
/// persists until this session is dropped.
pub struct HostDiscoverableSession {
    host: Weak<HostDeviceState>,
}

impl Drop for HostDiscoverableSession {
    fn drop(&mut self) {
        trace!("HostDiscoverableSession ended");
        if let Some(host) = self.host.upgrade() {
            let await_response = host.proxy.set_discoverable(false);
            fasync::Task::spawn(async move {
                if let Err(err) = await_response.await {
                    // TODO(https://fxbug.dev/42121837) - we should close the host channel if an error is returned
                    warn!("Unexpected error response when disabling discoverable: {:?}", err);
                }
            })
            .detach();
        }
    }
}

#[derive(Clone)]
pub struct HostDevice(Arc<HostDeviceState>);

pub struct HostDeviceState {
    // TODO(https://fxbug.dev/324954255): Remove device_path
    device_path: String,
    proxy: HostProxy,
    info: RwLock<Inspectable<HostInfo>>,
    bonding_delegate_proxy: BondingDelegateProxy,
}

/// A type for easy debug printing of the main identifiers for the host device, namely: The device
/// path, the host address and the host id.
#[derive(Clone, Debug)]
pub struct HostDebugIdentifiers {
    // Field is only read by the Debug impl for logging.
    #[allow(dead_code)]
    id: HostId,
    // Field is only read by the Debug impl for logging.
    #[allow(dead_code)]
    address: Address,
    // Field is only read by the Debug impl for logging.
    #[allow(dead_code)]
    path: String,
}

// Many HostDevice methods return impl Future rather than being implemented as `async`. This has an
// important behavioral difference in that the function body is triggered immediately when called.
//
// If they were instead declared async, the function body would not be executed until the first time
// the future was polled.
impl HostDevice {
    pub fn new(device_path: String, proxy: HostProxy, info: Inspectable<HostInfo>) -> Self {
        let (bonding_delegate_proxy, server) =
            fidl::endpoints::create_proxy::<BondingDelegateMarker>();
        proxy.set_bonding_delegate(server).unwrap();
        HostDevice(Arc::new(HostDeviceState {
            device_path,
            proxy,
            info: RwLock::new(info),
            bonding_delegate_proxy,
        }))
    }

    pub fn proxy(&self) -> &HostProxy {
        &self.0.proxy
    }

    pub fn info(&self) -> HostInfo {
        self.0.info.read().clone()
    }

    pub fn id(&self) -> HostId {
        self.0.info.read().id.into()
    }

    pub fn public_address(&self) -> Address {
        // The list of known Host Addresses is always nonempty. The Public address is guaranteed to
        // be listed first.
        self.0.info.read().addresses[0]
    }

    /// Validates that the raw FIDL bonding data is well-formed and that its
    /// `local_address` matches this host adapter's public controller address.
    /// Returns the converted internal `BondingData` type if successful.
    fn validate_bond_data(&self, fidl_data: sys::BondingData) -> types::Result<BondingData> {
        let data: BondingData = fidl_data.try_into()?;
        let public_address = self.public_address();
        if data.local_address != public_address {
            return Err(fuchsia_bluetooth::Error::FailedConversion(format!(
                "mismatched local_address {:?} (expected {:?})",
                data.local_address, public_address
            ))
            .into());
        }
        Ok(data)
    }

    /// Convenience method to produce a type for easy debug printing of the main identifiers for the
    /// host device, namely: The device path, the host address and the host id.
    pub fn debug_identifiers(&self) -> HostDebugIdentifiers {
        HostDebugIdentifiers {
            id: self.id(),
            address: self.public_address(),
            path: self.path().to_string(),
        }
    }

    pub fn path(&self) -> &str {
        &self.0.device_path
    }

    pub fn set_name(&self, name: String) -> impl Future<Output = types::Result<()>> {
        self.0.proxy.set_local_name(&name).map(from_fidl_result)
    }

    pub fn set_device_class(&self, dc: DeviceClass) -> impl Future<Output = types::Result<()>> {
        self.0.proxy.set_device_class(&dc).map(from_fidl_result)
    }

    pub fn start_discovery(&self) -> types::Result<DiscoverySessionProxy> {
        let (proxy, server) = fidl::endpoints::create_proxy::<DiscoverySessionMarker>();
        let result = self.0.proxy.start_discovery(HostStartDiscoveryRequest {
            token: Some(server),
            ..Default::default()
        });
        result.map_err(Error::from).map(|_| proxy)
    }

    pub fn connect(&self, id: PeerId) -> impl Future<Output = types::Result<()>> {
        let id: fbt::PeerId = id.into();
        self.0.proxy.connect(&id).map(from_fidl_result)
    }

    pub fn disconnect(&self, id: PeerId) -> impl Future<Output = types::Result<()>> {
        let id: fbt::PeerId = id.into();
        self.0.proxy.disconnect(&id).map(from_fidl_result)
    }

    pub fn pair(
        &self,
        id: PeerId,
        options: sys::PairingOptions,
    ) -> impl Future<Output = types::Result<()>> {
        let id: fbt::PeerId = id.into();
        self.0.proxy.pair(&id, &options).map(from_fidl_result)
    }

    pub fn forget(&self, id: PeerId) -> impl Future<Output = types::Result<()>> {
        let id: fbt::PeerId = id.into();
        self.0.proxy.forget(&id).map(from_fidl_result)
    }

    pub fn shutdown(&self) -> types::Result<()> {
        self.0.proxy.shutdown().map_err(|e| e.into())
    }

    pub fn restore_bonds(
        &self,
        bonds: Vec<BondingData>,
    ) -> impl Future<Output = types::Result<Vec<sys::BondingData>>> + use<> {
        // TODO(https://fxbug.dev/42160922): Due to the maximum message size, the RestoreBonds call has an
        // upper limit on the number of bonds that may be restored at once. However, this limit is
        // based on the complexity of fields packed into sys::BondingData, which can be measured
        // dynamically with measure-tape as the vector is built. Maximizing the number of bonds per
        // call may improve performance. Instead, a conservative fixed maximum is chosen here (at
        // this time, the maximum message size is 64 KiB and a fully-populated BondingData without
        // peer service records is close to 700 B).
        const MAX_BONDS_PER_RESTORE_BONDS: usize = 16;
        let bonds: Vec<_> = bonds.into_iter().map(sys::BondingData::from).collect();
        let bonds_chunks = bonds.chunks(MAX_BONDS_PER_RESTORE_BONDS);

        // Bonds that can't be restored are sent back in Ok(Vec<_>), which would not cause
        // try_join_all to bail early
        try_join_all(
            bonds_chunks
                .map(|c| self.0.bonding_delegate_proxy.restore_bonds(c).map_err(|e| e.into())),
        )
        .map_ok(|v| v.into_iter().flatten().collect())
    }

    pub fn set_connectable(&self, value: bool) -> impl Future<Output = types::Result<()>> + use<> {
        self.0.proxy.set_connectable(value).map(from_fidl_result)
    }

    pub fn establish_discoverable_session(
        &self,
    ) -> impl Future<Output = types::Result<HostDiscoverableSession>> + use<> {
        let host = Arc::downgrade(&self.0);
        self.0
            .proxy
            .set_discoverable(true)
            .map(from_fidl_result)
            .map_ok(move |_| HostDiscoverableSession { host })
    }

    pub fn set_local_data(&self, data: HostData) -> types::Result<()> {
        self.0.proxy.set_local_data(&data.into()).map_err(|e| e.into())
    }

    pub fn enable_privacy(&self, enable: bool) -> types::Result<()> {
        self.0.proxy.enable_privacy(enable).map_err(Error::from)
    }

    pub fn enable_background_scan(&self, enable: bool) -> types::Result<()> {
        self.0.proxy.enable_background_scan(enable).map_err(Error::from)
    }

    pub fn set_bredr_security_mode(&self, mode: sys::BrEdrSecurityMode) -> types::Result<()> {
        self.0.proxy.set_br_edr_security_mode(mode).map_err(Error::from)
    }

    pub fn set_le_security_mode(&self, mode: sys::LeSecurityMode) -> types::Result<()> {
        self.0.proxy.set_le_security_mode(mode).map_err(Error::from)
    }

    pub fn apply_config(
        &self,
        config: build_config::Config,
    ) -> impl Future<Output = types::Result<()>> {
        let equivalent_settings = config.into();
        self.apply_sys_settings(&equivalent_settings)
    }

    /// `apply_sys_settings` applies each field present in `settings` to the host device, leaving
    /// omitted parameters unchanged. If present, the `Err` arm of the returned future's output
    /// is the error associated with the first failure to apply a setting to the host device.
    pub fn apply_sys_settings(
        &self,
        settings: &sys::Settings,
    ) -> impl Future<Output = types::Result<()>> + use<> {
        let mut error_occurred = settings.le_privacy.map(|en| self.enable_privacy(en)).transpose();
        if let Ok(_) = error_occurred {
            error_occurred =
                settings.le_background_scan.map(|en| self.enable_background_scan(en)).transpose()
        }
        if let Ok(_) = error_occurred {
            error_occurred =
                settings.bredr_security_mode.map(|m| self.set_bredr_security_mode(m)).transpose();
        }
        if let Ok(_) = error_occurred {
            error_occurred =
                settings.le_security_mode.map(|m| self.set_le_security_mode(m)).transpose();
        }
        let connectable_fut = error_occurred
            .map(|_| settings.bredr_connectable_mode.map(|c| self.set_connectable(c)));
        async move {
            match connectable_fut {
                Ok(Some(fut)) => fut.await,
                res => res.map(|_| ()),
            }
        }
    }

    /// Monitors updates from a bt-host device and notifies `listener`. The returned Future represents
    /// a task that never ends in successful operation and only returns in case of a failure to
    /// communicate with the bt-host device.
    pub async fn watch_events<H: HostListener + Clone>(self, listener: H) -> anyhow::Result<()> {
        let watch_peers = self.watch_peers(listener.clone());
        let watch_bonds = self.watch_bonds(listener.clone());
        let watch_state = self.watch_state(listener);
        let watch_peers = pin!(watch_peers);
        let watch_state = pin!(watch_state);
        futures::select! {
            res = watch_peers.fuse() => res.context("failed to relay peer watcher from Host"),
            res = watch_state.fuse() => res.context("failed to watch Host for HostInfo"),
            res = watch_bonds.fuse() => res.context("failed to watch bonds"),
        }
    }

    pub(crate) fn watch_bonds<H: HostListener>(
        &self,
        mut listener: H,
    ) -> impl Future<Output = types::Result<()>> + use<H> {
        let bonding_delegate_proxy = self.0.bonding_delegate_proxy.clone();
        let host = self.clone();
        async move {
            loop {
                match bonding_delegate_proxy.watch_bonds().await {
                    Ok(BondingDelegateWatchBondsResponse::Updated(data)) => {
                        info!("Received bonding data");
                        let data = match host.validate_bond_data(data) {
                            Err(e) => {
                                error!("Invalid bonding data, ignoring: {:#?}", e);
                                continue;
                            }
                            Ok(data) => data,
                        };
                        if let Err(e) = listener.on_new_host_bond(data.into()).await {
                            error!("Failed to persist bonding data: {:#?}", e);
                        }
                    }
                    Ok(BondingDelegateWatchBondsResponse::Removed(_)) => {
                        // TODO(https://fxbug.dev/42158854): Support removing bonds.
                        info!("ignoring watch_bonds() removed bond result");
                    }
                    Ok(_) => {
                        debug!("ignoring watch_bonds() unknown result");
                    }
                    Err(err) => {
                        return Err(format_err!("watch_bonds() error: {:?}", err).into());
                    }
                }
            }
        }
    }

    fn watch_peers<H: HostListener>(
        &self,
        mut listener: H,
    ) -> impl Future<Output = types::Result<()>> + use<H> {
        let proxy = self.0.proxy.clone();
        async move {
            let (peer_watcher_proxy, peer_watcher_server) =
                fidl::endpoints::create_proxy::<PeerWatcherMarker>();
            proxy.set_peer_watcher(peer_watcher_server)?;
            loop {
                match peer_watcher_proxy.get_next().await {
                    Ok(PeerWatcherGetNextResponse::Updated(updated)) => {
                        for peer in updated.into_iter() {
                            listener.on_peer_updated(peer.try_into()?).await;
                        }
                    }
                    Ok(PeerWatcherGetNextResponse::Removed(removed)) => {
                        for id in removed.into_iter() {
                            listener.on_peer_removed(id.into()).await;
                        }
                    }
                    Ok(_) => {
                        debug!("ignoring unknown PeerWatcher response");
                    }
                    Err(err) => {
                        return Err(format_err!("PeerWatcher error: {:?}", err).into());
                    }
                }
            }
        }
    }

    async fn watch_state<H: HostListener>(self, mut listener: H) -> types::Result<()> {
        loop {
            let info = self.clone().refresh_host_info().await?;
            listener.on_host_updated(info).await?;
        }
    }

    async fn refresh_host_info(self) -> types::Result<HostInfo> {
        let proxy = self.0.proxy.clone();
        let info = proxy.watch_state().await?;
        let info: HostInfo = info.try_into()?;
        debug!("HostDevice::refresh_host_info: {:?}", info);
        self.0.info.write().update(info.clone());
        Ok(info)
    }

    #[cfg(test)]
    pub(crate) async fn refresh_test_host_info(self) -> types::Result<HostInfo> {
        self.refresh_host_info().await
    }

    #[cfg(test)]
    pub(crate) fn mock_from_id(
        id: HostId,
    ) -> (fidl_fuchsia_bluetooth_host::HostRequestStream, HostDevice) {
        let (host_proxy, host_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_bluetooth_host::HostMarker>();
        let id_val = id.0 as u8;
        let address = Address::Public([id_val; 6]);
        let path = format!("/dev/host{}", id_val);
        let host_device = HostDevice::mock(id, address, path, host_proxy);
        (host_stream, host_device)
    }

    #[cfg(test)]
    pub(crate) fn mock(id: HostId, address: Address, path: String, proxy: HostProxy) -> HostDevice {
        let info = HostInfo {
            id,
            technology: TechnologyType::DualMode,
            addresses: vec![address],
            active: false,
            local_name: None,
            discoverable: false,
            discovering: false,
        };
        HostDevice::new(path, proxy, Inspectable::new(info, fuchsia_inspect::Node::default()))
    }
}

/// A type that can be notified when a Host or the peers it knows about change
///
/// Each of these trait methods returns a future that should be polled to completion. Once that
/// returned future is complete, the target type can be considered to have been notified of the
/// update event. This allows asynchronous notifications such as via an asynchronous msg channel.
///
/// The host takes care to serialize updates so that subsequent notifications are not triggered
/// until the previous future has been completed. This allows a HostListener type to ensure they
/// maintain ordering. If required, the implementation of these methods should ensure that
/// completing the future before sending a new update is sufficient to ensure ordering.
///
/// Since the notifying Host will wait on the completion of the returned futures, HostListeners
/// should not perform heavy work that may block or take an unnecessary length of time. If the
/// implementor needs to perform potentially-blocking work in response to these notifications, that
/// should be done in a separate task or thread that does not block the returned future.
pub trait HostListener {
    /// The return Future type of `on_peer_updated`
    type PeerUpdatedFut: Future<Output = ()>;
    /// The return Future type of `on_peer_removed`
    type PeerRemovedFut: Future<Output = ()>;
    /// The return Future type of `on_new_host_bond`
    type HostBondFut: Future<Output = Result<(), anyhow::Error>>;
    /// The return Future type of `on_host_updated`
    type HostInfoFut: Future<Output = Result<(), anyhow::Error>>;

    /// Indicate that a Peer `Peer` has been added or updated
    fn on_peer_updated(&mut self, peer: Peer) -> Self::PeerUpdatedFut;

    /// Indicate that a Peer identified by `id` has been removed
    fn on_peer_removed(&mut self, id: PeerId) -> Self::PeerRemovedFut;

    /// Indicate that a new bond described by `data` has been made
    fn on_new_host_bond(&mut self, data: BondingData) -> Self::HostBondFut;

    /// Indicate that the Host now has properties described by `info`
    fn on_host_updated(&mut self, info: HostInfo) -> Self::HostInfoFut;
}

#[cfg(test)]
pub(crate) mod test {
    use fidl_fuchsia_bluetooth_host::{BondingDelegateRequestStream, HostWatchStateResponder};

    use super::*;

    use async_helpers::maybe_stream::MaybeStream;
    use fidl_fuchsia_bluetooth_host::{
        BondingDelegateRequest, DiscoverySessionRequest, DiscoverySessionRequestStream,
        HostRequest, HostRequestStream, PeerWatcherRequestStream,
    };
    use fidl_fuchsia_bluetooth_sys::HostInfo as FidlHostInfo;
    use futures::{StreamExt, select};

    pub(crate) struct FakeHostServer {
        host_stream: HostRequestStream,
        host_info: Arc<RwLock<HostInfo>>,
        discovery_stream: MaybeStream<DiscoverySessionRequestStream>,
        peer_watcher_stream: MaybeStream<PeerWatcherRequestStream>,
        bonding_delegate_stream: MaybeStream<BondingDelegateRequestStream>,
        watch_state_responder: Option<HostWatchStateResponder>,
        last_host_info: Option<HostInfo>,
        pub num_restore_bonds_calls: i32,
    }

    impl FakeHostServer {
        pub(crate) fn new(
            server: HostRequestStream,
            host_info: Arc<RwLock<HostInfo>>,
        ) -> FakeHostServer {
            FakeHostServer {
                host_stream: server,
                host_info,
                discovery_stream: MaybeStream::default(),
                peer_watcher_stream: MaybeStream::default(),
                bonding_delegate_stream: MaybeStream::default(),
                watch_state_responder: None,
                last_host_info: None,
                num_restore_bonds_calls: 0,
            }
        }

        async fn maybe_send_watch_state_response(&mut self) {
            if self.watch_state_responder.is_none() {
                return;
            }
            let host_info = self.host_info.read().clone();
            if self.last_host_info.as_ref() != Some(&host_info) {
                let responder = self.watch_state_responder.take().unwrap();
                assert_matches::assert_matches!(
                    responder.send(&FidlHostInfo::from(host_info.clone())),
                    Ok(())
                );
                self.last_host_info = Some(host_info);
            }
        }

        pub(crate) async fn run(&mut self) -> Result<(), anyhow::Error> {
            loop {
                select! {
                    req = self.bonding_delegate_stream.next() => {
                        info!("FakeHostServer: bonding_delegate_stream: {:?}", req);
                        match req {
                            Some(Ok(BondingDelegateRequest::RestoreBonds { responder, .. })) => {
                                let _ = responder.send(&[]);
                                self.num_restore_bonds_calls += 1;
                            }
                            Some(Ok(BondingDelegateRequest::WatchBonds {..})) => {}
                            x => panic!("Unexpected request in fake host server: {:?}", x),
                        }
                    },
                    req = self.discovery_stream.next() => {
                         info!("FakeHostServer: discovery_stream: {:?}", req);
                         match req {
                            Some(Ok(DiscoverySessionRequest::Stop { .. })) | None => {
                                 assert!(self.host_info.read().discovering);
                                 self.host_info.write().discovering = false;
                                 self.discovery_stream = MaybeStream::default();
                            }
                            x => panic!("Unexpected request in fake host server: {:?}", x),
                         }
                    }
                    req = self.host_stream.next() => {
                         info!("FakeHostServer: {:?}", req);
                         match req {
                             Some(Ok(HostRequest::SetLocalName {local_name, responder })) => {
                                 self.host_info.write().local_name = Some(local_name);
                                 let _ = responder.send(Ok(()))?;
                             }
                             Some(Ok(HostRequest::StartDiscovery { payload, .. })) => {
                                 assert!(!self.host_info.read().discovering);
                                 self.host_info.write().discovering = true;
                                 self.discovery_stream.set(payload.token.unwrap().into_stream());
                             }
                             Some(Ok(HostRequest::WatchState { responder })) => {
                                self.watch_state_responder = Some(responder);
                             }
                             Some(Ok(HostRequest::SetPeerWatcher {peer_watcher, ..})) => {
                                assert!(!self.peer_watcher_stream.is_some());
                                self.peer_watcher_stream.set(peer_watcher.into_stream());
                             }
                             Some(Ok(HostRequest::SetBondingDelegate { delegate, .. })) => {
                                assert!(!self.bonding_delegate_stream.is_some());
                                self.bonding_delegate_stream.set(delegate.into_stream());
                             }
                             None => {
                                 self.host_info.write().discovering = false;
                                 self.maybe_send_watch_state_response().await;
                                 return Ok(());
                             }
                             x => panic!("Unexpected request in fake host server: {:?}", x),
                         }
                    }
                }
                self.maybe_send_watch_state_response().await;
            }
        }
    }

    /// Runs a HostRequestStream that handles StartDiscovery & WatchState requests.
    pub(crate) async fn run_discovery_host_server(
        server: HostRequestStream,
        host_info: Arc<RwLock<HostInfo>>,
    ) -> Result<(), anyhow::Error> {
        let mut host_server = FakeHostServer::new(server, host_info);
        host_server.run().await
    }
}
