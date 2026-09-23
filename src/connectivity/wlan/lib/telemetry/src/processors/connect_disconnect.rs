// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::DeviceMobility;
use crate::convert::{
    convert_channel_band, convert_is_owe_transition, convert_rssi_bucket, convert_security_type,
    convert_snr_bucket,
};
use crate::processors::toggle_events::ClientConnectionsToggleEvent;
use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use derivative::Derivative;
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async as fasync;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::id_enum::IdEnum;
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::nodes::{BoundedListNode, LruCacheNode};
use fuchsia_inspect_derive::Unit;
use fuchsia_sync::Mutex;
use ieee80211::OuiFmt;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use strum_macros::{Display, EnumCount};
use windowed_stats::experimental::inspect::{InspectSender, InspectedTimeMatrix};
use windowed_stats::experimental::series::interpolation::{ConstantSample, LastSample};
use windowed_stats::experimental::series::metadata::{BitsetMap, BitsetNode};
use windowed_stats::experimental::series::statistic::Union;
use windowed_stats::experimental::series::{SamplingProfile, TimeMatrix};
use wlan_common::bss::BssDescription;
use wlan_common::channel::Channel;
use wlan_legacy_metrics_registry as metrics;
use zx;

const INSPECT_CONNECT_EVENTS_LIMIT: usize = 10;
const INSPECT_DISCONNECT_EVENTS_LIMIT: usize = 20;
const INSPECT_CONNECT_ATTEMPT_RESULTS_LIMIT: usize = 50;
const INSPECT_CONNECTED_NETWORKS_ID_LIMIT: usize = 16;
const INSPECT_DISCONNECT_SOURCES_ID_LIMIT: usize = 32;
const INSPECT_CONNECT_ATTEMPT_RESULTS_ID_LIMIT: usize = 32;
const SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_TIMEOUT: zx::BootDuration =
    zx::BootDuration::from_minutes(2);
const DAILY_METRICS_LOG_INTERVAL: zx::BootDuration = zx::BootDuration::from_hours(24);

#[derive(Clone, Debug, Display, EnumCount)]
enum ConnectionState {
    Idle(IdleState),
    Connected(ConnectedState),
    Disconnected(DisconnectedState),
    ConnectFailed(ConnectFailedState),
    FailedToStart(FailedToStartState),
    FailedToStop(FailedToStopState),
    PnoScanFailedIdle(PnoScanFailedIdleState),
}

// Update the ConnectDisconnectTimeSeries BitsetMap when making changes to this enum.
impl IdEnum for ConnectionState {
    type Id = u8;
    fn to_id(&self) -> Self::Id {
        match self {
            Self::Idle(_) => 0,
            Self::Disconnected(_) => 1,
            Self::ConnectFailed(_) => 2,
            Self::Connected(_) => 3,
            Self::FailedToStart(_) => 4,
            Self::FailedToStop(_) => 5,
            Self::PnoScanFailedIdle(_) => 6,
        }
    }
}

#[derive(Clone, Debug)]
struct IdleState {}

#[derive(Clone, Debug, PartialEq)]
struct ConnectedState {
    bss: Box<BssDescription>,
    is_owe_transition: bool,
}

#[derive(Clone, Debug)]
struct DisconnectedState {}

#[derive(Clone, Debug)]
struct ConnectFailedState {}

#[derive(Clone, Debug)]
struct FailedToStartState {}

#[derive(Clone, Debug)]
struct FailedToStopState {}

#[derive(Clone, Debug)]
struct PnoScanFailedIdleState {}

#[derive(Derivative, Unit)]
#[derivative(PartialEq, Eq, Hash)]
struct InspectConnectedNetwork {
    bssid: String,
    ssid: String,
    protection: String,
    ht_cap: Option<Vec<u8>>,
    vht_cap: Option<Vec<u8>>,
    #[derivative(PartialEq = "ignore")]
    #[derivative(Hash = "ignore")]
    wsc: Option<InspectNetworkWsc>,
    is_wmm_assoc: bool,
    wmm_param: Option<Vec<u8>>,
}

impl From<&BssDescription> for InspectConnectedNetwork {
    fn from(bss_description: &BssDescription) -> Self {
        Self {
            bssid: bss_description.bssid.to_string(),
            ssid: bss_description.ssid.to_string(),
            protection: format!("{:?}", bss_description.protection()),
            ht_cap: bss_description.raw_ht_cap().map(|cap| cap.bytes.into()),
            vht_cap: bss_description.raw_vht_cap().map(|cap| cap.bytes.into()),
            wsc: bss_description.probe_resp_wsc().as_ref().map(InspectNetworkWsc::from),
            is_wmm_assoc: bss_description.find_wmm_param().is_some(),
            wmm_param: bss_description.find_wmm_param().map(|bytes| bytes.into()),
        }
    }
}

#[derive(PartialEq, Unit, Hash)]
struct InspectNetworkWsc {
    device_name: String,
    manufacturer: String,
    model_name: String,
    model_number: String,
}

impl From<&wlan_common::ie::wsc::ProbeRespWsc> for InspectNetworkWsc {
    fn from(wsc: &wlan_common::ie::wsc::ProbeRespWsc) -> Self {
        Self {
            device_name: String::from_utf8_lossy(&wsc.device_name[..]).to_string(),
            manufacturer: String::from_utf8_lossy(&wsc.manufacturer[..]).to_string(),
            model_name: String::from_utf8_lossy(&wsc.model_name[..]).to_string(),
            model_number: String::from_utf8_lossy(&wsc.model_number[..]).to_string(),
        }
    }
}

#[derive(PartialEq, Eq, Unit, Hash)]
struct InspectConnectAttemptResult {
    status_code: u16,
    result: String,
}

#[derive(PartialEq, Eq, Unit, Hash)]
struct InspectDisconnectSource {
    source: String,
    reason: String,
    mlme_event_name: Option<String>,
}

