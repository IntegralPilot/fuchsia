// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::bss_scorer::BssScorer;
use crate::security::{Credential, get_authenticator, security_matches_key_mgmt};
use anyhow::{Context, Error, bail, format_err};
use async_trait::async_trait;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_device_service as fidl_device_service;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fidl_fuchsia_wlan_stats as fidl_stats;
use fidl_fuchsia_wlan_wlanix as fidl_wlanix;
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::lock::Mutex as MutexAsync;
use futures::{FutureExt, TryFutureExt, TryStreamExt, select};
use ieee80211::{Bssid, MacAddr, Ssid};
use log::{debug, error, info, warn};
use state_recorder as power_observability_state_recorder;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::pin::pin;
use std::sync::Arc;
use strum_macros::{Display, EnumIter, EnumString};
use wlan_common::bss::{self, BssDescription};
use wlan_common::scan::{Compatibility, CompatibilityExt as _, Compatible};
use wlan_common::security::SecurityDescriptor;
use wlan_telemetry::{TelemetryEvent, TelemetrySender};

/// A long amount of time that a scan should be able to finish within. If a scan takes longer than
/// this is indicates something is wrong.
const SCAN_TIMEOUT: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(60);
const CONNECT_TIMEOUT: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(30);
const DISCONNECT_TIMEOUT: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(10);

/// If the scan results are older than this duration when handling a connect request, refresh
/// the scan results.
const SCAN_CACHE_AGE_LIMIT: zx::BootDuration = zx::BootDuration::from_seconds(30);
/// This is the acceptable difference in RSSI to treat two BSS's as similar. It is used for OWE
/// transition networks to compare the open advertising BSS with the OWE BSS it points to. If the
/// OWE BSS's signal is at least this close or better, we can restrict the active scan to its channel.
const RSSI_DELTA_FOR_CLOSE_NETWORK: i8 = 5;

#[async_trait]
pub(crate) trait IfaceManager: Send + Sync {
    type Client: ClientIface + 'static;

    async fn list_phys(&self) -> Result<Vec<u16>, Error>;
    fn list_ifaces(&self) -> Vec<u16>;
    async fn get_country(&self, phy_id: u16) -> Result<[u8; 2], Error>;
    async fn set_country(&self, phy_id: u16, country: [u8; 2]) -> Result<(), Error>;
    async fn power_down(&self, phy_id: u16) -> Result<(), Error>;
    async fn power_up(&self, phy_id: u16) -> Result<(), Error>;
    async fn get_power_state(&self, phy_id: u16) -> Result<bool, Error>;
    async fn get_power_element_dependency_token(
        &self,
        phy_id: u16,
    ) -> Result<fidl_fuchsia_power_broker::DependencyToken, Error>;
    async fn query_iface(
        &self,
        iface_id: u16,
    ) -> Result<fidl_device_service::QueryIfaceResponse, Error>;
    async fn query_iface_capabilities(
        &self,
        iface_id: u16,
    ) -> Result<fidl_common::ApfPacketFilterSupport, Error>;
    async fn create_client_iface(&self, phy_id: u16) -> Result<u16, Error>;
    async fn reset_phy(&self, phy_id: u16) -> Result<(), Error>;
    async fn reset_tx_power_scenario(&self, phy_id: u16) -> Result<(), Error>;
    async fn set_tx_power_scenario(
        &self,
        phy_id: u16,
        scenario: fidl_internal::TxPowerScenario,
    ) -> Result<(), Error>;
    async fn get_client_iface(&self, iface_id: u16) -> Result<Arc<Self::Client>, Error>;
    async fn destroy_iface(&self, iface_id: u16) -> Result<(), Error>;
}

pub struct DeviceMonitorIfaceManager {
    monitor_svc: fidl_device_service::DeviceMonitorProxy,
    ifaces: Mutex<HashMap<u16, Arc<SmeClientIface>>>,
    telemetry_sender: TelemetrySender,
}

impl DeviceMonitorIfaceManager {
    pub fn new(
        device_monitor_svc: fidl_device_service::DeviceMonitorProxy,
        telemetry_sender: TelemetrySender,
    ) -> Result<Self, Error> {
        Ok(Self {
            monitor_svc: device_monitor_svc,
            ifaces: Mutex::new(HashMap::new()),
            telemetry_sender,
        })
    }
}

#[async_trait]
impl IfaceManager for DeviceMonitorIfaceManager {
    type Client = SmeClientIface;

    async fn list_phys(&self) -> Result<Vec<u16>, Error> {
        self.monitor_svc.list_phys().await.map_err(Into::into)
    }

    fn list_ifaces(&self) -> Vec<u16> {
        self.ifaces.lock().keys().cloned().collect::<Vec<_>>()
    }

    async fn get_country(&self, phy_id: u16) -> Result<[u8; 2], Error> {
        let result = self.monitor_svc.get_country(phy_id).await.map_err(Into::<Error>::into)?;
        match result {
            Ok(get_country_response) => Ok(get_country_response.alpha2),
            Err(e) => match zx::Status::ok(e) {
                Err(e) => Err(e.into()),
                Ok(()) => Err(format_err!("get_country returned error with ok status")),
            },
        }
    }

