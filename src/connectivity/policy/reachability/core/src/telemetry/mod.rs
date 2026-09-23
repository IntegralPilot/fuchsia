// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod convert;
pub mod processors;
use crate::fetch::FetchError;
use crate::ping::PingError;
use crate::telemetry::processors::link_properties_state::{
    LinkProperties, LinkPropertiesStateLogger,
};
use crate::telemetry::processors::{
    InterfaceIdentifier, InterfaceTimeSeriesGrouping, InterfaceType,
};
use crate::{FetchParameters, IpVersions, LinkState, PingParameters, State};
use processors::interface_aware_logger::InterfaceAwareLogger;

use anyhow::{Context, Error, format_err};
use cobalt_client::traits::AsEventCode;
use fidl_fuchsia_metrics::MetricEvent;
use fuchsia_async as fasync;
use fuchsia_cobalt_builders::MetricEventExt;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::{Future, StreamExt, select};
use log::{info, warn};
use network_policy_metrics_registry as metrics;
use static_assertions::const_assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windowed_stats::experimental::inspect::TimeMatrixClient;

#[cfg(test)]
mod testing;

pub async fn create_metrics_logger(
    factory_proxy: fidl_fuchsia_metrics::MetricEventLoggerFactoryProxy,
) -> Result<fidl_fuchsia_metrics::MetricEventLoggerProxy, Error> {
    let (cobalt_proxy, cobalt_server) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_metrics::MetricEventLoggerMarker>();

    let project_spec = fidl_fuchsia_metrics::ProjectSpec {
        customer_id: None, // defaults to fuchsia.
        project_id: Some(metrics::PROJECT_ID),
        ..Default::default()
    };

    let status = factory_proxy
        .create_metric_event_logger(&project_spec, cobalt_server)
        .await
        .context("create metrics event logger")?;

    match status {
        Ok(_) => Ok(cobalt_proxy),
        Err(err) => Err(format_err!("failed to create metrics event logger: {:?}", err)),
    }
}

#[derive(Clone, Debug)]
pub struct TelemetrySender {
    sender: Arc<Mutex<mpsc::Sender<TelemetryEvent>>>,
    sender_is_blocked: Arc<AtomicBool>,
}

impl TelemetrySender {
    pub fn new(sender: mpsc::Sender<TelemetryEvent>) -> Self {
        Self {
            sender: Arc::new(Mutex::new(sender)),
            sender_is_blocked: Arc::new(AtomicBool::new(false)),
        }
    }

