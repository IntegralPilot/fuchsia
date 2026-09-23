// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Context as _, Error, format_err};
use fidl_fuchsia_power_battery as fidl_battery;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fuchsia_async as fasync;
use fuchsia_inspect::Node as InspectNode;
use futures::channel::mpsc;
use futures::{Future, StreamExt, select};
use log::error;
use std::boxed::Box;
use std::sync::Arc;
use windowed_stats::experimental::inspect::TimeMatrixClient;
use wlan_common::bss::BssDescription;
use wlan_legacy_metrics_registry as metrics;

mod config;
mod convert;
mod processors;
pub(crate) mod util;
pub use crate::config::{CobaltAllowlist, DeviceMobility, TelemetryConfig};
pub use crate::processors::connect_disconnect::DisconnectInfo;
pub use crate::processors::pno_scan::PnoScanDisabledReason;
pub use crate::processors::power::{IfacePowerLevel, UnclearPowerDemand};
pub use crate::processors::scan::ScanResult;
pub use crate::processors::sme_timeout::TimeoutSource;
pub use crate::processors::toggle_events::ClientConnectionsToggleEvent;
pub use util::sender::TelemetrySender;
#[cfg(test)]
mod testing;

#[derive(Debug)]
pub enum TelemetryEvent {
    ConnectResult {
        result: fidl_ieee80211::StatusCode,
        bss: Box<BssDescription>,
        is_credential_rejected: bool,
        is_owe_transition: bool,
    },
    Disconnect {
        info: DisconnectInfo,
    },
    ChannelSwitched {
        channel: wlan_common::channel::Channel,
    },
    // We should maintain docstrings if we can see any possibility of ambiguity for an enum
    /// Client connections enabled or disabled
    ClientConnectionsToggle {
        event: ClientConnectionsToggleEvent,
    },
    ClientIfaceCreated {
        iface_id: u16,
    },
    ClientIfaceDestroyed {
        iface_id: u16,
    },
    IfaceCreationFailure,
    IfaceDestructionFailure,
    ScanStart,
    ScanResult {
        result: ScanResult,
    },
    IfacePowerLevelChanged {
        iface_power_level: IfacePowerLevel,
        iface_id: u16,
    },
    /// System suspension imminent
    SuspendImminent,
    /// Unclear power level requested by policy layer
    UnclearPowerDemand(UnclearPowerDemand),
    BatteryChargeStatus(fidl_battery::ChargeStatus),
    RecoveryEvent,
    RecoveryResult {
        result: Result<(), ()>,
    },
    SmeTimeout {
        source: TimeoutSource,
    },
    ChipPowerUpFailure,
    ChipPowerDownFailure,
    ResetTxPowerScenario,
    SetTxPowerScenario {
        scenario: fidl_internal::TxPowerScenario,
    },
    PnoScanFailure,
    PnoScanEnabled,
    PnoScanResultsReceived,
    PnoScanDisabled {
        reason: PnoScanDisabledReason,
    },
}

/// If metrics cannot be reported for extended periods of time, logging new metrics will fail and
/// the error messages tend to clutter up the logs.  This container limits the rate at which such
/// potentially noisy logs are reported.  Duplicate error messages are aggregated periodically
/// reported.
pub struct ThrottledErrorLogger {
    time_of_last_log: fasync::MonotonicInstant,
    pub suppressed_errors: std::collections::HashMap<String, usize>,
    minutes_between_reports: i64,
}

impl ThrottledErrorLogger {
    pub fn new(minutes_between_reports: i64) -> Self {
        Self {
            time_of_last_log: fasync::MonotonicInstant::from_nanos(0),
            suppressed_errors: std::collections::HashMap::new(),
            minutes_between_reports,
        }
    }

    pub fn throttle_log(&mut self, message: String, level: log::Level) {
        let curr_time = fasync::MonotonicInstant::now();
        let time_since_last_log = curr_time - self.time_of_last_log;

        if time_since_last_log.into_minutes() > self.minutes_between_reports {
            log::log!(level, "{}", message);
            if !self.suppressed_errors.is_empty() {
                for (log, count) in self.suppressed_errors.iter() {
                    log::warn!("Suppressed {} instances: {}", count, log);
                }
                self.suppressed_errors.clear();
            }
            self.time_of_last_log = curr_time;
        } else {
            let count = self.suppressed_errors.entry(message).or_default();
            *count += 1;
        }
    }

