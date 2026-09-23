// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use log::warn;
use {fidl_fuchsia_metrics as metrics, fuchsia_async as fasync};

pub use bt_metrics_registry::*;

/// Connects to the MetricEventLoggerFactory service to create a
/// MetricEventLoggerProxy for the caller.
pub fn create_metrics_logger() -> Result<metrics::MetricEventLoggerProxy, Error> {
    let factory_proxy =
        fuchsia_component::client::connect_to_protocol::<metrics::MetricEventLoggerFactoryMarker>()
            .context("connecting to metrics")?;

    let (cobalt_proxy, cobalt_server) =
        fidl::endpoints::create_proxy::<metrics::MetricEventLoggerMarker>();

    let project_spec = metrics::ProjectSpec {
        customer_id: None, // defaults to fuchsia
        project_id: Some(PROJECT_ID),
        ..Default::default()
    };

    fasync::Task::spawn(async move {
        match factory_proxy.create_metric_event_logger(&project_spec, cobalt_server).await {
            Err(e) => warn!("FIDL failure setting up event logger: {e:?}"),
            Ok(Err(e)) => warn!("CreateMetricEventLogger failure: {e:?}"),
            Ok(Ok(())) => {}
        }
    })
    .detach();

    Ok(cobalt_proxy)
}

pub fn log_on_failure(result: Result<Result<(), metrics::Error>, fidl::Error>) {
    match result {
        Ok(Ok(())) => (),
        e => warn!("failed to log metrics: {:?}", e),
    };
}

/// An optional client connection to the Cobalt logging service.
#[derive(Clone, Default)]
pub struct MetricsLogger(Option<metrics::MetricEventLoggerProxy>);

impl MetricsLogger {
    pub fn new() -> Self {
        let logger =
            create_metrics_logger().map_err(|e| warn!("Failed to create metrics logger: {e}")).ok();
        Self(logger)
    }

    /// Test-only
    pub fn from_proxy(proxy: metrics::MetricEventLoggerProxy) -> Self {
        Self(Some(proxy))
    }

    /// Logs an occurrence metric using the Cobalt logger. Does not block execution.
    pub fn log_occurrence(&self, id: u32, event_codes: Vec<u32>) {
        let Some(c) = self.0.clone() else { return };
        fuchsia_async::Task::spawn(async move {
            log_on_failure(c.log_occurrence(id, 1, &event_codes).await);
        })
        .detach();
    }

    /// Logs an integer metric using the Cobalt logger. Does not block execution.
    pub fn log_integer(&self, id: u32, value: i64, event_codes: Vec<u32>) {
        let Some(c) = self.0.clone() else { return };
        fuchsia_async::Task::spawn(async move {
            log_on_failure(c.log_integer(id, value, &event_codes).await);
        })
        .detach();
    }

    pub fn log_occurrences<I: 'static + IntoIterator<Item = u32> + std::marker::Send>(
        &self,
        id: u32,
        event_codes: I,
    ) where
        <I as IntoIterator>::IntoIter: std::marker::Send,
    {
        let Some(c) = self.0.clone() else { return };
        fasync::Task::spawn(async move {
            for code in event_codes {
                log_on_failure(c.log_occurrence(id, 1, &[code]).await);
            }
        })
        .detach();
    }
}

/// Test-only
pub fn respond_to_metrics_req_for_test(
    request: metrics::MetricEventLoggerRequest,
) -> metrics::MetricEvent {
    match request {
        metrics::MetricEventLoggerRequest::LogOccurrence {
            metric_id,
            count,
            event_codes,
            responder,
        } => {
            let _ = responder.send(Ok(())).unwrap();
            metrics::MetricEvent {
                metric_id,
                event_codes,
                payload: metrics::MetricEventPayload::Count(count),
            }
        }
        metrics::MetricEventLoggerRequest::LogInteger {
            metric_id,
            value,
            event_codes,
            responder,
        } => {
            let _ = responder.send(Ok(())).unwrap();
            metrics::MetricEvent {
                metric_id,
                event_codes,
                payload: metrics::MetricEventPayload::IntegerValue(value),
            }
        }
        _ => panic!("unexpected logging to Cobalt"),
    }
}
