// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_stats as fidl_stats;
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_inspect_contrib::nodes::LruCacheNode;
use fuchsia_inspect_derive::{InspectNode, Unit};
use futures::TryFutureExt;
use futures::lock::Mutex;

use log::{error, warn};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use windowed_stats::experimental::inspect::{InspectSender, InspectedTimeMatrix};
use windowed_stats::experimental::series::interpolation::{ConstantSample, LastSample};
use windowed_stats::experimental::series::metadata::BitsetNode;
use windowed_stats::experimental::series::statistic::{
    ArithmeticMean, Last, LatchMax, Max, Min, PostAggregation, Sum, Union,
};
use windowed_stats::experimental::series::{SamplingProfile, TimeMatrix};
use wlan_legacy_metrics_registry as metrics;

// Include a timeout on stats calls so that if the driver deadlocks, telemetry doesn't get stuck.
const GET_IFACE_STATS_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(5);
const GET_SIGNAL_REPORT_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(5);

#[derive(Debug)]
enum IfaceState {
    NotAvailable,
    Created { iface_id: u16, telemetry_proxy: Option<fidl_fuchsia_wlan_sme::TelemetryProxy> },
}

#[derive(Debug)]
enum GetIfaceStatsError {
    FutureTimeout,
    FidlError,
    ResponseError,
}

type CountersTimeSeriesMap = HashMap<u16, InspectedTimeMatrix<u64>>;
type GaugesTimeSeriesMap = HashMap<u16, Vec<InspectedTimeMatrix<i64>>>;

pub struct ClientIfaceCountersLogger<S> {
    iface_state: Arc<Mutex<IfaceState>>,
    cobalt_proxy: Arc<FilteredCobaltLogger>,
    monitor_svc_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
    inspect_metadata_node: Mutex<InspectMetadataNode>,
    time_series_stats: IfaceCountersTimeSeries,
    signal_time_series: SignalTimeSeries,
    driver_counters_time_matrix_client: S,
    driver_counters_time_series: Arc<Mutex<CountersTimeSeriesMap>>,
    driver_gauges_time_matrix_client: S,
    driver_gauges_time_series: Arc<Mutex<GaugesTimeSeriesMap>>,
    prev_connection_stats: Arc<Mutex<Option<fidl_stats::ConnectionStats>>>,
    boot_mono_drift: AtomicI64,
}

impl<S: InspectSender> ClientIfaceCountersLogger<S> {
    pub fn new(
        cobalt_proxy: Arc<FilteredCobaltLogger>,
        monitor_svc_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
        inspect_metadata_node: &InspectNode,
        inspect_metadata_path: &str,
        time_matrix_client: &S,
        driver_counters_time_matrix_client: S,
        driver_gauges_time_matrix_client: S,
    ) -> Self {
        Self {
            iface_state: Arc::new(Mutex::new(IfaceState::NotAvailable)),
            cobalt_proxy,
            monitor_svc_proxy,
            inspect_metadata_node: Mutex::new(InspectMetadataNode::new(inspect_metadata_node)),
            time_series_stats: IfaceCountersTimeSeries::new(time_matrix_client),
            signal_time_series: SignalTimeSeries::new(time_matrix_client, inspect_metadata_path),
            driver_counters_time_matrix_client,
            driver_counters_time_series: Arc::new(Mutex::new(HashMap::new())),
            driver_gauges_time_matrix_client,
            driver_gauges_time_series: Arc::new(Mutex::new(HashMap::new())),
            prev_connection_stats: Arc::new(Mutex::new(None)),
            boot_mono_drift: AtomicI64::new(0),
        }
    }

    pub async fn handle_iface_created(&self, iface_id: u16) {
        let (proxy, server) = fidl::endpoints::create_proxy();
        let telemetry_proxy = match self.monitor_svc_proxy.get_sme_telemetry(iface_id, server).await
        {
            Ok(Ok(())) => {
                let (inspect_counter_configs, inspect_gauge_configs) = match proxy
                    .query_telemetry_support()
                    .await
                {
                    Ok(Ok(support)) => {
                        (support.inspect_counter_configs, support.inspect_gauge_configs)
                    }
                    Ok(Err(code)) => {
                        warn!(
                            "Failed to query telemetry support with status code {}. No driver-specific stats will be captured",
                            code
                        );
                        (None, None)
                    }
                    Err(e) => {
                        error!(
                            "Failed to query telemetry support with error {}. No driver-specific stats will be captured",
                            e
                        );
                        (None, None)
                    }
                };
                if let Some(inspect_counter_configs) = &inspect_counter_configs {
                    let mut driver_counters_time_series =
                        self.driver_counters_time_series.lock().await;
                    for inspect_counter_config in inspect_counter_configs {
                        if let fidl_stats::InspectCounterConfig {
                            counter_id: Some(counter_id),
                            counter_name: Some(counter_name),
                            ..
                        } = inspect_counter_config
                        {
                            let _time_matrix_ref = driver_counters_time_series
                                .entry(*counter_id)
                                .or_insert_with(|| {
                                    self.driver_counters_time_matrix_client.inspect_time_matrix(
                                        counter_name,
                                        TimeMatrix::<LatchMax<u64>, LastSample>::new(
                                            SamplingProfile::balanced(),
                                            LastSample::or(0),
                                        ),
                                    )
                                });
                        }
                    }
                }
                if let Some(inspect_gauge_configs) = &inspect_gauge_configs {
                    let mut driver_gauges_time_series = self.driver_gauges_time_series.lock().await;
                    for inspect_gauge_config in inspect_gauge_configs {
                        if let fidl_stats::InspectGaugeConfig {
                            gauge_id: Some(gauge_id),
                            gauge_name: Some(gauge_name),
                            statistics: Some(statistics),
                            ..
                        } = inspect_gauge_config
                        {
                            for statistic in statistics {
                                if let Some(time_matrix) = create_time_series_for_gauge(
                                    &self.driver_gauges_time_matrix_client,
                                    gauge_name,
                                    statistic,
                                ) {
                                    let time_matrices =
                                        driver_gauges_time_series.entry(*gauge_id).or_default();
                                    time_matrices.push(time_matrix);
                                }
                            }
                        }
                    }
                }
                Some(proxy)
            }
            Ok(Err(e)) => {
                error!(
                    "Request for SME telemetry for iface {} completed with error {}. No telemetry will be captured.",
                    iface_id, e
                );
                None
            }
            Err(e) => {
                error!(
                    "Failed to request SME telemetry for iface {} with error {}. No telemetry will be captured.",
                    iface_id, e
                );
                None
            }
        };
        *self.iface_state.lock().await = IfaceState::Created { iface_id, telemetry_proxy }
    }

