// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::{EventCode, MetricId, MetricType, ProjectId};
use fidl_fuchsia_diagnostics::Selector;
use selectors::SelectorDisplayOptions;
use serde::ser::{Error, SerializeSeq};
use serde::{Deserialize, Serialize, Serializer};

/// Configuration for a single project to map Inspect data to its Cobalt metrics.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct ProjectConfig {
    /// Project ID that metrics are being sampled and forwarded on behalf of.
    pub project_id: ProjectId,

    /// Groupings of metrics that share a poll rate for this project.
    pub data_sets: Vec<DataSetConfig>,
}

/// Grouping unit for metrics within a single project that share a polling
/// frequency.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct DataSetConfig {
    /// The frequency with which metrics are sampled, in seconds.
    #[serde(deserialize_with = "crate::utils::greater_than_zero")]
    pub poll_rate_sec: i64,

    /// The collection of mappings from Inspect to Cobalt.
    pub metrics: Vec<MetricConfig>,
}

/// Configuration for a single metric to map from an Inspect property to a Cobalt metric.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct MetricConfig {
    /// Selector identifying the metric to sample via the diagnostics platform.
    #[serde(
        rename = "selector",
        deserialize_with = "crate::utils::one_or_many_selectors",
        serialize_with = "selectors_to_string"
    )]
    pub selectors: Vec<Selector>,

    /// Cobalt metric id to map the selector to.
    pub metric_id: MetricId,

    /// Data type to transform the metric to.
    pub metric_type: MetricType,

    /// Event codes defining the dimensions of the Cobalt metric. Note: Order matters, and must
    /// match the order of the defined dimensions in the Cobalt metric file.
    /// Missing field means the same as empty list.
    #[serde(default)]
    pub event_codes: Vec<EventCode>,

    /// Optional boolean specifying whether to upload the specified metric only once, the first time
    /// it becomes available to the sampler. Defaults to false.
    #[serde(default)]
    pub upload_once: bool,
}

