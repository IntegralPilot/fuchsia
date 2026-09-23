// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::nl80211::{Nl80211RateInfoAttr, Nl80211StaInfoAttr};
use crate::security::Credential;
use crate::security::wep::WepKeys;
use anyhow::{Context, Error, bail, format_err};
use fidl::endpoints::{ProtocolMarker, Proxy};
use fidl_fuchsia_power_system as fsystem;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_device_service as fidl_device_service;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_ieee80211::WlanBand;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fidl_fuchsia_wlan_wlanix as fidl_wlanix;
use fidl_fuchsia_wlan_wlanix::{
    Nl80211MessageResponder, Nl80211MessageResponse, Nl80211MessageV2Responder,
    WifiLegacyHalResetTxPowerScenarioResponder, WifiLegacyHalSelectTxPowerScenarioRequest,
    WifiLegacyHalSelectTxPowerScenarioResponder, WifiLegacyHalStatus,
};
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use fuchsia_trace_provider as trace_provider;
use futures::channel::mpsc;
use futures::{FutureExt, StreamExt, TryFutureExt, future};
use ieee80211::{Bssid, MacAddrBytes};
use log::{debug, error, info, warn};
use netlink_packet_core::{NetlinkDeserializable, NetlinkHeader, NetlinkSerializable};
use netlink_packet_generic::GenlMessage;
use netlink_packet_generic::message::EmptyDeserializeOptions;
use std::convert::{TryFrom, TryInto};
use std::sync::Arc;
use wlan_common::bss::BssDescription;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_telemetry::{self, TelemetryEvent, TelemetrySender, ThrottledErrorLogger};
mod bss_scorer;
mod default_drop;
mod ifaces;
mod nl80211;
mod scheduled_scans;

mod security;

use default_drop::{DefaultDrop, WithDefaultDrop};
use ifaces::{ClientIface, ConnectResult, IfaceManager, ScanEnd};
use nl80211::{
    Nl80211, Nl80211Attr, Nl80211BandAttr, Nl80211Cmd, Nl80211FrequencyAttr,
    Nl80211SchedScanMatchAttr, Nl80211SchedScanPlanAttr,
};
use scheduled_scans::ScheduledScanController;
use wlan_power_manager::{
    DevicePowerManager, POWER_LEVEL_ACTIVE, POWER_LEVEL_SUSPEND, PowerManager,
};

// TODO(https://fxbug.dev/368005870): Need to reconsider the consequences of using
// the same iface name, even when an iface is recreated.
const IFACE_NAME: &str = "wlan";
/// Time between potentially frequent error logs to prevent cluttering up the syslog.
const MIN_MINUTES_BETWEEN_FREQUENT_ERRORS: i64 = 60;
const INVALID_RSSI: i8 = -127;

async fn handle_wifi_sta_iface_request<I: IfaceManager, P: PowerManager>(
    req: fidl_wlanix::WifiStaIfaceRequest,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::WifiStaIfaceRequest::GetName { responder } => {
            info!("fidl_wlanix::WifiStaIfaceRequest::GetName");
            let response = fidl_wlanix::WifiStaIfaceGetNameResponse {
                iface_name: Some(IFACE_NAME.to_string()),
                ..Default::default()
            };
            responder.send(&response).context("send GetName response")?;
        }
        fidl_wlanix::WifiStaIfaceRequest::SetScanOnlyMode { responder, payload } => {
            let enabled = payload.enable;
            info!("fidl_wlanix::WifiStaIfaceRequest::SetScanOnlyMode: {:?}", enabled);
            let res = match enabled {
                Some(true) => {
                    // TODO(b/443061003): make a call to the driver here
                    Err(zx::sys::ZX_ERR_NOT_SUPPORTED)
                }
                Some(false) => {
                    // TODO(b/443061003): make a call to the driver here
                    Err(zx::sys::ZX_ERR_NOT_SUPPORTED)
                }
                None => Err(zx::sys::ZX_ERR_INVALID_ARGS),
            };
            responder.send(res).context("send SetScanOnlyMode response")?;
        }
        fidl_wlanix::WifiStaIfaceRequest::SetMacAddress { mac_addr, responder } => {
            let (iface, _iface_id) = get_iface_and_log(
                "fidl_wlanix::WifiStaIfaceRequest::SetMacAddress",
                iface_manager,
                IFACE_NAME,
            )
            .await?;
            let result = iface
                .set_mac_address(mac_addr)
                .await
                .map_err(|status| status.into_raw())
                .inspect_err(|raw_status| {
                    if let Err(e) = zx::Status::ok(*raw_status) {
                        error!("Failed to set mac address: {:?}", e);
                    }
                });
            responder.send(result).context("send SetMacAddress response")?;
        }
        fidl_wlanix::WifiStaIfaceRequest::GetApfPacketFilterSupport { responder } => {
            let (_iface, iface_id) = get_iface_and_log(
                "fidl_wlanix::WifiStaIfaceRequest::GetApfPacketFilterSupport",
                iface_manager.clone(),
                IFACE_NAME,
            )
            .await?;
            let resp = iface_manager.query_iface_capabilities(iface_id).await;
            let result = match resp {
                Ok(support) if support.supported == Some(true) => {
                    Ok(fidl_wlanix::WifiStaIfaceGetApfPacketFilterSupportResponse {
                        version: support.version,
                        max_filter_length: support.max_filter_length,
                        ..Default::default()
                    })
                }
                Ok(_) => Err(zx::sys::ZX_ERR_NOT_SUPPORTED),
                _ => Err(zx::sys::ZX_ERR_NOT_SUPPORTED),
            };
            let result = result.as_ref().map_err(|status| *status);
            responder.send(result).context("send GetApfPacketFilterSupport response")?;
        }
        fidl_wlanix::WifiStaIfaceRequest::InstallApfPacketFilter { responder, payload } => {
            let _wake_lease =
                power_manager.take_wake_lease("wlanix-install-apf-packet-filter").await;
            let (iface, _) = get_iface_and_log(
                "fidl_wlanix::WifiStaIfaceRequest::InstallApfPacketFilter",
                iface_manager,
                IFACE_NAME,
            )
            .await?;
            let result = match payload.program {
                Some(program) => iface
                    .install_apf_packet_filter(program)
                    .await
                    .map_err(|status| status.into_raw())
                    .inspect_err(|raw_status| {
                        warn!("Failed to install APF packet filter: {:?}", raw_status);
                    }),
                None => {
                    warn!("InstallApfPacketFilter was missing a program");
                    Err(zx::sys::ZX_ERR_INVALID_ARGS)
                }
            };
            responder.send(result).context("send InstallApfPacketFilter response")?;
        }
        fidl_wlanix::WifiStaIfaceRequest::ReadApfPacketFilterData { responder } => {
            let _wake_lease =
                power_manager.take_wake_lease("wlanix-read-apf-packet-filter-data").await;
            let (iface, _) = get_iface_and_log(
                "fidl_wlanix::WifiStaIfaceRequest::ReadApfPacketFilterData",
                iface_manager,
                IFACE_NAME,
            )
            .await?;
            let result = iface
                .read_apf_packet_filter_data()
                .await
                .map(|memory| fidl_wlanix::WifiStaIfaceReadApfPacketFilterDataResponse {
                    memory: Some(memory),
                    ..Default::default()
                })
                .map_err(|status| status.into_raw())
                .inspect_err(|raw_status| {
                    warn!("Failed to read APF packet filter: {:?}", raw_status);
                });
            let result = result.as_ref().map_err(|status| *status);
            responder.send(result).context("send ReadApfPacketFilterData response")?;
        }
        fidl_wlanix::WifiStaIfaceRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown WifiStaIfaceRequest ordinal: {}", ordinal);
        }
    }
    Ok(())
}

async fn serve_wifi_sta_iface<I: IfaceManager, P: PowerManager>(
    iface_id: u16,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    reqs: fidl_wlanix::WifiStaIfaceRequestStream,
) {
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) = handle_wifi_sta_iface_request(
                    req,
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                )
                .await
                {
                    warn!("Failed to handle WifiStaIfaceRequest for iface {}: {}", iface_id, e);
                }
            }
            Err(e) => {
                error!("Wifi sta iface {} request stream failed: {}", iface_id, e);
            }
        }
    })
    .await;
}

async fn handle_wifi_chip_request<I: IfaceManager, P: PowerManager>(
    req: fidl_wlanix::WifiChipRequest,
    chip_id: u16,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
    state: Arc<Mutex<WifiState>>,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::WifiChipRequest::CreateStaIface { payload, responder, .. } => {
            info!("fidl_wlanix::WifiChipRequest::CreateStaIface");
            let wake_lease = power_manager.take_wake_lease("wlanix-create-iface").await;
            match payload.iface {
                Some(iface) => {
                    let reqs = iface.into_stream();
                    match iface_manager
                        .create_client_iface(chip_id)
                        .inspect_err(|_e| {
                            telemetry_sender.send(TelemetryEvent::IfaceCreationFailure)
                        })
                        .await
                    {
                        Ok(iface_id) => {
                            telemetry_sender.send(TelemetryEvent::ClientIfaceCreated { iface_id });
                            responder.send(Ok(())).context("send CreateStaIface response")?;
                            // Drop the wake lease now that the interface is created, before the
                            // long-running serve_wifi_sta_iface task takes over.
                            drop(wake_lease);
                            serve_wifi_sta_iface(
                                iface_id,
                                Arc::clone(&iface_manager),
                                Arc::clone(&power_manager),
                                reqs,
                            )
                            .await;
                        }
                        Err(e) => {
                            // It is possible that interface creation fails due to the driver
                            // being in a bad state or in the middle of suspending.  In such cases,
                            // the driver internally holds an interface reference that cannot be
                            // used.  The stack ends up in a bad state as wlandevicemonitor and
                            // wlanix will assume that interface creation failed.  The caller will
                            // likely repeatedly attempt to create a STA iface.  In such cases,
                            // trigger a reset and once the reset succeeds, send an
                            // OnSubsystemRestart callback to notify the framework that it should
                            // trigger Wi-Fi recovery.
                            error!("Failed to create client iface: {}", e);
                            info!("Resetting PHY {}", chip_id);
                            if let Err(e) = iface_manager.reset_phy(chip_id).await {
                                error!("Failed to reset PHY: {}", e);
                                if let Err(e) = responder.send(Err(zx::sys::ZX_ERR_INTERNAL)) {
                                    error!(
                                        "Failed to send CreateStaIface response on PHY reset failure: {}",
                                        e
                                    );
                                }

                                // If interfaces cannot be created and the PHY cannot be reset, WLAN
                                // cannot be controlled.  In this case, wlanix should exit.
                                panic!("Unable to create interfaces or reset PHY.");
                            }

                            let mut state = state.lock();
                            maybe_run_callback(
                                "WifiEventCallbackProxy::OnSubsystemRestart",
                                |callback_proxy| {
                                    callback_proxy.on_subsystem_restart(
                                        fidl_wlanix::WifiEventCallbackOnSubsystemRestartRequest {
                                            status: Some(zx::sys::ZX_ERR_INTERNAL),
                                            ..Default::default()
                                        },
                                    )
                                },
                                &mut state.callback,
                            );

                            responder
                                .send(Err(zx::sys::ZX_ERR_INTERNAL))
                                .context("send CreateStaIface response")?;
                        }
                    }
                }
                None => {
                    responder
                        .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                        .context("send CreateStaIface response")?;
                }
            }
        }
        fidl_wlanix::WifiChipRequest::GetStaIfaceNames { responder } => {
            // TODO(b/323586414): Unit test once we actually support this.
            debug!("fidl_wlanix::WifiChipRequest::GetStaIfaceNames");
            let ifaces = iface_manager.list_ifaces();
            // TODO(b/298030634): Serve actual interface names.
            let response = fidl_wlanix::WifiChipGetStaIfaceNamesResponse {
                iface_names: Some(vec![IFACE_NAME.to_string(); ifaces.len()]),
                ..Default::default()
            };
            responder.send(&response).context("send GetStaIfaceNames response")?;
        }
        fidl_wlanix::WifiChipRequest::GetStaIface { payload, responder } => {
            // TODO(b/323586414): Unit test once we actually support this.
            debug!("fidl_wlanix::WifiChipRequest::GetStaIface");
            let wake_lease = power_manager.take_wake_lease("wlanix-get-iface").await;
            match payload.iface {
                Some(iface) => {
                    // TODO(b/298030634): Use the iface name to identify the correct iface here.
                    let reqs = iface.into_stream();
                    let ifaces = iface_manager.list_ifaces();
                    if ifaces.is_empty() {
                        warn!("No iface available for GetStaIface.");
                        responder
                            .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                            .context("send GetStaIface response")?;
                    } else {
                        responder.send(Ok(())).context("send GetStaIface response")?;
                        // Drop the wake lease before the long-running serve_wifi_sta_iface
                        // task takes over.
                        drop(wake_lease);
                        serve_wifi_sta_iface(
                            ifaces[0],
                            Arc::clone(&iface_manager),
                            Arc::clone(&power_manager),
                            reqs,
                        )
                        .await;
                    }
                }
                None => {
                    responder
                        .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                        .context("send GetStaIface response")?;
                }
            }
        }
        fidl_wlanix::WifiChipRequest::RemoveStaIface { payload: _, responder, .. } => {
            info!("fidl_wlanix::WifiChipRequest::RemoveStaIface");
            let _wake_lease = power_manager.take_wake_lease("wlanix-remove-iface").await;
            // TODO(b/298030634): Use the iface name to identify the correct iface here.
            let ifaces = iface_manager.list_ifaces();
            if ifaces.is_empty() {
                warn!("No iface available to remove.");
                responder
                    .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                    .context("send RemoveStaIface response")?;
            } else {
                info!("Removing iface {}", ifaces[0]);
                match iface_manager.destroy_iface(ifaces[0]).await {
                    Ok(()) => {
                        telemetry_sender
                            .send(TelemetryEvent::ClientIfaceDestroyed { iface_id: ifaces[0] });
                        responder.send(Ok(())).context("send RemoveStaIface response")?;
                    }
                    Err(e) => {
                        error!("Failed to remove iface: {}", e);
                        telemetry_sender.send(TelemetryEvent::IfaceDestructionFailure);
                        responder
                            .send(Err(zx::sys::ZX_ERR_NOT_SUPPORTED))
                            .context("send RemoveStaIface response")?;
                    }
                }
            }
        }
        fidl_wlanix::WifiChipRequest::SetCountryCode { payload, responder } => {
            info!("fidl_wlanix::WifiChipRequest::SetCountryCode");
            let result = match payload.code {
                Some(code) => iface_manager.set_country(chip_id, code).await.map_err(|e| {
                    error!("Failed to set country code {:?} in phy: {}", code, e);
                    zx::sys::ZX_ERR_INTERNAL
                }),
                None => {
                    error!("SetCountryCode missing country code");
                    Err(zx::sys::ZX_ERR_INVALID_ARGS)
                }
            };
            responder.send(result).context("send SetCountryCode response")?;
        }
        // TODO(https://fxbug.dev/366027488): GetAvailableModes is hardcoded.
        fidl_wlanix::WifiChipRequest::GetAvailableModes { responder } => {
            info!("fidl_wlanix::WifiChipRequest::GetAvailableModes");
            let response = fidl_wlanix::WifiChipGetAvailableModesResponse {
                chip_modes: Some(vec![fidl_wlanix::ChipMode {
                    id: Some(chip_id as u32),
                    available_combinations: Some(vec![fidl_wlanix::ChipConcurrencyCombination {
                        limits: Some(vec![fidl_wlanix::ChipConcurrencyCombinationLimit {
                            types: Some(vec![fidl_wlanix::IfaceConcurrencyType::Sta]),
                            max_ifaces: Some(1),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            responder.send(&response).context("send GetAvailableModes response")?;
        }
        fidl_wlanix::WifiChipRequest::GetId { responder } => {
            info!("fidl_wlanix::WifiChipRequest::GetId");
            let response = fidl_wlanix::WifiChipGetIdResponse {
                id: Some(chip_id as u32),
                ..Default::default()
            };
            responder.send(&response).context("send GetId response")?;
        }
        // TODO(https://fxbug.dev/366028666): GetMode is hardcoded.
        fidl_wlanix::WifiChipRequest::GetMode { responder } => {
            debug!("fidl_wlanix::WifiChipRequest::GetMode");
            let response =
                fidl_wlanix::WifiChipGetModeResponse { mode: Some(0), ..Default::default() };
            responder.send(&response).context("send GetMode response")?;
        }
        // TODO(https://fxbug.dev/366027491): GetCapabilities is hardcoded.
        fidl_wlanix::WifiChipRequest::GetCapabilities { responder } => {
            debug!("fidl_wlanix::WifiChipRequest::GetCapabilities");
            let response = fidl_wlanix::WifiChipGetCapabilitiesResponse {
                capabilities_mask: Some(0),
                ..Default::default()
            };
            responder.send(&response).context("send GetCapabilities response")?;
        }
        fidl_wlanix::WifiChipRequest::TriggerSubsystemRestart { responder } => {
            info!("fidl_wlanix::WifiChipRequest::TriggerSubsystemRestart");

            // Request a PHY reset for the specified ID.
            let result = iface_manager.reset_phy(chip_id).await;

            // Notify telemetry that the reset has been triggered.
            telemetry_sender.send(TelemetryEvent::RecoveryEvent);
            telemetry_sender.send(TelemetryEvent::RecoveryResult {
                result: if result.is_ok() { Ok(()) } else { Err(()) },
            });

            // Notify listeners of the reset if it was successful.
            if result.is_ok() {
                let mut state_lock = state.lock();
                maybe_run_callback(
                    "WifiEventCallback::OnSubsystemRestart",
                    |callback_proxy| {
                        callback_proxy.on_subsystem_restart(
                            fidl_wlanix::WifiEventCallbackOnSubsystemRestartRequest {
                                status: Some(zx::sys::ZX_OK),
                                ..Default::default()
                            },
                        )
                    },
                    &mut state_lock.callback,
                );
            } else {
                warn!("Reset failed on PHY {}", chip_id);
            }

            // Send the result of the reset to the caller.
            responder
                .send(result.map_err(|e| match e.downcast_ref::<zx::Status>() {
                    Some(status) => status.into_raw(),
                    None => zx::Status::INTERNAL.into_raw(),
                }))
                .context("send TriggerSubsystemRestart response")?;
        }
        fidl_wlanix::WifiChipRequest::ResetTxPowerScenario { responder } => {
            if let Err(e) = iface_manager.reset_tx_power_scenario(chip_id).await {
                warn!("{}", e);
            }
            responder.send().context("send ResetTxPowerScenario responder")?;
        }

        fidl_wlanix::WifiChipRequest::SelectTxPowerScenario { scenario, responder } => {
            if let Some(scenario) = wifi_chip_tx_power_scenario_to_internal(scenario) {
                if let Err(e) = iface_manager.set_tx_power_scenario(chip_id, scenario).await {
                    warn!("{}", e);
                }
                responder.send().context("send SetTxPowerScenario responder")?;
            }
        }
        fidl_wlanix::WifiChipRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown WifiChipRequest ordinal: {}", ordinal);
        }
    }
    Ok(())
}

async fn serve_wifi_chip<I: IfaceManager, P: PowerManager>(
    chip_id: u16,
    reqs: fidl_wlanix::WifiChipRequestStream,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
    state: Arc<Mutex<WifiState>>,
) {
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) = handle_wifi_chip_request(
                    req,
                    chip_id,
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    telemetry_sender.clone(),
                    Arc::clone(&state),
                )
                .await
                {
                    warn!("Failed to handle WifiChipRequest: {}", e);
                }
            }
            Err(e) => {
                error!("Wifi chip request stream failed: {}", e);
            }
        }
    })
    .await;
}

fn wifi_chip_tx_power_scenario_to_internal(
    scenario: fidl_wlanix::WifiChipTxPowerScenario,
) -> Option<fidl_internal::TxPowerScenario> {
    match scenario {
        fidl_wlanix::WifiChipTxPowerScenario::VoiceCall => {
            Some(fidl_internal::TxPowerScenario::VoiceCall)
        }
        fidl_wlanix::WifiChipTxPowerScenario::OnBodyCellOff => {
            Some(fidl_internal::TxPowerScenario::BodyCellOff)
        }
        fidl_wlanix::WifiChipTxPowerScenario::OnBodyCellOn => {
            Some(fidl_internal::TxPowerScenario::BodyCellOn)
        }
        fidl_wlanix::WifiChipTxPowerScenario::OnHeadCellOff => {
            Some(fidl_internal::TxPowerScenario::HeadCellOff)
        }
        fidl_wlanix::WifiChipTxPowerScenario::OnHeadCellOn => {
            Some(fidl_internal::TxPowerScenario::HeadCellOn)
        }
        other => {
            warn!("Unexpected power scenario: {:?}", other);
            None
        }
    }
}

fn maybe_run_callback<T: fidl::endpoints::Proxy>(
    event_name: &'static str,
    callback_fn: impl Fn(&T) -> Result<(), fidl::Error>,
    callback: &mut Option<T>,
) {
    let dropped = callback.take_if(|c| c.is_closed());
    if dropped.is_some() {
        warn!("Dropped {} proxy because channel is closed", T::Protocol::DEBUG_NAME);
    }
    if let Some(callback) = callback
        && let Err(e) = callback_fn(callback)
    {
        warn!("Failed sending {} event: {}", event_name, e);
    }
}

trait MulticastProxySet {
    const NAME: &'static str;

    fn proxies(&mut self) -> &mut Vec<fidl_wlanix::Nl80211MulticastProxy>;

    fn add_proxy(&mut self, proxy: fidl_wlanix::Nl80211MulticastProxy) {
        self.proxies().retain(|p| !p.is_closed());
        self.proxies().push(proxy);
    }

    fn send(&mut self, mut msg_fn: impl FnMut() -> fidl_wlanix::Nl80211MulticastMessageRequest) {
        self.proxies().retain(|proxy| {
            if proxy.is_closed() {
                false
            } else {
                match proxy.message(msg_fn()) {
                    Ok(()) => true,
                    Err(fidl::Error::ClientChannelClosed { .. }) => false,
                    Err(e) => {
                        warn!("Failed sending {} multicast message: {}", Self::NAME, e);
                        true
                    }
                }
            }
        });
    }
}

#[derive(Default)]
struct ScanMulticastProxySet {
    proxies: Vec<fidl_wlanix::Nl80211MulticastProxy>,
}

impl MulticastProxySet for ScanMulticastProxySet {
    const NAME: &'static str = "scan";

    fn proxies(&mut self) -> &mut Vec<fidl_wlanix::Nl80211MulticastProxy> {
        &mut self.proxies
    }
}

impl ScanMulticastProxySet {
    fn send_new_scan_results(&mut self, iface_id: u32) {
        self.send(|| fidl_wlanix::Nl80211MulticastMessageRequest {
            message: Some(build_nl80211_message(
                Nl80211Cmd::NewScanResults,
                vec![Nl80211Attr::IfaceIndex(iface_id)],
            )),
            ..Default::default()
        });
    }

    fn send_scan_aborted(&mut self, iface_id: u32) {
        self.send(|| fidl_wlanix::Nl80211MulticastMessageRequest {
            message: Some(build_nl80211_message(
                Nl80211Cmd::ScanAborted,
                vec![Nl80211Attr::IfaceIndex(iface_id)],
            )),
            ..Default::default()
        });
    }

    fn send_sched_scan_results(&mut self, iface_id: u32) {
        self.send(|| fidl_wlanix::Nl80211MulticastMessageRequest {
            message: Some(build_nl80211_message(
                Nl80211Cmd::SchedScanResults,
                vec![Nl80211Attr::IfaceIndex(iface_id)],
            )),
            ..Default::default()
        });
    }

    fn send_sched_scan_stopped(&mut self, iface_id: u32) {
        self.send(|| fidl_wlanix::Nl80211MulticastMessageRequest {
            message: Some(build_nl80211_message(
                Nl80211Cmd::SchedScanStopped,
                vec![Nl80211Attr::IfaceIndex(iface_id)],
            )),
            ..Default::default()
        });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.proxies.len()
    }
}

#[derive(Default)]
struct MlmeMulticastProxySet {
    proxies: Vec<fidl_wlanix::Nl80211MulticastProxy>,
}

impl MulticastProxySet for MlmeMulticastProxySet {
    const NAME: &'static str = "mlme";

    fn proxies(&mut self) -> &mut Vec<fidl_wlanix::Nl80211MulticastProxy> {
        &mut self.proxies
    }
}

impl MlmeMulticastProxySet {
    fn send_disconnect(&mut self, iface_id: u32, mac: [u8; 6]) {
        self.send(|| fidl_wlanix::Nl80211MulticastMessageRequest {
            message: Some(build_nl80211_message(
                Nl80211Cmd::Disconnect,
                vec![Nl80211Attr::IfaceIndex(iface_id), Nl80211Attr::Mac(mac)],
            )),
            ..Default::default()
        });
    }

    fn send_connect(&mut self, iface_id: u32, mac: [u8; 6], status_code: u16) {
        self.send(|| fidl_wlanix::Nl80211MulticastMessageRequest {
            message: Some(build_nl80211_message(
                Nl80211Cmd::Connect,
                vec![
                    Nl80211Attr::IfaceIndex(iface_id),
                    Nl80211Attr::Mac(mac),
                    Nl80211Attr::StatusCode(status_code),
                ],
            )),
            ..Default::default()
        });
    }
}

#[derive(Default)]
struct WifiState {
    started: bool,
    callback: Option<fidl_wlanix::WifiEventCallbackProxy>,
    scan_multicast_proxies: ScanMulticastProxySet,
    mlme_multicast_proxies: MlmeMulticastProxySet,
    power_dependency_leases: Vec<(
        u16,
        fidl_fuchsia_power_broker::DependencyToken,
        fidl_fuchsia_power_broker::LeaseToken,
    )>,
}

async fn handle_wifi_request<I: IfaceManager, P: PowerManager>(
    req: fidl_wlanix::WifiRequest,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::WifiRequest::RegisterEventCallback { payload, .. } => {
            info!("fidl_wlanix::WifiRequest::RegisterEventCallback");
            if let Some(callback) = payload.callback
                && state.lock().callback.replace(callback.into_proxy()).is_some()
            {
                warn!("Replaced a WifiEventCallbackProxy when there's one existing");
            }
        }
        fidl_wlanix::WifiRequest::Start { responder } => {
            info!("fidl_wlanix::WifiRequest::Start");
            let _wake_lease = power_manager.take_wake_lease("wlanix-power-up").await;
            let mut result: Result<(), i32> = Ok(());
            let mut driver_started: bool = true;
            let phy_ids = iface_manager.list_phys().await?;
            for phy_id in phy_ids {
                // Get the current state from the driver.
                let power_state = iface_manager.get_power_state(phy_id).await?;

                if !power_state {
                    if let Err(e) = iface_manager.power_up(phy_id).await {
                        error!(
                            "Failed to start phy {} in response to WifiRequest::Start: {}",
                            phy_id, e
                        );
                        // Attempt to power the chip back down, to ensure we're in a low-power
                        // state after the failure to power up.
                        if let Err(e) = iface_manager.power_down(phy_id).await {
                            error!(
                                "Failed to stop phy {} to recover from failed WifiRequest::Start: {}",
                                phy_id, e
                            )
                        };
                        telemetry_sender.send(TelemetryEvent::ChipPowerUpFailure);
                        driver_started = false;
                        result = Err(zx::sys::ZX_ERR_BAD_STATE);
                    }
                } else {
                    warn!("Phy {} already started", phy_id);
                }

                // Query the PHY driver for its power element dependency token to check if power
                // element control is supported.
                let power_elem_dependency_token = iface_manager
                    .get_power_element_dependency_token(phy_id)
                    .await
                    .inspect_err(|e| {
                        warn!("Failed to get power dependency token for phy {}: {}", phy_id, e);
                    })
                    .ok();
                // Duplicate the dependency token (if available) so we can keep a copy.
                let token_dup = power_elem_dependency_token.as_ref().and_then(|token| {
                    token
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .inspect_err(|e| {
                            warn!("Failed to duplicate token: {}", e);
                            driver_started = false;
                            result = Err(zx::sys::ZX_ERR_BAD_STATE);
                        })
                        .ok()
                });
                // If the driver supports power element control, acquire an active lease.
                if let (Some(token_dup), Some(token_orig)) =
                    (token_dup, power_elem_dependency_token)
                {
                    let lease_name = format!("wlanix-phy-{}-dependency", phy_id);
                    match power_manager
                        .power_element_lease(&lease_name, token_orig, POWER_LEVEL_ACTIVE)
                        .await
                    {
                        Ok(lease_token_local) => {
                            // Retain the active lease to keep the hardware powered.
                            // Also hold a copy of the dependency token, so subsequent
                            // suspend/resume transitions can take leases without querying the
                            // driver again.
                            let mut state_lock = state.lock();
                            state_lock.power_dependency_leases.push((
                                phy_id,
                                token_dup,
                                lease_token_local,
                            ));
                        }
                        Err(e) => {
                            warn!(
                                "Failed to acquire power dependency lease {:?} for phy {}: {:?}",
                                lease_name, phy_id, e
                            );
                            driver_started = false;
                            result = Err(zx::sys::ZX_ERR_BAD_STATE);
                        }
                    }
                } else {
                    // Power elements are unsupported or unavailable on this PHY; proceed unmanaged.
                    info!("Skipping power element leases due to missing dependency token")
                }
            }
            let mut state = state.lock();
            state.started = driver_started;
            if driver_started {
                maybe_run_callback(
                    "WifiEventCallbackProxy::OnStart",
                    fidl_wlanix::WifiEventCallbackProxy::on_start,
                    &mut state.callback,
                );

                let event = wlan_telemetry::ClientConnectionsToggleEvent::Enabled;
                telemetry_sender.send(TelemetryEvent::ClientConnectionsToggle { event });
            } else {
                state.power_dependency_leases.clear();
            }
            responder.send(result).context("send Start response")?;
        }

        fidl_wlanix::WifiRequest::Stop { responder } => {
            info!("fidl_wlanix::WifiRequest::Stop");
            let _wake_lease = power_manager.take_wake_lease("wlanix-power-down").await;
            let mut result: Result<(), i32> = Ok(());
            let mut driver_stopped: bool = true;
            let phy_ids = iface_manager.list_phys().await?;
            for phy_id in phy_ids {
                // Get the current state from the driver.
                let power_state = iface_manager.get_power_state(phy_id).await?;

                // If powered up, attempt to power it down.
                if power_state {
                    // Tear down all ifaces before calling power_down.
                    for iface in iface_manager.list_ifaces() {
                        if let Err(e) = iface_manager.destroy_iface(iface).await {
                            telemetry_sender.send(TelemetryEvent::IfaceDestructionFailure);
                            error!(
                                "Failed to destroy iface {} in response to WifiRequest::Stop: {}",
                                iface, e
                            );
                        } else {
                            info!("Successfully deleted iface {} in phy {}", iface, phy_id);
                        }
                    }
                    if let Err(e) = iface_manager.power_down(phy_id).await {
                        error!(
                            "Failed to stop phy {} in response to WifiRequest::Stop: {}",
                            phy_id, e
                        );
                        driver_stopped = false;
                        result = Err(zx::sys::ZX_ERR_BAD_STATE);
                        telemetry_sender.send(TelemetryEvent::ChipPowerDownFailure);
                    }
                } else {
                    warn!("Phy {} already stopped", phy_id);
                }
            }
            let mut state = state.lock();
            state.started = !driver_stopped;
            if driver_stopped {
                state.power_dependency_leases.clear();
                maybe_run_callback(
                    "WifiEventCallbackProxy::OnStop",
                    fidl_wlanix::WifiEventCallbackProxy::on_stop,
                    &mut state.callback,
                );

                let event = wlan_telemetry::ClientConnectionsToggleEvent::Disabled;
                telemetry_sender.send(TelemetryEvent::ClientConnectionsToggle { event });
            }
            responder.send(result).context("send Stop response")?;
        }

        fidl_wlanix::WifiRequest::GetState { responder } => {
            debug!("fidl_wlanix::WifiRequest::GetState");
            let response = fidl_wlanix::WifiGetStateResponse {
                is_started: Some(state.lock().started),
                ..Default::default()
            };
            responder.send(&response).context("send GetState response")?;
        }
        fidl_wlanix::WifiRequest::GetChipIds { responder } => {
            debug!("fidl_wlanix::WifiRequest::GetChipIds");
            let phy_ids = iface_manager.list_phys().await?;
            let response = fidl_wlanix::WifiGetChipIdsResponse {
                chip_ids: Some(phy_ids.into_iter().map(Into::into).collect()),
                ..Default::default()
            };
            responder.send(&response).context("send GetChipIds response")?;
        }
        fidl_wlanix::WifiRequest::GetChip { payload, responder } => {
            debug!("fidl_wlanix::WifiRequest::GetChip - chip_id {:?}", payload.chip_id);
            match (payload.chip_id, payload.chip) {
                (Some(chip_id), Some(chip)) => {
                    let chip_stream = chip.into_stream();
                    match u16::try_from(chip_id) {
                        Ok(chip_id) => {
                            responder.send(Ok(())).context("send GetChip response")?;
                            serve_wifi_chip(
                                chip_id,
                                chip_stream,
                                iface_manager,
                                power_manager,
                                telemetry_sender,
                                Arc::clone(&state),
                            )
                            .await;
                        }
                        Err(_e) => {
                            warn!("fidl_wlanix::WifiRequest::GetChip chip_id > u16::MAX");
                            responder
                                .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                                .context("send GetChip response")?;
                        }
                    }
                }
                _ => {
                    warn!("No chip_id or chip in fidl_wlanix::WifiRequest::GetChip");
                    responder
                        .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                        .context("send GetChip response")?;
                }
            }
        }
        fidl_wlanix::WifiRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown WifiRequest ordinal: {}", ordinal);
        }
    }
    Ok(())
}

async fn serve_wifi<I: IfaceManager, P: PowerManager>(
    reqs: fidl_wlanix::WifiRequestStream,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
) {
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) = handle_wifi_request(
                    req,
                    Arc::clone(&state),
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    telemetry_sender.clone(),
                )
                .await
                {
                    warn!("Failed to handle WifiRequest: {}", e);
                }
            }
            Err(e) => {
                error!("Wifi request stream failed: {}", e);
            }
        }
    })
    .await;
}