    pub async fn handle_iface_destroyed(&self, iface_id: u16) {
        let destroyed = matches!(*self.iface_state.lock().await, IfaceState::Created { iface_id: existing_iface_id, .. } if iface_id == existing_iface_id);
        if destroyed {
            *self.iface_state.lock().await = IfaceState::NotAvailable;
        }
    }

    pub async fn handle_periodic_telemetry(&self) {
        let boot_now = fasync::BootInstant::now();
        let mono_now = fasync::MonotonicInstant::now();
        let boot_mono_drift = boot_now.into_nanos() - mono_now.into_nanos();
        let prev_boot_mono_drift = self.boot_mono_drift.swap(boot_mono_drift, Ordering::SeqCst);
        // If the difference between boot time and monotonic time has increased, it means that
        // there was a suspension since the last time `handle_periodic_telemetry` was called.
        let suspended_during_last_period = boot_mono_drift > prev_boot_mono_drift;
        match &*self.iface_state.lock().await {
            IfaceState::NotAvailable => (),
            IfaceState::Created { telemetry_proxy, .. } => {
                if let Some(telemetry_proxy) = &telemetry_proxy {
                    let result = telemetry_proxy
                        .get_iface_stats()
                        .map_err(|_e| GetIfaceStatsError::FidlError)
                        .map_ok(|response| response.map_err(|_e| GetIfaceStatsError::ResponseError))
                        .on_timeout(GET_IFACE_STATS_TIMEOUT, || {
                            Err(GetIfaceStatsError::FutureTimeout)
                        })
                        .await;

                    match result {
                        Ok(Ok(stats)) => {
                            self.log_iface_stats_inspect(&stats).await;
                            self.log_iface_stats_cobalt(stats, suspended_during_last_period).await;
                        }
                        Ok(Err(e)) | Err(e) => {
                            warn!("Failed to get interface stats: {:?}", e);
                            self.log_get_iface_stats_failure_cobalt(&e).await;
                        }
                    }

                    match telemetry_proxy
                        .get_signal_report()
                        .on_timeout(GET_SIGNAL_REPORT_TIMEOUT, || {
                            Ok(Err(zx::Status::TIMED_OUT.into_raw()))
                        })
                        .await
                    {
                        Ok(Ok(report)) => {
                            self.log_signal_report_inspect(&report).await;
                        }
                        error => {
                            warn!("Failed to get signal report: {:?}", error);
                        }
                    }
                }
            }
        }
    }

    async fn log_iface_stats_inspect(&self, stats: &fidl_stats::IfaceStats) {
        // Iface-level driver specific counters
        if let Some(counters) = &stats.driver_specific_counters {
            let time_series = Arc::clone(&self.driver_counters_time_series);
            log_driver_specific_counters(&counters[..], time_series).await;
        }
        // Iface-level driver specific gauges
        if let Some(gauges) = &stats.driver_specific_gauges {
            let time_series = Arc::clone(&self.driver_gauges_time_series);
            log_driver_specific_gauges(&gauges[..], time_series).await;
        }
        log_connection_stats_inspect(
            stats,
            &self.time_series_stats,
            Arc::clone(&self.driver_counters_time_series),
            Arc::clone(&self.driver_gauges_time_series),
        )
        .await;
    }

    async fn log_iface_stats_cobalt(
        &self,
        stats: fidl_stats::IfaceStats,
        suspended_during_last_period: bool,
    ) {
        let mut prev_connection_stats = self.prev_connection_stats.lock().await;
        // Only log to Cobalt if there was no suspension in-between
        if !suspended_during_last_period
            && let (Some(prev_connection_stats), Some(current_connection_stats)) =
                (prev_connection_stats.as_ref(), stats.connection_stats.as_ref())
        {
            match (prev_connection_stats.connection_id, current_connection_stats.connection_id) {
                (Some(prev_id), Some(current_id)) if prev_id == current_id => {
                    diff_and_log_connection_stats_cobalt(
                        &self.cobalt_proxy,
                        prev_connection_stats,
                        current_connection_stats,
                    )
                    .await;
                }
                _ => (),
            }
        }
        *prev_connection_stats = stats.connection_stats;
    }

    async fn log_get_iface_stats_failure_cobalt(&self, error: &GetIfaceStatsError) {
        let mut metric_events = vec![MetricEvent {
            metric_id: metrics::GET_IFACE_STATS_FAILURE_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];
        match error {
            GetIfaceStatsError::FutureTimeout => {
                metric_events.push(MetricEvent {
                    metric_id: metrics::GET_IFACE_STATS_TIMEOUT_METRIC_ID,
                    event_codes: vec![],
                    payload: MetricEventPayload::Count(1),
                });
            }
            GetIfaceStatsError::FidlError | GetIfaceStatsError::ResponseError => {
                metric_events.push(MetricEvent {
                    metric_id: metrics::GET_IFACE_STATS_ERROR_IN_RESPONSE_METRIC_ID,
                    event_codes: vec![],
                    payload: MetricEventPayload::Count(1),
                });
            }
        }
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "log_get_iface_stats_failure_cobalt");
    }

    async fn log_signal_report_inspect(&self, report: &fidl_stats::SignalReport) {
        if let Some(report) = &report.connection_signal_report {
            if let Some(channel) = report.primary {
                let cbw = report.bandwidth.unwrap_or(fidl_ieee80211::ChannelBandwidth::Cbw20);
                let secondary80 = report.vht_secondary_80_channel;
                let channel = InspectWlanChannel::new(channel, cbw, secondary80);
                let channel_id =
                    self.inspect_metadata_node.lock().await.wlan_channels.insert(channel) as u64;
                self.signal_time_series.wlan_channels.fold_or_log_error(1 << channel_id);
            }
            if let Some(tx_rate_500kbps) = report.tx_rate_500kbps {
                self.signal_time_series.tx_rate_500kbps.fold_or_log_error(tx_rate_500kbps as u64);
            }
            if let Some(rssi) = report.rssi_dbm {
                self.signal_time_series.rssi.fold_or_log_error(rssi as i64);
            }
            if let Some(snr) = report.snr_db {
                self.signal_time_series.snr.fold_or_log_error(snr as i64);
            }
        }
    }
}