pub fn selectors_to_string<S>(selectors: &[Selector], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq = serializer.serialize_seq(Some(selectors.len()))?;
    for selector in selectors {
        let selector_string =
            selectors::selector_to_string(selector, SelectorDisplayOptions::never_wrap_in_quotes())
                .map_err(S::Error::custom)?;
        seq.serialize_element(&selector_string)?;
    }
    seq.end()
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    fn parse_valid_sampler_config() {
        let ok_json = r#"{
            "project_id": 5,
            "data_sets": [{
              "poll_rate_sec": 60,
              "metrics": [
                {
                  "selector": "bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests",
                  "metric_id": 1,
                  "metric_type": "Occurrence",
                  "event_codes": [0, 0]
                }
              ]
            }]
        }"#;

        let config: ProjectConfig = serde_json5::from_str(ok_json).expect("parse json");
        assert_eq!(config, ProjectConfig {
            project_id: ProjectId(5),
            data_sets: vec![DataSetConfig {
                poll_rate_sec: 60,
                metrics: vec![
                    MetricConfig {
                        selectors: vec![
                            selectors::parse_verbose("bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests").unwrap(),
                        ],
                        metric_id: MetricId(1),
                        metric_type: MetricType::Occurrence,
                        event_codes: vec![EventCode(0), EventCode(0)],
                        upload_once: false,
                    }
                ]
            }]
        });
    }

    #[fuchsia::test]
    fn parse_valid_sampler_config_json5() {
        let ok_json = r#"{
          project_id: 5,
          data_sets: [{
            poll_rate_sec: 3,
            metrics: [
              {
                // Test comment for json5 portability.
                selector: "single_counter_test_component:root:counter",
                metric_id: 1,
                metric_type: "Occurrence",
                event_codes: [0, 0]
              }
            ]
          }]
        }"#;

        let config: ProjectConfig = serde_json5::from_str(ok_json).expect("parse json");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 3,
                    metrics: vec![MetricConfig {
                        selectors: vec![
                            selectors::parse_verbose("single_counter_test_component:root:counter")
                                .unwrap(),
                        ],
                        metric_id: MetricId(1),
                        metric_type: MetricType::Occurrence,
                        upload_once: false,
                        event_codes: vec![EventCode(0), EventCode(0)],
                    }]
                }]
            }
        );
    }

    #[fuchsia::test]
    fn parse_valid_sampler_config_multiple_selectors() {
        let ok_json = r#"{
          project_id: 5,
          data_sets: [{
            poll_rate_sec: 3,
            metrics: [
              {
                selector: ["component:root:one", "component:root:two"],
                metric_id: 1,
                metric_type: "Occurrence",
                event_codes: [0, 0]
              }
            ]
          }]
        }"#;

        let config: ProjectConfig = serde_json5::from_str(ok_json).expect("parse json");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 3,
                    metrics: vec![MetricConfig {
                        selectors: vec![
                            selectors::parse_verbose("component:root:one").unwrap(),
                            selectors::parse_verbose("component:root:two").unwrap(),
                        ],
                        metric_id: MetricId(1),
                        metric_type: MetricType::Occurrence,
                        upload_once: false,
                        event_codes: vec![EventCode(0), EventCode(0)],
                    }]
                }]
            }
        );
    }

    #[fuchsia::test]
    fn parse_invalid_config() {
        let invalid_json = r#"{
          "project_id": 5,
          "poll_rate_sec": 3,
          "invalid_field": "bad bad bad"
        }"#;

        serde_json5::from_str::<ProjectConfig>(invalid_json)
            .expect_err("fail to load invalid config");
    }

    #[fuchsia::test]
    fn parse_optional_args() {
        let true_json = r#"{
           "project_id": 5,
           "data_sets": [{
             "poll_rate_sec": 60,
             "metrics": [
               {
                 // Test comment for json5 portability.
                 "selector": "bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests",
                 "metric_id": 1,
                 "metric_type": "Occurrence",
                 "event_codes": [0, 0],
                 "upload_once": true,
               }
             ]
           }]
         }"#;

        let config: ProjectConfig = serde_json5::from_str(true_json).expect("parse json");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        selectors: vec![
                            selectors::parse_verbose("bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests").unwrap(),
                        ],
                        metric_id: MetricId(1),
                        metric_type: MetricType::Occurrence,
                        upload_once: true,
                        event_codes: vec![EventCode(0), EventCode(0)],
                    }]
                }]
            }
        );

        let false_json = r#"{
          "project_id": 5,
          "data_sets": [{
            "poll_rate_sec": 60,
            "metrics": [
              {
                // Test comment for json5 portability.
                "selector": "bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests",
                "metric_id": 1,
                "metric_type": "Occurrence",
                "event_codes": [0, 0],
                "upload_once": false,
              }
            ]
          }]
        }"#;
        let config: ProjectConfig = serde_json5::from_str(false_json).expect("parse json");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        selectors: vec![
                            selectors::parse_verbose("bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests").unwrap(),
                        ],
                        metric_id: MetricId(1),
                        metric_type: MetricType::Occurrence,
                        upload_once: false,
                        event_codes: vec![EventCode(0), EventCode(0)],
                    }]
                }]
            }
        );
    }
    #[fuchsia::test]
    fn default_customer_id() {
        let default_json = r#"{
          "project_id": 5,
          "data_sets": [{
            "poll_rate_sec": 60,
            "metrics": [
              {
                "selector": "bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests",
                "metric_id": 1,
                "metric_type": "Occurrence",
                "event_codes": [0, 0]
              }
            ]
          }]
        }"#;

        let config: ProjectConfig = serde_json5::from_str(default_json).expect("deserialize");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        selectors: vec![
                            selectors::parse_verbose("bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests").unwrap(),
                        ],
                        metric_id: MetricId(1),
                        metric_type: MetricType::Occurrence,
                        upload_once: false,
                        event_codes: vec![EventCode(0), EventCode(0)],
                    }],
                }]
            }
        );
    }

    #[fuchsia::test]
    fn missing_event_codes_ok() {
        let default_json = r#"{
          "project_id": 5,
          "data_sets": [{
            "poll_rate_sec": 60,
            "metrics": [
              {
                "selector": "bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests",
                "metric_id": 1,
                "metric_type": "Occurrence",
              }
            ]
          }]
        }"#;

        let config: ProjectConfig = serde_json5::from_str(default_json).expect("deserialize");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        selectors: vec![
                            selectors::parse_verbose("bootstrap/archivist:root/all_archive_accessor:inspect_batch_iterator_get_next_requests").unwrap(),
                        ],
                        metric_id: MetricId(1),
                        metric_type: MetricType::Occurrence,
                        upload_once: false,
                        event_codes: vec![],
                    }]
                }]
            }
        );
    }
}