struct SupplicantStaNetworkState {
    ssid: Option<Vec<u8>>,
    credential: Credential,
    bssid: Option<Bssid>,
    // This is set through the HAL. If it is not set, there will be no restrictions on the security
    // type used.
    key_mgmt: Option<fidl_wlanix::KeyMgmtMask>,
}

impl Default for SupplicantStaNetworkState {
    fn default() -> Self {
        Self { ssid: None, credential: Credential::None, bssid: None, key_mgmt: None }
    }
}

struct SupplicantStaIfaceState {
    callback: Option<fidl_wlanix::SupplicantStaIfaceCallbackProxy>,
}

struct ConnectionContext {
    stream: fidl_sme::ConnectTransactionEventStream,
    original_bss_desc: Box<BssDescription>,
    most_recent_connect_time: fasync::BootInstant,
    current_rssi_dbm: i8,
    current_snr_db: i8,
    current_channel: Channel,
}

fn send_disconnect_event<C: ClientIface>(
    source: &fidl_sme::DisconnectSource,
    ctx: &ConnectionContext,
    sta_iface_state: &mut SupplicantStaIfaceState,
    wifi_state: &mut WifiState,
    iface: &C,
    iface_id: u16,
) {
    // We expect both an OnDisconnected and an OnStateChanged event.
    let (locally_generated, reason_code) = match source {
        fidl_sme::DisconnectSource::Ap(cause) => (false, cause.reason_code),
        fidl_sme::DisconnectSource::Mlme(cause) => (true, cause.reason_code),
        fidl_sme::DisconnectSource::User(user_reason) => {
            warn!("Disconnected by user with reason: {:?}", user_reason);
            (true, fidl_fuchsia_wlan_ieee80211::ReasonCode::UnspecifiedReason)
        }
    };
    let disconnected_event = fidl_wlanix::SupplicantStaIfaceCallbackOnDisconnectedRequest {
        bssid: Some(ctx.original_bss_desc.bssid.to_array()),
        locally_generated: Some(locally_generated),
        reason_code: Some(reason_code),
        ..Default::default()
    };
    maybe_run_callback(
        "SupplicantStaIfaceCallbackProxy::onDisconnected",
        |callback_proxy| callback_proxy.on_disconnected(&disconnected_event),
        &mut sta_iface_state.callback,
    );
    let state_changed_event = fidl_wlanix::SupplicantStaIfaceCallbackOnStateChangedRequest {
        new_state: Some(fidl_wlanix::StaIfaceCallbackState::Disconnected),
        bssid: Some(ctx.original_bss_desc.bssid.to_array()),
        // TODO(b/316034688): do we need to keep track of actual id?
        id: Some(1),
        ssid: Some(ctx.original_bss_desc.ssid.to_vec()),
        ..Default::default()
    };
    maybe_run_callback(
        "SupplicantStaIfaceCallbackProxy::onStateChanged",
        |callback_proxy| callback_proxy.on_state_changed(&state_changed_event),
        &mut sta_iface_state.callback,
    );
    // Also communicate the state change via nl80211.
    wifi_state
        .mlme_multicast_proxies
        .send_disconnect(iface_id.into(), ctx.original_bss_desc.bssid.to_array());

    // Let iface know about disconnect so it clears any intermediate state
    iface.on_disconnect(source);
}

#[allow(clippy::too_many_arguments)]
async fn handle_client_connect_transactions<C: ClientIface + 'static, P: PowerManager>(
    mut ctx: ConnectionContext,
    sta_iface_state: Arc<Mutex<SupplicantStaIfaceState>>,
    wifi_state: Arc<Mutex<WifiState>>,
    telemetry_sender: TelemetrySender,
    iface: Arc<C>,
    iface_id: u16,
    power_manager: Arc<P>,
) {
    // If we receive a disconnect but attempt to reconnect, we will deliver the
    // disconnect event later if the reconnect attempt fails.
    let mut disconnect_with_ongoing_reconnect: Option<fidl_sme::DisconnectSource> = None;

    loop {
        // The transaction stream will exit cleanly when the connection has fully terminated.
        let req = match ctx.stream.next().await {
            Some(req) => req,
            None => return,
        };
        match req {
            Ok(fidl_sme::ConnectTransactionEvent::OnConnectResult { result }) => {
                let _wake_lease =
                    power_manager.take_wake_lease("wlanix-process-connect-result").await;
                match (disconnect_with_ongoing_reconnect.as_ref(), result.is_reconnect) {
                    (Some(info), true) => {
                        if result.code == fidl_fuchsia_wlan_ieee80211::StatusCode::Success {
                            ctx.most_recent_connect_time = fasync::BootInstant::now();
                            info!("Successfully reconnected after disconnect");
                        } else {
                            send_disconnect_event(
                                info,
                                &ctx,
                                &mut sta_iface_state.lock(),
                                &mut wifi_state.lock(),
                                &*iface,
                                iface_id,
                            );
                        }
                        disconnect_with_ongoing_reconnect = None;
                    }
                    (Some(_), false) => {
                        error!("Received non-reconnect connect result while reconnecting")
                    }

                    (None, true) => error!("Received reconnect result while not reconnecting"),
                    (None, false) => error!(
                        "Received unexpected connect result after connection already established."
                    ),
                }
            }
            Ok(fidl_sme::ConnectTransactionEvent::OnRoamResult { result }) => {
                let _wake_lease = power_manager.take_wake_lease("wlanix-process-roam-result").await;
                match result.status_code {
                    fidl_fuchsia_wlan_ieee80211::StatusCode::Success => {
                        info!("Connection roamed successfully");
                        // TODO(https://fxbug.dev/352557875): Plumb SME RoamResult into wlanix.
                    }
                    _ => {
                        info!("Connection failed to roam");
                        let source = match result.disconnect_info {
                            Some(disconnect_info) => disconnect_info.disconnect_source,
                            None => {
                                error!(
                                    "RoamResult indicates failure, but disconnect source is missing"
                                );
                                fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                                    mlme_event_name:
                                        fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                                    reason_code:
                                        fidl_fuchsia_wlan_ieee80211::ReasonCode::UnspecifiedReason,
                                })
                            }
                        };
                        send_disconnect_event(
                            &source,
                            &ctx,
                            &mut sta_iface_state.lock(),
                            &mut wifi_state.lock(),
                            &*iface,
                            iface_id,
                        );
                    }
                }
            }
            Ok(fidl_sme::ConnectTransactionEvent::OnDisconnect { info }) => {
                let _wake_lease = power_manager.take_wake_lease("wlanix-process-disconnect").await;
                let connected_duration = fasync::BootInstant::now() - ctx.most_recent_connect_time;
                telemetry_sender.send(TelemetryEvent::Disconnect {
                    info: wlan_telemetry::DisconnectInfo {
                        iface_id,
                        connected_duration,
                        is_sme_reconnecting: info.is_sme_reconnecting,
                        disconnect_source: info.disconnect_source,
                        original_bss_desc: ctx.original_bss_desc.clone(),
                        current_rssi_dbm: ctx.current_rssi_dbm,
                        current_snr_db: ctx.current_snr_db,
                        current_channel: ctx.current_channel,
                    },
                });
                if info.is_sme_reconnecting {
                    info!("Connection interrupted, awaiting reconnect: {:?}", info);
                    disconnect_with_ongoing_reconnect = Some(info.disconnect_source);
                } else {
                    info!(
                        "Connection terminated by disconnect, lasted {:?}: {:?}",
                        connected_duration, info
                    );
                    send_disconnect_event(
                        &info.disconnect_source,
                        &ctx,
                        &mut sta_iface_state.lock(),
                        &mut wifi_state.lock(),
                        &*iface,
                        iface_id,
                    );
                    disconnect_with_ongoing_reconnect = None;
                }
            }
            Ok(fidl_sme::ConnectTransactionEvent::OnSignalReport { ind }) => {
                ctx.current_rssi_dbm = ind.rssi_dbm;
                ctx.current_snr_db = ind.snr_db;
                iface.on_signal_report(ind);
            }
            Ok(fidl_sme::ConnectTransactionEvent::OnChannelSwitched { info }) => {
                ctx.current_channel.primary = info.new_primary_channel.number;
                ctx.current_channel.band = info.new_primary_channel.band;

                match Bandwidth::from_fidl(info.bandwidth, info.vht_secondary_80_channel.number) {
                    Ok(cbw) => ctx.current_channel.bandwidth = cbw,
                    Err(e) => warn!("CBW conversion failed: {}", e),
                }
                info!("Connection switching to channel {}", info.new_primary_channel.number);
            }
            Err(e) => {
                error!("Error on connect transaction event stream: {}", e);
            }
        }
    }
}

/// This is the same as get_iface_and_log without logging for the functions that would be called
/// frequently and would cause log spam.
async fn get_iface<I: IfaceManager>(iface_manager: Arc<I>) -> Result<(Arc<I::Client>, u16), Error> {
    // TODO(https://fxbug.dev/299349496): Fetch the iface by name.
    let ifaces = iface_manager.list_ifaces();
    if ifaces.is_empty() {
        bail!("no iface available");
    } else {
        match iface_manager.get_client_iface(ifaces[0]).await {
            Ok(iface) => Ok((iface, ifaces[0])),
            Err(e) => bail!("failed to get client iface: {}", e),
        }
    }
}

async fn get_iface_and_log<I: IfaceManager>(
    label: &str,
    iface_manager: Arc<I>,
    iface_name: &str,
) -> Result<(Arc<I::Client>, u16), Error> {
    // TODO(https://fxbug.dev/299349496): Fetch the iface by name.
    let ifaces = iface_manager.list_ifaces();
    if ifaces.is_empty() {
        warn!("{} -- no iface available to serve {}", label, iface_name);
        bail!("no iface available");
    } else {
        match iface_manager.get_client_iface(ifaces[0]).await {
            Ok(iface) => {
                info!("{} ({}, iface {})", label, iface_name, ifaces[0]);
                Ok((iface, ifaces[0]))
            }
            Err(e) => {
                error!("{} -- failed to get client iface: {}", label, e);
                bail!("failed to get client iface: {}", e);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_supplicant_sta_network_request<I: IfaceManager, P: PowerManager>(
    telemetry_sender: TelemetrySender,
    req: fidl_wlanix::SupplicantStaNetworkRequest,
    sta_network_state: Arc<Mutex<SupplicantStaNetworkState>>,
    sta_iface_state: Arc<Mutex<SupplicantStaIfaceState>>,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    iface_name: String,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::SupplicantStaNetworkRequest::SetBssid { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::SetBssid",
                iface_manager,
                &iface_name,
            )
            .await?;
            if let Some(bssid) = payload.bssid {
                sta_network_state.lock().bssid.replace(Bssid::from(bssid));
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::ClearBssid { .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::ClearBssid",
                iface_manager,
                &iface_name,
            )
            .await?;
            sta_network_state.lock().bssid.take();
        }
        fidl_wlanix::SupplicantStaNetworkRequest::SetSsid { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::SetSsid",
                iface_manager,
                &iface_name,
            )
            .await?;
            if let Some(ssid) = payload.ssid {
                sta_network_state.lock().ssid.replace(ssid);
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::SetKeyMgmt { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::SetKeyMgmt",
                iface_manager,
                &iface_name,
            )
            .await?;
            if let Some(key_mgmt_mask) = payload.key_mgmt_mask {
                sta_network_state.lock().key_mgmt.replace(key_mgmt_mask);
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::SetPskPassphrase { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::SetPskPassphrase",
                iface_manager,
                &iface_name,
            )
            .await?;
            if let Some(passphrase) = payload.passphrase {
                sta_network_state.lock().credential = Credential::Password(passphrase);
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::SetSaePassword { payload, .. } => {
            info!("fidl_wlanix::SupplicantStaNetworkRequest::SetSaePassword");
            if let Some(password) = payload.password {
                sta_network_state.lock().credential = Credential::SaePassword(password);
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::SetWepKey { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::SetWepKey",
                iface_manager,
                &iface_name,
            )
            .await?;
            let mut sta_network_state = sta_network_state.lock();
            let key = payload.key.ok_or_else(|| format_err!("SetWepKey's key is None"))?;
            let index =
                payload.key_idx.ok_or_else(|| format_err!("SetWepKey's index is None"))? as usize;

            match sta_network_state.credential {
                Credential::None => {
                    let mut wep_keys = WepKeys::new();
                    wep_keys.set_key(key, index)?;

                    sta_network_state.credential = Credential::WepKey(wep_keys);
                }
                Credential::Password(_) | Credential::SaePassword(_) => {
                    warn!(
                        "SetWepKey was called for a network that already has a passphrase; ignoring"
                    );
                }
                Credential::WepKey(ref mut wep_keys) => {
                    wep_keys
                        .set_key(key, index)
                        .map_err(|e| format_err!("Error setting WEP key: {}", e))?;
                }
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::SetWepTxKeyIdx { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::SetWepTxKeyIdx",
                iface_manager,
                &iface_name,
            )
            .await?;
            let index = payload.key_idx.ok_or_else(|| format_err!("WEP key index is None"))?;
            let mut sta_network = sta_network_state.lock();

            match sta_network.credential {
                Credential::None => {
                    warn!("Setting WEP key index unexpectedly before setting WEP key");
                    let mut wep_keys = WepKeys::new();
                    wep_keys.set_index(index as usize)?;
                    sta_network.credential = Credential::WepKey(wep_keys);
                }
                Credential::WepKey(ref mut wep_keys) => {
                    wep_keys.set_index(index as usize)?;
                }
                Credential::Password(_) | Credential::SaePassword(_) => {
                    warn!(
                        "SetWepTxKeyIdx was called when the credential has been set to Password; ignoring."
                    );
                }
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::Select { responder } => {
            let wake_lease = power_manager.take_wake_lease("wlanix-select-connect").await;
            let (iface, iface_id) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaNetworkRequest::Select",
                iface_manager,
                &iface_name,
            )
            .await?;
            let (ssid, credential, bssid, key_mgmt) = {
                let state = sta_network_state.lock();
                let credential = state.credential.clone();
                (state.ssid.clone(), credential, state.bssid, state.key_mgmt)
            };

            let (result, status_code, connected_bssid, connection_ctx) = match ssid {
                Some(ssid) => {
                    match iface.connect_to_network(&ssid[..], credential, bssid, key_mgmt).await {
                        Ok(ConnectResult::Success(connected)) => {
                            info!("Connected to requested network");
                            // Report the requested SSID for OWE transition networks.
                            let is_owe_transition = connected.ssid_if_owe_transition.is_some();
                            let ssid = connected
                                .ssid_if_owe_transition
                                .unwrap_or_else(|| connected.bss.ssid.clone());

                            telemetry_sender.send(TelemetryEvent::ConnectResult {
                                result: fidl_ieee80211::StatusCode::Success,
                                bss: connected.bss.clone(),
                                is_credential_rejected: false,
                                is_owe_transition,
                            });
                            let event =
                                fidl_wlanix::SupplicantStaIfaceCallbackOnStateChangedRequest {
                                    new_state: Some(fidl_wlanix::StaIfaceCallbackState::Completed),
                                    bssid: Some(connected.bss.bssid.to_array()),
                                    // TODO(b/316034688): do we need to keep track of actual id?
                                    id: Some(1),
                                    ssid: Some(ssid.into()),
                                    ..Default::default()
                                };
                            maybe_run_callback(
                                "SupplicantStaIfaceCallbackProxy::onStateChanged",
                                |callback_proxy| callback_proxy.on_state_changed(&event),
                                &mut sta_iface_state.lock().callback,
                            );
                            (
                                Ok(()),
                                fidl_ieee80211::StatusCode::Success,
                                Some(connected.bss.bssid),
                                Some(ConnectionContext {
                                    stream: connected.transaction_stream,
                                    original_bss_desc: connected.bss.clone(),
                                    most_recent_connect_time: fasync::BootInstant::now(),
                                    current_rssi_dbm: connected.bss.rssi_dbm,
                                    current_snr_db: connected.bss.snr_db,
                                    current_channel: connected.bss.channel,
                                }),
                            )
                        }
                        Ok(ConnectResult::Fail(fail)) => {
                            warn!("Connection failed with status code: {:?}", fail.status_code);
                            telemetry_sender.send(TelemetryEvent::ConnectResult {
                                result: fail.status_code,
                                bss: fail.bss.clone(),
                                is_credential_rejected: fail.is_credential_rejected,
                                is_owe_transition: fail.is_owe_transition,
                            });
                            let event =
                            fidl_wlanix::SupplicantStaIfaceCallbackOnAssociationRejectedRequest {
                                ssid: Some(fail.bss.ssid.to_vec()),
                                bssid: Some(fail.bss.bssid.to_array()),
                                status_code: Some(fail.status_code),
                                timed_out: Some(fail.timed_out),
                                ..Default::default()
                            };
                            maybe_run_callback(
                                "SupplicantStaIfaceCallbackProxy::onAssociationRejected",
                                |callback_proxy| callback_proxy.on_association_rejected(&event),
                                &mut sta_iface_state.lock().callback,
                            );
                            (Ok(()), fail.status_code, None, None)
                        }
                        Err(e) => {
                            error!("Error while connecting to network: {}", e);
                            (
                                Err(zx::sys::ZX_ERR_INTERNAL),
                                fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                                None,
                                None,
                            )
                        }
                    }
                }
                None => {
                    warn!("No SSID set. fidl_wlanix::SupplicantStaNetworkRequest::Select ignored");
                    (
                        Err(zx::sys::ZX_ERR_BAD_STATE),
                        fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                        None,
                        None,
                    )
                }
            };
            responder.send(result).context("send Select response")?;
            {
                let bssid = connected_bssid.map(|bssid| bssid.to_array()).unwrap_or_default();
                state.lock().mlme_multicast_proxies.send_connect(
                    iface_id.into(),
                    bssid,
                    status_code.into_primitive(),
                );
            }
            // Drop the wake lease now that the connection is established, before the long-running
            // handle_client_connect_transactions task takes over.
            drop(wake_lease);
            if let Some(ctx) = connection_ctx {
                // Continue to process connection updates until the connection terminates.
                // We can do this here because calls to this function are all executed
                // concurrently, so it doesn't block other requests.
                handle_client_connect_transactions(
                    ctx,
                    Arc::clone(&sta_iface_state),
                    Arc::clone(&state),
                    telemetry_sender,
                    Arc::clone(&iface),
                    iface_id,
                    Arc::clone(&power_manager),
                )
                .await;
            }
        }
        fidl_wlanix::SupplicantStaNetworkRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown SupplicantStaNetworkRequest ordinal: {}", ordinal);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn serve_supplicant_sta_network<I: IfaceManager, P: PowerManager>(
    telemetry_sender: TelemetrySender,
    reqs: fidl_wlanix::SupplicantStaNetworkRequestStream,
    sta_iface_state: Arc<Mutex<SupplicantStaIfaceState>>,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    iface_name: String,
) {
    let sta_network_state = Arc::new(Mutex::new(SupplicantStaNetworkState::default()));
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) = handle_supplicant_sta_network_request(
                    telemetry_sender.clone(),
                    req,
                    Arc::clone(&sta_network_state),
                    Arc::clone(&sta_iface_state),
                    Arc::clone(&state),
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    iface_name.clone(),
                )
                .await
                {
                    warn!("Failed to handle SupplicantStaNetwork: {}", e);
                }
            }
            Err(e) => {
                error!("SupplicantStaNetwork request stream failed: {}", e);
            }
        }
    })
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn handle_supplicant_sta_iface_request<I: IfaceManager, P: PowerManager>(
    telemetry_sender: TelemetrySender,
    req: fidl_wlanix::SupplicantStaIfaceRequest,
    sta_iface_state: Arc<Mutex<SupplicantStaIfaceState>>,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    iface_name: String,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::SupplicantStaIfaceRequest::RegisterCallback { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::RegisterCallback",
                iface_manager,
                &iface_name,
            )
            .await?;
            if let Some(callback) = payload.callback {
                if sta_iface_state.lock().callback.replace(callback.into_proxy()).is_some() {
                    warn!("Replaced a SupplicantStaIfaceCallbackProxy when there's one existing");
                }
            } else {
                warn!("Empty callback field in received RegisterCallback request.")
            }
        }
        fidl_wlanix::SupplicantStaIfaceRequest::AddNetwork { payload, .. } => {
            let _ = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::AddNetwork",
                Arc::clone(&iface_manager),
                &iface_name,
            )
            .await?;
            if let Some(supplicant_sta_network) = payload.network {
                let supplicant_sta_network_stream = supplicant_sta_network.into_stream();
                // TODO(https://fxbug.dev/316035436): Should we return NetworkAdded event?
                serve_supplicant_sta_network(
                    telemetry_sender,
                    supplicant_sta_network_stream,
                    sta_iface_state,
                    state,
                    iface_manager,
                    power_manager,
                    iface_name,
                )
                .await;
            }
        }
        fidl_wlanix::SupplicantStaIfaceRequest::Disconnect { responder } => {
            let (iface, _) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::Disconnect",
                iface_manager,
                &iface_name,
            )
            .await?;
            if let Err(e) = iface.disconnect().await {
                warn!("iface.disconnect() error: {}", e);
            }
            if let Err(e) = responder.send() {
                warn!("Failed to send disconnect response: {}", e);
            }
        }
        fidl_wlanix::SupplicantStaIfaceRequest::GetMacAddress { responder } => {
            let (iface, iface_id) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::GetMacAddress",
                iface_manager,
                &iface_name,
            )
            .await?;
            let result = match iface.query().await {
                Ok(response) => Ok(fidl_wlanix::SupplicantStaIfaceGetMacAddressResponse {
                    mac_addr: Some(response.sta_addr),
                    ..Default::default()
                }),
                Err(e) => {
                    error!("Failed to query iface {}: {}", iface_id, e);
                    Err(zx::sys::ZX_ERR_INTERNAL)
                }
            };
            let result = result.as_ref().map_err(|status| *status);
            responder.send(result).context("send GetMacAddress response")?;
        }
        fidl_wlanix::SupplicantStaIfaceRequest::GetFactoryMacAddress { responder } => {
            let (iface, iface_id) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::GetFactoryMacAddress",
                iface_manager,
                &iface_name,
            )
            .await?;
            let result = match iface.query().await {
                Ok(response) => Ok(response.factory_addr),
                Err(e) => {
                    error!("Failed to query iface {}: {}", iface_id, e);
                    Err(zx::sys::ZX_ERR_INTERNAL)
                }
            };
            let result = result.as_ref().map_err(|status| *status);
            responder.send(result).context("send GetFactoryMacAddress response")?;
        }
        fidl_wlanix::SupplicantStaIfaceRequest::SetBtCoexistenceMode { payload, responder } => {
            let (iface, _) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::SetBtCoexistenceMode",
                iface_manager,
                &iface_name,
            )
            .await?;
            let result = match payload.mode {
                Some(mode) => {
                    use fidl_wlanix::BtCoexistenceMode as BtCoexMode;
                    let internal_mode = match mode {
                        BtCoexMode::Enabled => fidl_internal::BtCoexistenceMode::ModeAuto,
                        BtCoexMode::Disabled => fidl_internal::BtCoexistenceMode::ModeOff,
                        BtCoexMode::Sense => fidl_internal::BtCoexistenceMode::ModeAuto,
                        _ => {
                            warn!(
                                "Unrecognized BtCoexistenceMode: {:?}, defaulting to Enabled",
                                mode
                            );
                            fidl_internal::BtCoexistenceMode::ModeAuto
                        }
                    };
                    iface.set_bt_coexistence_mode(internal_mode).await
                }
                None => {
                    error!("Got SetBtCoexistenceMode without a payload");
                    Err(fidl_wlanix::WlanixError::InvalidArgs)
                }
            };
            if let Err(e) = responder.send(result) {
                warn!("Failed to send SetBtCoexistenceMode response: {}", e);
            }
        }
        fidl_wlanix::SupplicantStaIfaceRequest::SetPowerSave { payload, responder } => {
            let (iface, _) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::SetPowerSave",
                iface_manager,
                &iface_name,
            )
            .await?;
            match payload.enable {
                Some(enable) => match iface.set_power_save_mode(enable).await {
                    Ok(()) => info!("Set power save mode to {}", enable),
                    Err(e) => warn!("Failed to set power save mode: {:?}", e),
                },
                None => error!("Got SetPowerSave without a payload"),
            }
            if let Err(e) = responder.send() {
                warn!("Failed to send disconnect response: {}", e);
            }
        }
        fidl_wlanix::SupplicantStaIfaceRequest::SetSuspendModeEnabled { payload, responder } => {
            let (iface, _) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::SetSuspendModeEnabled",
                iface_manager,
                &iface_name,
            )
            .await?;
            match payload.enable {
                Some(enable) => match iface.set_suspend_mode(enable).await {
                    Ok(()) => info!("Set suspend mode to {}", enable),
                    Err(e) => warn!("Failed to set suspend mode: {:?}", e),
                },
                None => error!("Got SetSuspendModeEnabled without a payload"),
            }
            if let Err(e) = responder.send() {
                warn!("Failed to send disconnect response: {}", e);
            }
        }
        fidl_wlanix::SupplicantStaIfaceRequest::SetStaCountryCode { payload, responder } => {
            let (iface, _) = get_iface_and_log(
                "fidl_wlanix::SupplicantStaIfaceRequest::SetStaCountryCode",
                iface_manager,
                &iface_name,
            )
            .await?;
            let result = match payload.code {
                Some(code) => iface.set_country(code).await.map_err(|e| {
                    error!("Failed to set country code {:?} for iface: {}", code, e);
                    zx::sys::ZX_ERR_INTERNAL
                }),
                None => {
                    error!("SetStaCountryCode missing country code");
                    Err(zx::sys::ZX_ERR_INVALID_ARGS)
                }
            };
            responder.send(result).context("send SetStaCountryCode response")?;
        }
        fidl_wlanix::SupplicantStaIfaceRequest::GetSignalPollResults { responder } => {
            debug!(
                "fidl_wlanix::SupplicantStaIfaceRequest::GetSignalPollResults (iface {}",
                iface_name
            );
            let (iface, _) = get_iface(iface_manager).await?;
            let result = match iface.get_signal_report().await {
                Ok(report) => {
                    let (current_rssi_dbm, tx_bitrate_mbps, rx_bitrate_mbps, frequency_mhz) =
                        report
                            .connection_signal_report
                            .map(|conn| {
                                let tx_bitrate_mbps = conn.tx_rate_500kbps.unwrap_or(0) / 2;
                                // TODO(496331508): Rx rate is not sent up in get_signal_report yet.
                                let rx_bitrate_mbps = 0;
                                let current_rssi_dbm = conn.rssi_dbm.unwrap_or(0) as i32;

                                let frequency_mhz = match (conn.primary, conn.bandwidth) {
                                    (Some(c), Some(cbw)) => {
                                        let sec80 = conn.vht_secondary_80_channel.unwrap_or(
                                            fidl_ieee80211::ChannelNumber {
                                                number: 0,
                                                band: c.band,
                                            },
                                        );
                                        Channel::from_fidl(c, cbw, sec80)
                                            .map(|chan| chan.get_center_freq().unwrap_or(0) as u32)
                                            .unwrap_or(0)
                                    }
                                    _ => 0,
                                };
                                (current_rssi_dbm, tx_bitrate_mbps, rx_bitrate_mbps, frequency_mhz)
                            })
                            .unwrap_or((0, 0, 0, 0));
                    Ok(fidl_wlanix::SupplicantStaIfaceGetSignalPollResultsResponse {
                        current_rssi_dbm: Some(current_rssi_dbm),
                        tx_bitrate_mbps: Some(tx_bitrate_mbps),
                        rx_bitrate_mbps: Some(rx_bitrate_mbps),
                        frequency_mhz: Some(frequency_mhz),
                        ..Default::default()
                    })
                }
                Err(e) => {
                    // Log an error if RSSI is not available since it is core functionality.
                    log_throttler.lock().throttle_log(
                        format!("Failed to get signal report {}", e),
                        log::Level::Error,
                    );
                    Err(zx::sys::ZX_ERR_INTERNAL)
                }
            };
            responder
                .send(result.as_ref().map_err(|&e| e))
                .context("send GetSignalPollResults response")?;
        }
        fidl_wlanix::SupplicantStaIfaceRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown SupplicantStaIfaceRequest ordinal: {}", ordinal);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn serve_supplicant_sta_iface<I: IfaceManager, P: PowerManager>(
    telemetry_sender: TelemetrySender,
    reqs: fidl_wlanix::SupplicantStaIfaceRequestStream,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    iface_name: String,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
) {
    let sta_iface_state = Arc::new(Mutex::new(SupplicantStaIfaceState { callback: None }));
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) = handle_supplicant_sta_iface_request(
                    telemetry_sender.clone(),
                    req,
                    Arc::clone(&sta_iface_state),
                    Arc::clone(&state),
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    iface_name.clone(),
                    Arc::clone(&log_throttler),
                )
                .await
                {
                    warn!("Failed to handle SupplicantRequest: {}", e);
                }
            }
            Err(e) => {
                error!("SupplicantStaIface request stream failed: {}", e);
            }
        }
    })
    .await;
}

