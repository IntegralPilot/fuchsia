// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use fuchsia_async as fasync;
use std::sync::Arc;
use wlan_legacy_metrics_registry as metrics;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PnoScanDisabledReason {
    ApiRequest,
    Internal,
    Firmware,
}

pub struct PnoScanLogger {
    cobalt_proxy: Arc<FilteredCobaltLogger>,
    enabled_at: Option<fasync::BootInstant>,
    has_scan_results: bool,
}

impl PnoScanLogger {
    pub fn new(cobalt_proxy: Arc<FilteredCobaltLogger>) -> Self {
        Self { cobalt_proxy, enabled_at: None, has_scan_results: false }
    }

    pub async fn handle_pno_scan_enabled(&mut self, is_connected: bool) {
        if self.enabled_at.is_none() {
            self.enabled_at = Some(fasync::BootInstant::now());
            self.has_scan_results = false;

            if is_connected {
                let metric_events = vec![MetricEvent {
                    metric_id: metrics::PNO_SCAN_ENABLED_WHILE_CONNECTED_METRIC_ID,
                    event_codes: vec![],
                    payload: MetricEventPayload::Count(1),
                }];
                log_cobalt_batch!(
                    self.cobalt_proxy,
                    &metric_events,
                    "pno_scan_enabled_while_connected"
                );
            }
        } else {
            // It is unexpected that PNO scans are enabled and then enabled again prior to a
            // cancellation of the first request.  The accounting surrounding PNO scan enablement
            // should not be updated in this case, since PNO scans have been enabled since the
            // first enablement.
            let metric_events = vec![MetricEvent {
                metric_id: metrics::PNO_SCAN_REQUEST_COLLISION_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::Count(1),
            }];
            log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_pno_scan_enabled");
        }
    }

    pub async fn handle_pno_scan_results_received(&mut self) {
        // Log only the first instance of PNO scan results.  This metric is only logged if PNO
        // scans are currently enabled.  Scan results are not expected to be received if PNO
        // scans are not enabled.
        if !self.has_scan_results {
            self.has_scan_results = true;

            if let Some(enabled_at) = self.enabled_at {
                let elapsed = fasync::BootInstant::now() - enabled_at;
                let metric_events = vec![MetricEvent {
                    metric_id: metrics::PNO_SCAN_FIRST_RESULTS_ELAPSED_TIME_METRIC_ID,
                    event_codes: vec![],
                    payload: MetricEventPayload::IntegerValue(elapsed.into_millis()),
                }];
                log_cobalt_batch!(
                    self.cobalt_proxy,
                    &metric_events,
                    "handle_pno_scan_results_received"
                );
            }
        }
    }

    pub async fn handle_pno_scan_disabled(&mut self, reason: PnoScanDisabledReason) {
        if let Some(enabled_at) = self.enabled_at.take() {
            let now = fasync::BootInstant::now();
            let elapsed = now - enabled_at;

            let mut metric_events = vec![];

            // Log elapsed time
            metric_events.push(MetricEvent {
                metric_id: metrics::PNO_SCAN_CANCELLED_ELAPSED_TIME_METRIC_ID,
                event_codes: vec![if self.has_scan_results {
                    metrics::PnoScanCancelledElapsedTimeMetricDimensionHadAnyScanResults::True
                        as u32
                } else {
                    metrics::PnoScanCancelledElapsedTimeMetricDimensionHadAnyScanResults::False
                        as u32
                }],
                payload: MetricEventPayload::IntegerValue(elapsed.into_millis()),
            });

            // Log cancellation source and presence of scan results
            let had_scan_results = if self.has_scan_results {
                metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionHasScanResults::True as u32
            } else {
                metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionHasScanResults::False as u32
            };

            let cancellation_source = match reason {
                PnoScanDisabledReason::ApiRequest => metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionCancellationSource::ApiRequest as u32,
                PnoScanDisabledReason::Internal => metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionCancellationSource::Internal as u32,
                PnoScanDisabledReason::Firmware => metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionCancellationSource::Firmware as u32,
            };

            metric_events.push(MetricEvent {
                metric_id: metrics::PNO_SCAN_CANCELLATION_BREAKDOWN_BY_RESULTS_AND_SOURCE_METRIC_ID,
                event_codes: vec![had_scan_results, cancellation_source],
                payload: MetricEventPayload::Count(1),
            });

            log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_pno_scan_disabled");
        }

        self.has_scan_results = false;
    }

