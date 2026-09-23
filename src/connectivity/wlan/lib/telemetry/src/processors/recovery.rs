// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use std::sync::Arc;

use wlan_legacy_metrics_registry as metrics;

pub struct RecoveryLogger {
    cobalt_proxy: Arc<FilteredCobaltLogger>,
}

impl RecoveryLogger {
    pub fn new(cobalt_proxy: Arc<FilteredCobaltLogger>) -> Self {
        Self { cobalt_proxy }
    }

    pub async fn handle_recovery_event(&self) {
        let metric_events = vec![MetricEvent {
            metric_id: metrics::RECOVERY_OCCURRENCE_2_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_recovery_event");
    }

    pub async fn handle_recovery_result(&self, result: Result<(), ()>) {
        let event_code = match result {
            Ok(()) => metrics::TriggerSubsystemResetBreakdownByResultMetricDimensionResult::Success,
            Err(()) => {
                metrics::TriggerSubsystemResetBreakdownByResultMetricDimensionResult::Failure
            }
        };
        let metric_events = vec![MetricEvent {
            metric_id: metrics::TRIGGER_SUBSYSTEM_RESET_BREAKDOWN_BY_RESULT_METRIC_ID,
            event_codes: vec![event_code as u32],
            payload: MetricEventPayload::Count(1),
        }];
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_recovery_result");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestHelper, setup_test};
    use futures::task::Poll;
    use std::pin::pin;

    fn run_handle_recovery_event(test_helper: &mut TestHelper, recovery_logger: &RecoveryLogger) {
        let mut test_fut = pin!(recovery_logger.handle_recovery_event());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    fn run_handle_recovery_result(
        test_helper: &mut TestHelper,
        recovery_logger: &RecoveryLogger,
        result: Result<(), ()>,
    ) {
        let mut test_fut = pin!(recovery_logger.handle_recovery_result(result));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    #[fuchsia::test]
    fn test_handle_recovery_event() {
        let mut test_helper = setup_test();
        let recovery_logger = RecoveryLogger::new(test_helper.filtered_cobalt_logger());

        run_handle_recovery_event(&mut test_helper, &recovery_logger);

        let recovery_occurrence_metrics =
            test_helper.get_logged_metrics(metrics::RECOVERY_OCCURRENCE_2_METRIC_ID);
        assert_eq!(recovery_occurrence_metrics.len(), 1);
        assert_eq!(recovery_occurrence_metrics[0].payload, MetricEventPayload::Count(1));

        let recovery_result_metrics = test_helper
            .get_logged_metrics(metrics::TRIGGER_SUBSYSTEM_RESET_BREAKDOWN_BY_RESULT_METRIC_ID);
        assert_eq!(recovery_result_metrics.len(), 0);
    }

    #[fuchsia::test]
    fn test_handle_recovery_result_success() {
        let mut test_helper = setup_test();
        let recovery_logger = RecoveryLogger::new(test_helper.filtered_cobalt_logger());

        run_handle_recovery_result(&mut test_helper, &recovery_logger, Ok(()));

        let recovery_occurrence_metrics =
            test_helper.get_logged_metrics(metrics::RECOVERY_OCCURRENCE_2_METRIC_ID);
        assert_eq!(recovery_occurrence_metrics.len(), 0);

        let recovery_result_metrics = test_helper
            .get_logged_metrics(metrics::TRIGGER_SUBSYSTEM_RESET_BREAKDOWN_BY_RESULT_METRIC_ID);
        assert_eq!(recovery_result_metrics.len(), 1);
        assert_eq!(
            recovery_result_metrics[0].event_codes,
            vec![
                metrics::TriggerSubsystemResetBreakdownByResultMetricDimensionResult::Success
                    as u32
            ]
        );
    }

    #[fuchsia::test]
    fn test_handle_recovery_result_failure() {
        let mut test_helper = setup_test();
        let recovery_logger = RecoveryLogger::new(test_helper.filtered_cobalt_logger());

        run_handle_recovery_result(&mut test_helper, &recovery_logger, Err(()));

        let recovery_occurrence_metrics =
            test_helper.get_logged_metrics(metrics::RECOVERY_OCCURRENCE_2_METRIC_ID);
        assert_eq!(recovery_occurrence_metrics.len(), 0);

        let recovery_result_metrics = test_helper
            .get_logged_metrics(metrics::TRIGGER_SUBSYSTEM_RESET_BREAKDOWN_BY_RESULT_METRIC_ID);
        assert_eq!(recovery_result_metrics.len(), 1);
        assert_eq!(
            recovery_result_metrics[0].event_codes,
            vec![
                metrics::TriggerSubsystemResetBreakdownByResultMetricDimensionResult::Failure
                    as u32
            ]
        );
    }
}