    async fn set_country(&self, phy_id: u16, country: [u8; 2]) -> Result<(), Error> {
        let result = self
            .monitor_svc
            .set_country(&fidl_device_service::SetCountryRequest { phy_id, alpha2: country })
            .await
            .map_err(Into::<Error>::into)?;
        match zx::Status::ok(result) {
            Ok(()) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn power_down(&self, phy_id: u16) -> Result<(), Error> {
        let result = self.monitor_svc.power_down(phy_id).await.map_err(Into::<Error>::into)?;
        match result {
            Ok(()) => Ok(()),
            Err(e) => match zx::Status::ok(e) {
                Ok(()) => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }

    async fn power_up(&self, phy_id: u16) -> Result<(), Error> {
        let result = self.monitor_svc.power_up(phy_id).await.map_err(Into::<Error>::into)?;
        match result {
            Ok(()) => Ok(()),
            Err(e) => match zx::Status::ok(e) {
                Ok(()) => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }

    async fn get_power_state(&self, phy_id: u16) -> Result<bool, Error> {
        let result = self.monitor_svc.get_power_state(phy_id).await.map_err(Into::<Error>::into)?;
        match result {
            Ok(power_state) => Ok(power_state),
            Err(e) => match zx::Status::ok(e) {
                Err(e) => Err(e.into()),
                Ok(()) => Err(format_err!("get_power_state returned error with ok status")),
            },
        }
    }

    async fn get_power_element_dependency_token(
        &self,
        phy_id: u16,
    ) -> Result<fidl_fuchsia_power_broker::DependencyToken, Error> {
        let result = self
            .monitor_svc
            .get_power_element_dependency_token(phy_id)
            .await
            .map_err(Into::<Error>::into)?;
        match result {
            Ok(token) => Ok(token),
            Err(e) => {
                let status = zx::Status::err_from_raw(e);
                Err(format_err!(
                    "unable to get power dependency token for phy {}: {}",
                    phy_id,
                    status
                ))
            }
        }
    }

    async fn query_iface(
        &self,
        iface_id: u16,
    ) -> Result<fidl_device_service::QueryIfaceResponse, Error> {
        self.monitor_svc
            .query_iface(iface_id)
            .await?
            .map_err(zx::Status::err_from_raw)
            .context("Could not query iface info")
    }

    async fn query_iface_capabilities(
        &self,
        iface_id: u16,
    ) -> Result<fidl_common::ApfPacketFilterSupport, Error> {
        self.monitor_svc
            .query_iface_capabilities(iface_id)
            .await?
            .map_err(zx::Status::err_from_raw)
            .context("Could not query iface device capabilities")
    }

    async fn create_client_iface(&self, phy_id: u16) -> Result<u16, Error> {
        fuchsia_trace::duration!("wlan", "create_client_iface");
        // TODO(b/298030838): Remove unmanaged iface support when wlanix is the sole config path.
        let existing_iface_ids = self.monitor_svc.list_ifaces().await?;
        let mut unmanaged_iface_id = None;
        for iface_id in existing_iface_ids {
            if !self.ifaces.lock().contains_key(&iface_id) {
                let iface = self.query_iface(iface_id).await?;
                if iface.role == fidl_common::WlanMacRole::Client {
                    info!("Found existing client iface -- skipping iface creation");
                    unmanaged_iface_id = Some(iface_id);
                    break;
                }
            }
        }
        let (iface_id, wlanix_provisioned) = match unmanaged_iface_id {
            Some(id) => (id, false),
            None => {
                let response = self
                    .monitor_svc
                    .create_iface(&fidl_device_service::DeviceMonitorCreateIfaceRequest {
                        phy_id: Some(phy_id),
                        role: Some(fidl_fuchsia_wlan_common::WlanMacRole::Client),
                        // TODO(b/322060085): Determine if we need to populate this and how.
                        sta_address: Some([0u8; 6]),
                        ..Default::default()
                    })
                    .await?
                    .map_err(|e| format_err!("Failed to create iface: {:?}", e))?;
                (
                    response
                        .iface_id
                        .ok_or_else(|| format_err!("Missing iface id in CreateIfaceResponse"))?,
                    true,
                )
            }
        };

        let (sme_proxy, server) = create_proxy::<fidl_sme::ClientSmeMarker>();
        self.monitor_svc
            .get_client_sme(iface_id, server)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        let (telemetry_proxy, server) = create_proxy::<fidl_sme::TelemetryMarker>();
        self.monitor_svc
            .get_sme_telemetry(iface_id, server)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        let mut iface = SmeClientIface::new(
            phy_id,
            iface_id,
            sme_proxy,
            telemetry_proxy,
            self.monitor_svc.clone(),
            self.telemetry_sender.clone(),
        );
        iface.wlanix_provisioned = wlanix_provisioned;
        let _ = self.ifaces.lock().insert(iface_id, Arc::new(iface));
        Ok(iface_id)
    }

    async fn get_client_iface(&self, iface_id: u16) -> Result<Arc<SmeClientIface>, Error> {
        match self.ifaces.lock().get(&iface_id) {
            Some(iface) => Ok(iface.clone()),
            None => Err(format_err!("Requested unknown iface {}", iface_id)),
        }
    }

    async fn destroy_iface(&self, iface_id: u16) -> Result<(), Error> {
        fuchsia_trace::duration!("wlan", "destroy_client_iface");
        // TODO(b/298030838): Remove unmanaged iface support when wlanix is the sole config path.
        let removed_iface = self.ifaces.lock().remove(&iface_id);
        if let Some(iface) = removed_iface {
            if iface.wlanix_provisioned {
                let status = self
                    .monitor_svc
                    .destroy_iface(&fidl_device_service::DestroyIfaceRequest { iface_id })
                    .await?;
                zx::Status::ok(status).map_err(|e| e.into())
            } else {
                info!("Iface {} was not provisioned by wlanix. Skipping destruction.", iface_id);
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    async fn reset_phy(&self, phy_id: u16) -> Result<(), Error> {
        let result = self.monitor_svc.reset(phy_id).await.map_err(Into::<Error>::into)?;
        match result {
            Ok(()) => Ok(()),
            Err(e) => match zx::Status::ok(e) {
                Ok(()) => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }

    async fn reset_tx_power_scenario(&self, phy_id: u16) -> Result<(), Error> {
        self.telemetry_sender.send(TelemetryEvent::ResetTxPowerScenario);
        match self.monitor_svc.reset_tx_power_scenario(phy_id).await {
            Ok(result) => match result {
                Ok(()) => Ok(()),
                Err(e) => Err(format_err!("resetting Tx power scenario failed: {:?}", e)),
            },
            Err(e) => Err(format_err!("unable to reset Tx power scenario: {}", e)),
        }
    }

    async fn set_tx_power_scenario(
        &self,
        phy_id: u16,
        scenario: fidl_internal::TxPowerScenario,
    ) -> Result<(), Error> {
        self.telemetry_sender.send(TelemetryEvent::SetTxPowerScenario { scenario });
        match self.monitor_svc.set_tx_power_scenario(phy_id, scenario).await {
            Ok(result) => match result {
                Ok(()) => Ok(()),
                Err(e) => Err(format_err!("setting Tx power scenario failed: {:?}", e)),
            },
            Err(e) => Err(format_err!("unable to set Tx power scenario: {}", e)),
        }
    }
}

pub(crate) struct ConnectSuccess {
    pub bss: Box<BssDescription>,
    pub transaction_stream: fidl_sme::ConnectTransactionEventStream,
    /// If the connected network is OWE transition, include the SSID to report for the connection.
    pub ssid_if_owe_transition: Option<Ssid>,
}

#[derive(Debug)]
pub(crate) struct ConnectFail {
    pub bss: Box<BssDescription>,
    pub status_code: fidl_ieee80211::StatusCode,
    pub is_credential_rejected: bool,
    pub timed_out: bool,
    pub is_owe_transition: bool,
}

#[derive(Debug)]
pub(crate) enum ConnectResult {
    Success(ConnectSuccess),
    Fail(ConnectFail),
}

impl std::fmt::Debug for ConnectSuccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "ConnectSuccess {{ ssid: {:?}, bssid: {:?} }}", self.bss.ssid, self.bss.bssid)
    }
}

#[derive(Debug)]
pub(crate) enum ScanEnd {
    Complete,
    Cancelled,
}

#[derive(Copy, Clone, Debug, Display, EnumIter, EnumString, Eq, PartialEq, Hash)]
#[repr(u8)]
enum StaIfacePowerLevel {
    Suspended = 0,
    Normal = 1,
    NoPowerSavings = 2,
}

impl From<StaIfacePowerLevel> for u64 {
    fn from(value: StaIfacePowerLevel) -> Self {
        value as Self
    }
}

#[derive(Debug)]
pub(crate) struct PowerState {
    suspend_mode_enabled: bool,
    power_save_enabled: bool,
    apf_filter_installed: bool,
    recorder: Option<power_observability_state_recorder::EnumStateRecorder<StaIfacePowerLevel>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectedNetwork {
    pub bssid: Bssid,
    pub rssi: i8,
}

#[async_trait]
pub(crate) trait ClientIface: Sync + Send {
    async fn query(&self) -> Result<fidl_device_service::QueryIfaceResponse, Error>;
    async fn trigger_scan(&self, ssid: Option<&Ssid>, channels: Vec<u8>) -> Result<ScanEnd, Error>;
    async fn abort_scan(&self) -> Result<(), Error>;
    fn get_last_scan_results(&self) -> Vec<fidl_sme::ScanResult>;
    async fn connect_to_network(
        &self,
        ssid: &[u8],
        credential: Credential,
        requested_bssid: Option<Bssid>,
        key_mgmt: Option<fidl_wlanix::KeyMgmtMask>,
    ) -> Result<ConnectResult, Error>;
    async fn disconnect(&self) -> Result<(), Error>;
    fn get_connected_network(&self) -> Option<ConnectedNetwork>;

    fn on_disconnect(&self, info: &fidl_sme::DisconnectSource);
    fn on_signal_report(&self, ind: fidl_internal::SignalReportIndication);
    async fn set_bt_coexistence_mode(
        &self,
        mode: fidl_internal::BtCoexistenceMode,
    ) -> Result<(), fidl_wlanix::WlanixError>;
    async fn set_power_save_mode(&self, enabled: bool) -> Result<(), Error>;
    async fn set_suspend_mode(&self, enabled: bool) -> Result<(), Error>;
    async fn set_country(&self, code: [u8; 2]) -> Result<(), Error>;
    async fn set_mac_address(&self, mac_addr: [u8; 6]) -> Result<(), zx::Status>;
    async fn install_apf_packet_filter(&self, program: Vec<u8>) -> Result<(), zx::Status>;
    async fn read_apf_packet_filter_data(&self) -> Result<Vec<u8>, zx::Status>;
    async fn start_sched_scan(
        &self,
        request: fidl_common::ScheduledScanRequest,
    ) -> Result<fidl::endpoints::ClientEnd<fidl_sme::ScheduledScanTransactionMarker>, Error>;
    async fn get_signal_report(&self) -> Result<fidl_stats::SignalReport, Error>;
    async fn get_iface_stats(&self) -> Result<fidl_stats::IfaceStats, Error>;
    fn update_last_scan_results(&self, results: Vec<fidl_sme::ScanResult>);
}

#[derive(Debug, Clone)]
struct LastScanResults {
    timestamp: fasync::BootInstant,
    results: Vec<fidl_sme::ScanResult>,
}

impl LastScanResults {
    fn new(timestamp: fasync::BootInstant, results: Vec<fidl_sme::ScanResult>) -> Self {
        Self { timestamp, results }
    }
}

#[derive(Debug)]
pub(crate) struct SmeClientIface {
    phy_id: u16,
    iface_id: u16,
    monitor_svc: fidl_device_service::DeviceMonitorProxy,
    sme_proxy: fidl_sme::ClientSmeProxy,
    telemetry_proxy: fidl_sme::TelemetryProxy,
    last_scan_results: Arc<Mutex<Option<LastScanResults>>>,
    scan_abort_signal: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    connected_network: Arc<Mutex<Option<ConnectedNetwork>>>,
    // TODO(b/298030838): Remove unmanaged iface support when wlanix is the sole config path.
    wlanix_provisioned: bool,
    bss_scorer: BssScorer,
    power_state: Arc<MutexAsync<PowerState>>,
    telemetry_sender: TelemetrySender,
}

impl SmeClientIface {
    fn new(
        phy_id: u16,
        iface_id: u16,
        sme_proxy: fidl_sme::ClientSmeProxy,
        telemetry_proxy: fidl_sme::TelemetryProxy,
        monitor_svc: fidl_device_service::DeviceMonitorProxy,
        telemetry_sender: TelemetrySender,
    ) -> Self {
        let element_name = format!("wlanix-sta-iface-{iface_id}-supplicant-power");

        // As an initial guess as to an appropriate number, keep up to 100 samples in the circular
        // buffer for power observability purposes.
        static NUM_POWER_OBSERVABILITY_SAMPLES_PER_IFACE: usize = 100;
        let recorder = match power_observability_state_recorder::EnumStateRecorder::new(
            element_name,
            c"power",
            power_observability_state_recorder::RecorderOptions {
                capacity: NUM_POWER_OBSERVABILITY_SAMPLES_PER_IFACE,
                lazy_record: true,
                manager: None,
                ..Default::default()
            },
        ) {
            Ok(mut r) => {
                // We assume the driver starts out with no power savings. The higher level
                // applications don't rely on this, it's only for reporting here, so even if it's
                // wrong it won't cause logic errors. So far, this is a safe assumption based on the
                // drivers we have. TODO(https://fxbug.dev/378878423): Read this from the driver at
                // initialization.
                r.record(StaIfacePowerLevel::NoPowerSavings);
                Some(r)
            }
            Err(e) => {
                error!(
                    "Error constructing state recorder ({:?}); power observability logging will be \
                    disabled.",
                    e
                );
                None
            }
        };

        SmeClientIface {
            iface_id,
            phy_id,
            sme_proxy,
            telemetry_proxy,
            monitor_svc,
            last_scan_results: Arc::new(Mutex::new(None)),
            scan_abort_signal: Arc::new(Mutex::new(None)),
            connected_network: Arc::new(Mutex::new(None)),
            wlanix_provisioned: true,
            bss_scorer: BssScorer::new(),
            power_state: Arc::new(MutexAsync::new(PowerState {
                suspend_mode_enabled: false,
                power_save_enabled: false,
                apf_filter_installed: false,
                recorder,
            })),
            telemetry_sender,
        }
    }

    /// Sets the power level for the phy that this interface belongs to. Although this is a phy-
    /// level operation, the wlanix FIDLs expose it on an interface. When no interfaces exist, there
    /// is no way to alter power levels via the wlanix FIDLs. However, this is insignificant, as
    /// empirical measurements show that the chips have virtually no power consumption when no
    /// interfaces exist.
    async fn update_power_level(&self, new_level: StaIfacePowerLevel) -> Result<(), Error> {
        // If the Power Observability Library is initialized, report the new state
        if let Some(recorder) = &mut self.power_state.lock().await.recorder {
            recorder.record(new_level);
        }

        // Send a telemetry update with the new state
        self.telemetry_sender.send(TelemetryEvent::IfacePowerLevelChanged {
            iface_id: self.iface_id,
            iface_power_level: match new_level {
                StaIfacePowerLevel::Suspended => wlan_telemetry::IfacePowerLevel::SuspendMode,
                StaIfacePowerLevel::Normal => wlan_telemetry::IfacePowerLevel::Normal,
                StaIfacePowerLevel::NoPowerSavings => {
                    wlan_telemetry::IfacePowerLevel::NoPowerSavings
                }
            },
        });

        // Apply (or turn off) the APF optimizations for "suspend mode"
        let apf_filter_installed = {
            let power_state = self.power_state.lock().await;
            power_state.apf_filter_installed
        };
        if apf_filter_installed {
            if new_level == StaIfacePowerLevel::Suspended {
                match self.sme_proxy.set_apf_packet_filter_enabled(true).await {
                    Ok(Ok(())) => {}
                    e => {
                        warn!("Failed to enable APF packet filter: {:?}", e)
                    }
                }
            } else {
                match self.sme_proxy.set_apf_packet_filter_enabled(false).await {
                    Ok(Ok(())) => {}
                    e => {
                        warn!("Failed to disable APF packet filter: {:?}", e)
                    }
                }
            }
        } else {
            debug!("Skipping APF enable/disable as no filter is installed");
        }

        // Set the hardware power-saving level
        let power_mode = match new_level {
            StaIfacePowerLevel::Suspended => fidl_common::PowerSaveType::PsModeUltraLowPower,
            StaIfacePowerLevel::Normal => fidl_common::PowerSaveType::PsModeBalanced,
            StaIfacePowerLevel::NoPowerSavings => fidl_common::PowerSaveType::PsModePerformance,
        };
        let req = fidl_device_service::SetPowerSaveModeRequest {
            phy_id: self.phy_id,
            ps_mode: power_mode,
        };
        match self.monitor_svc.set_power_save_mode(&req).await {
            Ok(zx::sys::ZX_OK) => {}
            Ok(other) => warn!("Failed to set hardware power state: {}", other),
            Err(e) => warn!("Failed to set hardware power state: {:?}", e),
        };

        Ok(())
    }

    async fn find_owe_transition_bss(&self, bss: &BssDescription) -> Result<BssDescription, Error> {
        // Look for the BSSID and SSID of the BSS's OWE transition IE.
        let owe_ie = bss.owe_transition().context("Failed to parse OWE transition IE")?;

        // Look for the OWE BSS in the recent scan results where the OWE transition BSS was seen:
        // look for the BSSID of the transition IE.
        let found_bss = self.last_scan_results.lock().clone().and_then(|scan_results| {
            scan_results.results.into_iter().find_map(|r| {
                let bss_desc = BssDescription::try_from(r.bss_description).ok()?;
                let bssid_matches = bss_desc.bssid == owe_ie.bssid;
                if bssid_matches {
                    if bss_desc.has_owe_configured() {
                        Some(bss_desc)
                    } else {
                        info!("Matching OWE transition BSSID found but it's not OWE, ignoring");
                        None
                    }
                } else {
                    None
                }
            })
        });

        // If the OWE BSS was seen and has a good enough RSSI, only scan on that channel. Otherwise
        // scan on all channels to find the best OWE BSS in case there are multiple. Don't scan on
        // the channel specified in the IE first if it wasn't in the scan cache to keep it simple.
        let scan_channels = if let Some(owe_bss) = &found_bss {
            if owe_bss.rssi_dbm >= bss.rssi_dbm - RSSI_DELTA_FOR_CLOSE_NETWORK {
                // The BssDescription is necessary for connecting because the driver uses the SSID
                // from the BssDescription instead of the SSID from the connect request to connect.
                info!(
                    "Found OWE BSS in cache. Performing active scan on channel {}",
                    owe_bss.channel.primary
                );
                vec![owe_bss.channel.primary]
            } else {
                info!("RSSI of seen OWE BSS not good enough, starting active scan on all channels");
                vec![]
            }
        } else {
            info!("OWE BSS not found in scan results. Will active scan on all channels");
            vec![]
        };

        self.trigger_scan(Some(&owe_ie.ssid), scan_channels).await?;

        let last_scan_results = self
            .last_scan_results
            .lock()
            .clone()
            .ok_or_else(|| format_err!("No scan results after active scan"))?;

        find_matching_network_in_scan(
            &owe_ie.ssid,
            &None,
            &Some(fidl_wlanix::KeyMgmtMask::OWE),
            last_scan_results.results,
            &self.bss_scorer,
        )
        .map(|(bss, _)| bss)
        .ok_or_else(|| format_err!("OWE BSS not found after active scan"))
    }
}

#[async_trait]
impl ClientIface for SmeClientIface {
    async fn query(&self) -> Result<fidl_device_service::QueryIfaceResponse, Error> {
        self.monitor_svc
            .query_iface(self.iface_id)
            .await?
            .map_err(zx::Status::err_from_raw)
            .context("Could not query iface info")
    }

    fn update_last_scan_results(&self, results: Vec<fidl_sme::ScanResult>) {
        *self.last_scan_results.lock() =
            Some(LastScanResults::new(fasync::BootInstant::now(), results));
    }

    /// Trigger a scan with the given SSID and channels.
    ///
    /// If an SSID is provided, an active scan will be performed. Otherwise, a passive scan
    /// will be performed. If channels to scan on are not provided, the scan will be performed over
    /// all supported channels.
    async fn trigger_scan(&self, ssid: Option<&Ssid>, channels: Vec<u8>) -> Result<ScanEnd, Error> {
        let scan_request = match ssid {
            Some(ssid) => fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
                ssids: vec![ssid.to_vec()],
                channels,
            }),
            None => fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels }),
        };
        let (abort_sender, mut abort_receiver) = oneshot::channel();
        self.scan_abort_signal.lock().replace(abort_sender);
        let mut fut = pin!(
            self.sme_proxy
                .scan(&scan_request)
                .map_err(|e| format_err!("Failed to request scan: {:?}", e))
                .on_timeout(SCAN_TIMEOUT, || {
                    self.telemetry_sender.send(TelemetryEvent::SmeTimeout {
                        source: wlan_telemetry::TimeoutSource::Scan,
                    });
                    Err(format_err!("Timed out waiting on scan response from SME"))
                })
                .fuse()
        );
        select! {
            scan_results = fut => {
                let scan_result_vmo = match scan_results? {
                    Ok(vmo) => vmo,
                    Err(e) => match e {
                        fidl_sme::ScanErrorCode::ShouldWait
                        | fidl_sme::ScanErrorCode::CanceledByDriverOrFirmware => return Ok(ScanEnd::Cancelled),
                        _ => bail!("Scan ended with error: {:?}", e),
                    }
                };
                info!("Got scan results from SME.");
                *self.last_scan_results.lock() = Some(LastScanResults::new(
                    fasync::BootInstant::now(),
                    wlan_common::scan::read_vmo(scan_result_vmo)?
                ));
                self.scan_abort_signal.lock().take();
                Ok(ScanEnd::Complete)
            }
            _ = abort_receiver => {
                info!("Scan cancelled, ignoring results from SME.");
                Ok(ScanEnd::Cancelled)
            }
        }
    }

    async fn abort_scan(&self) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42079074): Actually pipe this call down to SME.
        if let Some(sender) = self.scan_abort_signal.lock().take() {
            sender.send(()).map_err(|_| format_err!("Unable to send scan abort signal"))
        } else {
            Ok(())
        }
    }

    fn get_last_scan_results(&self) -> Vec<fidl_sme::ScanResult> {
        self.last_scan_results.lock().clone().map(|r| r.results).unwrap_or_default()
    }

    async fn connect_to_network(
        &self,
        ssid: &[u8],
        credential: Credential,
        bssid: Option<Bssid>,
        key_mgmt: Option<fidl_wlanix::KeyMgmtMask>,
    ) -> Result<ConnectResult, Error> {
        // Sometimes a connect request is sent before the first scan.
        let refresh_scan = match &*self.last_scan_results.lock() {
            Some(r) => fasync::BootInstant::now() - r.timestamp > SCAN_CACHE_AGE_LIMIT,
            None => true,
        };
        if refresh_scan {
            info!("Scan results too old or no results available. Starting a connect scan");
            match self.trigger_scan(None, vec![]).await {
                Ok(ScanEnd::Complete) => info!("Connect scan completed"),
                Ok(ScanEnd::Cancelled) => bail!("Connect scan was cancelled"),
                Err(e) => bail!("Connect scan failed: {}", e),
            }
        }

        let last_scan_results = match self.last_scan_results.lock().clone() {
            Some(scan_results) => scan_results.results,
            None => bail!("No scan results available for connect attempt"),
        };
        info!("Checking for network in last scan: {} access points", last_scan_results.len());
        let chosen_scan = find_matching_network_in_scan(
            ssid,
            &bssid,
            &key_mgmt,
            last_scan_results,
            &self.bss_scorer,
        );

        let (bss_description, compatible, ssid_if_owe_transition) = match chosen_scan {
            Some((bss_description, compatible)) => {
                // If the public BSS of an OWE transition network was chosen and OWE is supported,
                // we need to find the OWE BSS to actually connect to.
                if bss_description.protection() == bss::Protection::OpenOweTransition
                    && compatible.mutual_security_protocols().contains(&SecurityDescriptor::Owe)
                {
                    if let Ok(bss) = self.find_owe_transition_bss(&bss_description).await {
                        (bss, compatible, Some(bss_description.ssid))
                    } else {
                        bail!("OWE BSS of selected OWE transition network not found.");
                    }
                } else {
                    (bss_description, compatible, None)
                }
            }
            None => bail!("Requested network not found"),
        };

        let authenticator = match get_authenticator(
            bss_description.bssid,
            compatible,
            &credential,
            key_mgmt,
        ) {
            Some(authenticator) => authenticator,
            None => bail!(
                "Failed to create authenticator for requested network. Unsupported security type, channel, or data rate."
            ),
        };

        info!("Selected BSS for connection");
        let (connect_txn, remote) = create_proxy();
        let bssid = bss_description.bssid;
        let connect_req = fidl_sme::ConnectRequest {
            ssid: bss_description.ssid.clone().into(),
            bss_description: bss_description.clone().into(),
            multiple_bss_candidates: false,
            authentication: authenticator.into(),
            deprecated_scan_type: fidl_common::ScanType::Passive,
        };
        self.sme_proxy.connect(&connect_req, Some(remote))?;

        info!("Waiting for connect result from SME");
        let mut stream = connect_txn.take_event_stream();
        let (sme_result, timed_out) = wait_for_connect_result(&mut stream)
            .map(|res| (res, false))
            .on_timeout(CONNECT_TIMEOUT, || {
                warn!("Timed out waiting for connect result");
                self.telemetry_sender.send(TelemetryEvent::SmeTimeout {
                    source: wlan_telemetry::TimeoutSource::Connect,
                });
                (
                    Ok(fidl_sme::ConnectResult {
                        code: fidl_ieee80211::StatusCode::RejectedSequenceTimeout,
                        is_credential_rejected: false,
                        is_reconnect: false,
                    }),
                    true,
                )
            })
            .await;
        let sme_result = sme_result?;

        info!("Received connect result from SME: {:?}", sme_result);
        if sme_result.code == fidl_ieee80211::StatusCode::Success {
            *self.connected_network.lock() = Some(ConnectedNetwork {
                bssid: bss_description.bssid,
                rssi: bss_description.rssi_dbm,
            });
            Ok(ConnectResult::Success(ConnectSuccess {
                bss: Box::new(bss_description),
                transaction_stream: stream,
                ssid_if_owe_transition,
            }))
        } else {
            let is_owe_transition = ssid_if_owe_transition.is_some();
            self.bss_scorer.report_connect_failure(bssid, &sme_result);
            Ok(ConnectResult::Fail(ConnectFail {
                bss: Box::new(bss_description),
                status_code: sme_result.code,
                is_credential_rejected: sme_result.is_credential_rejected,
                timed_out,
                is_owe_transition,
            }))
        }
    }

    async fn disconnect(&self) -> Result<(), Error> {
        // Note: we are forwarding disconnect request to SME, but we are not clearing
        //       any connected network state here because we expect this struct's `on_disconnect`
        //       to be called later.
        self.sme_proxy
            .disconnect(fidl_sme::UserDisconnectReason::Unknown)
            .map_err(|e| format_err!("Failed to request disconnect: {:?}", e))
            .on_timeout(DISCONNECT_TIMEOUT, || {
                self.telemetry_sender.send(TelemetryEvent::SmeTimeout {
                    source: wlan_telemetry::TimeoutSource::Disconnect,
                });
                Err(format_err!("Timed out waiting for disconnect"))
            })
            .await?;
        Ok(())
    }

    fn get_connected_network(&self) -> Option<ConnectedNetwork> {
        self.connected_network.lock().clone()
    }

    fn on_disconnect(&self, _info: &fidl_sme::DisconnectSource) {
        self.connected_network.lock().take();
    }

    fn on_signal_report(&self, ind: fidl_internal::SignalReportIndication) {
        if let Some(connected_network) = self.connected_network.lock().as_mut() {
            connected_network.rssi = ind.rssi_dbm;
        }
    }

    async fn set_bt_coexistence_mode(
        &self,
        mode: fidl_internal::BtCoexistenceMode,
    ) -> Result<(), fidl_wlanix::WlanixError> {
        self.monitor_svc
            .set_bt_coexistence_mode(self.phy_id, mode)
            .await
            .map_err(|e| {
                warn!("Encountered FIDL error when setting BT coexistence mode: {:?}", e);
                fidl_wlanix::WlanixError::InternalError
            })?
            .map_err(|e| {
                warn!("Failed to set BT coexistence mode: {:?}", e);
                fidl_wlanix::WlanixError::InternalError
            })
    }

    async fn set_power_save_mode(&self, enabled: bool) -> Result<(), Error> {
        // Update our cache
        let mut power_state = self.power_state.lock().await;
        power_state.power_save_enabled = enabled;
        // Figure out the new state
        let new_level = if power_state.suspend_mode_enabled {
            info!("Got SetPowerSave {} while SetSuspendModeEnabled is true", enabled);
            self.telemetry_sender.send(TelemetryEvent::UnclearPowerDemand(
                wlan_telemetry::UnclearPowerDemand::PowerSaveRequestedWhileSuspendModeEnabled,
            ));
            StaIfacePowerLevel::Suspended
        } else {
            match enabled {
                true => StaIfacePowerLevel::Normal,
                false => StaIfacePowerLevel::NoPowerSavings,
            }
        };
        drop(power_state);
        self.update_power_level(new_level).await
    }

    async fn set_suspend_mode(&self, enabled: bool) -> Result<(), Error> {
        let mut power_state = self.power_state.lock().await;
        power_state.suspend_mode_enabled = enabled;
        // Figure out the new state
        let new_level = if enabled {
            // Assume that this overrides any SetPowerSave
            StaIfacePowerLevel::Suspended
        } else {
            // Suspend mode is off
            if power_state.power_save_enabled {
                // This case is frequently seen in practice today, where the Policy layer above us
                // performs the following sequence: (1) iface creation, (2) suspend_mode = true,
                // (3) power_save = true, (4) suspend_mode = false. In this case, we should remain
                // in power save model.
                info!(
                    "SetSuspendModeEnabled=false while SetPowerSave={:?}, reverting to power save mode",
                    power_state.power_save_enabled
                );
                StaIfacePowerLevel::Normal
            } else {
                warn!(
                    "SetSuspendModeEnabled=false while SetPowerSave={:?}, moving to high performance",
                    power_state.power_save_enabled
                );
                StaIfacePowerLevel::NoPowerSavings
            }
        };
        drop(power_state);
        self.update_power_level(new_level).await
    }

    async fn set_country(&self, code: [u8; 2]) -> Result<(), Error> {
        let result = self
            .monitor_svc
            .set_country(&fidl_device_service::SetCountryRequest {
                phy_id: self.phy_id,
                alpha2: code,
            })
            .await
            .map_err(Into::<Error>::into)?;
        match zx::Status::ok(result) {
            Ok(()) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn set_mac_address(&self, mac_addr: [u8; 6]) -> Result<(), zx::Status> {
        info!("SmeClientIface.set_mac_address called with mac_addr: {}", MacAddr::from(mac_addr));
        self.sme_proxy
            .set_mac_address(&mac_addr)
            .await
            .map_err(|e| {
                error!("FIDL error calling set_mac_address: {:?}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    async fn install_apf_packet_filter(&self, program: Vec<u8>) -> Result<(), zx::Status> {
        let result = self
            .sme_proxy
            .install_apf_packet_filter(&program)
            .await
            .map_err(|e| {
                error!("FIDL error calling install_apf_packet_filter: {:?}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw);

        if result.is_ok() {
            let mut power_state = self.power_state.lock().await;
            power_state.apf_filter_installed = true;
        }
        result
    }

    async fn read_apf_packet_filter_data(&self) -> Result<Vec<u8>, zx::Status> {
        self.sme_proxy
            .read_apf_packet_filter_data()
            .await
            .map_err(|e| {
                error!("FIDL error calling read_apf_packet_filter_data: {:?}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    async fn start_sched_scan(
        &self,
        request: fidl_common::ScheduledScanRequest,
    ) -> Result<fidl::endpoints::ClientEnd<fidl_sme::ScheduledScanTransactionMarker>, Error> {
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fidl_sme::ScheduledScanTransactionMarker>();
        self.sme_proxy
            .start_scheduled_scan(&request, server_end)
            .await
            .map_err(|e| {
                error!("FIDL error calling start_sched_scan: {:?}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)?;
        Ok(client_end)
    }

    async fn get_signal_report(&self) -> Result<fidl_stats::SignalReport, Error> {
        self.telemetry_proxy
            .get_signal_report()
            .await?
            .map_err(|e| format_err!("Failed to get signal report: {:?}", e))
    }

    async fn get_iface_stats(&self) -> Result<fidl_stats::IfaceStats, Error> {
        self.telemetry_proxy
            .get_iface_stats()
            .await?
            .map_err(|e| format_err!("Failed to get iface stats: {:?}", e))
    }
}

fn check_scan_result_matches(
    bss: &BssDescription,
    compatible: &Compatible,
    ssid: &[u8],
    bssid: &Option<Bssid>,
    key_mgmt: &Option<fidl_wlanix::KeyMgmtMask>,
) -> bool {
    if bss.ssid != *ssid {
        return false;
    }
    if let Some(target_bssid) = bssid
        && bss.bssid != *target_bssid
    {
        return false;
    }
    if let Some(mask) = key_mgmt {
        let has_matching_protocol = compatible
            .mutual_security_protocols()
            .iter()
            .any(|proto| security_matches_key_mgmt(proto, *mask));
        if !has_matching_protocol {
            let label = if bssid.is_some() { "SSID and BSSID" } else { "SSID" };
            warn!(
                "Scan result has matching {} but its compatible security {:?} do not match the requested key management mask: {:?}",
                label,
                compatible.mutual_security_protocols(),
                mask
            );
            return false;
        }
    }
    true
}

fn find_matching_network_in_scan(
    ssid: &[u8],
    bssid: &Option<Bssid>,
    key_mgmt: &Option<fidl_wlanix::KeyMgmtMask>,
    scan_results: Vec<fidl_sme::ScanResult>,
    bss_scorer: &BssScorer,
) -> Option<(BssDescription, Compatible)> {
    let mut scan_results = scan_results
        .iter()
        .filter_map(|r| {
            let bss_description = BssDescription::try_from(r.bss_description.clone()).ok()?;
            let compatibility = Compatibility::try_from_fidl(r.compatibility.clone()).ok()?;
            match compatibility {
                Ok(compatible) => {
                    if check_scan_result_matches(
                        &bss_description,
                        &compatible,
                        ssid,
                        bssid,
                        key_mgmt,
                    ) {
                        Some((bss_description, compatible))
                    } else {
                        None
                    }
                }
                Err(incompatible) => {
                    error!("BSS ({:?}) is incompatible: {}", bss_description.bssid, incompatible,);
                    None
                }
            }
        })
        .collect::<Vec<_>>();
    scan_results.sort_by_key(|(bss_description, _)| bss_scorer.score_bss(bss_description));
    scan_results.pop()
}

/// Wait until stream returns an OnConnectResult event or None. Ignore other event types.
/// TODO(https://fxbug.dev/42084621): Function taken from wlancfg. Dedupe later.
async fn wait_for_connect_result(
    stream: &mut fidl_sme::ConnectTransactionEventStream,
) -> Result<fidl_sme::ConnectResult, Error> {
    loop {
        let stream_fut = stream.try_next();
        match stream_fut
            .await
            .map_err(|e| format_err!("Failed to receive connect result from sme: {:?}", e))?
        {
            Some(fidl_sme::ConnectTransactionEvent::OnConnectResult { result }) => {
                return Ok(result);
            }
            Some(other) => {
                info!(
                    "Expected ConnectTransactionEvent::OnConnectResult, got {}. Ignoring.",
                    connect_txn_event_name(&other)
                );
            }
            None => {
                return Err(format_err!(
                    "Server closed the ConnectTransaction channel before sending a response"
                ));
            }
        };
    }
}

fn connect_txn_event_name(event: &fidl_sme::ConnectTransactionEvent) -> &'static str {
    match event {
        fidl_sme::ConnectTransactionEvent::OnConnectResult { .. } => "OnConnectResult",
        fidl_sme::ConnectTransactionEvent::OnRoamResult { .. } => "OnRoamResult",
        fidl_sme::ConnectTransactionEvent::OnDisconnect { .. } => "OnDisconnect",
        fidl_sme::ConnectTransactionEvent::OnSignalReport { .. } => "OnSignalReport",
        fidl_sme::ConnectTransactionEvent::OnChannelSwitched { .. } => "OnChannelSwitched",
    }
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use fidl::endpoints::RequestStream;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use ieee80211::{MacAddrBytes, Ssid};
    use wlan_common::random_bss_description;

    pub static FAKE_IFACE_RESPONSE: fidl_device_service::QueryIfaceResponse =
        fidl_device_service::QueryIfaceResponse {
            role: fidl_fuchsia_wlan_common::WlanMacRole::Client,
            id: 1,
            phy_id: 10,
            phy_assigned_id: 100,
            sta_addr: [1, 2, 3, 4, 5, 6],
            factory_addr: [1, 2, 3, 4, 5, 6],
        };

    pub fn fake_scan_result() -> fidl_sme::ScanResult {
        fidl_sme::ScanResult {
            compatibility: fidl_sme::Compatibility::Incompatible(fidl_sme::Incompatible {
                description: String::from("unknown"),
                disjoint_security_protocols: None,
            }),
            timestamp_nanos: 1000,
            bss_description: fidl_ieee80211::BssDescription {
                bssid: [1, 2, 3, 4, 5, 6],
                bss_type: fidl_ieee80211::BssType::Infrastructure,
                beacon_period: 100,
                capability_info: 123,
                ies: vec![1, 2, 3, 2, 1],
                primary: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 1,
                },
                bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
                vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 0,
                },
                rssi_dbm: -40,
                snr_db: -50,
            },
        }
    }

    pub fn fake_connected_network() -> ConnectedNetwork {
        ConnectedNetwork { bssid: Bssid::from([1, 2, 3, 4, 5, 6]), rssi: -35 }
    }

    #[derive(Debug, Clone)]
    pub enum ClientIfaceCall {
        Query,
        TriggerScan,
        AbortScan,
        GetLastScanResults,
        #[expect(dead_code)]
        UpdateLastScanResults(Vec<fidl_sme::ScanResult>),
        ConnectToNetwork {
            ssid: Vec<u8>,
            credential: Credential,
            bssid: Option<Bssid>,
            key_mgmt: Option<fidl_wlanix::KeyMgmtMask>,
        },
        Disconnect,
        GetConnectedNetworkRssi,
        OnDisconnect {
            info: fidl_sme::DisconnectSource,
        },
        OnSignalReport {
            ind: fidl_internal::SignalReportIndication,
        },
        SetBtCoexistenceMode {
            mode: fidl_internal::BtCoexistenceMode,
        },
        SetPowerSaveMode(bool),
        SetSuspendMode(bool),
        SetCountry([u8; 2]),
        SetMacAddress([u8; 6]),
        InstallApfPacketFilter(Vec<u8>),
        ReadApfPacketFilterData,
        StartSchedScan {
            _request: fidl_common::ScheduledScanRequest,
        },
        GetSignalReport,
        GetIfaceStats,
    }

    pub struct TestClientIface {
        pub transaction_handle: Mutex<Option<fidl_sme::ConnectTransactionControlHandle>>,
        pub pno_transaction_handle: Mutex<Option<fidl_sme::ScheduledScanTransactionControlHandle>>,
        scan_end_receiver: Mutex<Option<oneshot::Receiver<Result<ScanEnd, Error>>>>,
        pub calls: Arc<Mutex<Vec<ClientIfaceCall>>>,
        pub connect_success: Mutex<bool>,
        pub scan_results: Mutex<Vec<fidl_sme::ScanResult>>,
        pub signal_report: Mutex<Option<fidl_stats::SignalReport>>,
        pub iface_stats: Mutex<Option<fidl_stats::IfaceStats>>,
        pub fail_start_sched_scan: Mutex<bool>,
        pub start_sched_scan_yielding_rx: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl TestClientIface {
        pub fn new() -> Self {
            Self {
                transaction_handle: Mutex::new(None),
                pno_transaction_handle: Mutex::new(None),
                scan_end_receiver: Mutex::new(None),
                calls: Arc::new(Mutex::new(vec![])),
                connect_success: Mutex::new(true),
                scan_results: Mutex::new(vec![fake_scan_result()]),
                signal_report: Mutex::new(None),
                iface_stats: Mutex::new(None),
                fail_start_sched_scan: Mutex::new(false),
                start_sched_scan_yielding_rx: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl ClientIface for TestClientIface {
        async fn query(&self) -> Result<fidl_device_service::QueryIfaceResponse, Error> {
            self.calls.lock().push(ClientIfaceCall::Query);
            Ok(fidl_device_service::QueryIfaceResponse {
                role: fidl_common::WlanMacRole::Client,
                id: 1,
                phy_id: 42,
                phy_assigned_id: 1337,
                sta_addr: [13, 37, 13, 37, 13, 37],
                factory_addr: [13, 37, 13, 37, 13, 37],
            })
        }

        fn update_last_scan_results(&self, results: Vec<fidl_sme::ScanResult>) {
            self.calls.lock().push(ClientIfaceCall::UpdateLastScanResults(results));
        }

        async fn trigger_scan(
            &self,
            _ssid: Option<&Ssid>,
            _channels: Vec<u8>,
        ) -> Result<ScanEnd, Error> {
            self.calls.lock().push(ClientIfaceCall::TriggerScan);
            let scan_end_receiver = self.scan_end_receiver.lock().take();
            match scan_end_receiver {
                Some(receiver) => receiver.await.expect("scan_end_signal failed"),
                None => Ok(ScanEnd::Complete),
            }
        }
        async fn abort_scan(&self) -> Result<(), Error> {
            self.calls.lock().push(ClientIfaceCall::AbortScan);
            Ok(())
        }
        fn get_last_scan_results(&self) -> Vec<fidl_sme::ScanResult> {
            self.calls.lock().push(ClientIfaceCall::GetLastScanResults);
            self.scan_results.lock().clone()
        }
        async fn connect_to_network(
            &self,
            ssid: &[u8],
            credential: Credential,
            bssid: Option<Bssid>,
            key_mgmt: Option<fidl_wlanix::KeyMgmtMask>,
        ) -> Result<ConnectResult, Error> {
            self.calls.lock().push(ClientIfaceCall::ConnectToNetwork {
                ssid: ssid.to_vec(),
                credential: credential.clone(),
                bssid,
                key_mgmt,
            });
            if *self.connect_success.lock() {
                let (proxy, server) =
                    fidl::endpoints::create_proxy::<fidl_sme::ConnectTransactionMarker>();
                let (_, handle) = server.into_stream_and_control_handle();
                *self.transaction_handle.lock() = Some(handle);
                Ok(ConnectResult::Success(ConnectSuccess {
                    bss: Box::new(random_bss_description!(
                        ssid: Ssid::try_from(ssid).unwrap(),
                        bssid: bssid.map(|b| b.to_array()).unwrap_or([42, 42, 42, 42, 42, 42]),
                    )),
                    transaction_stream: proxy.take_event_stream(),
                    ssid_if_owe_transition: None,
                }))
            } else {
                Ok(ConnectResult::Fail(ConnectFail {
                    bss: Box::new(random_bss_description!(
                        ssid: Ssid::try_from(ssid).unwrap(),
                        bssid: bssid.map(|b| b.to_array()).unwrap_or([42, 42, 42, 42, 42, 42]),
                    )),
                    status_code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                    is_credential_rejected: false,
                    timed_out: false,
                    is_owe_transition: false,
                }))
            }
        }

        async fn disconnect(&self) -> Result<(), Error> {
            self.calls.lock().push(ClientIfaceCall::Disconnect);
            Ok(())
        }

        fn get_connected_network(&self) -> Option<ConnectedNetwork> {
            self.calls.lock().push(ClientIfaceCall::GetConnectedNetworkRssi);
            None
        }

        fn on_disconnect(&self, info: &fidl_sme::DisconnectSource) {
            self.calls.lock().push(ClientIfaceCall::OnDisconnect { info: *info });
        }

        fn on_signal_report(&self, ind: fidl_internal::SignalReportIndication) {
            self.calls.lock().push(ClientIfaceCall::OnSignalReport { ind });
        }

        async fn set_bt_coexistence_mode(
            &self,
            mode: fidl_internal::BtCoexistenceMode,
        ) -> Result<(), fidl_wlanix::WlanixError> {
            self.calls.lock().push(ClientIfaceCall::SetBtCoexistenceMode { mode });
            Ok(())
        }

        async fn set_power_save_mode(&self, enabled: bool) -> Result<(), Error> {
            self.calls.lock().push(ClientIfaceCall::SetPowerSaveMode(enabled));
            Ok(())
        }

        async fn set_suspend_mode(&self, enabled: bool) -> Result<(), Error> {
            self.calls.lock().push(ClientIfaceCall::SetSuspendMode(enabled));
            Ok(())
        }

        async fn set_country(&self, code: [u8; 2]) -> Result<(), Error> {
            self.calls.lock().push(ClientIfaceCall::SetCountry(code));
            Ok(())
        }

        async fn set_mac_address(&self, mac_addr: [u8; 6]) -> Result<(), zx::Status> {
            self.calls.lock().push(ClientIfaceCall::SetMacAddress(mac_addr));
            Ok(())
        }

        async fn install_apf_packet_filter(&self, program: Vec<u8>) -> Result<(), zx::Status> {
            self.calls.lock().push(ClientIfaceCall::InstallApfPacketFilter(program));
            Ok(())
        }

        async fn read_apf_packet_filter_data(&self) -> Result<Vec<u8>, zx::Status> {
            self.calls.lock().push(ClientIfaceCall::ReadApfPacketFilterData);
            Ok(vec![2, 2, 2, 2])
        }

        async fn start_sched_scan(
            &self,
            request: fidl_common::ScheduledScanRequest,
        ) -> Result<fidl::endpoints::ClientEnd<fidl_sme::ScheduledScanTransactionMarker>, Error>
        {
            self.calls.lock().push(ClientIfaceCall::StartSchedScan { _request: request });
            if *self.fail_start_sched_scan.lock() {
                return Err(zx::Status::NOT_SUPPORTED.into());
            }

            // If a start_sched_scan_yielding_rx is configured (used in concurrency tests),
            // await a signal over the channel before completing the start request.
            // This allows tests to pause the asynchronous start execution mid-flight
            // to simulate race conditions (e.g., StopScan arriving while StartScan is pending).
            let mut rx_opt = {
                let mut yield_receiver = self.start_sched_scan_yielding_rx.lock();
                yield_receiver.take()
            }; // Lock guard is dropped immediately here to keep the future Send

            if let Some(mut rx) = rx_opt.take() {
                let _ = rx.next().await;
                *self.start_sched_scan_yielding_rx.lock() = Some(rx);
            }

            let (client_end, server_end) =
                fidl::endpoints::create_endpoints::<fidl_sme::ScheduledScanTransactionMarker>();
            *self.pno_transaction_handle.lock() = Some(server_end.into_stream().control_handle());
            Ok(client_end)
        }

        async fn get_signal_report(&self) -> Result<fidl_stats::SignalReport, Error> {
            self.calls.lock().push(ClientIfaceCall::GetSignalReport);
            if let Some(report) = self.signal_report.lock().clone() {
                Ok(report)
            } else {
                Err(format_err!("get signal report not mocked"))
            }
        }

        async fn get_iface_stats(&self) -> Result<fidl_stats::IfaceStats, Error> {
            self.calls.lock().push(ClientIfaceCall::GetIfaceStats);
            if let Some(stats) = self.iface_stats.lock().clone() {
                Ok(stats)
            } else {
                Err(format_err!("get iface stats not mocked"))
            }
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum IfaceManagerCall {
        ListPhys,
        ListIfaces,
        GetCountry,
        SetCountry { phy_id: u16, country: [u8; 2] },
        QueryIface(u16),
        QueryIfaceCapabilities(u16),
        CreateClientIface(u16),
        GetClientIface(u16),
        DestroyIface(u16),
        PowerDown(u16),
        PowerUp(u16),
        GetPowerState(u16),
        GetPowerElementDependencyToken(u16),
        ResetTxPowerScenario(u16),
        SetTxPowerScenario { phy_id: u16, scenario: fidl_internal::TxPowerScenario },
        ResetPhy(u16),
    }

    pub struct TestIfaceManager {
        pub client_iface: Mutex<Option<Arc<TestClientIface>>>,
        pub calls: Arc<Mutex<Vec<IfaceManagerCall>>>,
        country: Arc<Mutex<[u8; 2]>>,
        pub power_state: Arc<Mutex<bool>>,
        mock_create_client_iface_result: Result<u16, Error>,
        mock_destroy_client_iface_result: Result<(), Error>,
        mock_power_up_result: Result<(), Error>,
        mock_power_down_result: Result<(), Error>,
        mock_reset_tx_power_scenario_result: Result<(), Error>,
        mock_set_tx_power_scenario_result: Result<(), Error>,
        mock_reset_phy_result: Result<(), Error>,
        mock_list_phys_result: Result<Vec<u16>, Error>,
        mock_get_power_element_dependency_token_result: Result<(), Error>,
        iface_id: Arc<Mutex<u16>>,
    }

    impl TestIfaceManager {
        pub fn new() -> Self {
            Self {
                client_iface: Mutex::new(None),
                calls: Arc::new(Mutex::new(vec![])),
                country: Arc::new(Mutex::new(*b"XX")),
                power_state: Arc::new(Mutex::new(true)),
                mock_create_client_iface_result: Ok(FAKE_IFACE_RESPONSE.id),
                mock_destroy_client_iface_result: Ok(()),
                mock_power_up_result: Ok(()),
                mock_power_down_result: Ok(()),
                mock_reset_tx_power_scenario_result: Ok(()),
                mock_set_tx_power_scenario_result: Ok(()),
                mock_reset_phy_result: Ok(()),
                mock_list_phys_result: Ok(vec![1]),
                mock_get_power_element_dependency_token_result: Ok(()),
                iface_id: Arc::new(Mutex::new(FAKE_IFACE_RESPONSE.id)),
            }
        }

        pub fn new_with_client() -> Self {
            Self { client_iface: Mutex::new(Some(Arc::new(TestClientIface::new()))), ..Self::new() }
        }

        pub fn new_with_client_and_scan_end_sender()
        -> (Self, oneshot::Sender<Result<ScanEnd, Error>>) {
            let (sender, receiver) = oneshot::channel();
            (
                Self {
                    client_iface: Mutex::new(Some(Arc::new(TestClientIface {
                        scan_end_receiver: Mutex::new(Some(receiver)),
                        ..TestClientIface::new()
                    }))),
                    ..Self::new()
                },
                sender,
            )
        }

        pub fn get_client_iface(&self) -> Arc<TestClientIface> {
            Arc::clone(self.client_iface.lock().as_ref().expect("No client iface found"))
        }

        pub fn get_iface_call_history(&self) -> Arc<Mutex<Vec<ClientIfaceCall>>> {
            let iface = self.client_iface.lock();
            let iface_ref = iface.as_ref().expect("client iface should exist");
            Arc::clone(&iface_ref.calls)
        }

        pub fn mock_create_client_iface_failure(self) -> Self {
            Self {
                mock_create_client_iface_result: Err(format_err!(
                    "mocked CreateClientIface failure"
                )),
                ..self
            }
        }

        pub fn mock_destroy_client_iface_failure(self) -> Self {
            Self {
                mock_destroy_client_iface_result: Err(format_err!(
                    "mocked DestroyClientIface failure"
                )),
                ..self
            }
        }

        pub fn mock_power_up_failure(self) -> Self {
            Self { mock_power_up_result: Err(format_err!("mocked PowerUp failure")), ..self }
        }

        pub fn mock_power_down_failure(self) -> Self {
            Self { mock_power_down_result: Err(format_err!("mocked PowerDown failure")), ..self }
        }

        pub fn mock_reset_tx_power_scenario_failure(self) -> Self {
            Self {
                mock_reset_tx_power_scenario_result: Err(format_err!(
                    "mocked ResetTxPowerScenario failure"
                )),
                ..self
            }
        }

        pub fn mock_set_tx_power_scenario_failure(self) -> Self {
            Self {
                mock_set_tx_power_scenario_result: Err(format_err!(
                    "mocked SetTxPowerScenario failure"
                )),
                ..self
            }
        }

        pub fn set_iface_id(&self, new_id: u16) {
            *self.iface_id.lock() = new_id;
        }

        pub fn mock_list_phys_failure(self) -> Self {
            Self { mock_list_phys_result: Err(format_err!("mocked ListPhys failure")), ..self }
        }

        pub fn mock_no_phys_available(self) -> Self {
            Self { mock_list_phys_result: Ok(vec![]), ..self }
        }

        pub fn mock_reset_phy_failure(self) -> Self {
            Self { mock_reset_phy_result: Err(format_err!("mocked ResetPhy failure")), ..self }
        }

        pub fn mock_get_power_element_dependency_token_failure(self) -> Self {
            Self {
                mock_get_power_element_dependency_token_result: Err(format_err!(
                    "mocked GetPowerElementDependencyToken failure"
                )),
                ..self
            }
        }
    }

    #[async_trait]
    impl IfaceManager for TestIfaceManager {
        type Client = TestClientIface;

        async fn list_phys(&self) -> Result<Vec<u16>, Error> {
            self.calls.lock().push(IfaceManagerCall::ListPhys);
            match &self.mock_list_phys_result {
                Ok(phys) => Ok(phys.clone()),
                Err(e) => bail!("{e}"),
            }
        }

        fn list_ifaces(&self) -> Vec<u16> {
            self.calls.lock().push(IfaceManagerCall::ListIfaces);
            if self.client_iface.lock().is_some() { vec![*self.iface_id.lock()] } else { vec![] }
        }

        async fn get_country(&self, _phy_id: u16) -> Result<[u8; 2], Error> {
            self.calls.lock().push(IfaceManagerCall::GetCountry);
            Ok(*self.country.lock())
        }

        async fn set_country(&self, phy_id: u16, country: [u8; 2]) -> Result<(), Error> {
            self.calls.lock().push(IfaceManagerCall::SetCountry { phy_id, country });
            *self.country.lock() = country;
            Ok(())
        }

        async fn query_iface(
            &self,
            iface_id: u16,
        ) -> Result<fidl_device_service::QueryIfaceResponse, Error> {
            self.calls.lock().push(IfaceManagerCall::QueryIface(iface_id));
            if self.client_iface.lock().is_some() && iface_id == *self.iface_id.lock() {
                Ok(FAKE_IFACE_RESPONSE)
            } else {
                Err(format_err!("Unexpected query for iface id {}", iface_id))
            }
        }

        async fn query_iface_capabilities(
            &self,
            iface_id: u16,
        ) -> Result<fidl_common::ApfPacketFilterSupport, Error> {
            self.calls.lock().push(IfaceManagerCall::QueryIfaceCapabilities(iface_id));
            if self.client_iface.lock().is_some() && iface_id == *self.iface_id.lock() {
                Ok(fidl_common::ApfPacketFilterSupport {
                    supported: Some(true),
                    version: Some(1),
                    max_filter_length: Some(1),
                    ..fidl_common::ApfPacketFilterSupport::default()
                })
            } else {
                Err(format_err!("Unexpected query for iface id {}", iface_id))
            }
        }

        async fn create_client_iface(&self, phy_id: u16) -> Result<u16, Error> {
            self.calls.lock().push(IfaceManagerCall::CreateClientIface(phy_id));
            let iface_id = match &self.mock_create_client_iface_result {
                Ok(iface_id) => *iface_id,
                Err(e) => bail!("{e}"),
            };
            assert!(self.client_iface.lock().is_none());
            let _ = self.client_iface.lock().replace(Arc::new(TestClientIface {
                scan_end_receiver: Mutex::new(None),
                ..TestClientIface::new()
            }));
            Ok(iface_id)
        }

        async fn get_client_iface(&self, iface_id: u16) -> Result<Arc<TestClientIface>, Error> {
            self.calls.lock().push(IfaceManagerCall::GetClientIface(iface_id));
            if iface_id == *self.iface_id.lock() {
                match self.client_iface.lock().as_ref() {
                    Some(iface) => Ok(Arc::clone(iface)),
                    None => Err(format_err!("Unexpected get_client_iface when no client exists")),
                }
            } else {
                Err(format_err!("Unexpected get_client_iface for missing iface id {}", iface_id))
            }
        }

        async fn destroy_iface(&self, iface_id: u16) -> Result<(), Error> {
            self.calls.lock().push(IfaceManagerCall::DestroyIface(iface_id));
            match &self.mock_destroy_client_iface_result {
                Ok(()) => *self.client_iface.lock() = None,
                Err(e) => bail!("{e}"),
            }
            Ok(())
        }

        async fn power_down(&self, phy_id: u16) -> Result<(), Error> {
            self.calls.lock().push(IfaceManagerCall::PowerDown(phy_id));
            match &self.mock_power_down_result {
                Ok(()) => {
                    *self.power_state.lock() = false;
                    Ok(())
                }
                Err(e) => bail!("{e}"),
            }
        }

        async fn power_up(&self, phy_id: u16) -> Result<(), Error> {
            self.calls.lock().push(IfaceManagerCall::PowerUp(phy_id));
            match &self.mock_power_up_result {
                Ok(()) => {
                    *self.power_state.lock() = true;
                    Ok(())
                }
                Err(e) => bail!("{e}"),
            }
        }

        async fn get_power_state(&self, phy_id: u16) -> Result<bool, Error> {
            self.calls.lock().push(IfaceManagerCall::GetPowerState(phy_id));
            Ok(*self.power_state.lock())
        }

        async fn get_power_element_dependency_token(
            &self,
            phy_id: u16,
        ) -> Result<fidl_fuchsia_power_broker::DependencyToken, Error> {
            self.calls.lock().push(IfaceManagerCall::GetPowerElementDependencyToken(phy_id));
            match &self.mock_get_power_element_dependency_token_result {
                Ok(()) => Ok(zx::Event::create()),
                Err(e) => bail!("{}", e),
            }
        }

        async fn reset_tx_power_scenario(&self, phy_id: u16) -> Result<(), Error> {
            self.calls.lock().push(IfaceManagerCall::ResetTxPowerScenario(phy_id));
            match &self.mock_reset_tx_power_scenario_result {
                Ok(()) => Ok(()),
                Err(e) => bail!("{}", e),
            }
        }

        async fn set_tx_power_scenario(
            &self,
            phy_id: u16,
            scenario: fidl_internal::TxPowerScenario,
        ) -> Result<(), Error> {
            self.calls.lock().push(IfaceManagerCall::SetTxPowerScenario { phy_id, scenario });
            match &self.mock_set_tx_power_scenario_result {
                Ok(()) => Ok(()),
                Err(e) => bail!("{}", e),
            }
        }

        async fn reset_phy(&self, phy_id: u16) -> Result<(), Error> {
            self.calls.lock().push(IfaceManagerCall::ResetPhy(phy_id));
            match &self.mock_reset_phy_result {
                Ok(()) => Ok(()),
                Err(e) => bail!("{}", e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use fidl_ieee80211::WlanBand::TwoGhz;
    use std::pin::pin;

    use super::test_utils::FAKE_IFACE_RESPONSE;
    use super::*;
    use crate::security::wep::WepKeys;
    use fidl::endpoints::create_proxy_and_stream;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use futures::task::Poll;
    use ieee80211::{MacAddrBytes, Ssid};
    use test_case::test_case;
    use wlan_common::channel::{Bandwidth, Channel};
    use wlan_common::test_utils::ExpectWithin;
    use wlan_common::test_utils::fake_stas::FakeProtectionCfg;
    use wlan_common::{fake_fidl_bss_description, ie};
    #[allow(
        clippy::single_component_path_imports,
        reason = "mass allow for https://fxbug.dev/381896734"
    )]
    use {
        fidl_fuchsia_wlan_internal as fidl_security, fidl_fuchsia_wlan_internal as fidl_internal,
        fuchsia_async as fasync, rand,
    };

    pub struct TestValuesNoIface {
        pub monitor_stream: fidl_device_service::DeviceMonitorRequestStream,
        pub telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        pub manager: DeviceMonitorIfaceManager,
        // The executor is last in the struct so it gets dropped last.
        pub exec: fasync::TestExecutor,
    }

    /// For tests that should start without any ifaces
    fn setup_test_manager() -> TestValuesNoIface {
        let exec = fasync::TestExecutor::new();
        let (monitor_svc, monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        TestValuesNoIface {
            exec,
            monitor_stream,
            telemetry_receiver,
            manager: DeviceMonitorIfaceManager {
                monitor_svc,
                ifaces: Mutex::new(HashMap::new()),
                telemetry_sender: TelemetrySender::new(telemetry_sender),
            },
        }
    }

    pub struct TestValuesWithIface {
        pub monitor_stream: fidl_device_service::DeviceMonitorRequestStream,
        pub sme_stream: fidl_sme::ClientSmeRequestStream,
        /// This is the stream  of telemetry events sent out by the IfaceManager to be logged
        /// in the telemetry module.
        pub telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        /// This is the stream to serve SME telemetry requests, such as get_iface_stats.
        pub telemetry_stream: fidl_sme::TelemetryRequestStream,
        pub manager: DeviceMonitorIfaceManager,
        pub iface: Arc<SmeClientIface>,
        // The executor is last in the struct so it gets dropped last.
        pub exec: fasync::TestExecutor,
    }

    const TEST_IFACE_ID: u16 = 123;
    /// For tests that should start with an iface. The iface can be accessed through the returned
    /// test values struct and has ID TEST_FACE_ID.
    fn setup_test_manager_with_iface() -> TestValuesWithIface {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let manager = DeviceMonitorIfaceManager {
            monitor_svc: monitor_svc.clone(),
            ifaces: Mutex::new(HashMap::new()),
            telemetry_sender: TelemetrySender::new(telemetry_sender.clone()),
        };
        let (sme_proxy, sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let (telemetry_proxy, telemetry_stream) =
            create_proxy_and_stream::<fidl_sme::TelemetryMarker>();
        let phy_id = rand::random();
        let iface = SmeClientIface::new(
            phy_id,
            TEST_IFACE_ID,
            sme_proxy,
            telemetry_proxy,
            monitor_svc,
            TelemetrySender::new(telemetry_sender),
        );
        manager.ifaces.lock().insert(TEST_IFACE_ID, Arc::new(iface));
        let mut client_fut = manager.get_client_iface(TEST_IFACE_ID);
        let iface = exec.run_singlethreaded(&mut client_fut).expect("Failed to get client iface");
        drop(client_fut);
        TestValuesWithIface {
            monitor_stream,
            sme_stream,
            telemetry_stream,
            telemetry_receiver,
            manager,
            iface,
            exec,
        }
    }

    fn setup_test_manager_with_iface_and_fake_time() -> TestValuesWithIface {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let (monitor_svc, monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (sme_proxy, sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let (telemetry_proxy, telemetry_stream) =
            create_proxy_and_stream::<fidl_sme::TelemetryMarker>();
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let manager = DeviceMonitorIfaceManager {
            monitor_svc: monitor_svc.clone(),
            ifaces: Mutex::new(HashMap::new()),
            telemetry_sender: TelemetrySender::new(telemetry_sender.clone()),
        };
        let iface = SmeClientIface {
            iface_id: 13,
            phy_id: 42,
            sme_proxy,
            telemetry_proxy,
            monitor_svc,
            last_scan_results: Arc::new(Mutex::new(None)),
            scan_abort_signal: Arc::new(Mutex::new(None)),
            connected_network: Arc::new(Mutex::new(None)),
            wlanix_provisioned: true,
            bss_scorer: BssScorer::new(),
            power_state: Arc::new(MutexAsync::new(PowerState {
                suspend_mode_enabled: false,
                power_save_enabled: false,
                apf_filter_installed: false,
                recorder: Some(
                    power_observability_state_recorder::EnumStateRecorder::new(
                        "test_state".into(),
                        c"test",
                        power_observability_state_recorder::RecorderOptions {
                            capacity: 1,
                            lazy_record: true,
                            manager: None,
                            persistence: None,
                        },
                    )
                    .expect("StateRecorder construction failed"),
                ),
            })),
            telemetry_sender: TelemetrySender::new(telemetry_sender),
        };

        manager.ifaces.lock().insert(1, Arc::new(iface));
        let mut client_fut = manager.get_client_iface(1);
        let iface = assert_matches!(exec.run_until_stalled(&mut client_fut), Poll::Ready(Ok(iface)) => iface);
        drop(client_fut);
        TestValuesWithIface {
            monitor_stream,
            sme_stream,
            telemetry_stream,
            telemetry_receiver,
            manager,
            iface,
            exec,
        }
    }

    #[test]
    fn test_query_interface() {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let manager = DeviceMonitorIfaceManager {
            monitor_svc,
            ifaces: Mutex::new(HashMap::new()),
            telemetry_sender: TelemetrySender::new(telemetry_sender),
        };
        let mut fut = manager.query_iface(FAKE_IFACE_RESPONSE.id);

        // We should query device monitor for info on the iface.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let (iface_id, responder) = assert_matches!(
                 exec.run_until_stalled(&mut monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::QueryIface { iface_id, responder })) => (iface_id, responder));
        assert_eq!(iface_id, FAKE_IFACE_RESPONSE.id);
        responder.send(Ok(&FAKE_IFACE_RESPONSE)).expect("Failed to respond to QueryIfaceResponse");

        let result =
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(info)) => info);
        assert_eq!(result, FAKE_IFACE_RESPONSE);
    }

    #[test]
    fn test_query_iface_capabilities() {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let manager = DeviceMonitorIfaceManager {
            monitor_svc,
            ifaces: Mutex::new(HashMap::new()),
            telemetry_sender: TelemetrySender::new(telemetry_sender),
        };
        let iface_id = 42;
        let mut fut = manager.query_iface_capabilities(iface_id);

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let (req_iface_id, responder) = assert_matches!(
                 exec.run_until_stalled(&mut monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::QueryIfaceCapabilities { iface_id, responder })) => (iface_id, responder));
        assert_eq!(req_iface_id, iface_id);

        let apf_support = fidl_common::ApfPacketFilterSupport {
            supported: Some(true),
            version: Some(1),
            max_filter_length: Some(1024),
            ..Default::default()
        };
        responder.send(Ok(&apf_support)).expect("Failed to respond to QueryIfaceCapabilities");

        let result =
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(info)) => info);
        assert_eq!(result, apf_support);
    }

    #[test]
    fn test_get_country() {
        let mut test_values = setup_test_manager();
        let mut fut = test_values.manager.get_country(123);

        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let (phy_id, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::GetCountry { phy_id, responder })) => (phy_id, responder));
        assert_eq!(phy_id, 123);
        responder
            .send(Ok(&fidl_device_service::GetCountryResponse { alpha2: *b"AB" }))
            .expect("Failed to respond to GetCountry");

        let country = assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Ok(info)) => info);
        assert_eq!(country, [b'A', b'B']);
    }

    #[test]
    fn test_create_and_serve_client_iface() {
        let mut test_values = setup_test_manager();
        let mut fut = test_values.manager.create_client_iface(0);

        // No interfaces to begin.
        assert!(test_values.manager.list_ifaces().is_empty());

        // Indicate that there are no existing ifaces.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::ListIfaces { responder })) => responder);
        responder.send(&[]).expect("Failed to respond to ListIfaces");

        // Create a new iface.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::CreateIface { responder, .. })) => responder);
        responder
            .send(Ok(&fidl_device_service::DeviceMonitorCreateIfaceResponse {
                iface_id: Some(FAKE_IFACE_RESPONSE.id),
                ..Default::default()
            }))
            .expect("Failed to send CreateIface response");

        // Establish a connection to the new iface.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::GetClientSme { responder, .. })) => responder);
        responder.send(Ok(())).expect("Failed to send GetClientSme response");

        // Establish a connection to the telemetry proxy.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::GetSmeTelemetry { responder, .. })) => responder);
        responder.send(Ok(())).expect("Failed to send GetSmeTelemetry response");

        // Creation complete!
        let request_id =
            test_values.exec.run_singlethreaded(&mut fut).expect("Creation completes ok");
        assert_eq!(request_id, FAKE_IFACE_RESPONSE.id);

        // The new iface shows up in ListInterfaces.
        assert_eq!(test_values.manager.list_ifaces(), vec![FAKE_IFACE_RESPONSE.id]);

        // The new iface is ready for use.
        let _iface = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.manager.get_client_iface(FAKE_IFACE_RESPONSE.id)),
            Poll::Ready(Ok(i)) => i
        );
    }

    #[test]
    fn test_create_iface_fails() {
        let mut test_values = setup_test_manager();
        let mut fut = test_values.manager.create_client_iface(0);

        // Indicate that there are no existing ifaces.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::ListIfaces { responder })) => responder);
        responder.send(&[]).expect("Failed to respond to ListIfaces");

        // Return an error for CreateIface.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::CreateIface { responder, .. })) => responder);
        responder
            .send(Err(fidl_device_service::DeviceMonitorError::unknown()))
            .expect("Failed to send CreateIface response");

        assert_matches!(
            test_values.exec.run_until_stalled(
                &mut test_values.manager.get_client_iface(FAKE_IFACE_RESPONSE.id)
            ),
            Poll::Ready(Err(_))
        );
    }

    // TODO(b/298030838): Delete test when wlanix is the sole config path.
    #[test]
    fn test_create_iface_with_unmanaged() {
        let mut test_values = setup_test_manager();
        let mut fut = test_values.manager.create_client_iface(0);

        // No interfaces to begin.
        assert!(test_values.manager.list_ifaces().is_empty());

        // Indicate that there is a fake iface.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::ListIfaces { responder })) => responder);
        responder.send(&[FAKE_IFACE_RESPONSE.id]).expect("Failed to respond to ListIfaces");

        // Respond with iface info.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let (iface_id, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::QueryIface { iface_id, responder })) => (iface_id, responder));
        assert_eq!(iface_id, FAKE_IFACE_RESPONSE.id);
        responder.send(Ok(&FAKE_IFACE_RESPONSE)).expect("Failed to respond to QueryIface");

        // Respond to GetClientSme.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::GetClientSme { responder, .. })) => responder);
        responder.send(Ok(())).expect("Failed to send GetClientSme response");

        // Respond to GetSmeTelemetry.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::GetSmeTelemetry { responder, .. })) => responder);
        responder.send(Ok(())).expect("Failed to send GetSmeTelemetry response");

        // We finish up and have a new iface. This may take longer than one try, since resolving
        // the power broker FIDL can take a few loops.
        let mut fut_with_timeout =
            pin!(fut.expect_within(zx::MonotonicDuration::from_seconds(5), "Awaiting iface"));
        let id = assert_matches!(test_values.exec.run_singlethreaded(&mut fut_with_timeout), Ok(id) => id);
        assert_eq!(id, FAKE_IFACE_RESPONSE.id);
        assert_eq!(&test_values.manager.list_ifaces()[..], [id]);
    }

    #[test]
    fn test_destroy_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let mut fut = test_values.manager.destroy_iface(TEST_IFACE_ID);

        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::DestroyIface { responder, .. })) => responder);
        responder.send(zx::sys::ZX_OK).expect("Failed to send DestroyIface response");
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        assert!(test_values.manager.ifaces.lock().is_empty());
    }

    // TODO(b/298030838): Delete test when wlanix is the sole config path.
    #[test]
    fn test_destroy_iface_not_wlanix() {
        // Create the manager here instead of using setup_test_manager(), since we need the
        // sme_proxy and monitor_svc to create the interface.
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (sme_proxy, _sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let manager = DeviceMonitorIfaceManager {
            monitor_svc: monitor_svc.clone(),
            ifaces: Mutex::new(HashMap::new()),
            telemetry_sender: TelemetrySender::new(telemetry_sender.clone()),
        };
        let iface = SmeClientIface {
            iface_id: 13,
            phy_id: 42,
            sme_proxy,
            telemetry_proxy: fidl::endpoints::create_proxy::<fidl_sme::TelemetryMarker>().0,
            monitor_svc,
            last_scan_results: Arc::new(Mutex::new(None)),
            scan_abort_signal: Arc::new(Mutex::new(None)),
            connected_network: Arc::new(Mutex::new(None)),
            wlanix_provisioned: false, // set to false for this test
            bss_scorer: BssScorer::new(),
            power_state: Arc::new(MutexAsync::new(PowerState {
                suspend_mode_enabled: false,
                power_save_enabled: false,
                apf_filter_installed: false,
                recorder: Some(
                    power_observability_state_recorder::EnumStateRecorder::new(
                        "test_state".into(),
                        c"test",
                        power_observability_state_recorder::RecorderOptions {
                            capacity: 1,
                            lazy_record: true,
                            manager: None,
                            persistence: None,
                        },
                    )
                    .expect("StateRecorder construction failed"),
                ),
            })),
            telemetry_sender: TelemetrySender::new(telemetry_sender),
        };
        let iface_id = 17;
        manager.ifaces.lock().insert(iface_id, Arc::new(iface));

        let mut fut = manager.destroy_iface(iface_id);

        // No destroy request is sent.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        assert_matches!(
            exec.run_until_stalled(&mut monitor_stream.select_next_some()),
            Poll::Pending
        );

        assert!(manager.ifaces.lock().is_empty());
    }

    #[test]
    fn test_get_client_iface_fails_no_such_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let mut fut = test_values.manager.get_client_iface(TEST_IFACE_ID + 1);

        // No ifaces exist, so this should always error.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Err(_e)));
    }

    #[test]
    fn test_destroy_iface_no_such_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let mut fut = test_values.manager.destroy_iface(TEST_IFACE_ID + 1);

        // No ifaces exist, so this should always return immediately.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[test]
    fn test_set_country() {
        let mut test_values = setup_test_manager_with_iface();
        let mut set_country_fut = test_values.manager.set_country(123, *b"WW");
        assert_matches!(test_values.exec.run_until_stalled(&mut set_country_fut), Poll::Pending);
        let (req, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetCountry { req, responder }))) => (req, responder));
        assert_eq!(req, fidl_device_service::SetCountryRequest { phy_id: 123, alpha2: *b"WW" });
        responder.send(zx::sys::ZX_OK).expect("Failed to send result");
        assert_matches!(
            test_values.exec.run_until_stalled(&mut set_country_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[test]
    fn test_set_country_on_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let mut set_country_fut = test_values.iface.set_country(*b"WW");
        assert_matches!(test_values.exec.run_until_stalled(&mut set_country_fut), Poll::Pending);
        let (req, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetCountry { req, responder }))) => (req, responder));
        assert_eq!(
            req,
            fidl_device_service::SetCountryRequest {
                phy_id: test_values.iface.phy_id,
                alpha2: *b"WW"
            }
        );
        responder.send(zx::sys::ZX_OK).expect("Failed to send result");
        assert_matches!(
            test_values.exec.run_until_stalled(&mut set_country_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[test]
    fn test_set_mac_address_on_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let test_mac_addr = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC];
        let mut set_mac_fut = test_values.iface.set_mac_address(test_mac_addr);

        assert_matches!(test_values.exec.run_until_stalled(&mut set_mac_fut), Poll::Pending);

        let (mac_addr, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::SetMacAddress { mac_addr, responder }))) => (mac_addr, responder)
        );
        assert_eq!(mac_addr, test_mac_addr);
        responder.send(Ok(())).expect("Failed to send SetMacAddress response");

        assert_matches!(test_values.exec.run_until_stalled(&mut set_mac_fut), Poll::Ready(Ok(())));
    }

    #[test]
    fn test_install_apf_packet_filter_on_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let test_program = vec![1, 2, 3, 4];
        let mut install_fut = test_values.iface.install_apf_packet_filter(test_program.clone());

        assert_matches!(test_values.exec.run_until_stalled(&mut install_fut), Poll::Pending);

        let (program, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::InstallApfPacketFilter { program, responder }))) => (program, responder)
        );
        assert_eq!(program, test_program);
        responder.send(Ok(())).expect("Failed to send InstallApfPacketFilter response");

        assert_matches!(test_values.exec.run_until_stalled(&mut install_fut), Poll::Ready(Ok(())));
    }

    #[test]
    fn test_read_apf_packet_filter_data_on_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let mut read_fut = test_values.iface.read_apf_packet_filter_data();

        assert_matches!(test_values.exec.run_until_stalled(&mut read_fut), Poll::Pending);

        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::ReadApfPacketFilterData { responder }))) => responder
        );
        let test_data = vec![5, 6, 7, 8];
        responder.send(Ok(&test_data)).expect("Failed to send ReadApfPacketFilterData response");

        let result = assert_matches!(test_values.exec.run_until_stalled(&mut read_fut), Poll::Ready(Ok(data)) => data);
        assert_eq!(result, test_data);
    }

    #[test]
    fn test_query_on_iface() {
        let mut test_values = setup_test_manager_with_iface();
        let mut query_fut = test_values.iface.query();
        assert_matches!(test_values.exec.run_until_stalled(&mut query_fut), Poll::Pending);
        let (iface_id, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::QueryIface { iface_id, responder }))) => (iface_id, responder));
        assert_eq!(iface_id, test_values.iface.iface_id);
        const RESPONSE: fidl_device_service::QueryIfaceResponse =
            fidl_device_service::QueryIfaceResponse {
                role: fidl_common::WlanMacRole::Client,
                id: 1,
                phy_id: 2,
                phy_assigned_id: 3,
                sta_addr: [4, 5, 6, 7, 8, 9],
                factory_addr: [4, 5, 6, 7, 8, 9],
            };
        responder.send(Ok(&RESPONSE)).expect("Failed to send result");
        let response = assert_matches!(test_values.exec.run_until_stalled(&mut query_fut), Poll::Ready(Ok(response)) => response);
        assert_eq!(response, RESPONSE);
    }

    #[test]
    fn test_trigger_scan_success() {
        let mut test_values = setup_test_manager_with_iface();
        assert!(test_values.iface.get_last_scan_results().is_empty());
        let mut scan_fut = test_values.iface.trigger_scan(None, vec![]);
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_fut), Poll::Pending);
        let (_req, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder));
        let result = wlan_common::scan::write_vmo(vec![test_utils::fake_scan_result()])
            .expect("Failed to write scan VMO");
        responder.send(Ok(result)).expect("Failed to send result");
        assert_matches!(
            test_values.exec.run_until_stalled(&mut scan_fut),
            Poll::Ready(Ok(ScanEnd::Complete))
        );
        assert_eq!(test_values.iface.get_last_scan_results().len(), 1);
    }

    #[test]
    fn test_trigger_scan_failure() {
        let mut test_values = setup_test_manager_with_iface();
        let mut scan_fut = test_values.iface.trigger_scan(None, vec![]);
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_fut), Poll::Pending);
        let (_req, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder));
        responder.send(Err(fidl_sme::ScanErrorCode::InternalError)).expect("Failed to send result");
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_fut), Poll::Ready(Err(_)));
    }

    #[test]
    fn test_trigger_scan_cancelled() {
        let mut test_values = setup_test_manager_with_iface();
        let mut scan_fut = test_values.iface.trigger_scan(None, vec![]);
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_fut), Poll::Pending);
        let (_req, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder));
        responder
            .send(Err(fidl_sme::ScanErrorCode::CanceledByDriverOrFirmware))
            .expect("Failed to send result");
        assert_matches!(
            test_values.exec.run_until_stalled(&mut scan_fut),
            Poll::Ready(Ok(ScanEnd::Cancelled))
        );
    }

    #[test]
    fn test_abort_scan() {
        let mut test_values = setup_test_manager_with_iface();
        assert!(test_values.iface.get_last_scan_results().is_empty());
        let mut scan_fut = test_values.iface.trigger_scan(None, vec![]);
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_fut), Poll::Pending);
        let (_req, _responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder));

        // trigger_scan returns after we abort the scan, even though we have no results from SME.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.iface.abort_scan()),
            Poll::Ready(Ok(()))
        );
        assert_matches!(
            test_values.exec.run_until_stalled(&mut scan_fut),
            Poll::Ready(Ok(ScanEnd::Cancelled))
        );
    }

    #[test]
    fn test_trigger_scan_timeout() {
        let mut test_values = setup_test_manager_with_iface_and_fake_time();
        assert!(test_values.iface.get_last_scan_results().is_empty());
        let mut scan_fut = test_values.iface.trigger_scan(None, vec![]);
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_fut), Poll::Pending);
        let (_req, _responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder));

        test_values.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(61_000_000_000));
        assert_matches!(test_values.exec.run_until_stalled(&mut scan_fut), Poll::Ready(Err(_)));

        let event =
            assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => event);
        assert_matches!(
            event,
            TelemetryEvent::SmeTimeout { source: wlan_telemetry::TimeoutSource::Scan }
        );
    }

    /// Build a WEP credential with the provided key saved as key 0. The key needs to be 5 or 13
    /// bytes or it will fail.
    fn build_wep_credential(key: Vec<u8>) -> Credential {
        let mut wep_keys = WepKeys::new();
        wep_keys.set_key(key, 0).expect("Failed to build WEP key for test");
        wep_keys.set_index(0).expect("Failed to set WEP key for test");
        Credential::WepKey(wep_keys)
    }

    #[test_case(
        FakeProtectionCfg::Open,
        vec![fidl_security::Protocol::Open],
        Credential::None,
        false,
        fidl_security::Authentication {
            protocol: fidl_security::Protocol::Open,
            credentials: None
        };
        "open_any_bssid"
    )]
    #[test_case(
        FakeProtectionCfg::Wpa2,
        vec![fidl_security::Protocol::Wpa2Personal],
        Credential::Password(b"password".to_vec()),
        false,
        fidl_security::Authentication {
            protocol: fidl_security::Protocol::Wpa2Personal,
            credentials: Some(Box::new(fidl_security::Credentials::Wpa(
                fidl_security::WpaCredentials::Passphrase(b"password".to_vec())
            )))
        };
        "wpa2_any_bssid"
    )]
    #[test_case(
        FakeProtectionCfg::Open,
        vec![fidl_security::Protocol::Open],
        Credential::None,
        false,
        fidl_security::Authentication {
            protocol: fidl_security::Protocol::Open,
            credentials: None
        };
        "bssid_specified"
    )]
    #[test_case(
        FakeProtectionCfg::Wpa1,
        vec![fidl_security::Protocol::Wpa1],
        Credential::Password(b"password".to_vec()),
        false,
        fidl_security::Authentication {
            protocol: fidl_security::Protocol::Wpa1,
            credentials: Some(Box::new(fidl_security::Credentials::Wpa(
                fidl_security::WpaCredentials::Passphrase(b"password".to_vec())
            )))
        };
        "wpa1_any_bssid"
    )]
    #[test_case(
        FakeProtectionCfg::Wep,
        vec![fidl_security::Protocol::Wep],
        build_wep_credential([1; wlan_common::security::wep::WEP40_KEY_BYTES].to_vec()),
        false,
        fidl_security::Authentication {
            protocol: fidl_security::Protocol::Wep,
            credentials: Some(Box::new(fidl_security::Credentials::Wep(
                fidl_security::WepCredentials{ key: [1; wlan_common::security::wep::WEP40_KEY_BYTES].into() }
            )))
        };
        "wep_any_bssid"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_connect_to_network(
        fake_protection_cfg: FakeProtectionCfg,
        mutual_security_protocols: Vec<fidl_security::Protocol>,
        credential: Credential,
        bssid_specified: bool,
        expected_authentication: fidl_security::Authentication,
    ) {
        let mut test_values = setup_test_manager_with_iface();

        let bss_description = fake_fidl_bss_description!(protection => fake_protection_cfg,
            ssid: Ssid::try_from("foo").unwrap(),
            bssid: [1, 2, 3, 4, 5, 6],
            rssi_dbm: -30,
        );
        *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::now(),
            vec![fidl_sme::ScanResult {
                bss_description: bss_description.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols,
                }),
                timestamp_nanos: 1,
            }],
        ));

        assert_matches!(test_values.iface.get_connected_network(), None);

        let bssid = if bssid_specified { Some(Bssid::from([1, 2, 3, 4, 5, 6])) } else { None };
        let mut connect_fut = test_values.iface.connect_to_network(b"foo", credential, bssid, None);
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (req, connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn: Some(txn), .. }))) => (req, txn));
        assert_eq!(req.bss_description, bss_description);
        assert_eq!(req.authentication, expected_authentication);

        let connect_txn_handle = connect_txn.into_stream_and_control_handle().1;
        let result = connect_txn_handle.send_on_connect_result(&fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        });
        assert_matches!(result, Ok(()));

        let connect_result = assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(r) => r);
        let connected_result = assert_matches!(connect_result, Ok(ConnectResult::Success(r)) => r);
        assert_eq!(connected_result.bss.ssid, Ssid::try_from("foo").unwrap());
        assert_eq!(connected_result.bss.bssid, Bssid::from([1, 2, 3, 4, 5, 6]));

        let connected_network =
            assert_matches!(test_values.iface.get_connected_network(), Some(n) => n);
        assert_eq!(connected_network.bssid, Bssid::from([1, 2, 3, 4, 5, 6]));
        assert_eq!(connected_network.rssi, -30);
    }

    #[fuchsia::test]
    fn test_connect_to_owe_transition_network_with_no_scan_needed() {
        let mut test_values = setup_test_manager_with_iface();
        let owe_ssid = "owe-ssid";
        let transition_ssid = "transition-ssid";
        const OWE_BSSID: [u8; 6] = [2, 2, 2, 2, 2, 2];
        const OPEN_BSSID: [u8; 6] = [1, 1, 1, 1, 1, 1];

        // 1. Create OWE transition IE for the Open network (points to OWE BSS)
        let mut open_transition_ie = vec![
            ie::Id::VENDOR_SPECIFIC.0,
            (11 + owe_ssid.len()) as u8, // Length of IE
            0x50,
            0x6f,
            0x9a, // 3 bytes of OUI
            wlan_common::ie::owe_transition::VENDOR_SPECIFIC_TYPE,
        ];
        open_transition_ie.extend_from_slice(&OWE_BSSID); // BSSID of OWE BSS
        open_transition_ie.push(owe_ssid.len() as u8);
        open_transition_ie.extend_from_slice(owe_ssid.as_bytes());

        // 2. Create OWE transition IE for the OWE BSS (points to Open BSS). This is not
        // currently used in the connect process but is accurate to what the spec requires.
        let mut owe_transition_ie = vec![
            ie::Id::VENDOR_SPECIFIC.0,
            (11 + transition_ssid.len()) as u8, // Length of IE
            0x50,
            0x6f,
            0x9a, // 3 bytes of OUI
            wlan_common::ie::owe_transition::VENDOR_SPECIFIC_TYPE,
        ];
        owe_transition_ie.extend_from_slice(&OPEN_BSSID); // BSSID of Open BSS
        owe_transition_ie.push(transition_ssid.len() as u8);
        owe_transition_ie.extend_from_slice(transition_ssid.as_bytes());

        // This BSS is created as open with the OWE transition IE pointing to the OWE BSS.
        let transition_bss = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Open,
            ssid: Ssid::try_from(transition_ssid).unwrap(),
            bssid: OPEN_BSSID,
            rssi_dbm: -50,
            ies_overrides: wlan_common::test_utils::fake_stas::IesOverrides::new()
                .set_raw(open_transition_ie),
        );

        // This is the BSS description that would show up for the hidden OWE BSS in a passive scan.
        let hidden_owe_bss = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Owe,
            ssid: Ssid::try_from("").unwrap(),
            bssid: OWE_BSSID,
            rssi_dbm: -40,
            ies_overrides: wlan_common::test_utils::fake_stas::IesOverrides::new()
                .set_raw(owe_transition_ie.clone()),
        );

        // This is the BSS description that would show up for the OWE BSS in an active scan.
        let owe_bss = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Owe,
            ssid: Ssid::try_from(owe_ssid).unwrap(),
            bssid: OWE_BSSID,
            rssi_dbm: -40,
            ies_overrides: wlan_common::test_utils::fake_stas::IesOverrides::new()
                .set_raw(owe_transition_ie),
        );

        // Build scan results that contain the transition and OWE BSSs and set the scan cache; the
        // connect process will use these scan results.
        *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::now(),
            vec![
                fidl_sme::ScanResult {
                    bss_description: transition_bss.clone(),
                    compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                        mutual_security_protocols: vec![
                            fidl_security::Protocol::Open,
                            fidl_security::Protocol::Owe,
                        ],
                    }),
                    timestamp_nanos: 1,
                },
                fidl_sme::ScanResult {
                    bss_description: hidden_owe_bss.clone(),
                    compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                        mutual_security_protocols: vec![fidl_security::Protocol::Owe],
                    }),
                    timestamp_nanos: 1,
                },
            ],
        ));

        // Initiate a connect request to the OWE transition network, which should use the
        // transition SSID and result in a connection to the OWE BSS.
        let mut connect_fut = test_values.iface.connect_to_network(
            transition_ssid.as_bytes(),
            Credential::None,
            None,
            Some(fidl_wlanix::KeyMgmtMask::OWE),
        );

        // Verify that an active scan is triggered for the OWE SSID on the channel of the cached BSS.
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (scan_req, scan_responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder)
        );
        let expected_channel = hidden_owe_bss.primary.number;
        assert_matches!(&scan_req, fidl_sme::ScanRequest::Active(active_scan_req) => {
            assert_eq!(active_scan_req.ssids, vec![Ssid::try_from(owe_ssid).unwrap().to_vec()]);
            assert_eq!(active_scan_req.channels, vec![expected_channel]);
        });

        // Respond to the scan request with the OWE BSS
        let scan_result_vmo = wlan_common::scan::write_vmo(vec![fidl_sme::ScanResult {
            bss_description: owe_bss.clone(),
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_security::Protocol::Owe],
            }),
            timestamp_nanos: 1,
        }])
        .unwrap();
        scan_responder.send(Ok(scan_result_vmo)).unwrap();

        // Now the connect request should be sent for the OWE BSS.
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (connect_req, connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn, .. }))) => (req, txn)
        );

        assert_eq!(connect_req.bss_description, owe_bss);
        assert_eq!(connect_req.authentication.protocol, fidl_security::Protocol::Owe);

        let connect_txn_handle = connect_txn.unwrap().into_stream_and_control_handle().1;
        connect_txn_handle
            .send_on_connect_result(&fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::Success,
                is_credential_rejected: false,
                is_reconnect: false,
            })
            .unwrap();

        let connect_result = test_values.exec.run_singlethreaded(&mut connect_fut).unwrap();
        let success = assert_matches!(connect_result, ConnectResult::Success(s) => s);
        assert_eq!(success.bss.bssid.to_array(), OWE_BSSID);
        // Check that SSID requested is reported so that this SSID will be reported back to the
        // caller.
        assert_eq!(success.ssid_if_owe_transition, Some(Ssid::try_from(transition_ssid).unwrap()));
    }

    #[fuchsia::test]
    fn test_connect_to_owe_transition_network_with_rssi_not_good_enough() {
        let mut test_values = setup_test_manager_with_iface();
        let owe_ssid = "owe-ssid";
        let transition_ssid = "transition-ssid";
        const OWE_BSSID: [u8; 6] = [2, 2, 2, 2, 2, 2];
        const OPEN_BSSID: [u8; 6] = [1, 1, 1, 1, 1, 1];
        const OTHER_OWE_BSSID: [u8; 6] = [3, 3, 3, 3, 3, 3];

        // The signal strength of the open network is very strong but the OWE BSS
        // it points to is a bit weak. A scan should be performed to see if there
        // is a better BSS. This test will contain another OWE BSS with a better signal.
        let open_rssi = -50;
        let owe_rssi = -70;
        let other_owe_rssi = -50;

        // 1. Create OWE transition IE for the Open network (points to OWE BSS)
        let mut open_transition_ie = vec![
            ie::Id::VENDOR_SPECIFIC.0,
            (11 + owe_ssid.len()) as u8, // Length of IE
            0x50,
            0x6f,
            0x9a, // 3 bytes of OUI
            wlan_common::ie::owe_transition::VENDOR_SPECIFIC_TYPE,
        ];
        open_transition_ie.extend_from_slice(&OWE_BSSID); // BSSID of OWE BSS
        open_transition_ie.push(owe_ssid.len() as u8);
        open_transition_ie.extend_from_slice(owe_ssid.as_bytes());

        // 2. Create OWE transition IE for the OWE BSS (points to Open BSS). This is not
        // currently used in the connect process but is accurate to what the spec requires.
        let mut owe_transition_ie = vec![
            ie::Id::VENDOR_SPECIFIC.0,
            (11 + transition_ssid.len()) as u8, // Length of IE
            0x50,
            0x6f,
            0x9a, // 3 bytes of OUI
            wlan_common::ie::owe_transition::VENDOR_SPECIFIC_TYPE,
        ];
        owe_transition_ie.extend_from_slice(&OPEN_BSSID); // BSSID of Open BSS
        owe_transition_ie.push(transition_ssid.len() as u8);
        owe_transition_ie.extend_from_slice(transition_ssid.as_bytes());

        // This BSS is created as open with the OWE transition IE pointing to the OWE BSS.
        let transition_bss = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Open,
            ssid: Ssid::try_from(transition_ssid).unwrap(),
            bssid: OPEN_BSSID,
            rssi_dbm: open_rssi,
            ies_overrides: wlan_common::test_utils::fake_stas::IesOverrides::new()
                .set_raw(open_transition_ie),
        );

        // This is the BSS description that would show up for the hidden OWE BSS in a passive scan.
        let hidden_owe_bss = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Owe,
            ssid: Ssid::try_from("").unwrap(),
            bssid: OWE_BSSID,
            rssi_dbm: owe_rssi,
            ies_overrides: wlan_common::test_utils::fake_stas::IesOverrides::new()
                .set_raw(owe_transition_ie.clone()),
        );

        // This is the BSS description that would show up for the OWE BSS in an active scan; the
        // only difference is that it contains the SSID.
        let owe_bss = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Owe,
            ssid: Ssid::try_from(owe_ssid).unwrap(),
            bssid: OWE_BSSID,
            rssi_dbm: owe_rssi,
            ies_overrides: wlan_common::test_utils::fake_stas::IesOverrides::new()
                .set_raw(owe_transition_ie.clone()),
        );

        // This is for another OWE BSS with the OWE
        let other_owe_bss = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Owe,
            ssid: Ssid::try_from(owe_ssid).unwrap(),
            bssid: OTHER_OWE_BSSID,
            rssi_dbm: other_owe_rssi,
            ies_overrides: wlan_common::test_utils::fake_stas::IesOverrides::new()
                .set_raw(owe_transition_ie),
        );

        // Build scan results that contain the transition and OWE BSSs and set the scan cache; the
        // connect process will use these scan results.
        *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::now(),
            vec![
                fidl_sme::ScanResult {
                    bss_description: transition_bss.clone(),
                    compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                        mutual_security_protocols: vec![
                            fidl_security::Protocol::Open,
                            fidl_security::Protocol::Owe,
                        ],
                    }),
                    timestamp_nanos: 1,
                },
                fidl_sme::ScanResult {
                    bss_description: hidden_owe_bss.clone(),
                    compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                        mutual_security_protocols: vec![fidl_security::Protocol::Owe],
                    }),
                    timestamp_nanos: 1,
                },
            ],
        ));

        // Initiate a connect request to the OWE transition network, which should use the
        // transition SSID and result in a connection to the OWE BSS.
        let mut connect_fut = test_values.iface.connect_to_network(
            transition_ssid.as_bytes(),
            Credential::None,
            None,
            Some(fidl_wlanix::KeyMgmtMask::OWE),
        );

        // Verify that an active scan is triggered for the OWE SSID.
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (scan_req, scan_responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder)
        );
        assert_matches!(&scan_req, fidl_sme::ScanRequest::Active(active_scan_req) => {
            assert_eq!(active_scan_req.ssids, vec![Ssid::try_from(owe_ssid).unwrap().to_vec()]);
        });

        // Respond to the scan request with the OWE BSS included in the OWE IE, and the other
        // matching OWE BSS that has a stronger signal.
        let scan_result_vmo = wlan_common::scan::write_vmo(vec![
            fidl_sme::ScanResult {
                bss_description: owe_bss.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Owe],
                }),
                timestamp_nanos: 1,
            },
            fidl_sme::ScanResult {
                bss_description: other_owe_bss.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Owe],
                }),
                timestamp_nanos: 2,
            },
        ])
        .unwrap();
        scan_responder.send(Ok(scan_result_vmo)).unwrap();

        // Now the connect request should be sent for the other OWE BSS with a stronger signal.
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (connect_req, connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn, .. }))) => (req, txn)
        );

        assert_eq!(connect_req.bss_description, other_owe_bss);
        assert_eq!(connect_req.authentication.protocol, fidl_security::Protocol::Owe);

        let connect_txn_handle = connect_txn.unwrap().into_stream_and_control_handle().1;
        connect_txn_handle
            .send_on_connect_result(&fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::Success,
                is_credential_rejected: false,
                is_reconnect: false,
            })
            .unwrap();

        let connect_result = test_values.exec.run_singlethreaded(&mut connect_fut).unwrap();
        let success = assert_matches!(connect_result, ConnectResult::Success(s) => s);
        assert_eq!(success.bss.bssid.to_array(), OTHER_OWE_BSSID);
        // Check that SSID requested is reported so that this SSID will be reported back to the
        // caller.
        assert_eq!(success.ssid_if_owe_transition, Some(Ssid::try_from(transition_ssid).unwrap()));
    }

    #[test]
    fn test_connect_to_network_before_scan() {
        let mut test_values = setup_test_manager_with_iface();

        let bssid = [1, 2, 3, 4, 5, 6];
        let bss_description = fake_fidl_bss_description!(protection => FakeProtectionCfg::Open,
            ssid: Ssid::try_from("foo").unwrap(),
            bssid: bssid,
        );
        let mut connect_fut = test_values.iface.connect_to_network(
            b"foo",
            Credential::None,
            Some(Bssid::from(bssid)),
            None,
        );
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (_req, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder));
        let result = wlan_common::scan::write_vmo(vec![fidl_sme::ScanResult {
            bss_description: bss_description.clone(),
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_security::Protocol::Open],
            }),
            timestamp_nanos: 1,
        }])
        .expect("Failed to write scan VMO");
        responder.send(Ok(result)).expect("Failed to send result");
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);

        let (req, connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn: Some(txn), .. }))) => (req, txn));
        assert_eq!(req.bss_description, bss_description);

        let connect_txn_handle = connect_txn.into_stream_and_control_handle().1;
        let result = connect_txn_handle.send_on_connect_result(&fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        });
        assert_matches!(result, Ok(()));

        let connect_result = assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(r) => r);
        let connected_result = assert_matches!(connect_result, Ok(ConnectResult::Success(r)) => r);
        assert_eq!(connected_result.bss.ssid, Ssid::try_from("foo").unwrap());
        assert_eq!(connected_result.bss.bssid, Bssid::from(bssid));
    }

    #[test]
    fn test_connect_to_network_stale_scan() {
        let mut test_values = setup_test_manager_with_iface_and_fake_time();

        let other_bss_description = fake_fidl_bss_description!(protection => FakeProtectionCfg::Open,
            ssid: Ssid::try_from("bar").unwrap(),
            bssid: [11, 22, 33, 44, 55, 66],
        );
        *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::from_nanos(1),
            vec![fidl_sme::ScanResult {
                bss_description: other_bss_description.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Open],
                }),
                timestamp_nanos: 1,
            }],
        ));

        // Set current time to 31st second so that a scan would be triggered when handling
        // connect request.
        test_values.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(31_000_000_000));
        let bssid = [1, 2, 3, 4, 5, 6];
        let bss_description = fake_fidl_bss_description!(protection => FakeProtectionCfg::Open,
            ssid: Ssid::try_from("foo").unwrap(),
            bssid: bssid,
        );
        let mut connect_fut = test_values.iface.connect_to_network(
            b"foo",
            Credential::None,
            Some(Bssid::from(bssid)),
            None,
        );
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (_req, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => (req, responder));
        let result = wlan_common::scan::write_vmo(vec![fidl_sme::ScanResult {
            bss_description: bss_description.clone(),
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_security::Protocol::Open],
            }),
            timestamp_nanos: 1,
        }])
        .expect("Failed to write scan VMO");
        responder.send(Ok(result)).expect("Failed to send result");
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);

        let (req, connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn: Some(txn), .. }))) => (req, txn));
        assert_eq!(req.bss_description, bss_description);

        let connect_txn_handle = connect_txn.into_stream_and_control_handle().1;
        let result = connect_txn_handle.send_on_connect_result(&fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        });
        assert_matches!(result, Ok(()));

        let connect_result = assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(r) => r);
        let connected_result = assert_matches!(connect_result, Ok(ConnectResult::Success(r)) => r);
        assert_eq!(connected_result.bss.ssid, Ssid::try_from("foo").unwrap());
        assert_eq!(connected_result.bss.bssid, Bssid::from(bssid));
    }

    #[test_case(
        false,
        FakeProtectionCfg::Open,
        vec![fidl_security::Protocol::Open],
        Credential::None,
        None;
        "network_not_found"
    )]
    #[test_case(
        true,
        FakeProtectionCfg::Open,
        vec![fidl_security::Protocol::Open],
        Credential::Password(b"password".to_vec()),
        None;
        "open_with_password"
    )]
    #[test_case(
        true,
        FakeProtectionCfg::Wpa2,
        vec![fidl_security::Protocol::Wpa2Personal],
        Credential::None,
        None;
        "wpa2_without_password"
    )]
    #[test_case(
        true,
        FakeProtectionCfg::Wpa2,
        vec![fidl_security::Protocol::Open],
        Credential::None,
        Some([24, 51, 32, 52, 41, 32].into());
        "bssid_not_found"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_connect_rejected(
        has_network: bool,
        fake_protection_cfg: FakeProtectionCfg,
        mutual_security_protocols: Vec<fidl_security::Protocol>,
        credential: Credential,
        bssid: Option<Bssid>,
    ) {
        let mut test_values = setup_test_manager_with_iface();

        if has_network {
            let bss_description = fake_fidl_bss_description!(protection => fake_protection_cfg,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [1, 2, 3, 4, 5, 6],
            );
            *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
                fasync::BootInstant::now(),
                vec![fidl_sme::ScanResult {
                    bss_description: bss_description.clone(),
                    compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                        mutual_security_protocols,
                    }),
                    timestamp_nanos: 1,
                }],
            ));
        } else {
            *test_values.iface.last_scan_results.lock() =
                Some(LastScanResults::new(fasync::BootInstant::now(), vec![]));
        }

        let mut connect_fut = test_values.iface.connect_to_network(b"foo", credential, bssid, None);
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(Err(_e)));
    }

    #[test]
    fn test_connect_fails_at_sme() {
        let mut test_values = setup_test_manager_with_iface();

        let bss_description = fake_fidl_bss_description!(Open,
            ssid: Ssid::try_from("foo").unwrap(),
            bssid: [1, 2, 3, 4, 5, 6],
        );
        *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::now(),
            vec![fidl_sme::ScanResult {
                bss_description: bss_description.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Open],
                }),
                timestamp_nanos: 1,
            }],
        ));

        let mut connect_fut =
            test_values.iface.connect_to_network(b"foo", Credential::None, None, None);
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (req, connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn: Some(txn), .. }))) => (req, txn));
        assert_eq!(req.bss_description, bss_description);
        assert_eq!(
            req.authentication,
            fidl_security::Authentication {
                protocol: fidl_security::Protocol::Open,
                credentials: None,
            }
        );

        let connect_txn_handle = connect_txn.into_stream_and_control_handle().1;
        let result = connect_txn_handle.send_on_connect_result(&fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::RefusedExternalReason,
            is_credential_rejected: false,
            is_reconnect: false,
        });
        assert_matches!(result, Ok(()));

        let connect_result = assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(r)) => r);
        let failure = assert_matches!(connect_result, ConnectResult::Fail(failure) => failure);
        assert_eq!(failure.status_code, fidl_ieee80211::StatusCode::RefusedExternalReason);
        assert!(!failure.timed_out);
    }

    #[test]
    fn test_connect_fails_with_timeout() {
        let mut test_values = setup_test_manager_with_iface_and_fake_time();

        let bss_description = fake_fidl_bss_description!(Open,
            ssid: Ssid::try_from("foo").unwrap(),
            bssid: [1, 2, 3, 4, 5, 6],
        );
        *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::now(),
            vec![fidl_sme::ScanResult {
                bss_description: bss_description.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Open],
                }),
                timestamp_nanos: 1,
            }],
        ));

        let mut connect_fut =
            test_values.iface.connect_to_network(b"foo", Credential::None, None, None);
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (_req, _connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn: Some(txn), .. }))) => (req, txn));
        test_values.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(40_000_000_000));

        let connect_result = assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(r)) => r);
        let failure = assert_matches!(connect_result, ConnectResult::Fail(failure) => failure);
        assert!(failure.timed_out);

        let event =
            assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => event);
        assert_matches!(
            event,
            TelemetryEvent::SmeTimeout { source: wlan_telemetry::TimeoutSource::Connect }
        );
    }

    #[test_case(
        vec![
            fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [1, 2, 3, 4, 5, 6],
                channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
                rssi_dbm: -40,
            ),
            fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [2, 3, 4, 5, 6, 7],
                channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
                rssi_dbm: -30,
            ),
            fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [3, 4, 5, 6, 7, 8],
                channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
                rssi_dbm: -50,
            ),
        ],
        None,
        Bssid::from([2, 3, 4, 5, 6, 7]);
        "no_penalty"
    )]
    #[test_case(
        vec![
            fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [1, 2, 3, 4, 5, 6],
                channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
                rssi_dbm: -40,
            ),
            fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [2, 3, 4, 5, 6, 7],
                channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
                rssi_dbm: -30,
            ),
            fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [3, 4, 5, 6, 7, 8],
                channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
                rssi_dbm: -50,
            ),
        ],
        Some((
            fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("foo").unwrap(),
                bssid: [2, 3, 4, 5, 6, 7],
                channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
                rssi_dbm: -30,
            ),
            fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::RefusedExternalReason,
                is_credential_rejected: true,
                is_reconnect: false,
            }
        )),
        Bssid::from([1, 2, 3, 4, 5, 6]);
        "recent_connect_failure"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_connect_to_network_bss_selection(
        scan_bss_descriptions: Vec<fidl_ieee80211::BssDescription>,
        recent_connect_failure: Option<(fidl_ieee80211::BssDescription, fidl_sme::ConnectResult)>,
        expected_bssid: Bssid,
    ) {
        let mut test_values = setup_test_manager_with_iface();
        let iface = test_values.iface;
        let mut sme_stream = test_values.sme_stream;

        if let Some((bss_description, connect_failure)) = recent_connect_failure {
            // Set up a connect failure so that later in the test, there'd be a score penalty
            // for the BSS described by `bss_description`
            *iface.last_scan_results.lock() = Some(LastScanResults::new(
                fasync::BootInstant::now(),
                vec![fidl_sme::ScanResult {
                    bss_description,
                    compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                        mutual_security_protocols: vec![fidl_security::Protocol::Open],
                    }),
                    timestamp_nanos: 1,
                }],
            ));

            let mut connect_fut = iface.connect_to_network(b"foo", Credential::None, None, None);
            assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
            let (_req, connect_txn) = assert_matches!(
                test_values.exec.run_until_stalled(&mut sme_stream.next()),
                Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn: Some(txn), .. }))) => (req, txn));
            let connect_txn_handle = connect_txn.into_stream_and_control_handle().1;
            let _result = connect_txn_handle.send_on_connect_result(&connect_failure);
            assert_matches!(
                test_values.exec.run_until_stalled(&mut connect_fut),
                Poll::Ready(Ok(_r))
            );
        }

        *iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::now(),
            scan_bss_descriptions
                .into_iter()
                .map(|bss_description| fidl_sme::ScanResult {
                    bss_description,
                    compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                        mutual_security_protocols: vec![fidl_security::Protocol::Open],
                    }),
                    timestamp_nanos: 1,
                })
                .collect::<Vec<_>>(),
        ));

        let mut connect_fut = iface.connect_to_network(b"foo", Credential::None, None, None);
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);
        let (req, _connect_txn) = assert_matches!(
            test_values.exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, txn: Some(txn), .. }))) => (req, txn));
        assert_eq!(req.bss_description.bssid, expected_bssid.to_array());
    }

    #[test]
    fn test_disconnect() {
        let mut test_values = setup_test_manager_with_iface();

        let mut disconnect_fut = test_values.iface.disconnect();
        assert_matches!(test_values.exec.run_until_stalled(&mut disconnect_fut), Poll::Pending);
        let (disconnect_reason, disconnect_responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Disconnect { reason, responder }))) => (reason, responder));
        assert_eq!(disconnect_reason, fidl_sme::UserDisconnectReason::Unknown);

        assert_matches!(disconnect_responder.send(), Ok(()));
        assert_matches!(
            test_values.exec.run_until_stalled(&mut disconnect_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[test]
    fn test_disconnect_timeout() {
        let mut test_values = setup_test_manager_with_iface_and_fake_time();

        let mut disconnect_fut = test_values.iface.disconnect();
        assert_matches!(test_values.exec.run_until_stalled(&mut disconnect_fut), Poll::Pending);
        let (_disconnect_reason, _disconnect_responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Disconnect { reason, responder }))) => (reason, responder));

        test_values.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(11_000_000_000));
        assert_matches!(
            test_values.exec.run_until_stalled(&mut disconnect_fut),
            Poll::Ready(Err(_))
        );

        let event =
            assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => event);
        assert_matches!(
            event,
            TelemetryEvent::SmeTimeout { source: wlan_telemetry::TimeoutSource::Disconnect }
        );
    }

    #[test]
    fn test_on_disconnect() {
        let test_values = setup_test_manager_with_iface();

        *test_values.iface.connected_network.lock() = Some(test_utils::fake_connected_network());
        assert_matches!(test_values.iface.get_connected_network(), Some(_));
        test_values.iface.on_disconnect(&fidl_sme::DisconnectSource::User(
            fidl_sme::UserDisconnectReason::Unknown,
        ));
        assert_matches!(test_values.iface.get_connected_network(), None);
    }

    #[test]
    fn test_on_signal_report() {
        let test_values = setup_test_manager_with_iface();

        assert_matches!(test_values.iface.get_connected_network(), None);
        test_values.iface.on_signal_report(fidl_internal::SignalReportIndication {
            rssi_dbm: -40,
            snr_db: 20,
            tx_rate_500kbps: 0,
        });
        assert_matches!(test_values.iface.get_connected_network(), None);

        *test_values.iface.connected_network.lock() = Some(test_utils::fake_connected_network());
        assert_matches!(test_values.iface.get_connected_network().map(|n| n.rssi), Some(-35));
        test_values.iface.on_signal_report(fidl_internal::SignalReportIndication {
            rssi_dbm: -40,
            snr_db: 20,
            tx_rate_500kbps: 0,
        });
        assert_matches!(test_values.iface.get_connected_network().map(|n| n.rssi), Some(-40));
    }

    #[test]
    fn test_get_signal_poll_results_success() {
        let mut test_values = setup_test_manager_with_iface();

        let mut signal_report_fut = test_values.iface.get_signal_report();
        assert_matches!(test_values.exec.run_until_stalled(&mut signal_report_fut), Poll::Pending);

        let responder = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.telemetry_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::TelemetryRequest::GetSignalReport { responder }))) => responder
        );

        let report = fidl_stats::SignalReport {
            connection_signal_report: Some(fidl_stats::ConnectionSignalReport {
                rssi_dbm: Some(-53),
                snr_db: Some(25),
                tx_rate_500kbps: Some(300),
                primary: Some(fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::FiveGhz,
                    number: 36,
                }),
                bandwidth: Some(fidl_ieee80211::ChannelBandwidth::Cbw20),
                vht_secondary_80_channel: Some(fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::FiveGhz,
                    number: 0,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        responder.send(Ok(&report)).expect("Failed to send mock signal report response");

        let response = assert_matches!(test_values.exec.run_until_stalled(&mut signal_report_fut), Poll::Ready(Ok(response)) => response);

        // Ensure values unpacked correctly
        let conn_report = response.connection_signal_report.expect("No connection report");
        assert_eq!(conn_report.tx_rate_500kbps, Some(300));
        assert_eq!(conn_report.rssi_dbm, Some(-53));
    }

    #[test]
    fn test_set_bt_coexistence_mode() {
        let mut test_values = setup_test_manager_with_iface();

        let mut set_bt_coex_fut =
            test_values.iface.set_bt_coexistence_mode(fidl_internal::BtCoexistenceMode::ModeAuto);
        assert_matches!(test_values.exec.run_until_stalled(&mut set_bt_coex_fut), Poll::Pending);
        let (mode, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::SetBtCoexistenceMode { mode, responder, .. })) => (mode, responder));
        assert_eq!(mode, fidl_internal::BtCoexistenceMode::ModeAuto);
        assert_matches!(responder.send(Ok(())), Ok(()));

        assert_matches!(
            test_values.exec.run_until_stalled(&mut set_bt_coex_fut),
            Poll::Ready(Ok(()))
        );
    }

    #[derive(PartialEq)]
    enum PowerCall {
        SetPowerSaveMode(bool),
        SetSuspendMode(bool),
    }
    #[test_case(vec![
        // Turning on power save mode should take us to PsModeBalanced
        (PowerCall::SetPowerSaveMode(true), fidl_common::PowerSaveType::PsModeBalanced),
        // Regardless of power save mode, suspend mode should take us to PsModeUltraLowPower
        (PowerCall::SetSuspendMode(true), fidl_common::PowerSaveType::PsModeUltraLowPower),
    ]; "Suspend mode overrides power save on")]
    #[test_case(vec![
        // Turning off power save mode should take us to PsModePerformance
        (PowerCall::SetPowerSaveMode(false), fidl_common::PowerSaveType::PsModePerformance),
        // Regardless of power save mode, suspend mode should take us to PsModeUltraLowPower
        (PowerCall::SetSuspendMode(true), fidl_common::PowerSaveType::PsModeUltraLowPower),
    ]; "Suspend mode overrides power save off")]
    #[test_case(vec![
        (PowerCall::SetSuspendMode(true), fidl_common::PowerSaveType::PsModeUltraLowPower),
        // Once we're in suspend mode, changing power save mode should have no effect
        (PowerCall::SetPowerSaveMode(true), fidl_common::PowerSaveType::PsModeUltraLowPower),
        (PowerCall::SetPowerSaveMode(false), fidl_common::PowerSaveType::PsModeUltraLowPower),
    ]; "Power save has no effect during suspend mode")]
    #[test_case(vec![
        (PowerCall::SetPowerSaveMode(true), fidl_common::PowerSaveType::PsModeBalanced),
        (PowerCall::SetSuspendMode(true), fidl_common::PowerSaveType::PsModeUltraLowPower),
        // When turning off suspend mode, we should revert to the previous setting of power save mode
        // If power save was on before suspend, it should be on after as well
        (PowerCall::SetSuspendMode(false), fidl_common::PowerSaveType::PsModeBalanced)
    ]; "Turning off suspend mode reverts to power save on")]
    #[test_case(vec![
        (PowerCall::SetPowerSaveMode(false), fidl_common::PowerSaveType::PsModePerformance),
        (PowerCall::SetSuspendMode(true), fidl_common::PowerSaveType::PsModeUltraLowPower),
        // When turning off suspend mode, we should revert to the previous setting of power save mode
        // If power save was off before suspend, it should be off after as well
        (PowerCall::SetSuspendMode(false), fidl_common::PowerSaveType::PsModePerformance)
    ]; "Turning off suspend mode reverts to power save off")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_set_power_mode(sequence: Vec<(PowerCall, fidl_common::PowerSaveType)>) {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (sme_proxy, mut sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let phy_id = rand::random();
        let (telemetry_sender, mut _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);

        // Create the interface with a power broker channel
        let iface = SmeClientIface::new(
            phy_id,
            TEST_IFACE_ID,
            sme_proxy,
            fidl::endpoints::create_proxy::<fidl_sme::TelemetryMarker>().0,
            monitor_svc,
            TelemetrySender::new(telemetry_sender),
        );

        // Simulate that a filter is installed so APF calls are not skipped in tests.
        exec.run_singlethreaded(async {
            let mut power_state = iface.power_state.lock().await;
            power_state.apf_filter_installed = true;
        });

        // Run each call in the test sequence
        for (call, expected_driver_val) in sequence {
            // Set the power save mode
            let power_call_fut = match call {
                PowerCall::SetPowerSaveMode(val) => iface.set_power_save_mode(val),
                PowerCall::SetSuspendMode(val) => iface.set_suspend_mode(val),
            };
            let mut power_call_fut = pin!(power_call_fut);
            assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);

            // Respond to the call to set APF status
            assert_matches!(
                exec.run_until_stalled(&mut sme_stream.next()),
                Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::SetApfPacketFilterEnabled { enabled: _, responder }))) => {
                    responder.send(Ok(())).expect("failed to send SME response");
                }
            );
            assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);

            // Validate the expected setting is made in the driver
            assert_matches!(
                exec.run_until_stalled(&mut monitor_stream.next()),
                Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetPowerSaveMode { req, responder }))) => {
                    assert_eq!(req.phy_id, phy_id);
                    assert_matches!(responder.send(zx::sys::ZX_OK), Ok(()));
                    assert_eq!(req.ps_mode, expected_driver_val);
            });

            // Future completes
            exec.run_singlethreaded(power_call_fut).expect("future finished");
        }
    }

    #[test_case((PowerCall::SetPowerSaveMode(true), fidl_common::PowerSaveType::PsModeBalanced))]
    #[test_case((PowerCall::SetPowerSaveMode(false), fidl_common::PowerSaveType::PsModePerformance))]
    #[test_case((PowerCall::SetSuspendMode(true), fidl_common::PowerSaveType::PsModeUltraLowPower))]
    #[fuchsia::test(add_test_attr = false)]
    fn test_set_power_mode_metrics(
        (call, expected_driver_val): (PowerCall, fidl_common::PowerSaveType),
    ) {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (sme_proxy, mut sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let phy_id = rand::random();
        let (telemetry_sender, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);

        // Create the interface with a power broker channel
        let iface = SmeClientIface::new(
            phy_id,
            TEST_IFACE_ID,
            sme_proxy,
            fidl::endpoints::create_proxy::<fidl_sme::TelemetryMarker>().0,
            monitor_svc,
            TelemetrySender::new(telemetry_sender),
        );

        // Simulate that a filter is installed so APF calls are not skipped in tests.
        exec.run_singlethreaded(async {
            let mut power_state = iface.power_state.lock().await;
            power_state.apf_filter_installed = true;
        });

        // Set the power save mode
        let power_call_fut = match call {
            PowerCall::SetPowerSaveMode(val) => iface.set_power_save_mode(val),
            PowerCall::SetSuspendMode(val) => iface.set_suspend_mode(val),
        };
        let mut power_call_fut = pin!(power_call_fut);
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);

        // Respond to the call to set APF status
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::SetApfPacketFilterEnabled { enabled: _, responder }))) => {
                responder.send(Ok(())).expect("failed to send SetApfPacketFilterEnabled response");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);
        // Respond to the call to set power state
        assert_matches!(
            exec.run_until_stalled(&mut monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetPowerSaveMode { req: _, responder }))) => {
                responder.send(zx::sys::ZX_OK).expect("failed to send SetPowerSaveMode response");
            }
        );

        // Future completes
        exec.run_singlethreaded(power_call_fut).expect("future finished");

        // Validate telemetry event is sent
        let expected_metric = match expected_driver_val {
            fidl_common::PowerSaveType::PsModeUltraLowPower => {
                wlan_telemetry::IfacePowerLevel::SuspendMode
            }
            fidl_common::PowerSaveType::PsModeLowPower => panic!("Unexpected value"),
            fidl_common::PowerSaveType::PsModeBalanced => wlan_telemetry::IfacePowerLevel::Normal,
            fidl_common::PowerSaveType::PsModePerformance => {
                wlan_telemetry::IfacePowerLevel::NoPowerSavings
            }
        };

        let event = assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => event);
        assert_matches!(event, TelemetryEvent::IfacePowerLevelChanged {
            iface_id,
            iface_power_level
        } => {
            assert_eq!(iface_id, TEST_IFACE_ID);
            assert_eq!(iface_power_level, expected_metric)
        });
    }

    #[test_case(PowerCall::SetPowerSaveMode(true))]
    #[test_case(PowerCall::SetPowerSaveMode(false))]
    #[fuchsia::test(add_test_attr = false)]
    fn test_set_power_mode_unclear_demand_metric(call: PowerCall) {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (sme_proxy, mut sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let phy_id = rand::random();
        let (telemetry_sender, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);

        // Create the interface with a power broker channel
        let iface = SmeClientIface::new(
            phy_id,
            TEST_IFACE_ID,
            sme_proxy,
            fidl::endpoints::create_proxy::<fidl_sme::TelemetryMarker>().0,
            monitor_svc,
            TelemetrySender::new(telemetry_sender),
        );

        // Simulate that a filter is installed so APF calls are not skipped in tests.
        exec.run_singlethreaded(async {
            let mut power_state = iface.power_state.lock().await;
            power_state.apf_filter_installed = true;
        });

        // Set suspend mode on
        let power_call_fut = iface.set_suspend_mode(true);
        let mut power_call_fut = pin!(power_call_fut);
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);
        // Respond to the call to set APF status
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::SetApfPacketFilterEnabled { enabled: _, responder }))) => {
                responder.send(Ok(())).expect("failed to send SetApfPacketFilterEnabled response");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);
        // Respond to the call to set power state
        assert_matches!(
            exec.run_until_stalled(&mut monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetPowerSaveMode { req: _, responder }))) => {
                responder.send(zx::sys::ZX_OK).expect("failed to send SetPowerSaveMode response");
            }
        );
        // Future completes
        exec.run_singlethreaded(power_call_fut).expect("future finished");

        let event = assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => event);
        assert_matches!(
            event,
            TelemetryEvent::IfacePowerLevelChanged { iface_power_level: _, iface_id: _ }
        );

        // Now that we're in suspend mode, any calls to SetPowerSaveMode should generate a metric
        // Set the power save mode
        let power_call_fut = match call {
            PowerCall::SetPowerSaveMode(val) => iface.set_power_save_mode(val),
            PowerCall::SetSuspendMode(val) => iface.set_suspend_mode(val),
        };
        let mut power_call_fut = pin!(power_call_fut);
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);
        // Respond to the call to set APF status
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::SetApfPacketFilterEnabled { enabled: _, responder }))) => {
                responder.send(Ok(())).expect("failed to send SetApfPacketFilterEnabled response");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);
        // Respond to the call to set power state
        assert_matches!(
            exec.run_until_stalled(&mut monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetPowerSaveMode { req: _, responder }))) => {
                responder.send(zx::sys::ZX_OK).expect("failed to send SetPowerSaveMode response");
            }
        );
        // Future completes
        exec.run_singlethreaded(power_call_fut).expect("future finished");

        // Check for the unclear power demand metric
        let event = assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => event);
        assert_matches!(
            event,
            TelemetryEvent::UnclearPowerDemand(
                wlan_telemetry::UnclearPowerDemand::PowerSaveRequestedWhileSuspendModeEnabled
            )
        );
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_update_power_level_sets_suspend_optimizations(suspend_mode: bool) {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (sme_proxy, mut sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let phy_id = rand::random();
        let (telemetry_sender, mut _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);

        // Create the interface with a power broker channel
        let iface = SmeClientIface::new(
            phy_id,
            TEST_IFACE_ID,
            sme_proxy,
            fidl::endpoints::create_proxy::<fidl_sme::TelemetryMarker>().0,
            monitor_svc,
            TelemetrySender::new(telemetry_sender),
        );

        // Simulate that a filter is installed so APF calls are not skipped in tests.
        exec.run_singlethreaded(async {
            let mut power_state = iface.power_state.lock().await;
            power_state.apf_filter_installed = true;
        });

        // Update the power level
        let level_to_set =
            if suspend_mode { StaIfacePowerLevel::Suspended } else { StaIfacePowerLevel::Normal };
        let power_call_fut = iface.update_power_level(level_to_set);
        let mut power_call_fut = pin!(power_call_fut);
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);

        // SME call to set APF enabled
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::SetApfPacketFilterEnabled { enabled, responder }))) => {
                assert_eq!(enabled, suspend_mode);
                responder.send(Ok(())).expect("failed to send SetApfPacketFilterEnabled response");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);
        // Respond to the call to set power state
        assert_matches!(
            exec.run_until_stalled(&mut monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetPowerSaveMode { req: _, responder }))) => {
                responder.send(zx::sys::ZX_OK).expect("failed to send SetPowerSaveMode response");
            }
        );

        // Future completes
        exec.run_singlethreaded(power_call_fut).expect("future finished");
    }

    #[fuchsia::test]
    fn test_update_power_level_skips_suspend_optimizations_when_no_filter() {
        let mut exec = fasync::TestExecutor::new();
        let (monitor_svc, mut monitor_stream) =
            create_proxy_and_stream::<fidl_device_service::DeviceMonitorMarker>();
        let (sme_proxy, mut sme_stream) = create_proxy_and_stream::<fidl_sme::ClientSmeMarker>();
        let phy_id = rand::random();
        let (telemetry_sender, mut _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);

        let iface = SmeClientIface::new(
            phy_id,
            TEST_IFACE_ID,
            sme_proxy,
            fidl::endpoints::create_proxy::<fidl_sme::TelemetryMarker>().0,
            monitor_svc,
            TelemetrySender::new(telemetry_sender),
        );

        let power_call_fut = iface.update_power_level(StaIfacePowerLevel::Suspended);
        let mut power_call_fut = pin!(power_call_fut);

        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Pending);
        // Respond to the call to set power state
        assert_matches!(
            exec.run_until_stalled(&mut monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_device_service::DeviceMonitorRequest::SetPowerSaveMode { req: _, responder }))) => {
                responder.send(zx::sys::ZX_OK).expect("failed to send SetPowerSaveMode response");
            }
        );

        assert_matches!(exec.run_until_stalled(&mut power_call_fut), Poll::Ready(Ok(())));
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);
    }

    #[fuchsia::test]
    fn test_reset_tx_power_scenario_succeeds() {
        let mut test_values = setup_test_manager();
        let test_phy_id = 123;

        // Attempt to reset the SAR scenario.
        let fut = test_values.manager.reset_tx_power_scenario(test_phy_id);
        let mut fut = pin!(fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify that the request has been passed on to the device monitor service.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::ResetTxPowerScenario {
                phy_id,
                responder })
            ) => {
                assert_eq!(phy_id, test_phy_id);
                responder.send(Ok(())).expect("failed to send device monitor response")
            }
        );

        // Verify that metric has been logged.
        assert_matches!(
            test_values
                .exec
                .run_until_stalled(&mut test_values.telemetry_receiver.select_next_some()),
            Poll::Ready(TelemetryEvent::ResetTxPowerScenario)
        );

        // Run the future to completion.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[test]
    fn test_reset_tx_power_scenario_fails() {
        let mut test_values = setup_test_manager();
        let test_phy_id = 123;

        // Attempt to reset the SAR scenario.
        let fut = test_values.manager.reset_tx_power_scenario(test_phy_id);
        let mut fut = pin!(fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify that the request has been passed on to the device monitor service and send back
        // an error.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::ResetTxPowerScenario {
                phy_id,
                responder })
            ) => {
                assert_eq!(phy_id, test_phy_id);
                responder.send(Err(zx::sys::ZX_ERR_NO_MEMORY)).expect("failed to send device monitor response")
            }
        );

        // Verify that metric has been logged.
        assert_matches!(
            test_values
                .exec
                .run_until_stalled(&mut test_values.telemetry_receiver.select_next_some()),
            Poll::Ready(TelemetryEvent::ResetTxPowerScenario)
        );

        // Run the future to completion.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[test_case(fidl_internal::TxPowerScenario::Default)]
    #[test_case(fidl_internal::TxPowerScenario::VoiceCall)]
    #[test_case(fidl_internal::TxPowerScenario::HeadCellOff)]
    #[test_case(fidl_internal::TxPowerScenario::HeadCellOn)]
    #[test_case(fidl_internal::TxPowerScenario::BodyCellOff)]
    #[test_case(fidl_internal::TxPowerScenario::BodyCellOn)]
    #[test_case(fidl_internal::TxPowerScenario::BodyBtActive)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_set_tx_power_scenario_succeeds(test_scenario: fidl_internal::TxPowerScenario) {
        let mut test_values = setup_test_manager();
        let test_phy_id = 123;

        // Attempt to reset the SAR scenario.
        let fut = test_values.manager.set_tx_power_scenario(test_phy_id, test_scenario);
        let mut fut = pin!(fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify that the request has been passed on to the device monitor service.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::SetTxPowerScenario {
                phy_id,
                scenario,
                responder })
            ) => {
                assert_eq!(phy_id, test_phy_id);
                assert_eq!(scenario, test_scenario);
                responder.send(Ok(())).expect("failed to send device monitor response")
            }
        );

        // Verify that metric has been logged.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.telemetry_receiver.select_next_some()),
            Poll::Ready(TelemetryEvent::SetTxPowerScenario { scenario }) => {
                assert_eq!(scenario, test_scenario);
            }
        );

        // Run the future to completion.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[test]
    fn test_set_tx_power_scenario_fails() {
        let mut test_values = setup_test_manager();
        let test_phy_id = 123;
        let test_scenario = fidl_internal::TxPowerScenario::Default;

        // Attempt to reset the SAR scenario.
        let fut = test_values.manager.set_tx_power_scenario(test_phy_id, test_scenario);
        let mut fut = pin!(fut);
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify that the request has been passed on to the device monitor service and send back a
        // failure.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::SetTxPowerScenario {
                phy_id,
                scenario,
                responder })
            ) => {
                assert_eq!(phy_id, test_phy_id);
                assert_eq!(scenario, test_scenario);
                responder.send(Err(zx::sys::ZX_ERR_NO_MEMORY)).expect("failed to send device monitor response")
            }
        );

        // Verify that metric has been logged.
        assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.telemetry_receiver.select_next_some()),
            Poll::Ready(TelemetryEvent::SetTxPowerScenario { scenario }) => {
                assert_eq!(scenario, test_scenario);
            }
        );

        // Run the future to completion.
        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[test]
    fn test_reset_phy() {
        let mut test_values = setup_test_manager();
        let phy_id = 123;
        let mut fut = test_values.manager.reset_phy(phy_id);

        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let (req_phy_id, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::Reset { phy_id, responder })) => (phy_id, responder));
        assert_eq!(req_phy_id, phy_id);
        responder.send(Ok(())).expect("Failed to respond to ResetPhy");

        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[test]
    fn test_reset_phy_failure() {
        let mut test_values = setup_test_manager();
        let phy_id = 123;
        let mut fut = test_values.manager.reset_phy(phy_id);

        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Pending);
        let (req_phy_id, responder) = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.monitor_stream.select_next_some()),
            Poll::Ready(Ok(fidl_device_service::DeviceMonitorRequest::Reset { phy_id, responder })) => (phy_id, responder));
        assert_eq!(req_phy_id, phy_id);
        responder
            .send(Err(zx::Status::INTERNAL.into_raw()))
            .expect("Failed to respond to ResetPhy");

        assert_matches!(test_values.exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[test]
    fn test_scan_successfully_matches_key_mgmt() {
        use wlan_common::test_utils::fake_stas::FakeProtectionCfg;

        let bss_scorer = BssScorer::new();
        let ssid = b"foo";

        let bss_open = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Open,
            ssid: Ssid::try_from(&ssid[..]).unwrap(),
            bssid: [1, 2, 3, 4, 5, 6],
        );
        let bss_wpa2 = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Wpa2,
            ssid: Ssid::try_from(&ssid[..]).unwrap(),
            bssid: [1, 2, 3, 4, 5, 7],
        );

        let scan_results = vec![
            fidl_sme::ScanResult {
                bss_description: bss_open.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Open],
                }),
                timestamp_nanos: 1,
            },
            fidl_sme::ScanResult {
                bss_description: bss_wpa2.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Wpa2Personal],
                }),
                timestamp_nanos: 1,
            },
        ];

        // Check filtering for Open
        let match_open = find_matching_network_in_scan(
            ssid,
            &None,
            &Some(fidl_wlanix::KeyMgmtMask::NONE),
            scan_results.clone(),
            &bss_scorer,
        );
        assert!(match_open.is_some());
        assert_eq!(match_open.unwrap().0.bssid, Bssid::from([1, 2, 3, 4, 5, 6]));

        // Check filtering for WPA2 (WPA_PSK)
        let match_wpa2 = find_matching_network_in_scan(
            ssid,
            &None,
            &Some(fidl_wlanix::KeyMgmtMask::WPA_PSK),
            scan_results.clone(),
            &bss_scorer,
        );
        assert!(match_wpa2.is_some());
        assert_eq!(match_wpa2.unwrap().0.bssid, Bssid::from([1, 2, 3, 4, 5, 7]));
    }

    #[test]
    fn test_find_matching_network_in_scan_fails_if_no_matching_key_mgmt() {
        use wlan_common::test_utils::fake_stas::FakeProtectionCfg;

        let bss_scorer = BssScorer::new();
        let ssid = b"foo";

        let bss_open = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Open,
            ssid: Ssid::try_from(&ssid[..]).unwrap(),
            bssid: [1, 2, 3, 4, 5, 6],
        );
        let bss_wpa2 = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Wpa2,
            ssid: Ssid::try_from(&ssid[..]).unwrap(),
            bssid: [1, 2, 3, 4, 5, 7],
        );

        let scan_results = vec![
            fidl_sme::ScanResult {
                bss_description: bss_open.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Open],
                }),
                timestamp_nanos: 1,
            },
            fidl_sme::ScanResult {
                bss_description: bss_wpa2.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Wpa2Personal],
                }),
                timestamp_nanos: 1,
            },
        ];

        // Check filtering for WPA3 (SAE) returns None when only Open and WPA2 are present
        let match_wpa3 = find_matching_network_in_scan(
            ssid,
            &None,
            &Some(fidl_wlanix::KeyMgmtMask::SAE),
            scan_results,
            &bss_scorer,
        );
        assert!(match_wpa3.is_none());
    }

    #[test]
    fn test_find_matching_networks_in_scan_with_multiple_key_mgmt() {
        // This tests that if multiple BSS's match different bits in the key management mask,
        // both can be returned as matching if they have the highest score.
        use wlan_common::test_utils::fake_stas::FakeProtectionCfg;

        let bss_scorer = BssScorer::new();
        let ssid = b"foo";

        let bss_wpa2 = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Wpa2,
            ssid: Ssid::try_from(&ssid[..]).unwrap(),
            bssid: [1, 2, 3, 4, 5, 7],
            rssi_dbm: -30,
        );
        let mut bss_wpa3 = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Wpa3,
            ssid: Ssid::try_from(&ssid[..]).unwrap(),
            bssid: [1, 2, 3, 4, 5, 8],
            rssi_dbm: -20,
        );

        let scan_results = vec![
            fidl_sme::ScanResult {
                bss_description: bss_wpa2.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Wpa2Personal],
                }),
                timestamp_nanos: 1,
            },
            fidl_sme::ScanResult {
                bss_description: bss_wpa3.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Wpa3Personal],
                }),
                timestamp_nanos: 1,
            },
        ];

        // Check filtering with a mask of multiple bits
        let multiple_mask = fidl_wlanix::KeyMgmtMask::WPA_PSK | fidl_wlanix::KeyMgmtMask::SAE;

        let (bss, _compatible) = find_matching_network_in_scan(
            ssid,
            &None,
            &Some(multiple_mask),
            scan_results.clone(),
            &bss_scorer,
        )
        .expect("Failed to find any matching BSS");
        assert_eq!(bss.bssid, bss_wpa3.bssid.into());
        assert_eq!(bss.protection(), bss::Protection::Wpa3Personal);

        // Modify the RSSI's on the scan results so that the WPA3 network has weak signal.
        // Now the WPA2 network should be selected.
        bss_wpa3.rssi_dbm = -75;
        let scan_results = vec![
            fidl_sme::ScanResult {
                bss_description: bss_wpa2.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Wpa2Personal],
                }),
                timestamp_nanos: 1,
            },
            fidl_sme::ScanResult {
                bss_description: bss_wpa3.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_security::Protocol::Wpa3Personal],
                }),
                timestamp_nanos: 1,
            },
        ];
        let (bss, _compatible) = find_matching_network_in_scan(
            ssid,
            &None,
            &Some(multiple_mask),
            scan_results.clone(),
            &bss_scorer,
        )
        .expect("Failed to find any matching BSS");
        assert_eq!(bss.bssid, bss_wpa2.bssid.into());
        assert_eq!(bss.protection(), bss::Protection::Wpa2Personal);
    }

    #[fuchsia::test]
    fn test_connect_to_network_respects_key_mgmt() {
        use wlan_common::test_utils::fake_stas::FakeProtectionCfg;

        let mut test_values = setup_test_manager_with_iface();

        // AP supports WPA2 and WPA3 Personal mixed mode.
        let bss_description = fake_fidl_bss_description!(
            protection => FakeProtectionCfg::Wpa2Wpa3,
            ssid: Ssid::try_from("foo").unwrap(),
            bssid: [1, 2, 3, 4, 5, 6],
            rssi_dbm: -30,
        );

        *test_values.iface.last_scan_results.lock() = Some(LastScanResults::new(
            fasync::BootInstant::now(),
            vec![fidl_sme::ScanResult {
                bss_description: bss_description.clone(),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![
                        fidl_security::Protocol::Wpa2Personal,
                        fidl_security::Protocol::Wpa3Personal,
                    ],
                }),
                timestamp_nanos: 1,
            }],
        ));

        // Client explicitly restricts connection to WPA2 Personal (WPA_PSK) only.
        let key_mgmt = Some(fidl_wlanix::KeyMgmtMask::WPA_PSK);
        let credential = Credential::Password(b"password".to_vec());

        let mut connect_fut =
            test_values.iface.connect_to_network(b"foo", credential, None, key_mgmt);
        assert_matches!(test_values.exec.run_until_stalled(&mut connect_fut), Poll::Pending);

        let req = assert_matches!(
            test_values.exec.run_until_stalled(&mut test_values.sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Connect { req, .. }))) => req
        );

        // Wlanix must request WPA2 Personal (not WPA3 Personal) because of the key_mgmt restriction.
        assert_eq!(req.authentication.protocol, fidl_security::Protocol::Wpa2Personal);
    }
}
