// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use fidl_fuchsia_power_battery as fidl_battery;
use fuchsia_async as fasync;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use std::cmp::max;
use std::sync::Arc;
use wlan_legacy_metrics_registry as metrics;
use zx;

pub const INSPECT_TOGGLE_EVENTS_LIMIT: usize = 20;
const TIME_QUICK_TOGGLE_WIFI: zx::BootDuration = zx::BootDuration::from_seconds(5);

#[derive(Debug, PartialEq)]
pub enum ClientConnectionsToggleEvent {
    Enabled,
    Disabled,
}

pub struct ToggleLogger {
    toggle_inspect_node: BoundedListNode,
    cobalt_proxy: Arc<FilteredCobaltLogger>,
    /// This is None until telemetry is notified of an off or on event, since these metrics don't
    /// currently need to know the starting state.
    current_state: Option<ClientConnectionsToggleEvent>,
    /// The last time wlan was toggled on
    time_started: Option<fasync::BootInstant>,
    /// The last time wlan was toggled off, or None if it hasn't been. Used to determine if WLAN
    /// was turned on right after being turned off.
    time_stopped: Option<fasync::BootInstant>,
    /// When the device was last on battery. This is not populated on init and when the device is
    /// on charger.
    on_battery_since: Option<fasync::BootInstant>,
}

impl ToggleLogger {
    pub fn new(cobalt_proxy: Arc<FilteredCobaltLogger>, inspect_node: &InspectNode) -> Self {
        // Initialize inspect children
        let toggle_events = inspect_node.create_child("client_connections_toggle_events");
        let toggle_inspect_node = BoundedListNode::new(toggle_events, INSPECT_TOGGLE_EVENTS_LIMIT);
        let current_state = None;
        let time_started = None;
        let time_stopped = None;
        let on_battery_since = None;

        Self {
            toggle_inspect_node,
            cobalt_proxy,
            current_state,
            time_started,
            time_stopped,
            on_battery_since,
        }
    }

    pub async fn handle_toggle_event(&mut self, event_type: ClientConnectionsToggleEvent) {
        // This inspect macro logs the time as well
        inspect_log!(self.toggle_inspect_node, {
            event_type: std::format!("{:?}", event_type)
        });

        let mut metric_events = vec![];
        let now = fasync::BootInstant::now();
        match &event_type {
            ClientConnectionsToggleEvent::Enabled => {
                // Log an occurrence if the client connection was not already enabled
                if self.current_state != Some(ClientConnectionsToggleEvent::Enabled) {
                    self.time_started = Some(now);

                    metric_events.push(MetricEvent {
                        metric_id: metrics::CLIENT_CONNECTION_ENABLED_OCCURRENCE_METRIC_ID,
                        event_codes: vec![],
                        payload: MetricEventPayload::Count(1),
                    });
                }

                // If connections were just disabled before this, log a metric for the quick wifi
                // restart.
                if self.current_state == Some(ClientConnectionsToggleEvent::Disabled)
                    && let Some(time_stopped) = self.time_stopped
                    && now - time_stopped < TIME_QUICK_TOGGLE_WIFI
                {
                    metric_events.push(MetricEvent {
                        metric_id: metrics::CLIENT_CONNECTIONS_STOP_AND_START_METRIC_ID,
                        event_codes: vec![],
                        payload: MetricEventPayload::Count(1),
                    });
                }
            }
            ClientConnectionsToggleEvent::Disabled => {
                // Only change the time and log duration if connections were not already disabled.
                if self.current_state == Some(ClientConnectionsToggleEvent::Enabled) {
                    self.time_stopped = Some(now);

                    if let Some(time_started) = self.time_started {
                        let duration = now - time_started;
                        metric_events.push(MetricEvent {
                            metric_id: metrics::CLIENT_CONNECTION_ENABLED_DURATION_METRIC_ID,
                            event_codes: vec![],
                            payload: MetricEventPayload::IntegerValue(duration.into_millis()),
                        });

                        // If `on_battery_since` is `Some`, it indicates that we have been
                        // on battery, as otherwise this Option would be cleared out in
                        // `handle_battery_charge_status`
                        //
                        // Here we only handle the transition from connection-enabled +
                        // on-battery state -> connection disabled. The other case
                        // where we transition to on charger is handled in
                        // `handle_battery_charge_status`
                        if let Some(on_battery_since) = self.on_battery_since {
                            // Get the max of `time_started` and `on_battery_since` as it was
                            // when connection is enabled *and* device is on battery
                            let duration = now - max(time_started, on_battery_since);
                            metric_events.push(MetricEvent {
                                metric_id: metrics::CLIENT_CONNECTION_ENABLED_DURATION_ON_BATTERY_METRIC_ID,
                                event_codes: vec![],
                                payload: MetricEventPayload::IntegerValue(duration.into_millis()),
                            });
                        }
                    }
                }
            }
        }
        self.current_state = Some(event_type);

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_toggle_events");
    }