fn create_time_series_for_gauge<S: InspectSender>(
    time_matrix_client: &S,
    gauge_name: &str,
    statistic: &fidl_stats::GaugeStatistic,
) -> Option<InspectedTimeMatrix<i64>> {
    match statistic {
        fidl_stats::GaugeStatistic::Min => Some(time_matrix_client.inspect_time_matrix(
            format!("{gauge_name}__min"),
            TimeMatrix::<Min<i64>, ConstantSample>::new(
                SamplingProfile::balanced(),
                ConstantSample::default(),
            ),
        )),
        fidl_stats::GaugeStatistic::Max => Some(time_matrix_client.inspect_time_matrix(
            format!("{gauge_name}__max"),
            TimeMatrix::<Max<i64>, ConstantSample>::new(
                SamplingProfile::balanced(),
                ConstantSample::default(),
            ),
        )),
        fidl_stats::GaugeStatistic::Sum => Some(time_matrix_client.inspect_time_matrix(
            format!("{gauge_name}__sum"),
            TimeMatrix::<Sum<i64>, ConstantSample>::new(
                SamplingProfile::balanced(),
                ConstantSample::default(),
            ),
        )),
        fidl_stats::GaugeStatistic::Last => Some(time_matrix_client.inspect_time_matrix(
            format!("{gauge_name}__last"),
            TimeMatrix::<Last<i64>, ConstantSample>::new(
                SamplingProfile::balanced(),
                ConstantSample::default(),
            ),
        )),
        fidl_stats::GaugeStatistic::Mean => Some(time_matrix_client.inspect_time_matrix(
            format!("{gauge_name}__mean"),
            TimeMatrix::<ArithmeticMean<i64>, ConstantSample>::new(
                SamplingProfile::balanced(),
                ConstantSample::default(),
            ),
        )),
        _ => None,
    }
}

async fn log_connection_stats_inspect(
    stats: &fidl_stats::IfaceStats,
    time_series_stats: &IfaceCountersTimeSeries,
    driver_counters_time_series: Arc<Mutex<CountersTimeSeriesMap>>,
    driver_gauges_time_series: Arc<Mutex<GaugesTimeSeriesMap>>,
) {
    let connection_stats = match &stats.connection_stats {
        Some(counters) => counters,
        None => return,
    };

    // Enforce that `connection_id` field is there for us to log driver counters.
    match &connection_stats.connection_id {
        Some(_connection_id) => (),
        _ => {
            warn!("connection_id is not present, no connection counters will be logged");
            return;
        }
    }

    if let fidl_stats::ConnectionStats {
        rx_unicast_total: Some(rx_unicast_total),
        rx_unicast_drop: Some(rx_unicast_drop),
        ..
    } = connection_stats
    {
        time_series_stats.log_rx_unicast_total(*rx_unicast_total);
        time_series_stats.log_rx_unicast_drop(*rx_unicast_drop);
    }

    if let fidl_stats::ConnectionStats {
        tx_total: Some(tx_total), tx_drop: Some(tx_drop), ..
    } = connection_stats
    {
        time_series_stats.log_tx_total(*tx_total);
        time_series_stats.log_tx_drop(*tx_drop);
    }

    // Connection-level driver-specific counters
    if let Some(counters) = &connection_stats.driver_specific_counters {
        log_driver_specific_counters(&counters[..], driver_counters_time_series).await;
    }
    // Connection-level driver-specific gauges
    if let Some(gauges) = &connection_stats.driver_specific_gauges {
        log_driver_specific_gauges(&gauges[..], driver_gauges_time_series).await;
    }
}

async fn log_driver_specific_counters(
    driver_specific_counters: &[fidl_stats::UnnamedCounter],
    driver_counters_time_series: Arc<Mutex<CountersTimeSeriesMap>>,
) {
    let time_series_map = driver_counters_time_series.lock().await;
    for counter in driver_specific_counters {
        if let Some(ts) = time_series_map.get(&counter.id) {
            ts.fold_or_log_error(counter.count);
        }
    }
}

async fn log_driver_specific_gauges(
    driver_specific_gauges: &[fidl_stats::UnnamedGauge],
    driver_gauges_time_series: Arc<Mutex<GaugesTimeSeriesMap>>,
) {
    let time_series_map = driver_gauges_time_series.lock().await;
    for gauge in driver_specific_gauges {
        if let Some(time_matrices) = time_series_map.get(&gauge.id) {
            for ts in time_matrices {
                ts.fold_or_log_error(gauge.value);
            }
        }
    }
}

async fn diff_and_log_connection_stats_cobalt(
    cobalt_proxy: &FilteredCobaltLogger,
    prev: &fidl_stats::ConnectionStats,
    current: &fidl_stats::ConnectionStats,
) {
    // Early return if the counters have dropped. This indicates that the counters have reset
    // due to reasons like PHY reset. Counters being reset due to re-connection is already
    // handled outside this function.
    match (current.rx_unicast_total, prev.rx_unicast_total) {
        (Some(current), Some(prev)) if current < prev => return,
        _ => (),
    }
    match (current.rx_unicast_drop, prev.rx_unicast_drop) {
        (Some(current), Some(prev)) if current < prev => return,
        _ => (),
    }
    match (current.tx_total, prev.tx_total) {
        (Some(current), Some(prev)) if current < prev => return,
        _ => (),
    }
    match (current.tx_drop, prev.tx_drop) {
        (Some(current), Some(prev)) if current < prev => return,
        _ => (),
    }

    diff_and_log_rx_cobalt(cobalt_proxy, prev, current).await;
    diff_and_log_tx_cobalt(cobalt_proxy, prev, current).await;
}

