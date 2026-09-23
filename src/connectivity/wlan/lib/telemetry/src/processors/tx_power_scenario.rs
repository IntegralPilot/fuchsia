// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::util::cobalt_logger::{FilteredCobaltLogger, log_cobalt_batch};
use cobalt_client::traits::AsEventCode;
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use fidl_fuchsia_wlan_internal::TxPowerScenario;
use log::warn;
use std::sync::Arc;

use wlan_legacy_metrics_registry as metrics;

fn fidl_scenario_to_cobalt_dimension(
    scenario: TxPowerScenario,
) -> Option<metrics::ConnectivityWlanMetricDimensionScenario> {
    let scenario = match scenario {
        TxPowerScenario::Default => metrics::ConnectivityWlanMetricDimensionScenario::Default,
        TxPowerScenario::VoiceCall => metrics::ConnectivityWlanMetricDimensionScenario::VoiceCall,
        TxPowerScenario::HeadCellOff => {
            metrics::ConnectivityWlanMetricDimensionScenario::HeadCellOff
        }
        TxPowerScenario::HeadCellOn => metrics::ConnectivityWlanMetricDimensionScenario::HeadCellOn,
        TxPowerScenario::BodyCellOff => {
            metrics::ConnectivityWlanMetricDimensionScenario::BodyCellOff
        }
        TxPowerScenario::BodyCellOn => metrics::ConnectivityWlanMetricDimensionScenario::BodyCellOn,
        TxPowerScenario::BodyBtActive => {
            metrics::ConnectivityWlanMetricDimensionScenario::BodyBtActive
        }
        other => {
            warn!("Unable to convert power scenario to metric definition: {:?}", other);
            return None;
        }
    };

    Some(scenario)
}

pub struct TxPowerScenarioLogger {
    cobalt_proxy: Arc<FilteredCobaltLogger>,
}

impl TxPowerScenarioLogger {
    pub fn new(cobalt_proxy: Arc<FilteredCobaltLogger>) -> Self {
        Self { cobalt_proxy }
    }

    pub async fn handle_sar_reset(&self) {
        let metric_events = vec![MetricEvent {
            metric_id: metrics::SAR_SCENARIO_RESET_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];
        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_sar_reset");
    }

    pub async fn handle_set_sar(&self, scenario: TxPowerScenario) {
        let mut metric_events = vec![MetricEvent {
            metric_id: metrics::SET_SAR_SCENARIO_OCCURRENCE_METRIC_ID,
            event_codes: vec![],
            payload: MetricEventPayload::Count(1),
        }];

        if let Some(scenario) = fidl_scenario_to_cobalt_dimension(scenario) {
            metric_events.push(MetricEvent {
                metric_id: metrics::SET_SAR_SCENARIO_BREAKDOWN_BY_SCENARIO_METRIC_ID,
                event_codes: vec![scenario.as_event_code()],
                payload: MetricEventPayload::Count(1),
            })
        };

        log_cobalt_batch!(self.cobalt_proxy, &metric_events, "handle_set_sar");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::setup_test;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;

    #[fuchsia::test]
    fn test_handle_reset_event() {
        let mut test_helper = setup_test();
        let logger = TxPowerScenarioLogger::new(test_helper.filtered_cobalt_logger());

        let mut test_fut = pin!(logger.handle_sar_reset());
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics = test_helper.get_logged_metrics(metrics::SAR_SCENARIO_RESET_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
    }

    #[fuchsia::test]
    fn test_handle_set_event() {
        let mut test_helper = setup_test();
        let logger = TxPowerScenarioLogger::new(test_helper.filtered_cobalt_logger());

        let mut test_fut = pin!(logger.handle_set_sar(TxPowerScenario::Default));
        assert_eq!(
            test_helper.run_until_stalled_drain_cobalt_events(&mut test_fut),
            Poll::Ready(())
        );

        let metrics =
            test_helper.get_logged_metrics(metrics::SET_SAR_SCENARIO_OCCURRENCE_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));

        let metrics = test_helper
            .get_logged_metrics(metrics::SET_SAR_SCENARIO_BREAKDOWN_BY_SCENARIO_METRIC_ID);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].payload, MetricEventPayload::Count(1));
        assert_eq!(
            metrics[0].event_codes,
            vec![metrics::ConnectivityWlanMetricDimensionScenario::Default.as_event_code()]
        );
    }

    #[test_case(
        TxPowerScenario::Default,
        Some(metrics::ConnectivityWlanMetricDimensionScenario::Default)
    )]
    #[test_case(
        TxPowerScenario::VoiceCall,
        Some(metrics::ConnectivityWlanMetricDimensionScenario::VoiceCall)
    )]
    #[test_case(
        TxPowerScenario::HeadCellOff,
        Some(metrics::ConnectivityWlanMetricDimensionScenario::HeadCellOff)
    )]
    #[test_case(
        TxPowerScenario::HeadCellOn,
        Some(metrics::ConnectivityWlanMetricDimensionScenario::HeadCellOn)
    )]
    #[test_case(
        TxPowerScenario::BodyCellOff,
        Some(metrics::ConnectivityWlanMetricDimensionScenario::BodyCellOff)
    )]
    #[test_case(
        TxPowerScenario::BodyCellOn,
        Some(metrics::ConnectivityWlanMetricDimensionScenario::BodyCellOn)
    )]
    #[test_case(
        TxPowerScenario::BodyBtActive,
        Some(metrics::ConnectivityWlanMetricDimensionScenario::BodyBtActive)
    )]
    #[test_case(TxPowerScenario::unknown(), None)]
    fn test_fidl_scenario_to_cobalt_dimension_conversion(
        scenario: TxPowerScenario,
        expected: Option<metrics::ConnectivityWlanMetricDimensionScenario>,
    ) {
        assert_eq!(fidl_scenario_to_cobalt_dimension(scenario), expected);
    }
}