async fn handle_supplicant_request<I: IfaceManager, P: PowerManager>(
    req: fidl_wlanix::SupplicantRequest,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
    state: Arc<Mutex<WifiState>>,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::SupplicantRequest::AddStaInterface { payload, .. } => {
            info!("fidl_wlanix::SupplicantRequest::AddStaInterface");
            let iface_name = match payload.iface_name {
                Some(name) => name,
                None => bail!("No iface name in AddStaInterface"),
            };
            // TODO(b/299349496): Check that the iface name matches the request.
            if let Some(supplicant_sta_iface) = payload.iface {
                let ifaces = iface_manager.list_ifaces();
                if ifaces.is_empty() {
                    bail!("AddStaInterface but no interfaces exist.");
                } else {
                    info!("AddStaInterface: serving iface {}", ifaces[0]);
                    let _client_iface = iface_manager.get_client_iface(ifaces[0]).await?;
                    let supplicant_sta_iface_stream = supplicant_sta_iface.into_stream();
                    serve_supplicant_sta_iface(
                        telemetry_sender,
                        supplicant_sta_iface_stream,
                        state,
                        Arc::clone(&iface_manager),
                        Arc::clone(&power_manager),
                        iface_name.clone(),
                        Arc::clone(&log_throttler),
                    )
                    .await;
                }
            }
        }
        fidl_wlanix::SupplicantRequest::RemoveInterface { .. } => {
            info!("fidl_wlanix::SupplicantRequest::RemoveInterface");
            let ifaces = iface_manager.list_ifaces();
            if ifaces.is_empty() {
                bail!("RemoveInterface but no interfaces exist.");
            } else {
                // As a supplicant call, RemoveInterface implies that the interface should no
                // longer serve connections but does not actually destroy the interface. We
                // simulate this by tearing down any existing connection.
                let client_iface = iface_manager.get_client_iface(ifaces[0]).await?;
                if let Err(e) = client_iface.disconnect().await {
                    error!("Failed to disconnect on RemoveInterface: {e}");
                }
            }
        }
        fidl_wlanix::SupplicantRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown SupplicantRequest ordinal: {}", ordinal);
        }
    }
    Ok(())
}

async fn serve_supplicant<I: IfaceManager, P: PowerManager>(
    reqs: fidl_wlanix::SupplicantRequestStream,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
    state: Arc<Mutex<WifiState>>,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
) {
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) = handle_supplicant_request(
                    req,
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    telemetry_sender.clone(),
                    Arc::clone(&state),
                    Arc::clone(&log_throttler),
                )
                .await
                {
                    warn!("Failed to handle SupplicantRequest: {}", e);
                }
            }
            Err(e) => {
                error!("Supplicant request stream failed: {}", e);
            }
        }
    })
    .await;
}

fn nl80211_message_resp(messages: Vec<fidl_wlanix::Nl80211Message>) -> zx::Vmo {
    let output = fidl::persist(&fidl_wlanix::Nl80211MessageArray { messages }).unwrap();
    let vmo = zx::Vmo::create(output.len() as u64).unwrap();
    vmo.write(&output, 0).unwrap();
    vmo.set_content_size(&(output.len() as u64)).unwrap();
    vmo
}

fn build_nl80211_message(cmd: Nl80211Cmd, attrs: Vec<Nl80211Attr>) -> fidl_wlanix::Nl80211Message {
    let resp = GenlMessage::from_payload(Nl80211 { cmd, attrs });
    let mut buffer = vec![0u8; resp.buffer_len()];
    resp.serialize(&mut buffer);
    fidl_wlanix::Nl80211Message::Message(fidl_wlanix::Message { payload: buffer })
}

fn build_nl80211_ack() -> fidl_wlanix::Nl80211Message {
    fidl_wlanix::Nl80211Message::Ack(fidl_wlanix::Ack)
}

fn build_nl80211_err(error_code: zx::Status) -> fidl_wlanix::Nl80211Message {
    fidl_wlanix::Nl80211Message::Error(fidl_wlanix::Error { error_code: error_code.into_raw() })
}

fn build_nl80211_done() -> fidl_wlanix::Nl80211Message {
    fidl_wlanix::Nl80211Message::Done(fidl_wlanix::Done)
}

fn get_supported_frequencies() -> Vec<Vec<Nl80211FrequencyAttr>> {
    // TODO(b/316037008): Reevaluate this list later. This does not reflect
    // actual support. We should instead get supported frequencies from the phy.
    #[rustfmt::skip]
    let two_ghz_channels: Vec<fidl_ieee80211::ChannelNumber> = [
        // 2.4 GHz
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11
    ].iter().map(|c| fidl_ieee80211::ChannelNumber {
        band: WlanBand::TwoGhz,
        number: *c,
    }).collect();

    let five_ghz_channels: Vec<fidl_ieee80211::ChannelNumber> = [
        // 5 GHz
        36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
        149, 153, 157, 161, 165,
    ]
    .iter()
    .map(|c| fidl_ieee80211::ChannelNumber { band: WlanBand::FiveGhz, number: *c })
    .collect();

    let channels = [two_ghz_channels, five_ghz_channels].concat();

    channels
        .into_iter()
        .map(|channel_idx| {
            // We report the frequency of the beacon, which is always 20MHz on the primary channel.
            let freq = Channel::new(channel_idx.number, Bandwidth::Cbw20, channel_idx.band)
                .get_center_freq()
                .unwrap();
            vec![Nl80211FrequencyAttr::Frequency(freq.into())]
        })
        .collect()
}

trait MessageResponder {
    fn send(self, result: Result<Vec<fidl_wlanix::Nl80211Message>, i32>)
    -> Result<(), fidl::Error>;
}

impl MessageResponder for Nl80211MessageV2Responder {
    fn send(
        self,
        result: Result<Vec<fidl_fuchsia_wlan_wlanix::Nl80211Message>, i32>,
    ) -> Result<(), fidl::Error> {
        Nl80211MessageV2Responder::send(self, result.map(nl80211_message_resp))
    }
}

impl MessageResponder for Nl80211MessageResponder {
    fn send(
        self,
        result: Result<Vec<fidl_fuchsia_wlan_wlanix::Nl80211Message>, i32>,
    ) -> Result<(), fidl::Error> {
        Nl80211MessageResponder::send(
            self,
            result.map(|responses| Nl80211MessageResponse {
                responses: Some(responses),
                ..Default::default()
            }),
        )
    }
}

async fn handle_nl80211_message<I: IfaceManager>(
    netlink_message: fidl_wlanix::Nl80211Message,
    responder: WithDefaultDrop<impl MessageResponder + DefaultDrop + Send + 'static>,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    telemetry_sender: TelemetrySender,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
    scheduled_scan_controller: &Arc<ScheduledScanController>,
) -> Result<(), Error> {
    let payload = match netlink_message {
        fidl_wlanix::Nl80211Message::Message(m) => m.payload,
        _ => return Ok(()),
    };
    let deserialized = GenlMessage::<Nl80211>::deserialize(
        &NetlinkHeader::default(),
        &payload[..],
        EmptyDeserializeOptions,
    );
    let Ok(message) = deserialized else {
        responder
            .take()
            .send(Err(zx::sys::ZX_ERR_INTERNAL))
            .context("sending error status on failing to parse nl80211 message")?;
        bail!("Failed to parse nl80211 message: {}", deserialized.unwrap_err())
    };
    match message.payload.cmd {
        Nl80211Cmd::GetWiphy => {
            debug!("Nl80211Cmd::GetWiphy");
            let phys = iface_manager.list_phys().await?;
            let mut resp = vec![];
            for phy_id in phys {
                resp.push(build_nl80211_message(
                    Nl80211Cmd::NewWiphy,
                    vec![
                        // Phy ID
                        Nl80211Attr::Wiphy(phy_id as u32),
                        // Supported bands
                        Nl80211Attr::WiphyBands(vec![vec![Nl80211BandAttr::Frequencies(
                            get_supported_frequencies(),
                        )]]),
                        // Scan capabilities
                        Nl80211Attr::MaxScanSsids(32),
                        Nl80211Attr::MaxScheduledScanSsids(32),
                        Nl80211Attr::MaxMatchSets(32),
                        // Feature flags
                        Nl80211Attr::FeatureFlags(0),
                        Nl80211Attr::ExtendedFeatures(vec![]),
                    ],
                ));
            }
            responder.take().send(Ok(resp)).context("Failed to send NewWiphy")?;
        }
        Nl80211Cmd::GetInterface => {
            info!("Nl80211Cmd::GetInterface");
            let ifaces = iface_manager.list_ifaces();
            let mut resp = vec![];
            for iface in ifaces {
                let iface_info = iface_manager.query_iface(iface).await?;
                resp.push(build_nl80211_message(
                    Nl80211Cmd::NewInterface,
                    vec![
                        Nl80211Attr::Wiphy(iface_info.phy_id.into()),
                        Nl80211Attr::IfaceIndex(iface.into()),
                        Nl80211Attr::IfaceName(IFACE_NAME.to_string()),
                        Nl80211Attr::Mac(iface_info.sta_addr),
                    ],
                ));
            }
            resp.push(build_nl80211_done());
            responder.take().send(Ok(resp)).context("Failed to send scan results")?;
        }
        Nl80211Cmd::GetStation => {
            debug!("Nl80211Cmd::GetStation");
            // GetStation also has a MAC address attribute. We don't check whether it
            // matches the connected network BSSID and simply assume that it does.
            match get_client_iface_and_id(&message.payload.attrs[..], &iface_manager).await {
                Ok((client_iface, _)) => {
                    let (tx_packets, rx_packets, tx_failed) = client_iface
                        .get_iface_stats()
                        .await
                        .ok()
                        .and_then(|stats| stats.connection_stats)
                        .map(|conn| (conn.tx_total, conn.rx_unicast_total, conn.tx_drop))
                        .unwrap_or((None, None, None));

                    let (rssi, tx_rate) = client_iface
                        .get_signal_report()
                        .await
                        .ok()
                        .and_then(|report| report.connection_signal_report)
                        .map(|report| (report.rssi_dbm, report.tx_rate_500kbps.map(|r| r * 5)))
                        .unwrap_or((None, None));

                    // Signal, tx packets, and rx packets are expected to be present, so if they
                    // are missing use a default value. If the others are not available, do not
                    // include them.
                    let mut attrs = vec![
                        Nl80211StaInfoAttr::TxPackets(tx_packets.unwrap_or_else(|| {
                            log_throttler.lock().throttle_log(
                                "TxPackets missing in stats for GetStation".to_string(),
                                log::Level::Warn,
                            );
                            0
                        }) as u32),
                        Nl80211StaInfoAttr::RxPackets(rx_packets.unwrap_or_else(|| {
                            log_throttler.lock().throttle_log(
                                "RxPackets missing in stats for GetStation".to_string(),
                                log::Level::Warn,
                            );
                            0
                        }) as u32),
                        Nl80211StaInfoAttr::Signal(rssi.unwrap_or_else(|| {
                            log_throttler.lock().throttle_log(
                                "RSSI missing in signal report for GetStation".to_string(),
                                log::Level::Error,
                            );
                            INVALID_RSSI
                        })),
                    ];

                    if let Some(failed) = tx_failed {
                        attrs.push(Nl80211StaInfoAttr::TxFailed(failed as u32));
                    } else {
                        log_throttler.lock().throttle_log(
                            "TxFailed missing in stats for GetStation".to_string(),
                            log::Level::Warn,
                        );
                    }

                    if let Some(rate) = tx_rate {
                        attrs.push(Nl80211StaInfoAttr::TxBitrate(vec![
                            Nl80211RateInfoAttr::Bitrate32(rate),
                        ]));
                    } else {
                        log_throttler.lock().throttle_log(
                            "Tx rate missing in signal report for GetStation".to_string(),
                            log::Level::Warn,
                        );
                    }

                    responder
                        .take()
                        .send(Ok(vec![build_nl80211_message(
                            Nl80211Cmd::NewStation,
                            vec![Nl80211Attr::StaInfo(attrs)],
                        )]))
                        .context("Failed to send GetStation")?;
                }
                Err(e) => {
                    responder.take().send(Err(e)).context("sending error status for GetStation")?;
                    bail!("Could not get a client iface for GetStation")
                }
            }
        }
        Nl80211Cmd::GetProtocolFeatures => {
            info!("Nl80211Cmd::GetProtocolFeatures");
            responder
                .take()
                .send(Ok(vec![build_nl80211_message(
                    Nl80211Cmd::GetProtocolFeatures,
                    vec![Nl80211Attr::ProtocolFeatures(0)],
                )]))
                .context("Failed to send GetProtocolFeatures")?;
        }
        Nl80211Cmd::TriggerScan => {
            info!("Nl80211Cmd::TriggerScan, attrs={:?}", message.payload.attrs);
            match get_client_iface_and_id(&message.payload.attrs[..], &iface_manager).await {
                Ok((client_iface, iface_id)) => {
                    responder
                        .take()
                        .send(Ok(vec![build_nl80211_ack()]))
                        .context("Failed to ack TriggerScan")?;
                    telemetry_sender.send(TelemetryEvent::ScanStart);
                    match client_iface.trigger_scan(None, vec![]).await {
                        Ok(ScanEnd::Complete) => {
                            info!("Passive scan completed successfully");
                            telemetry_sender.send(TelemetryEvent::ScanResult {
                                result: wlan_telemetry::ScanResult::Complete {
                                    num_results: client_iface.get_last_scan_results().len(),
                                },
                            });
                            state.lock().scan_multicast_proxies.send_new_scan_results(iface_id);
                        }
                        Ok(ScanEnd::Cancelled) => {
                            info!("Passive scan terminated");
                            telemetry_sender.send(TelemetryEvent::ScanResult {
                                result: wlan_telemetry::ScanResult::Cancelled,
                            });
                            state.lock().scan_multicast_proxies.send_scan_aborted(iface_id);
                        }
                        Err(e) => {
                            error!("Failed to run passive scan: {:?}", e);
                            telemetry_sender.send(TelemetryEvent::ScanResult {
                                result: wlan_telemetry::ScanResult::Failed,
                            });
                            state.lock().scan_multicast_proxies.send_scan_aborted(iface_id);
                        }
                    }
                }
                Err(e) => {
                    responder
                        .take()
                        .send(Err(e))
                        .context("sending error status for TriggerScan")?;
                    bail!("Could not get a client iface for TriggerScan")
                }
            }
        }
        Nl80211Cmd::AbortScan => {
            info!("Nl80211Cmd::AbortScan");
            match get_client_iface_and_id(&message.payload.attrs[..], &iface_manager).await {
                Ok((client_iface, _)) => match client_iface.abort_scan().await {
                    Ok(()) => {
                        info!("Aborted scan successfully");
                        telemetry_sender.send(TelemetryEvent::ScanResult {
                            result: wlan_telemetry::ScanResult::Cancelled,
                        });
                        responder
                            .take()
                            .send(Ok(vec![build_nl80211_ack()]))
                            .context("Failed to ack AbortScan")?;
                    }
                    Err(e) => {
                        error!("Failed to abort scan: {:?}", e);
                        responder
                            .take()
                            .send(Ok(vec![build_nl80211_err(zx::Status::BAD_STATE)]))
                            .context("Failed to ack AbortScan")?;
                    }
                },
                Err(e) => {
                    responder.take().send(Err(e)).context("sending error status for AbortScan")?;
                    bail!("Could not get a client iface for AbortScan")
                }
            }
        }
        Nl80211Cmd::StartSchedScan => {
            info!("Nl80211Cmd::StartSchedScan");
            match get_client_iface_and_id(&message.payload.attrs[..], &iface_manager).await {
                Ok((client_iface, iface_id)) => {
                    let mut request = fidl_common::ScheduledScanRequest::default();
                    let mut default_interval = None;

                    for attr in &message.payload.attrs {
                        match attr {
                            Nl80211Attr::ScanSsids(s) => {
                                for ssid_bytes in s {
                                    request
                                        .ssids
                                        .get_or_insert_with(Vec::new)
                                        .push(ssid_bytes.clone());
                                }
                            }
                            Nl80211Attr::SchedScanPlans(plans) => {
                                let mut parsed_plans = Vec::new();
                                for plan in plans {
                                    let mut interval = None;
                                    let mut iterations = None;
                                    for plan_attr in plan {
                                        match plan_attr {
                                            Nl80211SchedScanPlanAttr::Interval(i) => {
                                                interval = Some(*i);
                                            }
                                            Nl80211SchedScanPlanAttr::Iterations(i) => {
                                                iterations = Some(*i);
                                            }
                                        }
                                    }
                                    if let (Some(interval), Some(iterations)) =
                                        (interval, iterations)
                                    {
                                        parsed_plans.push(fidl_common::ScheduledScanPlan {
                                            interval,
                                            iterations,
                                        });
                                    } else {
                                        warn!(
                                            "Scan plan attribute is missing interval or iterations, ignoring."
                                        );
                                    }
                                }
                                request.scan_plans = Some(parsed_plans);
                            }
                            Nl80211Attr::ScanFrequencies(f) => {
                                request.frequencies = Some(f.clone());
                            }
                            Nl80211Attr::SchedScanMatch(matches) => {
                                for match_set_attrs in matches {
                                    let mut match_set =
                                        fidl_common::ScheduledScanMatchSet::default();
                                    for match_attr in match_set_attrs {
                                        match match_attr {
                                            Nl80211SchedScanMatchAttr::Ssid(ssid_bytes)
                                                if !ssid_bytes.is_empty() =>
                                            {
                                                match_set.ssid = Some(ssid_bytes.clone());
                                            }
                                            Nl80211SchedScanMatchAttr::Rssi(val) => {
                                                match_set.min_rssi_threshold = Some(i8::try_from(*val).unwrap_or_else(|_| {
                                                    let clamped = (*val).clamp(i8::MIN.into(), i8::MAX.into()) as i8;
                                                    warn!("RSSI threshold {} unexpectedly out of range, clamping to {}", val, clamped);
                                                    clamped
                                                }));
                                            }
                                            Nl80211SchedScanMatchAttr::RelativeRssi(val) => {
                                                match_set.relative_rssi_threshold = Some(i8::try_from(*val).unwrap_or_else(|_| {
                                                    let clamped = (*val).clamp(i8::MIN.into(), i8::MAX.into()) as i8;
                                                    warn!("Relative RSSI threshold {} unexpectedly out of range, clamping to {}", val, clamped);
                                                    clamped
                                                }));
                                            }
                                            Nl80211SchedScanMatchAttr::RssiAdjust(val) => {
                                                let bytes = val.to_ne_bytes();
                                                let band = bytes[0];
                                                let rssi_adjustment = bytes[1] as i8;
                                                let wlan_band = match band {
                                                    0 => Some(fidl_ieee80211::WlanBand::TwoGhz),
                                                    1 => Some(fidl_ieee80211::WlanBand::FiveGhz),
                                                    _ => {
                                                        debug!(
                                                            "Unsupported band {} in RssiAdjust, ignoring attribute",
                                                            band
                                                        );
                                                        None
                                                    }
                                                };
                                                if let Some(band) = wlan_band {
                                                    match_set
                                                        .band_rssi_adjustments
                                                        .get_or_insert_with(Vec::new)
                                                        .push(fidl_common::BandRssiAdjustment {
                                                            band,
                                                            rssi_adjustment,
                                                        });
                                                }
                                            }
                                            _ => {
                                                debug!(
                                                    "Unsupported SchedScanMatchAttr: {:?}",
                                                    match_attr
                                                );
                                            }
                                        }
                                    }
                                    request.match_sets.get_or_insert_with(Vec::new).push(match_set);
                                }
                            }
                            Nl80211Attr::SchedScanInterval(interval) => {
                                // Unlike the interval provided in scheduled scan plans, this value
                                // is provided in milliseconds.
                                default_interval = Some(*interval / 1000);
                            }
                            Nl80211Attr::RelativeRssi(val) => {
                                request.relative_rssi_threshold = Some(i8::try_from(*val).unwrap_or_else(|_| {
                                    let clamped = (*val).clamp(i8::MIN.into(), i8::MAX.into()) as i8;
                                    warn!("Relative RSSI threshold {} unexpectedly out of range, clamping to {}", val, clamped);
                                    clamped
                                }));
                            }
                            Nl80211Attr::RssiAdjust(val) => {
                                let bytes = val.to_ne_bytes();
                                let band = bytes[0];
                                let rssi_adjustment = bytes[1] as i8;
                                let wlan_band = match band {
                                    0 => Some(fidl_ieee80211::WlanBand::TwoGhz),
                                    1 => Some(fidl_ieee80211::WlanBand::FiveGhz),
                                    _ => {
                                        debug!(
                                            "Unsupported band {} in RssiAdjust, ignoring attribute",
                                            band
                                        );
                                        None
                                    }
                                };
                                if let Some(band) = wlan_band {
                                    request
                                        .band_rssi_adjustments
                                        .get_or_insert_with(Vec::new)
                                        .push(fidl_common::BandRssiAdjustment {
                                            band,
                                            rssi_adjustment,
                                        });
                                }
                            }
                            Nl80211Attr::IfaceIndex(_) => {
                                // Already parsed via get_client_iface_and_id()
                            }
                            _ => {
                                warn!("Unhandled SchedScanAttr: {:?}", attr);
                            }
                        }
                    }

                    // Treat a top-level interval attribute as an indefinite scan plan if no plans were provided.
                    if let Some(interval) = default_interval {
                        request
                            .scan_plans
                            .get_or_insert_with(Vec::new)
                            .push(fidl_common::ScheduledScanPlan { interval, iterations: 0 });
                    }

                    if request.scan_plans.as_ref().is_none_or(|p| p.is_empty()) {
                        warn!(
                            "Received scheduled scan request without any scan plans or interval. Ignoring."
                        );
                        responder
                            .take()
                            .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                            .unwrap_or_else(|e| error!("Failed to ack: {:?}", e));
                        return Ok(());
                    }

                    match scheduled_scan_controller
                        .on_start_sched_scan(request.clone(), iface_id, client_iface.clone())
                        .await
                    {
                        Ok(_) => {
                            responder
                                .take()
                                .send(Ok(vec![build_nl80211_ack()]))
                                .context("Failed to ack StartSchedScan")?;
                        }
                        Err(e) => {
                            warn!("Error starting scheduled scan: {}", e);
                            responder
                                .take()
                                .send(Err(zx::sys::ZX_ERR_INTERNAL))
                                .context("Failed to ack StartSchedScan")?;
                        }
                    }
                }
                Err(e) => {
                    responder
                        .take()
                        .send(Err(e))
                        .context("sending error status for StartSchedScan")?;
                    bail!("Could not get a client iface for StartSchedScan")
                }
            }
        }
        Nl80211Cmd::StopSchedScan => {
            info!("Nl80211Cmd::StopSchedScan");
            match get_client_iface_and_id(&message.payload.attrs[..], &iface_manager).await {
                Ok((_, iface_id)) => {
                    scheduled_scan_controller.on_stop_sched_scan(iface_id).await;
                    info!("Stopped scheduled scan successfully for iface {}", iface_id);
                    responder
                        .take()
                        .send(Ok(vec![build_nl80211_ack()]))
                        .context("Failed to ack StopSchedScan")?;
                }
                Err(e) => {
                    responder
                        .take()
                        .send(Err(e))
                        .context("sending error status for StopSchedScan")?;
                    bail!("Could not get a client iface for StopSchedScan")
                }
            }
        }
        Nl80211Cmd::GetScan => {
            info!("Nl80211Cmd::GetScan");
            match get_client_iface_and_id(&message.payload.attrs[..], &iface_manager).await {
                Ok((client_iface, iface_id)) => {
                    let results = client_iface.get_last_scan_results();
                    info!("Processing {} scan results", results.len());
                    let connected_bssid =
                        client_iface.get_connected_network().map(|network| network.bssid);
                    let mut resp = vec![];
                    for result in results {
                        let is_associated = connected_bssid
                            .is_some_and(|bssid| *bssid.as_array() == result.bss_description.bssid);

                        match convert_scan_result(result, is_associated) {
                            Ok(scan_result) => {
                                resp.push(build_nl80211_message(
                                    Nl80211Cmd::NewScanResults,
                                    vec![Nl80211Attr::IfaceIndex(iface_id), scan_result],
                                ));
                            }
                            Err(e) => {
                                error!("Skipping scan result that failed to convert: {}", e);
                            }
                        }
                    }
                    resp.push(build_nl80211_done());
                    responder.take().send(Ok(resp)).context("Failed to send scan results")?;
                }
                Err(e) => {
                    responder.take().send(Err(e)).context("sending error status for GetScan")?;
                    bail!("Could not get a client iface for GetScan");
                }
            }
        }
        Nl80211Cmd::GetReg => {
            info!("Nl80211Cmd::GetReg");
            match find_phy_id(&message.payload.attrs[..]) {
                Some(phy_id) => match iface_manager.get_country(phy_id.try_into()?).await {
                    Ok(mut country) => {
                        // Fuchsia has used "WW" by convention, but the more broadly accepted value
                        // for worldwide is "00".  Report that instead.
                        if country == *b"WW" {
                            country = *b"00";
                            info!("Converting country code from WW to 00 for GetReg response.");
                        }

                        let resp = build_nl80211_message(
                            Nl80211Cmd::GetReg,
                            vec![Nl80211Attr::RegulatoryRegionAlpha2(country)],
                        );
                        responder
                            .take()
                            .send(Ok(vec![resp]))
                            .context("Failed to respond to GetReg")?;
                    }
                    Err(e) => {
                        error!("Failed to get regulatory region from phy: {:?}", e);
                        responder
                            .take()
                            .send(Err(zx::sys::ZX_ERR_INTERNAL))
                            .context("Failed to respond to GetReg with error")?;
                    }
                },
                None => {
                    responder
                        .take()
                        .send(Err(zx::sys::ZX_ERR_INVALID_ARGS))
                        .context("sending error status due to missing iface id on GetReg")?;
                    bail!("GetReg did not include a phy id");
                }
            }
        }
        _ => {
            warn!("Dropping nl80211 message: {:?}", message);
            responder.take().send(Ok(vec![])).context("Failed to respond to unhandled message")?;
        }
    }
    Ok(())
}

impl DefaultDrop for fidl_wlanix::Nl80211MessageResponder {
    fn default_drop(self) {
        error!("Dropped Nl80211MessageResponder without responding.");
        if let Err(e) = self.send(Err(zx::sys::ZX_ERR_INTERNAL)) {
            error!("Failed to send internal error response: {}", e);
        }
    }
}

impl DefaultDrop for fidl_wlanix::Nl80211MessageV2Responder {
    fn default_drop(self) {
        error!("Dropped Nl80211MessageV2Responder without responding.");
        if let Err(e) = self.send(Err(zx::sys::ZX_ERR_INTERNAL)) {
            error!("Failed to send internal error response: {}", e);
        }
    }
}

// Convert the scan result to nl80211 attribute.
//
// If the device is associated to the BSS in this scan result, pass `is_associated = true`,
// otherwise, pass `false`. The |is_associated| flag is used to compute the BSS status attribute
// value within the nl80211 attribute returned by this function.
// Returns an error if there is an issue getting the center frequency of the channel.
fn convert_scan_result(
    result: fidl_sme::ScanResult,
    is_associated: bool,
) -> Result<Nl80211Attr, Error> {
    use crate::nl80211::{ChainSignalAttr, Nl80211BssAttr, Nl80211BssStatus};

    // Explicitly set cbw20 to find the center frequency of the primary channel.
    let channel = Channel::new(
        result.bss_description.primary.number,
        Bandwidth::Cbw20,
        result.bss_description.primary.band,
    );
    let center_freq = match channel.get_center_freq() {
        Ok(freq) => freq.into(),
        Err(e) => {
            return Err(format_err!("Failed to get center frequency for scan result: {}", e));
        }
    };
    // If the device is connected to this BSS, upstream expects the associated status to be
    // indicated in the scan result. Otherwise it won't signal poll. See b/473850388#comment10
    let bss_status = if is_associated {
        Nl80211BssStatus::Associated
    } else {
        Nl80211BssStatus::NotAuthenticated
    };
    Ok(Nl80211Attr::Bss(vec![
        Nl80211BssAttr::Bssid(result.bss_description.bssid),
        Nl80211BssAttr::Frequency(center_freq),
        Nl80211BssAttr::InformationElement(result.bss_description.ies),
        Nl80211BssAttr::LastSeenBoottime(fasync::BootInstant::now().into_nanos() as u64),
        Nl80211BssAttr::SignalMbm(result.bss_description.rssi_dbm as i32 * 100),
        Nl80211BssAttr::Capability(result.bss_description.capability_info),
        Nl80211BssAttr::Status(bss_status),
        // TODO(b/316038074): Determine whether we should provide real chain signals.
        Nl80211BssAttr::ChainSignal(vec![ChainSignalAttr {
            id: 0,
            rssi: result.bss_description.rssi_dbm,
        }]),
    ]))
}

fn find_phy_id(attrs: &[Nl80211Attr]) -> Option<u32> {
    attrs
        .iter()
        .filter_map(|attr| match attr {
            Nl80211Attr::Wiphy(idx) => Some(idx),
            _ => None,
        })
        .cloned()
        .next()
}

fn find_iface_id(attrs: &[Nl80211Attr]) -> Option<u32> {
    attrs
        .iter()
        .filter_map(|attr| match attr {
            Nl80211Attr::IfaceIndex(idx) => Some(idx),
            _ => None,
        })
        .cloned()
        .next()
}

async fn get_client_iface_and_id<I: IfaceManager>(
    attrs: &[Nl80211Attr],
    iface_manager: &Arc<I>,
) -> Result<(Arc<I::Client>, u32), i32> {
    match find_iface_id(attrs) {
        Some(iface_id) => {
            let iface_id_u16 = u16::try_from(iface_id).map_err(|_| zx::sys::ZX_ERR_BAD_STATE)?;
            iface_manager
                .get_client_iface(iface_id_u16)
                .await
                .map(|iface| (iface, iface_id))
                .map_err(|_| zx::sys::ZX_ERR_NOT_FOUND)
        }
        None => Err(zx::sys::ZX_ERR_INVALID_ARGS),
    }
}

async fn handle_nl80211_request<I: IfaceManager>(
    req: fidl_wlanix::Nl80211Request,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    telemetry_sender: TelemetrySender,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
    scheduled_scan_controller: &Arc<ScheduledScanController>,
) {
    match req {
        fidl_wlanix::Nl80211Request::MessageV2 { message, responder } => {
            if let Err(e) = handle_nl80211_message(
                message,
                WithDefaultDrop::new(responder),
                Arc::clone(&state),
                Arc::clone(&iface_manager),
                telemetry_sender.clone(),
                Arc::clone(&log_throttler),
                scheduled_scan_controller,
            )
            .await
            {
                error!("Failed to handle Nl80211 message: {}", e);
            }
        }
        fidl_wlanix::Nl80211Request::Message { payload, responder } => {
            if let Err(e) = handle_nl80211_message(
                payload.message.unwrap(),
                WithDefaultDrop::new(responder),
                Arc::clone(&state),
                Arc::clone(&iface_manager),
                telemetry_sender.clone(),
                Arc::clone(&log_throttler),
                scheduled_scan_controller,
            )
            .await
            {
                error!("Failed to handle Nl80211 message: {}", e);
            }
        }
        fidl_wlanix::Nl80211Request::GetMulticast { payload, .. } => {
            if let Some(multicast) = payload.multicast {
                let mut state = state.lock();
                if payload.group == Some("scan".to_string()) {
                    state.scan_multicast_proxies.add_proxy(multicast.into_proxy());
                } else if payload.group == Some("mlme".to_string()) {
                    state.mlme_multicast_proxies.add_proxy(multicast.into_proxy());
                } else {
                    warn!("Dropping channel for unsupported multicast group {:?}", payload.group);
                }
            }
        }
        fidl_wlanix::Nl80211Request::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown Nl80211Request ordinal: {}", ordinal);
        }
    }
}

async fn serve_nl80211<I: IfaceManager>(
    reqs: fidl_wlanix::Nl80211RequestStream,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    telemetry_sender: TelemetrySender,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
    scheduled_scan_controller: Arc<ScheduledScanController>,
) {
    reqs.for_each_concurrent(None, async |req| match req {
        Ok(req) => {
            handle_nl80211_request(
                req,
                Arc::clone(&state),
                Arc::clone(&iface_manager),
                telemetry_sender.clone(),
                Arc::clone(&log_throttler),
                &scheduled_scan_controller,
            )
            .await;
        }
        Err(e) => {
            error!("Nl80211 request stream failed: {}", e);
        }
    })
    .await;

    warn!("Nl80211 stream terminated. Should only happen during shutdown.");
}

