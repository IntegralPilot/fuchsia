// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::{self, IfaceDevice, IfaceMap, IfaceWrapper, NewIface, PhyDevice, PhyMap};
use crate::{phy_event_service, watcher_service};
use anyhow::{Error, format_err};
use core::sync::atomic::AtomicUsize;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_device_service::{self as fidl_svc, DeviceMonitorRequest};
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_internal::TxPowerScenario;
use fidl_fuchsia_wlan_sme as fidl_sme;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, select};
use ieee80211::{MacAddr, MacAddrBytes, NULL_ADDR};
use log::{error, info, warn};
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};
use wlan_fidl_ext::{ResponderExt, TryUnpack, WithName};

/// Thread-safe counter for spawned ifaces.
pub struct IfaceCounter(AtomicUsize);

impl IfaceCounter {
    pub fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    /// Provides the caller with a new unique id.
    pub fn next_iface_id(&self) -> usize {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

#[allow(clippy::too_many_arguments, reason = "mass allow for https://fxbug.dev/381896734")]
pub(crate) async fn handle_monitor_request(
    request: DeviceMonitorRequest,
    phys: &PhyMap,
    iface_wrapper: &IfaceWrapper<'_>,
    watcher_service: &watcher_service::WatcherService<PhyDevice, IfaceDevice>,
    phy_event_service: &phy_event_service::PhyEventService,
    new_iface_sink: &mpsc::UnboundedSender<NewIface>,
    iface_counter: &IfaceCounter,
    cfg: &wlandevicemonitor_config::Config,
) -> Result<(), Error> {
    match request {
        DeviceMonitorRequest::ListPhys { responder } => responder.send(&list_phys(phys))?,
        DeviceMonitorRequest::ListIfaces { responder } => {
            responder.send(&list_ifaces(iface_wrapper.ifaces))?
        }
        DeviceMonitorRequest::GetSupportedMacRoles { phy_id, responder } => responder
            .send(get_supported_mac_roles(phys, phy_id).await.as_deref().map_err(|e| *e))?,
        DeviceMonitorRequest::WatchDevices { watcher, control_handle: _ } => {
            watcher_service
                .add_watcher(watcher)
                .unwrap_or_else(|e| error!("error registering a device watcher: {}", e));
        }
        DeviceMonitorRequest::WatchPhyEvents { watcher, control_handle: _ } => {
            phy_event_service.add_watcher(watcher);
        }
        DeviceMonitorRequest::GetCountry { phy_id, responder } => {
            responder.send(get_country(phys, phy_id).await.as_ref().map_err(|s| s.into_raw()))?;
        }
        DeviceMonitorRequest::SetCountry { req, responder } => {
            let status = set_country(phys, req).await;
            responder.send(zx::Status::result_into_raw(status))?;
        }
        DeviceMonitorRequest::ClearCountry { req, responder } => {
            let status = clear_country(phys, req).await;
            responder.send(zx::Status::result_into_raw(status))?;
        }
        DeviceMonitorRequest::SetPowerSaveMode { req, responder } => {
            let status = set_power_save_mode(phys, req).await;
            responder.send(zx::Status::result_into_raw(status))?;
        }
        DeviceMonitorRequest::GetPowerSaveMode { phy_id, responder } => responder
            .send(get_power_save_mode(phys, phy_id).await.as_ref().map_err(|s| s.into_raw()))?,
        DeviceMonitorRequest::CreateIface { payload, responder } => {
            let ((phy_id, role, sta_address), responder) = match responder
                .unpack_fields_or_else_send(
                    (
                        payload.phy_id.with_name("phy_id"),
                        payload.role.with_name("role"),
                        payload.sta_address.with_name("sta_address"),
                    ),
                    || Err(fidl_svc::DeviceMonitorError::unknown()),
                ) {
                Err(e) => {
                    warn!("Failed to create iface: {:?}", e);
                    return Ok(());
                }
                Ok(x) => x,
            };
            let sta_address = MacAddr::from(sta_address);

            let (new_iface, inspect_vmo) = match create_iface(
                new_iface_sink,
                phys,
                phy_id,
                role,
                sta_address,
                iface_counter,
                cfg,
            )
            .await
            {
                Err(e) => {
                    error!("{:?}", e.context("Failed to create iface"));
                    responder.send(Err(fidl_svc::DeviceMonitorError::unknown()))?;
                    return Ok(());
                }
                Ok(x) => x,
            };

            info!("iface #{} started ({:?})", new_iface.id, new_iface.phy_ownership);
            iface_wrapper.insert(
                new_iface.id,
                IfaceDevice {
                    phy_ownership: new_iface.phy_ownership,
                    generic_sme: new_iface.generic_sme,
                },
                inspect_vmo,
            );

            let resp = fidl_svc::DeviceMonitorCreateIfaceResponse {
                iface_id: Some(new_iface.id),
                ..Default::default()
            };
            responder.send(Ok(&resp))?;
        }
        DeviceMonitorRequest::QueryIface { iface_id, responder } => {
            let result = query_iface(iface_wrapper.ifaces, iface_id).await;
            responder.send(result.as_ref().map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::QueryIfaceCapabilities { iface_id, responder } => {
            let result = query_device_capability(iface_wrapper.ifaces, iface_id).await;
            responder.send(result.as_ref().map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::DestroyIface { req, responder } => {
            let result = destroy_iface(phys, iface_wrapper, req.iface_id).await;
            let status = into_status_and_opt(result).0;
            responder.send(zx::Status::result_into_raw(status))?;
        }
        DeviceMonitorRequest::PowerDown { phy_id, responder } => {
            let status = power_down(phys, phy_id).await;
            responder.send(status.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::PowerUp { phy_id, responder } => {
            let status = power_up(phys, phy_id).await;
            responder.send(status.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::Reset { phy_id, responder } => {
            let status = reset(phys, phy_id).await;
            responder.send(status.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::GetPowerElementDependencyToken { phy_id, responder } => {
            responder
                .send(get_power_element_dependency_token(phys, phy_id).map_err(|s| s.into_raw()))?;
        }
        DeviceMonitorRequest::GetPowerState { phy_id, responder } => {
            responder.send(
                get_power_state(phys, phy_id).await.as_ref().map_err(|s| s.into_raw()).copied(),
            )?;
        }
        DeviceMonitorRequest::SetBtCoexistenceMode { phy_id, mode, responder } => {
            responder.send(
                set_bt_coexistence_mode(phys, phy_id, mode)
                    .await
                    .as_ref()
                    .map_err(|s| s.into_raw())
                    .copied(),
            )?;
        }
        DeviceMonitorRequest::GetClientSme { iface_id, sme_server, responder } => {
            let result = get_client_sme(iface_wrapper.ifaces, iface_id, sme_server).await;
            responder.send(result.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::GetApSme { iface_id, sme_server, responder } => {
            let result = get_ap_sme(iface_wrapper.ifaces, iface_id, sme_server).await;
            responder.send(result.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::GetSmeTelemetry { iface_id, telemetry_server, responder } => {
            let result = get_sme_telemetry(iface_wrapper.ifaces, iface_id, telemetry_server).await;
            responder.send(result.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::SetTxPowerScenario { phy_id, scenario, responder } => {
            let result = set_tx_power_scenario(phys, phy_id, &scenario).await;
            responder.send(result.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::ResetTxPowerScenario { phy_id, responder } => {
            let result = reset_tx_power_scenario(phys, phy_id).await;
            responder.send(result.map_err(|e| e.into_raw()))?;
        }
        DeviceMonitorRequest::GetTxPowerScenario { phy_id, responder } => {
            let result = get_tx_power_scenario(phys, phy_id).await;
            responder.send(result.map_err(|e| e.into_raw()))?;
        }
    }
    Ok(())
}

pub(crate) async fn handle_new_iface_stream(
    phys: &PhyMap,
    iface_wrapper: &IfaceWrapper<'_>,
    mut iface_stream: mpsc::UnboundedReceiver<NewIface>,
) -> Result<(), Error> {
    let mut futures_unordered = FuturesUnordered::new();
    loop {
        select! {
            _ = futures_unordered.select_next_some() => (),
            new_iface = iface_stream.next() => {
                if let Some(new_iface) = new_iface {
                    futures_unordered.push(
                        handle_single_new_iface(
                            phys,
                            iface_wrapper,
                            new_iface
                        )
                    );
                }
            }
        }
    }
}

async fn handle_single_new_iface(
    phys: &PhyMap,
    iface_wrapper: &IfaceWrapper<'_>,
    new_iface: NewIface,
) {
    let mut event_stream = new_iface.generic_sme.take_event_stream().fuse();
    // GenericSme does not currently have any client-bound messages, so this is
    // exclusively to monitor for the channel epitaph and respond appropriately.
    // A clean SME shutdown should always result in an OK epitaph -- initiate
    // iface destruction on any other behavior to remove stale info.
    while !event_stream.is_done() {
        match event_stream.next().await {
            None => (),
            Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. })) => {
                if epitaph.is_ok() {
                    info!("Generic SME event stream closed cleanly.");
                    return;
                } else {
                    error!("Generic SME event stream closed with error: {epitaph:?}");
                    break;
                }
            }
            Some(message) => {
                error!(
                    "Unexpected message from Generic SME event stream: {:?}. Aborting Generic SME.",
                    message
                );
                break;
            }
        }
    }
    match destroy_iface(phys, iface_wrapper, new_iface.id).await {
        Ok(()) => info!("Destroyed iface {} after its SME event stream ended", new_iface.id),
        Err(e) if e == zx::Status::NOT_FOUND => {
            info!(
                "iface {} not found when attempting to destroy after its SME event stream ended; assume successful destruction ",
                new_iface.id
            )
        }
        Err(e) => error!("Error while destroying iface {}: {}", new_iface.id, e),
    }
}

fn list_phys(phys: &PhyMap) -> Vec<u16> {
    phys.get_snapshot().keys().copied().collect()
}

fn list_ifaces(ifaces: &IfaceMap) -> Vec<u16> {
    ifaces.get_snapshot().keys().copied().collect()
}

async fn get_country(
    phys: &PhyMap,
    phy_id: u16,
) -> Result<fidl_svc::GetCountryResponse, zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    let alpha2 = phy.proxy.get_country().await.map_phy_result(phy_id, "GetCountry")?;
    Ok(fidl_svc::GetCountryResponse { alpha2 })
}

async fn set_country(phys: &PhyMap, req: fidl_svc::SetCountryRequest) -> Result<(), zx::Status> {
    let phy_id = req.phy_id;
    let phy = match phys.get(&req.phy_id) {
        None => return Err(zx::Status::NOT_FOUND),
        Some(p) => p,
    };

    let phy_req = req.alpha2;
    phy.proxy.set_country(&phy_req).await.map_phy_status(phy_id, "SetCountry")
}

async fn clear_country(
    phys: &PhyMap,
    req: fidl_svc::ClearCountryRequest,
) -> Result<(), zx::Status> {
    let phy = match phys.get(&req.phy_id) {
        None => return Err(zx::Status::NOT_FOUND),
        Some(p) => p,
    };

    phy.proxy.clear_country().await.map_phy_status(req.phy_id, "ClearCountry")
}

async fn power_down(phys: &PhyMap, phy_id: u16) -> Result<(), zx::Status> {
    let phy = match phys.get(&phy_id) {
        None => return Err(zx::Status::NOT_FOUND),
        Some(p) => p,
    };

    phy.proxy.power_down().await.map_phy_status(phy_id, "PowerDown")
}

async fn power_up(phys: &PhyMap, phy_id: u16) -> Result<(), zx::Status> {
    let phy = match phys.get(&phy_id) {
        None => return Err(zx::Status::NOT_FOUND),
        Some(p) => p,
    };
    phy.proxy.power_up().await.map_phy_status(phy_id, "PowerUp")
}

async fn reset(phys: &PhyMap, phy_id: u16) -> Result<(), zx::Status> {
    let phy = match phys.get(&phy_id) {
        None => return Err(zx::Status::NOT_FOUND),
        Some(p) => p,
    };
    phy.proxy.reset().await.map_phy_status(phy_id, "Reset")
}

fn get_power_element_dependency_token(
    phys: &PhyMap,
    phy_id: u16,
) -> Result<fidl_fuchsia_power_broker::DependencyToken, zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    if let Some(token) = phy.power_dependency_token.as_ref() {
        token.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(|_| zx::Status::INTERNAL)
    } else {
        Err(zx::Status::NOT_SUPPORTED)
    }
}

async fn get_power_state(phys: &PhyMap, phy_id: u16) -> Result<bool, zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    let resp = phy.proxy.get_power_state().await.map_phy_result(phy_id, "GetPowerState")?;
    resp.power_on.with_name("power_on").try_unpack().map_err(|_| zx::Status::INTERNAL)
}

async fn set_power_save_mode(
    phys: &PhyMap,
    req: fidl_svc::SetPowerSaveModeRequest,
) -> Result<(), zx::Status> {
    let phy_id = req.phy_id;
    let phy = match phys.get(&req.phy_id) {
        None => return Err(zx::Status::NOT_FOUND),
        Some(p) => p,
    };
    let phy_req = fidl_fuchsia_wlan_phy::WlanPhySetPowerSaveModeRequest {
        ps_mode: Some(req.ps_mode),
        ..Default::default()
    };
    phy.proxy.set_power_save_mode(&phy_req).await.map_phy_status(phy_id, "SetPowerSaveMode")
}

async fn get_power_save_mode(
    phys: &PhyMap,
    phy_id: u16,
) -> Result<fidl_svc::GetPowerSaveModeResponse, zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    let resp = phy.proxy.get_power_save_mode().await.map_phy_result(phy_id, "GetPowerSaveMode")?;
    let ps_mode =
        resp.ps_mode.with_name("ps_mode").try_unpack().map_err(|_| zx::Status::INTERNAL)?;
    Ok(fidl_svc::GetPowerSaveModeResponse { ps_mode })
}

async fn get_supported_mac_roles(
    phys: &PhyMap,
    id: u16,
) -> Result<Vec<fidl_common::WlanMacRole>, zx::sys::zx_status_t> {
    let phy = phys.get(&id).ok_or(zx::sys::ZX_ERR_NOT_FOUND)?;
    let resp = phy.proxy.get_supported_mac_roles().await.map_err(move |fidl_error| {
        error!("get_supported_mac_roles(id = {}): error sending 'GetSupportedMacRoles' request to phy: {}", id, fidl_error);
        zx::sys::ZX_ERR_INTERNAL
    })?.map_err(move |e| {
        let status = zx::Status::err_from_raw(e);
        error!("get_supported_mac_roles(id = {}): returned an error: {}", id, status);
        status.into_raw()
    })?;

    resp.supported_mac_roles
        .with_name("supported_mac_roles")
        .try_unpack()
        .map_err(|_| zx::sys::ZX_ERR_INTERNAL)
}
async fn set_bt_coexistence_mode(
    phys: &PhyMap,
    phy_id: u16,
    mode: fidl_internal::BtCoexistenceMode,
) -> Result<(), zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    let req = fidl_fuchsia_wlan_phy::WlanPhySetBtCoexistenceModeRequest {
        mode: Some(mode),
        ..Default::default()
    };
    phy.proxy.set_bt_coexistence_mode(&req).await.map_phy_result(phy_id, "SetBtCoexistenceMode")
}

async fn set_tx_power_scenario(
    phys: &PhyMap,
    phy_id: u16,
    scenario: &TxPowerScenario,
) -> Result<(), zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    let req = fidl_fuchsia_wlan_phy::WlanPhySetTxPowerScenarioRequest {
        scenario: Some(*scenario),
        ..Default::default()
    };
    phy.proxy.set_tx_power_scenario(&req).await.map_phy_result(phy_id, "SetTxPowerScenario")
}

async fn reset_tx_power_scenario(phys: &PhyMap, phy_id: u16) -> Result<(), zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    phy.proxy.reset_tx_power_scenario().await.map_phy_result(phy_id, "ResetTxPowerScenario")
}

async fn get_tx_power_scenario(phys: &PhyMap, phy_id: u16) -> Result<TxPowerScenario, zx::Status> {
    let phy = phys.get(&phy_id).ok_or(zx::Status::NOT_FOUND)?;
    phy.proxy.get_tx_power_scenario().await.map_phy_result(phy_id, "GetTxPowerScenario")
}

#[allow(clippy::too_many_arguments, reason = "mass allow for https://fxbug.dev/381896734")]
async fn create_iface(
    new_iface_sink: &mpsc::UnboundedSender<NewIface>,
    phys: &PhyMap,
    phy_id: u16,
    role: fidl_common::WlanMacRole,
    sta_address: MacAddr,
    iface_counter: &IfaceCounter,
    cfg: &wlandevicemonitor_config::Config,
) -> Result<(NewIface, zx::Vmo), Error> {
    let phy = phys.get(&phy_id).ok_or_else(|| format_err!("PHY not found: phy_id {}", phy_id))?;

    // Create the bootstrap channel. This channel is only used for initial communication
    // with the USME device.
    let (usme_bootstrap_client, usme_bootstrap_server) =
        create_endpoints::<fidl_sme::UsmeBootstrapMarker>();
    let usme_bootstrap_proxy = usme_bootstrap_client.into_proxy();

    // Create a GenericSme channel. This channel will be used for continued communication with
    // the USME driver and hence will be persisted.
    let (generic_sme_client, generic_sme_server) = create_endpoints::<fidl_sme::GenericSmeMarker>();
    let generic_sme_proxy = generic_sme_client.into_proxy();

    let legacy_privacy_support = fidl_sme::LegacyPrivacySupport {
        wep_supported: cfg.wep_supported,
        wpa1_supported: cfg.wpa1_supported,
    };

    // Pipeline the GenericSme channel and the legacy privacy support configuration, via
    // the bootstrap channel. This request will be handled after our call to
    // create_iface in the phy proxy, so we cannot await the result yet.
    let bootstrap_result = usme_bootstrap_proxy.start(generic_sme_server, &legacy_privacy_support);

    let phy_req = fidl_fuchsia_wlan_phy::WlanPhyCreateIfaceRequest {
        role: Some(role),
        mlme_channel: Some(usme_bootstrap_server.into_channel()),
        init_sta_addr: Some(sta_address.to_array()),
        ..Default::default()
    };
    let create_iface_resp = phy
        .proxy
        .create_iface(phy_req)
        .await
        .map_err(move |e| format_err!("CreateIface request failed with FIDL error {}", e))?
        .map_err(move |e| format_err!("CreateIface request failed with error {}", e))?;

    let phy_assigned_iface_id = create_iface_resp
        .iface_id
        .with_name("iface_id")
        .try_unpack()
        .map_err(move |_| format_err!("Missing iface_id in CreateIface response"))?;

    let inspect_vmo =
        bootstrap_result.await.map_err(|e| format_err!("Failed to bootstrap USME: {}", e))?;

    let iface_id = iface_counter.next_iface_id() as u16;

    let new_iface = NewIface {
        id: iface_id,
        phy_ownership: device::PhyOwnership { phy_id, phy_assigned_id: phy_assigned_iface_id },
        generic_sme: generic_sme_proxy,
    };
    new_iface_sink.unbounded_send(new_iface.clone()).map_err(|e| {
        format_err!("Failed to register Generic SME event stream with internal handler: {}", e)
    })?;

    let fidl_sme::GenericSmeQuery { role: iface_role, sta_addr: iface_sta_address, .. } = new_iface
        .generic_sme
        .query()
        .await
        .map_err(|e| format_err!("Failed to initially query iface: FIDL error {}", e))?;

    // TODO(https://fxbug.dev/382075991): Log diagnostic information if the initial query contains
    // incoherent results, compared to the request. However, don't fail iface creation because such
    // incoherent results have been allowed for some time.
    if iface_role != role {
        warn!("Created iface has unexpected role: expected {:?}, actual {:?}", role, iface_role);
    }
    if sta_address != NULL_ADDR && iface_sta_address != sta_address.to_array() {
        warn!(
            "Created iface has unexpected sta_address: expected {:?}, actual {:?}",
            sta_address.to_array(),
            iface_sta_address
        );
    }

    Ok((new_iface, inspect_vmo))
}

async fn query_iface(
    ifaces: &IfaceMap,
    id: u16,
) -> Result<fidl_svc::QueryIfaceResponse, zx::Status> {
    info!("query_iface(id = {})", id);
    let iface = ifaces.get(&id).ok_or(zx::Status::NOT_FOUND)?;
    let iface_query_info = iface.generic_sme.query().await.map_err(|_| zx::Status::CANCELED)?;
    let phy_ownership = &iface.phy_ownership;
    Ok(fidl_svc::QueryIfaceResponse {
        role: iface_query_info.role,
        id,
        phy_id: phy_ownership.phy_id,
        phy_assigned_id: phy_ownership.phy_assigned_id,
        sta_addr: iface_query_info.sta_addr,
        factory_addr: iface_query_info.factory_addr,
    })
}

async fn query_device_capability(
    ifaces: &IfaceMap,
    id: u16,
) -> Result<fidl_common::ApfPacketFilterSupport, zx::Status> {
    info!("query_device_capability(id = {})", id);
    let iface = ifaces.get(&id).ok_or(zx::Status::NOT_FOUND)?;
    let info =
        iface.generic_sme.query_iface_capabilities().await.map_err(|_| zx::Status::INTERNAL)?;
    info.map_err(zx::Status::err_from_raw)
}

async fn destroy_iface(
    phys: &PhyMap,
    iface_wrapper: &IfaceWrapper<'_>,
    id: u16,
) -> Result<(), zx::Status> {
    // There is a race between this function call returning and handle_single_new_iface realizing
    // that the interface has gone down.  If this function has not returned, then it is possible
    // that the driver has not yet fully removed the interface and handle_single_new_iface's call
    // of this function will alert watchers that the interface is gone.  Attempts to create a new
    // interface during this period will result in failure.
    //
    // Each call to this function should acquire the mutex to ensure that internal state remains
    // consistent through this critical section.
    static DESTROY_IFACE_LOCK: LazyLock<Arc<Mutex<()>>> =
        LazyLock::new(|| Arc::new(Mutex::new(())));
    let _guard = DESTROY_IFACE_LOCK.lock().await;

    info!("destroy_iface(id = {})", id);
    let iface = iface_wrapper.ifaces.get(&id).ok_or(zx::Status::NOT_FOUND)?;

    let (telemetry_proxy, telemetry_server) = fidl::endpoints::create_proxy();
    let result = get_sme_telemetry(iface_wrapper.ifaces, id, telemetry_server).await;
    let destroyed_iface_vmo = match result {
        Ok(()) => match telemetry_proxy.clone_inspect_vmo().await {
            Ok(Ok(vmo)) => Some(vmo),
            _ => {
                // We may reach this case if the driver crashes, which unfortunately means
                // that inspect data can no longer be recovered.
                error!("Failed to clone iface inspect VMO before destruction");
                None
            }
        },
        Err(e) => {
            // If the SME is already gone, we won't be able to get it here
            info!("Failed to get iface client sme before destruction: {}", e);
            None
        }
    };

    let phy_ownership = &iface.phy_ownership;
    let phy = match phys.get(&phy_ownership.phy_id) {
        Some(phy) => phy,
        None => {
            iface_wrapper.remove(id, destroyed_iface_vmo);
            error!(
                "Attempting to remove interface (ID: {}) from non-existent PHY (ID: {})",
                id, phy_ownership.phy_id
            );
            return Err(zx::Status::NOT_FOUND);
        }
    };
    let phy_req = fidl_fuchsia_wlan_phy::WlanPhyDestroyIfaceRequest {
        iface_id: Some(phy_ownership.phy_assigned_id),
        ..Default::default()
    };

    let destroy_iface_result = match phy.proxy.destroy_iface(&phy_req).await {
        Ok(result) => result,
        Err(fidl::Error::ClientChannelClosed { .. }) => {
            warn!(
                "Failed to send 'DestroyIface' request to phy {:?}: client channel closed. Assuming phy removed.",
                phy_ownership
            );
            Ok(())
        }
        Err(error) => {
            error!("Error sending 'DestroyIface' request to phy {:?}: {}", phy_ownership, error);
            return Err(zx::Status::INTERNAL);
        }
    };

    // If the removal is successful or the interface cannot be found, update the internal interface
    // accounting.
    match destroy_iface_result {
        Ok(()) => {
            iface_wrapper.remove(id, destroyed_iface_vmo);
            zx::Status::ok(zx::sys::ZX_OK)
        }
        Err(status) => {
            if status == zx::sys::ZX_ERR_NOT_FOUND
                && iface_wrapper.ifaces.get_snapshot().contains_key(&id)
            {
                info!(
                    "Encountered NOT_FOUND while removing iface #{}, potentially due to recovery.",
                    id
                );
                iface_wrapper.remove(id, destroyed_iface_vmo);
            }
            zx::Status::ok(status)
        }
    }
}

async fn get_client_sme(
    ifaces: &IfaceMap,
    id: u16,
    sme_server: fidl::endpoints::ServerEnd<fidl_sme::ClientSmeMarker>,
) -> Result<(), zx::Status> {
    info!("get_client_sme(id = {})", id);
    let iface = ifaces.get(&id).ok_or(zx::Status::NOT_FOUND)?;
    let result = iface.generic_sme.get_client_sme(sme_server).await.map_err(|e| {
        error!("Failed to request client SME: {}", e);
        zx::Status::INTERNAL
    })?;
    result.map_err(zx::Status::err_from_raw)
}

async fn get_ap_sme(
    ifaces: &IfaceMap,
    id: u16,
    sme_server: fidl::endpoints::ServerEnd<fidl_sme::ApSmeMarker>,
) -> Result<(), zx::Status> {
    info!("get_ap_sme(id = {})", id);
    let iface = ifaces.get(&id).ok_or(zx::Status::NOT_FOUND)?;
    let result = iface.generic_sme.get_ap_sme(sme_server).await.map_err(|e| {
        error!("Failed to request AP SME: {}", e);
        zx::Status::INTERNAL
    })?;
    result.map_err(zx::Status::err_from_raw)
}

async fn get_sme_telemetry(
    ifaces: &IfaceMap,
    id: u16,
    telemetry_server: fidl::endpoints::ServerEnd<fidl_sme::TelemetryMarker>,
) -> Result<(), zx::Status> {
    info!("get_sme_telemetry(id = {})", id);
    let iface = ifaces.get(&id).ok_or(zx::Status::NOT_FOUND)?;
    let result = iface.generic_sme.get_sme_telemetry(telemetry_server).await.map_err(|e| {
        info!("Failed to request SME telemetry: {}", e);
        zx::Status::INTERNAL
    })?;
    result.map_err(zx::Status::err_from_raw)
}

fn into_status_and_opt<T>(r: Result<T, zx::Status>) -> (Result<(), zx::Status>, Option<T>) {
    match r {
        Ok(x) => (Ok(()), Some(x)),
        Err(status) => (Err(status), None),
    }
}

trait PhyResultExt<T> {
    fn map_phy_result(self, phy_id: u16, context: &str) -> Result<T, zx::Status>;
    fn map_phy_status(self, phy_id: u16, context: &str) -> Result<(), zx::Status>
    where
        Self: Sized,
    {
        self.map_phy_result(phy_id, context).map(|_| ())
    }
}

impl<T> PhyResultExt<T> for Result<Result<T, zx::sys::zx_status_t>, fidl::Error> {
    fn map_phy_result(self, phy_id: u16, context: &str) -> Result<T, zx::Status> {
        match self {
            Ok(result) => result.map_err(zx::Status::err_from_raw),
            Err(e) => {
                error!("Error sending '{}' request to phy #{}: {}", context, phy_id, e);
                Err(zx::Status::INTERNAL)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::PhyOwnership;
    use crate::inspect::IfacesTree;
    use assert_matches::assert_matches;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream};
    use fidl_fuchsia_wlan_common as fidl_wlan_common;
    use fuchsia_async as fasync;
    use fuchsia_inspect::Inspector;
    use futures::TryStreamExt;
    use futures::future::BoxFuture;
    use futures::task::Poll;
    use ieee80211::NULL_ADDR;
    use std::convert::Infallible;
    use std::pin::pin;
    use std::sync::Arc;
    use test_case::test_case;

    const TEST_IFACE_ID: u16 = 42;

    struct TestValues {
        monitor_proxy: fidl_svc::DeviceMonitorProxy,
        monitor_stream: fidl_svc::DeviceMonitorRequestStream,
        phys: Arc<PhyMap>,
        ifaces: Arc<IfaceMap>,
        watcher_service: watcher_service::WatcherService<PhyDevice, IfaceDevice>,
        watcher_fut: BoxFuture<'static, Result<Infallible, Error>>,
        #[expect(unused)]
        phy_event_sink: mpsc::Sender<(u16, crate::device::PhyEvent)>,
        phy_event_service: phy_event_service::PhyEventService,
        #[expect(unused)]
        phy_event_fut: BoxFuture<'static, Result<Infallible, Error>>,
        new_iface_stream: mpsc::UnboundedReceiver<NewIface>,
        new_iface_sink: mpsc::UnboundedSender<NewIface>,
        iface_counter: Arc<IfaceCounter>,
        inspector: Inspector,
        ifaces_tree: Arc<IfacesTree>,
    }

    fn test_setup() -> TestValues {
        let (monitor_proxy, requests) = create_proxy::<fidl_svc::DeviceMonitorMarker>();
        let monitor_stream = requests.into_stream();
        let (phys, phy_events) = PhyMap::new();
        let phys = Arc::new(phys);

        let (ifaces, iface_events) = IfaceMap::new();
        let ifaces = Arc::new(ifaces);

        let (watcher_service, watcher_fut) =
            watcher_service::serve_watchers(phys.clone(), ifaces.clone(), phy_events, iface_events);

        let (phy_event_sink, phy_event_stream) = mpsc::channel(5);
        let (phy_event_service, phy_event_fut) =
            phy_event_service::serve_phy_events(phy_event_stream);

        let (new_iface_sink, new_iface_stream) = mpsc::unbounded();

        let iface_counter = Arc::new(IfaceCounter::new());

        let inspector = Inspector::default();
        let ifaces_tree = Arc::new(IfacesTree::new(inspector.clone()));

        TestValues {
            monitor_proxy,
            monitor_stream,
            phys,
            ifaces,
            watcher_service,
            watcher_fut: Box::pin(watcher_fut),
            phy_event_sink,
            phy_event_service,
            phy_event_fut: Box::pin(phy_event_fut),
            new_iface_stream,
            new_iface_sink,
            iface_counter,
            inspector,
            ifaces_tree,
        }
    }

    fn fake_phy_device() -> (PhyDevice, fidl_fuchsia_wlan_phy::WlanPhyRequestStream) {
        let (proxy, server) = create_proxy::<fidl_fuchsia_wlan_phy::WlanPhyMarker>();
        let stream = server.into_stream();
        (PhyDevice { proxy, power_dependency_token: None }, stream)
    }

    fn expect_get_supported_mac_roles(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        roles: &[fidl_wlan_common::WlanMacRole],
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::GetSupportedMacRoles { responder }))) => responder
        );
        let converted_roles: Vec<_> = roles
            .iter()
            .map(|&role| match role {
                fidl_wlan_common::WlanMacRole::Client => {
                    fidl_fuchsia_wlan_common::WlanMacRole::Client
                }
                fidl_wlan_common::WlanMacRole::Ap => fidl_fuchsia_wlan_common::WlanMacRole::Ap,
                fidl_wlan_common::WlanMacRole::Mesh => fidl_fuchsia_wlan_common::WlanMacRole::Mesh,
                _ => panic!("unknown mac role"),
            })
            .collect();
        let response = fidl_fuchsia_wlan_phy::WlanPhyGetSupportedMacRolesResponse {
            supported_mac_roles: Some(converted_roles),
            ..Default::default()
        };
        responder.send(Ok(&response)).expect("failed to send QueryResponse");
    }

    fn expect_create_iface(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        expected_role: fidl_wlan_common::WlanMacRole,
        expected_sta_addr: [u8; 6],
        result: Result<u16, zx::Status>,
    ) -> Option<zx::Channel> {
        let (payload, responder) = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::CreateIface { payload, responder }))) => (payload, responder)
        );
        assert_eq!(payload.role, Some(expected_role));
        assert_eq!(payload.init_sta_addr, Some(expected_sta_addr));
        let mlme = payload.mlme_channel;
        let response =
            result.as_ref().map(|&iface_id| fidl_fuchsia_wlan_phy::WlanPhyCreateIfaceResponse {
                iface_id: Some(iface_id),
                ..Default::default()
            });
        responder
            .send(response.as_ref().map_err(|s| s.into_raw()))
            .expect("failed to send response");
        mlme
    }

    fn expect_get_country(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<[u8; 2], zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::GetCountry { responder }))) => responder
        );
        responder.send(result.as_ref().map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_set_country(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        expected_alpha2: [u8; 2],
        result: Result<(), zx::Status>,
    ) {
        let (country, responder) = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::SetCountry { country, responder }))) => (country, responder)
        );
        assert_eq!(country, expected_alpha2);
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_clear_country(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<(), zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::ClearCountry { responder }))) => responder
        );
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_set_bt_coex_mode(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        expected_mode: fidl_internal::BtCoexistenceMode,
        result: Result<(), zx::Status>,
    ) {
        let (payload, responder) = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::SetBtCoexistenceMode { payload, responder }))) => (payload, responder)
        );
        assert_eq!(payload.mode, Some(expected_mode));
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_set_power_scenario(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        expected_scenario: TxPowerScenario,
        result: Result<(), zx::Status>,
    ) {
        let (payload, responder) = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::SetTxPowerScenario { payload, responder }))) => (payload, responder)
        );
        assert_eq!(payload.scenario, Some(expected_scenario));
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_reset_power_scenario(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<(), zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::ResetTxPowerScenario { responder }))) => responder
        );
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_get_power_scenario(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<TxPowerScenario, zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::GetTxPowerScenario { responder }))) => responder
        );
        let result_to_send = result.map_err(|s| s.into_raw());
        responder.send(result_to_send).expect("failed to send response");
    }

    fn expect_set_power_save_mode(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        expected_mode: fidl_wlan_common::PowerSaveType,
        result: Result<(), zx::Status>,
    ) {
        let (payload, responder) = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::SetPowerSaveMode { payload, responder }))) => (payload, responder)
        );
        assert_eq!(payload.ps_mode, Some(expected_mode));
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_get_power_save_mode(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<fidl_wlan_common::PowerSaveType, zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::GetPowerSaveMode { responder }))) => responder
        );
        let response = result.as_ref().map(|&ps_mode| {
            fidl_fuchsia_wlan_phy::WlanPhyGetPowerSaveModeResponse {
                ps_mode: Some(ps_mode),
                ..Default::default()
            }
        });
        responder
            .send(response.as_ref().map_err(|s| s.into_raw()))
            .expect("failed to send response");
    }

    fn expect_power_down(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<(), zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::PowerDown { responder }))) => responder
        );
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_power_up(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<(), zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::PowerUp { responder }))) => responder
        );
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_get_power_state(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<bool, zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::GetPowerState { responder }))) => responder
        );
        let response =
            result.as_ref().map(|&power_on| fidl_fuchsia_wlan_phy::WlanPhyGetPowerStateResponse {
                power_on: Some(power_on),
                ..Default::default()
            });
        responder
            .send(response.as_ref().map_err(|s| s.into_raw()))
            .expect("failed to send response");
    }

    fn expect_reset(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        result: Result<(), zx::Status>,
    ) {
        let responder = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::Reset { responder }))) => responder
        );
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn expect_destroy_iface(
        exec: &mut fasync::TestExecutor,
        stream: &mut fidl_fuchsia_wlan_phy::WlanPhyRequestStream,
        expected_iface_id: u16,
        result: Result<(), zx::Status>,
    ) {
        let (payload, responder) = assert_matches!(exec.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::DestroyIface { payload, responder }))) => (payload, responder)
        );
        assert_eq!(payload.iface_id, Some(expected_iface_id));
        responder.send(result.map_err(|s| s.into_raw())).expect("failed to send response");
    }

    fn fake_alpha2() -> [u8; 2] {
        let mut alpha2: [u8; 2] = [0, 0];
        alpha2.copy_from_slice("MX".as_bytes());
        alpha2
    }

    fn fake_wlandevicemonitor_config() -> wlandevicemonitor_config::Config {
        wlandevicemonitor_config::Config { wep_supported: false, wpa1_supported: false }
    }

    #[allow(clippy::too_many_arguments)]
    async fn serve_monitor_requests(
        mut monitor_stream: fidl_svc::DeviceMonitorRequestStream,
        phys: &PhyMap,
        ifaces: &IfaceMap,
        watcher_service: &watcher_service::WatcherService<PhyDevice, IfaceDevice>,
        phy_event_service: &phy_event_service::PhyEventService,
        new_iface_sink: &mpsc::UnboundedSender<NewIface>,
        iface_counter: &IfaceCounter,
        ifaces_tree: &IfacesTree,
        cfg: &wlandevicemonitor_config::Config,
    ) {
        while let Some(request) = monitor_stream.try_next().await.unwrap() {
            let iface_wrapper = IfaceWrapper::new(ifaces, ifaces_tree);
            handle_monitor_request(
                request,
                phys,
                &iface_wrapper,
                watcher_service,
                phy_event_service,
                new_iface_sink,
                iface_counter,
                cfg,
            )
            .await
            .unwrap()
        }
    }

    #[fuchsia::test]
    fn test_list_phys() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request the list of available PHYs.
        let list_fut = test_values.monitor_proxy.list_phys();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the PHYs should complete and no PHYs should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut),Poll::Ready(Ok(phys)) => {
            assert!(phys.is_empty())
        });

        // Add a PHY to the PhyMap.
        let (phy, _req_stream) = fake_phy_device();
        test_values.phys.insert(0, phy);

        // Request the list of available PHYs.
        let list_fut = test_values.monitor_proxy.list_phys();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the PHYs should complete and the PHY should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Ready(Ok(phys)) => {
            assert_eq!(vec![0u16], phys);
        });

        // Remove the PHY from the map.
        test_values.phys.remove(&0);

        // Request the list of available PHYs.
        let list_fut = test_values.monitor_proxy.list_phys();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the PHYs should complete and no PHYs should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Ready(Ok(phys)) => {
            assert!(phys.is_empty())
        });
    }

    #[fuchsia::test]
    fn test_list_ifaces() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request the list of available ifaces.
        let list_fut = test_values.monitor_proxy.list_ifaces();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the ifaces should complete and no ifaces should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut),Poll::Ready(Ok(ifaces)) => {
            assert!(ifaces.is_empty())
        });

        // Add a fake iface.
        let (generic_sme, _) = create_proxy::<fidl_sme::GenericSmeMarker>();
        let fake_iface = IfaceDevice {
            phy_ownership: PhyOwnership { phy_id: 0, phy_assigned_id: 0 },
            generic_sme,
        };
        test_values.ifaces.insert(0, fake_iface);

        // Request the list of available ifaces.
        let list_fut = test_values.monitor_proxy.list_ifaces();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the ifaces should complete and the iface should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Ready(Ok(ifaces)) => {
            assert_eq!(vec![0u16], ifaces);
        });
    }
    #[fuchsia::test]
    fn test_get_mac_roles_success() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        test_values.phys.insert(10u16, phy);

        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        let get_supported_mac_roles_fut = test_values.monitor_proxy.get_supported_mac_roles(10u16);
        let mut get_supported_mac_roles_fut = pin!(get_supported_mac_roles_fut);
        assert_matches!(exec.run_until_stalled(&mut get_supported_mac_roles_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        expect_get_supported_mac_roles(
            &mut exec,
            &mut phy_stream,
            &[fidl_wlan_common::WlanMacRole::Client],
        );

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        assert_matches!(
            exec.run_until_stalled(&mut get_supported_mac_roles_fut),
            Poll::Ready(Ok(Ok(supported_mac_roles))) => {
                assert_eq!(supported_mac_roles, vec![fidl_wlan_common::WlanMacRole::Client]);
            }
        );
    }
    #[fuchsia::test]
    fn test_get_mac_roles_phy_not_found() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Query a PHY's dev path.
        let get_supported_mac_roles_fut = test_values.monitor_proxy.get_supported_mac_roles(0);
        let mut get_supported_mac_roles_fut = pin!(get_supported_mac_roles_fut);
        assert_matches!(exec.run_until_stalled(&mut get_supported_mac_roles_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The attempt to query the PHY's information should fail.
        assert_matches!(
            exec.run_until_stalled(&mut get_supported_mac_roles_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NOT_FOUND)))
        );
    }

    #[fuchsia::test]
    fn test_watch_devices_add_remove_phy() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let watcher_fut = test_values.watcher_fut;
        let mut watcher_fut = pin!(watcher_fut);

        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Watch for new devices.
        let (watcher_proxy, watcher_server_end) = fidl::endpoints::create_proxy();
        test_values
            .monitor_proxy
            .watch_devices(watcher_server_end)
            .expect("failed to watch devices");

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Initially there should be no devices and the future should be pending.
        let mut events_fut = watcher_proxy.take_event_stream();
        let next_fut = events_fut.try_next();
        let mut next_fut = pin!(next_fut);
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Pending);

        // Add a PHY and make sure the update is received.
        let (phy, _phy_stream) = fake_phy_device();
        test_values.phys.insert(0, phy);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnPhyAdded { phy_id: 0 })))
        );

        // Remove the PHY and make sure the update is received.
        test_values.phys.remove(&0);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnPhyRemoved { phy_id: 0 })))
        );
    }

    #[fuchsia::test]
    fn test_watch_devices_remove_existing_phy() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let watcher_fut = test_values.watcher_fut;
        let mut watcher_fut = pin!(watcher_fut);

        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Add a PHY before beginning to watch for devices.
        let (phy, _phy_stream) = fake_phy_device();
        test_values.phys.insert(0, phy);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);

        // Watch for new devices.
        let (watcher_proxy, watcher_server_end) = fidl::endpoints::create_proxy();
        test_values
            .monitor_proxy
            .watch_devices(watcher_server_end)
            .expect("failed to watch devices");

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Start listening for device events.
        let mut events_fut = watcher_proxy.take_event_stream();
        let next_fut = events_fut.try_next();
        let mut next_fut = pin!(next_fut);
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Pending);

        // We should be notified of the existing PHY.
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnPhyAdded { phy_id: 0 })))
        );

        // Remove the PHY and make sure the update is received.
        test_values.phys.remove(&0);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnPhyRemoved { phy_id: 0 })))
        );
    }

    #[fuchsia::test]
    fn test_watch_devices_add_remove_iface() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let watcher_fut = test_values.watcher_fut;
        let mut watcher_fut = pin!(watcher_fut);

        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Watch for new devices.
        let (watcher_proxy, watcher_server_end) = fidl::endpoints::create_proxy();
        test_values
            .monitor_proxy
            .watch_devices(watcher_server_end)
            .expect("failed to watch devices");

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Initially there should be no devices and the future should be pending.
        let mut events_fut = watcher_proxy.take_event_stream();
        let next_fut = events_fut.try_next();
        let mut next_fut = pin!(next_fut);
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Pending);

        // Create a generic SME proxy but drop the server since we won't use it.
        let (generic_sme, _) = create_proxy::<fidl_sme::GenericSmeMarker>();
        // Add an interface and make sure the update is received.
        let fake_iface = IfaceDevice {
            phy_ownership: PhyOwnership { phy_id: 0, phy_assigned_id: 0 },
            generic_sme,
        };
        test_values.ifaces.insert(0, fake_iface);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnIfaceAdded { iface_id: 0 })))
        );

        // Remove the PHY and make sure the update is received.
        test_values.ifaces.remove(&0);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnIfaceRemoved { iface_id: 0 })))
        );
    }

    #[fuchsia::test]
    fn test_watch_devices_remove_existing_iface() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let watcher_fut = test_values.watcher_fut;
        let mut watcher_fut = pin!(watcher_fut);

        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Create a generic SME proxy but drop the server since we won't use it.
        let (generic_sme, _) = create_proxy::<fidl_sme::GenericSmeMarker>();
        // Add an interface before beginning to watch for devices.
        let fake_iface = IfaceDevice {
            phy_ownership: PhyOwnership { phy_id: 0, phy_assigned_id: 0 },
            generic_sme,
        };
        test_values.ifaces.insert(0, fake_iface);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);

        // Watch for new devices.
        let (watcher_proxy, watcher_server_end) = fidl::endpoints::create_proxy();
        test_values
            .monitor_proxy
            .watch_devices(watcher_server_end)
            .expect("failed to watch devices");

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Start listening for device events.
        let mut events_fut = watcher_proxy.take_event_stream();
        let next_fut = events_fut.try_next();
        let mut next_fut = pin!(next_fut);
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Pending);

        // We should be notified of the existing interface.
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnIfaceAdded { iface_id: 0 })))
        );

        // Remove the interface and make sure the update is received.
        test_values.ifaces.remove(&0);
        assert_matches!(exec.run_until_stalled(&mut watcher_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fidl_svc::DeviceWatcherEvent::OnIfaceRemoved { iface_id: 0 })))
        );
    }
    #[fuchsia::test]
    fn test_set_country_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let alpha2 = fake_alpha2();

        let req_msg = fidl_svc::SetCountryRequest { phy_id, alpha2 };
        let req_fut = super::set_country(&test_values.phys, req_msg);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_set_country(&mut exec, &mut phy_stream, alpha2, Ok(()));

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }
    #[fuchsia::test]
    fn test_set_country_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let alpha2 = fake_alpha2();

        let req_msg = fidl_svc::SetCountryRequest { phy_id, alpha2 };
        let req_fut = super::set_country(&test_values.phys, req_msg);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_set_country(&mut exec, &mut phy_stream, alpha2, Err(zx::Status::NOT_SUPPORTED));

        assert_eq!(
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED)),
            exec.run_until_stalled(&mut req_fut)
        );
    }
    #[fuchsia::test]
    fn test_get_country_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let alpha2 = fake_alpha2();

        let req_fut = super::get_country(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_get_country(&mut exec, &mut phy_stream, Ok(alpha2));

        assert_eq!(
            exec.run_until_stalled(&mut req_fut),
            Poll::Ready(Ok(fidl_svc::GetCountryResponse { alpha2 }))
        );
    }
    #[fuchsia::test]
    fn test_get_country_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_country(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_get_country(&mut exec, &mut phy_stream, Err(zx::Status::NOT_SUPPORTED));

        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(_)));
    }
    #[fuchsia::test]
    fn test_clear_country_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_msg = fidl_svc::ClearCountryRequest { phy_id };
        let req_fut = super::clear_country(&test_values.phys, req_msg);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_clear_country(&mut exec, &mut phy_stream, Ok(()));

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }
    #[fuchsia::test]
    fn test_clear_country_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_msg = fidl_svc::ClearCountryRequest { phy_id };
        let req_fut = super::clear_country(&test_values.phys, req_msg);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_clear_country(&mut exec, &mut phy_stream, Err(zx::Status::NOT_SUPPORTED));

        assert_eq!(
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED)),
            exec.run_until_stalled(&mut req_fut)
        );
    }
    #[fuchsia::test]
    fn test_set_power_save_mode_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_msg = fidl_svc::SetPowerSaveModeRequest {
            phy_id,
            ps_mode: fidl_wlan_common::PowerSaveType::PsModeBalanced,
        };
        let req_fut = super::set_power_save_mode(&test_values.phys, req_msg);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_set_power_save_mode(
            &mut exec,
            &mut phy_stream,
            fidl_wlan_common::PowerSaveType::PsModeBalanced,
            Ok(()),
        );

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }
    #[fuchsia::test]
    fn test_set_power_save_mode_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_msg = fidl_svc::SetPowerSaveModeRequest {
            phy_id,
            ps_mode: fidl_wlan_common::PowerSaveType::PsModeLowPower,
        };
        let req_fut = super::set_power_save_mode(&test_values.phys, req_msg);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_set_power_save_mode(
            &mut exec,
            &mut phy_stream,
            fidl_wlan_common::PowerSaveType::PsModeLowPower,
            Err(zx::Status::NOT_SUPPORTED),
        );

        assert_eq!(
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED)),
            exec.run_until_stalled(&mut req_fut)
        );
    }
    #[fuchsia::test]
    fn test_get_power_save_mode_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_power_save_mode(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_get_power_save_mode(
            &mut exec,
            &mut phy_stream,
            Ok(fidl_wlan_common::PowerSaveType::PsModePerformance),
        );

        assert_eq!(
            exec.run_until_stalled(&mut req_fut),
            Poll::Ready(Ok(fidl_svc::GetPowerSaveModeResponse {
                ps_mode: fidl_wlan_common::PowerSaveType::PsModePerformance
            }))
        );
    }
    #[fuchsia::test]
    fn test_get_power_save_mode_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_power_save_mode(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_get_power_save_mode(&mut exec, &mut phy_stream, Err(zx::Status::NOT_SUPPORTED));

        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(_)));
    }
    #[fuchsia::test]
    fn test_power_down_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::power_down(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_power_down(&mut exec, &mut phy_stream, Ok(()));

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }
    #[fuchsia::test]
    fn test_power_down_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::power_down(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_power_down(&mut exec, &mut phy_stream, Err(zx::Status::NOT_SUPPORTED));

        assert_eq!(
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED)),
            exec.run_until_stalled(&mut req_fut)
        );
    }
    #[fuchsia::test]
    fn test_power_down_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::power_down(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        // Drop stream to cause peer closed error
        drop(phy_stream);

        assert_eq!(Poll::Ready(Err(zx::Status::INTERNAL)), exec.run_until_stalled(&mut req_fut));
    }
    #[fuchsia::test]
    fn test_power_down_no_phy_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let _phy = fake_phy_device();
        let phy_id = 10u16;

        let req_fut = super::power_down(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Ready(Err(zx::Status::NOT_FOUND)), exec.run_until_stalled(&mut req_fut));
    }
    #[fuchsia::test]
    fn test_power_up_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::power_up(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_power_up(&mut exec, &mut phy_stream, Ok(()));

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }
    #[fuchsia::test]
    fn test_power_up_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::power_up(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_power_up(&mut exec, &mut phy_stream, Err(zx::Status::NOT_SUPPORTED));

        assert_eq!(
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED)),
            exec.run_until_stalled(&mut req_fut)
        );
    }
    #[fuchsia::test]
    fn test_power_up_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::power_up(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        // Drop stream to cause peer closed error
        drop(phy_stream);

        assert_eq!(Poll::Ready(Err(zx::Status::INTERNAL)), exec.run_until_stalled(&mut req_fut));
    }
    #[fuchsia::test]
    fn test_power_up_no_phy_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let _phy = fake_phy_device();
        let phy_id = 10u16;

        let req_fut = super::power_up(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Ready(Err(zx::Status::NOT_FOUND)), exec.run_until_stalled(&mut req_fut));
    }
    #[fuchsia::test]
    fn test_reset_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::reset(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_reset(&mut exec, &mut phy_stream, Ok(()));

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }
    #[fuchsia::test]
    fn test_reset_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::reset(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_reset(&mut exec, &mut phy_stream, Err(zx::Status::NOT_SUPPORTED));

        assert_eq!(
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED)),
            exec.run_until_stalled(&mut req_fut)
        );
    }
    #[fuchsia::test]
    fn test_reset_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::reset(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        // Drop stream to cause peer closed error
        drop(phy_stream);

        assert_eq!(Poll::Ready(Err(zx::Status::INTERNAL)), exec.run_until_stalled(&mut req_fut));
    }
    #[fuchsia::test]
    fn test_reset_no_phy_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let _phy = fake_phy_device();
        let phy_id = 10u16;

        let req_fut = super::reset(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Ready(Err(zx::Status::NOT_FOUND)), exec.run_until_stalled(&mut req_fut));
    }
    #[fuchsia::test]
    fn test_get_power_state_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_power_state(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_get_power_state(&mut exec, &mut phy_stream, Ok(true));

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(true)));
    }
    #[fuchsia::test]
    fn test_get_power_state_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_power_state(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_get_power_state(&mut exec, &mut phy_stream, Err(zx::Status::NOT_SUPPORTED));

        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(_)));
    }
    #[fuchsia::test]
    fn test_get_power_state_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_power_state(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        // Drop stream to cause peer closed error
        drop(phy_stream);

        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(_)));
    }
    #[fuchsia::test]
    fn test_get_power_state_no_phy_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let _phy = fake_phy_device();
        let phy_id = 10u16;

        let req_fut = super::get_power_state(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_matches!(
            exec.run_until_stalled(&mut req_fut),
            Poll::Ready(Err(zx::Status::NOT_FOUND))
        );
    }
    #[fuchsia::test]
    fn test_set_bt_coexistence_mode() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::set_bt_coexistence_mode(
            &test_values.phys,
            phy_id,
            fidl_internal::BtCoexistenceMode::ModeAuto,
        );
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_set_bt_coex_mode(
            &mut exec,
            &mut phy_stream,
            fidl_internal::BtCoexistenceMode::ModeAuto,
            Ok(()),
        );

        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_set_tx_power_scenario_nonexistent_phy_id() {
        // Setup environment
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, _phy_stream) = fake_phy_device();

        test_values.phys.insert(0, phy);

        // Set the power state for a PHY ID that is not present.
        let req_fut =
            super::set_tx_power_scenario(&test_values.phys, 9999, &TxPowerScenario::HeadCellOff);
        let mut req_fut = pin!(req_fut);

        // Verify that a NOT_FOUND error is returned.
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(zx::Status::NOT_FOUND)));
    }

    #[fuchsia::test]
    fn test_set_tx_power_scenario_request_fails() {
        // Setup environment
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        // Drop the phy stream so that the request fails.
        drop(phy_stream);
        let req_fut =
            super::set_tx_power_scenario(&test_values.phys, phy_id, &TxPowerScenario::HeadCellOff);
        let mut req_fut = pin!(req_fut);

        // Verify that an INTERNAL error is returned.
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(zx::Status::INTERNAL)));
    }
    #[fuchsia::test]
    fn test_set_tx_power_scenario_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);
        let test_scenario = TxPowerScenario::HeadCellOff;

        let req_fut = super::set_tx_power_scenario(&test_values.phys, phy_id, &test_scenario);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        expect_set_power_scenario(
            &mut exec,
            &mut phy_stream,
            test_scenario,
            Err(zx::Status::NOT_SUPPORTED),
        );

        assert_eq!(
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED)),
            exec.run_until_stalled(&mut req_fut)
        );
    }
    #[fuchsia::test]
    fn test_set_tx_power_scenario_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);
        let test_scenario = TxPowerScenario::HeadCellOff;

        let req_fut = super::set_tx_power_scenario(&test_values.phys, phy_id, &test_scenario);
        let mut req_fut = pin!(req_fut);
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Pending);

        expect_set_power_scenario(&mut exec, &mut phy_stream, test_scenario, Ok(()));

        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_set_nonexistent_phy() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, _phy_stream) = fake_phy_device();
        test_values.phys.insert(0, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario set for a bogus PHY ID
        let test_scenario = TxPowerScenario::BodyCellOff;
        let set_fut = test_values.monitor_proxy.set_tx_power_scenario(9999, test_scenario);
        let mut set_fut = pin!(set_fut);
        assert_matches!(exec.run_until_stalled(&mut set_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that a not-found error is returned
        assert_matches!(
            exec.run_until_stalled(&mut set_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NOT_FOUND)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_set_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        // Drop the PHY request stream so that all requests fail.
        drop(phy_stream);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario set for a bogus PHY ID
        let test_scenario = TxPowerScenario::BodyCellOff;
        let set_fut = test_values.monitor_proxy.set_tx_power_scenario(phy_id, test_scenario);
        let mut set_fut = pin!(set_fut);
        assert_matches!(exec.run_until_stalled(&mut set_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that an internal error is returned
        assert_matches!(
            exec.run_until_stalled(&mut set_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_INTERNAL)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_set_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario be set
        let test_scenario = TxPowerScenario::BodyCellOff;
        let set_fut = test_values.monitor_proxy.set_tx_power_scenario(phy_id, test_scenario);
        let mut set_fut = pin!(set_fut);
        assert_matches!(exec.run_until_stalled(&mut set_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        expect_set_power_scenario(
            &mut exec,
            &mut phy_stream,
            test_scenario,
            Err(zx::Status::NO_MEMORY),
        );

        // Allow the service to process the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that the response is returned to the caller.
        assert_matches!(
            exec.run_until_stalled(&mut set_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NO_MEMORY)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_set_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario be set
        let test_scenario = TxPowerScenario::BodyCellOff;
        let set_fut = test_values.monitor_proxy.set_tx_power_scenario(phy_id, test_scenario);
        let mut set_fut = pin!(set_fut);
        assert_matches!(exec.run_until_stalled(&mut set_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        expect_set_power_scenario(&mut exec, &mut phy_stream, test_scenario, Ok(()));

        // Allow the service to process the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that the response is returned to the caller.
        assert_matches!(exec.run_until_stalled(&mut set_fut), Poll::Ready(Ok(Ok(()))));
    }

    #[fuchsia::test]
    fn test_get_tx_power_scenario_nonexistent_phy_id() {
        // Setup environment
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, _phy_stream) = fake_phy_device();

        test_values.phys.insert(0, phy);

        // Get the power state for a PHY ID that is not present.
        let req_fut = super::get_tx_power_scenario(&test_values.phys, 9999);
        let mut req_fut = pin!(req_fut);

        // Verify that a NOT_FOUND error is returned.
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(zx::Status::NOT_FOUND)));
    }

    #[fuchsia::test]
    fn test_get_tx_power_scenario_request_fails() {
        // Setup environment
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, phy_stream) = fake_phy_device();

        // Drop the request stream so that the request fails.
        drop(phy_stream);

        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        // Get the power state for a PHY ID that is not present.
        let req_fut = super::get_tx_power_scenario(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);

        // The future should complete immediately with an internal error.
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(zx::Status::INTERNAL)));
    }
    #[fuchsia::test]
    fn test_get_tx_power_scenario_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_tx_power_scenario(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Pending);

        expect_get_power_scenario(&mut exec, &mut phy_stream, Err(zx::Status::NO_MEMORY));

        assert_matches!(
            exec.run_until_stalled(&mut req_fut),
            Poll::Ready(Err(zx::Status::NO_MEMORY))
        );
    }
    #[fuchsia::test]
    fn test_get_tx_power_scenario_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::get_tx_power_scenario(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Pending);

        expect_get_power_scenario(&mut exec, &mut phy_stream, Ok(TxPowerScenario::HeadCellOff));

        assert_matches!(
            exec.run_until_stalled(&mut req_fut),
            Poll::Ready(Ok(TxPowerScenario::HeadCellOff))
        );
    }
    #[fuchsia::test]
    fn test_service_tx_power_scenario_get_nonexistent_phy() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, _phy_stream) = fake_phy_device();
        test_values.phys.insert(0, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Get Tx power scenario reset for a bogus PHY ID
        let reset_fut = test_values.monitor_proxy.get_tx_power_scenario(9999);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that a not-found error is returned
        assert_matches!(
            exec.run_until_stalled(&mut reset_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NOT_FOUND)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_get_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        // Drop the PHY request stream so that all requests fail.
        drop(phy_stream);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Get Tx power scenario reset for a bogus PHY ID
        let reset_fut = test_values.monitor_proxy.get_tx_power_scenario(phy_id);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that an internal error is returned
        assert_matches!(
            exec.run_until_stalled(&mut reset_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_INTERNAL)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_get_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request the Tx power scenario
        let reset_fut = test_values.monitor_proxy.get_tx_power_scenario(phy_id);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        expect_get_power_scenario(&mut exec, &mut phy_stream, Err(zx::Status::NO_MEMORY));

        // Allow the service to process the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that the response is returned to the caller.
        assert_matches!(
            exec.run_until_stalled(&mut reset_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NO_MEMORY)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_get_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario reset
        let reset_fut = test_values.monitor_proxy.get_tx_power_scenario(phy_id);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        let scenario = TxPowerScenario::BodyBtActive;
        expect_get_power_scenario(&mut exec, &mut phy_stream, Ok(scenario));

        // Allow the service to process the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that the response is returned to the caller.
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Ready(Ok(Ok(response))) => {
            assert_eq!(response, scenario);
        });
    }

    #[fuchsia::test]
    fn test_reset_tx_power_scenario_nonexistent_phy_id() {
        // Setup environment
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, _phy_stream) = fake_phy_device();

        test_values.phys.insert(0, phy);

        // Reset the power state for a PHY ID that is not present.
        let req_fut = super::reset_tx_power_scenario(&test_values.phys, 9999);
        let mut req_fut = pin!(req_fut);

        // Verify that a NOT_FOUND error is returned.
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(zx::Status::NOT_FOUND)));
    }

    #[fuchsia::test]
    fn test_reset_tx_power_scenario_request_fails() {
        // Setup environment
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        // Drop the PHY request stream so that the request will fail.
        drop(phy_stream);

        // Reset the power state for a valid PHY ID.
        let req_fut = super::reset_tx_power_scenario(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);

        // Verify that there is an internal error
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(zx::Status::INTERNAL)));
    }
    #[fuchsia::test]
    fn test_reset_tx_power_scenario_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::reset_tx_power_scenario(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Pending);

        expect_reset_power_scenario(&mut exec, &mut phy_stream, Err(zx::Status::NO_MEMORY));

        assert_matches!(
            exec.run_until_stalled(&mut req_fut),
            Poll::Ready(Err(zx::Status::NO_MEMORY))
        );
    }
    #[fuchsia::test]
    fn test_reset_tx_power_scenario_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let req_fut = super::reset_tx_power_scenario(&test_values.phys, phy_id);
        let mut req_fut = pin!(req_fut);
        assert_eq!(exec.run_until_stalled(&mut req_fut), Poll::Pending);

        expect_reset_power_scenario(&mut exec, &mut phy_stream, Ok(()));

        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_reset_nonexistent_phy() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, _phy_stream) = fake_phy_device();
        test_values.phys.insert(0, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario reset for a bogus PHY ID
        let reset_fut = test_values.monitor_proxy.reset_tx_power_scenario(9999);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that a not-found error is returned
        assert_matches!(
            exec.run_until_stalled(&mut reset_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NOT_FOUND)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_reset_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        // Drop the PHY request stream so that all requests fail.
        drop(phy_stream);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario reset for a bogus PHY ID
        let reset_fut = test_values.monitor_proxy.reset_tx_power_scenario(phy_id);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that an internal error is returned
        assert_matches!(
            exec.run_until_stalled(&mut reset_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_INTERNAL)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_reset_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario reset
        let reset_fut = test_values.monitor_proxy.reset_tx_power_scenario(phy_id);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        expect_reset_power_scenario(&mut exec, &mut phy_stream, Err(zx::Status::NO_MEMORY));

        // Allow the service to process the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that the response is returned to the caller.
        assert_matches!(
            exec.run_until_stalled(&mut reset_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NO_MEMORY)))
        );
    }

    #[fuchsia::test]
    fn test_service_tx_power_scenario_reset_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id: u16 = 0;
        test_values.phys.insert(phy_id, phy);

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request Tx power scenario reset
        let reset_fut = test_values.monitor_proxy.reset_tx_power_scenario(phy_id);
        let mut reset_fut = pin!(reset_fut);
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Pending);

        // Advance the service fut to handle the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        expect_reset_power_scenario(&mut exec, &mut phy_stream, Ok(()));

        // Allow the service to process the request.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Verify that the response is returned to the caller.
        assert_matches!(exec.run_until_stalled(&mut reset_fut), Poll::Ready(Ok(Ok(()))));
    }

    #[fuchsia::test]
    fn test_iface_counter() {
        let iface_counter = IfaceCounter::new();
        assert_eq!(0, iface_counter.next_iface_id());
        assert_eq!(1, iface_counter.next_iface_id());
        assert_eq!(2, iface_counter.next_iface_id());
        assert_eq!(3, iface_counter.next_iface_id());
    }
    #[test_case([0, 1, 2, 3, 4, 5]; "New PHY - MAC 00:01:02:03:04:05")]
    #[test_case(NULL_ADDR.to_array(); "New PHY - MAC 00:00:00:00:00:00")]
    fn create_iface_succeeds(sta_address: [u8; 6]) {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        let list_fut = test_values.monitor_proxy.list_ifaces();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Ready(Ok(ifaces)) => {
            assert!(ifaces.is_empty())
        });

        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10;
        test_values.phys.insert(phy_id, phy);

        let create_iface_fut =
            test_values.monitor_proxy.create_iface(&fidl_svc::DeviceMonitorCreateIfaceRequest {
                phy_id: Some(phy_id),
                role: Some(fidl_wlan_common::WlanMacRole::Client),
                sta_address: Some(sta_address),
                ..Default::default()
            });
        let mut create_iface_fut = pin!(create_iface_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut create_iface_fut), Poll::Pending);

        let bootstrap_channel = expect_create_iface(
            &mut exec,
            &mut phy_stream,
            fidl_wlan_common::WlanMacRole::Client,
            sta_address,
            Ok(123),
        )
        .expect("no MLME channel");

        // Complete the USME bootstrap.
        let mut bootstrap_stream =
            fidl::endpoints::ServerEnd::<fidl_sme::UsmeBootstrapMarker>::new(bootstrap_channel)
                .into_stream();
        let mut generic_sme_stream = assert_matches!(
            exec.run_until_stalled(&mut bootstrap_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::UsmeBootstrapRequest::Start {
                responder, generic_sme_server, ..
            }))) => {
                responder
                    .send(test_values.inspector.duplicate_vmo().unwrap())
                    .expect("Failed to send bootstrap result");
                generic_sme_server.into_stream()
            }
        );

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut generic_sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::GenericSmeRequest::Query { responder }))) => {
                responder
                    .send(&fidl_sme::GenericSmeQuery {
                        role: fidl_wlan_common::WlanMacRole::Client,
                        sta_addr: sta_address,
                        factory_addr: NULL_ADDR.to_array(),
                    })
                    .expect("Failed to send query result");
            }
        );

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut create_iface_fut), Poll::Ready(Ok(Ok(resp))) => {
            assert_eq!(resp.iface_id, Some(0));
        });

        // Request the list of available ifaces.
        let list_fut = test_values.monitor_proxy.list_ifaces();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the ifaces should complete and the iface should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Ready(Ok(mut ifaces)) => {
            ifaces.sort();
            assert_eq!(vec![0], ifaces);
        });
    }
    #[fuchsia::test]
    fn create_iface_fails_on_error_from_phy() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10;
        test_values.phys.insert(phy_id, phy);

        let create_iface_fut =
            test_values.monitor_proxy.create_iface(&fidl_svc::DeviceMonitorCreateIfaceRequest {
                phy_id: Some(phy_id),
                role: Some(fidl_wlan_common::WlanMacRole::Client),
                sta_address: Some([0, 1, 2, 3, 4, 5]),
                ..Default::default()
            });
        let mut create_iface_fut = pin!(create_iface_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut create_iface_fut), Poll::Pending);

        expect_create_iface(
            &mut exec,
            &mut phy_stream,
            fidl_wlan_common::WlanMacRole::Client,
            [0, 1, 2, 3, 4, 5],
            Err(zx::Status::NO_RESOURCES),
        );

        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut create_iface_fut),
            Poll::Ready(Ok(Err(e))) => assert_eq!(e, fidl_svc::DeviceMonitorError::unknown())
        );
    }
    #[fuchsia::test]
    fn create_iface_fails_when_fields_are_missing_from_request() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request the list of available ifaces.
        let list_fut = test_values.monitor_proxy.list_ifaces();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the ifaces should complete and no ifaces should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut),Poll::Ready(Ok(ifaces)) => {
            assert!(ifaces.is_empty())
        });

        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10;
        test_values.phys.insert(phy_id, phy);

        // Iterate through create iface requests that are each missing one field and verify an
        // error is returned for each request.
        for request in [
            fidl_svc::DeviceMonitorCreateIfaceRequest {
                phy_id: None,
                role: Some(fidl_wlan_common::WlanMacRole::Client),
                sta_address: Some([0, 1, 2, 3, 4, 5]),
                ..Default::default()
            },
            fidl_svc::DeviceMonitorCreateIfaceRequest {
                phy_id: Some(phy_id),
                role: None,
                sta_address: Some([0, 1, 2, 3, 4, 5]),
                ..Default::default()
            },
            fidl_svc::DeviceMonitorCreateIfaceRequest {
                phy_id: Some(phy_id),
                role: Some(fidl_wlan_common::WlanMacRole::Client),
                sta_address: None,
                ..Default::default()
            },
        ] {
            let create_iface_fut = test_values.monitor_proxy.create_iface(&request);
            let mut create_iface_fut = pin!(create_iface_fut);
            assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

            assert_matches!(
                exec.run_until_stalled(&mut create_iface_fut),
                Poll::Ready(Ok(Err(e))) => assert_eq!(e, fidl_svc::DeviceMonitorError::unknown())
            );
            assert_matches!(exec.run_until_stalled(&mut phy_stream.next()), Poll::Pending);
        }
    }

    #[fuchsia::test]
    fn test_create_multiple_ifaces() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();
        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request the list of available ifaces.
        let list_fut = test_values.monitor_proxy.list_ifaces();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the ifaces should complete and no ifaces should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut),Poll::Ready(Ok(ifaces)) => {
            assert!(ifaces.is_empty())
        });

        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10;
        test_values.phys.insert(phy_id, phy);

        for (sta_address, phy_assigned_iface_id, sme_assigned_iface_id) in
            [([0x0, 0x1, 0x2, 0x3, 0x4, 0x5], 123, 0), ([0x6, 0x7, 0x8, 0x9, 0xa, 0xb], 0x123, 1)]
        {
            let create_iface_fut = test_values.monitor_proxy.create_iface(
                &fidl_svc::DeviceMonitorCreateIfaceRequest {
                    phy_id: Some(phy_id),
                    role: Some(fidl_wlan_common::WlanMacRole::Client),
                    sta_address: Some(sta_address),
                    ..Default::default()
                },
            );
            let mut create_iface_fut = pin!(create_iface_fut);
            assert_matches!(exec.run_until_stalled(&mut create_iface_fut), Poll::Pending);
            assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

            // Validate the PHY request
            let bootstrap_channel = assert_matches!(exec.run_until_stalled(&mut phy_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::CreateIface { payload, responder }))) => {
                assert_eq!(Some(fidl_wlan_common::WlanMacRole::Client), payload.role);
                assert_eq!(Some(sta_address), payload.init_sta_addr);
                responder.send(Ok(&fidl_fuchsia_wlan_phy::WlanPhyCreateIfaceResponse {
                    iface_id: Some(phy_assigned_iface_id),
                    ..Default::default()
                })).expect("failed to send iface id");
                payload.mlme_channel.expect("no MLME channel")
            });

            // Complete the USME bootstrap.
            let mut bootstrap_stream =
                fidl::endpoints::ServerEnd::<fidl_sme::UsmeBootstrapMarker>::new(bootstrap_channel)
                    .into_stream();
            let mut generic_sme_stream = assert_matches!(
            exec.run_until_stalled(&mut bootstrap_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::UsmeBootstrapRequest::Start {
                responder, generic_sme_server, ..
            }))) => {
                responder
                .send(test_values.inspector.duplicate_vmo().unwrap())
                .expect("Failed to send bootstrap result");
                generic_sme_server.into_stream()
            });
            // The original future should resolve into a response.
            assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);
            assert_matches!(
            exec.run_until_stalled(&mut generic_sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::GenericSmeRequest::Query {
                responder
            }))) => {
                responder
                .send(&fidl_sme::GenericSmeQuery {
                    role: fidl_common::WlanMacRole::Client,
                    sta_addr: sta_address,
                    factory_addr: sta_address,
                })
                .expect("Failed to send GenericSme.Query response");
            });
            assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

            assert_matches!(exec.run_until_stalled(&mut create_iface_fut),
            Poll::Ready(Ok(Ok(response))) => {
                assert_eq!(Some(sme_assigned_iface_id), response.iface_id);
            });

            // The new interface should be pushed into the new iface stream.
            assert_matches!(
            exec.run_until_stalled(&mut test_values.new_iface_stream.next()),
                    Poll::Ready(Some(NewIface {
                        id,
                        phy_ownership,
                        ..
                    })) => {
                        assert_eq!(sme_assigned_iface_id, id);
                        assert_eq!(phy_assigned_iface_id, phy_ownership.phy_assigned_id);
                        assert_eq!(phy_id, phy_ownership.phy_id);
                    });
        }

        // Request the list of available ifaces.
        let list_fut = test_values.monitor_proxy.list_ifaces();
        let mut list_fut = pin!(list_fut);
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Pending);

        // Progress the service loop.
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // The future to list the ifaces should complete and the iface should be present.
        assert_matches!(exec.run_until_stalled(&mut list_fut), Poll::Ready(Ok(mut ifaces)) => {
            ifaces.sort();
            assert_eq!(vec![0, 1], ifaces);
        });
    }

    #[fuchsia::test]
    fn create_iface_on_invalid_phy_id() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let iface_counter = Arc::new(IfaceCounter::new());

        let fut = super::create_iface(
            &test_values.new_iface_sink,
            &test_values.phys,
            10,
            fidl_wlan_common::WlanMacRole::Client,
            NULL_ADDR,
            &iface_counter,
            &wlandevicemonitor_config::Config { wep_supported: true, wpa1_supported: true },
        );
        let mut fut = pin!(fut);
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Err(_)),
            "expected failure on invalid PHY"
        );
    }
    fn fake_destroy_iface_env(
        phy_map: &PhyMap,
        iface_map: &IfaceMap,
        ifaces_tree: &crate::inspect::IfacesTree,
    ) -> fidl_fuchsia_wlan_phy::WlanPhyRequestStream {
        let (phy, stream) = fake_phy_device();
        phy_map.insert(10, phy);
        let (proxy, _) = create_proxy::<fidl_sme::GenericSmeMarker>();
        iface_map.insert(
            TEST_IFACE_ID,
            device::IfaceDevice {
                phy_ownership: PhyOwnership { phy_id: 10, phy_assigned_id: 0 },
                generic_sme: proxy,
            },
        );
        ifaces_tree.add_iface(TEST_IFACE_ID, zx::Vmo::create(0).unwrap());
        stream
    }
    #[fuchsia::test]
    fn destroy_iface_success() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut phy_stream = fake_destroy_iface_env(
            &test_values.phys,
            &test_values.ifaces,
            &test_values.ifaces_tree,
        );

        let iface_wrapper = IfaceWrapper::new(&test_values.ifaces, &test_values.ifaces_tree);
        let destroy_fut = super::destroy_iface(&test_values.phys, &iface_wrapper, TEST_IFACE_ID);
        let mut destroy_fut = pin!(destroy_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut destroy_fut));

        expect_destroy_iface(&mut exec, &mut phy_stream, 0, Ok(()));

        assert_eq!(exec.run_until_stalled(&mut destroy_fut), Poll::Ready(Ok(())));
        assert!(test_values.ifaces.get(&TEST_IFACE_ID).is_none());
        assert!(!test_values.ifaces_tree.has_iface(TEST_IFACE_ID));
    }
    #[fuchsia::test]
    fn destroy_iface_failure() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut phy_stream = fake_destroy_iface_env(
            &test_values.phys,
            &test_values.ifaces,
            &test_values.ifaces_tree,
        );

        let iface_wrapper = IfaceWrapper::new(&test_values.ifaces, &test_values.ifaces_tree);
        let destroy_fut = super::destroy_iface(&test_values.phys, &iface_wrapper, TEST_IFACE_ID);
        let mut destroy_fut = pin!(destroy_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut destroy_fut));

        expect_destroy_iface(&mut exec, &mut phy_stream, 0, Err(zx::Status::NOT_SUPPORTED));

        assert_eq!(
            exec.run_until_stalled(&mut destroy_fut),
            Poll::Ready(Err(zx::Status::NOT_SUPPORTED))
        );
        assert!(test_values.ifaces.get(&TEST_IFACE_ID).is_some());
        assert!(test_values.ifaces_tree.has_iface(TEST_IFACE_ID));
    }
    #[fuchsia::test]
    fn destroy_iface_recovery() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut phy_stream = fake_destroy_iface_env(
            &test_values.phys,
            &test_values.ifaces,
            &test_values.ifaces_tree,
        );

        let iface_wrapper = IfaceWrapper::new(&test_values.ifaces, &test_values.ifaces_tree);
        let destroy_fut = super::destroy_iface(&test_values.phys, &iface_wrapper, TEST_IFACE_ID);
        let mut destroy_fut = pin!(destroy_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut destroy_fut));

        // In the recovery scenario, the interface will have already been destroyed and the PHY
        // will have no record of it and will reply to the destruction request with a
        // ZX_ERR_NOT_FOUND. In this case, we should verify that the internal accounting is still
        // updated.
        expect_destroy_iface(&mut exec, &mut phy_stream, 0, Err(zx::Status::NOT_FOUND));

        assert_eq!(
            exec.run_until_stalled(&mut destroy_fut),
            Poll::Ready(Err(zx::Status::NOT_FOUND))
        );
        assert!(
            test_values.ifaces.get(&TEST_IFACE_ID).is_none(),
            "iface should have been removed."
        );
        assert!(!test_values.ifaces_tree.has_iface(TEST_IFACE_ID));
    }
    #[fuchsia::test]
    fn destroy_iface_not_found() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let _phy_stream = fake_destroy_iface_env(
            &test_values.phys,
            &test_values.ifaces,
            &test_values.ifaces_tree,
        );

        let iface_wrapper = IfaceWrapper::new(&test_values.ifaces, &test_values.ifaces_tree);
        let fut = super::destroy_iface(&test_values.phys, &iface_wrapper, 43);
        let mut fut = pin!(fut);
        assert_eq!(Poll::Ready(Err(zx::Status::NOT_FOUND)), exec.run_until_stalled(&mut fut));
    }
    #[fuchsia::test]
    fn destroy_iface_phy_not_found() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();

        // Set the PHY ID of the interface to be some non-existent PHY ID.
        let (proxy, _) = create_proxy::<fidl_sme::GenericSmeMarker>();
        test_values.ifaces.insert(
            1,
            device::IfaceDevice {
                phy_ownership: PhyOwnership { phy_id: 0, phy_assigned_id: 0 },
                generic_sme: proxy,
            },
        );

        // Destroy the interface that does not have a parent PHY ID.
        let iface_wrapper = IfaceWrapper::new(&test_values.ifaces, &test_values.ifaces_tree);
        let fut = super::destroy_iface(&test_values.phys, &iface_wrapper, 1);
        let mut fut = pin!(fut);
        assert_eq!(Poll::Ready(Err(zx::Status::NOT_FOUND)), exec.run_until_stalled(&mut fut));

        assert!(test_values.ifaces.get(&1).is_none());
    }
    #[fuchsia::test]
    fn phy_removed_during_destroy_iface() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let mut phy_stream = fake_destroy_iface_env(
            &test_values.phys,
            &test_values.ifaces,
            &test_values.ifaces_tree,
        );

        let iface_wrapper = IfaceWrapper::new(&test_values.ifaces, &test_values.ifaces_tree);
        let destroy_fut = super::destroy_iface(&test_values.phys, &iface_wrapper, TEST_IFACE_ID);
        let mut destroy_fut = pin!(destroy_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut destroy_fut));

        {
            let (_payload, responder) = assert_matches!(exec.run_until_stalled(&mut phy_stream.next()),
                Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::DestroyIface { payload, responder }))) => (payload, responder)
            );
            drop(responder);
        }
        drop(phy_stream);
        assert_eq!(Poll::Ready(Ok(())), exec.run_until_stalled(&mut destroy_fut));
        assert!(test_values.ifaces.get(&TEST_IFACE_ID).is_none(), "iface expected to be deleted");
    }
    #[fuchsia::test]
    fn destroy_iface_called_twice() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (iface_map, mut iface_events) = IfaceMap::new();
        let mut phy_stream =
            fake_destroy_iface_env(&test_values.phys, &iface_map, &test_values.ifaces_tree);

        if let Ok(Some(crate::watchable_map::MapEvent::KeyInserted(iface_id))) =
            iface_events.try_next()
        {
            assert_eq!(iface_id, TEST_IFACE_ID);
        } else {
            panic!("No iface ID was added.")
        }

        let iface_wrapper = IfaceWrapper::new(&iface_map, &test_values.ifaces_tree);
        let first_destroy_fut =
            super::destroy_iface(&test_values.phys, &iface_wrapper, TEST_IFACE_ID);
        let mut first_destroy_fut = pin!(first_destroy_fut);

        let second_destroy_fut =
            super::destroy_iface(&test_values.phys, &iface_wrapper, TEST_IFACE_ID);
        let mut second_destroy_fut = pin!(second_destroy_fut);

        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut first_destroy_fut));

        let responder = {
            let (_payload, responder) = assert_matches!(exec.run_until_stalled(&mut phy_stream.next()),
                Poll::Ready(Some(Ok(fidl_fuchsia_wlan_phy::WlanPhyRequest::DestroyIface { payload, responder }))) => (payload, responder)
            );
            responder
        };

        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut second_destroy_fut));

        {
            assert_matches!(exec.run_until_stalled(&mut phy_stream.next()), Poll::Pending);
        }

        if iface_events.try_next().is_ok() {
            panic!("Received unexpected iface event.");
        }

        // Respond to the initial destroy iface request and verify that the initial request runs to
        // completion and produces an OnIfaceRemoved event.
        responder.send(Ok(())).expect("failed to send DestroyIfaceResponse");
        assert_eq!(exec.run_until_stalled(&mut first_destroy_fut), Poll::Ready(Ok(())));
        if let Ok(Some(crate::watchable_map::MapEvent::KeyRemoved(iface_id))) =
            iface_events.try_next()
        {
            assert_eq!(iface_id, TEST_IFACE_ID);
        } else {
            panic!("No iface ID was removed.")
        }

        assert_eq!(
            exec.run_until_stalled(&mut second_destroy_fut),
            Poll::Ready(Err(zx::Status::NOT_FOUND))
        );
        if iface_events.try_next().is_ok() {
            panic!("Received unexpected iface event.");
        }
    }
    #[fuchsia::test]
    fn get_client_sme() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, _phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let (generic_sme_proxy, mut generic_sme_stream) =
            create_proxy_and_stream::<fidl_sme::GenericSmeMarker>();

        test_values.ifaces.insert(
            TEST_IFACE_ID,
            device::IfaceDevice {
                phy_ownership: PhyOwnership { phy_id: 10, phy_assigned_id: 0 },
                generic_sme: generic_sme_proxy,
            },
        );

        let (client_sme_proxy, client_sme_server) = create_proxy::<fidl_sme::ClientSmeMarker>();

        let req_fut = super::get_client_sme(&test_values.ifaces, TEST_IFACE_ID, client_sme_server);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        // Respond to a client SME request with a client endpoint.
        let sme_server = assert_matches!(exec.run_until_stalled(&mut generic_sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::GenericSmeRequest::GetClientSme { sme_server, responder }))) => {
                responder.send(Ok(())).expect("Failed to send response");
                sme_server
            }
        );
        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(())));

        // Verify that the correct endpoint is served.
        let _status_req = client_sme_proxy.status();

        let mut sme_stream = sme_server.into_stream();
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Status { .. })))
        );
    }

    #[fuchsia::test]
    fn get_client_sme_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, _phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let (generic_sme_proxy, generic_sme_server) = create_proxy::<fidl_sme::GenericSmeMarker>();

        test_values.ifaces.insert(
            TEST_IFACE_ID,
            device::IfaceDevice {
                phy_ownership: PhyOwnership { phy_id: 10, phy_assigned_id: 0 },
                generic_sme: generic_sme_proxy,
            },
        );

        let (_client_sme_proxy, client_sme_server) = create_proxy::<fidl_sme::ClientSmeMarker>();

        let req_fut = super::get_client_sme(&test_values.ifaces, TEST_IFACE_ID, client_sme_server);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        // Respond to a client SME request with an error.
        let mut generic_sme_stream = generic_sme_server.into_stream();
        assert_matches!(exec.run_until_stalled(&mut generic_sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::GenericSmeRequest::GetClientSme { responder, .. }))) => {
                responder.send(Err(1)).expect("Failed to send response");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn get_client_sme_invalid_iface() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, _phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let (generic_sme_proxy, _generic_sme_server) = create_proxy::<fidl_sme::GenericSmeMarker>();

        test_values.ifaces.insert(
            TEST_IFACE_ID,
            device::IfaceDevice {
                phy_ownership: PhyOwnership { phy_id: 10, phy_assigned_id: 0 },
                generic_sme: generic_sme_proxy,
            },
        );

        let (_client_sme_proxy, client_sme_server) = create_proxy::<fidl_sme::ClientSmeMarker>();

        let req_fut = super::get_client_sme(&test_values.ifaces, 1337, client_sme_server);
        let mut req_fut = pin!(req_fut);
        assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn query_iface() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let (phy, _phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let (generic_sme_proxy, mut generic_sme_stream) =
            create_proxy_and_stream::<fidl_sme::GenericSmeMarker>();

        test_values.ifaces.insert(
            TEST_IFACE_ID,
            device::IfaceDevice {
                phy_ownership: PhyOwnership { phy_id: 10, phy_assigned_id: 0 },
                generic_sme: generic_sme_proxy,
            },
        );

        let req_fut = super::query_iface(&test_values.ifaces, TEST_IFACE_ID);
        let mut req_fut = pin!(req_fut);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut req_fut));

        // Respond to a query with appropriate info.
        assert_matches!(
            exec.run_until_stalled(&mut generic_sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::GenericSmeRequest::Query { responder, .. }))) => {
                responder.send(&fidl_sme::GenericSmeQuery {
                    role: fidl_common::WlanMacRole::Client,
                    sta_addr: [2; 6],
                    factory_addr: [2; 6],
                }).expect("Failed to send query response");
            }
        );
        let resp =
            assert_matches!(exec.run_until_stalled(&mut req_fut), Poll::Ready(Ok(resp)) => resp);
        assert_eq!(
            resp,
            fidl_svc::QueryIfaceResponse {
                role: fidl_fuchsia_wlan_common::WlanMacRole::Client,
                id: TEST_IFACE_ID,
                phy_id: 10,
                phy_assigned_id: 0,
                sta_addr: [2; 6],
                factory_addr: [2; 6],
            }
        );
    }

    #[fuchsia::test]
    fn test_service_query_iface_capabilities() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let cfg = fake_wlandevicemonitor_config();

        let (phy, _phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);
        let (generic_sme_proxy, mut generic_sme_stream) =
            create_proxy_and_stream::<fidl_sme::GenericSmeMarker>();

        let iface_id = TEST_IFACE_ID;
        test_values.ifaces.insert(
            iface_id,
            device::IfaceDevice {
                phy_ownership: PhyOwnership { phy_id: 10, phy_assigned_id: 0 },
                generic_sme: generic_sme_proxy,
            },
        );

        let service_fut = serve_monitor_requests(
            test_values.monitor_stream,
            &test_values.phys,
            &test_values.ifaces,
            &test_values.watcher_service,
            &test_values.phy_event_service,
            &test_values.new_iface_sink,
            &test_values.iface_counter,
            &test_values.ifaces_tree,
            &cfg,
        );
        let mut service_fut = pin!(service_fut);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Request query_iface_capabilities via DeviceMonitor proxy
        let query_fut = test_values.monitor_proxy.query_iface_capabilities(iface_id);
        let mut query_fut = pin!(query_fut);
        assert_matches!(exec.run_until_stalled(&mut query_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        // Respond to a query_iface_capabilities with appropriate info.
        let apf_support = fidl_common::ApfPacketFilterSupport {
            supported: Some(true),
            version: Some(1),
            max_filter_length: Some(1024),
            ..Default::default()
        };
        assert_matches!(
            exec.run_until_stalled(&mut generic_sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::GenericSmeRequest::QueryIfaceCapabilities { responder, .. }))) => {
                responder.send(Ok(&apf_support)).expect("Failed to send query response");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut service_fut), Poll::Pending);

        let resp = assert_matches!(exec.run_until_stalled(&mut query_fut), Poll::Ready(Ok(Ok(resp))) => resp);
        assert_eq!(resp, apf_support);
    }
    #[test_case(Ok(()), false; "New PHY - Generic SME with OK epitaph shuts down cleanly")]
    #[test_case(Err(zx::Status::INTERNAL), true; "New PHY - Generic SME with error epitaph initiates iface removal")]
    fn new_iface_stream_epitaph(epitaph: Result<(), zx::Status>, should_destroy_iface: bool) {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let iface_wrapper = IfaceWrapper::new(&test_values.ifaces, &test_values.ifaces_tree);
        let new_iface_fut = handle_new_iface_stream(
            &test_values.phys,
            &iface_wrapper,
            test_values.new_iface_stream,
        );
        let mut new_iface_fut = pin!(new_iface_fut);

        let (phy, mut phy_stream) = fake_phy_device();
        let phy_id = 10u16;
        test_values.phys.insert(phy_id, phy);

        let id = TEST_IFACE_ID;
        let phy_ownership = PhyOwnership { phy_id, phy_assigned_id: 0 };

        let (generic_sme_proxy, generic_sme_server) = create_proxy::<fidl_sme::GenericSmeMarker>();

        test_values.ifaces.insert(
            id,
            device::IfaceDevice {
                phy_ownership: phy_ownership.clone(),
                generic_sme: generic_sme_proxy.clone(),
            },
        );

        let new_iface = NewIface { id, phy_ownership, generic_sme: generic_sme_proxy };
        test_values.new_iface_sink.unbounded_send(new_iface).expect("Failed to send new iface");
        assert_matches!(exec.run_until_stalled(&mut new_iface_fut), Poll::Pending);

        let (_generic_sme_stream, generic_sme_control) =
            generic_sme_server.into_stream_and_control_handle();
        generic_sme_control.shutdown_with_epitaph(epitaph);

        assert_matches!(exec.run_until_stalled(&mut new_iface_fut), Poll::Pending);

        if should_destroy_iface {
            expect_destroy_iface(&mut exec, &mut phy_stream, 0, Ok(()));
        } else {
            {
                assert_matches!(exec.run_until_stalled(&mut phy_stream.next()), Poll::Pending);
            }
        }
    }
}
