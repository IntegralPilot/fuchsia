// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt};
use cobalt_client::traits::AsEventCode;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use futures::lock::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use wlan_legacy_metrics_registry as metrics;

pub const INSPECT_POWER_EVENTS_LIMIT: usize = 20;

#[derive(Debug, PartialEq)]
pub enum IfacePowerLevel {
    Disconnected,
    SuspendMode,
    Normal,
    NoPowerSavings,
}

#[derive(Debug)]
pub enum UnclearPowerDemand {
    PowerSaveRequestedWhileSuspendModeEnabled,
}

pub struct PowerLogger {
    power_inspect_node: Mutex<BoundedListNode>,
    cobalt_proxy: Arc<FilteredCobaltLogger>,
    iface_power_states: Arc<Mutex<HashMap<u16, IfacePowerLevel>>>,
}

impl PowerLogger {
    pub fn new(cobalt_proxy: Arc<FilteredCobaltLogger>, inspect_node: &InspectNode) -> Self {
        // Initialize inspect children
        let iface_power_events = inspect_node.create_child("iface_power_events");
        let power_inspect_node: BoundedListNode =
            BoundedListNode::new(iface_power_events, INSPECT_POWER_EVENTS_LIMIT);

        Self {
            power_inspect_node: Mutex::new(power_inspect_node),
            cobalt_proxy,
            iface_power_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn log_iface_power_event(&self, iface_power_level: IfacePowerLevel, iface_id: u16) {
        inspect_log!(self.power_inspect_node.lock().await, {
            power_level: std::format!("{:?}", iface_power_level),
            iface_id: iface_id,
        });
        let _ = self.iface_power_states.lock().await.insert(iface_id, iface_power_level);
    }

    pub async fn handle_iface_disconnect(&self, iface_id: u16) {
        let _ =
            self.iface_power_states.lock().await.insert(iface_id, IfacePowerLevel::Disconnected);
    }

    pub async fn handle_iface_destroyed(&self, iface_id: u16) {
        let _ = self.iface_power_states.lock().await.remove(&iface_id);
    }

    pub async fn handle_suspend_imminent(&self) {
        for iface_power_level in self.iface_power_states.lock().await.values() {
            use metrics::PowerLevelAtSuspendMetricDimensionPowerLevel as dim;
            log_cobalt!(
                self.cobalt_proxy,
                log_occurrence,
                metrics::POWER_LEVEL_AT_SUSPEND_METRIC_ID,
                1,
                &[match iface_power_level {
                    IfacePowerLevel::Disconnected => dim::Disconnected,
                    IfacePowerLevel::SuspendMode => dim::SuspendMode,
                    IfacePowerLevel::Normal => dim::PowerSaveMode,
                    IfacePowerLevel::NoPowerSavings => dim::HighPerformanceMode,
                }
                .as_event_code()]
            )
        }
    }

    pub async fn handle_unclear_power_demand(&self, demand: UnclearPowerDemand) {
        use metrics::UnclearPowerLevelDemandMetricDimensionReason as dim;
        log_cobalt!(
            self.cobalt_proxy,
            log_occurrence,
            metrics::UNCLEAR_POWER_LEVEL_DEMAND_METRIC_ID,
            1,
            &[match demand {
                UnclearPowerDemand::PowerSaveRequestedWhileSuspendModeEnabled =>
                    dim::PowerSaveRequestedWhileSuspendModeEnabled,
            }
            .as_event_code()]
        )
    }

    pub async fn handle_chip_power_up_failure(&self) {
        log_cobalt!(
            self.cobalt_proxy,
            log_occurrence,
            metrics::CHIP_POWER_UP_FAILURE_METRIC_ID,
            1,
            &[]
        )
    }

    pub async fn chip_power_down_failure(&self) {
        log_cobalt!(
            self.cobalt_proxy,
            log_occurrence,
            metrics::STOP_FAILURE_OCCURRENCE_METRIC_ID,
            1,
            &[]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::setup_test;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyNumericProperty, assert_data_tree};
    use futures::pin_mut;
    use futures::task::Poll;
    use std::pin::pin;

    #[fuchsia::test]
    fn test_iface_power_event_in_inspect() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let test_fut = power_logger.log_iface_power_event(IfacePowerLevel::NoPowerSavings, 11);
        pin_mut!(test_fut);
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let test_fut = power_logger.log_iface_power_event(IfacePowerLevel::NoPowerSavings, 22);
        pin_mut!(test_fut);
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let mut test_fut = pin!(power_logger.log_iface_power_event(IfacePowerLevel::Normal, 33));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Update the first one
        let test_fut = power_logger.log_iface_power_event(IfacePowerLevel::SuspendMode, 11);
        pin_mut!(test_fut);
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        assert_data_tree!(@executor test_helper.exec, test_helper.inspector, root: contains {
            wlan_mock_node: {
                iface_power_events: {
                    "0": {
                        "power_level": "NoPowerSavings",
                        "iface_id": 11_u64,
                        "@time": AnyNumericProperty
                    },
                    "1": {
                        "power_level": "NoPowerSavings",
                        "iface_id": 22_u64,
                        "@time": AnyNumericProperty
                    },
                    "2": {
                        "power_level": "Normal",
                        "iface_id": 33_u64,
                        "@time": AnyNumericProperty
                    },
                    "3": {
                        "power_level": "SuspendMode",
                        "iface_id": 11_u64,
                        "@time": AnyNumericProperty
                    },
                }
            }
        });
    }

    #[fuchsia::test]
    fn test_iface_power_event_adds_to_internal_hashmap() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let test_fut = power_logger.log_iface_power_event(IfacePowerLevel::NoPowerSavings, 11);
        pin_mut!(test_fut);
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let test_fut = power_logger.log_iface_power_event(IfacePowerLevel::NoPowerSavings, 22);
        pin_mut!(test_fut);
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        let mut test_fut = pin!(power_logger.log_iface_power_event(IfacePowerLevel::Normal, 33));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        // Send a new value for the first iface_id, ensuring it gets updated
        let test_fut = power_logger.log_iface_power_event(IfacePowerLevel::SuspendMode, 11);
        pin_mut!(test_fut);
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        assert_eq!(power_logger.iface_power_states.try_lock().unwrap().len(), 3);
        assert_eq!(
            power_logger.iface_power_states.try_lock().unwrap().get(&11),
            Some(&IfacePowerLevel::SuspendMode)
        );
        assert_eq!(
            power_logger.iface_power_states.try_lock().unwrap().get(&22),
            Some(&IfacePowerLevel::NoPowerSavings)
        );
        assert_eq!(
            power_logger.iface_power_states.try_lock().unwrap().get(&33),
            Some(&IfacePowerLevel::Normal)
        );
    }

    #[fuchsia::test]
    fn test_disconnect_updates_internal_hashmap() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let _ =
            power_logger.iface_power_states.try_lock().unwrap().insert(33, IfacePowerLevel::Normal);
        assert_eq!(power_logger.iface_power_states.try_lock().unwrap().len(), 1);

        let mut test_fut = pin!(power_logger.handle_iface_disconnect(33));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        assert_eq!(power_logger.iface_power_states.try_lock().unwrap().len(), 1);
        assert_eq!(
            power_logger.iface_power_states.try_lock().unwrap().get(&33),
            Some(&IfacePowerLevel::Disconnected)
        );
    }

