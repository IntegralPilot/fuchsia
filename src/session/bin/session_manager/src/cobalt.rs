// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, format_err};
use fidl_fuchsia_metrics::{MetricEventLoggerFactoryMarker, MetricEventLoggerProxy, ProjectSpec};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use log::warn;
use session_framework_metrics_registry::cobalt_registry as metrics;

/// Creates a `LoggerProxy` connected to Cobalt.
///
/// The connection is performed in a Future run on the global executor, but the `LoggerProxy`
/// can be used immediately.
///
/// # Returns
/// `LoggerProxy` for log messages to be sent to.
pub fn get_logger() -> Result<MetricEventLoggerProxy, Error> {
    let (logger_proxy, server_end) = fidl::endpoints::create_proxy();
    let logger_factory = connect_to_protocol::<MetricEventLoggerFactoryMarker>()
        .context("Failed to connect to the Cobalt MetricEventLoggerFactory")?;

    fasync::Task::spawn(async move {
        if let Err(err) = logger_factory
            .create_metric_event_logger(
                &ProjectSpec { project_id: Some(metrics::PROJECT_ID), ..Default::default() },
                server_end,
            )
            .await
        {
            warn!(err:%; "Failed to create Cobalt logger");
        }
    })
    .detach();

    Ok(logger_proxy)
}

/// Reports the time elapsed while launching a session.
///
/// # Parameters
/// - `logger_proxy`: The cobalt logger.
/// - `start_time`: The time when `session_manager` starts launching a session.
/// - `end_time`: The time when `session_manager` has bound to a session. This must be strictly after
///   `start_time`.
///
/// # Returns
/// `Ok` if the time elapsed was logged successfully.
pub async fn log_session_launch_time(
    logger_proxy: MetricEventLoggerProxy,
    start_time: zx::MonotonicInstant,
    end_time: zx::MonotonicInstant,
) -> Result<(), Error> {
    let elapsed_time = (end_time - start_time).into_micros();
    if elapsed_time < 0 {
        return Err(format_err!("End time must be after start time."));
    }

    logger_proxy
        .log_integer(
            metrics::SESSION_LAUNCH_TIME_MIGRATED_METRIC_ID,
            elapsed_time,
            &[metrics::SessionLaunchTimeMigratedMetricDimensionStatus::Success as u32],
        )
        .await
        .context("Could not log session launch time.")?
        .map_err(|e| format_err!("Logging session launch time returned an error: {e:?}"))?;

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_metrics::{MetricEventLoggerMarker, MetricEventLoggerRequest};
    use futures::TryStreamExt;

    /// Tests that the right payload is sent to Cobalt when logging the session launch time.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_log_session_launch_time() {
        let (logger_proxy, mut logger_server) =
            create_proxy_and_stream::<MetricEventLoggerMarker>();
        let start_time = zx::MonotonicInstant::from_nanos(0);
        let end_time = zx::MonotonicInstant::from_nanos(5000);

        fasync::Task::spawn(async move {
            let _ = log_session_launch_time(logger_proxy, start_time, end_time).await;
        })
        .detach();

        assert_matches!(
            logger_server.try_next().await.unwrap(),
            Some(MetricEventLoggerRequest::LogInteger {
                metric_id: metrics::SESSION_LAUNCH_TIME_MIGRATED_METRIC_ID,
                value: 5,
                event_codes,
                responder: _,
            }) => {
                assert_eq!(
                    event_codes,
                    vec![metrics::SessionLaunchTimeMigratedMetricDimensionStatus::Success as u32]
                );
            }
        )
    }

    /// Tests that an error is raised if `end_time` < `start_time`.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_log_session_launch_time_swap_start_end_time() {
        let (logger_proxy, _logger_server) = create_proxy_and_stream::<MetricEventLoggerMarker>();
        let start_time = zx::MonotonicInstant::from_nanos(0);
        let end_time = zx::MonotonicInstant::from_nanos(5000);

        assert!(log_session_launch_time(logger_proxy, end_time, start_time).await.is_err());
    }
}