async fn diff_and_log_rx_cobalt(
    cobalt_proxy: &FilteredCobaltLogger,
    prev: &fidl_stats::ConnectionStats,
    current: &fidl_stats::ConnectionStats,
) {
    let mut metric_events = vec![];

    let (current_rx_unicast_total, prev_rx_unicast_total) =
        match (current.rx_unicast_total, prev.rx_unicast_total) {
            (Some(current), Some(prev)) => (current, prev),
            _ => return,
        };
    let (current_rx_unicast_drop, prev_rx_unicast_drop) =
        match (current.rx_unicast_drop, prev.rx_unicast_drop) {
            (Some(current), Some(prev)) => (current, prev),
            _ => return,
        };

    let rx_total = match current_rx_unicast_total.checked_sub(prev_rx_unicast_total) {
        Some(diff) => diff,
        _ => return,
    };
    let rx_drop = match current_rx_unicast_drop.checked_sub(prev_rx_unicast_drop) {
        Some(diff) => diff,
        _ => return,
    };
    let rx_drop_rate = if rx_total > 0 { rx_drop as f64 / rx_total as f64 } else { 0f64 };

    metric_events.push(MetricEvent {
        metric_id: metrics::BAD_RX_RATE_METRIC_ID,
        event_codes: vec![],
        payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(rx_drop_rate)),
    });
    metric_events.push(MetricEvent {
        metric_id: metrics::RX_UNICAST_PACKETS_METRIC_ID,
        event_codes: vec![],
        payload: MetricEventPayload::IntegerValue(rx_total as i64),
    });

    log_cobalt_batch!(cobalt_proxy, &metric_events, "diff_and_log_rx_cobalt",);
}

async fn diff_and_log_tx_cobalt(
    cobalt_proxy: &FilteredCobaltLogger,
    prev: &fidl_stats::ConnectionStats,
    current: &fidl_stats::ConnectionStats,
) {
    let mut metric_events = vec![];

    let (current_tx_total, prev_tx_total) = match (current.tx_total, prev.tx_total) {
        (Some(current), Some(prev)) => (current, prev),
        _ => return,
    };
    let (current_tx_drop, prev_tx_drop) = match (current.tx_drop, prev.tx_drop) {
        (Some(current), Some(prev)) => (current, prev),
        _ => return,
    };

    let tx_total = match current_tx_total.checked_sub(prev_tx_total) {
        Some(diff) => diff,
        _ => return,
    };
    let tx_drop = match current_tx_drop.checked_sub(prev_tx_drop) {
        Some(diff) => diff,
        _ => return,
    };
    let tx_drop_rate = if tx_total > 0 { tx_drop as f64 / tx_total as f64 } else { 0f64 };

    metric_events.push(MetricEvent {
        metric_id: metrics::BAD_TX_RATE_METRIC_ID,
        event_codes: vec![],
        payload: MetricEventPayload::IntegerValue(float_to_ten_thousandth(tx_drop_rate)),
    });

    log_cobalt_batch!(cobalt_proxy, &metric_events, "diff_and_log_tx_cobalt",);
}

// Convert float to an integer in "ten thousandth" unit
// Example: 0.02f64 (i.e. 2%) -> 200 per ten thousand
fn float_to_ten_thousandth(value: f64) -> i64 {
    (value * 10000f64) as i64
}

#[derive(Debug, Clone)]
struct IfaceCountersTimeSeries {
    rx_unicast_total: InspectedTimeMatrix<u64>,
    rx_unicast_drop: InspectedTimeMatrix<u64>,
    tx_total: InspectedTimeMatrix<u64>,
    tx_drop: InspectedTimeMatrix<u64>,
}

impl IfaceCountersTimeSeries {
    pub fn new<S: InspectSender>(client: &S) -> Self {
        let rx_unicast_total = client.inspect_time_matrix(
            "rx_unicast_total",
            TimeMatrix::<LatchMax<u64>, LastSample>::new(
                SamplingProfile::balanced(),
                LastSample::or(0),
            ),
        );
        let rx_unicast_drop = client.inspect_time_matrix(
            "rx_unicast_drop",
            TimeMatrix::<LatchMax<u64>, LastSample>::new(
                SamplingProfile::balanced(),
                LastSample::or(0),
            ),
        );
        let tx_total = client.inspect_time_matrix(
            "tx_total",
            TimeMatrix::<LatchMax<u64>, LastSample>::new(
                SamplingProfile::balanced(),
                LastSample::or(0),
            ),
        );
        let tx_drop = client.inspect_time_matrix(
            "tx_drop",
            TimeMatrix::<LatchMax<u64>, LastSample>::new(
                SamplingProfile::balanced(),
                LastSample::or(0),
            ),
        );
        Self { rx_unicast_total, rx_unicast_drop, tx_total, tx_drop }
    }

    fn log_rx_unicast_total(&self, data: u64) {
        self.rx_unicast_total.fold_or_log_error(data);
    }

    fn log_rx_unicast_drop(&self, data: u64) {
        self.rx_unicast_drop.fold_or_log_error(data);
    }

    fn log_tx_total(&self, data: u64) {
        self.tx_total.fold_or_log_error(data);
    }

    fn log_tx_drop(&self, data: u64) {
        self.tx_drop.fold_or_log_error(data);
    }
}

#[derive(Debug, Clone)]
struct SignalTimeSeries {
    wlan_channels: InspectedTimeMatrix<u64>,
    tx_rate_500kbps: InspectedTimeMatrix<u64>,
    rssi: InspectedTimeMatrix<i64>,
    snr: InspectedTimeMatrix<i64>,
}

