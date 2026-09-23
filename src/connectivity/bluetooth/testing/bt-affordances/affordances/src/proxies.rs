// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::hanging_get::client::HangingGetStream;
use fidl_fuchsia_bluetooth::PeerId;
use fidl_fuchsia_bluetooth_gatt2::{RemoteServiceProxy, Server_Marker, Server_Proxy};
use fidl_fuchsia_bluetooth_le::{
    CentralMarker, CentralProxy, ConnectionProxy, PrivilegedPeripheralMarker,
    PrivilegedPeripheralProxy,
};
use fidl_fuchsia_bluetooth_sys::{
    AccessMarker, AccessProxy, AddressLookupMarker, AddressLookupProxy, HostInfo,
    HostWatcherMarker, HostWatcherProxy, Peer, ProcedureTokenProxy,
};
use fuchsia_async::Task;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_sync::Mutex;

pub(crate) struct Proxies {
    pub(crate) access_proxy: AccessProxy,
    pub(crate) central_proxy: CentralProxy,
    pub(crate) gatt_server_proxy: Server_Proxy,
    pub(crate) peripheral_proxy: PrivilegedPeripheralProxy,
    pub(crate) address_lookup_proxy: AddressLookupProxy,
    pub(crate) host_watcher_stream: HangingGetStream<HostWatcherProxy, Vec<HostInfo>>,
    pub(crate) peer_watcher_stream: HangingGetStream<AccessProxy, (Vec<Peer>, Vec<PeerId>)>,
    pub(crate) discovery_session: Mutex<Option<ProcedureTokenProxy>>,
    pub(crate) discoverability_session: Mutex<Option<ProcedureTokenProxy>>,
    pub(crate) suppress_connections_session: Mutex<Option<ProcedureTokenProxy>>,
    pub(crate) le_scan_task: Mutex<Option<Task<()>>>,
    pub(crate) central_connection: Mutex<Option<ConnectionProxy>>,
    pub(crate) gatt_client: Option<fidl_fuchsia_bluetooth_gatt2::ClientProxy>,
    pub(crate) remote_service_proxy: Mutex<Option<RemoteServiceProxy>>,
    pub(crate) characteristic_notifier_task: Mutex<Option<Task<()>>>,
}

impl Proxies {
    pub(crate) fn connect() -> Result<Self, anyhow::Error> {
        // TODO(https://fxbug.dev/485277855): Consider exposing some of these proxies to clients
        // in order to enable RAII, e.g. `gatt_client` and `remote_service_proxy`.
        let access_proxy = connect_to_protocol::<AccessMarker>()?;
        let central_proxy = connect_to_protocol::<CentralMarker>()?;
        let gatt_server_proxy = connect_to_protocol::<Server_Marker>()?;
        let peripheral_proxy = connect_to_protocol::<PrivilegedPeripheralMarker>()?;
        let host_watcher_stream = HangingGetStream::new_with_fn_ptr(
            connect_to_protocol::<HostWatcherMarker>()?,
            HostWatcherProxy::watch,
        );
        let address_lookup_proxy = connect_to_protocol::<AddressLookupMarker>()?;
        let peer_watcher_stream =
            HangingGetStream::new_with_fn_ptr(access_proxy.clone(), AccessProxy::watch_peers);

        Ok(Proxies {
            access_proxy,
            central_proxy,
            gatt_server_proxy,
            peripheral_proxy,
            address_lookup_proxy,
            host_watcher_stream,
            peer_watcher_stream,
            discovery_session: Mutex::new(None),
            discoverability_session: Mutex::new(None),
            suppress_connections_session: Mutex::new(None),
            le_scan_task: Mutex::new(None),
            central_connection: Mutex::new(None),
            gatt_client: None,
            remote_service_proxy: Mutex::new(None),
            characteristic_notifier_task: Mutex::new(None),
        })
    }
}
