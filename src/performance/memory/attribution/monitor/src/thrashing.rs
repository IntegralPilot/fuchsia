// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_metrics as fmetrics;
use fuchsia_inspect::Node;
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use memory_metrics_registry::cobalt_registry;
use serde::Deserialize;
use stalls::refaults::RefaultProvider;
use zx::MonotonicInstant;

pub const DEFAULT_POLLING_INTERVAL_SECONDS: u64 = 60;
pub const DEFAULT_PAGE_REFAULT_THRESHOLD: u64 = 5000;

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct ThrashingConfig {
    pub polling_interval_seconds: u64,
    pub page_refault_threshold: u64,
}

impl Default for ThrashingConfig {
    fn default() -> Self {
        Self {
            polling_interval_seconds: DEFAULT_POLLING_INTERVAL_SECONDS,
            page_refault_threshold: DEFAULT_PAGE_REFAULT_THRESHOLD,
        }
    }
}

/// Reads the thrashing configuration.
/// Tries `/config/data/thrashing.json` first (mutable config),
/// then `/pkg/data/thrashing.json` (packaged default).
/// Returns the default configuration if both are missing or malformed.
pub fn read_thrashing_config() -> ThrashingConfig {
    if let Ok(file) = std::fs::File::open("/config/data/thrashing.json") {
        return parse_config(file);
    }
    if let Ok(file) = std::fs::File::open("/pkg/data/thrashing.json") {
        return parse_config(file);
    }
    ThrashingConfig::default()
}

fn parse_config<R: std::io::Read>(reader: R) -> ThrashingConfig {
    serde_json::from_reader(reader).unwrap_or_default()
}

pub struct ThrashingDetector<P: RefaultProvider> {
    config: ThrashingConfig,
    _events_node_parent: Node,
    events_node: BoundedListNode,
    refault_provider: P,
    metric_logger: fmetrics::MetricEventLoggerProxy,
    last_refaults: Option<u64>,
}

impl<P: RefaultProvider> ThrashingDetector<P> {
    pub fn new(
        config: ThrashingConfig,
        node: Node,
        refault_provider: P,
        metric_logger: fmetrics::MetricEventLoggerProxy,
    ) -> Self {
        // Create a bounded list node to store the last 50 thrashing events.
        let events_node = BoundedListNode::new(node.create_child("events"), 50);

        // Record the configuration in inspect
        node.record_uint("polling_interval_seconds", config.polling_interval_seconds);
        node.record_uint("page_refault_threshold", config.page_refault_threshold);

        Self {
            config,
            _events_node_parent: node,
            events_node,
            refault_provider,
            metric_logger,
            last_refaults: None,
        }
    }

