// Copyright 2019 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use crate::metrics::{Metrics, MetricsReporter};
use anyhow::Error;
use log::info;

/// A stub implementation of MetricsReporter which only log metrics.
#[derive(Debug)]
pub struct StubMetricsReporter;

impl MetricsReporter for StubMetricsReporter {
    fn report_metrics(&mut self, metrics: Metrics) -> Result<(), Error> {
        info!("Received request to report metrics: {:?}", metrics);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_stub_metrics_reporter() {
        let mut stub = StubMetricsReporter;
        let result = stub.report_metrics(Metrics::UpdateCheckResponseTime {
            response_time: Duration::from_secs(2),
            successful: true,
        });
        assert!(result.is_ok(), "{result:?}");
    }
}