    #[fuchsia::test]
    fn test_destroy_removes_from_internal_hashmap() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let _ =
            power_logger.iface_power_states.try_lock().unwrap().insert(33, IfacePowerLevel::Normal);
        assert_eq!(power_logger.iface_power_states.try_lock().unwrap().len(), 1);

        let mut test_fut = pin!(power_logger.handle_iface_destroyed(33));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        assert_eq!(power_logger.iface_power_states.try_lock().unwrap().len(), 0);
    }

    #[fuchsia::test]
    fn test_imminent_suspension_logs_to_cobalt() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let _ =
            power_logger.iface_power_states.try_lock().unwrap().insert(33, IfacePowerLevel::Normal);
        let mut test_fut = pin!(power_logger.handle_suspend_imminent());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let logged_metrics =
            test_helper.get_logged_metrics(metrics::POWER_LEVEL_AT_SUSPEND_METRIC_ID);
        assert_matches!(&logged_metrics[..], [metric] => {
            let expected_metric = fidl_fuchsia_metrics::MetricEvent {
                metric_id: metrics::POWER_LEVEL_AT_SUSPEND_METRIC_ID,
                event_codes: vec![metrics::PowerLevelAtSuspendMetricDimensionPowerLevel::PowerSaveMode
                    .as_event_code()],
                payload: fidl_fuchsia_metrics::MetricEventPayload::Count(1),
            };
            assert_eq!(metric, &expected_metric);
        });
    }

    #[fuchsia::test]
    fn test_unclear_power_demand_logs_to_cobalt() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let mut test_fut = pin!(power_logger.handle_unclear_power_demand(
            UnclearPowerDemand::PowerSaveRequestedWhileSuspendModeEnabled
        ));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let logged_metrics =
            test_helper.get_logged_metrics(metrics::UNCLEAR_POWER_LEVEL_DEMAND_METRIC_ID);
        assert_matches!(&logged_metrics[..], [metric] => {
            let expected_metric = fidl_fuchsia_metrics::MetricEvent {
                metric_id: metrics::UNCLEAR_POWER_LEVEL_DEMAND_METRIC_ID,
                event_codes: vec![metrics::UnclearPowerLevelDemandMetricDimensionReason::PowerSaveRequestedWhileSuspendModeEnabled
                    .as_event_code()],
                payload: fidl_fuchsia_metrics::MetricEventPayload::Count(1),
            };
            assert_eq!(metric, &expected_metric);
        });
    }

    #[fuchsia::test]
    fn test_chip_power_up_failure_logs_to_cobalt() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let mut test_fut = pin!(power_logger.handle_chip_power_up_failure());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let logged_metrics =
            test_helper.get_logged_metrics(metrics::CHIP_POWER_UP_FAILURE_METRIC_ID);
        assert_matches!(&logged_metrics[..], [metric] => {
            let expected_metric = fidl_fuchsia_metrics::MetricEvent {
                metric_id: metrics::CHIP_POWER_UP_FAILURE_METRIC_ID,
                event_codes: vec![],
                payload: fidl_fuchsia_metrics::MetricEventPayload::Count(1),
            };
            assert_eq!(metric, &expected_metric);
        });
    }

    #[fuchsia::test]
    fn test_failed_to_stop_logs_to_cobalt() {
        let mut test_helper = setup_test();
        let node = test_helper.create_inspect_node("wlan_mock_node");
        let power_logger = PowerLogger::new(test_helper.filtered_cobalt_logger(), &node);

        let mut test_fut = pin!(power_logger.chip_power_down_failure());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let logged_metrics =
            test_helper.get_logged_metrics(metrics::STOP_FAILURE_OCCURRENCE_METRIC_ID);
        assert_matches!(&logged_metrics[..], [metric] => {
            let expected_metric = fidl_fuchsia_metrics::MetricEvent {
                metric_id: metrics::STOP_FAILURE_OCCURRENCE_METRIC_ID,
                event_codes: vec![],
                payload: fidl_fuchsia_metrics::MetricEventPayload::Count(1),
            };
            assert_eq!(metric, &expected_metric);
        });
    }
}