    // Send telemetry event. Log an error if it fails.
    pub fn send(&self, event: TelemetryEvent) {
        match self.sender.lock().try_send(event) {
            Ok(_) => {
                // If sender has been blocked before, set bool to false and log message.
                if self
                    .sender_is_blocked
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    info!("TelemetrySender recovered and resumed sending");
                }
            }
            Err(_) => {
                // If sender has not been blocked before, set bool to true and log error message.
                if self
                    .sender_is_blocked
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    warn!(
                        "TelemetrySender dropped a msg: either buffer is full or no receiver is waiting"
                    );
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct SystemStateSummary {
    system_state: IpVersions<Option<State>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct NetworkConfig {
    has_default_ipv4_route: bool,
    has_default_ipv6_route: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SystemStateUpdate {
    pub(crate) system_state: IpVersions<Option<State>>,
}

#[derive(Debug)]
pub enum TelemetryEvent {
    SystemStateUpdate {
        update: SystemStateUpdate,
    },
    NetworkConfig {
        has_default_ipv4_route: bool,
        has_default_ipv6_route: bool,
    },
    GatewayProbe {
        internet_available: bool,
        gateway_discoverable: bool,
        gateway_pingable: bool,
    },
    // The LinkProperties update corresponding to the interface described by
    // the vector of interface identifiers.
    LinkPropertiesUpdate {
        interface_identifiers: Vec<InterfaceIdentifier>,
        link_properties: IpVersions<LinkProperties>,
    },
    // The LinkState update corresponding to the interface described by the
    // vector of interface identifiers.
    LinkStateUpdate {
        interface_identifiers: Vec<InterfaceIdentifier>,
        link_state: IpVersions<LinkState>,
    },
    GatewayPingResult {
        interface_identifiers: Vec<InterfaceIdentifier>,
        ping_parameters: PingParameters,
        gateway_ping_result: Result<(), PingError>,
    },
    InternetPingResult {
        interface_identifiers: Vec<InterfaceIdentifier>,
        ping_parameters: PingParameters,
        internet_ping_result: Result<(), PingError>,
    },
    FetchResult {
        interface_identifiers: Vec<InterfaceIdentifier>,
        fetch_parameters: FetchParameters,
        fetch_result: Result<u16, FetchError>,
    },
}

/// Capacity of "first come, first serve" slots available to clients of
/// the mpsc::Sender<TelemetryEvent>. This threshold is arbitrary.
const TELEMETRY_EVENT_BUFFER_SIZE: usize = 100;

const TELEMETRY_QUERY_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(10);
const METADATA_NODE_NAME: &str = "metadata";

pub fn serve_telemetry(
    cobalt_proxy: fidl_fuchsia_metrics::MetricEventLoggerProxy,
    inspect_node: InspectNode,
) -> (TelemetrySender, impl Future<Output = Result<(), Error>>) {
    // Inspect nodes to hold Inspect events
    let inspect_events_node = inspect_node.create_child("events");

    // Inspect nodes to hold time series and metadata for other nodes.
    let inspect_time_series_node = inspect_node.create_child("time_series");
    let link_properties_state_time_series_node =
        inspect_time_series_node.create_child("link_properties_state");
    let time_matrix_client =
        TimeMatrixClient::new(link_properties_state_time_series_node.clone_weak());

    inspect_node.record(link_properties_state_time_series_node);
    let inspect_metadata_node = inspect_node.create_child(METADATA_NODE_NAME);
    let link_properties_state_logger = LinkPropertiesStateLogger::new(
        &inspect_metadata_node,
        &format!("root/telemetry/{METADATA_NODE_NAME}"),
        InterfaceTimeSeriesGrouping::Type(vec![InterfaceType::Ethernet, InterfaceType::WlanClient]),
        &time_matrix_client,
    );

    let interface_aware_events_node = inspect_events_node.create_child("interfaces");
    let interface_aware_time_series_node = inspect_time_series_node.create_child("interfaces");
    let interface_aware_logger = InterfaceAwareLogger::new(
        &inspect_metadata_node,
        &format!("root/telemetry/{METADATA_NODE_NAME}"),
        InterfaceTimeSeriesGrouping::Type(vec![InterfaceType::Ethernet, InterfaceType::WlanClient]),
        interface_aware_events_node,
        interface_aware_time_series_node,
    );

    // Record the time series and metadata nodes so they do not get dropped.
    inspect_node.record(inspect_events_node);
    inspect_node.record(inspect_time_series_node);
    inspect_node.record(inspect_metadata_node);

    let (sender, fut) = serve_telemetry_inner(
        cobalt_proxy,
        inspect_node,
        link_properties_state_logger,
        interface_aware_logger,
    );
    (sender, fut)
}

fn serve_telemetry_inner(
    cobalt_proxy: fidl_fuchsia_metrics::MetricEventLoggerProxy,
    inspect_node: InspectNode,
    link_properties_state_logger: LinkPropertiesStateLogger,
    interface_aware_logger: InterfaceAwareLogger,
) -> (TelemetrySender, impl Future<Output = Result<(), Error>>) {
    let (sender, mut receiver) = mpsc::channel::<TelemetryEvent>(TELEMETRY_EVENT_BUFFER_SIZE);
    let sender = TelemetrySender::new(sender);
    let cloned_sender = sender.clone();

    let fut = async move {
        let link_properties_state_logger = link_properties_state_logger;

        let mut report_interval_stream = fasync::Interval::new(TELEMETRY_QUERY_INTERVAL);
        const ONE_MINUTE: zx::MonotonicDuration = zx::MonotonicDuration::from_minutes(1);
        const_assert_eq!(ONE_MINUTE.into_nanos() % TELEMETRY_QUERY_INTERVAL.into_nanos(), 0);
        const INTERVAL_TICKS_PER_MINUTE: u64 =
            (ONE_MINUTE.into_nanos() / TELEMETRY_QUERY_INTERVAL.into_nanos()) as u64;
        const INTERVAL_TICKS_PER_HR: u64 = INTERVAL_TICKS_PER_MINUTE * 60;
        let mut interval_tick = 0u64;
        let mut telemetry = Telemetry::new(
            cloned_sender,
            cobalt_proxy,
            link_properties_state_logger,
            interface_aware_logger,
            inspect_node,
        );
        loop {
            select! {
                event = receiver.next() => {
                    if let Some(event) = event {
                        telemetry.handle_telemetry_event(event).await;
                    }
                }
                _ = report_interval_stream.next() => {
                    interval_tick += 1;
                    if interval_tick % INTERVAL_TICKS_PER_HR == 0 {
                        telemetry.handle_hourly_telemetry().await;
                    }
                }
            }
        }
    };
    (sender, fut)
}

// Macro wrapper for logging simple events (occurrence, integer, histogram, string)
// and log a warning when the status is not Ok.
macro_rules! log_cobalt {
    ($cobalt_proxy:expr, $method_name:ident, $metric_id:expr, $value:expr, $event_codes:expr $(,)?) => {{
        let status = $cobalt_proxy.$method_name($metric_id, $value, $event_codes).await;
        match status {
            Ok(Ok(())) => (),
            Ok(Err(e)) => warn!("Failed logging metric: {}, error: {:?}", $metric_id, e),
            Err(e) => warn!("Failed logging metric: {}, error: {}", $metric_id, e),
        }
    }};
}

macro_rules! log_cobalt_batch {
    ($cobalt_proxy:expr, $events:expr, $context:expr $(,)?) => {{
        let status = $cobalt_proxy.log_metric_events($events).await;
        match status {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                warn!("Failed logging batch metrics, context: {}, error: {:?}", $context, e)
            }
            Err(e) => warn!("Failed logging batch metrics, context: {}, error: {}", $context, e),
        }
    }};
}

struct Telemetry {
    cobalt_proxy: fidl_fuchsia_metrics::MetricEventLoggerProxy,
    state_summary: Option<SystemStateSummary>,
    state_last_refreshed_for_cobalt: fasync::MonotonicInstant,
    state_last_refreshed_for_inspect: fasync::MonotonicInstant,
    reachability_lost_at: Option<(
        fasync::MonotonicInstant,
        metrics::ReachabilityGlobalSnapshotDurationMetricDimensionRouteConfig,
    )>,
    network_config: Option<NetworkConfig>,
    network_config_last_refreshed: fasync::MonotonicInstant,
    link_properties_state_logger: LinkPropertiesStateLogger,
    interface_aware_logger: InterfaceAwareLogger,

    _inspect_node: InspectNode,
}

impl Telemetry {
    pub fn new(
        _telemetry_sender: TelemetrySender,
        cobalt_proxy: fidl_fuchsia_metrics::MetricEventLoggerProxy,
        link_properties_state_logger: LinkPropertiesStateLogger,
        interface_aware_logger: InterfaceAwareLogger,
        inspect_node: InspectNode,
    ) -> Self {
        Self {
            cobalt_proxy,
            state_summary: None,
            state_last_refreshed_for_cobalt: fasync::MonotonicInstant::now(),
            state_last_refreshed_for_inspect: fasync::MonotonicInstant::now(),
            reachability_lost_at: None,
            network_config: None,
            network_config_last_refreshed: fasync::MonotonicInstant::now(),
            link_properties_state_logger,
            interface_aware_logger,
            _inspect_node: inspect_node,
        }
    }

    pub async fn handle_telemetry_event(&mut self, event: TelemetryEvent) {
        let now = fasync::MonotonicInstant::now();
        match event {
            TelemetryEvent::SystemStateUpdate { update: SystemStateUpdate { system_state } } => {
                let new_state = Some(match &self.state_summary {
                    Some(summary) => SystemStateSummary { system_state, ..summary.clone() },
                    None => SystemStateSummary { system_state, ..SystemStateSummary::default() },
                });
                if self.state_summary != new_state {
                    // Only log if system state has changed to prevent spamming Cobalt,
                    // since this telemetry event may happen in quick succession.
                    self.log_system_state_metrics(
                        new_state,
                        now,
                        "handle_telemetry_event(TelemetryEvent::SystemStateUpdate)",
                    )
                    .await
                }
            }
            TelemetryEvent::NetworkConfig { has_default_ipv4_route, has_default_ipv6_route } => {
                let new_config =
                    Some(NetworkConfig { has_default_ipv4_route, has_default_ipv6_route });
                self.log_network_config_metrics(
                    new_config,
                    now,
                    "handle_telemetry_event(TelemetryEvent::NetworkConfig)",
                )
                .await;
            }
            TelemetryEvent::GatewayProbe {
                internet_available,
                gateway_discoverable,
                gateway_pingable,
            } => {
                if internet_available {
                    let metric_id = match (gateway_discoverable, gateway_pingable) {
                        (true, true) => None,
                        (true, false) => {
                            Some(metrics::INTERNET_AVAILABLE_GATEWAY_NOT_PINGABLE_METRIC_ID)
                        }
                        (false, true) => {
                            Some(metrics::INTERNET_AVAILABLE_GATEWAY_NOT_DISCOVERABLE_METRIC_ID)
                        }
                        (false, false) => Some(metrics::INTERNET_AVAILABLE_GATEWAY_LOST_METRIC_ID),
                    };
                    if let Some(metric_id) = metric_id {
                        log_cobalt!(self.cobalt_proxy, log_occurrence, metric_id, 1, &[]);
                    }
                }
            }
            TelemetryEvent::LinkPropertiesUpdate { interface_identifiers, link_properties } => {
                self.link_properties_state_logger
                    .update_link_properties(interface_identifiers, &link_properties);
            }
            TelemetryEvent::LinkStateUpdate { interface_identifiers, link_state } => {
                self.link_properties_state_logger
                    .update_link_state(interface_identifiers, &link_state);
            }
            TelemetryEvent::GatewayPingResult {
                interface_identifiers,
                ping_parameters,
                gateway_ping_result,
            } => {
                self.interface_aware_logger.log_gateway_ping_result(
                    interface_identifiers,
                    &ping_parameters,
                    &gateway_ping_result,
                );
            }
            TelemetryEvent::InternetPingResult {
                interface_identifiers,
                ping_parameters,
                internet_ping_result,
            } => {
                self.interface_aware_logger.log_internet_ping_result(
                    interface_identifiers,
                    &ping_parameters,
                    &internet_ping_result,
                );
            }
            TelemetryEvent::FetchResult {
                interface_identifiers,
                fetch_parameters,
                fetch_result,
            } => {
                self.interface_aware_logger.log_fetch_result(
                    interface_identifiers,
                    &fetch_parameters,
                    &fetch_result,
                );
            }
        }
    }

    async fn log_system_state_metrics(
        &mut self,
        new_state: Option<SystemStateSummary>,
        now: fasync::MonotonicInstant,
        ctx: &'static str,
    ) {
        let duration_cobalt = now - self.state_last_refreshed_for_cobalt;
        let mut metric_events = vec![];

        if let Some(prev) = &self.state_summary {
            if prev.system_state.has_interface_up() {
                metric_events.push(
                    MetricEvent::builder(
                        metrics::REACHABILITY_STATE_UP_OR_ABOVE_DURATION_METRIC_ID,
                    )
                    .as_integer(duration_cobalt.into_micros()),
                );

                let route_config_dim = convert::convert_route_config(&prev.system_state);
                let internet_available =
                    convert::convert_yes_no_dim(prev.system_state.has_internet());
                let gateway_reachable =
                    convert::convert_yes_no_dim(prev.system_state.has_gateway());
                let dns_active = convert::convert_yes_no_dim(prev.system_state.has_dns());
                let http_status = if prev.system_state.has_http() {
                    metrics::NetworkPolicyMetricDimensionHttpStatus::HttpOnly
                } else {
                    metrics::NetworkPolicyMetricDimensionHttpStatus::Neither
                };
                // We only log metric when at least one of IPv4 or IPv6 is in the SystemState.
                if let Some(route_config) = route_config_dim {
                    metric_events.push(
                        MetricEvent::builder(
                            metrics::REACHABILITY_GLOBAL_SNAPSHOT_DURATION_METRIC_ID,
                        )
                        .with_event_codes(metrics::ReachabilityGlobalSnapshotDurationEventCodes {
                            route_config,
                            internet_available,
                            gateway_reachable,
                            dns_active,
                            http_status,
                        })
                        .as_integer(duration_cobalt.into_micros()),
                    );
                }
            }
        }

        if let (Some(prev), Some(new_state)) = (&self.state_summary, &new_state) {
            let previously_reachable =
                prev.system_state.has_internet() && prev.system_state.has_dns();
            let now_reachable =
                new_state.system_state.has_internet() && new_state.system_state.has_dns();
            if previously_reachable && !now_reachable {
                let route_config_dim = convert::convert_route_config(&prev.system_state);
                if let Some(route_config_dim) = route_config_dim {
                    metric_events.push(
                        MetricEvent::builder(metrics::REACHABILITY_LOST_METRIC_ID)
                            .with_event_code(route_config_dim.as_event_code())
                            .as_occurrence(1),
                    );
                    self.reachability_lost_at = Some((now, route_config_dim));
                }
            }

            if !previously_reachable && now_reachable {
                if let Some((reachability_lost_at, route_config_dim)) = self.reachability_lost_at {
                    metric_events.push(
                        MetricEvent::builder(metrics::REACHABILITY_LOST_DURATION_METRIC_ID)
                            .with_event_code(route_config_dim.as_event_code())
                            .as_integer((now - reachability_lost_at).into_micros()),
                    );
                }
                self.reachability_lost_at = None;
            }
        }

        if !metric_events.is_empty() {
            log_cobalt_batch!(self.cobalt_proxy, &metric_events, ctx);
        }

        if let Some(new_state) = new_state {
            self.state_summary = Some(new_state);
        }
        self.state_last_refreshed_for_cobalt = now;
        self.state_last_refreshed_for_inspect = now;
    }

    async fn log_network_config_metrics(
        &mut self,
        new_config: Option<NetworkConfig>,
        now: fasync::MonotonicInstant,
        ctx: &'static str,
    ) {
        let duration = now - self.network_config_last_refreshed;
        let mut metric_events = vec![];

        if let Some(prev) = &self.network_config {
            let default_route_dim = convert::convert_default_route(
                prev.has_default_ipv4_route,
                prev.has_default_ipv6_route,
            );
            // We only log metric when at least one of IPv4 or IPv6 has default route.
            if let Some(default_route_dim) = default_route_dim {
                metric_events.push(
                    MetricEvent::builder(
                        metrics::REACHABILITY_GLOBAL_DEFAULT_ROUTE_DURATION_METRIC_ID,
                    )
                    .with_event_code(default_route_dim.as_event_code())
                    .as_integer(duration.into_micros()),
                );
                metric_events.push(
                    MetricEvent::builder(
                        metrics::REACHABILITY_GLOBAL_DEFAULT_ROUTE_OCCURRENCE_METRIC_ID,
                    )
                    .with_event_code(default_route_dim.as_event_code())
                    .as_occurrence(1),
                );
            }
        }

        if !metric_events.is_empty() {
            log_cobalt_batch!(self.cobalt_proxy, &metric_events, ctx);
        }

        if let Some(new_config) = new_config {
            self.network_config = Some(new_config);
        }
        self.network_config_last_refreshed = now;
    }

    pub async fn handle_hourly_telemetry(&mut self) {
        let now = fasync::MonotonicInstant::now();
        self.log_system_state_metrics(None, now, "handle_hourly_telemetry").await;
        self.log_network_config_metrics(None, now, "handle_hourly_telemetry").await;
    }
}

#[cfg(test)]
mod tests {
    use crate::{ApplicationState, LinkState};

    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_metrics::MetricEventPayload;
    use fuchsia_inspect::Inspector;
    use fuchsia_inspect_contrib::id_enum::IdEnum;

    use futures::task::Poll;
    use std::pin::Pin;
    use test_case::test_case;
    use windowed_stats::experimental::clock::Timed;
    use windowed_stats::experimental::testing::{MockTimeMatrixClient, TimeMatrixCall};

    const STEP_INCREMENT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(1);

    #[test]
    fn test_log_state_change() {
        let (mut test_helper, mut test_fut) = setup_test();

        let mut update = SystemStateUpdate {
            system_state: IpVersions { ipv4: Some(LinkState::Internet.into()), ipv6: None },
        };
        test_helper
            .telemetry_sender
            .send(TelemetryEvent::SystemStateUpdate { update: update.clone() });

        test_helper.advance_by(zx::MonotonicDuration::from_seconds(25), &mut test_fut);

        update.system_state = IpVersions {
            ipv4: Some(State {
                link: LinkState::Internet,
                application: ApplicationState { dns_resolved: true, http_fetch_succeeded: true },
            }),
            ipv6: None,
        };
        test_helper
            .telemetry_sender
            .send(TelemetryEvent::SystemStateUpdate { update: update.clone() });

        test_helper.advance_test_fut(&mut test_fut);

        let logged_metrics = test_helper
            .get_logged_metrics(metrics::REACHABILITY_STATE_UP_OR_ABOVE_DURATION_METRIC_ID);
        assert_eq!(logged_metrics.len(), 1);
        assert_eq!(logged_metrics[0].payload, MetricEventPayload::IntegerValue(25_000_000));

        let logged_metrics = test_helper
            .get_logged_metrics(metrics::REACHABILITY_GLOBAL_SNAPSHOT_DURATION_METRIC_ID);
        assert_eq!(logged_metrics.len(), 1);
        assert_eq!(logged_metrics[0].payload, MetricEventPayload::IntegerValue(25_000_000));
        assert_eq!(
            logged_metrics[0].event_codes,
            &[
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionRouteConfig::Ipv4Only
                    as u32,
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionInternetAvailable::Yes
                    as u32,
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionGatewayReachable::Yes
                    as u32,
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionDnsActive::No as u32,
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionHttpStatus::Neither
                    as u32,
            ]
        );

        test_helper.cobalt_events.clear();
        test_helper.advance_by(zx::MonotonicDuration::from_seconds(3575), &mut test_fut);

        // At the 1 hour mark, the new state is logged via periodic telemetry.
        // All metrics are the same as before, except for the elapsed duration and the
        // `dns_active` flag is now true.
        let logged_metrics = test_helper
            .get_logged_metrics(metrics::REACHABILITY_STATE_UP_OR_ABOVE_DURATION_METRIC_ID);
        assert_eq!(logged_metrics.len(), 1);
        assert_eq!(logged_metrics[0].payload, MetricEventPayload::IntegerValue(3575_000_000));

        let logged_metrics = test_helper
            .get_logged_metrics(metrics::REACHABILITY_GLOBAL_SNAPSHOT_DURATION_METRIC_ID);
        assert_eq!(logged_metrics.len(), 1);
        assert_eq!(logged_metrics[0].payload, MetricEventPayload::IntegerValue(3575_000_000));
        assert_eq!(
            logged_metrics[0].event_codes,
            &[
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionRouteConfig::Ipv4Only
                    as u32,
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionInternetAvailable::Yes
                    as u32,
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionGatewayReachable::Yes
                    as u32,
                // This time dns_active is Yes, and http_status is HttpOnly
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionDnsActive::Yes as u32,
                metrics::ReachabilityGlobalSnapshotDurationMetricDimensionHttpStatus::HttpOnly
                    as u32,
            ]
        );
    }

    #[test]
    fn test_log_reachability_lost() {
        let (mut test_helper, mut test_fut) = setup_test();

        let mut update = SystemStateUpdate {
            system_state: IpVersions {
                ipv4: None,
                ipv6: Some(State {
                    link: LinkState::Internet,
                    application: ApplicationState { dns_resolved: true, ..Default::default() },
                }),
            },
            ..SystemStateUpdate::default()
        };
        test_helper
            .telemetry_sender
            .send(TelemetryEvent::SystemStateUpdate { update: update.clone() });
        test_helper.advance_test_fut(&mut test_fut);

        update.system_state = IpVersions {
            ipv4: None,
            ipv6: Some(State { link: LinkState::Internet, ..Default::default() }),
        };
        test_helper
            .telemetry_sender
            .send(TelemetryEvent::SystemStateUpdate { update: update.clone() });
        test_helper.advance_test_fut(&mut test_fut);

        // Reachability lost metric is lost because previously both `internet_available` and
        // `dns_active` were true, and now the latter is false.
        let logged_metrics = test_helper.get_logged_metrics(metrics::REACHABILITY_LOST_METRIC_ID);
        assert_eq!(logged_metrics.len(), 1);
        assert_eq!(logged_metrics[0].payload, MetricEventPayload::Count(1));
        assert_eq!(
            logged_metrics[0].event_codes,
            &[metrics::ReachabilityLostMetricDimensionRouteConfig::Ipv6Only as u32]
        );

        test_helper.cobalt_events.clear();

        update.system_state = IpVersions {
            ipv4: None,
            ipv6: Some(State { link: LinkState::Down, ..Default::default() }),
        };
        test_helper
            .telemetry_sender
            .send(TelemetryEvent::SystemStateUpdate { update: update.clone() });
        test_helper.advance_test_fut(&mut test_fut);

        // Reachability lost metric is not logged again even when `internet_available`
        // becomes false, because it was already considered lost previously.
        let logged_metrics = test_helper.get_logged_metrics(metrics::REACHABILITY_LOST_METRIC_ID);
        assert_eq!(logged_metrics.len(), 0);

        test_helper.cobalt_events.clear();

        test_helper.advance_by(zx::MonotonicDuration::from_hours(2), &mut test_fut);
        update.system_state = IpVersions {
            ipv4: Some(State {
                link: LinkState::Internet,
                application: ApplicationState { dns_resolved: true, ..Default::default() },
            }),
            ipv6: None,
        };
        test_helper
            .telemetry_sender
            .send(TelemetryEvent::SystemStateUpdate { update: update.clone() });
        test_helper.advance_test_fut(&mut test_fut);

        // When reachability is recovered, the duration that reachability was lost is logged.
        let logged_metrics =
            test_helper.get_logged_metrics(metrics::REACHABILITY_LOST_DURATION_METRIC_ID);
        assert_eq!(logged_metrics.len(), 1);
        assert_eq!(logged_metrics[0].payload, MetricEventPayload::IntegerValue(7200_000_000));
        assert_eq!(
            logged_metrics[0].event_codes,
            // Reachability is recovered on ipv4, but we still log the ipv6 dimension because
            // that was the config when reachability was lost.
            &[metrics::ReachabilityLostMetricDimensionRouteConfig::Ipv6Only as u32]
        );
    }

    #[test_case(true, true, Some(metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute::Ipv4Ipv6); "ipv4+ipv6 default routes")]
    #[test_case(true, false, Some(metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute::Ipv4Only); "Ipv4Only default routes")]
    #[test_case(false, true, Some(metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute::Ipv6Only); "Ipv6Only default routes")]
    #[test_case(false, false, None; "no default routes")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_log_default_route(
        has_default_ipv4_route: bool,
        has_default_ipv6_route: bool,
        expected_dim: Option<
            metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute,
        >,
    ) {
        let (mut test_helper, mut test_fut) = setup_test();

        test_helper
            .telemetry_sender
            .send(TelemetryEvent::NetworkConfig { has_default_ipv4_route, has_default_ipv6_route });
        test_helper.advance_by(zx::MonotonicDuration::from_hours(1), &mut test_fut);

        let logged_metrics = test_helper
            .get_logged_metrics(metrics::REACHABILITY_GLOBAL_DEFAULT_ROUTE_DURATION_METRIC_ID);
        match expected_dim {
            Some(dim) => {
                assert_eq!(logged_metrics.len(), 1);
                assert_eq!(
                    logged_metrics[0].payload,
                    MetricEventPayload::IntegerValue(3600_000_000)
                );
                assert_eq!(logged_metrics[0].event_codes, &[dim as u32]);
            }
            None => {
                assert_eq!(logged_metrics.len(), 0);
            }
        }

        let logged_metrics = test_helper
            .get_logged_metrics(metrics::REACHABILITY_GLOBAL_DEFAULT_ROUTE_OCCURRENCE_METRIC_ID);
        match expected_dim {
            Some(dim) => {
                assert_eq!(logged_metrics.len(), 1);
                assert_eq!(logged_metrics[0].payload, MetricEventPayload::Count(1));
                assert_eq!(logged_metrics[0].event_codes, &[dim as u32]);
            }
            None => {
                assert_eq!(logged_metrics.len(), 0);
            }
        }
    }

    #[test_case(true, true, vec![]; "negative")]
    #[test_case(true, false, vec![metrics::INTERNET_AVAILABLE_GATEWAY_NOT_PINGABLE_METRIC_ID]; "gateway_discoverable_but_not_pingable")]
    #[test_case(false, true, vec![metrics::INTERNET_AVAILABLE_GATEWAY_NOT_DISCOVERABLE_METRIC_ID]; "gateway_pingable_but_not_discoverable")]
    #[test_case(false, false, vec![metrics::INTERNET_AVAILABLE_GATEWAY_LOST_METRIC_ID]; "gateway_neither_dicoverable_nor_pingable")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_log_abnormal_state_situation(
        gateway_discoverable: bool,
        gateway_pingable: bool,
        expected_metrics: Vec<u32>,
    ) {
        let (mut test_helper, mut test_fut) = setup_test();

        test_helper.telemetry_sender.send(TelemetryEvent::GatewayProbe {
            internet_available: true,
            gateway_discoverable,
            gateway_pingable,
        });
        test_helper.advance_test_fut(&mut test_fut);

        let logged_metrics: Vec<u32> = [
            metrics::INTERNET_AVAILABLE_GATEWAY_NOT_PINGABLE_METRIC_ID,
            metrics::INTERNET_AVAILABLE_GATEWAY_NOT_DISCOVERABLE_METRIC_ID,
            metrics::INTERNET_AVAILABLE_GATEWAY_LOST_METRIC_ID,
        ]
        .into_iter()
        .filter(|id| test_helper.get_logged_metrics(*id).len() > 0)
        .collect();

        assert_eq!(logged_metrics, expected_metrics);
    }

    #[test]
    fn test_link_properties_update() {
        let (mut test_helper, mut test_fut) = setup_test();

        // Iterate through all of the different combinations of booleans in the
        // LinkProperties struct. There are 4 different booleans, so 2^4
        // different combinations.
        for i in 0..=15 {
            let link_properties = LinkProperties::from(i);
            let mut expected_data_vec = vec![];
            // Index 0 indicates that all the booleans are false which is the
            // same as the default value of LinkProperties. In this case no
            // data is reported, so expected_data_vec should be empty.
            if i != 0 {
                expected_data_vec.push(TimeMatrixCall::Fold(Timed::now(i)));
            }

            //  There should be no calls to the `TYPE_ethernet` or `TYPE_wlanclient`
            // time series since nothing has been logged yet.
            let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_ethernet")[..],
                &[]
            );
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_ethernet")[..],
                &[]
            );
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_wlanclient")[..],
                &[]
            );
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_wlanclient")[..],
                &[]
            );

