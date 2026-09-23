// Copyright 2024 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use anyhow::Error;
use log::info;
use omaha_client::metrics::{Metrics, MetricsReporter};

/// A minimal implementation of MetricsReporter which only log metrics.
/// Similar to the Stub in the omaha-client lib, but derives clone.
#[derive(Clone, Debug)]
pub struct MinimalMetricsReporter;

impl MetricsReporter for MinimalMetricsReporter {
    fn report_metrics(&mut self, metrics: Metrics) -> Result<(), Error> {
        info!("Metrics received: {:?}", metrics);
        Ok(())
    }
}
