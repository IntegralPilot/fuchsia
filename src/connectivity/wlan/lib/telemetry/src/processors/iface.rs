// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use std::sync::Arc;

use wlan_legacy_metrics_registry as metrics;

pub struct IfaceLogger {
    cobalt_proxy: Arc<FilteredCobaltLogger>,
}

impl IfaceLogger {
    pub fn new(cobalt_proxy: Arc<FilteredCobaltLogger>) -> Self {
        Self { cobalt_proxy }
    }

    pub async fn handle_iface_creation_failure(&self) {
        let metric_events = vec![MetricEvent {
            metric_id: metrics::INTERFACE_CREATION_FAILURE_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_iface_creation_failure");
    }

    pub async fn handle_iface_destruction_failure(&self) {
        let metric_events = vec![MetricEvent {
            metric_id: metrics::INTERFACE_DESTRUCTION_FAILURE_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_iface_destruction_failure");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestHelper, setup_test};
    use futures::task::Poll;
    use std::pin::pin;

    fn run_handle_iface_creation_failure(test_helper: &mut TestHelper, iface_logger: &IfaceLogger) {
        let mut test_fut = pin!(iface_logger.handle_iface_creation_failure());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    fn run_handle_iface_destruction_failure(
        test_helper: &mut TestHelper,
        iface_logger: &IfaceLogger,
    ) {
        let mut test_fut = pin!(iface_logger.handle_iface_destruction_failure());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    #[fuchsia::test]
    fn test_handle_iface_creation_failure() {
        let mut test_helper = setup_test();
        let iface_logger = IfaceLogger::new(test_helper.filtered_cobalt_logger());

        run_handle_iface_creation_failure(&mut test_helper, &iface_logger);

        let metrics = test_helper.get_logged_metrics(metrics::INTERFACE_CREATION_FAILURE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_handle_iface_event() {
        let mut test_helper = setup_test();
        let iface_logger = IfaceLogger::new(test_helper.filtered_cobalt_logger());

        run_handle_iface_destruction_failure(&mut test_helper, &iface_logger);

        let metrics =
            test_helper.get_logged_metrics(metrics::INTERFACE_DESTRUCTION_FAILURE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }
}