    pub async fn handle_periodic_telemetry(&mut self) {
        if let Some(enabled_at) = self.enabled_at {
            let elapsed = fasync::BootInstant::now() - enabled_at;
            let hours = elapsed.into_hours();
            let capped_hours = std::cmp::min(hours, 24);

            let metric_events = vec![MetricEvent {
                metric_id: metrics::ONGOING_PNO_SCAN_ELAPSED_HOURS_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::IntegerValue(capped_hours),
            }];
            log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_periodic_telemetry");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::setup_test;
    use fidl_fuchsia_metrics::MetricEventPayload;
    use futures::task::Poll;
    use std::pin::pin;

    #[fuchsia::test]
    fn test_pno_scan_collision() {
        let mut test_helper = setup_test();
        let mut logger = PnoScanLogger::new(test_helper.filtered_cobalt_logger());

        {
            let mut test_fut = pin!(logger.handle_pno_scan_enabled(false));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        // Call again to trigger collision
        {
            let mut test_fut = pin!(logger.handle_pno_scan_enabled(false));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics = test_helper.get_logged_metrics(metrics::PNO_SCAN_REQUEST_COLLISION_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_pno_scan_enabled_while_connected() {
        let mut test_helper = setup_test();
        let mut logger = PnoScanLogger::new(test_helper.filtered_cobalt_logger());

        {
            let mut test_fut = pin!(logger.handle_pno_scan_enabled(true));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::PNO_SCAN_ENABLED_WHILE_CONNECTED_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_pno_scan_results_received_metrics() {
        let mut test_helper = setup_test();
        let mut logger = PnoScanLogger::new(test_helper.filtered_cobalt_logger());

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000));
        {
            let mut test_fut = pin!(logger.handle_pno_scan_enabled(false));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(25_000_000));
        {
            let mut test_fut = pin!(logger.handle_pno_scan_results_received());
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::PNO_SCAN_FIRST_RESULTS_ELAPSED_TIME_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(15)); // 15ms
    }

    #[fuchsia::test]
    fn test_pno_scan_disabled_no_results() {
        let mut test_helper = setup_test();
        let mut logger = PnoScanLogger::new(test_helper.filtered_cobalt_logger());

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000));
        {
            let mut test_fut = pin!(logger.handle_pno_scan_enabled(false));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(30_000_000));
        {
            let mut test_fut =
                pin!(logger.handle_pno_scan_disabled(PnoScanDisabledReason::Internal));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::PNO_SCAN_CANCELLED_ELAPSED_TIME_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(20)); // 20ms
        assert_eq!(
            metrics[0].event_codes,
            vec![
                metrics::PnoScanCancelledElapsedTimeMetricDimensionHadAnyScanResults::False as u32
            ]
        ); // no results

        let metrics = test_helper.get_logged_metrics(
            metrics::PNO_SCAN_CANCELLATION_BREAKDOWN_BY_RESULTS_AND_SOURCE_METRIC_ID,
        );
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].event_codes, vec![
            metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionHasScanResults::False as u32,
            metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionCancellationSource::Internal as u32
        ]); // no results, Internal
    }

    #[fuchsia::test]
    fn test_pno_scan_disabled_with_results() {
        let mut test_helper = setup_test();
        let mut logger = PnoScanLogger::new(test_helper.filtered_cobalt_logger());

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000));
        {
            let mut test_fut = pin!(logger.handle_pno_scan_enabled(false));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(20_000_000));
        {
            let mut test_fut = pin!(logger.handle_pno_scan_results_received());
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::PNO_SCAN_FIRST_RESULTS_ELAPSED_TIME_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(10)); // 10ms

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(30_000_000));
        {
            let mut test_fut =
                pin!(logger.handle_pno_scan_disabled(PnoScanDisabledReason::ApiRequest));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::PNO_SCAN_CANCELLED_ELAPSED_TIME_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(20)); // 20ms
        assert_eq!(
            metrics[0].event_codes,
            vec![metrics::PnoScanCancelledElapsedTimeMetricDimensionHadAnyScanResults::True as u32]
        ); // had results

        let metrics = test_helper.get_logged_metrics(
            metrics::PNO_SCAN_CANCELLATION_BREAKDOWN_BY_RESULTS_AND_SOURCE_METRIC_ID,
        );
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].event_codes, vec![
            metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionHasScanResults::True as u32,
            metrics::PnoScanCancellationBreakdownByResultsAndSourceMetricDimensionCancellationSource::ApiRequest as u32
        ]); // had results, ApiRequest
    }

    #[fuchsia::test]
    fn test_pno_scan_periodic_telemetry() {
        let mut test_helper = setup_test();
        let mut logger = PnoScanLogger::new(test_helper.filtered_cobalt_logger());

        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000));
        {
            let mut test_fut = pin!(logger.handle_pno_scan_enabled(false));
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        // Advance time by 5 hours
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(
            10_000_000 + 5 * 3600 * 1_000_000_000,
        ));
        {
            let mut test_fut = pin!(logger.handle_periodic_telemetry());
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::ONGOING_PNO_SCAN_ELAPSED_HOURS_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::IntegerValue(5));

        // Advance time by another 20 hours (total 25)
        test_helper.exec.set_fake_time(fasync::MonotonicInstant::from_nanos(
            10_000_000 + 25 * 3600 * 1_000_000_000,
        ));
        {
            let mut test_fut = pin!(logger.handle_periodic_telemetry());
            assert_eq!(
                test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
                Poll::Ready(())
            );
        }

        let metrics =
            test_helper.get_logged_metrics(metrics::ONGOING_PNO_SCAN_ELAPSED_HOURS_METRIC_ID);
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[1].payload, MetricEventPayload::IntegerValue(24)); // Capped at 24
    }
}
