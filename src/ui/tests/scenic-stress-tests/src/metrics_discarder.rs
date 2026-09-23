// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_metrics::{
    MetricEventLoggerFactoryRequest, MetricEventLoggerFactoryRequestStream,
    MetricEventLoggerRequestStream,
};
use fuchsia_component_test::LocalComponentHandles;
use futures::StreamExt as _;

/// Registers a discoverable endpoint for connecting to `fuchsia.metrics.MetricEventLoggerFactory`.
///
/// Requests to create a logger will always succeed, and the metrics reported via those loggers
/// will always succeed, silently discarding the metrics provided to
/// `fuchsia.metrics.MetricEventLogger`.
pub(crate) async fn serve_metric_event_logger_factory(
    handles: LocalComponentHandles,
) -> Result<(), anyhow::Error> {
    let mut fs = fuchsia_component::server::ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: MetricEventLoggerFactoryRequestStream| stream);
    fs.serve_connection(handles.outgoing_dir)?;
    fs.for_each_concurrent(0, handle_metric_event_logger_factory_request_stream).await;
    Ok(())
}

async fn handle_metric_event_logger_factory_request_stream(
    mut stream: MetricEventLoggerFactoryRequestStream,
) {
    while let Some(request) = stream.next().await {
        match request {
            Ok(MetricEventLoggerFactoryRequest::CreateMetricEventLogger {
                logger,
                responder,
                ..
            }) => {
                // `ok()`: Bad logger channel is fine.
                let s = logger.into_stream();
                fuchsia_async::Task::local(handle_metric_event_logger_client(s)).detach();
                responder.send(Ok(())).ok();
            }
            Ok(MetricEventLoggerFactoryRequest::CreateMetricEventLoggerWithExperiments {
                logger,
                responder,
                ..
            }) => {
                // `ok()`: Bad logger channel is fine.
                let s = logger.into_stream();
                fuchsia_async::Task::local(handle_metric_event_logger_client(s)).detach();
                responder.send(Ok(())).ok();
            }
            Err(_) => (), // Causing errors on the metrics logger factory is fine
        }
    }
}

async fn handle_metric_event_logger_client(mut stream: MetricEventLoggerRequestStream) {
    use fidl_fuchsia_metrics::MetricEventLoggerRequest::*;
    while let Some(request) = stream.next().await {
        match request {
            Ok(LogOccurrence { responder, .. }) => {
                responder.send(Ok(())).ok();
            }
            Ok(LogInteger { responder, .. }) => {
                responder.send(Ok(())).ok();
            }
            Ok(LogIntegerHistogram { responder, .. }) => {
                responder.send(Ok(())).ok();
            }
            Ok(LogString { responder, .. }) => {
                responder.send(Ok(())).ok();
            }
            Ok(LogMetricEvents { responder, .. }) => {
                responder.send(Ok(())).ok();
            }
            Err(_) => (), // Causing errors on the metrics logger is fine
        }
    }
}
