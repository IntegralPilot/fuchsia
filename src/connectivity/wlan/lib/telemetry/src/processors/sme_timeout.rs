// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use cobalt_client::traits::AsEventCode;
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use std::sync::Arc;

use wlan_legacy_metrics_registry as metrics;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeoutSource {
    Scan,
    Connect,
    Disconnect,
    ClientStatus,
    WmmStatus,
    ApStart,
    ApStop,
    ApStatus,
    GetIfaceStats,
    GetHistogramStats,
}

impl TimeoutSource {
    fn to_metric_dimension(self) -> metrics::SmeOperationTimeoutMetricDimensionStalledOperation {
        use metrics::SmeOperationTimeoutMetricDimensionStalledOperation::*;
        match self {
            Self::Scan => Scan_,
            Self::Connect => Connect_,
            Self::Disconnect => Disconnect_,
            Self::ClientStatus => ClientStatus_,
            Self::WmmStatus => WmmStatus_,
            Self::ApStart => ApStart_,
            Self::ApStop => ApStop_,
            Self::ApStatus => ApStatus_,
            Self::GetIfaceStats => GetCounterStats_,
            Self::GetHistogramStats => GetHistogramStats_,
        }
    }
}

pub struct SmeTimeoutLogger {
    cobalt_proxy: Arc<FilteredCobaltLogger>,
}

impl SmeTimeoutLogger {
    pub fn new(cobalt_proxy: Arc<FilteredCobaltLogger>) -> Self {
        Self { cobalt_proxy }
    }

    pub async fn handle_sme_timeout_event(&self, source: TimeoutSource) {
        let metric_events = vec![
            MetricEvent {
                metric_id: metrics::SME_OPERATION_TIMEOUT_2_METRIC_ID,
                event_codes: vec![],
                payload: MetricEventPayload::Count(1),
            },
            MetricEvent {
                metric_id: metrics::SME_OPERATION_TIMEOUT_METRIC_ID,
                event_codes: vec![source.to_metric_dimension().as_event_code()],
                payload: MetricEventPayload::Count(1),
            },
        ];
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_sme_timeout_event");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestHelper, setup_test};
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;

    fn run_handle_sme_timeout_event(
        test_helper: &mut TestHelper,
        sme_timeout_logger: &SmeTimeoutLogger,
        source: TimeoutSource,
    ) {
        let mut test_fut = pin!(sme_timeout_logger.handle_sme_timeout_event(source));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );
    }

    #[test_case(
        TimeoutSource::Scan,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::Scan_ ;
        "log scan timeout"
    )]
    #[test_case(
        TimeoutSource::Connect,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::Connect_ ;
        "log connect"
    )]
    #[test_case(
        TimeoutSource::Disconnect,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::Disconnect_ ;
        "log disconnect timeout"
    )]
    #[test_case(
        TimeoutSource::ClientStatus,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::ClientStatus_ ;
        "log client status timeout"
    )]
    #[test_case(
        TimeoutSource::WmmStatus,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::WmmStatus_ ;
        "log WMM status timeout"
    )]
    #[test_case(
        TimeoutSource::ApStart,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::ApStart_ ;
        "log AP start timeout"
    )]
    #[test_case(
        TimeoutSource::ApStop,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::ApStop_ ;
        "log Ap stop timeout"
    )]
    #[test_case(
        TimeoutSource::ApStatus,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::ApStatus_ ;
        "log AP status timeout"
    )]
    #[test_case(
        TimeoutSource::GetIfaceStats,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::GetCounterStats_ ;
        "log iface stats timeout"
    )]
    #[test_case(
        TimeoutSource::GetHistogramStats,
        metrics::SmeOperationTimeoutMetricDimensionStalledOperation::GetHistogramStats_ ;
        "log histogram stats timeout"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_handle_sme_timeout_event_with_source(
        source: TimeoutSource,
        expected_dimension: metrics::SmeOperationTimeoutMetricDimensionStalledOperation,
    ) {
        let mut test_helper = setup_test();
        let sme_timeout_logger = SmeTimeoutLogger::new(test_helper.filtered_cobalt_logger());

        run_handle_sme_timeout_event(&mut test_helper, &sme_timeout_logger, source);

        let metrics_2 = test_helper.get_logged_metrics(metrics::SME_OPERATION_TIMEOUT_2_METRIC_ID);
        assert_eq!(metrics_2.len(), 1);
        assert_eq!(metrics_2[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper.get_logged_metrics(metrics::SME_OPERATION_TIMEOUT_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
        assert_eq!(metrics[0].event_codes, vec![expected_dimension.as_event_code()]);
    }
}