    pub fn throttle_error(&mut self, result: Result<(), Error>) {
        if let Err(e) = result {
            self.throttle_log(e.to_string(), log::Level::Error);
        }
    }
}

/// Attempts to connect to the Cobalt service.
pub async fn setup_cobalt_proxy()
-> Result<fidl_fuchsia_metrics::MetricEventLoggerProxy, anyhow::Error> {
    let cobalt_svc = fuchsia_component::client::connect_to_protocol::<
        fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker,
    >()
    .context("failed to connect to metrics service")?;

    let (cobalt_proxy, cobalt_server) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_metrics::MetricEventLoggerMarker>();

    let project_spec = fidl_fuchsia_metrics::ProjectSpec {
        customer_id: Some(metrics::CUSTOMER_ID),
        project_id: Some(metrics::PROJECT_ID),
        ..Default::default()
    };

    match cobalt_svc.create_metric_event_logger(&project_spec, cobalt_server).await {
        Ok(_) => Ok(cobalt_proxy),
        Err(err) => Err(format_err!("failed to create metrics event logger: {:?}", err)),
    }
}

/// Attempts to create a disconnected FIDL channel with types matching the Cobalt service. This
/// allows for a fallback with a uniform code path in case of a failure to connect to Cobalt.
pub fn setup_disconnected_cobalt_proxy()
-> Result<fidl_fuchsia_metrics::MetricEventLoggerProxy, anyhow::Error> {
    // Create a disconnected proxy
    Ok(fidl::endpoints::create_proxy::<fidl_fuchsia_metrics::MetricEventLoggerMarker>().0)
}

/// How often to refresh time series stats. Also how often to request packet counters.
const TELEMETRY_QUERY_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(10);