impl SignalTimeSeries {
    pub fn new<S: InspectSender>(client: &S, inspect_metadata_path: &str) -> Self {
        let wlan_channels = client.inspect_time_matrix_with_metadata(
            "wlan_channels",
            TimeMatrix::<Union<u64>, ConstantSample>::new(
                SamplingProfile::highly_granular(),
                ConstantSample::default(),
            ),
            BitsetNode::from_path(format!(
                "{}/{}",
                inspect_metadata_path,
                InspectMetadataNode::WLAN_CHANNELS,
            )),
        );
        let tx_rate_500kbps = client.inspect_time_matrix(
            "tx_rate_500kbps",
            TimeMatrix::<_, ConstantSample>::with_statistic(
                SamplingProfile::default(),
                ConstantSample::default(),
                PostAggregation::<ArithmeticMean<u64>, _>::from_transform(|aggregation: f32| {
                    aggregation.ceil() as u64
                }),
            ),
        );
        let rssi = client.inspect_time_matrix(
            "rssi",
            TimeMatrix::<_, ConstantSample>::with_statistic(
                SamplingProfile::default(),
                ConstantSample::default(),
                PostAggregation::<ArithmeticMean<i64>, _>::from_transform(|aggregation: f32| {
                    aggregation.ceil() as i64
                }),
            ),
        );
        let snr = client.inspect_time_matrix(
            "snr",
            TimeMatrix::<_, ConstantSample>::with_statistic(
                SamplingProfile::default(),
                ConstantSample::default(),
                PostAggregation::<ArithmeticMean<i64>, _>::from_transform(|aggregation: f32| {
                    aggregation.ceil() as i64
                }),
            ),
        );
        Self { wlan_channels, tx_rate_500kbps, rssi, snr }
    }
}

#[derive(PartialEq, Eq, Unit, Hash)]
struct InspectWlanChannel {
    primary: u8,
    band: String,
    cbw: String,
    secondary80_number: u8,
    secondary80_band: String,
}

impl InspectWlanChannel {
    fn new(
        channel: fidl_ieee80211::ChannelNumber,
        cbw: fidl_ieee80211::ChannelBandwidth,
        secondary80: Option<fidl_ieee80211::ChannelNumber>,
    ) -> Self {
        let (secondary80_number, secondary80_band) = match secondary80 {
            Some(c) => (c.number, format!("{:?}", c.band)),
            None => (0, "None".to_string()),
        };
        Self {
            primary: channel.number,
            band: format!("{:?}", channel.band),
            cbw: format!("{:?}", cbw),
            secondary80_number,
            secondary80_band,
        }
    }
}

struct InspectMetadataNode {
    wlan_channels: LruCacheNode<InspectWlanChannel>,
}

impl InspectMetadataNode {
    const WLAN_CHANNELS: &'static str = "wlan_channels";
    const WLAN_CHANNELS_ID_LIMIT: usize = 32;

    fn new(inspect_node: &InspectNode) -> Self {
        let wlan_channels = inspect_node.create_child(Self::WLAN_CHANNELS);
        Self { wlan_channels: LruCacheNode::new(wlan_channels, Self::WLAN_CHANNELS_ID_LIMIT) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyNumericProperty, assert_data_tree};
    use futures::TryStreamExt;
    use std::pin::pin;
    use std::task::Poll;
    use test_case::test_case;
    use windowed_stats::experimental::clock::Timed;
    use windowed_stats::experimental::testing::{MockTimeMatrixClient, TimeMatrixCall};

    const IFACE_ID: u16 = 66;

    #[fuchsia::test]
    fn test_handle_iface_created() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        let mut handle_iface_created_fut = pin!(logger.handle_iface_created(IFACE_ID));
        assert_eq!(
            test_helper.run_and_handle_get_sme_telemetry(&mut handle_iface_created_fut),
            Poll::Pending
        );

        let mocked_inspect_counter_configs = vec![fidl_stats::InspectCounterConfig {
            counter_id: Some(1),
            counter_name: Some("foo_counter".to_string()),
            ..Default::default()
        }];
        let telemetry_support = fidl_stats::TelemetrySupport {
            inspect_counter_configs: Some(mocked_inspect_counter_configs),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_query_telemetry_support(
                &mut handle_iface_created_fut,
                Ok(&telemetry_support)
            ),
            Poll::Ready(())
        );

        assert_matches!(logger.iface_state.try_lock().as_deref(), Some(IfaceState::Created { .. }));
        let driver_counters_time_series = logger.driver_counters_time_series.try_lock().unwrap();
        assert_eq!(driver_counters_time_series.keys().copied().collect::<Vec<u16>>(), vec![1u16],);
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_connection_stats() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        // Transition to IfaceCreated state
        handle_iface_created(&mut test_helper, &logger);

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                rx_unicast_total: Some(100),
                rx_unicast_drop: Some(5),
                rx_multicast: Some(30),
                tx_total: Some(50),
                tx_drop: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending,
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("rx_unicast_total")[..],
            &[TimeMatrixCall::Fold(Timed::now(100u64))]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("rx_unicast_drop")[..],
            &[TimeMatrixCall::Fold(Timed::now(5u64))]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("tx_total")[..],
            &[TimeMatrixCall::Fold(Timed::now(50u64))]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("tx_drop")[..],
            &[TimeMatrixCall::Fold(Timed::now(2u64))]
        );
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_driver_specific_counters() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        let mut handle_iface_created_fut = pin!(logger.handle_iface_created(IFACE_ID));
        assert_eq!(
            test_helper.run_and_handle_get_sme_telemetry(&mut handle_iface_created_fut),
            Poll::Pending
        );

