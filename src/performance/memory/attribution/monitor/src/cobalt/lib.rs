// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use memory_metrics_registry::cobalt_registry;

use fidl_fuchsia_metrics as fmetrics;

mod buckets;
mod stalls;

pub use buckets::{prepare_bucket_codes, upload_metrics};
pub use stalls::collect_stalls_forever;

fn error_from_metrics_error(error: fmetrics::Error) -> anyhow::Error {
    anyhow!("{:?}", error)
}

/// Provided a connection to a MetricEventLoggerFactory, request a MetricEventLogger appropriately
/// configured to log memory metrics.
pub async fn create_metric_event_logger(
    factory: fmetrics::MetricEventLoggerFactoryProxy,
) -> Result<fmetrics::MetricEventLoggerProxy> {
    let project_spec = fmetrics::ProjectSpec {
        customer_id: Some(cobalt_registry::CUSTOMER_ID),
        project_id: Some(cobalt_registry::PROJECT_ID),
        ..Default::default()
    };
    let (metric_event_logger, server_end) = fidl::endpoints::create_proxy();
    factory
        .create_metric_event_logger(&project_spec, server_end)
        .await?
        .map_err(error_from_metrics_error)?;
    Ok(metric_event_logger)
}
