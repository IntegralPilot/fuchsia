// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use cobalt_client::traits::AsEventCode;
use futures::StreamExt;
use memory_metrics_registry::cobalt_registry;
use stalls::{MemoryStallMetrics, StallProvider};
use zx::MonotonicInstant;
use {anyhow, fidl_fuchsia_metrics as fmetrics};

use crate::error_from_metrics_error;

/// Collect and publish to Cobalt memory stall increase rate, every hour.
pub async fn collect_stalls_forever(
    stalls_provider: impl StallProvider,
    metric_event_logger: fmetrics::MetricEventLoggerProxy,
) -> Result<()> {
    let mut last_stall = MemoryStallMetrics::default();

    // Wait for one hour after device start to get the first stall value. We don't use the one-hour
    // timer as we may have been started later than at boot exactly.
    fuchsia_async::Timer::new(MonotonicInstant::ZERO + zx::Duration::from_hours(1)).await;

    let mut timer = fuchsia_async::Interval::new(zx::Duration::from_hours(1));
    loop {
        let new_stall = stalls_provider.get_stall_info()?;

        // The Cobalt metrics for stalls expect milliseconds, as defined in the Cobalt registry.
        let stall_some_event = fmetrics::MetricEvent {
            metric_id: cobalt_registry::MEMORY_STALLS_PER_HOUR_METRIC_ID,
            payload: fmetrics::MetricEventPayload::IntegerValue(i64::try_from(
                (new_stall.some - last_stall.some).as_millis(),
            )?),
            event_codes: vec![
                cobalt_registry::MemoryMetricDimensionStallType::Some.as_event_code(),
            ],
        };
        let stall_full_event = fmetrics::MetricEvent {
            metric_id: cobalt_registry::MEMORY_STALLS_PER_HOUR_METRIC_ID,
            payload: fmetrics::MetricEventPayload::IntegerValue(i64::try_from(
                (new_stall.full - last_stall.full).as_millis(),
            )?),
            event_codes: vec![
                cobalt_registry::MemoryMetricDimensionStallType::Full.as_event_code(),
            ],
        };

        last_stall = new_stall;

        let events = vec![stall_some_event, stall_full_event];
        metric_event_logger.log_metric_events(&events).await?.map_err(error_from_metrics_error)?;
        timer.next().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use fuchsia_async as fasync;
    use futures::task::Poll;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    fn get_stall_provider() -> impl StallProvider {
        #[derive(Clone)]
        struct FakeStallProvider {
            count: Arc<AtomicU32>,
        }

        impl Default for FakeStallProvider {
            fn default() -> Self {
                Self { count: Arc::new(AtomicU32::new(1)) }
            }
        }

        impl StallProvider for FakeStallProvider {
            fn get_stall_info(&self) -> Result<MemoryStallMetrics, anyhow::Error> {
                let count = self.count.fetch_add(1, Ordering::Relaxed);
                let memory_stall = MemoryStallMetrics {
                    some: Duration::from_millis((count * 10).into()),
                    full: Duration::from_millis((count * 20).into()),
                };
                Ok(memory_stall)
            }
        }

        FakeStallProvider::default()
    }

    #[test]
    fn test_periodic_stalls_collection() -> anyhow::Result<()> {
        // Setup executor.
        let mut exec = fasync::TestExecutor::new_with_fake_time();

        // Setup mock data providers.
        let data_provider = get_stall_provider();

        // Setup test proxy to observe emitted events from the service.
        let (metric_event_logger, metric_event_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fmetrics::MetricEventLoggerMarker>();

        // Set the time to shortly after boot
        exec.set_fake_time(
            (zx::MonotonicInstant::ZERO + zx::Duration::from_seconds(3 * 60)).into(),
        );
        // Service under test.
        let mut stalls_collector = fuchsia_async::Task::spawn(async move {
            collect_stalls_forever(data_provider, metric_event_logger).await
        });

        // Give the service the opportunity to run.
        assert!(
            exec.run_until_stalled(&mut stalls_collector).is_pending(),
            "Stalls collection service returned unexpectedly early"
        );

        // Ensure no metrics has been uploaded yet.
        let mut metric_event_request_future = metric_event_request_stream.into_future();
        assert!(
            exec.run_until_stalled(&mut metric_event_request_future).is_pending(),
            "Stalls collection service returned unexpectedly early"
        );

        // Fake the passage of time, so that collect_metrics may do a capture.
        assert!(
            exec.run_until_stalled(&mut std::pin::pin!(fasync::TestExecutor::advance_to(
                exec.now() + zx::Duration::from_seconds(60 * 60 + 10)
            )))
            .is_ready(),
            "Failed to advance time"
        );

        // Ensure we have one and only one event ready for consumption.
        let Poll::Ready((event, metric_event_request_stream)) =
            exec.run_until_stalled(&mut metric_event_request_future)
        else {
            panic!("Failed to receive metrics")
        };
        let event = event.ok_or_else(|| anyhow!("Metrics stream unexpectedly closed"))??;
        match event {
            fmetrics::MetricEventLoggerRequest::LogMetricEvents { events, responder, .. } => {
                assert_eq!(events.len(), 2);
                // Kernel metrics
                assert_eq!(
                    events[0],
                    fmetrics::MetricEvent {
                        metric_id: cobalt_registry::MEMORY_STALLS_PER_HOUR_METRIC_ID,
                        event_codes: vec![
                            cobalt_registry::MemoryMetricDimensionStallType::Some.as_event_code()
                        ],
                        payload: fmetrics::MetricEventPayload::IntegerValue(10)
                    }
                );
                assert_eq!(
                    events[1],
                    fmetrics::MetricEvent {
                        metric_id: cobalt_registry::MEMORY_STALLS_PER_HOUR_METRIC_ID,
                        event_codes: vec![
                            cobalt_registry::MemoryMetricDimensionStallType::Full.as_event_code()
                        ],
                        payload: fmetrics::MetricEventPayload::IntegerValue(20)
                    }
                );
                responder.send(Ok(()))?;
            }
            _ => panic!("Unexpected metric event"),
        }

        let mut metric_event_request_future = metric_event_request_stream.into_future();

        assert!(exec.run_until_stalled(&mut metric_event_request_future).is_pending());

        // Advance to the next hour
        assert!(
            exec.run_until_stalled(&mut std::pin::pin!(fasync::TestExecutor::advance_to(
                (zx::MonotonicInstant::ZERO + zx::Duration::from_seconds(60 * 60 * 2 + 10)).into()
            )))
            .is_ready(),
            "Failed to advance time"
        );

        // Ensure we have one and only one event ready for consumption.
        let Poll::Ready((event, metric_event_request_stream)) =
            exec.run_until_stalled(&mut metric_event_request_future)
        else {
            panic!("Failed to receive metrics")
        };
        let event = event.ok_or_else(|| anyhow!("Metrics stream unexpectedly closed"))??;
        match event {
            fmetrics::MetricEventLoggerRequest::LogMetricEvents { events, responder, .. } => {
                assert_eq!(events.len(), 2);
                // Kernel metrics
                assert_eq!(
                    events[0],
                    fmetrics::MetricEvent {
                        metric_id: cobalt_registry::MEMORY_STALLS_PER_HOUR_METRIC_ID,
                        event_codes: vec![
                            cobalt_registry::MemoryMetricDimensionStallType::Some.as_event_code()
                        ],
                        payload: fmetrics::MetricEventPayload::IntegerValue(10)
                    }
                );
                assert_eq!(
                    events[1],
                    fmetrics::MetricEvent {
                        metric_id: cobalt_registry::MEMORY_STALLS_PER_HOUR_METRIC_ID,
                        event_codes: vec![
                            cobalt_registry::MemoryMetricDimensionStallType::Full.as_event_code()
                        ],
                        payload: fmetrics::MetricEventPayload::IntegerValue(20)
                    }
                );
                responder.send(Ok(()))?;
            }
            _ => panic!("Unexpected metric event"),
        }

        assert!(
            exec.run_until_stalled(&mut metric_event_request_stream.into_future()).is_pending()
        );

        Ok(())
    }
}