pub fn serve_telemetry(
    cobalt_proxy: fidl_fuchsia_metrics::MetricEventLoggerProxy,
    monitor_svc_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
    inspect_node: InspectNode,
    inspect_path: &str,
    config: TelemetryConfig,
    allowlist: CobaltAllowlist,
) -> (TelemetrySender, impl Future<Output = Result<(), Error>> + use<>) {
    let (sender, mut receiver) =
        mpsc::channel::<TelemetryEvent>(util::sender::TELEMETRY_EVENT_BUFFER_SIZE);
    let sender = TelemetrySender::new(sender);

    let cobalt_logger =
        Arc::new(util::cobalt_logger::FilteredCobaltLogger::new(cobalt_proxy, allowlist));

    // Inspect nodes to hold time series and metadata for other nodes
    const METADATA_NODE_NAME: &str = "metadata";
    let inspect_metadata_node = inspect_node.create_child(METADATA_NODE_NAME);
    let inspect_metadata_path = format!("{inspect_path}/{METADATA_NODE_NAME}");
    let inspect_time_series_node = inspect_node.create_child("time_series");

    let time_matrix_client = TimeMatrixClient::new(inspect_time_series_node.clone_weak());

    // Create and initialize modules
    let connect_disconnect = config.enable_connect_disconnect.then(|| {
        processors::connect_disconnect::ConnectDisconnectLogger::new(
            cobalt_logger.clone(),
            &inspect_node,
            &inspect_metadata_node,
            &inspect_metadata_path,
            &time_matrix_client,
            config.device_mobility,
        )
    });
    let iface_logger = config
        .enable_iface_logger
        .then(|| processors::iface::IfaceLogger::new(cobalt_logger.clone()));
    let power_logger = config
        .enable_power_logger
        .then(|| processors::power::PowerLogger::new(cobalt_logger.clone(), &inspect_node));
    let recovery_logger = config
        .enable_recovery_logger
        .then(|| processors::recovery::RecoveryLogger::new(cobalt_logger.clone()));
    let mut scan_logger = config
        .enable_scan_logger
        .then(|| processors::scan::ScanLogger::new(cobalt_logger.clone(), &time_matrix_client));
    let mut pno_scan_logger = config
        .enable_pno_scan_logger
        .then(|| processors::pno_scan::PnoScanLogger::new(cobalt_logger.clone()));
    let sme_timeout_logger = config
        .enable_sme_timeout_logger
        .then(|| processors::sme_timeout::SmeTimeoutLogger::new(cobalt_logger.clone()));
    let mut toggle_logger = config.enable_toggle_logger.then(|| {
        processors::toggle_events::ToggleLogger::new(cobalt_logger.clone(), &inspect_node)
    });
    let tx_power_scenario_logger = config
        .enable_tx_power_scenario_logger
        .then(|| processors::tx_power_scenario::TxPowerScenarioLogger::new(cobalt_logger.clone()));

    let client_iface_counters_logger = config.enable_client_iface_counters_logger.then(|| {
        let driver_specific_time_series_node =
            inspect_time_series_node.create_child("driver_specific");
        let driver_counters_time_series_node =
            driver_specific_time_series_node.create_child("counters");
        let driver_gauges_time_series_node =
            driver_specific_time_series_node.create_child("gauges");

        let driver_counters_time_series_client =
            TimeMatrixClient::new(driver_counters_time_series_node.clone_weak());
        let driver_gauges_time_series_client =
            TimeMatrixClient::new(driver_gauges_time_series_node.clone_weak());

        inspect_time_series_node.record(driver_specific_time_series_node);
        inspect_time_series_node.record(driver_counters_time_series_node);
        inspect_time_series_node.record(driver_gauges_time_series_node);

        processors::client_iface_counters::ClientIfaceCountersLogger::new(
            cobalt_logger.clone(),
            monitor_svc_proxy,
            &inspect_metadata_node,
            &inspect_metadata_path,
            &time_matrix_client,
            driver_counters_time_series_client,
            driver_gauges_time_series_client,
        )
    });

    let fut = async move {
        // Prevent the inspect nodes from being dropped while the loop is running.
        let _inspect_node = inspect_node;
        let _inspect_metadata_node = inspect_metadata_node;
        let _inspect_time_series_node = inspect_time_series_node;

        let mut telemetry_interval = fasync::Interval::new(TELEMETRY_QUERY_INTERVAL);
        loop {
            select! {
                event = receiver.next() => {
                    let Some(event) = event else {
                        error!("Telemetry event stream unexpectedly terminated.");
                        return Err(format_err!("Telemetry event stream unexpectedly terminated."));
                    };
                    use TelemetryEvent::*;
                    match event {
                        ConnectResult { result, bss, is_credential_rejected, is_owe_transition } => {
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_connect_attempt(result, &bss, is_credential_rejected, is_owe_transition).await;
                            }
                        }
                        Disconnect { info } => {
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.log_disconnect(&info).await;
                            }
                            if let Some(ref power_logger) = power_logger {
                                power_logger.handle_iface_disconnect(info.iface_id).await;
                            }
                        }
                        ChannelSwitched { channel } => {
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_channel_switched(channel).await;
                            }
                        }
                        ClientConnectionsToggle { event } => {
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_client_connections_toggle(&event).await;
                            }
                            if let Some(ref mut toggle_logger) = toggle_logger {
                                toggle_logger.handle_toggle_event(event).await;
                            }
                        }
                        ClientIfaceCreated { iface_id } => {
                            if let Some(ref client_iface_counters_logger) = client_iface_counters_logger {
                                client_iface_counters_logger.handle_iface_created(iface_id).await;
                            }
                        }
                        ClientIfaceDestroyed { iface_id } => {
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_iface_destroyed().await;
                            }
                            if let Some(ref client_iface_counters_logger) = client_iface_counters_logger {
                                client_iface_counters_logger.handle_iface_destroyed(iface_id).await;
                            }
                            if let Some(ref power_logger) = power_logger {
                                power_logger.handle_iface_destroyed(iface_id).await;
                            }
                        }
                        IfaceCreationFailure => {
                            if let Some(ref iface_logger) = iface_logger {
                                iface_logger.handle_iface_creation_failure().await;
                            }
                        }
                        IfaceDestructionFailure => {
                            if let Some(ref iface_logger) = iface_logger {
                                iface_logger.handle_iface_destruction_failure().await;
                            }
                        }
                        ScanStart => {
                            if let Some(ref mut scan_logger) = scan_logger {
                                scan_logger.handle_scan_start().await;
                            }
                        }
                        ScanResult { result } => {
                            if let Some(ref mut scan_logger) = scan_logger {
                                scan_logger.handle_scan_result(result).await;
                            }
                        }
                        IfacePowerLevelChanged { iface_power_level, iface_id } => {
                            if let Some(ref power_logger) = power_logger {
                                power_logger.log_iface_power_event(iface_power_level, iface_id).await;
                            }
                        }
                        // TODO(b/340921554): either watch for suspension directly in the library,
                        // or plumb this from callers once suspend mechanisms are integrated
                        SuspendImminent => {
                            if let Some(ref power_logger) = power_logger {
                                power_logger.handle_suspend_imminent().await;
                            }
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_suspend_imminent().await;
                            }
                        }
                        UnclearPowerDemand(demand) => {
                            if let Some(ref power_logger) = power_logger {
                                power_logger.handle_unclear_power_demand(demand).await;
                            }
                        }
                        ChipPowerUpFailure => {
                            if let Some(ref power_logger) = power_logger {
                                power_logger.handle_chip_power_up_failure().await;
                            }
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_client_connections_failed_to_start().await;
                            }
                        }
                        ChipPowerDownFailure => {
                            if let Some(ref power_logger) = power_logger {
                                power_logger.chip_power_down_failure().await;
                            }
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_client_connections_failed_to_stop().await;
                            }
                        }
                        BatteryChargeStatus(charge_status) => {
                            if let Some(ref mut scan_logger) = scan_logger {
                                scan_logger.handle_battery_charge_status(charge_status).await;
                            }
                            if let Some(ref mut toggle_logger) = toggle_logger {
                                toggle_logger.handle_battery_charge_status(charge_status).await;
                            }
                        }
                        RecoveryEvent => {
                            if let Some(ref recovery_logger) = recovery_logger {
                                recovery_logger.handle_recovery_event().await;
                            }
                        }
                        RecoveryResult { result } => {
                            if let Some(ref recovery_logger) = recovery_logger {
                                recovery_logger.handle_recovery_result(result).await;
                            }
                        }
                        SmeTimeout { source } => {
                            if let Some(ref sme_timeout_logger) = sme_timeout_logger {
                                sme_timeout_logger.handle_sme_timeout_event(source).await;
                            }
                        }
                        ResetTxPowerScenario => {
                            if let Some(ref tx_power_scenario_logger) = tx_power_scenario_logger {
                                tx_power_scenario_logger.handle_sar_reset().await;
                            }
                        }
                        SetTxPowerScenario {scenario} => {
                            if let Some(ref tx_power_scenario_logger) = tx_power_scenario_logger {
                                tx_power_scenario_logger.handle_set_sar(scenario).await;
                            }
                        }
                        PnoScanFailure => {
                            if let Some(ref connect_disconnect) = connect_disconnect {
                                connect_disconnect.handle_pno_scan_failure().await;
                            }
                        }
                        PnoScanEnabled => {
                            if let Some(ref mut pno_scan_logger) = pno_scan_logger {
                                let is_connected = connect_disconnect
                                    .as_ref()
                                    .map(|cd| cd.is_connected())
                                    .unwrap_or(false);
                                pno_scan_logger.handle_pno_scan_enabled(is_connected).await;
                            }
                        }
                        PnoScanResultsReceived => {
                            if let Some(ref mut pno_scan_logger) = pno_scan_logger {
                                pno_scan_logger.handle_pno_scan_results_received().await;
                            }
                        }
                        PnoScanDisabled { reason } => {
                            if let Some(ref mut pno_scan_logger) = pno_scan_logger {
                                pno_scan_logger.handle_pno_scan_disabled(reason).await;
                            }
                        }
                    }
                }
                _ = telemetry_interval.next() => {
                    if let Some(ref connect_disconnect) = connect_disconnect {
                        connect_disconnect.handle_periodic_telemetry().await;
                    }
                    if let Some(ref client_iface_counters_logger) = client_iface_counters_logger {
                        client_iface_counters_logger.handle_periodic_telemetry().await;
                    }
                    if let Some(ref mut pno_scan_logger) = pno_scan_logger {
                        pno_scan_logger.handle_periodic_telemetry().await;
                    }
                }
            }
        }
    };
    (sender, fut)
}
