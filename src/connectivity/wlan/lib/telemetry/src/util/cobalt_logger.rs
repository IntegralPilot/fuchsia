// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::CobaltAllowlist;
use fidl_fuchsia_metrics::{HistogramBucket, MetricEvent, MetricEventLoggerProxy};

/// Macro wrapper for logging simple events (occurrence, integer, histogram, string)
/// and log a warning when the status is not Ok.
// TODO(339221340): remove these allows once the skeleton has a few uses
#[allow(unused)]
macro_rules! log_cobalt {
    ($cobalt_proxy:expr, $method_name:ident, $metric_id:expr, $value:expr, $event_codes:expr $(,)?) => {{
        let status = $cobalt_proxy.$method_name($metric_id, $value, $event_codes).await;
        match status {
            Ok(Ok(())) => (),
            Ok(Err(e)) => log::info!("Failed logging metric: {}, error: {:?}", $metric_id, e),
            Err(e) => log::info!("Failed logging metric: {}, error: {}", $metric_id, e),
        }
    }};
}

macro_rules! log_cobalt_batch {
    ($cobalt_proxy:expr, $events:expr, $context:expr $(,)?) => {{
        if !$events.is_empty() {
            let status = $cobalt_proxy.log_metric_events($events).await;
            match status {
                Ok(Ok(())) => (),
                Ok(Err(e)) => {
                    log::info!(
                        "Failed logging batch metrics, context: {}, error: {:?}",
                        $context,
                        e
                    );
                }
                Err(e) => {
                    log::info!("Failed logging batch metrics, context: {}, error: {}", $context, e)
                }
            }
        }
    }};
}

// TODO(339221340): remove these allows once the skeleton has a few uses
#[allow(unused)]
pub(crate) use {log_cobalt, log_cobalt_batch};
pub struct FilteredCobaltLogger {
    proxy: MetricEventLoggerProxy,
    allowlist: CobaltAllowlist,
}

impl FilteredCobaltLogger {
    pub fn new(proxy: MetricEventLoggerProxy, allowlist: CobaltAllowlist) -> Self {
        Self { proxy, allowlist }
    }

    pub async fn log_occurrence(
        &self,
        metric_id: u32,
        count: u64,
        event_codes: &[u32],
    ) -> Result<Result<(), fidl_fuchsia_metrics::Error>, fidl::Error> {
        if !self.allowlist.contains(metric_id) {
            return Ok(Ok(()));
        }
        self.proxy.log_occurrence(metric_id, count, event_codes).await
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn log_integer(
        &self,
        metric_id: u32,
        value: i64,
        event_codes: &[u32],
    ) -> Result<Result<(), fidl_fuchsia_metrics::Error>, fidl::Error> {
        if !self.allowlist.contains(metric_id) {
            return Ok(Ok(()));
        }
        self.proxy.log_integer(metric_id, value, event_codes).await
    }

    #[expect(dead_code)]
    pub async fn log_string(
        &self,
        metric_id: u32,
        string_value: &str,
        event_codes: &[u32],
    ) -> Result<Result<(), fidl_fuchsia_metrics::Error>, fidl::Error> {
        if !self.allowlist.contains(metric_id) {
            return Ok(Ok(()));
        }
        self.proxy.log_string(metric_id, string_value, event_codes).await
    }

    #[expect(dead_code)]
    pub async fn log_integer_histogram(
        &self,
        metric_id: u32,
        histogram: &[HistogramBucket],
        event_codes: &[u32],
    ) -> Result<Result<(), fidl_fuchsia_metrics::Error>, fidl::Error> {
        if !self.allowlist.contains(metric_id) {
            return Ok(Ok(()));
        }
        self.proxy.log_integer_histogram(metric_id, histogram, event_codes).await
    }

    pub async fn log_metric_events(
        &self,
        events: &[MetricEvent],
    ) -> Result<Result<(), fidl_fuchsia_metrics::Error>, fidl::Error> {
        let filtered_events: Vec<MetricEvent> =
            events.iter().filter(|e| self.allowlist.contains(e.metric_id)).cloned().collect();
        if filtered_events.is_empty() {
            return Ok(Ok(()));
        }
        self.proxy.log_metric_events(&filtered_events).await
    }
}

#[cfg(test)]
mod tests {
    use crate::config::CobaltAllowlist;
    use crate::testing::setup_test;
    use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
    use std::collections::HashSet;
    use std::pin::pin;
    use std::task::Poll;

    #[fuchsia::test]
    fn test_filtered_cobalt_logger_allow_all() {
        let mut test_helper = setup_test();
        let logger = test_helper.filtered_cobalt_logger();

        let mut test_fut = pin!(async move {
            logger.log_occurrence(100, 1, &[]).await.unwrap().unwrap();
            logger.log_integer(101, 42, &[]).await.unwrap().unwrap();
        });

        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        assert_eq!(test_helper.cobalt_events.len(), 2);
    }

    #[fuchsia::test]
    fn test_filtered_cobalt_logger_allowlist_only() {
        let mut test_helper = setup_test();
        let allowlist = CobaltAllowlist::Only(HashSet::from([1, 2]));
        let logger = test_helper.filtered_cobalt_logger_with_allowlist(allowlist);

        let mut test_fut = pin!(async move {
            logger.log_occurrence(1, 1, &[]).await.unwrap().unwrap();
            logger.log_occurrence(3, 1, &[]).await.unwrap().unwrap();
            let events = vec![
                MetricEvent {
                    metric_id: 2,
                    event_codes: vec![],
                    payload: MetricEventPayload::Count(1),
                },
                MetricEvent {
                    metric_id: 4,
                    event_codes: vec![],
                    payload: MetricEventPayload::Count(1),
                },
            ];
            logger.log_metric_events(&events).await.unwrap().unwrap();
        });

        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
        assert_eq!(test_helper.cobalt_events.len(), 2);
        assert_eq!(test_helper.cobalt_events[0].metric_id, 1);
        assert_eq!(test_helper.cobalt_events[1].metric_id, 2);
    }
}
