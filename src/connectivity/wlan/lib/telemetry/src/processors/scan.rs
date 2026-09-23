// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use fidl_fuchsia_power_battery as fidl_battery;
use fuchsia_async as fasync;
use std::ops::BitOr;
use std::sync::Arc;
use windowed_stats::experimental::inspect::{InspectSender, InspectedTimeMatrix};
use windowed_stats::experimental::series::interpolation::ConstantSample;
use windowed_stats::experimental::series::metadata::BitsetMap;
use windowed_stats::experimental::series::statistic::Union;
use windowed_stats::experimental::series::{SamplingProfile, TimeMatrix};
use wlan_legacy_metrics_registry as metrics;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ScanResult {
    Complete { num_results: usize },
    Failed,
    Cancelled,
}

pub struct ScanLogger {
    cobalt_proxy: Arc<FilteredCobaltLogger>,
    time_series_stats: ScanTimeSeries,
    scan_started_at: Option<fasync::BootInstant>,
    on_battery: bool,
}

impl ScanLogger {
    pub fn new<S: InspectSender>(
        cobalt_proxy: Arc<FilteredCobaltLogger>,
        time_matrix_client: &S,
    ) -> Self {
        Self {
            cobalt_proxy,
            time_series_stats: ScanTimeSeries::new(time_matrix_client),
            scan_started_at: None,
            on_battery: false,
        }
    }

    pub async fn handle_scan_start(&mut self) {
        self.scan_started_at = Some(fasync::BootInstant::now());
        self.time_series_stats.scan_events.fold_or_log_error(ScanEvents::START);
        self.log_scan_start_cobalt().await;
    }

    pub async fn log_scan_start_cobalt(&mut self) {
        let mut metric_events = vec![MetricEvent {
            metric_id: metrics::SCAN_OCCURRENCE_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];
        if self.on_battery {
            metric_events.push(MetricEvent {
                metric_id: metrics::SCAN_OCCURRENCE_ON_BATTERY_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::Count(1),
            });
        }
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_scan_start");
    }

    pub async fn handle_scan_result(&mut self, result: ScanResult) {
        let mut metric_events = vec![];
        let now = fasync::BootInstant::now();
        // Only log scan result metrics if there was a scan
        if let Some(scan_started_at) = self.scan_started_at.take() {
            match result {
                ScanResult::Complete { num_results } => {
                    let scan_duration = now - scan_started_at;
                    metric_events.push(MetricEvent {
                        metric_id: metrics::SCAN_FULFILLMENT_TIME_METRIC_ID,
                        event_codes: vec![],
                        payload: MetricEventPayload::IntegerValue(scan_duration.into_millis()),
                    });
                    if num_results == 0 {
                        metric_events.push(MetricEvent {
                            metric_id: metrics::EMPTY_SCAN_RESULTS_METRIC_ID,
                            event_codes: vec![],
                            payload: MetricEventPayload::Count(1),
                        });
                    }
                }
                ScanResult::Failed => {
                    metric_events.push(MetricEvent {
                        metric_id: metrics::CLIENT_SCAN_FAILURE_METRIC_ID,
                        event_codes: vec![],
                        payload: MetricEventPayload::Count(1),
                    });
                }
                ScanResult::Cancelled => {
                    metric_events.push(MetricEvent {
                        metric_id: metrics::ABORTED_SCAN_METRIC_ID,
                        event_codes: vec![],
                        payload: MetricEventPayload::Count(1),
                    });
                }
            }
        }

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_scan_result");
    }

    pub async fn handle_battery_charge_status(
        &mut self,
        charge_status: fidl_battery::ChargeStatus,
    ) {
        self.on_battery = matches!(charge_status, fidl_battery::ChargeStatus::Discharging);
    }
}

#[derive(Default, Copy, Clone, Debug, PartialEq)]
struct ScanEvents(u64);
impl ScanEvents {
    // Note: Keep these bits in sync with ScanEvents::bit_set_map
    const START: Self = Self(1 << 0);
}

impl ScanEvents {
    fn bit_set_map() -> BitsetMap {
        BitsetMap::from_ordered(["start"])
    }
}

impl BitOr for ScanEvents {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl From<ScanEvents> for u64 {
    fn from(value: ScanEvents) -> u64 {
        value.0
    }
}

#[derive(Debug)]
struct ScanTimeSeries {
    scan_events: InspectedTimeMatrix<ScanEvents>,
}

impl ScanTimeSeries {
    pub fn new<S: InspectSender>(client: &S) -> Self {
        let scan_events = client.inspect_time_matrix_with_metadata(
            "scan_events",
            TimeMatrix::<Union<ScanEvents>, ConstantSample>::new(
                SamplingProfile::highly_granular(),
                ConstantSample::default(),
            ),
            ScanEvents::bit_set_map(),
        );
        Self { scan_events }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestHelper, setup_test};
    use diagnostics_assertions::{AnyBytesProperty, assert_data_tree};
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;
    use windowed_stats::experimental::clock::Timed;
    use windowed_stats::experimental::inspect::TimeMatrixClient;
    use windowed_stats::experimental::testing::TimeMatrixCall;