    pub async fn handle_battery_charge_status(
        &mut self,
        charge_status: fidl_battery::ChargeStatus,
    ) {
        let mut metric_events = vec![];
        let now = fasync::BootInstant::now();
        let on_battery_now = matches!(charge_status, fidl_battery::ChargeStatus::Discharging);

        match (self.on_battery_since, on_battery_now) {
            (None, true) => self.on_battery_since = Some(now),
            (Some(on_battery_since), false) => {
                let _on_battery_since = self.on_battery_since.take();
                // Here we only handle the transition from connection-enabled *and*
                // on-battery state -> on-charger state. The other case
                // where we transition to connection disabled is handled in
                // `handle_toggle_event`
                if let Some(ClientConnectionsToggleEvent::Enabled) = self.current_state
                    && let Some(time_started) = self.time_started
                {
                    // Get the max of `time_started` and `on_battery_since` as it was
                    // when connection is enabled *and* device is on battery
                    let duration = now - max(time_started, on_battery_since);
                    metric_events.push(MetricEvent {
                        metric_id: metrics::CLIENT_CONNECTION_ENABLED_DURATION_ON_BATTERY_METRIC_ID,
                        event_codes: vec![],
                        payload: MetricEventPayload::IntegerValue(duration.into_millis()),
                    });
                }
            }
            _ => (),
        }

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_battery_charge_status");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestHelper, setup_test};
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyNumericProperty, assert_data_tree};
    use futures::task::Poll;
    use std::pin::pin;