        let mocked_inspect_configs = vec![
            fidl_stats::InspectCounterConfig {
                counter_id: Some(1),
                counter_name: Some("foo_counter".to_string()),
                ..Default::default()
            },
            fidl_stats::InspectCounterConfig {
                counter_id: Some(2),
                counter_name: Some("bar_counter".to_string()),
                ..Default::default()
            },
            fidl_stats::InspectCounterConfig {
                counter_id: Some(3),
                counter_name: Some("baz_counter".to_string()),
                ..Default::default()
            },
        ];
        let telemetry_support = fidl_stats::TelemetrySupport {
            inspect_counter_configs: Some(mocked_inspect_configs),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_query_telemetry_support(
                &mut handle_iface_created_fut,
                Ok(&telemetry_support)
            ),
            Poll::Ready(())
        );

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            driver_specific_counters: Some(vec![fidl_stats::UnnamedCounter { id: 1, count: 50 }]),
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                driver_specific_counters: Some(vec![
                    fidl_stats::UnnamedCounter { id: 2, count: 100 },
                    fidl_stats::UnnamedCounter { id: 3, count: 150 },
                    // This one is no-op because it's not registered in QueryTelemetrySupport
                    fidl_stats::UnnamedCounter { id: 4, count: 200 },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending,
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert!(time_matrix_calls.is_empty());

        let mut driver_counters_matrix_calls = driver_counters_mock_matrix_client.drain_calls();
        assert_eq!(
            &driver_counters_matrix_calls.drain::<u64>("foo_counter")[..],
            &[TimeMatrixCall::Fold(Timed::now(50))]
        );
        assert_eq!(
            &driver_counters_matrix_calls.drain::<u64>("bar_counter")[..],
            &[TimeMatrixCall::Fold(Timed::now(100))]
        );
        assert_eq!(
            &driver_counters_matrix_calls.drain::<u64>("baz_counter")[..],
            &[TimeMatrixCall::Fold(Timed::now(150))]
        );

        let driver_gauges_matrix_calls = driver_gauges_mock_matrix_client.drain_calls();
        assert!(driver_gauges_matrix_calls.is_empty());
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_driver_specific_gauges() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        let mut handle_iface_created_fut = pin!(logger.handle_iface_created(IFACE_ID));
        assert_eq!(
            test_helper.run_and_handle_get_sme_telemetry(&mut handle_iface_created_fut),
            Poll::Pending
        );

        let mocked_inspect_configs = vec![
            fidl_stats::InspectGaugeConfig {
                gauge_id: Some(1),
                gauge_name: Some("foo_gauge".to_string()),
                statistics: Some(vec![
                    fidl_stats::GaugeStatistic::Mean,
                    fidl_stats::GaugeStatistic::Last,
                ]),
                ..Default::default()
            },
            fidl_stats::InspectGaugeConfig {
                gauge_id: Some(2),
                gauge_name: Some("bar_gauge".to_string()),
                statistics: Some(vec![
                    fidl_stats::GaugeStatistic::Min,
                    fidl_stats::GaugeStatistic::Sum,
                ]),
                ..Default::default()
            },
            fidl_stats::InspectGaugeConfig {
                gauge_id: Some(3),
                gauge_name: Some("baz_gauge".to_string()),
                statistics: Some(vec![fidl_stats::GaugeStatistic::Max]),
                ..Default::default()
            },
        ];
        let telemetry_support = fidl_stats::TelemetrySupport {
            inspect_gauge_configs: Some(mocked_inspect_configs),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_query_telemetry_support(
                &mut handle_iface_created_fut,
                Ok(&telemetry_support)
            ),
            Poll::Ready(())
        );

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            driver_specific_gauges: Some(vec![fidl_stats::UnnamedGauge { id: 1, value: 50 }]),
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                driver_specific_gauges: Some(vec![
                    fidl_stats::UnnamedGauge { id: 2, value: 100 },
                    fidl_stats::UnnamedGauge { id: 3, value: 150 },
                    // This one is no-op because it's not registered in QueryTelemetrySupport
                    fidl_stats::UnnamedGauge { id: 4, value: 200 },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert!(time_matrix_calls.is_empty());

        let driver_counters_matrix_calls = driver_counters_mock_matrix_client.drain_calls();
        assert!(driver_counters_matrix_calls.is_empty());

        let mut driver_gauges_matrix_calls = driver_gauges_mock_matrix_client.drain_calls();
        assert_eq!(
            &driver_gauges_matrix_calls.drain::<i64>("foo_gauge__mean")[..],
            &[TimeMatrixCall::Fold(Timed::now(50))]
        );
        assert_eq!(
            &driver_gauges_matrix_calls.drain::<i64>("foo_gauge__last")[..],
            &[TimeMatrixCall::Fold(Timed::now(50))]
        );
        assert_eq!(
            &driver_gauges_matrix_calls.drain::<i64>("bar_gauge__min")[..],
            &[TimeMatrixCall::Fold(Timed::now(100))]
        );
        assert_eq!(
            &driver_gauges_matrix_calls.drain::<i64>("bar_gauge__sum")[..],
            &[TimeMatrixCall::Fold(Timed::now(100))]
        );
        assert_eq!(
            &driver_gauges_matrix_calls.drain::<i64>("baz_gauge__max")[..],
            &[TimeMatrixCall::Fold(Timed::now(150))]
        );
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_signal_report() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        let mut handle_iface_created_fut = pin!(logger.handle_iface_created(IFACE_ID));
        assert_eq!(
            test_helper.run_and_handle_get_sme_telemetry(&mut handle_iface_created_fut),
            Poll::Pending
        );

        let telemetry_support = Default::default();
        assert_eq!(
            test_helper.run_and_respond_query_telemetry_support(
                &mut handle_iface_created_fut,
                Ok(&telemetry_support)
            ),
            Poll::Ready(())
        );

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = Default::default();
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        let signal_report = fidl_stats::SignalReport {
            connection_signal_report: Some(fidl_stats::ConnectionSignalReport {
                primary: Some(fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 6,
                }),
                bandwidth: Some(fidl_ieee80211::ChannelBandwidth::Cbw20),
                vht_secondary_80_channel: Some(fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 0,
                }),
                tx_rate_500kbps: Some(11),
                rssi_dbm: Some(-40),
                snr_db: Some(25),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("wlan_channels")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0))] // 0 is the ID of the WLAN channel
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("tx_rate_500kbps")[..],
            &[TimeMatrixCall::Fold(Timed::now(11))]
        );
        assert_eq!(
            &time_matrix_calls.drain::<i64>("rssi")[..],
            &[TimeMatrixCall::Fold(Timed::now(-40))]
        );
        assert_eq!(
            &time_matrix_calls.drain::<i64>("snr")[..],
            &[TimeMatrixCall::Fold(Timed::now(25))]
        );

        let tree = test_helper.get_inspect_data_tree();
        assert_data_tree!(@executor test_helper.exec, tree, root: contains {
            test_stats: contains {
                metadata: contains {
                    wlan_channels: contains {
                        "0": {
                            "@time": AnyNumericProperty,
                            "data": contains {
                                primary: 6u64,
                                band: "TwoGhz",
                                cbw: "Cbw20",
                                secondary80_number: 0u64,
                                secondary80_band: "TwoGhz",
                            }
                        }
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_cobalt() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        // Transition to IfaceCreated state
        handle_iface_created(&mut test_helper, &logger);

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                rx_unicast_total: Some(100),
                rx_unicast_drop: Some(5),
                rx_multicast: Some(30),
                tx_total: Some(50),
                tx_drop: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert!(metrics.is_empty());

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                rx_unicast_total: Some(200),
                rx_unicast_drop: Some(15),
                rx_multicast: Some(30),
                tx_total: Some(150),
                tx_drop: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        assert_eq!(test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut), Poll::Pending);
        let signal_report: fidl_fuchsia_wlan_stats::SignalReport = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(1000)); // 10%
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(100));
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(100)); // 1%
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_cobalt_changed_connection_id() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        // Transition to IfaceCreated state
        handle_iface_created(&mut test_helper, &logger);

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                rx_unicast_total: Some(100),
                rx_unicast_drop: Some(5),
                rx_multicast: Some(30),
                tx_total: Some(50),
                tx_drop: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert!(metrics.is_empty());

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(2),
                rx_unicast_total: Some(200),
                rx_unicast_drop: Some(15),
                rx_multicast: Some(30),
                tx_total: Some(150),
                tx_drop: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        // No metric is logged because the ID indicates it's a different connection, meaning
        // there is nothing to diff with
        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert!(metrics.is_empty());

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(2),
                rx_unicast_total: Some(300),
                rx_unicast_drop: Some(18),
                rx_multicast: Some(30),
                tx_total: Some(250),
                tx_drop: Some(5),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        assert_eq!(test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut), Poll::Pending);
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(300)); // 3%
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(100));
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(200)); // 2%
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_cobalt_suspension_in_between() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        // Transition to IfaceCreated state
        handle_iface_created(&mut test_helper, &logger);

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                rx_unicast_total: Some(100),
                rx_unicast_drop: Some(5),
                rx_multicast: Some(30),
                tx_total: Some(50),
                tx_drop: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert!(metrics.is_empty());

        test_helper.exec.set_fake_boot_to_mono_offset(zx::BootDuration::from_millis(1));
        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                rx_unicast_total: Some(200),
                rx_unicast_drop: Some(15),
                rx_multicast: Some(30),
                tx_total: Some(150),
                tx_drop: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        // No metric is logged because the increase in boot-to-mono offset indicates that
        // a suspension had happened in between
        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert!(metrics.is_empty());

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        let iface_stats = fidl_stats::IfaceStats {
            connection_stats: Some(fidl_stats::ConnectionStats {
                connection_id: Some(1),
                rx_unicast_total: Some(300),
                rx_unicast_drop: Some(18),
                rx_multicast: Some(30),
                tx_total: Some(250),
                tx_drop: Some(5),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            test_helper.run_and_respond_iface_stats_req(&mut test_fut, Ok(&iface_stats)),
            Poll::Pending
        );
        assert_eq!(test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut), Poll::Pending);
        let signal_report = Default::default();
        assert_eq!(
            test_helper.run_and_respond_get_signal_report(&mut test_fut, Ok(&signal_report)),
            Poll::Ready(())
        );

        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(300)); // 3%
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(100));
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(200)); // 2%
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_get_iface_stats_failure_non_timeout() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        // Transition to IfaceCreated state
        handle_iface_created(&mut test_helper, &logger);

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(
            test_helper
                .run_and_respond_iface_stats_req(&mut test_fut, Err(zx::sys::ZX_ERR_TIMED_OUT)),
            Poll::Pending
        );
        assert_eq!(test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut), Poll::Pending);

        let metrics = test_helper.get_logged_metrics(metrics::GET_IFACE_STATS_FAILURE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        // Note that even though the response is ZX_ERR_TIMED_OUT, the `get_iface_stats_timeout`
        // metric is not logged because the metric is specifically about timeout waiting for a
        // response to `get_iface_stats` call.
        let metrics = test_helper.get_logged_metrics(metrics::GET_IFACE_STATS_TIMEOUT_METRIC_ID);
        assert!(metrics.is_empty());

        let metrics =
            test_helper.get_logged_metrics(metrics::GET_IFACE_STATS_ERROR_IN_RESPONSE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_handle_periodic_telemetry_get_iface_stats_timeout() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        // Transition to IfaceCreated state
        handle_iface_created(&mut test_helper, &logger);

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut), Poll::Pending);
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000_000_000));
        assert_eq!(test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut), Poll::Pending);

        let metrics = test_helper.get_logged_metrics(metrics::GET_IFACE_STATS_FAILURE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper.get_logged_metrics(metrics::GET_IFACE_STATS_TIMEOUT_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics =
            test_helper.get_logged_metrics(metrics::GET_IFACE_STATS_ERROR_IN_RESPONSE_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_diff_and_log_rx_cobalt() {
        let mut test_helper = setup_test();
        let prev_stats = fidl_stats::ConnectionStats {
            rx_unicast_total: Some(100),
            rx_unicast_drop: Some(5),
            ..Default::default()
        };
        let current_stats = fidl_stats::ConnectionStats {
            rx_unicast_total: Some(300),
            rx_unicast_drop: Some(7),
            ..Default::default()
        };
        let cobalt_proxy = test_helper.filtered_cobalt_logger();
        let mut test_fut = pin!(diff_and_log_rx_cobalt(&cobalt_proxy, &prev_stats, &current_stats));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(100)); // 1%
        let metrics = test_helper.get_logged_metrics(metrics::RX_UNICAST_PACKETS_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(200));
    }

    #[test_case(
        fidl_stats::ConnectionStats { ..Default::default() },
        fidl_stats::ConnectionStats { ..Default::default() };
        "both empty"
    )]
    #[test_case(
        fidl_stats::ConnectionStats {
            rx_unicast_total: Some(100),
            rx_unicast_drop: Some(5),
            ..Default::default()
        },
        fidl_stats::ConnectionStats { ..Default::default() };
        "current empty"
    )]
    #[test_case(
        fidl_stats::ConnectionStats { ..Default::default() },
        fidl_stats::ConnectionStats {
            rx_unicast_total: Some(300),
            rx_unicast_drop: Some(7),
            ..Default::default()
        };
        "prev empty"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_diff_and_log_rx_cobalt_empty(
        prev_stats: fidl_stats::ConnectionStats,
        current_stats: fidl_stats::ConnectionStats,
    ) {
        let mut test_helper = setup_test();
        let cobalt_proxy = test_helper.filtered_cobalt_logger();
        let mut test_fut = pin!(diff_and_log_rx_cobalt(&cobalt_proxy, &prev_stats, &current_stats));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert!(metrics.is_empty())
    }

    #[fuchsia::test]
    fn test_diff_and_log_tx_cobalt() {
        let mut test_helper = setup_test();
        let prev_stats = fidl_stats::ConnectionStats {
            tx_total: Some(100),
            tx_drop: Some(5),
            ..Default::default()
        };
        let current_stats = fidl_stats::ConnectionStats {
            tx_total: Some(300),
            tx_drop: Some(7),
            ..Default::default()
        };
        let cobalt_proxy = test_helper.filtered_cobalt_logger();
        let mut test_fut = pin!(diff_and_log_tx_cobalt(&cobalt_proxy, &prev_stats, &current_stats));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(100)); // 1%
    }

    #[test_case(
        fidl_stats::ConnectionStats { ..Default::default() },
        fidl_stats::ConnectionStats { ..Default::default() };
        "both empty"
    )]
    #[test_case(
        fidl_stats::ConnectionStats {
            tx_total: Some(100),
            tx_drop: Some(5),
            ..Default::default()
        },
        fidl_stats::ConnectionStats { ..Default::default() };
        "current empty"
    )]
    #[test_case(
        fidl_stats::ConnectionStats { ..Default::default() },
        fidl_stats::ConnectionStats {
            tx_total: Some(300),
            tx_drop: Some(7),
            ..Default::default()
        };
        "prev empty"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_diff_and_log_tx_cobalt_empty(
        prev_stats: fidl_stats::ConnectionStats,
        current_stats: fidl_stats::ConnectionStats,
    ) {
        let mut test_helper = setup_test();
        let cobalt_proxy = test_helper.filtered_cobalt_logger();
        let mut test_fut = pin!(diff_and_log_tx_cobalt(&cobalt_proxy, &prev_stats, &current_stats));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert!(metrics.is_empty())
    }

    #[test_case(
        fidl_stats::ConnectionStats {
            rx_unicast_total: Some(100),
            rx_unicast_drop: Some(5),
            tx_total: Some(100),
            tx_drop: Some(5),
            ..Default::default()
        },
        fidl_stats::ConnectionStats {
            rx_unicast_total: Some(10),
            rx_unicast_drop: Some(1),
            tx_total: Some(100),
            tx_drop: Some(5),
            ..Default::default()
        };
        "rx regressed"
    )]
    #[test_case(
        fidl_stats::ConnectionStats {
            tx_total: Some(100),
            tx_drop: Some(5),
            rx_unicast_total: Some(100),
            rx_unicast_drop: Some(5),
            ..Default::default()
        },
        fidl_stats::ConnectionStats {
            tx_total: Some(10),
            tx_drop: Some(1),
            rx_unicast_total: Some(100),
            rx_unicast_drop: Some(5),
            ..Default::default()
        };
        "tx regressed"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_diff_and_log_connection_stats_cobalt_counters_reset(
        prev_stats: fidl_stats::ConnectionStats,
        current_stats: fidl_stats::ConnectionStats,
    ) {
        let mut test_helper = setup_test();
        let cobalt_proxy = test_helper.filtered_cobalt_logger();
        let mut test_fut =
            pin!(diff_and_log_connection_stats_cobalt(&cobalt_proxy, &prev_stats, &current_stats));
        // Verify no crash
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        // Verify neither RX nor TX metrics are logged
        let metrics = test_helper.get_logged_metrics(metrics::BAD_RX_RATE_METRIC_ID);
        assert!(metrics.is_empty());
        let metrics = test_helper.get_logged_metrics(metrics::BAD_TX_RATE_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_handle_iface_destroyed() {
        let mut test_helper = setup_test();
        let driver_counters_mock_matrix_client = MockTimeMatrixClient::new();
        let driver_gauges_mock_matrix_client = MockTimeMatrixClient::new();
        let logger = ClientIfaceCountersLogger::new(
            test_helper.filtered_cobalt_logger(),
            test_helper.monitor_svc_proxy.clone(),
            &test_helper.inspect_metadata_node,
            &test_helper.inspect_metadata_path,
            &test_helper.mock_time_matrix_client,
            driver_counters_mock_matrix_client.clone(),
            driver_gauges_mock_matrix_client.clone(),
        );

        // Transition to IfaceCreated state
        handle_iface_created(&mut test_helper, &logger);

        let mut handle_iface_destroyed_fut = pin!(logger.handle_iface_destroyed(IFACE_ID));
        assert_eq!(
            test_helper.exec.run_until_stalled(&mut handle_iface_destroyed_fut),
            Poll::Ready(())
        );

        let mut test_fut = pin!(logger.handle_periodic_telemetry());
        assert_eq!(test_helper.exec.run_until_stalled(&mut test_fut), Poll::Ready(()));
        let telemetry_svc_stream = test_helper.telemetry_svc_stream.as_mut().unwrap();
        let mut telemetry_svc_req_fut = pin!(telemetry_svc_stream.try_next());
        // Verify that no telemetry request is made now that the iface is destroyed
        match test_helper.exec.run_until_stalled(&mut telemetry_svc_req_fut) {
            Poll::Ready(Ok(None)) => (),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    fn handle_iface_created<S: InspectSender>(
        test_helper: &mut TestHelper,
        logger: &ClientIfaceCountersLogger<S>,
    ) {
        let mut handle_iface_created_fut = pin!(logger.handle_iface_created(IFACE_ID));
        assert_eq!(
            test_helper.run_and_handle_get_sme_telemetry(&mut handle_iface_created_fut),
            Poll::Pending
        );
        let telemetry_support = fidl_stats::TelemetrySupport::default();
        assert_eq!(
            test_helper.run_and_respond_query_telemetry_support(
                &mut handle_iface_created_fut,
                Ok(&telemetry_support)
            ),
            Poll::Ready(())
        );
    }
}