impl From<&fidl_sme::DisconnectSource> for InspectDisconnectSource {
    fn from(disconnect_source: &fidl_sme::DisconnectSource) -> Self {
        match disconnect_source {
            fidl_sme::DisconnectSource::User(reason) => Self {
                source: "user".to_string(),
                reason: format!("{reason:?}"),
                mlme_event_name: None,
            },
            fidl_sme::DisconnectSource::Ap(cause) => Self {
                source: "ap".to_string(),
                reason: format!("{:?}", cause.reason_code),
                mlme_event_name: Some(format!("{:?}", cause.mlme_event_name)),
            },
            fidl_sme::DisconnectSource::Mlme(cause) => Self {
                source: "mlme".to_string(),
                reason: format!("{:?}", cause.reason_code),
                mlme_event_name: Some(format!("{:?}", cause.mlme_event_name)),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisconnectInfo {
    pub iface_id: u16,
    pub connected_duration: zx::BootDuration,
    pub is_sme_reconnecting: bool,
    pub disconnect_source: fidl_sme::DisconnectSource,
    pub original_bss_desc: Box<BssDescription>,
    pub current_rssi_dbm: i8,
    pub current_snr_db: i8,
    pub current_channel: Channel,
}

pub struct ConnectDisconnectLogger {
    connection_state: Arc<Mutex<ConnectionState>>,
    cobalt_proxy: Arc<FilteredCobaltLogger>,
    connect_events_node: Mutex<BoundedListNode>,
    disconnect_events_node: Mutex<BoundedListNode>,
    connect_attempt_results_node: Mutex<BoundedListNode>,
    inspect_metadata_node: Mutex<InspectMetadataNode>,
    time_series_stats: ConnectDisconnectTimeSeries,
    successive_connect_attempt_failures: AtomicUsize,
    last_connect_failure_at: Arc<Mutex<Option<fasync::BootInstant>>>,
    last_disconnect_at: Arc<Mutex<Option<fasync::MonotonicInstant>>>,
    daily_connect_stats: Mutex<DailyConnectStats>,
    device_mobility: DeviceMobility,
}

impl ConnectDisconnectLogger {
    pub fn new<S: InspectSender>(
        cobalt_proxy: Arc<FilteredCobaltLogger>,
        inspect_node: &InspectNode,
        inspect_metadata_node: &InspectNode,
        inspect_metadata_path: &str,
        time_matrix_client: &S,
        device_mobility: DeviceMobility,
    ) -> Self {
        let connect_events = inspect_node.create_child("connect_events");
        let disconnect_events = inspect_node.create_child("disconnect_events");
        let connect_attempt_results = inspect_node.create_child("connect_attempt_results");
        let this = Self {
            cobalt_proxy,
            connection_state: Arc::new(Mutex::new(ConnectionState::Idle(IdleState {}))),
            connect_events_node: Mutex::new(BoundedListNode::new(
                connect_events,
                INSPECT_CONNECT_EVENTS_LIMIT,
            )),
            disconnect_events_node: Mutex::new(BoundedListNode::new(
                disconnect_events,
                INSPECT_DISCONNECT_EVENTS_LIMIT,
            )),
            connect_attempt_results_node: Mutex::new(BoundedListNode::new(
                connect_attempt_results,
                INSPECT_CONNECT_ATTEMPT_RESULTS_LIMIT,
            )),
            inspect_metadata_node: Mutex::new(InspectMetadataNode::new(inspect_metadata_node)),
            time_series_stats: ConnectDisconnectTimeSeries::new(
                time_matrix_client,
                inspect_metadata_path,
            ),
            successive_connect_attempt_failures: AtomicUsize::new(0),
            last_connect_failure_at: Arc::new(Mutex::new(None)),
            last_disconnect_at: Arc::new(Mutex::new(None)),
            daily_connect_stats: Mutex::new(DailyConnectStats::new(fasync::BootInstant::now())),
            device_mobility,
        };
        this.log_connection_state();
        this
    }

    fn update_connection_state(&self, state: ConnectionState) {
        *self.connection_state.lock() = state;
        self.log_connection_state();
    }

    fn log_connection_state(&self) {
        let wlan_connectivity_state_id = self.connection_state.lock().to_id() as u64;
        self.time_series_stats.log_wlan_connectivity_state(1 << wlan_connectivity_state_id);
    }

    pub fn is_connected(&self) -> bool {
        matches!(*self.connection_state.lock(), ConnectionState::Connected(_))
    }

    pub async fn handle_connect_attempt(
        &self,
        result: fidl_ieee80211::StatusCode,
        bss: &BssDescription,
        is_credential_rejected: bool,
        is_owe_transition: bool,
    ) {
        let mut flushed_successive_failures = None;
        let mut downtime_duration = None;
        if result == fidl_ieee80211::StatusCode::Success {
            self.update_connection_state(ConnectionState::Connected(ConnectedState {
                bss: Box::new(bss.clone()),
                is_owe_transition,
            }));
            flushed_successive_failures =
                Some(self.successive_connect_attempt_failures.swap(0, Ordering::SeqCst));
            downtime_duration =
                self.last_disconnect_at.lock().map(|t| fasync::MonotonicInstant::now() - t);
        } else if is_credential_rejected {
            self.update_connection_state(ConnectionState::Idle(IdleState {}));
            let _prev = self.successive_connect_attempt_failures.fetch_add(1, Ordering::SeqCst);
            let _prev = self.last_connect_failure_at.lock().replace(fasync::BootInstant::now());
        } else {
            self.update_connection_state(ConnectionState::ConnectFailed(ConnectFailedState {}));
            let _prev = self.successive_connect_attempt_failures.fetch_add(1, Ordering::SeqCst);
            let _prev = self.last_connect_failure_at.lock().replace(fasync::BootInstant::now());
        }

        self.log_connect_attempt_inspect(result, bss);
        self.log_connect_attempt_cobalt(result, flushed_successive_failures, downtime_duration)
            .await;
        if result == fidl_ieee80211::StatusCode::Success {
            self.log_device_connected_cobalt_metrics(bss, is_owe_transition).await;
        }

        let security_type = convert_security_type(&bss.protection());
        let primary_channel = bss.channel.primary;
        let channel_band = convert_channel_band(bss.channel.band);
        let rssi_bucket = convert_rssi_bucket(bss.rssi_dbm);
        let snr_bucket = convert_snr_bucket(bss.snr_db);
        let is_owe_transition_dim = convert_is_owe_transition(is_owe_transition);

        let mut daily_stats = self.daily_connect_stats.lock();
        daily_stats.connect_per_security_type.entry(security_type).or_default().increment(result);
        daily_stats
            .connect_per_primary_channel
            .entry(primary_channel)
            .or_default()
            .increment(result);
        daily_stats.connect_per_channel_band.entry(channel_band).or_default().increment(result);
        daily_stats.connect_per_rssi_bucket.entry(rssi_bucket).or_default().increment(result);
        daily_stats.connect_per_snr_bucket.entry(snr_bucket).or_default().increment(result);
        daily_stats
            .connect_per_is_owe_transition
            .entry(is_owe_transition_dim)
            .or_default()
            .increment(result);
    }

    fn log_connect_attempt_inspect(
        &self,
        result: fidl_ieee80211::StatusCode,
        bss: &BssDescription,
    ) {
        let mut inspect_metadata_node = self.inspect_metadata_node.lock();
        let connect_result_id =
            inspect_metadata_node.connect_attempt_results.insert(InspectConnectAttemptResult {
                status_code: result.into_primitive(),
                result: format!("{:?}", result),
            }) as u64;
        self.time_series_stats.log_connect_attempt_results(1 << connect_result_id);

        inspect_log!(self.connect_attempt_results_node.lock(), {
            result: format!("{:?}", result),
            ssid: bss.ssid.to_string(),
            bssid: bss.bssid.to_string(),
            protection: format!("{:?}", bss.protection()),
        });

        if result == fidl_ieee80211::StatusCode::Success {
            let connected_network = InspectConnectedNetwork::from(bss);
            let connected_network_id =
                inspect_metadata_node.connected_networks.insert(connected_network) as u64;

            self.time_series_stats.log_connected_networks(1 << connected_network_id);

            inspect_log!(self.connect_events_node.lock(), {
                network_id: connected_network_id,
            });
        }
    }

    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    async fn log_connect_attempt_cobalt(
        &self,
        result: fidl_ieee80211::StatusCode,
        flushed_successive_failures: Option<usize>,
        downtime_duration: Option<zx::MonotonicDuration>,
    ) {
        let mut metric_events = vec![];
        metric_events.push(MetricEvent {
            metric_id: metrics::CONNECT_ATTEMPT_BREAKDOWN_BY_STATUS_CODE_METRIC_ID,
            event_codes: vec![result.into_primitive() as u32],
            payload: MetricEventPayload::Count(1),
        });

        if let Some(failures) = flushed_successive_failures {
            metric_events.push(MetricEvent {
                metric_id: metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::IntegerValue(failures as i64),
            });
        }

        if let Some(duration) = downtime_duration {
            metric_events.push(MetricEvent {
                metric_id: metrics::DOWNTIME_POST_DISCONNECT_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::IntegerValue(duration.into_millis()),
            });
        }

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "log_connect_attempt_cobalt");
    }

    async fn log_device_connected_cobalt_metrics(
        &self,
        bss: &BssDescription,
        is_owe_transition: bool,
    ) {
        let mut metric_events = vec![];
        append_device_connected_cobalt_metrics(&mut metric_events, bss, is_owe_transition);
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "log_device_connected_cobalt_metrics");
    }

    pub async fn handle_channel_switched(&self, channel: Channel) {
        if let ConnectionState::Connected(ref mut state) = *self.connection_state.lock() {
            state.bss.channel = channel;
        }
        let mut metric_events = vec![];
        append_device_connected_channel_cobalt_metrics(&mut metric_events, channel);
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_channel_switched");
    }

    pub async fn log_disconnect(&self, info: &DisconnectInfo) {
        match self.device_mobility {
            DeviceMobility::Mobile => {
                // Mobile devices can be considered idle if they disconnect for reasons associated
                // with going out of range or are commanded to disconnect by upper layers.
                if !info.disconnect_source.should_log_for_mobile_device() {
                    self.update_connection_state(ConnectionState::Idle(IdleState {}));
                } else {
                    self.update_connection_state(ConnectionState::Disconnected(
                        DisconnectedState {},
                    ));
                }
            }
            DeviceMobility::Stationary => {
                self.update_connection_state(ConnectionState::Disconnected(DisconnectedState {}));
            }
        }
        let _prev = self.last_disconnect_at.lock().replace(fasync::MonotonicInstant::now());
        self.log_disconnect_inspect(info);
        self.log_disconnect_cobalt(info).await;
    }

    fn log_disconnect_inspect(&self, info: &DisconnectInfo) {
        let mut inspect_metadata_node = self.inspect_metadata_node.lock();
        let connected_network = InspectConnectedNetwork::from(&*info.original_bss_desc);
        let connected_network_id =
            inspect_metadata_node.connected_networks.insert(connected_network) as u64;
        let disconnect_source = InspectDisconnectSource::from(&info.disconnect_source);
        let disconnect_source_id =
            inspect_metadata_node.disconnect_sources.insert(disconnect_source) as u64;
        inspect_log!(self.disconnect_events_node.lock(), {
            connected_duration: info.connected_duration.into_nanos(),
            disconnect_source_id: disconnect_source_id,
            network_id: connected_network_id,
            rssi_dbm: info.current_rssi_dbm,
            snr_db: info.current_snr_db,
            channel: format!("{}", info.current_channel),
        });

        self.time_series_stats.log_disconnected_networks(1 << connected_network_id);
        self.time_series_stats.log_disconnect_sources(1 << disconnect_source_id);
    }

    async fn log_disconnect_cobalt(&self, info: &DisconnectInfo) {
        let mut metric_events = vec![];
        metric_events.push(MetricEvent {
            metric_id: metrics::TOTAL_DISCONNECT_COUNT_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        });

        if self.device_mobility == DeviceMobility::Mobile
            && info.disconnect_source.should_log_for_mobile_device()
        {
            metric_events.push(MetricEvent {
                metric_id: metrics::DISCONNECT_OCCURRENCE_FOR_MOBILE_DEVICE_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::Count(1),
            });
        }

        metric_events.push(MetricEvent {
            metric_id: metrics::CONNECTED_DURATION_ON_DISCONNECT_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::IntegerValue(info.connected_duration.into_millis()),
        });

        metric_events.push(MetricEvent {
            metric_id: metrics::DISCONNECT_BREAKDOWN_BY_REASON_CODE_METRIC_ID,
            event_codes: vec![
                u32::from(info.disconnect_source.cobalt_reason_code()),
                info.disconnect_source.as_cobalt_disconnect_source() as u32,
            ],
            payload: MetricEventPayload::Count(1),
        });

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "log_disconnect_cobalt");
    }

    pub async fn handle_periodic_telemetry(&self) {
        let mut metric_events = vec![];
        let now = fasync::BootInstant::now();
        if let Some(failed_at) = *self.last_connect_failure_at.lock()
            && now - failed_at >= SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_TIMEOUT
        {
            let failures = self.successive_connect_attempt_failures.swap(0, Ordering::SeqCst);
            if failures > 0 {
                metric_events.push(MetricEvent {
                    metric_id: metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID,
                    event_codes: vec![],
                    payload: MetricEventPayload::IntegerValue(failures as i64),
                });
            }
        }

        {
            let mut daily_stats = self.daily_connect_stats.lock();
            if now - daily_stats.last_log_time >= DAILY_METRICS_LOG_INTERVAL {
                if let ConnectionState::Connected(ref state) = *self.connection_state.lock() {
                    append_device_connected_cobalt_metrics(
                        &mut metric_events,
                        &state.bss,
                        state.is_owe_transition,
                    );
                }

                for (security_type, counter) in daily_stats.connect_per_security_type.drain() {
                    if counter.total > 0 {
                        let success_rate = counter.success as f64 / counter.total as f64;
                        metric_events.push(MetricEvent {
                            metric_id:
                                metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_SECURITY_TYPE_METRIC_ID,
                            event_codes: vec![security_type as u32],
                            payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(
                                success_rate,
                            )),
                        });
                    }
                }
                for (primary_channel, counter) in daily_stats.connect_per_primary_channel.drain() {
                    if counter.total > 0 {
                        let success_rate = counter.success as f64 / counter.total as f64;
                        metric_events.push(MetricEvent {
                            metric_id:
                                metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_PRIMARY_CHANNEL_METRIC_ID,
                            event_codes: vec![primary_channel as u32],
                            payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(
                                success_rate,
                            )),
                        });
                    }
                }
                for (channel_band, counter) in daily_stats.connect_per_channel_band.drain() {
                    if counter.total > 0 {
                        let success_rate = counter.success as f64 / counter.total as f64;
                        metric_events.push(MetricEvent {
                            metric_id:
                                metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_CHANNEL_BAND_METRIC_ID,
                            event_codes: vec![channel_band as u32],
                            payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(
                                success_rate,
                            )),
                        });
                    }
                }
                for (rssi_bucket, counter) in daily_stats.connect_per_rssi_bucket.drain() {
                    if counter.total > 0 {
                        let success_rate = counter.success as f64 / counter.total as f64;
                        metric_events.push(MetricEvent {
                            metric_id:
                                metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_RSSI_BUCKET_METRIC_ID,
                            event_codes: vec![rssi_bucket as u32],
                            payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(
                                success_rate,
                            )),
                        });
                    }
                }
                for (snr_bucket, counter) in daily_stats.connect_per_snr_bucket.drain() {
                    if counter.total > 0 {
                        let success_rate = counter.success as f64 / counter.total as f64;
                        metric_events.push(MetricEvent {
                            metric_id:
                                metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_SNR_BUCKET_METRIC_ID,
                            event_codes: vec![snr_bucket as u32],
                            payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(
                                success_rate,
                            )),
                        });
                    }
                }
                for (is_owe_transition, counter) in
                    daily_stats.connect_per_is_owe_transition.drain()
                {
                    if counter.total > 0 {
                        let success_rate = counter.success as f64 / counter.total as f64;
                        metric_events.push(MetricEvent {
                            metric_id:
                                metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_IS_OWE_TRANSITION_METRIC_ID,
                            event_codes: vec![is_owe_transition as u32],
                            payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(
                                success_rate,
                            )),
                        });
                    }
                }
                daily_stats.last_log_time = now;
            }
        }

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_periodic_telemetry");
    }

    pub async fn handle_suspend_imminent(&self) {
        let mut metric_events = vec![];

        let flushed_successive_failures =
            self.successive_connect_attempt_failures.swap(0, Ordering::SeqCst);
        if flushed_successive_failures > 0 {
            metric_events.push(MetricEvent {
                metric_id: metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::IntegerValue(flushed_successive_failures as i64),
            });
        }

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_suspend_imminent");
    }

    pub async fn handle_iface_destroyed(&self) {
        self.update_connection_state(ConnectionState::Idle(IdleState {}));
    }

    pub async fn handle_client_connections_toggle(&self, event: &ClientConnectionsToggleEvent) {
        if event == &ClientConnectionsToggleEvent::Disabled {
            self.update_connection_state(ConnectionState::Idle(IdleState {}));
        }
    }

    pub async fn handle_pno_scan_failure(&self) {
        let mut metric_events = vec![MetricEvent {
            metric_id: metrics::PNO_SCAN_FAILURE_OCCURRENCE_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];

        let state = self.connection_state.lock().clone();
        match state {
            ConnectionState::Idle(_)
            | ConnectionState::Disconnected(_)
            | ConnectionState::ConnectFailed(_)
            | ConnectionState::PnoScanFailedIdle(_) => {
                metric_events.push(MetricEvent {
                    metric_id: metrics::PNO_SCAN_FAILURE_WHILE_NOT_CONNECTED_OCCURRENCE_METRIC_ID,
                    event_codes: vec![],
                    payload: MetricEventPayload::Count(1),
                });

                // PNO scan failures while not connected indicate that the system is looking for
                // networks to connect to but it is unable to.  In this case, we should transition
                // to the PnoScanFailedIdle state to flag a period of potential connectivity loss.
                self.update_connection_state(ConnectionState::PnoScanFailedIdle(
                    PnoScanFailedIdleState {},
                ));
            }
            ConnectionState::Connected(_)
            | ConnectionState::FailedToStart(_)
            | ConnectionState::FailedToStop(_) => {
                // PNO scan failures while connected will not affect the current connectivity state.
                // If WLAN has already failed to start or failed to stop, the state should remain
                // unchanged until a different failure or successful connection occurs.
            }
        }

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_pno_scan_failure");
    }
    pub async fn handle_client_connections_failed_to_start(&self) {
        self.update_connection_state(ConnectionState::FailedToStart(FailedToStartState {}));
    }

    pub async fn handle_client_connections_failed_to_stop(&self) {
        self.update_connection_state(ConnectionState::FailedToStop(FailedToStopState {}));
    }
}