    #[fuchsia::test]
    fn test_toggle_is_recorded_to_inspect() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let mut toggle_logger = ToggleLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        assert_data_tree!(@executor test_helper.exec, test_helper.inspector, root: contains {
            wlan_mock_node: {
                client_connections_toggle_events: {
                    "0": {
                        "event_type": "Enabled",
                        "@time": AnyNumericProperty
                    },
                    "1": {
                        "event_type": "Disabled",
                        "@time": AnyNumericProperty
                    },
                    "2": {
                        "event_type": "Enabled",
                        "@time": AnyNumericProperty
                    },
                }
            }
        });
    }

    // Uses the test helper to run toggle_logger.handle_toggle_event so that any cobalt metrics sent
    // will be acked and not block anything. It expects no response from the handle_toggle_event.
    fn run_handle_toggle_event(
        test_helper: &mut TestHelper,
        toggle_logger: &mut ToggleLogger,
        event: ClientConnectionsToggleEvent,
    ) {
        let mut test_fut = pin!(toggle_logger.handle_toggle_event(event));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    fn run_handle_battery_charge_status(
        test_helper: &mut TestHelper,
        toggle_logger: &mut ToggleLogger,
        charge_status: fidl_battery::ChargeStatus,
    ) {
        let mut test_fut = pin!(toggle_logger.handle_battery_charge_status(charge_status));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    #[fuchsia::test]
    fn test_quick_toggle_metric_is_recorded() {
        let mut test_helper = setup_test();
        let inspect_node = test_helper.create_inspect_node("test_stats");
        let mut toggle_logger =
            ToggleLogger::new(test_helper.filtered_cobalt_logger(), &inspect_node);

        // Start with client connections enabled.
        let mut test_time = fasync::MonotonicInstant::from_nanos(123);
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Stop client connections and quickly start them again.
        test_time += fasync::MonotonicDuration::from_minutes(40);
        test_helper.exec.set_fake_time(test_time);
        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        test_time += fasync::MonotonicDuration::from_seconds(1);
        test_helper.exec.set_fake_time(test_time);
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Check that a metric is logged for the quick stop and start.
        let logged_metrics =
            test_helper.get_logged_metrics(metrics::CLIENT_CONNECTIONS_STOP_AND_START_METRIC_ID);
        assert_matches!(&logged_metrics[..], [metric] => {
            let expected_metric = fidl_fuchsia_metrics::MetricEvent {
                metric_id: metrics::CLIENT_CONNECTIONS_STOP_AND_START_METRIC_ID,
                event_codes: vec![],
                payload: fidl_fuchsia_metrics::MetricEventPayload::Count(1),
            };
            assert_eq!(metric, &expected_metric);
        });
    }

    #[fuchsia::test]
    fn test_quick_toggle_no_metric_is_recorded_if_not_quick() {
        let mut test_helper = setup_test();
        let inspect_node = test_helper.create_inspect_node("test_stats");
        let mut toggle_logger =
            ToggleLogger::new(test_helper.filtered_cobalt_logger(), &inspect_node);

        // Start with client connections enabled.
        let mut test_time = fasync::MonotonicInstant::from_nanos(123);
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Stop client connections and a while later start them again.
        test_time += fasync::MonotonicDuration::from_minutes(20);
        test_helper.exec.set_fake_time(test_time);
        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        test_time += fasync::MonotonicDuration::from_minutes(30);
        test_helper.exec.set_fake_time(test_time);
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Check that no metric is logged for quick toggles since there was a while between the
        // stop and start.
        let logged_metrics =
            test_helper.get_logged_metrics(metrics::CLIENT_CONNECTIONS_STOP_AND_START_METRIC_ID);
        assert!(logged_metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_quick_toggle_metric_second_disable_doesnt_update_time() {
        // Verify that if two consecutive disables happen, only the first is used to determine
        // quick toggles since the following ones don't change the state.
        let mut test_helper = setup_test();
        let inspect_node = test_helper.create_inspect_node("test_stats");
        let mut toggle_logger =
            ToggleLogger::new(test_helper.filtered_cobalt_logger(), &inspect_node);

        // Start with client connections enabled.
        let mut test_time = fasync::MonotonicInstant::from_nanos(123);
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Stop client connections and a while later stop them again.
        test_time += fasync::MonotonicDuration::from_minutes(40);
        test_helper.exec.set_fake_time(test_time);
        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        test_time += fasync::MonotonicDuration::from_minutes(30);
        test_helper.exec.set_fake_time(test_time);
        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Start client connections right after the last disable message.
        test_time += fasync::MonotonicDuration::from_seconds(1);
        test_helper.exec.set_fake_time(test_time);
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Check that no metric is logged since the enable message came a while after the first
        // disable message.
        let logged_metrics =
            test_helper.get_logged_metrics(metrics::CLIENT_CONNECTIONS_STOP_AND_START_METRIC_ID);
        assert!(logged_metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_log_client_connection_enabled() {
        let mut test_helper = setup_test();
        let inspect_node = test_helper.create_inspect_node("test_stats");
        let mut toggle_logger =
            ToggleLogger::new(test_helper.filtered_cobalt_logger(), &inspect_node);

        // Start with client connections enabled.
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000));
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        let metrics =
            test_helper.get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_OCCURRENCE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        // Send enabled event again. This should not log any metric because the device was not
        // in a disabled state
        test_helper.clear_cobalt_events();
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000_000));
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        let metrics =
            test_helper.get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_OCCURRENCE_METRIC_ID);
        assert!(metrics.is_empty());

        // Send disabled event. This should log the duration between now and when the
        // first enabled event was sent.
        test_helper.clear_cobalt_events();
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(100_000_000));
        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        let metrics =
            test_helper.get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_DURATION_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(90));
        let metrics = test_helper
            .get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_DURATION_ON_BATTERY_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_log_client_connection_enabled_duration_on_battery_with_reenable() {
        let mut test_helper = setup_test();
        let inspect_node = test_helper.create_inspect_node("test_stats");
        let mut toggle_logger =
            ToggleLogger::new(test_helper.filtered_cobalt_logger(), &inspect_node);

        // Start with client connections enabled.
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000));
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Set on battery
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(30_000_000));
        let charge_status = fidl_battery::ChargeStatus::Discharging;
        run_handle_battery_charge_status(&mut test_helper, &mut toggle_logger, charge_status);

        // Send disabled event. This should log the duration between now and when the client
        // connections enabled AND device is on battery
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(100_000_000));
        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        let metrics = test_helper
            .get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_DURATION_ON_BATTERY_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(70));

        test_helper.clear_cobalt_events();
        // Send client connections enabled and disabled events again.
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(110_000_000));
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(150_000_000));
        let event = ClientConnectionsToggleEvent::Disabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Verify that only the duration since the second enabled event is logged.
        let metrics = test_helper
            .get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_DURATION_ON_BATTERY_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(40));
    }

    #[fuchsia::test]
    fn test_log_client_connection_enabled_duration_on_battery_with_repeated_battery_change() {
        let mut test_helper = setup_test();
        let inspect_node = test_helper.create_inspect_node("test_stats");
        let mut toggle_logger =
            ToggleLogger::new(test_helper.filtered_cobalt_logger(), &inspect_node);

        // Start with client connections enabled.
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000));
        let event = ClientConnectionsToggleEvent::Enabled;
        run_handle_toggle_event(&mut test_helper, &mut toggle_logger, event);

        // Set on battery
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(30_000_000));
        let charge_status = fidl_battery::ChargeStatus::Discharging;
        run_handle_battery_charge_status(&mut test_helper, &mut toggle_logger, charge_status);

        // Set on charger. This should log the duration between now and when the client
        // connection is enabled *and* device is on battery
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(100_000_000));
        let charge_status = fidl_battery::ChargeStatus::Charging;
        run_handle_battery_charge_status(&mut test_helper, &mut toggle_logger, charge_status);

        let metrics = test_helper
            .get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_DURATION_ON_BATTERY_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(70));

        test_helper.clear_cobalt_events();
        // Set on battery and then on charger again.
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(110_000_000));
        let charge_status = fidl_battery::ChargeStatus::Discharging;
        run_handle_battery_charge_status(&mut test_helper, &mut toggle_logger, charge_status);
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(150_000_000));
        let charge_status = fidl_battery::ChargeStatus::Charging;
        run_handle_battery_charge_status(&mut test_helper, &mut toggle_logger, charge_status);

        // Verify that only the duration since the second enabled event is logged.
        let metrics = test_helper
            .get_logged_metrics(metrics::CLIENT_CONNECTION_ENABLED_DURATION_ON_BATTERY_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(40));
    }
}