    fn run_handle_scan_start(test_helper: &mut TestHelper, scan_logger: &mut ScanLogger) {
        let mut test_fut = pin!(scan_logger.handle_scan_start());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    fn run_handle_scan_result(
        test_helper: &mut TestHelper,
        scan_logger: &mut ScanLogger,
        scan_result: ScanResult,
    ) {
        let mut test_fut = pin!(scan_logger.handle_scan_result(scan_result));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    fn run_handle_battery_charge_status(
        test_helper: &mut TestHelper,
        scan_logger: &mut ScanLogger,
        charge_status: fidl_battery::ChargeStatus,
    ) {
        let mut test_fut = pin!(scan_logger.handle_battery_charge_status(charge_status));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    #[fuchsia::test]
    fn test_handle_scan_start() {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        let metrics = test_helper.get_logged_metrics(metrics::SCAN_OCCURRENCE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper.get_logged_metrics(metrics::SCAN_OCCURRENCE_ON_BATTERY_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_handle_scan_start_on_battery() {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        run_handle_battery_charge_status(
            &mut test_helper,
            &mut scan_logger,
            fidl_battery::ChargeStatus::Discharging,
        );
        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        let metrics = test_helper.get_logged_metrics(metrics::SCAN_OCCURRENCE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper.get_logged_metrics(metrics::SCAN_OCCURRENCE_ON_BATTERY_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        // Set charge status to Charging and verify that scan_onccurrence_on_battery is not
        // logged. This verifies that we do change back to off battery.
        test_helper.clear_cobalt_events();
        run_handle_battery_charge_status(
            &mut test_helper,
            &mut scan_logger,
            fidl_battery::ChargeStatus::Charging,
        );
        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        let metrics = test_helper.get_logged_metrics(metrics::SCAN_OCCURRENCE_ON_BATTERY_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_handle_scan_result_complete() {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(20_000_000));
        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(100_000_000));
        let scan_result = ScanResult::Complete { num_results: 10 };
        run_handle_scan_result(&mut test_helper, &mut scan_logger, scan_result);

        let metrics = test_helper.get_logged_metrics(metrics::SCAN_FULFILLMENT_TIME_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(80)); // 80ms
        let metrics = test_helper.get_logged_metrics(metrics::EMPTY_SCAN_RESULTS_METRIC_ID);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn test_handle_scan_result_empty() {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        let scan_result = ScanResult::Complete { num_results: 0 };
        run_handle_scan_result(&mut test_helper, &mut scan_logger, scan_result);

        let metrics = test_helper.get_logged_metrics(metrics::SCAN_FULFILLMENT_TIME_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        let metrics = test_helper.get_logged_metrics(metrics::EMPTY_SCAN_RESULTS_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_handle_scan_result_cancelled() {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        let scan_result = ScanResult::Cancelled;
        run_handle_scan_result(&mut test_helper, &mut scan_logger, scan_result);

        let metrics = test_helper.get_logged_metrics(metrics::ABORTED_SCAN_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_handle_scan_result_failure() {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        let scan_result = ScanResult::Failed;
        run_handle_scan_result(&mut test_helper, &mut scan_logger, scan_result);

        let metrics = test_helper.get_logged_metrics(metrics::CLIENT_SCAN_FAILURE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[test_case(
        ScanResult::Complete { num_results: 10 },
        metrics::SCAN_FULFILLMENT_TIME_METRIC_ID;
        "scan complete"
    )]
    #[test_case(
        ScanResult::Failed,
        metrics::CLIENT_SCAN_FAILURE_METRIC_ID;
        "scan failed"
    )]
    #[test_case(
        ScanResult::Cancelled,
        metrics::ABORTED_SCAN_METRIC_ID;
        "scan cancelled"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_handle_scan_result_no_logging_to_cobalt_if_scan_not_started(
        scan_result: ScanResult,
        metric_id: u32,
    ) {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        run_handle_scan_result(&mut test_helper, &mut scan_logger, scan_result);

        let metrics = test_helper.get_logged_metrics(metric_id);
        assert!(metrics.is_empty());
    }

    #[fuchsia::test]
    fn scan_logger_new_then_inspect_data_tree_contains_time_matrix_metadata() {
        let mut test_helper = setup_test();
        let client = TimeMatrixClient::new(test_helper.inspect_node.create_child("wlan_scan"));
        let _scan_logger = ScanLogger::new(test_helper.filtered_cobalt_logger(), &client);

        let tree = test_helper.get_inspect_data_tree();
        assert_data_tree!(
            @executor test_helper.exec,
            tree,
            root: contains {
                test_stats: contains {
                    wlan_scan: contains {
                        scan_events: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index: {
                                    "0": "start",
                                }
                            }
                        }
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    fn log_scan_start_inspect() {
        let mut test_helper = setup_test();
        let mut scan_logger = ScanLogger::new(
            test_helper.filtered_cobalt_logger(),
            &test_helper.mock_time_matrix_client,
        );

        run_handle_scan_start(&mut test_helper, &mut scan_logger);

        let mut time_matrix_calls = test_helper.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<ScanEvents>("scan_events")[..],
            &[TimeMatrixCall::Fold(Timed::now(ScanEvents::START))]
        );
    }
}