fn legacy_hal_tx_power_scenario_to_internal(
    scenario: fidl_wlanix::WifiLegacyHalTxPowerScenario,
) -> Option<fidl_internal::TxPowerScenario> {
    match scenario {
        // If the caller provides an explicitly invalid SAR scenario, do not attempt to use it.
        fidl_wlanix::WifiLegacyHalTxPowerScenario::Invalid => None,

        // Default and VoiceCall map directly to SAR scenario definitions.
        fidl_wlanix::WifiLegacyHalTxPowerScenario::Default => {
            Some(fidl_internal::TxPowerScenario::Default)
        }
        fidl_wlanix::WifiLegacyHalTxPowerScenario::VoiceCallLegacy => {
            Some(fidl_internal::TxPowerScenario::VoiceCall)
        }

        // These scenarios represent situations where the device is detected to be near the body
        // AND EITHER
        //   a. Cell is explicitly *OFF*
        //   b. Cell is assumed to be off due to lack of hotspot or cell presence
        fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOff
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOffUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOffUnfoldedCap
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyRearCamera
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyVideoRecording => {
            Some(fidl_internal::TxPowerScenario::BodyCellOff)
        }

        // These scenarios represent situations where
        // 1. The device is detected to be near the body
        // 2. No cell activity detected
        // 3. There is an ongoing BT stream
        fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyBt
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyBtUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyBtUnfoldedCap => {
            Some(fidl_internal::TxPowerScenario::BodyBtActive)
        }

        // These scenarios represent situations where
        // 1. The device is detected to be near the body
        // 2. Cell is explicitly enabled
        fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOn
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnUnfoldedCap
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnBt
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnBtUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnBtUnfoldedCap
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspot
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBt
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotMmw
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBtMmw
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBtUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotMmwUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBtMmwUnfolded => {
            Some(fidl_internal::TxPowerScenario::BodyCellOn)
        }

        // These scenarios represent situations where
        // 1. The device is detected to be near the head
        // 2. Cell is explicitly enabled
        fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOn
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOnUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspot
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspotMmw
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspotUnfolded
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspotMmwUnfolded => {
            Some(fidl_internal::TxPowerScenario::HeadCellOn)
        }

        // These scenarios represent situations where the device is detected to be near the head
        // AND Cell is explicitly off.
        fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOff
        | fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOffUnfolded => {
            Some(fidl_internal::TxPowerScenario::HeadCellOff)
        }
        other => {
            warn!("Invalid power scenario: {:?}", other);
            None
        }
    }
}

async fn select_tx_power_scenario<I: IfaceManager>(
    iface_manager: Arc<I>,
    req: WifiLegacyHalSelectTxPowerScenarioRequest,
    responder: WifiLegacyHalSelectTxPowerScenarioResponder,
) -> Result<(), Error> {
    // The incoming request must a TxPowerScenario.
    let scenario = match req.scenario {
        Some(scenario) => scenario,
        None => {
            responder
                .send(Err(WifiLegacyHalStatus::InvalidArgument))
                .context("Invalid arguments for setting Tx power scenario")?;
            return Err(format_err!("Tx power scenario ({:?}) must be defined", req.scenario));
        }
    };

    // Ensure that the requested power scenario is supported.
    let scenario = match legacy_hal_tx_power_scenario_to_internal(scenario) {
        Some(scenario) => scenario,
        None => {
            responder
                .send(Err(WifiLegacyHalStatus::InvalidArgument))
                .context("Invalid power scenario")?;
            return Err(format_err!("Invalid power scenario: {:?}", scenario));
        }
    };

    // Determine the list of PHYs that need to have their power scenarios set.
    let phys = match iface_manager.list_phys().await {
        Ok(phys) => phys,
        Err(e) => {
            responder.send(Err(WifiLegacyHalStatus::Internal)).context("Unable to list PHYs")?;
            return Err(format_err!("Could not list PHYs: {}", e));
        }
    };

    if phys.is_empty() {
        responder.send(Err(WifiLegacyHalStatus::Internal)).context("No PHYs available")?;
        return Err(format_err!("No PHYs available for TxPowerScenario selection"));
    }

    // Set the requested Tx power scenario.
    let mut response = Ok(());
    let mut result = Ok(());
    for phy_id in phys {
        if let Err(e) = iface_manager.set_tx_power_scenario(phy_id, scenario).await {
            warn!("PHY {}: Failed to select Tx power scenario: {:?}", phy_id, e);
            response = Err(WifiLegacyHalStatus::Internal);
            result = Err(format_err!("Could not apply power scenario {:?} to all PHYs", scenario));
        }
    }

    responder.send(response).context("Set Tx power scenario")?;
    result
}

async fn reset_tx_power_scenario<I: IfaceManager>(
    iface_manager: Arc<I>,
    responder: WifiLegacyHalResetTxPowerScenarioResponder,
) -> Result<(), Error> {
    let phys = match iface_manager.list_phys().await {
        Ok(phys) => phys,
        Err(e) => {
            responder.send(Err(WifiLegacyHalStatus::Internal)).context("Unable to list PHYs")?;
            return Err(format_err!("Could not list PHYs: {}", e));
        }
    };

    if phys.is_empty() {
        responder.send(Err(WifiLegacyHalStatus::Internal)).context("No PHYs available")?;
        return Err(format_err!("No PHYs available for TxPowerScenario reset"));
    }

    let mut response = Ok(());
    let mut result = Ok(());
    for phy_id in phys {
        if let Err(e) = iface_manager.reset_tx_power_scenario(phy_id).await {
            warn!("Failed to reset Tx power scenario for PHY {}: {}", phy_id, e);
            result = Err(format_err!("Could not reset Tx power scenario on all PHYs"));
            response = Err(WifiLegacyHalStatus::Internal);
        }
    }

    responder.send(response).context("Reset Tx power scenario")?;
    result
}

async fn handle_wifi_legacy_hal_request<I: IfaceManager>(
    req: fidl_wlanix::WifiLegacyHalRequest,
    iface_manager: Arc<I>,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::WifiLegacyHalRequest::SelectTxPowerScenario { payload, responder } => {
            select_tx_power_scenario(iface_manager, payload, responder).await
        }
        fidl_wlanix::WifiLegacyHalRequest::ResetTxPowerScenario { responder } => {
            reset_tx_power_scenario(iface_manager, responder).await
        }
        other => Err(format_err!("Unsupported legacy HAL request: {:?}", other)),
    }
}

async fn serve_wifi_legacy_hal_requests<I: IfaceManager>(
    reqs: fidl_wlanix::WifiLegacyHalRequestStream,
    iface_manager: Arc<I>,
) {
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) =
                    handle_wifi_legacy_hal_request(req, Arc::clone(&iface_manager)).await
                {
                    warn!("Failed to handle WifiLegacyHalRequest: {}", e);
                }
            }
            Err(e) => {
                error!("WifiLegacyHal request stream failed: {}", e);
            }
        }
    })
    .await;
}

async fn handle_wlanix_request<I: IfaceManager, P: PowerManager>(
    req: fidl_wlanix::WlanixRequest,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
    scheduled_scan_controller: Arc<ScheduledScanController>,
) -> Result<(), Error> {
    match req {
        fidl_wlanix::WlanixRequest::GetWifi { payload, .. } => {
            info!("fidl_wlanix::WlanixRequest::GetWifi");
            if let Some(wifi) = payload.wifi {
                let wifi_stream = wifi.into_stream();
                serve_wifi(
                    wifi_stream,
                    Arc::clone(&state),
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    telemetry_sender,
                )
                .await;
            }
        }
        fidl_wlanix::WlanixRequest::GetSupplicant { payload, .. } => {
            info!("fidl_wlanix::WlanixRequest::GetSupplicant");
            if let Some(supplicant) = payload.supplicant {
                let supplicant_stream = supplicant.into_stream();
                serve_supplicant(
                    supplicant_stream,
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    telemetry_sender,
                    Arc::clone(&state),
                    Arc::clone(&log_throttler),
                )
                .await;
            }
        }
        fidl_wlanix::WlanixRequest::GetNl80211 { payload, .. } => {
            info!("fidl_wlanix::WlanixRequest::GetNl80211");
            if let Some(nl80211) = payload.nl80211 {
                let nl80211_stream = nl80211.into_stream();
                serve_nl80211(
                    nl80211_stream,
                    Arc::clone(&state),
                    Arc::clone(&iface_manager),
                    telemetry_sender,
                    Arc::clone(&log_throttler),
                    Arc::clone(&scheduled_scan_controller),
                )
                .await;
            }
        }
        fidl_wlanix::WlanixRequest::GetWifiLegacyHal { payload, .. } => {
            info!("fidl_wlanix::WlanixRequest::GetWifiLegacyHal");
            if let Some(legacy_hal) = payload.legacy_hal {
                let legacy_hal_stream = legacy_hal.into_stream();
                serve_wifi_legacy_hal_requests(legacy_hal_stream, Arc::clone(&iface_manager)).await;
            }
        }
        fidl_wlanix::WlanixRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Unknown WlanixRequest ordinal: {}", ordinal);
        }
    }
    Ok(())
}

async fn serve_wlanix<I: IfaceManager, P: PowerManager>(
    reqs: fidl_wlanix::WlanixRequestStream,
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
    scheduled_scan_controller: Arc<ScheduledScanController>,
) {
    reqs.for_each_concurrent(None, |req| async {
        match req {
            Ok(req) => {
                if let Err(e) = handle_wlanix_request(
                    req,
                    Arc::clone(&state),
                    Arc::clone(&iface_manager),
                    Arc::clone(&power_manager),
                    telemetry_sender.clone(),
                    Arc::clone(&log_throttler),
                    Arc::clone(&scheduled_scan_controller),
                )
                .await
                {
                    warn!("Failed to handle WlanixRequest: {}", e);
                }
            }
            Err(e) => {
                error!("Wlanix request stream failed: {}", e);
            }
        }
    })
    .await;
}

async fn serve_fidl<I: IfaceManager, P: PowerManager>(
    state: Arc<Mutex<WifiState>>,
    iface_manager: Arc<I>,
    power_manager: Arc<P>,
    telemetry_sender: TelemetrySender,
    log_throttler: Arc<Mutex<ThrottledErrorLogger>>,
    scheduled_scan_controller: Arc<ScheduledScanController>,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    let _inspect_server_task = inspect_runtime::publish(
        fuchsia_inspect::component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    let _ = fs.dir("svc").add_fidl_service(move |reqs| {
        serve_wlanix(
            reqs,
            Arc::clone(&state),
            Arc::clone(&iface_manager),
            Arc::clone(&power_manager),
            telemetry_sender.clone(),
            Arc::clone(&log_throttler),
            Arc::clone(&scheduled_scan_controller),
        )
    });
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(None, |t| t).await;
    Ok(())
}

async fn report_battery_updates(
    state: Arc<Mutex<WifiState>>,
    telemetry_sender: TelemetrySender,
    scheduled_scan_controller: Arc<ScheduledScanController>,
) {
    match client::connect_to_protocol::<fidl_fuchsia_power_battery::BatteryManagerMarker>() {
        Ok(proxy) => {
            // Swallow and log failure to watch battery updates because they only affect some
            // Cobalt metrics and should not cause wlanix to shutdown.
            if let Err(e) = report_battery_updates_helper(
                proxy,
                Arc::clone(&state),
                telemetry_sender,
                scheduled_scan_controller,
            )
            .await
            {
                warn!("Failed to watch and report battery updates to telemetry: {:?}", e);
            }
        }
        Err(e) => {
            warn!("Failed to connect to BatteryManager for telemetry: {:?}", e);
        }
    };
}

fn is_charging(charge_info: &fidl_fuchsia_power_battery::BatteryInfo) -> bool {
    charge_info.charge_status == Some(fidl_fuchsia_power_battery::ChargeStatus::Charging)
        || (charge_info.charge_status == Some(fidl_fuchsia_power_battery::ChargeStatus::Full)
            && charge_info.charge_source != Some(fidl_fuchsia_power_battery::ChargeSource::None))
}

async fn report_battery_updates_helper(
    proxy: fidl_fuchsia_power_battery::BatteryManagerProxy,
    _state: Arc<Mutex<WifiState>>,
    telemetry_sender: TelemetrySender,
    scheduled_scan_controller: Arc<ScheduledScanController>,
) -> Result<(), Error> {
    let (battery_watcher_client_end, mut battery_watcher_stream) =
        fidl::endpoints::create_request_stream();
    proxy.watch(battery_watcher_client_end)?;

    // Send initial charge status to telemetry and scheduled scan controller
    let info = proxy.get_battery_info().await?;
    match info.charge_status {
        Some(charge_status) => {
            scheduled_scan_controller.set_charging_state(is_charging(&info));
            telemetry_sender.send(TelemetryEvent::BatteryChargeStatus(charge_status));
        }
        None => warn!("Battery info not sent to telemetry because it doesn't have charge_status"),
    }

    // Watch for battery charge status changes
    while let Some(event) = battery_watcher_stream.next().await {
        match event? {
            fidl_fuchsia_power_battery::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                info,
                responder,
                ..
            } => {
                scheduled_scan_controller.set_charging_state(is_charging(&info));
                if let Some(charge_status) = info.charge_status {
                    telemetry_sender.send(TelemetryEvent::BatteryChargeStatus(charge_status));
                }
                responder.send()?;
            }
        }
    }

    Ok(())
}

async fn serve_phy_events(
    phy_events: fidl_device_service::PhyEventWatcherProxy,
    state: Arc<Mutex<WifiState>>,
) -> Result<(), Error> {
    let mut event_stream = phy_events.take_event_stream();
    while let Some(Ok(event)) = event_stream.next().await {
        match event {
            fidl_device_service::PhyEventWatcherEvent::OnCriticalError { phy_id, reason_code } => {
                error!("Critical error on phy {}! ({:?})", phy_id, reason_code);
                let mut state = state.lock();
                let status = match reason_code {
                    fidl_internal::CriticalErrorReason::FwCrash => zx::sys::ZX_ERR_INTERNAL,
                };
                maybe_run_callback(
                    "WifiEventCallback::OnSubsystemRestart",
                    |callback_proxy| {
                        callback_proxy.on_subsystem_restart(
                            fidl_wlanix::WifiEventCallbackOnSubsystemRestartRequest {
                                status: Some(status),
                                ..Default::default()
                            },
                        )
                    },
                    &mut state.callback,
                );
            }
            other => {
                warn!("Unknown phy event: {:?}", other);
            }
        }
    }
    bail!("phy event stream terminated");
}

async fn handle_scheduled_scan_events(
    state: Arc<Mutex<WifiState>>,
    mut receiver: mpsc::UnboundedReceiver<scheduled_scans::ScheduledScanEvent>,
) {
    use scheduled_scans::ScheduledScanEvent;
    while let Some(event) = receiver.next().await {
        match event {
            ScheduledScanEvent::ResultsAvailable { iface_id } => {
                state.lock().scan_multicast_proxies.send_sched_scan_results(iface_id);
            }
            ScheduledScanEvent::Stopped { iface_id } => {
                state.lock().scan_multicast_proxies.send_sched_scan_stopped(iface_id);
            }
        }
    }
    info!("Scheduled scan event stream terminated");
}