struct InspectMetadataNode {
    connected_networks: LruCacheNode<InspectConnectedNetwork>,
    disconnect_sources: LruCacheNode<InspectDisconnectSource>,
    connect_attempt_results: LruCacheNode<InspectConnectAttemptResult>,
}

impl InspectMetadataNode {
    const CONNECTED_NETWORKS: &'static str = "connected_networks";
    const DISCONNECT_SOURCES: &'static str = "disconnect_sources";
    const CONNECT_ATTEMPT_RESULTS: &'static str = "connect_attempt_results";

    fn new(inspect_node: &InspectNode) -> Self {
        let connected_networks = inspect_node.create_child(Self::CONNECTED_NETWORKS);
        let disconnect_sources = inspect_node.create_child(Self::DISCONNECT_SOURCES);
        let connect_attempt_results = inspect_node.create_child(Self::CONNECT_ATTEMPT_RESULTS);
        Self {
            connected_networks: LruCacheNode::new(
                connected_networks,
                INSPECT_CONNECTED_NETWORKS_ID_LIMIT,
            ),
            disconnect_sources: LruCacheNode::new(
                disconnect_sources,
                INSPECT_DISCONNECT_SOURCES_ID_LIMIT,
            ),
            connect_attempt_results: LruCacheNode::new(
                connect_attempt_results,
                INSPECT_CONNECT_ATTEMPT_RESULTS_ID_LIMIT,
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct ConnectDisconnectTimeSeries {
    wlan_connectivity_states: InspectedTimeMatrix<u64>,
    connected_networks: InspectedTimeMatrix<u64>,
    disconnected_networks: InspectedTimeMatrix<u64>,
    disconnect_sources: InspectedTimeMatrix<u64>,
    connect_attempt_results: InspectedTimeMatrix<u64>,
}

impl ConnectDisconnectTimeSeries {
    pub fn new<S: InspectSender>(client: &S, inspect_metadata_path: &str) -> Self {
        let wlan_connectivity_states = client.inspect_time_matrix_with_metadata(
            "wlan_connectivity_states",
            TimeMatrix::<Union<u64>, LastSample>::new(
                SamplingProfile::highly_granular(),
                LastSample::or(0),
            ),
            // Update the ConnectionState IdEnum trait when making changes to this list.
            BitsetMap::from_ordered(Self::wlan_connectivity_states_bitset_map().iter().copied()),
        );
        let connected_networks = client.inspect_time_matrix_with_metadata(
            "connected_networks",
            TimeMatrix::<Union<u64>, ConstantSample>::new(
                SamplingProfile::granular(),
                ConstantSample::default(),
            ),
            BitsetNode::from_path(format!(
                "{}/{}",
                inspect_metadata_path,
                InspectMetadataNode::CONNECTED_NETWORKS
            )),
        );
        let disconnected_networks = client.inspect_time_matrix_with_metadata(
            "disconnected_networks",
            TimeMatrix::<Union<u64>, ConstantSample>::new(
                SamplingProfile::granular(),
                ConstantSample::default(),
            ),
            // This time matrix shares its bit labels with `connected_networks`.
            BitsetNode::from_path(format!(
                "{}/{}",
                inspect_metadata_path,
                InspectMetadataNode::CONNECTED_NETWORKS
            )),
        );
        let disconnect_sources = client.inspect_time_matrix_with_metadata(
            "disconnect_sources",
            TimeMatrix::<Union<u64>, ConstantSample>::new(
                SamplingProfile::granular(),
                ConstantSample::default(),
            ),
            BitsetNode::from_path(format!(
                "{}/{}",
                inspect_metadata_path,
                InspectMetadataNode::DISCONNECT_SOURCES,
            )),
        );
        let connect_attempt_results = client.inspect_time_matrix_with_metadata(
            "connect_attempt_results",
            TimeMatrix::<Union<u64>, ConstantSample>::new(
                SamplingProfile::granular(),
                ConstantSample::default(),
            ),
            BitsetNode::from_path(format!(
                "{}/{}",
                inspect_metadata_path,
                InspectMetadataNode::CONNECT_ATTEMPT_RESULTS,
            )),
        );
        Self {
            wlan_connectivity_states,
            connected_networks,
            disconnected_networks,
            disconnect_sources,
            connect_attempt_results,
        }
    }

    // TODO(https://fxbug.dev/504712259): Update BitsetMap to accept the enum type
    // it's associated with rather than constructing bit labels separately like this
    fn wlan_connectivity_states_bitset_map() -> &'static [&'static str] {
        &[
            "idle",
            "disconnected",
            "connect_failed",
            "connected",
            "start_failure",
            "stop_failure",
            "pno_scan_failed",
        ]
    }

    fn log_wlan_connectivity_state(&self, data: u64) {
        self.wlan_connectivity_states.fold_or_log_error(data);
    }
    fn log_connected_networks(&self, data: u64) {
        self.connected_networks.fold_or_log_error(data);
    }
    fn log_disconnected_networks(&self, data: u64) {
        self.disconnected_networks.fold_or_log_error(data);
    }
    fn log_disconnect_sources(&self, data: u64) {
        self.disconnect_sources.fold_or_log_error(data);
    }
    fn log_connect_attempt_results(&self, data: u64) {
        self.connect_attempt_results.fold_or_log_error(data);
    }
}

pub trait DisconnectSourceExt {
    fn should_log_for_mobile_device(&self) -> bool;
    fn cobalt_reason_code(&self) -> u16;
    fn as_cobalt_disconnect_source(
        &self,
    ) -> metrics::ConnectivityWlanMetricDimensionDisconnectSource;
}

impl DisconnectSourceExt for fidl_sme::DisconnectSource {
    fn should_log_for_mobile_device(&self) -> bool {
        match self {
            fidl_sme::DisconnectSource::Ap(_) => true,
            fidl_sme::DisconnectSource::Mlme(cause)
                if cause.reason_code != fidl_ieee80211::ReasonCode::MlmeLinkFailed =>
            {
                true
            }
            _ => false,
        }
    }

    fn cobalt_reason_code(&self) -> u16 {
        let cobalt_disconnect_reason_code = match self {
            fidl_sme::DisconnectSource::Ap(cause) | fidl_sme::DisconnectSource::Mlme(cause) => {
                cause.reason_code.into_primitive()
            }
            fidl_sme::DisconnectSource::User(reason) => *reason as u16,
        };
        // This `max_event_code: 1000` is set in the metrics registry, but doesn't show up in the
        // generated bindings.
        const REASON_CODE_MAX: u16 = 1000;
        std::cmp::min(cobalt_disconnect_reason_code, REASON_CODE_MAX)
    }

    fn as_cobalt_disconnect_source(
        &self,
    ) -> metrics::ConnectivityWlanMetricDimensionDisconnectSource {
        use metrics::ConnectivityWlanMetricDimensionDisconnectSource as DS;
        match self {
            fidl_sme::DisconnectSource::Ap(..) => DS::Ap,
            fidl_sme::DisconnectSource::User(..) => DS::User,
            fidl_sme::DisconnectSource::Mlme(..) => DS::Mlme,
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq)]
struct ConnectAttemptsCounter {
    success: u64,
    total: u64,
}

impl ConnectAttemptsCounter {
    fn increment(&mut self, code: fidl_ieee80211::StatusCode) {
        self.total += 1;
        if code == fidl_ieee80211::StatusCode::Success {
            self.success += 1;
        }
    }
}

struct DailyConnectStats {
    last_log_time: fasync::BootInstant,
    connect_per_security_type: HashMap<
        metrics::SuccessfulConnectBreakdownBySecurityTypeMetricDimensionSecurityType,
        ConnectAttemptsCounter,
    >,
    connect_per_primary_channel: HashMap<u8, ConnectAttemptsCounter>,
    connect_per_channel_band: HashMap<
        metrics::SuccessfulConnectBreakdownByChannelBandMetricDimensionChannelBand,
        ConnectAttemptsCounter,
    >,
    connect_per_rssi_bucket:
        HashMap<metrics::ConnectivityWlanMetricDimensionRssiBucket, ConnectAttemptsCounter>,
    connect_per_snr_bucket:
        HashMap<metrics::ConnectivityWlanMetricDimensionSnrBucket, ConnectAttemptsCounter>,
    connect_per_is_owe_transition: HashMap<
        metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition,
        ConnectAttemptsCounter,
    >,
}

impl DailyConnectStats {
    fn new(now: fasync::BootInstant) -> Self {
        Self {
            last_log_time: now,
            connect_per_security_type: HashMap::new(),
            connect_per_primary_channel: HashMap::new(),
            connect_per_channel_band: HashMap::new(),
            connect_per_rssi_bucket: HashMap::new(),
            connect_per_snr_bucket: HashMap::new(),
            connect_per_is_owe_transition: HashMap::new(),
        }
    }
}

// Convert float to an integer in "ten thousandth" unit
// Example: 0.02f64 (i.e. 2%) -> 200 per ten thousand
fn float_to_ten_thousandth(value: f64) -> i64 {
    (value * 10000f64) as i64
}

fn append_device_connected_cobalt_metrics(
    metric_events: &mut Vec<MetricEvent>,
    bss: &BssDescription,
    is_owe_transition: bool,
) {
    metric_events.push(MetricEvent {
        metric_id: metrics::NUMBER_OF_CONNECTED_DEVICES_METRIC_ID,
        event_codes: vec![],
        payload: MetricEventPayload::Count(1),
    });

    let security_type_dim = convert_security_type(&bss.protection());
    metric_events.push(MetricEvent {
        metric_id: metrics::CONNECTED_NETWORK_SECURITY_TYPE_METRIC_ID,
        event_codes: vec![security_type_dim as u32],
        payload: MetricEventPayload::Count(1),
    });

    if bss.supports_uapsd() {
        metric_events.push(MetricEvent {
            metric_id: metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_APSD_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        });
    }

    if let Some(rm_enabled_cap) = bss.rm_enabled_cap() {
        if rm_enabled_cap.link_measurement_enabled() {
            metric_events.push(MetricEvent {
                metric_id: metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_LINK_MEASUREMENT_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::Count(1),
            });
        }
        if rm_enabled_cap.neighbor_report_enabled() {
            metric_events.push(MetricEvent {
                metric_id: metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_NEIGHBOR_REPORT_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::Count(1),
            });
        }
    }

    if bss.supports_ft() {
        metric_events.push(MetricEvent {
            metric_id: metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_FT_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        });
    }

    if let Some(cap) = bss.ext_cap().and_then(|cap| cap.ext_caps_octet_3)
        && cap.bss_transition()
    {
        metric_events.push(MetricEvent {
            metric_id:
                metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_BSS_TRANSITION_MANAGEMENT_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        });
    }

    append_device_connected_channel_cobalt_metrics(metric_events, bss.channel);

    let oui_string = bss.bssid.to_oui_uppercase("");
    metric_events.push(MetricEvent {
        metric_id: metrics::DEVICE_CONNECTED_TO_AP_OUI_2_METRIC_ID,
        event_codes: vec![],
        payload: MetricEventPayload::StringValue(oui_string),
    });

    let is_owe_transition_dim = convert_is_owe_transition(is_owe_transition);
    metric_events.push(MetricEvent {
        metric_id: metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_IS_OWE_TRANSITION_METRIC_ID,
        event_codes: vec![is_owe_transition_dim as u32],
        payload: MetricEventPayload::Count(1),
    });
}

fn append_device_connected_channel_cobalt_metrics(
    metric_events: &mut Vec<MetricEvent>,
    channel: Channel,
) {
    metric_events.push(MetricEvent {
        metric_id: metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_PRIMARY_CHANNEL_METRIC_ID,
        event_codes: vec![channel.primary as u32],
        payload: MetricEventPayload::Count(1),
    });

    let channel_band_dim = convert_channel_band(channel.band);
    metric_events.push(MetricEvent {
        metric_id: metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_CHANNEL_BAND_METRIC_ID,
        event_codes: vec![channel_band_dim as u32],
        payload: MetricEventPayload::Count(1),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{
        AnyBoolProperty, AnyBytesProperty, AnyNumericProperty, AnyStringProperty, assert_data_tree,
    };
    use futures::task::Poll;
    use ieee80211_testutils::{BSSID_REGEX, SSID_REGEX};
    use std::pin::pin;
    use strum::EnumCount;
    use test_case::test_case;
    use windowed_stats::experimental::clock::Timed;
    use windowed_stats::experimental::inspect::TimeMatrixClient;
    use windowed_stats::experimental::testing::TimeMatrixCall;
    use wlan_common::channel::{Bandwidth, Channel};
    use wlan_common::ie::IeType;
    use wlan_common::test_utils::fake_stas::IesOverrides;
    use wlan_common::{fake_bss_description, random_bss_description};

    #[fuchsia::test]
    fn log_connect_attempt_then_inspect_data_tree_contains_time_matrix_metadata() {
        let mut harness = setup_test();

        let client =
            TimeMatrixClient::new(harness.inspect_node.create_child("wlan_connect_disconnect"));
        let logger = ConnectDisconnectLogger::new(
            harness.filtered_cobalt_logger(),
            &harness.inspect_node,
            &harness.inspect_metadata_node,
            &harness.inspect_metadata_path,
            &client,
            DeviceMobility::Mobile,
        );
        let bss = random_bss_description!();
        let mut log_connect_attempt = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss,
            false,
            false
        ));
        assert!(
            harness.run_until_stalled_drain_cobalt_events(&mut log_connect_attempt).is_ready(),
            "`log_connect_attempt` did not complete",
        );

        let tree = harness.get_inspect_data_tree();
        assert_data_tree!(
            @executor harness.exec,
            tree,
            root: contains {
                test_stats: contains {
                    wlan_connect_disconnect: contains {
                        wlan_connectivity_states: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index: {
                                    "0": "idle",
                                    "1": "disconnected",
                                    "2": "connect_failed",
                                    "3": "connected",
                                    "4": "start_failure",
                                    "5": "stop_failure",
                                    "6": "pno_scan_failed",
                                },
                            },
                        },
                        connected_networks: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                "index_node_path": "root/test_stats/metadata/connected_networks",
                            },
                        },
                        disconnected_networks: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                "index_node_path": "root/test_stats/metadata/connected_networks",
                            },
                        },
                        disconnect_sources: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                "index_node_path": "root/test_stats/metadata/disconnect_sources",
                            },
                        },
                        connect_attempt_results: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                "index_node_path": "root/test_stats/metadata/connect_attempt_results",
                            },
                        },
                    },
                },
            }
        );
    }

    #[fuchsia::test]
    fn test_log_connect_attempt_inspect() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Log the event
        let bss_description = random_bss_description!();
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Validate Inspect data
        let data = test_helper.get_inspect_data_tree();
        assert_data_tree!(@executor test_helper.exec, data, root: contains {
            test_stats: contains {
                metadata: contains {
                    connected_networks: contains {
                        "0": {
                            "@time": AnyNumericProperty,
                            "data": contains {
                                bssid: &*BSSID_REGEX,
                                ssid: &*SSID_REGEX,
                            }
                        }
                    },
                    connect_attempt_results: contains {
                        "0": {
                            "@time": AnyNumericProperty,
                            "data": contains {
                                status_code: 0u64,
                                result: "Success",
                            }
                        }
                    },
                },
                connect_events: {
                    "0": {
                        "@time": AnyNumericProperty,
                        network_id: 0u64,
                    }
                },
                connect_attempt_results: {
                    "0": {
                        "@time": AnyNumericProperty,
                        result: "Success",
                        ssid: &*SSID_REGEX,
                        bssid: &*BSSID_REGEX,
                        protection: AnyStringProperty,
                    }
                }
            }
        });

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("wlan_connectivity_states")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0)), TimeMatrixCall::Fold(Timed::now(1 << 3)),]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("connected_networks")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0))]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("connect_attempt_results")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0))]
        );
    }

    #[fuchsia::test]
    fn test_log_connect_attempt_cobalt() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Generate BSS Description
        let bss_description = random_bss_description!(Wpa2,
            channel: Channel::new(157, Bandwidth::Cbw40, fidl_ieee80211::WlanBand::FiveGhz),
            bssid: [0x00, 0xf6, 0x20, 0x03, 0x04, 0x05],
        );

        // Log the event
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Validate Cobalt data
        let breakdowns_by_status_code = test_helper
            .get_logged_metrics(metrics::CONNECT_ATTEMPT_BREAKDOWN_BY_STATUS_CODE_METRIC_ID);
        assert_eq!(breakdowns_by_status_code.len(), 1);
        assert_eq!(
            breakdowns_by_status_code[0].event_codes,
            vec![fidl_ieee80211::StatusCode::Success.into_primitive() as u32]
        );
        assert_eq!(breakdowns_by_status_code[0].payload, MetricEventPayload::Count(1));

        let metrics_devices =
            test_helper.get_logged_metrics(metrics::NUMBER_OF_CONNECTED_DEVICES_METRIC_ID);
        assert_eq!(metrics_devices.len(), 1);
        assert_eq!(metrics_devices[0].payload, MetricEventPayload::Count(1));

        let metrics_security =
            test_helper.get_logged_metrics(metrics::CONNECTED_NETWORK_SECURITY_TYPE_METRIC_ID);
        assert_eq!(metrics_security.len(), 1);
        assert_eq!(metrics_security[0].event_codes, vec![5]); // Wpa2Personal

        let metrics_channel = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_PRIMARY_CHANNEL_METRIC_ID,
        );
        assert_eq!(metrics_channel.len(), 1);
        assert_eq!(metrics_channel[0].event_codes, vec![157]);

        let metrics_band = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_CHANNEL_BAND_METRIC_ID,
        );
        assert_eq!(metrics_band.len(), 1);
        assert_eq!(metrics_band[0].event_codes, vec![2]); // Band5Ghz

        let metrics_oui =
            test_helper.get_logged_metrics(metrics::DEVICE_CONNECTED_TO_AP_OUI_2_METRIC_ID);
        assert_eq!(metrics_oui.len(), 1);
        assert_eq!(metrics_oui[0].payload, MetricEventPayload::StringValue("00F620".to_string()));

        let metrics_owe_transition = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_IS_OWE_TRANSITION_METRIC_ID,
        );
        assert_eq!(metrics_owe_transition.len(), 1);
        assert_eq!(
            metrics_owe_transition[0].event_codes,
            vec![
                metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition::No
                    as u32
            ]
        );
    }

    #[fuchsia::test]
    fn test_handle_channel_switched() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let channel = Channel::new(157, Bandwidth::Cbw40, fidl_ieee80211::WlanBand::FiveGhz);
        let mut test_fut = pin!(logger.handle_channel_switched(channel));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics_channel = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_PRIMARY_CHANNEL_METRIC_ID,
        );
        assert_eq!(metrics_channel.len(), 1);
        assert_eq!(metrics_channel[0].event_codes, vec![157]);
        assert_eq!(metrics_channel[0].payload, MetricEventPayload::Count(1));

        let metrics_band = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_CHANNEL_BAND_METRIC_ID,
        );
        assert_eq!(metrics_band.len(), 1);
        assert_eq!(metrics_band[0].event_codes, vec![2]); // Band5Ghz
        assert_eq!(metrics_band[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_successive_connect_attempt_failures_cobalt_zero_failures() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let bss_description = random_bss_description!(Wpa2);
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(0));
    }

    #[fuchsia::test]
    fn test_log_device_connected_metrics_capabilities() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let wmm_info = vec![0x80]; // U-APSD enabled
        #[rustfmt::skip]
        let rm_enabled_capabilities = vec![
            0x03, // link measurement and neighbor report enabled
            0x00, 0x00, 0x00, 0x00,
        ];
        #[rustfmt::skip]
        let ext_capabilities = vec![
            0x04, 0x00,
            0x08, // BSS transition supported
            0x00, 0x00, 0x00, 0x00, 0x40
        ];

        let bss_description = fake_bss_description!(Wpa2,
            ies_overrides: IesOverrides::new()
                .remove(IeType::WMM_PARAM)
                .set(IeType::WMM_INFO, wmm_info)
                .set(IeType::RM_ENABLED_CAPABILITIES, rm_enabled_capabilities)
                .set(IeType::MOBILITY_DOMAIN, vec![0x00; 3])
                .set(IeType::EXT_CAPABILITIES, ext_capabilities),
        );

        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics = test_helper
            .get_logged_metrics(metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_APSD_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_BSS_TRANSITION_MANAGEMENT_METRIC_ID,
        );
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_LINK_MEASUREMENT_METRIC_ID,
        );
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_THAT_SUPPORTS_NEIGHBOR_REPORT_METRIC_ID,
        );
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[test_case(1; "one_failure")]
    #[test_case(2; "two_failures")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_successive_connect_attempt_failures_cobalt_one_failure_then_success(n_failures: usize) {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let bss_description = random_bss_description!(Wpa2);
        for _i in 0..n_failures {
            let mut test_fut = pin!(logger.handle_connect_attempt(
                fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                &bss_description,
                false,
                false
            ));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert!(metrics.is_empty());

        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(n_failures as i64));

        // Verify subsequent successes would report 0 failures
        test_helper.clear_cobalt_events();
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(0));
    }

    #[test_case(1; "one_failure")]
    #[test_case(2; "two_failures")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_successive_connect_attempt_failures_cobalt_one_failure_then_timeout(n_failures: usize) {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let bss_description = random_bss_description!(Wpa2);
        for _i in 0..n_failures {
            let mut test_fut = pin!(logger.handle_connect_attempt(
                fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                &bss_description,
                false,
                false
            ));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(60_000_000_000));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Not enough time has passed, so successive_connect_attempt_failures is not flushed yet
        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert!(metrics.is_empty());

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(120_000_000_000));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(n_failures as i64));

        // Verify timeout fires only once
        test_helper.clear_cobalt_events();
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(240_000_000_000));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_daily_connect_success_rate_breakdowns() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let mut bss = random_bss_description!(Wpa2);
        bss.channel = Channel::new(6, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz); // primary channel 6 -> Band2Dot4Ghz
        bss.rssi_dbm = -50; // rssi -50 -> From50To35 (event code 11)
        bss.snr_db = 15; // snr 15 -> From11To15 (event code 3)

        // 1 success, 1 failure => 50% success rate
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss,
            false,
            true
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
            &bss,
            false,
            true
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Before 24 hours pass, no daily metrics should be logged
        test_helper.clear_cobalt_events();
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(24 * 3600 * 1_000_000_000 - 1));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        assert!(
            test_helper
                .get_logged_metrics(
                    metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_SECURITY_TYPE_METRIC_ID
                )
                .is_empty()
        );

        // After 24 hours pass, daily metrics should be logged
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(24 * 3600 * 1_000_000_000));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Check security type breakdown
        let daily_security_metrics = test_helper.get_logged_metrics(
            metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_SECURITY_TYPE_METRIC_ID,
        );
        assert_eq!(daily_security_metrics.len(), 1);
        assert_eq!(
            daily_security_metrics[0].event_codes,
            vec![
                metrics::SuccessfulConnectBreakdownBySecurityTypeMetricDimensionSecurityType::Wpa2Personal
                    as u32
            ]
        );
        assert_eq!(daily_security_metrics[0].payload, MetricEventPayload::IntegerValue(5000));

        // Check primary channel breakdown
        let daily_channel_metrics = test_helper.get_logged_metrics(
            metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_PRIMARY_CHANNEL_METRIC_ID,
        );
        assert_eq!(daily_channel_metrics.len(), 1);
        assert_eq!(daily_channel_metrics[0].event_codes, vec![6]);
        assert_eq!(daily_channel_metrics[0].payload, MetricEventPayload::IntegerValue(5000));

        // Check channel band breakdown
        let daily_band_metrics = test_helper.get_logged_metrics(
            metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_CHANNEL_BAND_METRIC_ID,
        );
        assert_eq!(daily_band_metrics.len(), 1);
        assert_eq!(
            daily_band_metrics[0].event_codes,
            vec![
                metrics::SuccessfulConnectBreakdownByChannelBandMetricDimensionChannelBand::Band2Dot4Ghz
                    as u32
            ]
        );
        assert_eq!(daily_band_metrics[0].payload, MetricEventPayload::IntegerValue(5000));

        // Check rssi bucket breakdown
        let daily_rssi_metrics = test_helper.get_logged_metrics(
            metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_RSSI_BUCKET_METRIC_ID,
        );
        assert_eq!(daily_rssi_metrics.len(), 1);
        assert_eq!(
            daily_rssi_metrics[0].event_codes,
            vec![metrics::ConnectivityWlanMetricDimensionRssiBucket::From50To35 as u32]
        );
        assert_eq!(daily_rssi_metrics[0].payload, MetricEventPayload::IntegerValue(5000));

        // Check snr bucket breakdown
        let daily_snr_metrics = test_helper.get_logged_metrics(
            metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_SNR_BUCKET_METRIC_ID,
        );
        assert_eq!(daily_snr_metrics.len(), 1);
        assert_eq!(
            daily_snr_metrics[0].event_codes,
            vec![metrics::ConnectivityWlanMetricDimensionSnrBucket::From11To15 as u32]
        );
        assert_eq!(daily_snr_metrics[0].payload, MetricEventPayload::IntegerValue(5000));

        // Check is_owe_transition breakdown
        let daily_owe_metrics = test_helper.get_logged_metrics(
            metrics::DAILY_CONNECT_SUCCESS_RATE_BREAKDOWN_BY_IS_OWE_TRANSITION_METRIC_ID,
        );
        assert_eq!(daily_owe_metrics.len(), 1);
        assert_eq!(
            daily_owe_metrics[0].event_codes,
            vec![
                metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition::Yes
                    as u32
            ]
        );
        assert_eq!(daily_owe_metrics[0].payload, MetricEventPayload::IntegerValue(5000));
    }

    #[fuchsia::test]
    fn test_log_device_connected_cobalt_metrics_periodically() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let mut bss = random_bss_description!(Wpa2,
            bssid: [0x00, 0xf6, 0x20, 0x03, 0x04, 0x05],
        );
        bss.channel = Channel::new(6, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz);

        // Connect
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let connected_metrics =
            test_helper.get_logged_metrics(metrics::NUMBER_OF_CONNECTED_DEVICES_METRIC_ID);
        assert_eq!(connected_metrics.len(), 1);

        // Before 24 hours pass, no periodic device connected metrics should be logged
        test_helper.clear_cobalt_events();
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(24 * 3600 * 1_000_000_000 - 1));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        assert!(
            test_helper
                .get_logged_metrics(metrics::NUMBER_OF_CONNECTED_DEVICES_METRIC_ID)
                .is_empty()
        );

        // After 24 hours pass, device connected metrics should be logged again
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(24 * 3600 * 1_000_000_000));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let connected_metrics =
            test_helper.get_logged_metrics(metrics::NUMBER_OF_CONNECTED_DEVICES_METRIC_ID);
        assert_eq!(connected_metrics.len(), 1);
        assert_eq!(connected_metrics[0].payload, MetricEventPayload::Count(1));

        let security_metrics =
            test_helper.get_logged_metrics(metrics::CONNECTED_NETWORK_SECURITY_TYPE_METRIC_ID);
        assert_eq!(security_metrics.len(), 1);

        let channel_metrics = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_PRIMARY_CHANNEL_METRIC_ID,
        );
        assert_eq!(channel_metrics.len(), 1);
        assert_eq!(channel_metrics[0].event_codes, vec![6]);

        let oui_metrics =
            test_helper.get_logged_metrics(metrics::DEVICE_CONNECTED_TO_AP_OUI_2_METRIC_ID);
        assert_eq!(oui_metrics.len(), 1);
        assert_eq!(oui_metrics[0].payload, MetricEventPayload::StringValue("00F620".to_string()));
    }

    #[fuchsia::test]
    fn test_log_device_connected_cobalt_metrics_periodically_channel_switched() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let mut bss = random_bss_description!(Wpa2);
        bss.channel = Channel::new(6, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz);

        // Connect on channel 6
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Switch channel to 36
        let new_channel = Channel::new(36, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::FiveGhz);
        let mut test_fut = pin!(logger.handle_channel_switched(new_channel));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        test_helper.clear_cobalt_events();

        // After 24 hours pass, daily device connected metrics should reflect the switched channel
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(24 * 3600 * 1_000_000_000));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let channel_metrics = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_PRIMARY_CHANNEL_METRIC_ID,
        );
        assert_eq!(channel_metrics.len(), 1);
        assert_eq!(channel_metrics[0].event_codes, vec![36]);
    }

    #[fuchsia::test]
    fn test_log_device_connected_cobalt_metrics_periodically_not_connected() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        test_helper.clear_cobalt_events();

        // After 24 hours pass, since device is not connected, no device connected metrics are logged
        test_helper
            .exec
            .set_fake_time(fasync::MonotonicInstant::from_nanos(24 * 3600 * 1_000_000_000));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        assert!(
            test_helper
                .get_logged_metrics(metrics::NUMBER_OF_CONNECTED_DEVICES_METRIC_ID)
                .is_empty()
        );
    }

    #[fuchsia::test]
    fn test_log_connect_attempt_cobalt_owe_transition() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Generate BSS Description
        let bss_description = random_bss_description!(Wpa2,
            channel: Channel::new(157, Bandwidth::Cbw40, fidl_ieee80211::WlanBand::FiveGhz),
            bssid: [0x00, 0xf6, 0x20, 0x03, 0x04, 0x05],
        );

        // Log the event with is_owe_transition = true
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            true
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics_owe_transition = test_helper.get_logged_metrics(
            metrics::DEVICE_CONNECTED_TO_AP_BREAKDOWN_BY_IS_OWE_TRANSITION_METRIC_ID,
        );
        assert_eq!(metrics_owe_transition.len(), 1);
        assert_eq!(
            metrics_owe_transition[0].event_codes,
            vec![
                metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition::Yes
                    as u32
            ]
        );
    }

    #[fuchsia::test]
    fn test_zero_successive_connect_attempt_failures_on_suspend() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let mut test_fut = pin!(logger.handle_suspend_imminent());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[test_case(1; "one_failure")]
    #[test_case(2; "two_failures")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_one_or_more_successive_connect_attempt_failures_on_suspend(n_failures: usize) {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let bss_description = random_bss_description!(Wpa2);
        for _i in 0..n_failures {
            let mut test_fut = pin!(logger.handle_connect_attempt(
                fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                &bss_description,
                false,
                false
            ));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let mut test_fut = pin!(logger.handle_suspend_imminent());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(n_failures as i64));

        test_helper.clear_cobalt_events();
        let mut test_fut = pin!(logger.handle_suspend_imminent());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Count of successive failures shouldn't be logged again since it was already logged
        let metrics =
            test_helper.get_logged_metrics(metrics::SUCCESSIVE_CONNECT_ATTEMPT_FAILURES_METRIC_ID);
        assert!(metrics.is_empty());

        // Verify that the connection state has transitioned to ConnectFailed
        assert_matches!(*logger.connection_state.lock(), ConnectionState::ConnectFailed(_));
    }

    #[fuchsia::test]
    fn test_log_disconnect_inspect() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Log the event
        let bss_description = fake_bss_description!(Open);
        let channel = bss_description.channel;
        let disconnect_info = DisconnectInfo {
            iface_id: 32,
            connected_duration: zx::BootDuration::from_seconds(30),
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
            original_bss_desc: Box::new(bss_description),
            current_rssi_dbm: -30,
            current_snr_db: 25,
            current_channel: channel,
        };
        let mut test_fut = pin!(logger.log_disconnect(&disconnect_info));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Validate Inspect data
        let data = test_helper.get_inspect_data_tree();
        assert_data_tree!(@executor test_helper.exec, data, root: contains {
            test_stats: contains {
                metadata: contains {
                    connected_networks: {
                        "0": {
                            "@time": AnyNumericProperty,
                            "data": {
                                bssid: &*BSSID_REGEX,
                                ssid: &*SSID_REGEX,
                                ht_cap: AnyBytesProperty,
                                vht_cap: AnyBytesProperty,
                                protection: "Open",
                                is_wmm_assoc: AnyBoolProperty,
                                wmm_param: AnyBytesProperty,
                            }
                        }
                    },
                    disconnect_sources: {
                        "0": {
                            "@time": AnyNumericProperty,
                            "data": {
                                source: "ap",
                                reason: "UnspecifiedReason",
                                mlme_event_name: "DeauthenticateIndication",
                            }
                        }
                    },
                },
                disconnect_events: {
                    "0": {
                        "@time": AnyNumericProperty,
                        connected_duration: zx::BootDuration::from_seconds(30).into_nanos(),
                        disconnect_source_id: 0u64,
                        network_id: 0u64,
                        rssi_dbm: -30i64,
                        snr_db: 25i64,
                        channel: AnyStringProperty,
                    }
                }
            }
        });

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("wlan_connectivity_states")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0)), TimeMatrixCall::Fold(Timed::now(1 << 1)),]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("disconnected_networks")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0))]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("disconnect_sources")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0))]
        );
    }

    #[fuchsia::test]
    fn test_log_disconnect_cobalt() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Log the event
        let disconnect_info = DisconnectInfo {
            connected_duration: zx::BootDuration::from_millis(300_000),
            disconnect_source: fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
                reason_code: fidl_ieee80211::ReasonCode::ApInitiated,
            }),
            ..fake_disconnect_info()
        };
        let mut test_fut = pin!(logger.log_disconnect(&disconnect_info));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let disconnect_count_metrics =
            test_helper.get_logged_metrics(metrics::TOTAL_DISCONNECT_COUNT_METRIC_ID);
        assert_eq!(disconnect_count_metrics.len(), 1);
        assert_eq!(disconnect_count_metrics[0].payload, MetricEventPayload::Count(1));

        let connected_duration_metrics =
            test_helper.get_logged_metrics(metrics::CONNECTED_DURATION_ON_DISCONNECT_METRIC_ID);
        assert_eq!(connected_duration_metrics.len(), 1);
        assert_eq!(
            connected_duration_metrics[0].payload,
            MetricEventPayload::IntegerValue(300_000)
        );

        let disconnect_by_reason_metrics =
            test_helper.get_logged_metrics(metrics::DISCONNECT_BREAKDOWN_BY_REASON_CODE_METRIC_ID);
        assert_eq!(disconnect_by_reason_metrics.len(), 1);
        assert_eq!(disconnect_by_reason_metrics[0].payload, MetricEventPayload::Count(1));
        assert_eq!(disconnect_by_reason_metrics[0].event_codes.len(), 2);
        assert_eq!(
            disconnect_by_reason_metrics[0].event_codes[0],
            fidl_ieee80211::ReasonCode::ApInitiated.into_primitive() as u32
        );
        assert_eq!(
            disconnect_by_reason_metrics[0].event_codes[1],
            metrics::ConnectivityWlanMetricDimensionDisconnectSource::Ap as u32
        );
    }

    #[test_case(
        fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
            mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
        }),
        true;
        "ap_disconnect_source"
    )]
    #[test_case(
        fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
            mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
        }),
        true;
        "mlme_disconnect_source_not_link_failed"
    )]
    #[test_case(
        fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
            mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            reason_code: fidl_ieee80211::ReasonCode::MlmeLinkFailed,
        }),
        false;
        "mlme_link_failed"
    )]
    #[test_case(
        fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::Unknown),
        false;
        "user_disconnect_source"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_log_disconnect_for_mobile_device_cobalt(
        disconnect_source: fidl_sme::DisconnectSource,
        should_log: bool,
    ) {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Log the event
        let disconnect_info = DisconnectInfo { disconnect_source, ..fake_disconnect_info() };
        let mut test_fut = pin!(logger.log_disconnect(&disconnect_info));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics = test_helper
            .get_logged_metrics(metrics::DISCONNECT_OCCURRENCE_FOR_MOBILE_DEVICE_METRIC_ID);
        if should_log {
            assert_eq!(metrics.len(), 1);
            assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
            assert_matches!(*logger.connection_state.lock(), ConnectionState::Disconnected(_));
        } else {
            assert!(metrics.is_empty());
            assert_matches!(*logger.connection_state.lock(), ConnectionState::Idle(_));
        }
    }

    #[test_case(
        fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
            mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
        });
        "mlme_disconnect_source_not_link_failed"
    )]
    #[test_case(
        fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
            mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
            reason_code: fidl_ieee80211::ReasonCode::MlmeLinkFailed,
        });
        "mlme_link_failed"
    )]
    #[test_case(
        fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::Unknown);
        "user_disconnect_source"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_log_disconnect_for_stationary_device(disconnect_source: fidl_sme::DisconnectSource) {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Stationary,
        );

        // Log the event
        let disconnect_info = DisconnectInfo { disconnect_source, ..fake_disconnect_info() };
        let mut test_fut = pin!(logger.log_disconnect(&disconnect_info));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics = test_helper
            .get_logged_metrics(metrics::DISCONNECT_OCCURRENCE_FOR_MOBILE_DEVICE_METRIC_ID);
        assert!(metrics.is_empty());
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Disconnected(_));
    }

    #[fuchsia::test]
    fn test_log_downtime_post_disconnect_on_reconnect() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Connect at 15th second
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(15_000_000_000));
        let bss_description = random_bss_description!(Wpa2);
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Verify no downtime metric is logged on first successful connect
        let metrics = test_helper.get_logged_metrics(metrics::DOWNTIME_POST_DISCONNECT_METRIC_ID);
        assert!(metrics.is_empty());

        // Verify that the connection state has transitioned to Connected
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Connected(_));

        // Disconnect at 25th second
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(25_000_000_000));
        let disconnect_info = DisconnectInfo {
            connected_duration: zx::BootDuration::from_millis(300_000),
            disconnect_source: fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
                reason_code: fidl_ieee80211::ReasonCode::ApInitiated,
            }),
            ..fake_disconnect_info()
        };
        let mut test_fut = pin!(logger.log_disconnect(&disconnect_info));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Verify that the connection state has transitioned to Disconnected
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Disconnected(_));

        // Reconnect at 60th second
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(60_000_000_000));
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Verify that downtime metric is logged
        let metrics = test_helper.get_logged_metrics(metrics::DOWNTIME_POST_DISCONNECT_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(35_000));

        // Verify that the connection state has transitioned to Connected
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Connected(_));
    }

    #[fuchsia::test]
    fn test_log_iface_destroyed() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Log connect event to move state to connected
        let bss_description = random_bss_description!();
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Verify that the connection state has transitioned to Connected
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Connected(_));

        // Log iface destroyed event to move state to idle
        let mut test_fut = pin!(logger.handle_iface_destroyed());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("wlan_connectivity_states")[..],
            &[
                TimeMatrixCall::Fold(Timed::now(1 << 0)),
                TimeMatrixCall::Fold(Timed::now(1 << 3)),
                TimeMatrixCall::Fold(Timed::now(1 << 0))
            ]
        );

        // Verify that the connection state has transitioned to Idle
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Idle(_));
    }

    #[fuchsia::test]
    fn test_log_disable_client_connections() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Log connect event to move state to connected
        let bss_description = random_bss_description!();
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::Success,
            &bss_description,
            false,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Verify that the connection state has transitioned to Connected
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Connected(_));

        // Disable client connections to move state to idle
        let mut test_fut =
            pin!(logger.handle_client_connections_toggle(&ClientConnectionsToggleEvent::Disabled));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("wlan_connectivity_states")[..],
            &[
                TimeMatrixCall::Fold(Timed::now(1 << 0)),
                TimeMatrixCall::Fold(Timed::now(1 << 3)),
                TimeMatrixCall::Fold(Timed::now(1 << 0))
            ]
        );

        // Verify that the connection state has transitioned to Idle
        assert_matches!(*logger.connection_state.lock(), ConnectionState::Idle(_));
    }

    #[fuchsia::test]
    fn test_wlan_connectivity_states_credential_rejected() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Log connect failure with credential rejected to move state to idle
        let bss_description = random_bss_description!();
        let mut test_fut = pin!(logger.handle_connect_attempt(
            fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
            &bss_description,
            true,
            false
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        assert_matches!(*logger.connection_state.lock(), ConnectionState::Idle(_));
    }

    #[fuchsia::test]
    fn test_wlan_connectivity_states_failed_to_start() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let mut test_fut = pin!(logger.handle_client_connections_failed_to_start());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("wlan_connectivity_states")[..],
            &[
                TimeMatrixCall::Fold(Timed::now(1 << 0)), // Initialization
                TimeMatrixCall::Fold(Timed::now(1 << 4)), // FailedToStart ID is 4 -> bit 1 << 4
            ]
        );
        assert_matches!(*logger.connection_state.lock(), ConnectionState::FailedToStart(_));
    }

    #[fuchsia::test]
    fn test_wlan_connectivity_states_failed_to_stop() {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        let mut test_fut = pin!(logger.handle_client_connections_failed_to_stop());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("wlan_connectivity_states")[..],
            &[
                TimeMatrixCall::Fold(Timed::now(1 << 0)), // Initialization
                TimeMatrixCall::Fold(Timed::now(1 << 5)), // FailedToStop ID is 5 -> bit 1 << 5
            ]
        );

        assert_matches!(*logger.connection_state.lock(), ConnectionState::FailedToStop(_));
    }

    #[test_case(ConnectionState::Idle(IdleState {}))]
    #[test_case(ConnectionState::Disconnected(DisconnectedState {}))]
    #[test_case(ConnectionState::ConnectFailed(ConnectFailedState {}))]
    #[test_case(ConnectionState::PnoScanFailedIdle(PnoScanFailedIdleState {}))]
    fn test_connectivity_state_transition_on_pno_scan_failure(initial_state: ConnectionState) {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Transition to initial state
        *logger.connection_state.lock() = initial_state.clone();

        // Log a PNO scan failure
        let mut test_fut = pin!(logger.handle_pno_scan_failure());
        assert_matches!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Verify the metrics were logged
        let metric_events = test_helper
            .get_logged_metrics(metrics::PNO_SCAN_FAILURE_WHILE_NOT_CONNECTED_OCCURRENCE_METRIC_ID);
        assert_eq!(metric_events.len(), 1);
        assert_eq!(metric_events[0].payload, MetricEventPayload::Count(1));

        let metric_events =
            test_helper.get_logged_metrics(metrics::PNO_SCAN_FAILURE_OCCURRENCE_METRIC_ID);
        assert_eq!(metric_events.len(), 1);
        assert_eq!(metric_events[0].payload, MetricEventPayload::Count(1));

        // Verify the time matrix shows the PNO scan failure state
        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            *time_matrix_calls.drain::<u64>("wlan_connectivity_states")[..].last().unwrap(),
            TimeMatrixCall::Fold(Timed::now(1 << 6)), // PnoScanFailedIdle ID is 6 -> bit 1 << 6
        );

        // A PNO scan failure should cause a transition to PnoScanFailedIdle
        assert_matches!(*logger.connection_state.lock(), ConnectionState::PnoScanFailedIdle(_));
    }

    #[test_case(ConnectionState::Connected(ConnectedState {
        bss: Box::new(fake_bss_description!(Wpa2)),
        is_owe_transition: false,
    }))]
    #[test_case(ConnectionState::FailedToStart(FailedToStartState {}))]
    #[test_case(ConnectionState::FailedToStop(FailedToStopState {}))]
    fn test_no_connectivity_state_transition_on_pno_scan_failure(initial_state: ConnectionState) {
        let mut test_helper = setup_test();
        let logger = ConnectDisconnectLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.inspect_node,
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            DeviceMobility::Mobile,
        );

        // Transition to initial state
        *logger.connection_state.lock() = initial_state.clone();

        // Log a PNO scan failure
        let mut test_fut = pin!(logger.handle_pno_scan_failure());
        assert_matches!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Verify the metrics were logged
        let metric_events =
            test_helper.get_logged_metrics(metrics::PNO_SCAN_FAILURE_OCCURRENCE_METRIC_ID);
        assert_eq!(metric_events.len(), 1);
        assert_eq!(metric_events[0].payload, MetricEventPayload::Count(1));

        // State should not change
        assert_eq!(logger.connection_state.lock().to_id(), initial_state.to_id());
    }

    #[fuchsia::test]
    fn test_wlan_connectivity_states_bitset_map_size() {
        let enum_variant_count = ConnectionState::COUNT;
        let bitset_map_size =
            ConnectDisconnectTimeSeries::wlan_connectivity_states_bitset_map().len();
        assert_eq!(enum_variant_count, bitset_map_size);
    }

    fn fake_disconnect_info() -> DisconnectInfo {
        let bss_description = random_bss_description!(Wpa2);
        let channel = bss_description.channel;
        DisconnectInfo {
            iface_id: 1,
            connected_duration: zx::BootDuration::from_hours(6),
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::User(
                fidl_sme::UserDisconnectReason::Unknown,
            ),
            original_bss_desc: bss_description.into(),
            current_rssi_dbm: -30,
            current_snr_db: 25,
            current_channel: channel,
        }
    }
}