    pub async fn run_one_iteration(&mut self) -> Result<()> {
        let current_refaults = self.refault_provider.get_count();

        if let Some(last) = self.last_refaults {
            // Calculate deltas
            let refaults_delta = current_refaults.saturating_sub(last);

            let is_thrashing = refaults_delta > self.config.page_refault_threshold;

            if is_thrashing {
                let timestamp = MonotonicInstant::get().into_nanos();
                self.events_node.add_entry(move |node| {
                    node.record_int("timestamp_ns", timestamp);
                    node.record_uint("refaults_delta", refaults_delta);
                });

                self.metric_logger
                    .log_occurrence(cobalt_registry::MEMORY_THRASHING_EVENTS_METRIC_ID, 1, &[])
                    .await?
                    .map_err(|e| anyhow::anyhow!("Cobalt error: {:?}", e))?;
            }
        }

        self.last_refaults = Some(current_refaults);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Clone)]
    struct MockRefaultProvider {
        count: Arc<AtomicU64>,
    }

    impl RefaultProvider for MockRefaultProvider {
        fn get_count(&self) -> u64 {
            self.count.load(Ordering::Relaxed)
        }
    }

    #[fuchsia::test]
    fn test_config_defaults() {
        let config = ThrashingConfig::default();
        assert_eq!(config.polling_interval_seconds, 60);
        assert_eq!(config.page_refault_threshold, 5000);
    }

    #[fuchsia::test]
    async fn test_thrashing_detection() {
        use futures::StreamExt;

        let inspector = fuchsia_inspect::Inspector::default();
        let config = ThrashingConfig { polling_interval_seconds: 60, page_refault_threshold: 100 };

        let refaults = Arc::new(AtomicU64::new(0));
        let provider = MockRefaultProvider { count: refaults.clone() };

        let (metric_event_logger, mut metric_event_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fmetrics::MetricEventLoggerMarker>();

        let mut detector = ThrashingDetector::new(
            config,
            inspector.root().create_child("thrashing"),
            provider,
            metric_event_logger,
        );

        // First iteration - baseline 0
        detector.run_one_iteration().await.unwrap();

        // Second iteration - delta 200 > 100 threshold
        refaults.store(200, Ordering::Relaxed);

        let iteration_fut = detector.run_one_iteration();

        let mock_fut = async move {
            if let Some(Ok(fmetrics::MetricEventLoggerRequest::LogOccurrence {
                metric_id,
                count,
                event_codes,
                responder,
            })) = metric_event_request_stream.next().await
            {
                assert_eq!(metric_id, cobalt_registry::MEMORY_THRASHING_EVENTS_METRIC_ID);
                assert_eq!(count, 1);
                assert!(event_codes.is_empty());
                responder.send(Ok(())).unwrap();
            } else {
                panic!("Missing or invalid metric event logger request");
            }
        };

        let (iteration_res, _) = futures::join!(iteration_fut, mock_fut);
        iteration_res.unwrap();

        assert_data_tree!(inspector, root: {
            thrashing: {
                polling_interval_seconds: 60u64,
                page_refault_threshold: 100u64,
                events: {
                    "0": {
                        timestamp_ns: AnyProperty,
                        refaults_delta: 200u64,
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_thrashing_detection_with_metrics() {
        use futures::StreamExt;

        let inspector = fuchsia_inspect::Inspector::default();
        let config = ThrashingConfig { polling_interval_seconds: 60, page_refault_threshold: 100 };

        let refaults = Arc::new(AtomicU64::new(0));
        let provider = MockRefaultProvider { count: refaults.clone() };

        let (metric_event_logger, mut metric_event_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fmetrics::MetricEventLoggerMarker>();

        let mut detector = ThrashingDetector::new(
            config,
            inspector.root().create_child("thrashing"),
            provider,
            metric_event_logger,
        );

        // First iteration - baseline 0
        detector.run_one_iteration().await.unwrap();

        // Second iteration - delta 200 > 100 threshold
        refaults.store(200, Ordering::Relaxed);

        let iteration_fut = detector.run_one_iteration();

        let mock_fut = async move {
            if let Some(Ok(fmetrics::MetricEventLoggerRequest::LogOccurrence {
                metric_id,
                count,
                event_codes,
                responder,
            })) = metric_event_request_stream.next().await
            {
                assert_eq!(metric_id, cobalt_registry::MEMORY_THRASHING_EVENTS_METRIC_ID);
                assert_eq!(count, 1);
                assert!(event_codes.is_empty());
                responder.send(Ok(())).unwrap();
            } else {
                panic!("Missing or invalid metric event logger request");
            }
        };

        let (iteration_res, _) = futures::join!(iteration_fut, mock_fut);
        iteration_res.unwrap();
    }

    #[fuchsia::test]
    fn test_parse_config_valid() {
        let json = r#"{
            "polling_interval_seconds": 30,
            "page_refault_threshold": 1000
        }"#;
        let config = parse_config(json.as_bytes());
        assert_eq!(config.polling_interval_seconds, 30);
        assert_eq!(config.page_refault_threshold, 1000);
    }

    #[fuchsia::test]
    fn test_parse_config_malformed_json() {
        let json = r#"{
            "polling_interval_seconds": 30,
            "page_refault_threshold":
        }"#;
        let config = parse_config(json.as_bytes());
        assert_eq!(config, ThrashingConfig::default());
    }

    #[fuchsia::test]
    fn test_parse_config_unknown_fields() {
        // Unknown fields should be ignored if serde is lenient, or cause error if strict.
        // By default serde_json ignores unknown fields.
        let json = r#"{
            "polling_interval_seconds": 45,
            "page_refault_threshold": 2000,
            "unknown_field": "value"
        }"#;
        let config = parse_config(json.as_bytes());
        assert_eq!(config.polling_interval_seconds, 45);
        assert_eq!(config.page_refault_threshold, 2000);
    }

    #[fuchsia::test]
    fn test_parse_config_missing_fields() {
        // Missing fields result in error for derived Deserialize without default attributes.
        // Since we want robust defaults, we might expect this to typically fail to parse
        // and return default() if we rely on `unwrap_or_default`.
        let json = r#"{
            "polling_interval_seconds": 45
        }"#;
        let config = parse_config(json.as_bytes());
        // Since our struct doesn't have #[serde(default)] on fields,
        // missing fields cause a parse error, so we fall back to global default.
        assert_eq!(config, ThrashingConfig::default());
    }

    #[fuchsia::test]
    fn test_parse_config_empty() {
        let json = r#""#;
        let config = parse_config(json.as_bytes());
        assert_eq!(config, ThrashingConfig::default());
    }
}
