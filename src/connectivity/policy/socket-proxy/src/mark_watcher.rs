// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use anyhow::Context as _;
use fidl_fuchsia_net_policy_properties as fnp_properties;
use futures::FutureExt as _;
use log::{debug, error};

use crate::SocketProxy;
use crate::registry::{NetcfgMarkState, Registry};

/// Manages the state of the active network's property watcher.
#[derive(Default)]
struct WatcherState {
    watcher: Option<fnp_properties::PropertyWatcherProxy>,
    next_watch: futures::future::OptionFuture<
        fidl::client::QueryResponseFut<
            Result<fnp_properties::PropertyUpdate, fnp_properties::PropertyWatcherError>,
        >,
    >,
}

impl WatcherState {
    /// Sets a new active property watcher and initiates a watch future.
    fn set(&mut self, watcher: fnp_properties::PropertyWatcherProxy) {
        self.next_watch = Some(watcher.watch()).into();
        self.watcher = Some(watcher);
    }

    /// Re-arms the watch future on the current active watcher.
    fn watch_again(&mut self) {
        self.next_watch = self.watcher.as_ref().map(|w| w.watch()).into();
    }

    /// Clears the active watcher state and resets the netcfg mark to `None`.
    async fn reset(&mut self, registry: &Registry) {
        self.watcher = None;
        self.next_watch = None.into();
        registry.set_netcfg_mark(NetcfgMarkState::NoDefault).await;
    }
}

/// Continuously watches network properties from
/// `fuchsia.net.policy.properties.Networks`.
pub(crate) async fn watch_properties(proxy: Arc<SocketProxy>) {
    let networks_proxy = match fuchsia_component::client::connect_to_protocol::<
        fnp_properties::NetworksMarker,
    >() {
        Ok(p) => p,
        Err(e) => {
            debug!("failed to connect to fuchsia.net.policy.properties.Networks protocol: {e:?}");
            proxy.registry.set_netcfg_mark(NetcfgMarkState::NoDefault).await;
            return;
        }
    };

    if let Err(e) = run_watch_properties_loop(&proxy, &networks_proxy).await {
        debug!("Networks property watcher loop ended: {e:?}");
    }
    proxy.registry.set_netcfg_mark(NetcfgMarkState::NoDefault).await;
}

/// Drives two coordinated hanging-get loops:
/// * `WatchDefault`: tracks changes to the active default network token.
/// * `PropertyWatcher.Watch`: tracks socket mark property changes on that active network.
///
/// If the default network is unset, disconnected, or errors, the netcfg mark is reset
/// to `None` so socket-proxy falls back to the legacy/Starnix mark.
async fn run_watch_properties_loop(
    proxy: &Arc<SocketProxy>,
    networks_proxy: &fnp_properties::NetworksProxy,
) -> Result<(), anyhow::Error> {
    let mut next_default = networks_proxy.watch_default().fuse();
    let mut watcher_state = WatcherState::default();

    let create_watcher = |token: fnp_properties::NetworkToken| {
        let (watcher, server_end) =
            fidl::endpoints::create_proxy::<fnp_properties::PropertyWatcherMarker>();
        let request = fnp_properties::NetworksWatchPropertiesRequest {
            network: Some(token),
            properties: Some(fnp_properties::PropertyInterest::SOCKET_MARKS),
            watcher: Some(server_end),
            ..Default::default()
        };
        (networks_proxy.watch_properties(request), watcher)
    };

    loop {
        futures::select! {
            response = next_default => {
                let response = match response {
                    Ok(r) => r,
                    Err(fidl::Error::ClientChannelClosed { .. }) => {
                        debug!("Networks.WatchDefault channel closed");
                        return Ok(());
                    }
                    Err(e) => return Err(e).context("WatchDefault RPC failed"),
                };
                match response {
                    fnp_properties::NetworksWatchDefaultResponse::Network(token) => {
                        let (watch_props_fut, watcher) = create_watcher(token);
                        match watch_props_fut.await {
                            Ok(Ok(())) => {
                                watcher_state.set(watcher);
                            }
                            Ok(Err(fnp_properties::WatchError::InvalidNetworkToken)) => {
                                debug!("WatchProperties: network token is no longer valid");
                                watcher_state.reset(&proxy.registry).await;
                            }
                            Err(fidl::Error::ClientChannelClosed { .. }) => {
                                debug!("Networks.WatchProperties channel closed");
                                watcher_state.reset(&proxy.registry).await;
                                return Ok(());
                            }
                            err => {
                                error!("WatchProperties failed: {err:?}");
                                watcher_state.reset(&proxy.registry).await;
                            }
                        }
                    }
                    fnp_properties::NetworksWatchDefaultResponse::NoDefaultNetwork(_) => {
                        watcher_state.reset(&proxy.registry).await;
                    }
                    fnp_properties::NetworksWatchDefaultResponse::__SourceBreaking { .. } => {
                        unreachable!("Networks.WatchDefault should not return __SourceBreaking");
                    }
                }
                next_default = networks_proxy.watch_default().fuse();
            }

            props_res = &mut watcher_state.next_watch => {
                match props_res.expect("next_watch should always be Some") {
                    Ok(Ok(updates)) => {
                        let mark = updates.socket_marks.as_ref().and_then(|m| m.mark_1);
                        proxy.registry.set_netcfg_mark(NetcfgMarkState::Default(mark)).await;
                        watcher_state.watch_again();
                    }
                    Ok(Err(fnp_properties::PropertyWatcherError::NetworkGone))
                    | Err(fidl::Error::ClientChannelClosed { .. }) => {
                        debug!("PropertyWatcher ended (network removed or watcher closed)");
                        watcher_state.reset(&proxy.registry).await;
                    }
                    err => {
                        error!("PropertyWatcher failed: {err:?}");
                        watcher_state.reset(&proxy.registry).await;
                    }
                }
            }
        }
    }
}