            test_helper.telemetry_sender.send(TelemetryEvent::LinkPropertiesUpdate {
                interface_identifiers: vec![InterfaceIdentifier::Type(InterfaceType::Ethernet)],
                link_properties: IpVersions {
                    ipv4: link_properties.clone(),
                    ipv6: link_properties,
                },
            });
            test_helper.advance_test_fut(&mut test_fut);

            time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_ethernet")[..],
                expected_data_vec
            );
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_ethernet")[..],
                expected_data_vec
            );
            // There should be no calls to the `TYPE_wlanclient` time series since the
            // update above was for `Ethernet`.
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_wlanclient")[..],
                &[]
            );
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_wlanclient")[..],
                &[]
            );
        }
    }

    #[test]
    fn test_link_state_update() {
        let (mut test_helper, mut test_fut) = setup_test();

        // Iterate through all of the different variants in the LinkState enum.
        // LinkState::None is the default so there should be no data recorded
        // for that value.
        let initial_value = LinkState::None.to_id() as u64;
        let final_value = LinkState::Internet.to_id() as u64;
        for i in initial_value..=final_value {
            let link_state = i.into();
            let mut expected_data_vec = vec![];
            if i != initial_value {
                expected_data_vec.push(TimeMatrixCall::Fold(Timed::now(1 << i)));
            }

            //  There should be no calls to the `TYPE_ethernet` or `TYPE_wlanclient`
            // time series since nothing has been logged yet.
            let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
            assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v4_TYPE_ethernet")[..], &[]);
            assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v6_TYPE_ethernet")[..], &[]);
            assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v4_TYPE_wlanclient")[..], &[]);
            assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v6_TYPE_wlanclient")[..], &[]);

            test_helper.telemetry_sender.send(TelemetryEvent::LinkStateUpdate {
                interface_identifiers: vec![InterfaceIdentifier::Type(InterfaceType::Ethernet)],
                link_state: IpVersions { ipv4: link_state, ipv6: link_state },
            });
            test_helper.advance_test_fut(&mut test_fut);

            time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_state_v4_TYPE_ethernet")[..],
                expected_data_vec
            );
            assert_eq!(
                &time_matrix_calls.drain::<u64>("link_state_v6_TYPE_ethernet")[..],
                expected_data_vec
            );
            // There should be no calls to the `TYPE_wlanclient` time series since the
            // update above was for `Ethernet`.
            assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v4_TYPE_wlanclient")[..], &[]);
            assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v6_TYPE_wlanclient")[..], &[]);
        }
    }

    struct TestHelper {
        telemetry_sender: TelemetrySender,
        _inspector: Inspector,
        cobalt_stream: fidl_fuchsia_metrics::MetricEventLoggerRequestStream,
        /// As requests to Cobalt are responded to via `self.drain_cobalt_events()`,
        /// their payloads are drained to this HashMap.
        cobalt_events: Vec<MetricEvent>,
        mock_time_matrix_client: MockTimeMatrixClient,

        // Note: keep the executor field last in the struct so it gets dropped last.
        exec: fasync::TestExecutor,
    }

    impl TestHelper {
        /// Advance executor until stalled, taking care of any blocking requests
        fn advance_test_fut<T>(&mut self, test_fut: &mut (impl Future<Output = T> + Unpin)) {
            let mut made_progress = true;
            while made_progress {
                let _result = self.exec.run_until_stalled(test_fut);
                made_progress = false;
                while let Poll::Ready(Some(Ok(req))) =
                    self.exec.run_until_stalled(&mut self.cobalt_stream.next())
                {
                    self.cobalt_events.append(&mut req.respond_to_metric_req(Ok(())));
                    made_progress = true;
                }
            }

            match self.exec.run_until_stalled(test_fut) {
                Poll::Pending => (),
                _ => panic!("expect test_fut to resolve to Poll::Pending"),
            }
        }

        /// Advance executor by `duration`.
        /// This function repeatedly advances the executor by 1 second, triggering
        /// any expired timers and running the test_fut, until `duration` is reached.
        fn advance_by<T>(
            &mut self,
            duration: zx::MonotonicDuration,
            test_fut: &mut (impl Future<Output = T> + Unpin),
        ) {
            assert_eq!(
                duration.into_nanos() % STEP_INCREMENT.into_nanos(),
                0,
                "duration {:?} is not divisible by STEP_INCREMENT",
                duration,
            );
            const_assert_eq!(
                TELEMETRY_QUERY_INTERVAL.into_nanos() % STEP_INCREMENT.into_nanos(),
                0
            );

            self.advance_test_fut(test_fut);

            for _i in 0..(duration.into_nanos() / STEP_INCREMENT.into_nanos()) {
                self.exec.set_fake_time(fasync::MonotonicInstant::after(STEP_INCREMENT));
                let _ = self.exec.wake_expired_timers();
                self.advance_test_fut(test_fut);
            }
        }

        fn get_logged_metrics(&self, metric_id: u32) -> Vec<MetricEvent> {
            self.cobalt_events.iter().filter(|ev| ev.metric_id == metric_id).cloned().collect()
        }
    }

    trait CobaltExt {
        // Respond to MetricEventLoggerRequest and extract its MetricEvent.
        fn respond_to_metric_req(
            self,
            result: Result<(), fidl_fuchsia_metrics::Error>,
        ) -> Vec<fidl_fuchsia_metrics::MetricEvent>;
    }

    impl CobaltExt for fidl_fuchsia_metrics::MetricEventLoggerRequest {
        fn respond_to_metric_req(
            self,
            result: Result<(), fidl_fuchsia_metrics::Error>,
        ) -> Vec<fidl_fuchsia_metrics::MetricEvent> {
            match self {
                Self::LogOccurrence { metric_id, count, event_codes, responder } => {
                    assert!(responder.send(result).is_ok());
                    vec![MetricEvent {
                        metric_id,
                        event_codes,
                        payload: MetricEventPayload::Count(count),
                    }]
                }
                Self::LogInteger { metric_id, value, event_codes, responder } => {
                    assert!(responder.send(result).is_ok());
                    vec![MetricEvent {
                        metric_id,
                        event_codes,
                        payload: MetricEventPayload::IntegerValue(value),
                    }]
                }
                Self::LogIntegerHistogram { metric_id, histogram, event_codes, responder } => {
                    assert!(responder.send(result).is_ok());
                    vec![MetricEvent {
                        metric_id,
                        event_codes,
                        payload: MetricEventPayload::Histogram(histogram),
                    }]
                }
                Self::LogString { metric_id, string_value, event_codes, responder } => {
                    assert!(responder.send(result).is_ok());
                    vec![MetricEvent {
                        metric_id,
                        event_codes,
                        payload: MetricEventPayload::StringValue(string_value),
                    }]
                }
                Self::LogMetricEvents { events, responder } => {
                    assert!(responder.send(result).is_ok());
                    events
                }
            }
        }
    }

    fn setup_test() -> (TestHelper, Pin<Box<impl Future<Output = Result<(), Error>>>>) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let (cobalt_proxy, cobalt_stream) =
            create_proxy_and_stream::<fidl_fuchsia_metrics::MetricEventLoggerMarker>();

        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("telemetrytest");
        let inspect_metadata_node = inspect_node.create_child(METADATA_NODE_NAME);
        let mock_time_matrix_client = MockTimeMatrixClient::new();

        let link_properties_state_logger = LinkPropertiesStateLogger::new(
            &inspect_metadata_node,
            &format!("root/telemetrytest/{METADATA_NODE_NAME}"),
            InterfaceTimeSeriesGrouping::Type(vec![
                InterfaceType::Ethernet,
                InterfaceType::WlanClient,
            ]),
            &mock_time_matrix_client,
        );

        let interface_aware_logger_node = inspect_node.create_child("interfaces");
        let events_node = interface_aware_logger_node.create_child("events");
        let time_series_node = interface_aware_logger_node.create_child("time_series");
        let interface_aware_logger = InterfaceAwareLogger::new(
            &inspect_metadata_node,
            &format!("root/telemetrytest/{METADATA_NODE_NAME}"),
            InterfaceTimeSeriesGrouping::Type(vec![
                InterfaceType::Ethernet,
                InterfaceType::WlanClient,
            ]),
            events_node,
            time_series_node,
        );

        inspect_node.record(interface_aware_logger_node);

        let (telemetry_sender, test_fut) = serve_telemetry_inner(
            cobalt_proxy,
            inspect_node,
            link_properties_state_logger,
            interface_aware_logger,
        );
        let mut test_fut = Box::pin(test_fut);

        assert_matches::assert_matches!(exec.run_until_stalled(&mut test_fut), Poll::Pending);

        let test_helper = TestHelper {
            telemetry_sender,
            _inspector: inspector,
            cobalt_stream,
            cobalt_events: vec![],
            mock_time_matrix_client,
            exec,
        };
        (test_helper, test_fut)
    }
}