async fn serve_suspend_blocker<P: PowerManager>(
    state: Arc<Mutex<WifiState>>,
    power_manager: Arc<P>,
    mut requests: fsystem::SuspendBlockerRequestStream,
) -> Result<(), Error> {
    while let Some(req) = requests.next().await {
        match req {
            Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                info!("Modifying WLAN driver lease to 'suspend' level");
                let tokens: Vec<(u16, fidl_fuchsia_power_broker::DependencyToken)> = {
                    let state_lock = state.lock();
                    state_lock
                        .power_dependency_leases
                        .iter()
                        .map(|(id, token, _)| {
                            (*id, token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                        })
                        .collect()
                };

                let mut new_leases = Vec::new();
                for (phy_id, token) in tokens {
                    let lease_name = format!("wlanix-phy-{}-dependency-suspend", phy_id);
                    match power_manager
                        .power_element_lease(&lease_name, token, POWER_LEVEL_SUSPEND)
                        .await
                    {
                        Ok(lease) => new_leases.push((phy_id, lease)),
                        Err(e) => error!("Failed to get suspend lease for phy {}: {}", phy_id, e),
                    }
                }

                {
                    let mut state_lock = state.lock();
                    for (phy_id, lease) in new_leases {
                        if let Some(pos) = state_lock
                            .power_dependency_leases
                            .iter()
                            .position(|(id, _, _)| *id == phy_id)
                        {
                            state_lock.power_dependency_leases[pos].2 = lease;
                        }
                    }
                }

                if let Err(e) = responder.send() {
                    warn!("Failed to respond to BeforeSuspend: {}", e);
                }
            }
            Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                info!("Modifying WLAN driver lease to 'on' level");
                let tokens: Vec<(u16, fidl_fuchsia_power_broker::DependencyToken)> = {
                    let state_lock = state.lock();
                    state_lock
                        .power_dependency_leases
                        .iter()
                        .map(|(id, token, _)| {
                            (*id, token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                        })
                        .collect()
                };

                let mut new_leases = Vec::new();
                for (phy_id, token) in tokens {
                    let lease_name = format!("wlanix-phy-{}-dependency", phy_id);
                    match power_manager
                        .power_element_lease(&lease_name, token, POWER_LEVEL_ACTIVE)
                        .await
                    {
                        Ok(lease) => new_leases.push((phy_id, lease)),
                        Err(e) => error!("Failed to get active lease for phy {}: {}", phy_id, e),
                    }
                }

                {
                    let mut state_lock = state.lock();
                    for (phy_id, lease) in new_leases {
                        if let Some(pos) = state_lock
                            .power_dependency_leases
                            .iter()
                            .position(|(id, _, _)| *id == phy_id)
                        {
                            state_lock.power_dependency_leases[pos].2 = lease;
                        }
                    }
                }

                if let Err(e) = responder.send() {
                    warn!("Failed to respond to AfterResume: {}", e);
                }
            }
            Ok(fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. }) => {
                warn!("Unknown SuspendBlocker method: {}", ordinal);
            }
            Err(e) => {
                error!("SuspendBlocker request stream error: {}", e);
                return Err(e.into());
            }
        }
    }

    // The suspend blocker stream dropping means ActivityGovernor disconnected.
    // Return an error to stop wlanix so it can be restarted to reconnect to power management.
    Err(anyhow::anyhow!("SuspendBlocker request stream terminated unexpectedly"))
}
#[fuchsia::main(logging = false)]
async fn main() {
    trace_provider::trace_provider_create_with_fdio();
    diagnostics_log::initialize(
        diagnostics_log::PublishOptions::default()
            .tags(&["wlan", "wlanix"])
            .enable_metatag(diagnostics_log::Metatag::Target),
    )
    .expect("Failed to initialize wlanix logs");
    let config = wlanix_config::Config::take_from_startup_handle();
    info!("Starting Wlanix (power_framework_enabled: {})", config.power_framework_enabled);

    // Register for suspend/resume events
    let proxies = if config.power_framework_enabled {
        match (
            client::connect_to_protocol::<fsystem::ActivityGovernorMarker>(),
            client::connect_to_protocol::<fidl_fuchsia_power_broker::TopologyMarker>(),
        ) {
            (Ok(ag), Ok(pb)) => Some((ag, pb)),
            (ag_res, pb_res) => {
                if let Err(e) = ag_res {
                    warn!("Failed to connect to fuchsia.power.system.ActivityGovernor: {:?}", e);
                }
                if let Err(e) = pb_res {
                    warn!("Failed to connect to fuchsia.power.broker.Topology: {:?}", e);
                }
                None
            }
        }
    } else {
        None
    };
    let power_manager = Arc::new(DevicePowerManager::new(proxies));
    let (suspend_blocker_lease, suspend_blocker_requests) = if config.power_framework_enabled {
        let (suspend_blocker_client, suspend_blocker_requests) =
            fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();
        // The registration returns a wake lease, which we can hold until we're done with init
        let lease = match power_manager
            .register_suspend_blocker(suspend_blocker_client, "wlanix-suspend-blocker")
            .await
        {
            Ok(lease) => Some(lease),
            Err(e) => {
                warn!("Failed to register suspend blocker: {:?}", e);
                None
            }
        };
        (lease, Some(suspend_blocker_requests))
    } else {
        (None, None)
    };

    let monitor_svc = client::connect_to_protocol::<fidl_device_service::DeviceMonitorMarker>()
        .expect("failed to connect to device monitor");

    // Setup phy event processing
    let (phy_events_proxy, phy_events_server) =
        fidl::endpoints::create_proxy::<fidl_device_service::PhyEventWatcherMarker>();
    monitor_svc.watch_phy_events(phy_events_server).expect("Failed to watch phy events");

    // Setup telemetry module
    let cobalt_logger = wlan_telemetry::setup_cobalt_proxy().await.unwrap_or_else(|err| {
        error!("Cobalt service unavailable, will discard all metrics: {}", err);
        // This will only happen if Cobalt is very broken in some way. Using the disconnected
        // proxy will result in log spam as metrics fail to send, but it's preferable to run rather
        // than panicking, so the user still has the option of wireless connectivity (e.g. to OTA).
        wlan_telemetry::setup_disconnected_cobalt_proxy()
            .expect("Failed to create any FIDL channels, panicking")
    });
    const CLIENT_STATS_NODE_NAME: &str = "client_stats";
    let (telemetry_sender, serve_telemetry_fut) = wlan_telemetry::serve_telemetry(
        cobalt_logger,
        monitor_svc.clone(),
        fuchsia_inspect::component::inspector().root().create_child(CLIENT_STATS_NODE_NAME),
        &format!("root/{CLIENT_STATS_NODE_NAME}"),
        wlan_telemetry::TelemetryConfig {
            enable_connect_disconnect: true,
            enable_iface_logger: true,
            enable_power_logger: true,
            enable_recovery_logger: true,
            enable_scan_logger: true,
            enable_pno_scan_logger: true,
            enable_sme_timeout_logger: true,
            enable_toggle_logger: true,
            enable_tx_power_scenario_logger: true,
            enable_client_iface_counters_logger: true,
            device_mobility: wlan_telemetry::DeviceMobility::Mobile,
        },
        wlan_telemetry::CobaltAllowlist::All,
    );
    let log_throttler =
        Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));

    let iface_manager = Arc::new(
        ifaces::DeviceMonitorIfaceManager::new(monitor_svc, telemetry_sender.clone())
            .expect("Failed to connect wlanix to wlandevicemonitor"),
    );

    let wifi_state = Arc::new(Mutex::new(WifiState::default()));
    let (event_sender, event_receiver) = mpsc::unbounded::<scheduled_scans::ScheduledScanEvent>();
    let scheduled_scan_controller =
        Arc::new(ScheduledScanController::new(telemetry_sender.clone(), event_sender));

    let suspend_blocker_fut: future::BoxFuture<'static, Result<(), Error>> =
        if let Some(requests) = suspend_blocker_requests {
            Box::pin(serve_suspend_blocker(
                Arc::clone(&wifi_state),
                Arc::clone(&power_manager),
                requests,
            ))
        } else {
            Box::pin(future::pending())
        };

    drop(suspend_blocker_lease);
    let res = futures::try_join!(
        serve_telemetry_fut,
        report_battery_updates(
            Arc::clone(&wifi_state),
            telemetry_sender.clone(),
            Arc::clone(&scheduled_scan_controller),
        )
        .map(Ok),
        handle_scheduled_scan_events(Arc::clone(&wifi_state), event_receiver).map(|()| Ok(())),
        suspend_blocker_fut,
        serve_fidl(
            Arc::clone(&wifi_state),
            Arc::clone(&iface_manager),
            Arc::clone(&power_manager),
            telemetry_sender,
            Arc::clone(&log_throttler),
            Arc::clone(&scheduled_scan_controller),
        ),
        serve_phy_events(phy_events_proxy, wifi_state)
    );
    match res {
        Ok(_) => info!("Wlanix exiting cleanly"),
        Err(e) => error!("Wlanix exiting with error: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream, create_request_stream};

    use crate::ifaces::test_utils::{
        ClientIfaceCall, FAKE_IFACE_RESPONSE, IfaceManagerCall, TestIfaceManager,
    };
    use anyhow::format_err;
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use futures::Future;
    use futures::channel::mpsc;
    use futures::task::Poll;
    use ieee80211::Ssid;
    use netlink_packet_utils::nla::NlasIterator;
    use netlink_packet_utils::parsers::parse_u32;
    use scheduled_scans::ScheduledScanState;
    use std::collections::HashSet;
    use std::pin::{Pin, pin};
    use test_case::test_case;
    use wlan_common::security::wep::WepKey;
    use wlan_power_manager_testing::TestPowerManager;
    const CHIP_ID: u32 = 1;
    const FAKE_IFACE_NAME: &str = "fake-iface-name";

    // This will only work if the message is a parseable nl80211 message. Some
    // attributes are currently write only in our NL80211 implementation. If a
    // write-only attribute is included, this function will panic.
    fn expect_nl80211_message(message: &fidl_wlanix::Nl80211Message) -> GenlMessage<Nl80211> {
        let message = assert_matches!(message, fidl_wlanix::Nl80211Message::Message(m) => m);
        GenlMessage::deserialize(
            &NetlinkHeader::default(),
            &message.payload,
            EmptyDeserializeOptions,
        )
        .expect("Failed to deserialize genetlink message")
    }

    #[fuchsia::test]
    fn test_maybe_run_callback() {
        let _exec = fasync::TestExecutor::new_with_fake_time();
        let (callback_proxy, server_end) = create_proxy::<fidl_wlanix::WifiEventCallbackMarker>();
        let mut callback = Some(callback_proxy);
        // Simple validation that running callback works fine
        maybe_run_callback(
            "WifiEventCallbackProxy::onStart",
            fidl_wlanix::WifiEventCallbackProxy::on_start,
            &mut callback,
        );
        assert!(callback.is_some());

        // Validate that dropping the server end would cause callback proxy to be removed
        std::mem::drop(server_end);
        maybe_run_callback(
            "WifiEventCallbackProxy::onStart",
            fidl_wlanix::WifiEventCallbackProxy::on_start,
            &mut callback,
        );
        assert!(callback.is_none());
    }

    #[fuchsia::test]
    fn test_wifi_get_state_is_started() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_state_fut = test_helper.wifi_proxy.get_state();
        let mut get_state_fut = pin!(get_state_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.is_started, Some(false));
    }

    #[fuchsia::test]
    fn test_wifi_start() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        // One lease for create_sta_iface in setup.
        assert_eq!(test_helper.power_manager.calls.lock().len(), 1);

        let start_fut = test_helper.wifi_proxy.start();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        // The chip is assumed to be powered on by default
        let get_state_fut = test_helper.wifi_proxy.get_state();
        let mut get_state_fut = pin!(get_state_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.is_started, Some(true));

        {
            let power_manager_calls = test_helper.power_manager.calls.lock();
            let calls = test_helper.iface_manager.calls.lock();
            assert_eq!(
                *power_manager_calls,
                vec!["wlanix-create-iface", "wlanix-power-up", "wlanix-phy-1-dependency"]
                    .into_iter()
                    .map(String::from)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                *calls,
                vec![
                    ifaces::test_utils::IfaceManagerCall::CreateClientIface(1),
                    ifaces::test_utils::IfaceManagerCall::ListPhys,
                    ifaces::test_utils::IfaceManagerCall::GetPowerState(1),
                    ifaces::test_utils::IfaceManagerCall::GetPowerElementDependencyToken(1),
                ]
            );
        }

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientConnectionsToggle {
                event: wlan_telemetry::ClientConnectionsToggleEvent::Enabled
            }))
        );
    }

    #[fuchsia::test]
    fn test_wifi_start_without_power_dep_token() {
        let (mut test_helper, mut test_fut) = setup_wifi_test_with_iface_manager(
            TestIfaceManager::new().mock_get_power_element_dependency_token_failure(),
        );

        // One lease for create_sta_iface in setup.
        assert_eq!(test_helper.power_manager.calls.lock().len(), 1);

        let start_fut = test_helper.wifi_proxy.start();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        // The chip is assumed to be powered on by default
        let get_state_fut = test_helper.wifi_proxy.get_state();
        let mut get_state_fut = pin!(get_state_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let start_response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut start_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_matches!(start_response, Ok(()));
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.is_started, Some(true));

        {
            let power_manager_calls = test_helper.power_manager.calls.lock();
            let calls = test_helper.iface_manager.calls.lock();
            // Power element leases should be skipped when the dependency token is missing.
            assert_eq!(
                *power_manager_calls,
                vec!["wlanix-create-iface", "wlanix-power-up"]
                    .into_iter()
                    .map(String::from)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                *calls,
                vec![
                    ifaces::test_utils::IfaceManagerCall::CreateClientIface(1),
                    ifaces::test_utils::IfaceManagerCall::ListPhys,
                    ifaces::test_utils::IfaceManagerCall::GetPowerState(1),
                    ifaces::test_utils::IfaceManagerCall::GetPowerElementDependencyToken(1),
                ]
            );
        }

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientConnectionsToggle {
                event: wlan_telemetry::ClientConnectionsToggleEvent::Enabled
            }))
        );
    }

    #[fuchsia::test]
    fn test_wifi_already_started() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let start_fut = test_helper.wifi_proxy.start();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        let start_fut2 = test_helper.wifi_proxy.start();
        let mut start_fut2 = pin!(start_fut2);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut2), Poll::Pending);

        let get_state_fut = test_helper.wifi_proxy.get_state();
        let mut get_state_fut = pin!(get_state_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut), Poll::Pending);

        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.is_started, Some(true));
        {
            let calls = test_helper.iface_manager.calls.lock();
            assert_eq!(
                *calls,
                vec![
                    ifaces::test_utils::IfaceManagerCall::CreateClientIface(1),
                    ifaces::test_utils::IfaceManagerCall::ListPhys,
                    ifaces::test_utils::IfaceManagerCall::GetPowerState(1),
                    ifaces::test_utils::IfaceManagerCall::GetPowerElementDependencyToken(1),
                    ifaces::test_utils::IfaceManagerCall::ListPhys,
                    ifaces::test_utils::IfaceManagerCall::GetPowerState(1),
                    ifaces::test_utils::IfaceManagerCall::GetPowerElementDependencyToken(1),
                ]
            );
        }

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientConnectionsToggle {
                event: wlan_telemetry::ClientConnectionsToggleEvent::Enabled
            }))
        );
    }

    #[fuchsia::test]
    fn test_wifi_start_stop_start() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        // PowerUp
        let start_fut = test_helper.wifi_proxy.start();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        // PowerDown
        let stop_fut = test_helper.wifi_proxy.stop();
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        // State should be false (stopped)
        let get_state_fut1 = test_helper.wifi_proxy.get_state();
        let mut get_state_fut1 = pin!(get_state_fut1);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut1), Poll::Pending);

        // PowerUp again
        let start_fut2 = test_helper.wifi_proxy.start();
        let mut start_fut2 = pin!(start_fut2);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut2), Poll::Pending);

        // State should be true (started)
        let get_state_fut2 = test_helper.wifi_proxy.get_state();
        let mut get_state_fut2 = pin!(get_state_fut2);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut2), Poll::Pending);

        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response1 = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut1),
            Poll::Ready(Ok(response1)) => response1
        );
        let response2 = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut2),
            Poll::Ready(Ok(response2)) => response2
        );
        assert_eq!(response1.is_started, Some(false));
        assert_eq!(response2.is_started, Some(true));
        {
            let power_manager_calls = test_helper.power_manager.calls.lock();
            let calls = test_helper.iface_manager.calls.lock();
            assert_eq!(
                *power_manager_calls,
                vec![
                    "wlanix-create-iface",
                    "wlanix-power-up",
                    "wlanix-phy-1-dependency",
                    "wlanix-power-down",
                    "wlanix-power-up",
                    "wlanix-phy-1-dependency",
                ]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
            );
            assert_eq!(
                *calls,
                vec![
                    ifaces::test_utils::IfaceManagerCall::CreateClientIface(1),
                    ifaces::test_utils::IfaceManagerCall::ListPhys,
                    ifaces::test_utils::IfaceManagerCall::GetPowerState(1),
                    ifaces::test_utils::IfaceManagerCall::GetPowerElementDependencyToken(1),
                    ifaces::test_utils::IfaceManagerCall::ListPhys,
                    ifaces::test_utils::IfaceManagerCall::GetPowerState(1),
                    ifaces::test_utils::IfaceManagerCall::ListIfaces,
                    ifaces::test_utils::IfaceManagerCall::DestroyIface(1),
                    ifaces::test_utils::IfaceManagerCall::PowerDown(1),
                    ifaces::test_utils::IfaceManagerCall::ListPhys,
                    ifaces::test_utils::IfaceManagerCall::GetPowerState(1),
                    ifaces::test_utils::IfaceManagerCall::PowerUp(1),
                    ifaces::test_utils::IfaceManagerCall::GetPowerElementDependencyToken(1),
                ]
            );
        }

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientConnectionsToggle {
                event: wlan_telemetry::ClientConnectionsToggleEvent::Enabled
            }))
        );
    }

    #[fuchsia::test]
    fn test_wifi_start_fails() {
        let (mut test_helper, mut test_fut) =
            setup_wifi_test_with_iface_manager(TestIfaceManager::new().mock_power_up_failure());

        // Precondition: chip is off
        let get_state_fut = test_helper.wifi_proxy.get_state();
        let mut get_state_fut = pin!(get_state_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut),
            Poll::Ready(Ok(response1)) => response1
        );
        assert_eq!(response.is_started, Some(false));

        // Also ensure the fake iface manager has matching chip power off
        *test_helper.iface_manager.power_state.lock() = false;

        // Clear any previous calls from test setup
        *test_helper.iface_manager.calls.lock() = vec![];

        // Power up
        let start_fut = test_helper.wifi_proxy.start();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Expect a failure to power up, and a metric logged for it
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut start_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert!(response.is_err());
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ChipPowerUpFailure))
        );

        // Expect we turn the chip back off after failure to start
        let calls = test_helper.iface_manager.calls.lock();
        assert_matches!(&calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
        assert_matches!(&calls[1], ifaces::test_utils::IfaceManagerCall::GetPowerState(_));
        assert_matches!(&calls[2], ifaces::test_utils::IfaceManagerCall::PowerUp(_));
        assert_matches!(&calls[3], ifaces::test_utils::IfaceManagerCall::PowerDown(_));
    }

    #[fuchsia::test]
    fn test_wifi_stop() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        // One lease for create_sta_iface in setup.
        assert_eq!(test_helper.power_manager.calls.lock().len(), 1);

        let start_fut = test_helper.wifi_proxy.start();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut), Poll::Pending);

        let stop_fut = test_helper.wifi_proxy.stop();
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut stop_fut), Poll::Pending);

        let get_state_fut = test_helper.wifi_proxy.get_state();
        let mut get_state_fut = pin!(get_state_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_state_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_state_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.is_started, Some(false));

        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert!(power_manager_calls.contains(&"wlanix-power-down".to_string()));

        // On stop, we shut down all remaining ifaces and power down.
        let calls = test_helper.iface_manager.calls.lock();
        assert!(!calls.is_empty());
        assert_matches!(
            &calls[calls.len() - 1],
            ifaces::test_utils::IfaceManagerCall::PowerDown(_)
        );
        assert_matches!(
            &calls[calls.len() - 2],
            ifaces::test_utils::IfaceManagerCall::DestroyIface(_)
        );

        // There was a start and a stop, so expect enabled and disabled mesages.
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientConnectionsToggle {
                event: wlan_telemetry::ClientConnectionsToggleEvent::Enabled
            }))
        );
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientConnectionsToggle {
                event: wlan_telemetry::ClientConnectionsToggleEvent::Disabled
            }))
        );
        assert!(test_helper.telemetry_receiver.try_next().is_err());
    }

    #[fuchsia::test]
    fn test_wifi_stop_fails_to_destroy_iface() {
        let (mut test_helper, mut test_fut) = setup_wifi_test_with_iface_manager(
            TestIfaceManager::new().mock_destroy_client_iface_failure(),
        );

        let start_fut = test_helper.wifi_proxy.start();
        let mut start_fut = pin!(start_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut start_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        // Clear out the client connections toggle event so that we can test for the telemetry
        // event we are interested in later in this test.
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientConnectionsToggle {
                event: wlan_telemetry::ClientConnectionsToggleEvent::Enabled
            }))
        );

        let stop_fut = test_helper.wifi_proxy.stop();
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut stop_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Verify that telemetry event for iface destruction failure is sent.
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceDestructionFailure))
        );
    }

    #[fuchsia::test]
    fn test_wifi_get_chip_ids() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_chip_ids_fut = test_helper.wifi_proxy.get_chip_ids();
        let mut get_chip_ids_fut = pin!(get_chip_ids_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_chip_ids_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_chip_ids_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.chip_ids, Some(vec![1]));
    }

    #[fuchsia::test]
    fn test_wifi_set_country_code() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();
        const COUNTRY_CODE: [u8; 2] = *b"CA";

        let set_country_fut = test_helper.wifi_chip_proxy.set_country_code(
            fidl_wlanix::WifiChipSetCountryCodeRequest {
                code: Some(COUNTRY_CODE),
                ..Default::default()
            },
        );
        let mut set_country_fut = pin!(set_country_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut set_country_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let calls = test_helper.iface_manager.calls.lock();
        assert_eq!(calls.len(), 2);
        // CreateClientIface is called in setup_wifi_test.
        assert_matches!(&calls[0], ifaces::test_utils::IfaceManagerCall::CreateClientIface(_));
        assert_matches!(
            &calls[1],
            ifaces::test_utils::IfaceManagerCall::SetCountry { country, .. } => { assert_eq!(*country, COUNTRY_CODE) }
        );
    }

    #[fuchsia::test]
    fn test_wifi_chip_create_sta_iface_fails() {
        // Set up
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let (wlanix_proxy, wlanix_stream) = create_proxy_and_stream::<fidl_wlanix::WlanixMarker>();
        let (wifi_proxy, wifi_server_end) = create_proxy::<fidl_wlanix::WifiMarker>();
        let result = wlanix_proxy.get_wifi(fidl_wlanix::WlanixGetWifiRequest {
            wifi: Some(wifi_server_end),
            ..Default::default()
        });
        assert_matches!(result, Ok(()));

        let (wifi_chip_proxy, wifi_chip_server_end) = create_proxy::<fidl_wlanix::WifiChipMarker>();
        let get_chip_fut = wifi_proxy.get_chip(fidl_wlanix::WifiGetChipRequest {
            chip_id: Some(CHIP_ID),
            chip: Some(wifi_chip_server_end),
            ..Default::default()
        });
        let mut get_chip_fut = pin!(get_chip_fut);
        assert_matches!(exec.run_until_stalled(&mut get_chip_fut), Poll::Pending);

        let (_wifi_sta_iface_proxy, wifi_sta_iface_server_end) =
            create_proxy::<fidl_wlanix::WifiStaIfaceMarker>();
        let create_sta_iface_fut =
            wifi_chip_proxy.create_sta_iface(fidl_wlanix::WifiChipCreateStaIfaceRequest {
                iface: Some(wifi_sta_iface_server_end),
                ..Default::default()
            });
        let mut create_sta_iface_fut = pin!(create_sta_iface_fut);
        assert_matches!(exec.run_until_stalled(&mut create_sta_iface_fut), Poll::Pending);

        let wifi_state = Arc::new(Mutex::new(WifiState::default()));
        let (callback_proxy, mut callback_stream) =
            create_proxy_and_stream::<fidl_wlanix::WifiEventCallbackMarker>();
        wifi_state.lock().callback.replace(callback_proxy);

        let iface_manager = Arc::new(TestIfaceManager::new().mock_create_client_iface_failure());
        let power_manager = Arc::new(TestPowerManager::new());
        let (telemetry_sender_raw, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender_raw);
        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (scheduled_scan_event_sender, _) = mpsc::unbounded();
        let scheduled_scan_controller = Arc::new(ScheduledScanController::new(
            telemetry_sender.clone(),
            scheduled_scan_event_sender,
        ));
        let test_fut = serve_wlanix(
            wlanix_stream,
            wifi_state,
            Arc::clone(&iface_manager),
            power_manager,
            telemetry_sender,
            Arc::clone(&log_throttler),
            scheduled_scan_controller,
        );
        let mut test_fut = Box::pin(test_fut);
        assert_eq!(exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut get_chip_fut), Poll::Ready(Ok(Ok(()))));

        // Execute test
        assert_matches!(
            exec.run_until_stalled(&mut create_sta_iface_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_INTERNAL)))
        );

        // Verify that the PHY reset was requested.
        let calls = iface_manager.calls.lock();
        assert_matches!(
            calls.last().expect("iface call history is empty"),
            &ifaces::test_utils::IfaceManagerCall::ResetPhy { .. }
        );

        // Verify telemetry event for iface creation failure is sent
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceCreationFailure))
        );

        // Verify that OnSubsystemRestart callback was called.
        let callback_event = assert_matches!(
            exec.run_until_stalled(&mut callback_stream.next()),
            Poll::Ready(Some(Ok(req))) => req
        );
        assert_matches!(
            callback_event,
            fidl_wlanix::WifiEventCallbackRequest::OnSubsystemRestart { payload, .. } => {
                assert_eq!(payload.status, Some(zx::sys::ZX_ERR_INTERNAL));
            }
        );
    }

    #[fuchsia::test]
    #[should_panic]
    fn test_wifi_chip_create_sta_iface_fails_and_reset_fails() {
        // Set up
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let (wlanix_proxy, wlanix_stream) = create_proxy_and_stream::<fidl_wlanix::WlanixMarker>();
        let (wifi_proxy, wifi_server_end) = create_proxy::<fidl_wlanix::WifiMarker>();
        let result = wlanix_proxy.get_wifi(fidl_wlanix::WlanixGetWifiRequest {
            wifi: Some(wifi_server_end),
            ..Default::default()
        });
        assert_matches!(result, Ok(()));

        let (wifi_chip_proxy, wifi_chip_server_end) = create_proxy::<fidl_wlanix::WifiChipMarker>();
        let get_chip_fut = wifi_proxy.get_chip(fidl_wlanix::WifiGetChipRequest {
            chip_id: Some(CHIP_ID),
            chip: Some(wifi_chip_server_end),
            ..Default::default()
        });
        let mut get_chip_fut = pin!(get_chip_fut);
        assert_matches!(exec.run_until_stalled(&mut get_chip_fut), Poll::Pending);

        let (_wifi_sta_iface_proxy, wifi_sta_iface_server_end) =
            create_proxy::<fidl_wlanix::WifiStaIfaceMarker>();
        let create_sta_iface_fut =
            wifi_chip_proxy.create_sta_iface(fidl_wlanix::WifiChipCreateStaIfaceRequest {
                iface: Some(wifi_sta_iface_server_end),
                ..Default::default()
            });
        let mut create_sta_iface_fut = pin!(create_sta_iface_fut);
        assert_matches!(exec.run_until_stalled(&mut create_sta_iface_fut), Poll::Pending);

        let wifi_state = Arc::new(Mutex::new(WifiState::default()));
        let (callback_proxy, mut callback_stream) =
            create_proxy_and_stream::<fidl_wlanix::WifiEventCallbackMarker>();
        wifi_state.lock().callback.replace(callback_proxy);

        let iface_manager = Arc::new(
            TestIfaceManager::new().mock_create_client_iface_failure().mock_reset_phy_failure(),
        );
        let power_manager = Arc::new(TestPowerManager::new());
        let (telemetry_sender_raw, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender_raw);
        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (scheduled_scan_event_sender, _) = mpsc::unbounded();
        let scheduled_scan_controller = Arc::new(ScheduledScanController::new(
            telemetry_sender.clone(),
            scheduled_scan_event_sender,
        ));
        let test_fut = serve_wlanix(
            wlanix_stream,
            wifi_state,
            Arc::clone(&iface_manager),
            power_manager,
            telemetry_sender,
            Arc::clone(&log_throttler),
            scheduled_scan_controller,
        );
        let mut test_fut = Box::pin(test_fut);
        assert_eq!(exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut get_chip_fut), Poll::Ready(Ok(Ok(()))));

        // Execute test
        assert_matches!(exec.run_until_stalled(&mut create_sta_iface_fut), Poll::Ready(Ok(Err(_))));

        // Verify that the PHY reset was requested.
        let calls = iface_manager.calls.lock();
        assert_matches!(
            calls.last().expect("iface call history is empty"),
            &ifaces::test_utils::IfaceManagerCall::ResetPhy { .. }
        );

        // Verify telemetry event for iface creation failure is sent
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceCreationFailure))
        );

        // Verify that OnSubsystemRestart callback was not called.
        assert_matches!(exec.run_until_stalled(&mut callback_stream.next()), Poll::Pending);
    }

    #[fuchsia::test]
    fn test_wifi_chip_remove_sta_iface_fails() {
        let (mut test_helper, mut test_fut) = setup_wifi_test_with_iface_manager(
            TestIfaceManager::new().mock_destroy_client_iface_failure(),
        );

        let request = fidl_wlanix::WifiChipRemoveStaIfaceRequest {
            iface_name: Some("mock-sta-iface".to_string()),
            ..Default::default()
        };
        let remove_sta_iface_fut = test_helper.wifi_chip_proxy.remove_sta_iface(request);
        let mut remove_sta_iface_fut = pin!(remove_sta_iface_fut);

        assert_eq!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut remove_sta_iface_fut),
            Poll::Ready(Ok(Err(_)))
        );

        // Verify telemetry event for iface destruction failure is sent
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceDestructionFailure))
        );
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_available_modes() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_available_modes_fut = test_helper.wifi_chip_proxy.get_available_modes();
        let mut get_available_modes_fut = pin!(get_available_modes_fut);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_available_modes_fut),
            Poll::Pending
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_available_modes_fut),
            Poll::Ready(Ok(response)) => response
        );
        let expected_response = fidl_wlanix::WifiChipGetAvailableModesResponse {
            chip_modes: Some(vec![fidl_wlanix::ChipMode {
                id: Some(CHIP_ID),
                available_combinations: Some(vec![fidl_wlanix::ChipConcurrencyCombination {
                    limits: Some(vec![fidl_wlanix::ChipConcurrencyCombinationLimit {
                        types: Some(vec![fidl_wlanix::IfaceConcurrencyType::Sta]),
                        max_ifaces: Some(1),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }]),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert_eq!(response, expected_response);
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_id() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_id_fut = test_helper.wifi_chip_proxy.get_id();
        let mut get_id_fut = pin!(get_id_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_id_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(test_helper.exec.run_until_stalled(&mut get_id_fut), Poll::Ready(Ok(response)) => response);
        assert_eq!(response.id, Some(CHIP_ID));
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_mode() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_mode_fut = test_helper.wifi_chip_proxy.get_mode();
        let mut get_mode_fut = pin!(get_mode_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_mode_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(test_helper.exec.run_until_stalled(&mut get_mode_fut), Poll::Ready(Ok(response)) => response);
        assert_eq!(response.mode, Some(0));
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_capabilities() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_capabilities_fut = test_helper.wifi_chip_proxy.get_capabilities();
        let mut get_capabilities_fut = pin!(get_capabilities_fut);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_capabilities_fut),
            Poll::Pending
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_capabilities_fut),
            Poll::Ready(Ok(response)) => response);
        assert_eq!(response.capabilities_mask, Some(0));
    }

    #[fuchsia::test]
    fn test_wifi_chip_trigger_subsystem_restart() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        // Register a callback to be notified of the reset.
        let (callback_client, mut callback_stream) =
            create_request_stream::<fidl_wlanix::WifiEventCallbackMarker>();
        assert_matches!(
            test_helper.wifi_proxy.register_event_callback(
                fidl_wlanix::WifiRegisterEventCallbackRequest {
                    callback: Some(callback_client),
                    ..Default::default()
                }
            ),
            Ok(())
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Trigger a PHY reset.
        let request_fut = test_helper.wifi_chip_proxy.trigger_subsystem_restart();
        let mut request_fut = pin!(request_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut request_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Verify the reset was successful.
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(response)) => response);
        assert_matches!(response, Ok(()));

        // Verify that the telemetry events were sent.
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::RecoveryEvent))
        );
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::RecoveryResult { result: Ok(()) }))
        );

        // Verify that the PHY reset was called.
        let iface_calls = test_helper.iface_manager.calls.lock();
        assert_matches!(iface_calls.last().unwrap(), IfaceManagerCall::ResetPhy(id) => assert_eq!(*id, CHIP_ID as u16));

        // Verify that OnSubsystemRestart callback was called.
        let callback_event = assert_matches!(
            test_helper.exec.run_until_stalled(&mut callback_stream.next()),
            Poll::Ready(Some(Ok(req))) => req
        );
        assert_matches!(
            callback_event,
            fidl_wlanix::WifiEventCallbackRequest::OnSubsystemRestart { payload, .. } => {
                assert_eq!(payload.status, Some(zx::sys::ZX_OK));
            }
        );
    }

    #[fuchsia::test]
    fn test_wifi_chip_trigger_subsystem_restart_failure() {
        // Setup the test to fail the PHY reset request.
        let (mut test_helper, mut test_fut) =
            setup_wifi_test_with_iface_manager(TestIfaceManager::new().mock_reset_phy_failure());

        // Register a callback to be notified of the reset.
        let (callback_client, mut callback_stream) =
            create_request_stream::<fidl_wlanix::WifiEventCallbackMarker>();
        assert_matches!(
            test_helper.wifi_proxy.register_event_callback(
                fidl_wlanix::WifiRegisterEventCallbackRequest {
                    callback: Some(callback_client),
                    ..Default::default()
                }
            ),
            Ok(())
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Trigger a PHY reset.
        let request_fut = test_helper.wifi_chip_proxy.trigger_subsystem_restart();
        let mut request_fut = pin!(request_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut request_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Verify the caller is notified of the error.
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(response)) => response);
        assert_matches!(response, Err(status) if status == zx::Status::INTERNAL.into_raw());

        // Verify that the telemetry events were sent.
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::RecoveryEvent))
        );
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::RecoveryResult { result: Err(()) }))
        );

        // Verify that OnSubsystemRestart callback was not called.
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut callback_stream.next()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn test_wifi_chip_remove_sta_iface() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        // One lease for create_sta_iface in setup.
        assert_eq!(test_helper.power_manager.calls.lock().len(), 1);

        let remove_sta_iface_fut = test_helper.wifi_chip_proxy.remove_sta_iface(
            fidl_wlanix::WifiChipRemoveStaIfaceRequest {
                iface_name: Some("some_iface_name".to_string()), // iface name doesn't matter
                ..Default::default()
            },
        );
        let mut remove_sta_iface_fut = pin!(remove_sta_iface_fut);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut remove_sta_iface_fut),
            Poll::Pending
        );
        assert_eq!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut remove_sta_iface_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert_eq!(power_manager_calls.len(), 2);
        assert_eq!(power_manager_calls[1], "wlanix-remove-iface");

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ClientIfaceDestroyed { iface_id })) => {
                assert_eq!(iface_id, FAKE_IFACE_RESPONSE.id);
            }
        );
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_sta_iface_names() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        // We observe the iface created by setup_wifi_test.
        let get_iface_names_fut = test_helper.wifi_chip_proxy.get_sta_iface_names();
        let mut get_iface_names_fut = pin!(get_iface_names_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let result = assert_matches!(test_helper.exec.run_until_stalled(&mut get_iface_names_fut), Poll::Ready(Ok(result)) => result);
        assert_eq!(result.iface_names, Some(vec![IFACE_NAME.to_string()]));
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_sta_iface_names_no_ifaces() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        // Remove the iface from setup.
        let _ = test_helper.iface_manager.client_iface.lock().take();

        // No ifaces show up.
        let get_iface_names_fut = test_helper.wifi_chip_proxy.get_sta_iface_names();
        let mut get_iface_names_fut = pin!(get_iface_names_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let result = assert_matches!(test_helper.exec.run_until_stalled(&mut get_iface_names_fut), Poll::Ready(Ok(result)) => result);
        assert_eq!(result.iface_names, Some(vec![]));
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_sta_iface() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let (wifi_sta_iface_proxy, wifi_sta_iface_server_end) =
            create_proxy::<fidl_wlanix::WifiStaIfaceMarker>();
        let mut get_sta_iface_fut =
            test_helper.wifi_chip_proxy.get_sta_iface(fidl_wlanix::WifiChipGetStaIfaceRequest {
                iface_name: Some("FOO".to_string()),
                iface: Some(wifi_sta_iface_server_end),
                ..Default::default()
            });

        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let result = assert_matches!(test_helper.exec.run_until_stalled(&mut get_sta_iface_fut), Poll::Ready(Ok(result)) => result);
        assert!(result.is_ok());
        assert!(test_helper.power_manager.is_lease_dropped("wlanix-get-sta-iface"));

        let get_name_fut = wifi_sta_iface_proxy.get_name();
        let mut get_name_fut = pin!(get_name_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_name_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_name_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.iface_name, Some(IFACE_NAME.to_string()));
    }

    #[fuchsia::test]
    fn test_wifi_chip_get_sta_iface_no_iface() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();
        let _ = test_helper.iface_manager.client_iface.lock().take();

        let (_wifi_sta_iface_proxy, wifi_sta_iface_server_end) =
            create_proxy::<fidl_wlanix::WifiStaIfaceMarker>();
        let mut get_sta_iface_fut =
            test_helper.wifi_chip_proxy.get_sta_iface(fidl_wlanix::WifiChipGetStaIfaceRequest {
                iface_name: Some("FOO".to_string()),
                iface: Some(wifi_sta_iface_server_end),
                ..Default::default()
            });

        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let result = assert_matches!(test_helper.exec.run_until_stalled(&mut get_sta_iface_fut), Poll::Ready(Ok(result)) => result);
        assert!(result.is_err());
    }

    #[fuchsia::test]
    fn test_wifi_sta_iface_get_name() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_name_fut = test_helper.wifi_sta_iface_proxy.get_name();
        let mut get_name_fut = pin!(get_name_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut get_name_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_name_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_eq!(response.iface_name, Some(IFACE_NAME.to_string()));
    }

    #[fuchsia::test]
    fn test_wifi_sta_iface_apf_packet_filter() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let test_program = vec![1, 2, 3, 4];
        let mut install_fut = test_helper.wifi_sta_iface_proxy.install_apf_packet_filter(
            &fidl_wlanix::WifiStaIfaceInstallApfPacketFilterRequest {
                program: Some(test_program.clone()),
                ..Default::default()
            },
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut install_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert!(power_manager_calls.contains(&"wlanix-install-apf-packet-filter".to_string()));
        drop(power_manager_calls);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(iface_calls.lock().last().unwrap(), ClientIfaceCall::InstallApfPacketFilter(program) => assert_eq!(program, &test_program));
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut install_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let mut read_fut = test_helper.wifi_sta_iface_proxy.read_apf_packet_filter_data();
        assert_matches!(test_helper.exec.run_until_stalled(&mut read_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert!(power_manager_calls.contains(&"wlanix-read-apf-packet-filter-data".to_string()));
        drop(power_manager_calls);
        assert_matches!(
            iface_calls.lock().last().unwrap(),
            ClientIfaceCall::ReadApfPacketFilterData
        );
        let response = assert_matches!(test_helper.exec.run_until_stalled(&mut read_fut), Poll::Ready(Ok(Ok(response))) => response);
        assert_eq!(response.memory, Some(vec![2, 2, 2, 2]));
    }

    #[fuchsia::test]
    fn test_wifi_sta_iface_apf_packet_filter_support() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let mut support_fut = test_helper.wifi_sta_iface_proxy.get_apf_packet_filter_support();
        assert_matches!(test_helper.exec.run_until_stalled(&mut support_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let response = assert_matches!(test_helper.exec.run_until_stalled(&mut support_fut), Poll::Ready(Ok(Ok(response))) => response);
        assert_eq!(response.version, Some(1));
        assert_eq!(response.max_filter_length, Some(1));
    }

    struct WifiTestHelper {
        _wlanix_proxy: fidl_wlanix::WlanixProxy,
        wifi_proxy: fidl_wlanix::WifiProxy,
        wifi_chip_proxy: fidl_wlanix::WifiChipProxy,
        wifi_sta_iface_proxy: fidl_wlanix::WifiStaIfaceProxy,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        iface_manager: Arc<TestIfaceManager>,
        power_manager: Arc<TestPowerManager>,

        // Note: keep the executor field last in the struct so it gets dropped last.
        exec: fasync::TestExecutor,
    }

    fn setup_wifi_test() -> (WifiTestHelper, Pin<Box<impl Future<Output = ()>>>) {
        setup_wifi_test_with_iface_manager(TestIfaceManager::new())
    }

    fn setup_wifi_test_with_iface_manager(
        iface_manager: TestIfaceManager,
    ) -> (WifiTestHelper, Pin<Box<impl Future<Output = ()>>>) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let (wlanix_proxy, wlanix_stream) = create_proxy_and_stream::<fidl_wlanix::WlanixMarker>();
        let (wifi_proxy, wifi_server_end) = create_proxy::<fidl_wlanix::WifiMarker>();
        let result = wlanix_proxy.get_wifi(fidl_wlanix::WlanixGetWifiRequest {
            wifi: Some(wifi_server_end),
            ..Default::default()
        });
        assert_matches!(result, Ok(()));

        let (wifi_chip_proxy, wifi_chip_server_end) = create_proxy::<fidl_wlanix::WifiChipMarker>();
        let get_chip_fut = wifi_proxy.get_chip(fidl_wlanix::WifiGetChipRequest {
            chip_id: Some(CHIP_ID),
            chip: Some(wifi_chip_server_end),
            ..Default::default()
        });
        let mut get_chip_fut = pin!(get_chip_fut);
        assert_matches!(exec.run_until_stalled(&mut get_chip_fut), Poll::Pending);

        let (wifi_sta_iface_proxy, wifi_sta_iface_server_end) =
            create_proxy::<fidl_wlanix::WifiStaIfaceMarker>();
        let create_sta_iface_fut =
            wifi_chip_proxy.create_sta_iface(fidl_wlanix::WifiChipCreateStaIfaceRequest {
                iface: Some(wifi_sta_iface_server_end),
                ..Default::default()
            });
        let mut create_sta_iface_fut = pin!(create_sta_iface_fut);
        assert_matches!(exec.run_until_stalled(&mut create_sta_iface_fut), Poll::Pending);

        let wifi_state = Arc::new(Mutex::new(WifiState::default()));
        let iface_manager = Arc::new(iface_manager);
        let (telemetry_sender_raw, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender_raw);
        let power_manager = Arc::new(TestPowerManager::new());
        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (scheduled_scan_event_sender, _) = mpsc::unbounded();
        let scheduled_scan_controller = Arc::new(ScheduledScanController::new(
            telemetry_sender.clone(),
            scheduled_scan_event_sender,
        ));
        let test_fut = serve_wlanix(
            wlanix_stream,
            wifi_state,
            Arc::clone(&iface_manager),
            power_manager.clone(),
            telemetry_sender,
            Arc::clone(&log_throttler),
            scheduled_scan_controller,
        );
        let mut test_fut = Box::pin(test_fut);
        assert_eq!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut get_chip_fut), Poll::Ready(Ok(Ok(()))));
        assert_matches!(exec.run_until_stalled(&mut create_sta_iface_fut), Poll::Ready(Ok(Ok(()))));
        assert!(power_manager.is_lease_dropped("wlanix-create-sta-iface"));

        assert_matches!(telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::ClientIfaceCreated { iface_id })) => {
            assert_eq!(iface_id, FAKE_IFACE_RESPONSE.id);
        });

        // Quick check that telemetry event queue is now empty
        assert_matches!(telemetry_receiver.try_next(), Err(_));

        let test_helper = WifiTestHelper {
            _wlanix_proxy: wlanix_proxy,
            wifi_proxy,
            wifi_chip_proxy,
            wifi_sta_iface_proxy,
            telemetry_receiver,
            iface_manager,
            power_manager,
            exec,
        };
        (test_helper, test_fut)
    }

    #[fuchsia::test]
    fn test_supplicant_remove_interface() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        test_helper
            .supplicant_proxy
            .remove_interface(fidl_wlanix::SupplicantRemoveInterfaceRequest {
                iface_name: Some(FAKE_IFACE_NAME.to_string()),
                ..Default::default()
            })
            .expect("Failed to call RemoveInterface");
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::Disconnect);
    }

    #[fuchsia::test]
    fn test_supplicant_sta_iface_disconnect() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut disconnect_fut = test_helper.supplicant_sta_iface_proxy.disconnect();
        assert_matches!(test_helper.exec.run_until_stalled(&mut disconnect_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::Disconnect);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut disconnect_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[fuchsia::test]
    fn test_supplicant_sta_iface_get_mac_address() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut get_mac_address_fut = test_helper.supplicant_sta_iface_proxy.get_mac_address();
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_mac_address_fut),
            Poll::Pending
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::Query);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_mac_address_fut),
            Poll::Ready(Ok(Ok(response))) => response);
        assert_eq!(response.mac_addr.unwrap(), [13u8, 37, 13, 37, 13, 37]);
    }

    #[fuchsia::test]
    fn test_supplicant_sta_iface_get_mac_address_after_iface_restart() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        // Increment the iface id to simulate an iface restart.
        test_helper.iface_manager.set_iface_id(1234);

        // Under the hood we pick up the new iface seamlessly.
        let mut get_mac_address_fut = test_helper.supplicant_sta_iface_proxy.get_mac_address();
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_mac_address_fut),
            Poll::Pending
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::Query);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_mac_address_fut),
            Poll::Ready(Ok(Ok(response))) => response);
        assert_eq!(response.mac_addr.unwrap(), [13u8, 37, 13, 37, 13, 37]);
    }

    #[test_case(
        fidl_wlanix::BtCoexistenceMode::Enabled,
        fidl_internal::BtCoexistenceMode::ModeAuto
    )]
    #[test_case(
        fidl_wlanix::BtCoexistenceMode::Disabled,
        fidl_internal::BtCoexistenceMode::ModeOff
    )]
    #[test_case(fidl_wlanix::BtCoexistenceMode::Sense, fidl_internal::BtCoexistenceMode::ModeAuto)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_supplicant_sta_iface_set_bt_coexistence_mode(
        mocked_mode: fidl_wlanix::BtCoexistenceMode,
        desired_mode: fidl_internal::BtCoexistenceMode,
    ) {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        let mut set_bt_coexistence_mode_fut = test_helper
            .supplicant_sta_iface_proxy
            .set_bt_coexistence_mode(&fidl_wlanix::SupplicantStaIfaceSetBtCoexistenceModeRequest {
                mode: Some(mocked_mode),
                ..Default::default()
            });
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut set_bt_coexistence_mode_fut),
            Poll::Pending
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::SetBtCoexistenceMode { mode } => assert_eq!(*mode, desired_mode));
    }

    #[fuchsia::test]
    fn test_supplicant_sta_iface_set_sta_country_code() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        const COUNTRY_CODE: [u8; 2] = *b"WW";

        let mut set_sta_country_fut = test_helper.supplicant_sta_iface_proxy.set_sta_country_code(
            fidl_wlanix::SupplicantStaIfaceSetStaCountryCodeRequest {
                code: Some(COUNTRY_CODE),
                ..Default::default()
            },
        );
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut set_sta_country_fut),
            Poll::Pending
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::SetCountry(country) => assert_eq!(*country, COUNTRY_CODE));
    }

    #[fuchsia::test]
    fn test_supplicant_sta_open_network_connect_flow() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert!(power_manager_calls.contains(&"wlanix-select-connect".to_string()));

        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );
        assert!(test_helper.power_manager.is_lease_dropped("wlanix-select-connect"));

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let (ssid, credential, bssid) = assert_matches!(
            iface_calls.lock()[0].clone(),
            ClientIfaceCall::ConnectToNetwork { ssid, credential, bssid, .. } => (ssid, credential, bssid)
        );
        assert_eq!(ssid, vec![b'f', b'o', b'o']);
        assert_eq!(credential, Credential::None);
        assert_eq!(bssid, None);
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }))) => payload);
        assert_eq!(on_state_changed.new_state, Some(fidl_wlanix::StaIfaceCallbackState::Completed));
        assert_eq!(on_state_changed.bssid, Some([42, 42, 42, 42, 42, 42]));
        assert_eq!(on_state_changed.id, Some(1));
        assert_eq!(on_state_changed.ssid, Some(vec![b'f', b'o', b'o']));

        let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Connect);
        assert!(mcast_msg.payload.attrs.contains(&Nl80211Attr::Mac([42, 42, 42, 42, 42, 42])));

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ConnectResult { result, bss, is_credential_rejected: _, is_owe_transition: _ })) => {
                assert_eq!(result, fidl_ieee80211::StatusCode::Success);
                assert_eq!(bss.ssid, Ssid::try_from("foo").unwrap());
                assert_eq!(bss.bssid, Bssid::from([42, 42, 42, 42, 42, 42]));
            }
        );
    }

    #[fuchsia::test]
    fn test_supplicant_sta_protected_network_connect_flow() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let passphrase = vec![b'p', b'a', b's', b's'];
        let result = test_helper.supplicant_sta_network_proxy.set_psk_passphrase(
            &fidl_wlanix::SupplicantStaNetworkSetPskPassphraseRequest {
                passphrase: Some(passphrase.clone()),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let (ssid, credential, bssid) = assert_matches!(
            iface_calls.lock()[0].clone(),
            ClientIfaceCall::ConnectToNetwork { ssid, credential, bssid, .. } => (ssid, credential, bssid)
        );
        assert_eq!(ssid, vec![b'f', b'o', b'o']);
        assert_eq!(credential, Credential::Password(passphrase));
        assert_eq!(bssid, None);
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }))) => payload);
        assert_eq!(on_state_changed.new_state, Some(fidl_wlanix::StaIfaceCallbackState::Completed));

        let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Connect);
    }

    #[fuchsia::test]
    fn test_supplicant_sta_wpa3_network_connect_flow() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let password = vec![b'p', b'a', b's', b's'];
        let result = test_helper.supplicant_sta_network_proxy.set_sae_password(
            &fidl_wlanix::SupplicantStaNetworkSetSaePasswordRequest {
                password: Some(password.clone()),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let (ssid, credential, bssid) = assert_matches!(
            iface_calls.lock()[0].clone(),
            ClientIfaceCall::ConnectToNetwork { ssid, credential, bssid, .. } => (ssid, credential, bssid)
        );
        assert_eq!(ssid, vec![b'f', b'o', b'o']);
        assert_eq!(credential, Credential::SaePassword(password));
        assert_eq!(bssid, None);
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }))) => payload);
        assert_eq!(on_state_changed.new_state, Some(fidl_wlanix::StaIfaceCallbackState::Completed));

        let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Connect);
    }

    #[fuchsia::test]
    fn test_supplicant_sta_wep_network_connect_flow() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        // Save first WEP key
        let key1 = *b"wepke";
        let index1 = 0;
        let result = test_helper.supplicant_sta_network_proxy.set_wep_key(
            &fidl_wlanix::SupplicantStaNetworkSetWepKeyRequest {
                key: Some(key1.to_vec()),
                key_idx: Some(index1),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        // Save a second WEP key and set this as the one to use
        let key2 = *b"other";
        let index2 = 2;
        let result = test_helper.supplicant_sta_network_proxy.set_wep_key(
            &fidl_wlanix::SupplicantStaNetworkSetWepKeyRequest {
                key: Some(key2.to_vec()),
                key_idx: Some(index2),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        let result = test_helper.supplicant_sta_network_proxy.set_wep_tx_key_idx(
            &fidl_wlanix::SupplicantStaNetworkSetWepTxKeyIdxRequest {
                key_idx: Some(index2),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let (ssid, credential, bssid) = assert_matches!(
            iface_calls.lock()[0].clone(),
            ClientIfaceCall::ConnectToNetwork { ssid, credential, bssid, .. } => (ssid, credential, bssid)
        );
        assert_eq!(ssid, vec![b'f', b'o', b'o']);
        // Verify that the credential to use is the one that was designated as the index to use.
        assert_matches!(credential, Credential::WepKey(keys) => {
            assert_eq!(keys.get_key(), Some(WepKey::Wep40(key2)));
        });
        assert_eq!(bssid, None);
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }))) => payload);
        assert_eq!(on_state_changed.new_state, Some(fidl_wlanix::StaIfaceCallbackState::Completed));

        let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Connect);
    }

    #[fuchsia::test]
    fn test_supplicant_sta_network_connect_flow_with_bssid_set() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let result = test_helper.supplicant_sta_network_proxy.set_bssid(
            &fidl_wlanix::SupplicantStaNetworkSetBssidRequest {
                bssid: Some([1, 2, 3, 4, 5, 6]),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let (ssid, credential, bssid) = assert_matches!(
            iface_calls.lock()[0].clone(),
            ClientIfaceCall::ConnectToNetwork { ssid, credential, bssid, .. } => (ssid, credential, bssid)
        );
        assert_eq!(ssid, vec![b'f', b'o', b'o']);
        assert_eq!(credential, Credential::None);
        assert_eq!(bssid, Some(Bssid::from([1, 2, 3, 4, 5, 6])));
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }))) => payload);
        assert_eq!(on_state_changed.new_state, Some(fidl_wlanix::StaIfaceCallbackState::Completed));
        assert_eq!(on_state_changed.bssid, Some([1, 2, 3, 4, 5, 6]));
        assert_eq!(on_state_changed.id, Some(1));
        assert_eq!(on_state_changed.ssid, Some(vec![b'f', b'o', b'o']));

        let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Connect);
    }

    #[fuchsia::test]
    fn test_supplicant_sta_network_connect_flow_with_bssid_set_and_cleared() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let result = test_helper.supplicant_sta_network_proxy.set_bssid(
            &fidl_wlanix::SupplicantStaNetworkSetBssidRequest {
                bssid: Some([1, 2, 3, 4, 5, 6]),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let bssid = assert_matches!(
            iface_calls.lock()[0].clone(),
            ClientIfaceCall::ConnectToNetwork { bssid, .. } => bssid
        );
        assert_eq!(bssid, None);
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }))) => payload);
        assert_eq!(on_state_changed.new_state, Some(fidl_wlanix::StaIfaceCallbackState::Completed));
        assert_eq!(on_state_changed.bssid, Some([42, 42, 42, 42, 42, 42]));
    }

    #[fuchsia::test]
    fn test_supplicant_sta_network_set_key_mgmt() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_key_mgmt(
            &fidl_wlanix::SupplicantStaNetworkSetKeyMgmtRequest {
                key_mgmt_mask: Some(fidl_wlanix::KeyMgmtMask::WPA_PSK),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let key_mgmt = assert_matches!(
            iface_calls.lock()[0].clone(),
            ClientIfaceCall::ConnectToNetwork { key_mgmt, .. } => key_mgmt
        );
        assert_eq!(key_mgmt, Some(fidl_wlanix::KeyMgmtMask::WPA_PSK));
    }

    fn establish_open_connection(
        test_helper: &mut SupplicantTestHelper,
        test_fut: &mut Pin<Box<impl Future<Output = ()>>>,
        mcast_stream: &mut fidl_wlanix::Nl80211MulticastRequestStream,
    ) {
        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut network_select_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        {
            let next_mcast = next_mcast_message(mcast_stream);
            let mut next_mcast = pin!(next_mcast);
            let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
            assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Connect);
        }

        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }))) => payload);
        assert_eq!(on_state_changed.new_state, Some(fidl_wlanix::StaIfaceCallbackState::Completed));
    }

    #[fuchsia::test]
    fn test_supplicant_sta_network_connect_flow_failure() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        // Configure our iface manager to send a connect failure.
        *test_helper.iface_manager.get_client_iface().connect_success.lock() = false;

        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let result = test_helper.supplicant_sta_network_proxy.set_ssid(
            &fidl_wlanix::SupplicantStaNetworkSetSsidRequest {
                ssid: Some(vec![b'f', b'o', b'o']),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));
        assert_matches!(test_helper.supplicant_sta_network_proxy.clear_bssid(), Ok(()));

        let mut network_select_fut = test_helper.supplicant_sta_network_proxy.select();
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut network_select_fut),
            Poll::Ready(Ok(Ok(())))
        );

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(iface_calls.lock()[0].clone(), ClientIfaceCall::ConnectToNetwork { .. });
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let reject = assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Ready(Some(Ok(fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnAssociationRejected { payload, .. }))) => payload);
        assert_eq!(reject.bssid, Some([42, 42, 42, 42, 42, 42]));
        assert_eq!(reject.ssid, Some(vec![b'f', b'o', b'o']));
        assert_eq!(reject.status_code, Some(fidl_ieee80211::StatusCode::RefusedReasonUnspecified));
        assert_eq!(reject.timed_out, Some(false));
    }

    #[fuchsia::test]
    fn test_supplicant_sta_disconnect_signal() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");

        establish_open_connection(&mut test_helper, &mut test_fut, &mut mcast_stream);
        // Metrics: for this test, we don't care about the contents of the ConnectResult
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ConnectResult {
                result: _,
                bss: _,
                is_credential_rejected: _,
                is_owe_transition: _,
            }))
        );

        let connection_length_nanos: u16 = rand::random();
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(connection_length_nanos.into()));

        let mocked_disconnect_source = fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
            mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            reason_code: fidl_fuchsia_wlan_ieee80211::ReasonCode::ReasonInactivity,
        });
        let mocked_is_sme_reconnecting = false;
        {
            let client_iface = test_helper.iface_manager.get_client_iface();
            let transaction_handle = client_iface.transaction_handle.lock();
            let control_handle = transaction_handle.as_ref().expect("No control handle found");
            control_handle
                .send_on_disconnect(&fidl_sme::DisconnectInfo {
                    is_sme_reconnecting: mocked_is_sme_reconnecting,
                    disconnect_source: mocked_disconnect_source,
                })
                .expect("Failed to send OnDisconnect");
        }

        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();

        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert!(power_manager_calls.contains(&"wlanix-process-disconnect".to_string()));
        drop(power_manager_calls);

        assert_matches!(
            test_helper.exec.run_until_stalled(&mut next_callback_fut),
            Poll::Ready(Some(Ok(
                fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnDisconnected { .. }
            )))
        );
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        let on_state_changed = assert_matches!(
            test_helper.exec.run_until_stalled(&mut next_callback_fut),
            Poll::Ready(Some(Ok(
                fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }
            ))) => payload);
        assert_eq!(
            on_state_changed.new_state,
            Some(fidl_wlanix::StaIfaceCallbackState::Disconnected)
        );

        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Disconnect);

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let disconnect_info = assert_matches!(
            iface_calls.lock().pop().expect("iface call history should not be empty"),
            ClientIfaceCall::OnDisconnect { info } => info
        );
        assert_eq!(disconnect_info, mocked_disconnect_source);

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::Disconnect { info })) => {
                assert_eq!(info.connected_duration, zx::BootDuration::from_nanos(connection_length_nanos.into()));
                assert_eq!(info.is_sme_reconnecting, mocked_is_sme_reconnecting);
                assert_eq!(info.disconnect_source, mocked_disconnect_source);
                assert_eq!(info.original_bss_desc.ssid, Ssid::try_from("foo").unwrap());
                assert_eq!(info.original_bss_desc.bssid, Bssid::from([42, 42, 42, 42, 42, 42]));
            }
        );
    }

    #[test_case(fidl_sme::ConnectResult {
        code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
        is_credential_rejected: false,
        is_reconnect: true,
    }, false; "Successful reconnect")]
    #[test_case(fidl_sme::ConnectResult {
        code: fidl_fuchsia_wlan_ieee80211::StatusCode::RefusedReasonUnspecified,
        is_credential_rejected: false,
        is_reconnect: true,
    }, true; "Failed reconnect")]
    #[test_case(fidl_sme::ConnectResult {
        code: fidl_fuchsia_wlan_ieee80211::StatusCode::RefusedReasonUnspecified,
        is_credential_rejected: false,
        is_reconnect: false,
    }, false; "Ignore non-reconnect result")]
    fn test_supplicant_sta_sme_reconnect(
        reconnect_result: fidl_sme::ConnectResult,
        expect_disconnect: bool,
    ) {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");

        establish_open_connection(&mut test_helper, &mut test_fut, &mut mcast_stream);
        // Metrics: for this test, we don't care about the contents of the ConnectResult
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ConnectResult {
                result: _,
                bss: _,
                is_credential_rejected: _,
                is_owe_transition: _,
            }))
        );

        let connection_length_nanos: u16 = rand::random();
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(connection_length_nanos.into()));

        let mocked_disconnect_source = fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
            mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            reason_code: fidl_fuchsia_wlan_ieee80211::ReasonCode::ReasonInactivity,
        });
        let mocked_is_sme_reconnecting = true;
        {
            let client_iface = test_helper.iface_manager.get_client_iface();
            let transaction_handle = client_iface.transaction_handle.lock();
            let control_handle = transaction_handle.as_ref().expect("No control handle found");
            control_handle
                .send_on_disconnect(&fidl_sme::DisconnectInfo {
                    is_sme_reconnecting: mocked_is_sme_reconnecting,
                    disconnect_source: mocked_disconnect_source,
                })
                .expect("Failed to send OnDisconnect");
        }

        // No callbacks for disconnect, since we're awaiting a reconnect result.
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let mut next_callback_fut = test_helper.supplicant_sta_iface_callback_stream.next();
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_callback_fut), Poll::Pending);
        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        // We should always log a disconnect to the metrics module, even if reconnect is pending
        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::Disconnect { info })) => {
                assert_eq!(info.connected_duration, zx::BootDuration::from_nanos(connection_length_nanos.into()));
                assert_eq!(info.is_sme_reconnecting, mocked_is_sme_reconnecting);
                assert_eq!(info.disconnect_source, mocked_disconnect_source);
                assert_eq!(info.original_bss_desc.ssid, Ssid::try_from("foo").unwrap());
                assert_eq!(info.original_bss_desc.bssid, Bssid::from([42, 42, 42, 42, 42, 42]));
            }
        );

        // Send and process the reconnect result.
        let client_iface = test_helper.iface_manager.get_client_iface();
        let locked_handle = client_iface.transaction_handle.lock();
        let handle = locked_handle.as_ref().unwrap();
        handle
            .send_on_connect_result(&reconnect_result)
            .expect("Failed to send ConnectResult for reconnect");
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert!(power_manager_calls.contains(&"wlanix-process-disconnect".to_string()));
        assert!(power_manager_calls.contains(&"wlanix-process-connect-result".to_string()));
        drop(power_manager_calls);

        if expect_disconnect {
            assert_matches!(
                test_helper.exec.run_until_stalled(&mut next_callback_fut),
                Poll::Ready(Some(Ok(
                    fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnDisconnected { .. }
                )))
            );
            let on_state_changed = assert_matches!(
            test_helper.exec.run_until_stalled(&mut next_callback_fut),
            Poll::Ready(Some(Ok(
                fidl_wlanix::SupplicantStaIfaceCallbackRequest::OnStateChanged { payload, .. }
            ))) => payload);
            assert_eq!(
                on_state_changed.new_state,
                Some(fidl_wlanix::StaIfaceCallbackState::Disconnected)
            );

            let mcast_msg = assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
            assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::Disconnect);

            let iface_calls = test_helper.iface_manager.get_iface_call_history();
            let disconnect_info = assert_matches!(
                iface_calls.lock().pop().expect("iface call history should not be empty"),
                ClientIfaceCall::OnDisconnect { info } => info
            );
            assert_eq!(disconnect_info, mocked_disconnect_source);
        } else {
            // Still no messages, since the reconnect was successful.
            assert_matches!(
                test_helper.exec.run_until_stalled(&mut next_callback_fut),
                Poll::Pending
            );
            assert_matches!(test_helper.exec.run_until_stalled(&mut next_mcast), Poll::Pending);
        }

        // Metrics: no further messages expected, regardless of if reconnect is successful
        assert_matches!(test_helper.telemetry_receiver.try_next(), Err(_));
    }

    #[fuchsia::test]
    fn test_supplicant_sta_signal_report() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");

        establish_open_connection(&mut test_helper, &mut test_fut, &mut mcast_stream);

        let mocked_signal_report =
            fidl_internal::SignalReportIndication { rssi_dbm: -35, snr_db: 20, tx_rate_500kbps: 0 };
        {
            let client_iface = test_helper.iface_manager.get_client_iface();
            let transaction_handle = client_iface.transaction_handle.lock();
            let control_handle = transaction_handle.as_ref().expect("No control handle found");
            control_handle
                .send_on_signal_report(&mocked_signal_report)
                .expect("Failed to send OnDisconnect");
        }

        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        let signal_report_ind = assert_matches!(
            iface_calls.lock().pop().expect("iface call history should not be empty"),
            ClientIfaceCall::OnSignalReport { ind } => ind
        );
        assert_eq!(signal_report_ind, mocked_signal_report);
    }

    #[fuchsia::test]
    fn test_supplicant_get_signal_poll_results_success() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");

        establish_open_connection(&mut test_helper, &mut test_fut, &mut mcast_stream);

        // Mock the signal report response
        let mock_report = fidl_fuchsia_wlan_stats::SignalReport {
            connection_signal_report: Some(fidl_fuchsia_wlan_stats::ConnectionSignalReport {
                rssi_dbm: Some(-53),
                tx_rate_500kbps: Some(300),
                primary: Some(fidl_fuchsia_wlan_ieee80211::ChannelNumber {
                    band: fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz,
                    number: 36,
                }),
                bandwidth: Some(fidl_fuchsia_wlan_ieee80211::ChannelBandwidth::Cbw20),
                vht_secondary_80_channel: None,
                ..Default::default()
            }),
            ..Default::default()
        };

        let client_iface = test_helper.iface_manager.client_iface.lock().as_ref().unwrap().clone();
        client_iface.signal_report.lock().replace(mock_report);

        let mut get_fut = test_helper.supplicant_sta_iface_proxy.get_signal_poll_results();

        assert_matches!(test_helper.exec.run_until_stalled(&mut get_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(iface_calls.lock().last().unwrap(), ClientIfaceCall::GetSignalReport);

        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_fut),
            Poll::Ready(Ok(response)) => response
        );
        let result = assert_matches!(response, Ok(res) => res);

        assert_eq!(result.current_rssi_dbm, Some(-53));
        assert_eq!(result.tx_bitrate_mbps, Some(150)); // 300 / 2
        assert_eq!(result.rx_bitrate_mbps, Some(0)); // TODO(496331508): Rx rate isn't implemented yet
        assert_eq!(result.frequency_mhz, Some(5180)); // Channel 36 center freq
    }

    #[fuchsia::test]
    fn test_supplicant_sta_roam_result() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        let mut mcast_stream = get_nl80211_mcast(&test_helper.nl80211_proxy, "mlme");

        establish_open_connection(&mut test_helper, &mut test_fut, &mut mcast_stream);

        let mocked_roam_result = fidl_sme::RoamResult {
            bssid: [0; 6],
            status_code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
            original_association_maintained: false,
            bss_description: None,
            disconnect_info: None,
            is_credential_rejected: false,
        };
        {
            let client_iface = test_helper.iface_manager.get_client_iface();
            let transaction_handle = client_iface.transaction_handle.lock();
            let control_handle = transaction_handle.as_ref().expect("No control handle found");
            control_handle
                .send_on_roam_result(&mocked_roam_result)
                .expect("Failed to send OnRoamResult");
        }

        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let power_manager_calls = test_helper.power_manager.calls.lock();
        assert!(power_manager_calls.contains(&"wlanix-process-roam-result".to_string()));
        drop(power_manager_calls);
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_supplicant_set_power_save_mode(desired: bool) {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut set_fut: fidl::client::QueryResponseFut<()> = test_helper
            .supplicant_sta_iface_proxy
            .set_power_save(fidl_fuchsia_wlan_wlanix::SupplicantStaIfaceSetPowerSaveRequest {
                enable: Some(desired),
                ..Default::default()
            });
        assert_matches!(test_helper.exec.run_until_stalled(&mut set_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock().last().unwrap(), ClientIfaceCall::SetPowerSaveMode(setting) => assert_eq!(*setting, desired));
        assert_matches!(test_helper.exec.run_until_stalled(&mut set_fut), Poll::Ready(Ok(())));
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_supplicant_set_suspend_mode(desired: bool) {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();

        let mut set_fut: fidl::client::QueryResponseFut<()> =
            test_helper.supplicant_sta_iface_proxy.set_suspend_mode_enabled(
                fidl_fuchsia_wlan_wlanix::SupplicantStaIfaceSetSuspendModeEnabledRequest {
                    enable: Some(desired),
                    ..Default::default()
                },
            );
        assert_matches!(test_helper.exec.run_until_stalled(&mut set_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock().last().unwrap(), ClientIfaceCall::SetSuspendMode(setting)  => assert_eq!(*setting, desired));
        assert_matches!(test_helper.exec.run_until_stalled(&mut set_fut), Poll::Ready(Ok(())));
    }

    struct SupplicantTestHelper {
        _wlanix_proxy: fidl_wlanix::WlanixProxy,
        supplicant_proxy: fidl_wlanix::SupplicantProxy,
        supplicant_sta_iface_proxy: fidl_wlanix::SupplicantStaIfaceProxy,
        nl80211_proxy: fidl_wlanix::Nl80211Proxy,
        supplicant_sta_network_proxy: fidl_wlanix::SupplicantStaNetworkProxy,
        supplicant_sta_iface_callback_stream: fidl_wlanix::SupplicantStaIfaceCallbackRequestStream,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        iface_manager: Arc<TestIfaceManager>,
        power_manager: Arc<TestPowerManager>,
        scheduled_scan_controller: Arc<ScheduledScanController>,
        _event_receiver: mpsc::UnboundedReceiver<scheduled_scans::ScheduledScanEvent>,
        battery_manager_stream: fidl_fuchsia_power_battery::BatteryManagerRequestStream,

        // Note: keep the executor field last in the struct so it gets dropped last.
        exec: fasync::TestExecutor,
    }

    impl SupplicantTestHelper {
        fn get_battery_watcher(&mut self) -> fidl_fuchsia_power_battery::BatteryInfoWatcherProxy {
            let mut next_fut = self.battery_manager_stream.next();
            let watcher = assert_matches!(
                self.exec.run_until_stalled(&mut next_fut),
                Poll::Ready(Some(Ok(fidl_fuchsia_power_battery::BatteryManagerRequest::Watch { watcher, .. }))) => watcher.into_proxy()
            );

            // Respond to the daemon's startup GetBatteryInfo query to unblock the watcher loop
            let mut next_fut = self.battery_manager_stream.next();
            let responder = assert_matches!(
                self.exec.run_until_stalled(&mut next_fut),
                Poll::Ready(Some(Ok(fidl_fuchsia_power_battery::BatteryManagerRequest::GetBatteryInfo { responder, .. }))) => responder
            );
            responder
                .send(&fidl_fuchsia_power_battery::BatteryInfo::default())
                .expect("Failed to respond to GetBatteryInfo");

            watcher
        }
    }

    fn setup_supplicant_test() -> (SupplicantTestHelper, Pin<Box<impl Future<Output = ()>>>) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let (wlanix_proxy, wlanix_stream) = create_proxy_and_stream::<fidl_wlanix::WlanixMarker>();
        let (supplicant_proxy, supplicant_server_end) =
            create_proxy::<fidl_wlanix::SupplicantMarker>();
        let result = wlanix_proxy.get_supplicant(fidl_wlanix::WlanixGetSupplicantRequest {
            supplicant: Some(supplicant_server_end),
            ..Default::default()
        });
        assert_matches!(result, Ok(()));

        let (nl80211_proxy, nl80211_server_end) = create_proxy::<fidl_wlanix::Nl80211Marker>();
        let result = wlanix_proxy.get_nl80211(fidl_wlanix::WlanixGetNl80211Request {
            nl80211: Some(nl80211_server_end),
            ..Default::default()
        });
        assert_matches!(result, Ok(()));

        let (supplicant_sta_iface_proxy, supplicant_sta_iface_server_end) =
            create_proxy::<fidl_wlanix::SupplicantStaIfaceMarker>();
        let result =
            supplicant_proxy.add_sta_interface(fidl_wlanix::SupplicantAddStaInterfaceRequest {
                iface: Some(supplicant_sta_iface_server_end),
                iface_name: Some(FAKE_IFACE_NAME.to_string()),
                ..Default::default()
            });
        assert_matches!(result, Ok(()));

        let (supplicant_sta_iface_callback_client_end, supplicant_sta_iface_callback_stream) =
            create_request_stream::<fidl_wlanix::SupplicantStaIfaceCallbackMarker>();
        let result = supplicant_sta_iface_proxy.register_callback(
            fidl_wlanix::SupplicantStaIfaceRegisterCallbackRequest {
                callback: Some(supplicant_sta_iface_callback_client_end),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let (supplicant_sta_network_proxy, supplicant_sta_network_server_end) =
            create_proxy::<fidl_wlanix::SupplicantStaNetworkMarker>();
        let result = supplicant_sta_iface_proxy.add_network(
            fidl_wlanix::SupplicantStaIfaceAddNetworkRequest {
                network: Some(supplicant_sta_network_server_end),
                ..Default::default()
            },
        );
        assert_matches!(result, Ok(()));

        let wifi_state = Arc::new(Mutex::new(WifiState::default()));
        let iface_manager = Arc::new(TestIfaceManager::new_with_client());
        let power_manager = Arc::new(TestPowerManager::new());
        let (telemetry_sender_raw, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender_raw);
        let (event_sender, event_receiver) = mpsc::unbounded();
        let scheduled_scan_controller =
            Arc::new(ScheduledScanController::new(telemetry_sender.clone(), event_sender));
        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));

        let (battery_manager_proxy, battery_manager_stream) =
            create_proxy_and_stream::<fidl_fuchsia_power_battery::BatteryManagerMarker>();

        let test_fut = serve_wlanix(
            wlanix_stream,
            Arc::clone(&wifi_state),
            Arc::clone(&iface_manager),
            power_manager.clone(),
            telemetry_sender.clone(),
            Arc::clone(&log_throttler),
            Arc::clone(&scheduled_scan_controller),
        );
        let battery_loop_fut = report_battery_updates_helper(
            battery_manager_proxy,
            Arc::clone(&wifi_state),
            telemetry_sender.clone(),
            Arc::clone(&scheduled_scan_controller),
        );
        let combined_fut = async move {
            let serve_fut = std::pin::pin!(test_fut);
            let battery_fut = std::pin::pin!(battery_loop_fut);
            futures::future::select(serve_fut, battery_fut).await;
        };
        let mut test_fut = Box::pin(combined_fut);
        assert_eq!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let test_helper = SupplicantTestHelper {
            _wlanix_proxy: wlanix_proxy,
            supplicant_proxy,
            supplicant_sta_iface_proxy,
            nl80211_proxy,
            supplicant_sta_network_proxy,
            supplicant_sta_iface_callback_stream,
            telemetry_receiver,
            iface_manager,
            power_manager,
            scheduled_scan_controller,
            _event_receiver: event_receiver,
            battery_manager_stream,
            exec,
        };
        (test_helper, test_fut)
    }

    fn get_nl80211_mcast(
        nl80211_proxy: &fidl_wlanix::Nl80211Proxy,
        group: &str,
    ) -> fidl_wlanix::Nl80211MulticastRequestStream {
        let (mcast_client, mcast_stream) =
            create_request_stream::<fidl_wlanix::Nl80211MulticastMarker>();
        nl80211_proxy
            .get_multicast(fidl_wlanix::Nl80211GetMulticastRequest {
                group: Some(group.to_string()),
                multicast: Some(mcast_client),
                ..Default::default()
            })
            .expect("Failed to get multicast");
        mcast_stream
    }

    async fn next_mcast_message(
        stream: &mut fidl_wlanix::Nl80211MulticastRequestStream,
    ) -> GenlMessage<Nl80211> {
        let req = stream
            .next()
            .await
            .expect("Failed to request multicast message")
            .expect("Multicast message stream terminated");
        let mcast_msg = assert_matches!(req, fidl_wlanix::Nl80211MulticastRequest::Message {
            payload: fidl_wlanix::Nl80211MulticastMessageRequest {message: Some(m), .. }, ..} => m);
        expect_nl80211_message(&mcast_msg)
    }

    struct Nl80211TestValues {
        nl80211_fut: Pin<Box<dyn Future<Output = ()>>>,
        nl80211_proxy: fidl_wlanix::Nl80211Proxy,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        state: Arc<Mutex<WifiState>>,
        scheduled_scan_controller: Arc<ScheduledScanController>,
    }

    fn setup_nl80211_test(exec: &mut fasync::TestExecutor) -> Nl80211TestValues {
        setup_nl80211_test_with_iface_manager(exec, TestIfaceManager::new_with_client())
    }

    fn setup_nl80211_test_with_iface_manager(
        exec: &mut fasync::TestExecutor,
        iface_manager: TestIfaceManager,
    ) -> Nl80211TestValues {
        let (proxy, stream) = create_proxy_and_stream::<fidl_wlanix::Nl80211Marker>();
        let state = Arc::new(Mutex::new(WifiState::default()));
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (event_sender, event_receiver) = mpsc::unbounded();
        let scheduled_scan_controller =
            Arc::new(ScheduledScanController::new(telemetry_sender.clone(), event_sender));
        let nl80211_fut = serve_nl80211(
            stream,
            Arc::clone(&state),
            Arc::new(iface_manager),
            telemetry_sender,
            Arc::clone(&log_throttler),
            Arc::clone(&scheduled_scan_controller),
        );
        let event_loop_fut = handle_scheduled_scan_events(Arc::clone(&state), event_receiver);
        let combined_fut = async move {
            let nl80211_fut = std::pin::pin!(nl80211_fut);
            let event_loop_fut = std::pin::pin!(event_loop_fut);
            futures::future::select(nl80211_fut, event_loop_fut).await;
        };
        let mut nl80211_fut = Box::pin(combined_fut);
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);

        Nl80211TestValues {
            nl80211_fut,
            nl80211_proxy: proxy,
            telemetry_receiver,
            state,
            scheduled_scan_controller,
        }
    }

    #[fuchsia::test]
    fn get_nl80211() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, stream) = create_proxy_and_stream::<fidl_wlanix::WlanixMarker>();
        let state = Arc::new(Mutex::new(WifiState::default()));
        let iface_manager = Arc::new(TestIfaceManager::new());
        let (telemetry_sender_raw, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender_raw);
        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (scheduled_scan_event_sender, _) = mpsc::unbounded();
        let scheduled_scan_controller = Arc::new(ScheduledScanController::new(
            telemetry_sender.clone(),
            scheduled_scan_event_sender,
        ));
        let wlanix_fut = serve_wlanix(
            stream,
            state,
            iface_manager,
            Arc::new(TestPowerManager::new()),
            telemetry_sender,
            Arc::clone(&log_throttler),
            scheduled_scan_controller,
        );
        let mut wlanix_fut = pin!(wlanix_fut);
        let (nl_proxy, nl_server) = create_proxy::<fidl_wlanix::Nl80211Marker>();
        proxy
            .get_nl80211(fidl_wlanix::WlanixGetNl80211Request {
                nl80211: Some(nl_server),
                ..Default::default()
            })
            .expect("Failed to get Nl80211");
        assert_matches!(exec.run_until_stalled(&mut wlanix_fut), Poll::Pending);
        assert!(!nl_proxy.is_closed());
    }

    #[fuchsia::test]
    fn unsupported_mcast_group() {
        let mut exec = fasync::TestExecutor::new();

        let mut test_values = setup_nl80211_test(&mut exec);

        let mut mcast_stream = get_nl80211_mcast(&test_values.nl80211_proxy, "doesnt_exist");
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        // The stream should immediately terminate.
        let next_mcast = mcast_stream.next();
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Ready(None));

        // serve_nl80211 should complete successfully.
        drop(test_values.nl80211_proxy);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn unsupported_nl80211_command() {
        #[derive(Debug)]
        struct TestNl80211 {
            cmd: u8,
            attrs: Vec<Nl80211Attr>,
        }

        impl netlink_packet_generic::GenlFamily for TestNl80211 {
            fn family_name() -> &'static str {
                "nl80211"
            }

            fn command(&self) -> u8 {
                self.cmd
            }

            fn version(&self) -> u8 {
                1
            }
        }

        impl netlink_packet_utils::Emitable for TestNl80211 {
            fn emit(&self, buffer: &mut [u8]) {
                self.attrs.as_slice().emit(buffer)
            }

            fn buffer_len(&self) -> usize {
                self.attrs.as_slice().buffer_len()
            }
        }

        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        // Create an nl80211 message with invalid command
        let genl_message = GenlMessage::from_payload(TestNl80211 { cmd: 255, attrs: vec![] });
        let mut buffer = vec![0u8; genl_message.buffer_len()];
        genl_message.serialize(&mut buffer);
        let invalid_message =
            fidl_wlanix::Nl80211Message::Message(fidl_wlanix::Message { payload: buffer });

        let query_resp_fut = test_values.nl80211_proxy.message_v2(&invalid_message);
        let mut query_resp_fut = pin!(query_resp_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut query_resp_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_INTERNAL)))
        );
    }

    #[fuchsia::test]
    fn get_interface() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);
        let get_interface_message = build_nl80211_message(Nl80211Cmd::GetInterface, vec![]);
        let get_interface_fut = test_values.nl80211_proxy.message_v2(&get_interface_message);
        let mut get_interface_fut = pin!(get_interface_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut get_interface_fut),
            Poll::Ready(Ok(Ok(r))) => r));

        assert_eq!(responses.len(), 2);
        let message = expect_nl80211_message(&responses[0]);
        assert_eq!(message.payload.cmd, Nl80211Cmd::NewInterface);
        assert!(
            message.payload.attrs.contains(&Nl80211Attr::Wiphy(
                ifaces::test_utils::FAKE_IFACE_RESPONSE.phy_id.into()
            ))
        );
        assert!(message.payload.attrs.iter().any(|attr| *attr
            == Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())));
        assert!(
            message
                .payload
                .attrs
                .contains(&Nl80211Attr::Mac(ifaces::test_utils::FAKE_IFACE_RESPONSE.sta_addr))
        );
        assert_matches!(responses[1], fidl_wlanix::Nl80211Message::Done(_));
    }

    #[fuchsia::test]
    fn get_station() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();

        let tx_packets = 100;
        let tx_failed = 5;
        let rx_packets = 200;
        let rssi = -53;
        let tx_rate_500kbps = 300;
        let expected_tx_bitrate = 1500; // 300 * 5 = 1500

        // Build stats and signal report for testing iface to use
        {
            let client_iface = iface_manager.client_iface.lock().as_ref().unwrap().clone();
            let stats = fidl_fuchsia_wlan_stats::IfaceStats {
                connection_stats: Some(fidl_fuchsia_wlan_stats::ConnectionStats {
                    tx_total: Some(tx_packets.into()),
                    tx_drop: Some(tx_failed.into()),
                    rx_unicast_total: Some(rx_packets.into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            client_iface.iface_stats.lock().replace(stats);

            let signal_report = fidl_fuchsia_wlan_stats::SignalReport {
                connection_signal_report: Some(fidl_fuchsia_wlan_stats::ConnectionSignalReport {
                    rssi_dbm: Some(rssi),
                    tx_rate_500kbps: Some(tx_rate_500kbps),
                    bandwidth: Some(fidl_fuchsia_wlan_ieee80211::ChannelBandwidth::Cbw20),
                    vht_secondary_80_channel: None,
                    ..Default::default()
                }),
                ..Default::default()
            };
            client_iface.signal_report.lock().replace(signal_report);
        }

        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        let get_station_message = build_nl80211_message(
            Nl80211Cmd::GetStation,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let get_station_fut = test_values.nl80211_proxy.message_v2(&get_station_message);

        let mut get_station_fut = pin!(get_station_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut get_station_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        let message = assert_matches!(
            &responses[0],
            fidl_wlanix::Nl80211Message::Message(fidl_wlanix::Message { payload }) => payload
        );

        verify_get_station_response(
            message,
            tx_packets,
            tx_failed,
            rx_packets,
            rssi,
            expected_tx_bitrate,
        );
    }

    fn verify_get_station_response(
        payload: &[u8],
        expected_tx_packets: u32,
        expected_tx_failed: u32,
        expected_rx_packets: u32,
        expected_rssi: i8,
        expected_tx_bitrate: u32,
    ) {
        // Constants are from `crate::nl80211::constants`, which isn't exposed
        // because they aren't meant to be used directly.
        const NL80211_ATTR_STA_INFO: u16 = 21;
        const NL80211_STA_INFO_SIGNAL: u16 = 7;
        const NL80211_STA_INFO_TX_BITRATE: u16 = 8;
        const NL80211_STA_INFO_RX_PACKETS: u16 = 9;
        const NL80211_STA_INFO_TX_PACKETS: u16 = 10;
        const NL80211_STA_INFO_TX_FAILED: u16 = 12;
        const NL80211_RATE_INFO_BITRATE32: u16 = 5;

        // Verify that the response message type has type STA info.
        let sta_info = NlasIterator::new(&payload[4..])
            .find(|nla| nla.clone().expect("Failed to parse NLA").kind() == NL80211_ATTR_STA_INFO)
            .expect("Failed to find STA info in response")
            .expect("Failed to parse STA info");

        // Verify that the STA info has the expected values and no unexpected values.
        for nla in NlasIterator::new(sta_info.value()) {
            let nla = nla.expect("Failed to parse inner NLA");
            match nla.kind() {
                NL80211_STA_INFO_TX_PACKETS => {
                    assert_eq!(parse_u32(nla.value()).unwrap(), expected_tx_packets);
                }
                NL80211_STA_INFO_TX_FAILED => {
                    assert_eq!(parse_u32(nla.value()).unwrap(), expected_tx_failed);
                }
                NL80211_STA_INFO_RX_PACKETS => {
                    assert_eq!(parse_u32(nla.value()).unwrap(), expected_rx_packets);
                }
                NL80211_STA_INFO_SIGNAL => {
                    assert_eq!(nla.value()[0] as i8, expected_rssi);
                }
                NL80211_STA_INFO_TX_BITRATE => {
                    for rate_nla in NlasIterator::new(nla.value()) {
                        let rate_nla = rate_nla.unwrap();
                        if rate_nla.kind() == NL80211_RATE_INFO_BITRATE32 {
                            assert_eq!(parse_u32(rate_nla.value()).unwrap(), expected_tx_bitrate);
                        }
                    }
                }
                _ => {
                    panic!("Unexpected NLA kind: {}", nla.kind());
                }
            }
        }

        // Verify that all attributes are included. The loop above checks the types and values but
        // would ignore missing values.
        let attrs = NlasIterator::new(sta_info.value())
            .collect::<Result<Vec<_>, _>>()
            .expect("Failed to make attrs iter into vec");
        assert!(attrs.iter().any(|nla| nla.kind() == NL80211_STA_INFO_TX_PACKETS));
        assert!(attrs.iter().any(|nla| nla.kind() == NL80211_STA_INFO_TX_FAILED));
        assert!(attrs.iter().any(|nla| nla.kind() == NL80211_STA_INFO_RX_PACKETS));
        assert!(attrs.iter().any(|nla| nla.kind() == NL80211_STA_INFO_SIGNAL));
        assert!(attrs.iter().any(|nla| nla.kind() == NL80211_STA_INFO_TX_BITRATE));

        let attr_kinds: HashSet<u16> = NlasIterator::new(sta_info.value())
            .map(|nla| nla.expect("Failed to parse NLA").kind())
            .collect();

        let expected_kinds: HashSet<u16> = [
            NL80211_STA_INFO_TX_PACKETS,
            NL80211_STA_INFO_TX_FAILED,
            NL80211_STA_INFO_RX_PACKETS,
            NL80211_STA_INFO_SIGNAL,
            NL80211_STA_INFO_TX_BITRATE,
        ]
        .into_iter()
        .collect();

        assert_eq!(attr_kinds, expected_kinds);
    }

    #[fuchsia::test]
    fn trigger_scan() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let mut mcast_stream = get_nl80211_mcast(&test_values.nl80211_proxy, "scan");
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let trigger_scan_message = build_nl80211_message(
            Nl80211Cmd::TriggerScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let trigger_scan_fut = test_values.nl80211_proxy.message_v2(&trigger_scan_message);

        let mut trigger_scan_fut = pin!(trigger_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanStart))
        );

        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut trigger_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanResult {
                result: wlan_telemetry::ScanResult::Complete { .. }
            }))
        );

        // With our faked scan results we expect an immediate multicast notification.
        let mcast_msg =
            assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::NewScanResults);
    }

    #[fuchsia::test]
    fn start_sched_scan() {
        use crate::nl80211::{Nl80211SchedScanMatchAttr, Nl80211SchedScanPlanAttr};
        let mut exec = fasync::TestExecutor::new();

        let iface_manager = TestIfaceManager::new_with_client();
        let client_iface = iface_manager.client_iface.lock().clone().unwrap();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        let start_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StartSchedScan,
            vec![
                Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
                Nl80211Attr::ScanSsids(vec![b"TestSSID".to_vec(), b"".to_vec()]),
                Nl80211Attr::SchedScanMatch(vec![vec![
                    Nl80211SchedScanMatchAttr::Ssid(b"TestMatchSSID".to_vec()),
                    Nl80211SchedScanMatchAttr::Rssi(-50),
                    Nl80211SchedScanMatchAttr::RelativeRssi(10),
                    Nl80211SchedScanMatchAttr::RssiAdjust(5120), // band: 0 (2GHz), delta: 20
                ]]),
                Nl80211Attr::Ie(vec![1, 2, 3]),
                Nl80211Attr::RelativeRssi(5),
                Nl80211Attr::RssiAdjust(62977), // band: 1 (5GHz), delta: -10
                Nl80211Attr::SchedScanDelay(30),
                Nl80211Attr::SchedScanMulti(true),
                Nl80211Attr::SchedScanInterval(40000), // 40 seconds
                Nl80211Attr::SchedScanPlans(vec![vec![
                    Nl80211SchedScanPlanAttr::Interval(20), // 20 seconds
                    Nl80211SchedScanPlanAttr::Iterations(5),
                ]]),
            ],
        );
        let start_scan_fut = test_values.nl80211_proxy.message_v2(&start_sched_scan_message);
        let mut start_scan_fut = pin!(start_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut start_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        let calls = client_iface.calls.lock();
        assert_eq!(calls.len(), 1);
        let request =
            assert_matches!(&calls[0], ClientIfaceCall::StartSchedScan { _request } => _request);

        assert_eq!(request.ssids.as_ref().unwrap().len(), 2);
        assert_eq!(request.ssids.as_ref().unwrap()[0], b"TestSSID".to_vec());
        assert_eq!(request.ssids.as_ref().unwrap()[1], b"".to_vec());

        assert_eq!(request.match_sets.as_ref().unwrap().len(), 1);
        assert_eq!(request.match_sets.as_ref().unwrap()[0].ssid, Some(b"TestMatchSSID".to_vec()));
        assert_eq!(request.match_sets.as_ref().unwrap()[0].min_rssi_threshold, Some(-50));
        assert_eq!(request.match_sets.as_ref().unwrap()[0].relative_rssi_threshold, Some(10));
        assert_eq!(
            request.match_sets.as_ref().unwrap()[0].band_rssi_adjustments,
            Some(vec![fidl_common::BandRssiAdjustment {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                rssi_adjustment: 20
            }])
        );

        let scan_plans = request.scan_plans.as_ref().unwrap();
        assert_eq!(scan_plans.len(), 2);
        assert_eq!(scan_plans[0].interval, 20);
        assert_eq!(scan_plans[0].iterations, 5);
        assert_eq!(scan_plans[1].interval, 40);
        assert_eq!(scan_plans[1].iterations, 0);

        assert_eq!(request.relative_rssi_threshold, Some(5));
        assert_eq!(
            request.band_rssi_adjustments,
            Some(vec![fidl_common::BandRssiAdjustment {
                band: fidl_ieee80211::WlanBand::FiveGhz,
                rssi_adjustment: -10
            }])
        );
    }

    #[fuchsia::test]
    fn stop_sched_scan() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let client_iface = iface_manager.client_iface.lock().clone().unwrap();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        let stop_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StopSchedScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let stop_scan_fut = test_values.nl80211_proxy.message_v2(&stop_sched_scan_message);
        let mut stop_scan_fut = pin!(stop_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut stop_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        let calls = client_iface.calls.lock();
        assert!(calls.is_empty());
    }

    #[fuchsia::test]
    fn test_start_sched_scan_fallback_to_software_scheduled_scan() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let client_iface = iface_manager.client_iface.lock().clone().unwrap();

        // Make start_sched_scan fail with NOT_SUPPORTED
        *client_iface.fail_start_sched_scan.lock() = true;

        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        // Set charging to true to trigger the loop
        test_values.scheduled_scan_controller.set_charging_state(true);

        let attrs = vec![
            Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
            Nl80211Attr::ScanSsids(vec![b"TestSSID".to_vec()]),
            Nl80211Attr::SchedScanInterval(40),
        ];
        let start_sched_scan_message = build_nl80211_message(Nl80211Cmd::StartSchedScan, attrs);
        let start_scan_fut = test_values.nl80211_proxy.message_v2(&start_sched_scan_message);
        let mut start_scan_fut = pin!(start_scan_fut);

        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut start_scan_fut), Poll::Ready(Ok(Ok(_))));

        // Verify that state transitioned to SoftwareScansActive
        assert_matches!(
            test_values.scheduled_scan_controller.states.lock().get(&1),
            Some(ScheduledScanState::SoftwareScansActive { .. })
        );
    }

    #[fuchsia::test]
    fn test_stop_sched_scan_clears_software_scheduled_scan() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let iface = iface_manager.client_iface.lock().clone().unwrap();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        // Setup pending request and task
        {
            let mut s = test_values.scheduled_scan_controller.states.lock();
            s.insert(
                1,
                ScheduledScanState::SoftwareScansActive {
                    request: fidl_common::ScheduledScanRequest::default(),
                    iface: Arc::clone(&iface) as Arc<dyn ClientIface>,
                    task: fasync::Task::spawn(async {}),
                },
            );
        }

        let attrs =
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())];
        let stop_sched_scan_message = build_nl80211_message(Nl80211Cmd::StopSchedScan, attrs);
        let stop_scan_fut = test_values.nl80211_proxy.message_v2(&stop_sched_scan_message);
        let mut stop_scan_fut = pin!(stop_scan_fut);

        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        assert_matches!(exec.run_until_stalled(&mut stop_scan_fut), Poll::Ready(Ok(Ok(_))));

        // Verify that state transitioned to empty map
        assert!(test_values.scheduled_scan_controller.states.lock().is_empty());
    }

    #[fuchsia::test]
    fn test_start_sched_scan_spawns_task() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        let start_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StartSchedScan,
            vec![
                Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
                Nl80211Attr::ScanSsids(vec![b"TestSSID".to_vec()]),
                Nl80211Attr::SchedScanInterval(40),
            ],
        );
        let start_scan_fut = test_values.nl80211_proxy.message_v2(&start_sched_scan_message);
        let mut start_scan_fut = pin!(start_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut start_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        assert_matches!(
            test_values.scheduled_scan_controller.states.lock().get(&1),
            Some(ScheduledScanState::FirmwareScansActive { .. })
        );
    }

    #[fuchsia::test]
    fn test_stop_sched_scan_clears_state() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        // Start it first
        let start_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StartSchedScan,
            vec![
                Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
                Nl80211Attr::ScanSsids(vec![b"TestSSID".to_vec()]),
                Nl80211Attr::SchedScanInterval(40),
            ],
        );
        let start_scan_fut = test_values.nl80211_proxy.message_v2(&start_sched_scan_message);
        let mut start_scan_fut = pin!(start_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let _ = exec.run_until_stalled(&mut start_scan_fut);

        // Stop it
        let stop_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StopSchedScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let stop_scan_fut = test_values.nl80211_proxy.message_v2(&stop_sched_scan_message);
        let mut stop_scan_fut = pin!(stop_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut stop_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        assert!(test_values.scheduled_scan_controller.states.lock().is_empty());
    }

    #[fuchsia::test]
    fn test_on_scheduled_scan_stopped() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let client_iface = iface_manager.client_iface.lock().clone().unwrap();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        // Setup multicast channel
        let (proxy, mut stream) = create_proxy_and_stream::<fidl_wlanix::Nl80211MulticastMarker>();
        test_values.state.lock().scan_multicast_proxies.add_proxy(proxy);

        // Start it
        let start_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StartSchedScan,
            vec![
                Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
                Nl80211Attr::ScanSsids(vec![b"TestSSID".to_vec()]),
                Nl80211Attr::SchedScanInterval(40),
            ],
        );
        let start_scan_fut = test_values.nl80211_proxy.message_v2(&start_sched_scan_message);
        let mut start_scan_fut = pin!(start_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let _ = exec.run_until_stalled(&mut start_scan_fut);

        // Get control handle
        let control_handle = client_iface.pno_transaction_handle.lock().take().unwrap();

        // Send Epitaph instead of event
        control_handle.shutdown_with_epitaph(Ok(()));

        // Run executor to let task process it
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        // Verify state cleaned up
        assert!(test_values.scheduled_scan_controller.states.lock().is_empty());

        // Verify multicast message sent
        let mut stream_fut = stream.next();
        let event =
            assert_matches!(exec.run_until_stalled(&mut stream_fut), Poll::Ready(Some(Ok(e))) => e);
        match event {
            fidl_wlanix::Nl80211MulticastRequest::Message { payload, .. } => {
                let msg = payload.message.as_ref().unwrap();
                let genl_msg = expect_nl80211_message(msg);
                assert_eq!(genl_msg.header.cmd, Nl80211Cmd::SchedScanStopped as u8);
            }
            _ => panic!("unexpected event"),
        }
    }

    #[fuchsia::test]
    fn test_on_scheduled_scan_results() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let client_iface = iface_manager.client_iface.lock().clone().unwrap();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        // Setup multicast channel
        let (proxy, mut stream) = create_proxy_and_stream::<fidl_wlanix::Nl80211MulticastMarker>();
        test_values.state.lock().scan_multicast_proxies.add_proxy(proxy);

        // Start it
        let start_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StartSchedScan,
            vec![
                Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
                Nl80211Attr::ScanSsids(vec![b"TestSSID".to_vec()]),
                Nl80211Attr::SchedScanInterval(40),
            ],
        );
        let start_scan_fut = test_values.nl80211_proxy.message_v2(&start_sched_scan_message);
        let mut start_scan_fut = pin!(start_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let _ = exec.run_until_stalled(&mut start_scan_fut);

        // Get control handle
        let control_handle = client_iface.pno_transaction_handle.lock().take().unwrap();

        // Create a fake VMO with scan results
        let results = vec![ifaces::test_utils::fake_scan_result()];
        let vmo = wlan_common::scan::write_vmo(results).unwrap();

        // Send event
        control_handle.send_on_scheduled_scan_matches_available(vmo).unwrap();

        // Run executor to let task process it
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        // Verify cache updated
        let calls = client_iface.calls.lock();
        assert!(calls.iter().any(|c| matches!(c, ClientIfaceCall::UpdateLastScanResults(_))));

        // Verify multicast message sent
        let mut stream_fut = stream.next();
        let event =
            assert_matches!(exec.run_until_stalled(&mut stream_fut), Poll::Ready(Some(Ok(e))) => e);
        match event {
            fidl_wlanix::Nl80211MulticastRequest::Message { payload, .. } => {
                let msg = payload.message.as_ref().unwrap();
                let genl_msg = expect_nl80211_message(msg);
                assert_eq!(genl_msg.header.cmd, Nl80211Cmd::SchedScanResults as u8);
            }
            _ => panic!("unexpected event"),
        }
    }

    #[fuchsia::test]
    fn test_start_sched_scan_only_when_charging() {
        let mut exec = fasync::TestExecutor::new();
        let state = Arc::new(Mutex::new(WifiState::default()));
        let iface_manager = Arc::new(TestIfaceManager::new());
        let _ = exec.run_singlethreaded(
            iface_manager.create_client_iface(ifaces::test_utils::FAKE_IFACE_RESPONSE.phy_id),
        );

        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);

        let (proxy, stream) = create_proxy_and_stream::<fidl_wlanix::Nl80211Marker>();

        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (scheduled_scan_event_sender, _) = mpsc::unbounded();
        let scheduled_scan_controller = Arc::new(ScheduledScanController::new(
            telemetry_sender.clone(),
            scheduled_scan_event_sender,
        ));
        let nl80211_fut = serve_nl80211(
            stream,
            Arc::clone(&state),
            Arc::clone(&iface_manager),
            telemetry_sender,
            Arc::clone(&log_throttler),
            Arc::clone(&scheduled_scan_controller),
        );
        let mut nl80211_fut = Box::pin(nl80211_fut);
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);

        // Not charging, scheduled scans should be rejected.
        scheduled_scan_controller.set_charging_state(false);

        let start_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StartSchedScan,
            vec![
                Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
                Nl80211Attr::SchedScanInterval(10000),
            ],
        );
        let start_sched_scan_fut = proxy.message_v2(&start_sched_scan_message);

        let mut start_sched_scan_fut = pin!(start_sched_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);

        // Check that the scan is rejected.
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut start_sched_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        // Verify the iface_manager did receive a StartSchedScan call inside its history
        let client_calls = iface_manager.get_iface_call_history();
        assert!(
            client_calls
                .lock()
                .iter()
                .any(|c| matches!(c, ifaces::test_utils::ClientIfaceCall::StartSchedScan { .. }))
        );

        // Case 2: is_charging is true
        scheduled_scan_controller.set_charging_state(true);
        let start_sched_scan_message = build_nl80211_message(
            Nl80211Cmd::StartSchedScan,
            vec![
                Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()),
                Nl80211Attr::SchedScanInterval(10000),
            ],
        );
        let start_sched_scan_fut = proxy.message_v2(&start_sched_scan_message);
        let mut start_sched_scan_fut = pin!(start_sched_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);

        // It should ack successfully
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut start_sched_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        // It should have called start_sched_scan on the client interface mock
        assert!(
            client_calls
                .lock()
                .iter()
                .any(|c| matches!(c, ifaces::test_utils::ClientIfaceCall::StartSchedScan { .. }))
        );

        // Let it run background loop stalling
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn test_start_sched_scan_sends_results_workaround() {
        use futures::TryStreamExt;
        use ieee80211::Ssid;
        use wlan_common::fake_fidl_bss_description;
        use wlan_common::test_utils::fake_stas::FakeProtectionCfg;

        let mut exec = fasync::TestExecutor::new();
        let state = Arc::new(Mutex::new(WifiState::default()));
        let iface_manager = Arc::new(TestIfaceManager::new_with_client());
        let client_iface = iface_manager.client_iface.lock().clone().unwrap();

        // Make start_sched_scan fail with NOT_SUPPORTED to trigger workaround
        *client_iface.fail_start_sched_scan.lock() = true;

        // Populate scan results in the mock
        let scan_result = fidl_sme::ScanResult {
            bss_description: fake_fidl_bss_description!(protection => FakeProtectionCfg::Open,
                ssid: Ssid::try_from(b"TestMatchSSID".to_vec()).unwrap(),
                bssid: [1, 2, 3, 4, 5, 6],
                rssi_dbm: -30,
            ),
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![],
            }),
            timestamp_nanos: 0,
        };
        client_iface.scan_results.lock().push(scan_result);

        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);

        let (proxy, stream) = create_proxy_and_stream::<fidl_wlanix::Nl80211Marker>();

        // Set the scan multicast channel for sched scan results to be sent over
        let (mcast_proxy, mut mcast_stream) =
            create_proxy_and_stream::<fidl_wlanix::Nl80211MulticastMarker>();
        state.lock().scan_multicast_proxies.add_proxy(mcast_proxy);

        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (event_sender, event_receiver) = mpsc::unbounded();
        let scheduled_scan_controller =
            Arc::new(ScheduledScanController::new(telemetry_sender.clone(), event_sender));
        let nl80211_fut = serve_nl80211(
            stream,
            Arc::clone(&state),
            Arc::clone(&iface_manager),
            telemetry_sender,
            Arc::clone(&log_throttler),
            Arc::clone(&scheduled_scan_controller),
        );
        let event_loop_fut = handle_scheduled_scan_events(Arc::clone(&state), event_receiver);
        let combined_fut = async move {
            let nl80211_fut = std::pin::pin!(nl80211_fut);
            let event_loop_fut = std::pin::pin!(event_loop_fut);
            futures::future::select(nl80211_fut, event_loop_fut).await;
        };
        let mut nl80211_fut = Box::pin(combined_fut);
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);

        // Set charging to true so scan starts
        scheduled_scan_controller.set_charging_state(true);

        let mut attrs = Vec::new();
        attrs.push(Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into()));
        attrs.push(Nl80211Attr::SchedScanInterval(10000));

        // Add match set for TestMatchSSID
        let match_sets = vec![vec![Nl80211SchedScanMatchAttr::Ssid(b"TestMatchSSID".to_vec())]];
        attrs.push(Nl80211Attr::SchedScanMatch(match_sets));

        let start_sched_scan_message = build_nl80211_message(Nl80211Cmd::StartSchedScan, attrs);
        let start_sched_scan_fut = proxy.message_v2(&start_sched_scan_message);
        let mut start_sched_scan_fut = pin!(start_sched_scan_fut);

        // Poll nl80211_fut to process message and spawn software PNO loop
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);

        // Verify that the sched scan command was acked.
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut start_sched_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        // Progress the future running the software PNO loop (it triggers scan immediately)
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);

        // Check if we received the multicast message
        let mcast_msg = assert_matches!(
            exec.run_until_stalled(&mut mcast_stream.try_next()),
            Poll::Ready(Ok(Some(msg))) => msg
        );

        let mcast_payload = assert_matches!(
            mcast_msg,
            fidl_wlanix::Nl80211MulticastRequest::Message { payload, .. } => payload
        );
        let genl_msg = expect_nl80211_message(mcast_payload.message.as_ref().unwrap());
        assert_eq!(genl_msg.payload.cmd, Nl80211Cmd::SchedScanResults);
    }

    // This test verifies the software scheduled scan workaround behavior when battery info changes.
    // TODO(b/498247761): Remove once firmware scheduled scans are supported.
    #[fuchsia::test]
    fn test_software_scheduled_scan_report_battery_updates() {
        let (mut test_helper, mut test_fut) = setup_supplicant_test();
        let watcher = test_helper.get_battery_watcher();

        let info = fidl_fuchsia_power_battery::BatteryInfo {
            charge_status: Some(fidl_fuchsia_power_battery::ChargeStatus::Charging),
            ..Default::default()
        };

        // Run battery updates stream and verify state updated to charging
        let send_fut = watcher.on_change_battery_info(&info, None);
        let mut send_fut = std::pin::pin!(send_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut send_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut send_fut), Poll::Ready(Ok(())));

        assert!(test_helper.scheduled_scan_controller.is_charging());

        // Test transitioning to not charging
        let info_not_charging = fidl_fuchsia_power_battery::BatteryInfo {
            charge_status: Some(fidl_fuchsia_power_battery::ChargeStatus::Discharging),
            ..Default::default()
        };

        let send_fut = watcher.on_change_battery_info(&info_not_charging, None);
        let mut send_fut = std::pin::pin!(send_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut send_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut send_fut), Poll::Ready(Ok(())));

        assert!(!test_helper.scheduled_scan_controller.is_charging());
    }

    #[fuchsia::test]
    fn get_station_during_scan() {
        let mut exec = fasync::TestExecutor::new();
        let (iface_manager, scan_end_sender) =
            TestIfaceManager::new_with_client_and_scan_end_sender();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        let mut mcast_stream = get_nl80211_mcast(&test_values.nl80211_proxy, "scan");
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let trigger_scan_message = build_nl80211_message(
            Nl80211Cmd::TriggerScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let trigger_scan_fut = test_values.nl80211_proxy.message_v2(&trigger_scan_message);

        let mut trigger_scan_fut = pin!(trigger_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanStart))
        );

        // While the scan is running, handle a GetStation request.
        {
            let get_station_message = build_nl80211_message(
                Nl80211Cmd::GetStation,
                vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
            );
            let get_station_fut = test_values.nl80211_proxy.message_v2(&get_station_message);

            let mut get_station_fut = pin!(get_station_fut);
            assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
            let responses = deserialize(assert_matches!(
                exec.run_until_stalled(&mut get_station_fut),
                Poll::Ready(Ok(Ok(r))) => r));
            assert_eq!(responses.len(), 1);
            assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Message(_));
        }

        // Now the scan can wrap up.
        scan_end_sender.send(Ok(ScanEnd::Complete)).expect("Failed to send scan result");
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut trigger_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 1);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Ack(_));

        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanResult {
                result: wlan_telemetry::ScanResult::Complete { .. }
            }))
        );

        // With our faked scan results we expect an immediate multicast notification.
        let mcast_msg =
            assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Ready(msg) => msg);
        assert_eq!(mcast_msg.payload.cmd, Nl80211Cmd::NewScanResults);
    }

    #[fuchsia::test]
    fn trigger_scan_no_iface_arg() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let trigger_scan_message = build_nl80211_message(Nl80211Cmd::TriggerScan, vec![]);
        let trigger_scan_fut = test_values.nl80211_proxy.message_v2(&trigger_scan_message);

        let mut trigger_scan_fut = pin!(trigger_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut trigger_scan_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_INVALID_ARGS)))
        );
    }

    #[fuchsia::test]
    fn trigger_scan_invalid_iface() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let trigger_scan_message =
            build_nl80211_message(Nl80211Cmd::TriggerScan, vec![Nl80211Attr::IfaceIndex(123)]);
        let trigger_scan_fut = test_values.nl80211_proxy.message_v2(&trigger_scan_message);

        let mut trigger_scan_fut = pin!(trigger_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut trigger_scan_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_NOT_FOUND)))
        );
    }

    #[test_case(Ok(ScanEnd::Cancelled), wlan_telemetry::ScanResult::Cancelled; "Scan cancelled by user")]
    #[test_case(Err(format_err!("scan ended unexpectedly")), wlan_telemetry::ScanResult::Failed; "Scan fails with error")]
    fn trigger_scan_failed_or_cancelled(
        scan_result: Result<ScanEnd, Error>,
        expected_telemetry_result: wlan_telemetry::ScanResult,
    ) {
        let mut exec = fasync::TestExecutor::new();
        let (iface_manager, scan_end_sender) =
            TestIfaceManager::new_with_client_and_scan_end_sender();
        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        let mut mcast_stream = get_nl80211_mcast(&test_values.nl80211_proxy, "scan");
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let trigger_scan_message = build_nl80211_message(
            Nl80211Cmd::TriggerScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let trigger_scan_fut = test_values.nl80211_proxy.message_v2(&trigger_scan_message);
        let mut trigger_scan_fut = pin!(trigger_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut trigger_scan_fut), Poll::Ready(_));
        assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanStart))
        );

        // After ending the scan we expect wlanix to broadcast the scan abort.
        scan_end_sender.send(scan_result).expect("Failed to send scan result");
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let message = assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Ready(message) => message);
        assert_eq!(message.payload.cmd, Nl80211Cmd::ScanAborted);

        let scan_result = assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanResult { result })) => result
        );
        assert_eq!(scan_result, expected_telemetry_result);
    }

    #[fuchsia::test]
    fn abort_scan_sends_telemetry() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let mut mcast_stream = get_nl80211_mcast(&test_values.nl80211_proxy, "scan");
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        let next_mcast = next_mcast_message(&mut mcast_stream);
        let mut next_mcast = pin!(next_mcast);
        assert_matches!(exec.run_until_stalled(&mut next_mcast), Poll::Pending);

        let abort_scan_message = build_nl80211_message(
            Nl80211Cmd::AbortScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let abort_scan_fut = test_values.nl80211_proxy.message_v2(&abort_scan_message);

        let mut abort_scan_fut = pin!(abort_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut abort_scan_fut), Poll::Ready(_));

        let scan_result = assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanResult { result })) => result
        );
        assert_eq!(scan_result, wlan_telemetry::ScanResult::Cancelled);
    }

    #[fuchsia::test]
    fn get_scan_results() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let get_scan_message = build_nl80211_message(
            Nl80211Cmd::GetScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let get_scan_fut = test_values.nl80211_proxy.message_v2(&get_scan_message);

        let mut get_scan_fut = pin!(get_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut get_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));
        assert_eq!(responses.len(), 2);
        assert_matches!(responses[0], fidl_wlanix::Nl80211Message::Message(_));
        assert_matches!(responses[1], fidl_wlanix::Nl80211Message::Done(_));
    }

    #[fuchsia::test]
    fn get_scan_results_with_invalid_channel() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = TestIfaceManager::new_with_client();
        let client_iface = iface_manager.get_client_iface();

        let mut valid_scan = ifaces::test_utils::fake_scan_result();
        let valid_bssid = [1, 1, 1, 1, 1, 1];
        valid_scan.bss_description.bssid = valid_bssid;
        valid_scan.bss_description.primary.number = 1;

        let mut invalid_scan = ifaces::test_utils::fake_scan_result();
        invalid_scan.bss_description.bssid = [2, 2, 2, 2, 2, 2];
        // Channel 197 is invalid but could be a valid channel one day.
        invalid_scan.bss_description.primary.number = 197;

        *client_iface.scan_results.lock() = vec![valid_scan, invalid_scan];

        let mut test_values = setup_nl80211_test_with_iface_manager(&mut exec, iface_manager);

        let get_scan_message = build_nl80211_message(
            Nl80211Cmd::GetScan,
            vec![Nl80211Attr::IfaceIndex(ifaces::test_utils::FAKE_IFACE_RESPONSE.id.into())],
        );
        let get_scan_fut = test_values.nl80211_proxy.message_v2(&get_scan_message);

        let mut get_scan_fut = pin!(get_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut get_scan_fut),
            Poll::Ready(Ok(Ok(r))) => r));

        // We expect only the valid result and the "Done" message. One "NewScanResults" message is
        // sent for each scan result.
        assert_eq!(responses.len(), 2);
        let message = expect_nl80211_message(&responses[0]);
        assert_eq!(message.payload.cmd, Nl80211Cmd::NewScanResults);
        assert!(message.payload.attrs.iter().any(|attr| matches!(attr, Nl80211Attr::Bss(bss_attrs)
            if bss_attrs.iter().any(|bss_attr| matches!(bss_attr, crate::nl80211::Nl80211BssAttr::Bssid(bssid) if *bssid == valid_bssid))
        )));
        assert_matches!(responses[1], fidl_wlanix::Nl80211Message::Done(_));
    }

    #[fuchsia::test]
    fn get_scan_results_no_iface_args() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let get_scan_message = build_nl80211_message(Nl80211Cmd::GetScan, vec![]);
        let get_scan_fut = test_values.nl80211_proxy.message_v2(&get_scan_message);

        let mut get_scan_fut = pin!(get_scan_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut get_scan_fut),
            Poll::Ready(Ok(Err(zx::sys::ZX_ERR_INVALID_ARGS)))
        );
    }

    fn deserialize(vmo: zx::Vmo) -> Vec<fidl_wlanix::Nl80211Message> {
        let value = vmo.read_to_vec(0, vmo.get_content_size().unwrap()).unwrap();
        fidl::unpersist::<fidl_wlanix::Nl80211MessageArray>(&value).unwrap().messages
    }

    #[fuchsia::test]
    fn get_reg() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let get_reg_message =
            build_nl80211_message(Nl80211Cmd::GetReg, vec![Nl80211Attr::Wiphy(123)]);
        let get_reg_fut = test_values.nl80211_proxy.message_v2(&get_reg_message);

        let mut get_reg_fut = pin!(get_reg_fut);
        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut get_reg_fut),
            Poll::Ready(Ok(Ok(r))) => r));

        assert_eq!(responses.len(), 1);
        let message = expect_nl80211_message(&responses[0]);
        assert_eq!(message.payload.cmd, Nl80211Cmd::GetReg);
        // The default for the test class is XX
        assert!(message.payload.attrs.contains(&Nl80211Attr::RegulatoryRegionAlpha2(*b"XX")));
    }

    #[fuchsia::test]
    fn get_reg_worldwide_is_zeroes() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, stream) = create_proxy_and_stream::<fidl_wlanix::Nl80211Marker>();

        let state = Arc::new(Mutex::new(WifiState::default()));
        let iface_manager = Arc::new(TestIfaceManager::new_with_client());
        {
            // Set the Fake IfaceManager to return country code WW
            let set_country_fut = iface_manager.set_country(0, *b"WW");
            let mut set_country_fut = pin!(set_country_fut);
            assert_matches!(
                exec.run_until_stalled(&mut set_country_fut),
                Poll::Ready(Result::Ok(()))
            );
        }
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let log_throttler =
            Arc::new(Mutex::new(ThrottledErrorLogger::new(MIN_MINUTES_BETWEEN_FREQUENT_ERRORS)));
        let (scheduled_scan_event_sender, _) = mpsc::unbounded();
        let scheduled_scan_controller = Arc::new(ScheduledScanController::new(
            telemetry_sender.clone(),
            scheduled_scan_event_sender,
        ));
        let nl80211_fut = serve_nl80211(
            stream,
            state,
            iface_manager,
            telemetry_sender,
            Arc::clone(&log_throttler),
            scheduled_scan_controller,
        );
        let mut nl80211_fut = pin!(nl80211_fut);

        let get_reg_message =
            build_nl80211_message(Nl80211Cmd::GetReg, vec![Nl80211Attr::Wiphy(123)]);
        let get_reg_fut = proxy.message_v2(&get_reg_message);

        let mut get_reg_fut = pin!(get_reg_fut);
        assert_matches!(exec.run_until_stalled(&mut nl80211_fut), Poll::Pending);
        let responses = deserialize(assert_matches!(
            exec.run_until_stalled(&mut get_reg_fut),
            Poll::Ready(Ok(Ok(r))) => r));

        assert_eq!(responses.len(), 1);
        let message = expect_nl80211_message(&responses[0]);
        assert_eq!(message.payload.cmd, Nl80211Cmd::GetReg);
        // The country code 00 should be returned instead of WW
        assert!(message.payload.attrs.contains(&Nl80211Attr::RegulatoryRegionAlpha2(*b"00")));
    }

    #[test]
    fn test_reset_tx_power_scenario_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new());
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let power_manager = Arc::new(TestPowerManager::new());
        let phy_id = 123;

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiChipMarker>();
        let request_fut = proxy.reset_tx_power_scenario();
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the response was set on the
        // TestIfaceManager.
        let fut = handle_wifi_chip_request(
            req,
            phy_id,
            iface_manager.clone(),
            power_manager,
            telemetry_sender,
            Arc::new(Mutex::new(WifiState::default())),
        );
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify the response was sent to the client.
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Ready(Ok(())));

        // The TestIfaceManager should have a reset request logged
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(
            calls[0],
            ifaces::test_utils::IfaceManagerCall::ResetTxPowerScenario(id) => assert_eq!(id, phy_id)
        );
    }

    #[test]
    fn test_reset_tx_power_scenario_fails() {
        let mut exec = fasync::TestExecutor::new();
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let power_manager = Arc::new(TestPowerManager::new());
        let phy_id = 123;

        // Configure the reset Tx power scenario call to fail.
        let iface_manager =
            Arc::new(TestIfaceManager::new().mock_reset_tx_power_scenario_failure());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiChipMarker>();
        let request_fut = proxy.reset_tx_power_scenario();
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the response was set on the
        // TestIfaceManager.
        let fut = handle_wifi_chip_request(
            req,
            phy_id,
            iface_manager.clone(),
            power_manager,
            telemetry_sender,
            Arc::new(Mutex::new(WifiState::default())),
        );
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify the response was sent to the client.
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Ready(Ok(())));

        // The TestIfaceManager should have a reset request logged
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(
            calls[0],
            ifaces::test_utils::IfaceManagerCall::ResetTxPowerScenario(id) => assert_eq!(id, phy_id)
        );
    }

    #[test]
    fn test_set_tx_power_scenario_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new());
        let power_manager = Arc::new(TestPowerManager::new());
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let test_phy_id = 123;

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiChipMarker>();
        let request_fut =
            proxy.select_tx_power_scenario(fidl_wlanix::WifiChipTxPowerScenario::OnBodyCellOff);
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the response was set on the
        // TestIfaceManager.
        let fut = handle_wifi_chip_request(
            req,
            test_phy_id,
            iface_manager.clone(),
            power_manager,
            telemetry_sender,
            Arc::new(Mutex::new(WifiState::default())),
        );
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify the response was sent to the client.
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Ready(Ok(())));

        // The TestIfaceManager should have a set request logged
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(
            calls[0],
            ifaces::test_utils::IfaceManagerCall::SetTxPowerScenario { phy_id, scenario} => {
                assert_eq!(phy_id, test_phy_id);
                assert_eq!(scenario, fidl_internal::TxPowerScenario::BodyCellOff);
            }
        );
    }

    #[test]
    fn test_set_tx_power_scenario_fails() {
        let mut exec = fasync::TestExecutor::new();
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let power_manager = Arc::new(TestPowerManager::new());
        let test_phy_id = 123;

        // Configure the set Tx power scenario call to fail.
        let iface_manager = Arc::new(TestIfaceManager::new().mock_set_tx_power_scenario_failure());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiChipMarker>();
        let request_fut =
            proxy.select_tx_power_scenario(fidl_wlanix::WifiChipTxPowerScenario::VoiceCall);
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the response was set on the
        // TestIfaceManager.
        let fut = handle_wifi_chip_request(
            req,
            test_phy_id,
            iface_manager.clone(),
            power_manager,
            telemetry_sender,
            Arc::new(Mutex::new(WifiState::default())),
        );
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify the response was sent to the client.
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Ready(Ok(())));

        // The TestIfaceManager should have a set request logged
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(
            calls[0],
            ifaces::test_utils::IfaceManagerCall::SetTxPowerScenario { phy_id, scenario} => {
                assert_eq!(phy_id, test_phy_id);
                assert_eq!(scenario, fidl_internal::TxPowerScenario::VoiceCall);
            }
        );
    }

    #[test_case(fidl_wlanix::WifiChipTxPowerScenario::VoiceCall, Some(fidl_internal::TxPowerScenario::VoiceCall); "Voice Call")]
    #[test_case(fidl_wlanix::WifiChipTxPowerScenario::OnBodyCellOff, Some(fidl_internal::TxPowerScenario::BodyCellOff); "Body Cell Off")]
    #[test_case(fidl_wlanix::WifiChipTxPowerScenario::OnBodyCellOn, Some(fidl_internal::TxPowerScenario::BodyCellOn); "Body Cell On")]
    #[test_case(fidl_wlanix::WifiChipTxPowerScenario::OnHeadCellOff, Some(fidl_internal::TxPowerScenario::HeadCellOff); "Head Cell Off")]
    #[test_case(fidl_wlanix::WifiChipTxPowerScenario::OnHeadCellOn, Some(fidl_internal::TxPowerScenario::HeadCellOn); "Head Cell On")]
    fn test_scenario_conversion(
        wifi_chip_scenario: fidl_wlanix::WifiChipTxPowerScenario,
        internal_scenario: Option<fidl_internal::TxPowerScenario>,
    ) {
        assert_eq!(wifi_chip_tx_power_scenario_to_internal(wifi_chip_scenario), internal_scenario)
    }

    #[test]
    fn test_legacy_hardware_scenario_conversion_invalid() {
        assert_eq!(
            legacy_hal_tx_power_scenario_to_internal(
                fidl_wlanix::WifiLegacyHalTxPowerScenario::Invalid
            ),
            None
        )
    }

    #[test]
    fn test_legacy_hardware_scenario_conversion_default() {
        assert_eq!(
            legacy_hal_tx_power_scenario_to_internal(
                fidl_wlanix::WifiLegacyHalTxPowerScenario::Default
            ),
            Some(fidl_internal::TxPowerScenario::Default)
        )
    }

    #[test]
    fn test_legacy_hardware_scenario_conversion_voice_call() {
        assert_eq!(
            legacy_hal_tx_power_scenario_to_internal(
                fidl_wlanix::WifiLegacyHalTxPowerScenario::VoiceCallLegacy
            ),
            Some(fidl_internal::TxPowerScenario::VoiceCall)
        )
    }

    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOn; "cell on")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnUnfolded; "cell on unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnUnfoldedCap; "cell on unfolded cap")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnBt; "cell on BT")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnBtUnfolded; "cell on BT unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOnBtUnfoldedCap; "cell on BT unfolded cap")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspot; "hotspot")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBt; "hotspot BT")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotMmw; "hotspot mmw")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBtMmw; "hotspot BT mmw")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotUnfolded; "hotspot unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBtUnfolded; "hotspot BT unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotMmwUnfolded; "hotspot mmw unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyHotspotBtMmwUnfolded; "hotspot BT mmw unfolded")]
    fn test_legacy_hardware_scenario_conversion_on_body_cell_on(
        scenario: fidl_wlanix::WifiLegacyHalTxPowerScenario,
    ) {
        assert_eq!(
            legacy_hal_tx_power_scenario_to_internal(scenario),
            Some(fidl_internal::TxPowerScenario::BodyCellOn)
        )
    }

    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOff; "cell off")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOffUnfolded; "unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyCellOffUnfoldedCap; "unfolded cap")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyRearCamera; "rear camera")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnBodyVideoRecording; "video recording")]
    fn test_legacy_hardware_scenario_conversion_on_body_cell_off(
        scenario: fidl_wlanix::WifiLegacyHalTxPowerScenario,
    ) {
        assert_eq!(
            legacy_hal_tx_power_scenario_to_internal(scenario),
            Some(fidl_internal::TxPowerScenario::BodyCellOff)
        )
    }

    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOn; "cell on")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOnUnfolded; "unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspot; "hotspot")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspotMmw; "hotspot mmw")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspotUnfolded; "hotspot unfolded")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadHotspotMmwUnfolded; "hotspot mmw unfolded")]
    fn test_legacy_hardware_scenario_conversion_on_head_cell_on(
        scenario: fidl_wlanix::WifiLegacyHalTxPowerScenario,
    ) {
        assert_eq!(
            legacy_hal_tx_power_scenario_to_internal(scenario),
            Some(fidl_internal::TxPowerScenario::HeadCellOn)
        )
    }

    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOff; "cell off")]
    #[test_case(fidl_wlanix::WifiLegacyHalTxPowerScenario::OnHeadCellOffUnfolded; "unfolded")]
    fn test_legacy_hardware_scenario_conversion_on_head_cell_off(
        scenario: fidl_wlanix::WifiLegacyHalTxPowerScenario,
    ) {
        assert_eq!(
            legacy_hal_tx_power_scenario_to_internal(scenario),
            Some(fidl_internal::TxPowerScenario::HeadCellOff)
        )
    }

    #[test]
    fn test_reset_tx_power_scenario_no_phys() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new().mock_no_phys_available());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.reset_tx_power_scenario();
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since there are no PHYs available.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::Internal)))
        );

        // Verify that the list PHYs request was the only call to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
    }

    #[test]
    fn test_reset_tx_power_scenario_list_phys_fails() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new().mock_list_phys_failure());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.reset_tx_power_scenario();
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the error response was set on
        // the TestIfaceManager.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::Internal)))
        );

        // Verify that the list PHYs request was the only call to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
    }

    #[test]
    fn test_reset_tx_power_scenario_reset_fails() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager =
            Arc::new(TestIfaceManager::new().mock_reset_tx_power_scenario_failure());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.reset_tx_power_scenario();
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the error response was set on
        // the TestIfaceManager.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::Internal)))
        );

        // Verify the calls made to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
        assert_matches!(calls[1], ifaces::test_utils::IfaceManagerCall::ResetTxPowerScenario(1));
    }

    #[test]
    fn test_reset_tx_power_scenario_reset_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.reset_tx_power_scenario();
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Process the reset request and observe an immediate success.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify the response was sent to the client.
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Ready(Ok(Ok(()))));

        // Verify the calls made to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
        assert_matches!(calls[1], ifaces::test_utils::IfaceManagerCall::ResetTxPowerScenario(1));
    }

    #[test]
    fn test_select_tx_power_scenario_invalid_request() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.select_tx_power_scenario(
            fidl_wlanix::WifiLegacyHalSelectTxPowerScenarioRequest {
                scenario: None,
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the request is missing the scenario.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::InvalidArgument)))
        );

        // There should not have been any interaction with the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_select_tx_power_scenario_unsupported_scenario() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.select_tx_power_scenario(
            fidl_wlanix::WifiLegacyHalSelectTxPowerScenarioRequest {
                scenario: Some(fidl_wlanix::WifiLegacyHalTxPowerScenario::unknown()),
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the requested scenario is not handled.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::InvalidArgument)))
        );

        // There should not have been any interaction with the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_select_tx_power_scenario_no_phys_available() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new().mock_no_phys_available());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.select_tx_power_scenario(
            fidl_wlanix::WifiLegacyHalSelectTxPowerScenarioRequest {
                scenario: Some(fidl_wlanix::WifiLegacyHalTxPowerScenario::Default),
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since there are no PHYs available.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::Internal)))
        );

        // Verify that the list PHYs request was the only call to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
    }

    #[test]
    fn test_select_tx_power_scenario_list_phys_fails() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new().mock_list_phys_failure());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.select_tx_power_scenario(
            fidl_wlanix::WifiLegacyHalSelectTxPowerScenarioRequest {
                scenario: Some(fidl_wlanix::WifiLegacyHalTxPowerScenario::Default),
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the error response was set on
        // the TestIfaceManager.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::Internal)))
        );

        // Verify that the list PHYs request was the only call to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
    }

    #[test]
    fn test_select_tx_power_scenario_request_fails() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new().mock_set_tx_power_scenario_failure());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.select_tx_power_scenario(
            fidl_wlanix::WifiLegacyHalSelectTxPowerScenarioRequest {
                scenario: Some(fidl_wlanix::WifiLegacyHalTxPowerScenario::Default),
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // Handle the request.  It should complete immediately since the error response was set on
        // the TestIfaceManager.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // Verify the response was sent to the client.
        assert_matches!(
            exec.run_until_stalled(&mut request_fut),
            Poll::Ready(Ok(Err(WifiLegacyHalStatus::Internal)))
        );

        // Verify that the list PHYs request was the only call to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
        assert_matches!(
            calls[1],
            ifaces::test_utils::IfaceManagerCall::SetTxPowerScenario {
                phy_id: 1,
                scenario: fidl_internal::TxPowerScenario::Default,
            }
        );
    }

    #[test]
    fn test_select_tx_power_scenario_request_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let iface_manager = Arc::new(TestIfaceManager::new());

        // Create a proxy and server to instantiate a FIDL request and responder.
        let (proxy, mut server) = create_proxy_and_stream::<fidl_wlanix::WifiLegacyHalMarker>();
        let request_fut = proxy.select_tx_power_scenario(
            fidl_wlanix::WifiLegacyHalSelectTxPowerScenarioRequest {
                scenario: Some(fidl_wlanix::WifiLegacyHalTxPowerScenario::Default),
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Pending);
        let req = assert_matches!(exec.run_until_stalled(&mut server.next()), Poll::Ready(Some(Ok(req))) => req);

        // The request should succeed since all IfaceManager operations were configured to succeed.
        let fut = handle_wifi_legacy_hal_request(req, iface_manager.clone());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify the response was sent to the client.
        assert_matches!(exec.run_until_stalled(&mut request_fut), Poll::Ready(Ok(Ok(()))));

        // Verify that the list PHYs request was the only call to the IfaceManager.
        let calls = iface_manager.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_matches!(calls[0], ifaces::test_utils::IfaceManagerCall::ListPhys);
        assert_matches!(
            calls[1],
            ifaces::test_utils::IfaceManagerCall::SetTxPowerScenario {
                phy_id: 1,
                scenario: fidl_internal::TxPowerScenario::Default,
            }
        );
    }

    #[test]
    fn test_report_battery_updates() {
        let mut exec = fasync::TestExecutor::new();

        let (battery_manager_proxy, mut battery_manager_stream) =
            create_proxy_and_stream::<fidl_fuchsia_power_battery::BatteryManagerMarker>();
        let (telemetry_sender, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let wifi_state = Arc::new(Mutex::new(WifiState::default()));
        let iface_manager = Arc::new(TestIfaceManager::new());
        let (scheduled_scan_event_sender, _) = mpsc::unbounded();
        let scheduled_scan_controller = Arc::new(ScheduledScanController::new(
            telemetry_sender.clone(),
            scheduled_scan_event_sender,
        ));
        // Instantiate a mapped test client interface to catch StopSchedScan calls from the loop
        let _ = exec.run_singlethreaded(iface_manager.create_client_iface(0));

        let test_fut = report_battery_updates_helper(
            battery_manager_proxy,
            Arc::clone(&wifi_state),
            telemetry_sender,
            scheduled_scan_controller,
        );
        let mut test_fut = pin!(test_fut);
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Verify that `BatteryManagerProxy::watch` is called
        let mut next_fut = battery_manager_stream.next();
        let battery_watcher_proxy = assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Some(Ok(fidl_fuchsia_power_battery::BatteryManagerRequest::Watch { watcher, .. }))) => watcher.into_proxy());

        // Verify that `BatteryManagerProxy::get_battery_info` is called
        let mut next_fut: futures::stream::Next<
            '_,
            fidl_fuchsia_power_battery::BatteryManagerRequestStream,
        > = battery_manager_stream.next();
        let responder = assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Some(Ok(fidl_fuchsia_power_battery::BatteryManagerRequest::GetBatteryInfo { responder, .. }))) => responder);

        // Respond with a charge status to proceed with the test
        assert_matches!(
            responder.send(&fidl_fuchsia_power_battery::BatteryInfo {
                charge_status: Some(fidl_fuchsia_power_battery::ChargeStatus::Charging),
                ..Default::default()
            }),
            Ok(())
        );
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Verify such charge status is logged to telemetry
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::BatteryChargeStatus(
                fidl_fuchsia_power_battery::ChargeStatus::Charging
            )))
        );

        // Send battery info through watcher
        let battery_info = fidl_fuchsia_power_battery::BatteryInfo {
            charge_status: Some(fidl_fuchsia_power_battery::ChargeStatus::Discharging),
            ..Default::default()
        };
        let mut on_change_battery_fut =
            battery_watcher_proxy.on_change_battery_info(&battery_info, None);
        assert_matches!(exec.run_until_stalled(&mut on_change_battery_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Verify such charge status is logged to telemetry
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::BatteryChargeStatus(
                fidl_fuchsia_power_battery::ChargeStatus::Discharging
            )))
        );

        let client_calls = iface_manager.get_iface_call_history();

        // Transition back to charging and verify that stop_sched_scan is NOT called again
        let battery_info = fidl_fuchsia_power_battery::BatteryInfo {
            charge_status: Some(fidl_fuchsia_power_battery::ChargeStatus::Charging),
            ..Default::default()
        };
        let mut on_change_battery_fut =
            battery_watcher_proxy.on_change_battery_info(&battery_info, None);
        assert_matches!(exec.run_until_stalled(&mut on_change_battery_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let client_calls_len_after_recharge = client_calls.lock().len();

        let battery_info = fidl_fuchsia_power_battery::BatteryInfo {
            charge_status: Some(fidl_fuchsia_power_battery::ChargeStatus::Full),
            charge_source: Some(fidl_fuchsia_power_battery::ChargeSource::AcAdapter),
            ..Default::default()
        };
        let mut on_change_battery_fut =
            battery_watcher_proxy.on_change_battery_info(&battery_info, None);
        assert_matches!(exec.run_until_stalled(&mut on_change_battery_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        // Ensure length of client calls has not increased (StopSchedScan was NOT called)
        assert_eq!(client_calls.lock().len(), client_calls_len_after_recharge);
    }

    #[fuchsia::test]
    fn test_serve_phy_events_on_critical_error() {
        let mut exec = fasync::TestExecutor::new();
        let state = Arc::new(Mutex::new(WifiState::default()));
        let (phy_events_proxy, phy_events_server) =
            create_proxy::<fidl_device_service::PhyEventWatcherMarker>();
        let (callback_proxy, mut callback_stream) =
            create_proxy_and_stream::<fidl_wlanix::WifiEventCallbackMarker>();
        let (_phy_events_stream, phy_events_handle) =
            phy_events_server.into_stream_and_control_handle();

        state.lock().callback.replace(callback_proxy);

        let serve_fut = serve_phy_events(phy_events_proxy, Arc::clone(&state));
        let mut serve_fut = pin!(serve_fut);

        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Simulate an OnCriticalError event.
        phy_events_handle
            .send_on_critical_error(1, fidl_internal::CriticalErrorReason::FwCrash)
            .expect("Failed to send event");

        // We should see a callback.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let callback = assert_matches!(
            exec.run_until_stalled(&mut callback_stream.next()),
            Poll::Ready(Some(Ok(callback))) => callback
        );
        assert_matches!(
            callback,
            fidl_wlanix::WifiEventCallbackRequest::OnSubsystemRestart { payload, .. } => {
                assert_eq!(payload.status, Some(zx::sys::ZX_ERR_INTERNAL));
            }
        );
    }

    #[fuchsia::test]
    fn test_wifi_sta_iface_set_mac_address() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();
        let mac: [u8; 6] = [1, 2, 3, 4, 5, 6];

        let set_mac_fut = test_helper.wifi_sta_iface_proxy.set_mac_address(&mac);
        let mut set_mac_fut = pin!(set_mac_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut set_mac_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut set_mac_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_matches!(response, Ok(()));

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::SetMacAddress(addr) => {
            assert_eq!(*addr, mac);
        });
    }

    #[fuchsia::test]
    fn test_wifi_sta_iface_get_apf_packet_filter_support() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let get_apt_support_fut = test_helper.wifi_sta_iface_proxy.get_apf_packet_filter_support();
        let mut get_apt_support_fut = pin!(get_apt_support_fut);
        assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_apt_support_fut),
            Poll::Pending
        );
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut get_apt_support_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_matches!(response, Ok(fidl_wlanix::WifiStaIfaceGetApfPacketFilterSupportResponse {
            version,
            max_filter_length,
            ..
        }) => {
            assert_eq!(version, Some(1));
            assert_eq!(max_filter_length, Some(1));

        });

        let calls = test_helper.iface_manager.calls.lock();
        assert_matches!(
            &calls[calls.len() - 1],
            ifaces::test_utils::IfaceManagerCall::QueryIfaceCapabilities(_)
        );
    }

    #[fuchsia::test]
    fn test_wifi_sta_iface_install_apf_packet_filter() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();
        let expected_filter = vec![1, 2, 3, 4];

        let install_apf_fut = test_helper.wifi_sta_iface_proxy.install_apf_packet_filter(
            &fidl_fuchsia_wlan_wlanix::WifiStaIfaceInstallApfPacketFilterRequest {
                program: Some(expected_filter.clone()),
                ..Default::default()
            },
        );
        let mut install_apf_fut = pin!(install_apf_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut install_apf_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut install_apf_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_matches!(response, Ok(()));

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::InstallApfPacketFilter(filter) => {
            assert_eq!(*filter, expected_filter);
        });
    }

    #[fuchsia::test]
    fn test_wifi_sta_iface_read_apf_packet_filter_data() {
        let (mut test_helper, mut test_fut) = setup_wifi_test();

        let read_apf_fut = test_helper.wifi_sta_iface_proxy.read_apf_packet_filter_data();
        let mut read_apf_fut = pin!(read_apf_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut read_apf_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);
        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut read_apf_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_matches!(response, Ok(fidl_wlanix::WifiStaIfaceReadApfPacketFilterDataResponse {
            memory,
            ..
        }) => {
            assert_eq!(memory, Some(vec![2, 2, 2, 2]));
        });

        let iface_calls = test_helper.iface_manager.get_iface_call_history();
        assert_matches!(&iface_calls.lock()[0], ClientIfaceCall::ReadApfPacketFilterData);
    }

    #[fuchsia::test]
    fn test_wifi_stop_failure_logs_telemetry() {
        let iface_manager = TestIfaceManager::new().mock_power_down_failure();
        let (mut test_helper, mut test_fut) = setup_wifi_test_with_iface_manager(iface_manager);

        let stop_fut = test_helper.wifi_proxy.stop();
        let mut stop_fut = pin!(stop_fut);
        assert_matches!(test_helper.exec.run_until_stalled(&mut stop_fut), Poll::Pending);
        assert_matches!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let response = assert_matches!(
            test_helper.exec.run_until_stalled(&mut stop_fut),
            Poll::Ready(Ok(response)) => response
        );
        assert_matches!(response, Err(_));

        assert_matches!(
            test_helper.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ChipPowerDownFailure))
        );
    }

    #[fuchsia::test]
    fn test_multiple_multicast_clients() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = setup_nl80211_test(&mut exec);

        let (client_end1, mut stream1) =
            fidl::endpoints::create_request_stream::<fidl_wlanix::Nl80211MulticastMarker>();
        test_values
            .nl80211_proxy
            .get_multicast(fidl_wlanix::Nl80211GetMulticastRequest {
                group: Some("scan".to_string()),
                multicast: Some(client_end1),
                ..Default::default()
            })
            .expect("failed to request multicast");

        let (client_end2, mut stream2) =
            fidl::endpoints::create_request_stream::<fidl_wlanix::Nl80211MulticastMarker>();
        test_values
            .nl80211_proxy
            .get_multicast(fidl_wlanix::Nl80211GetMulticastRequest {
                group: Some("scan".to_string()),
                multicast: Some(client_end2),
                ..Default::default()
            })
            .expect("failed to request multicast");

        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);
        assert_eq!(test_values.state.lock().scan_multicast_proxies.len(), 2);

        // Send a multicast message
        test_values.state.lock().scan_multicast_proxies.send_new_scan_results(0);

        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        // Verify both clients received it
        let mut stream1_fut = stream1.next();
        assert_matches!(
            exec.run_until_stalled(&mut stream1_fut),
            Poll::Ready(Some(Ok(fidl_wlanix::Nl80211MulticastRequest::Message { .. })))
        );

        let mut stream2_fut = stream2.next();
        assert_matches!(
            exec.run_until_stalled(&mut stream2_fut),
            Poll::Ready(Some(Ok(fidl_wlanix::Nl80211MulticastRequest::Message { .. })))
        );

        // Now drop Client 1
        std::mem::drop(stream1);

        assert_matches!(exec.run_until_stalled(&mut test_values.nl80211_fut), Poll::Pending);

        // Send another message
        test_values.state.lock().scan_multicast_proxies.send_new_scan_results(0);
        // Only Client 2 should be in the proxies vector now
        assert_eq!(test_values.state.lock().scan_multicast_proxies.len(), 1);

        // Verify Client 2 received the second message
        let mut stream2_fut = stream2.next();
        assert_matches!(
            exec.run_until_stalled(&mut stream2_fut),
            Poll::Ready(Some(Ok(fidl_wlanix::Nl80211MulticastRequest::Message { .. })))
        );
    }
}
