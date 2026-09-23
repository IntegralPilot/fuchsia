// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fuchsia_inspect::{Node, NumericProperty, UintProperty};
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use sampler_component_config::Config as ComponentConfig;
use sampler_config::runtime::ProjectConfig;
use sampler_config::{MetricType, ProjectId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::iter::Sum;

/// Container for all configurations needed to instantiate the Sampler infrastructure.
/// Includes:
///      - Project configurations.
///      - Whether to configure the ArchiveReader for tests (e.g. longer timeouts)
///      - Minimum sample rate.
#[derive(Debug)]
pub struct SamplerConfig {
    pub project_configs: Vec<ProjectConfig>,
    pub stats: SamplerStats,
}

#[derive(Debug)]
pub struct ProjectStats {
    _project_node: Node,
    pub metrics_configured: UintProperty,
    pub cobalt_logs_sent: UintProperty,
    pub events: RefCell<BoundedListNode>,
}

#[derive(Default, Debug)]
pub struct SamplerStats {
    pub projects: HashMap<ProjectId, ProjectStats>,
}

fn flatten_configs(mut input: Vec<ProjectConfig>) -> Vec<ProjectConfig> {
    input.sort_unstable_by_key(|conf| *conf.project_id);
    input.dedup_by(|next, prev| {
        if next.project_id == prev.project_id {
            prev.data_sets.append(&mut next.data_sets);
            return true;
        }

        false
    });

    input
}

impl SamplerConfig {
    pub fn new(config: ComponentConfig, stats: &Node) -> Result<Self, Error> {
        let ComponentConfig { minimum_sample_rate_sec, project_configs } = config;
        let mut sampler_stats = SamplerStats::default();
        let project_configs = project_configs
            .into_iter()
            .map(|config| {
                let config: ProjectConfig = serde_json::from_str(&config)?;
                for ds in &config.data_sets {
                    if ds.poll_rate_sec < minimum_sample_rate_sec {
                        return Err(anyhow::anyhow!(
                            "Data set in project {} had illegal poll rate. Actual: {}s, Min: {}s",
                            config.project_id,
                            ds.poll_rate_sec,
                            minimum_sample_rate_sec
                        ));
                    }
                }

                Ok(config)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let project_configs = flatten_configs(project_configs);
        for config in &project_configs {
            sampler_stats
                .projects
                .entry(config.project_id)
                .and_modify(|project| {
                    project
                        .metrics_configured
                        .add(u64::sum(config.data_sets.iter().map(|ds| ds.metrics.len() as u64)));
                })
                .or_insert_with(|| {
                    let project_node = stats.create_child(format!("project_{}", config.project_id));
                    let metrics_configured = project_node.create_uint(
                        "metrics_configured",
                        u64::sum(config.data_sets.iter().map(|ds| ds.metrics.len() as u64)),
                    );
                    let cobalt_logs_sent = project_node.create_uint("cobalt_logs_sent", 0);
                    let events = RefCell::new(BoundedListNode::new(
                        project_node.create_child("events"),
                        300,
                    ));
                    ProjectStats {
                        _project_node: project_node,
                        metrics_configured,
                        cobalt_logs_sent,
                        events,
                    }
                });
        }

        Ok(Self { project_configs, stats: sampler_stats })
    }

    pub fn sample_data(&self) -> Vec<fdiagnostics::SampleDatum> {
        let mut data = vec![];
        for project in &self.project_configs {
            for data_set in &project.data_sets {
                for metric in &data_set.metrics {
                    let strategy = Some(match metric.metric_type {
                        MetricType::Integer | MetricType::String => {
                            fdiagnostics::SampleStrategy::Always
                        }
                        MetricType::IntHistogram | MetricType::Occurrence => {
                            fdiagnostics::SampleStrategy::OnDiff
                        }
                    });

                    for selector in &metric.selectors {
                        data.push(fdiagnostics::SampleDatum {
                            selector: Some(fdiagnostics::SelectorArgument::StructuredSelector(
                                selector.clone(),
                            )),
                            interval_secs: Some(data_set.poll_rate_sec),
                            strategy,
                            ..Default::default()
                        });
                    }
                }
            }
        }

        data
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use diagnostics_assertions::assert_json_diff;
    use fuchsia_inspect::*;
    use sampler_config::runtime::*;
    use sampler_config::*;
    use selectors::parse_verbose;

    const TEST_CONFIG_P5_1: &str =
        include_str!("../testing/realm-factory/configs/test_config.json");
    const TEST_CONFIG_P5_2: &str =
        include_str!("../testing/realm-factory/configs/reboot_required_config.json");
    const TEST_CONFIG_P6: &str =
        include_str!("../testing/realm-factory/configs/test_config_2.json");

    #[fuchsia::test]
    async fn single_config() {
        let inspector = Inspector::default();
        let sc = SamplerConfig::new(
            ComponentConfig {
                minimum_sample_rate_sec: 1,
                project_configs: vec![TEST_CONFIG_P5_1.to_string()],
            },
            inspector.root(),
        )
        .unwrap();

        let SamplerConfig { project_configs, stats: _ } = sc;

        assert_json_diff!(inspector, root: {
            project_5: {
                cobalt_logs_sent: 0,
                metrics_configured: 3,
                events: {},
            }
        });

        assert_eq!(project_configs.len(), 1);
        assert_eq!(
            project_configs[0],
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 3,
                    metrics: vec![
                        MetricConfig {
                            selectors: vec![
                                parse_verbose("single_counter:root/samples:counter").unwrap(),
                            ],
                            metric_id: MetricId(101),
                            metric_type: MetricType::Occurrence,
                            event_codes: vec![EventCode(0), EventCode(0)],
                            upload_once: false,
                        },
                        MetricConfig {
                            selectors: vec![
                                parse_verbose("single_counter:root/samples:integer_1").unwrap(),
                            ],
                            metric_id: MetricId(102),
                            metric_type: MetricType::Integer,
                            event_codes: vec![EventCode(0), EventCode(0)],
                            upload_once: false,
                        },
                        MetricConfig {
                            selectors: vec![
                                parse_verbose("single_counter:root/samples:integer_2").unwrap(),
                            ],
                            metric_id: MetricId(103),
                            metric_type: MetricType::Integer,
                            event_codes: vec![EventCode(0), EventCode(0)],
                            upload_once: true,
                        },
                    ],
                }]
            }
        );
    }

    #[fuchsia::test]
    fn error_on_invalid_sample_rate() {
        let inspector = Inspector::default();
        assert!(
            SamplerConfig::new(
                ComponentConfig {
                    minimum_sample_rate_sec: 1000,
                    project_configs: vec![TEST_CONFIG_P5_1.to_string()],
                },
                inspector.root(),
            )
            .is_err()
        );
    }

    #[fuchsia::test]
    async fn duped_project_id() {
        let inspector = Inspector::default();
        let sc = SamplerConfig::new(
            ComponentConfig {
                minimum_sample_rate_sec: 1,
                project_configs: vec![TEST_CONFIG_P5_1.to_string(), TEST_CONFIG_P5_2.to_string()],
            },
            inspector.root(),
        )
        .unwrap();

        let SamplerConfig { project_configs, stats: _ } = sc;

        assert_json_diff!(inspector, root: {
            project_5: {
                cobalt_logs_sent: 0,
                metrics_configured: 4,
                events: {},
            }
        });

        assert_eq!(project_configs.len(), 1);
        assert_eq!(
            project_configs[0],
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![
                    DataSetConfig {
                        poll_rate_sec: 3,
                        metrics: vec![
                            MetricConfig {
                                selectors: vec![
                                    parse_verbose("single_counter:root/samples:counter").unwrap(),
                                ],
                                metric_id: MetricId(101),
                                metric_type: MetricType::Occurrence,
                                event_codes: vec![EventCode(0), EventCode(0)],
                                upload_once: false,
                            },
                            MetricConfig {
                                selectors: vec![
                                    parse_verbose("single_counter:root/samples:integer_1").unwrap(),
                                ],
                                metric_id: MetricId(102),
                                metric_type: MetricType::Integer,
                                event_codes: vec![EventCode(0), EventCode(0)],
                                upload_once: false,
                            },
                            MetricConfig {
                                selectors: vec![
                                    parse_verbose("single_counter:root/samples:integer_2").unwrap(),
                                ],
                                metric_id: MetricId(103),
                                metric_type: MetricType::Integer,
                                event_codes: vec![EventCode(0), EventCode(0)],
                                upload_once: true,
                            },
                        ],
                    },
                    DataSetConfig {
                        poll_rate_sec: 3000,
                        metrics: vec![MetricConfig {
                            selectors: vec![
                                parse_verbose("single_counter:root/samples:counter").unwrap(),
                            ],
                            metric_id: MetricId(104),
                            metric_type: MetricType::Occurrence,
                            event_codes: vec![EventCode(0), EventCode(0)],
                            upload_once: false,
                        },],
                    },
                ]
            }
        );
    }

    #[fuchsia::test]
    async fn multi_project() {
        let inspector = Inspector::default();
        let sc = SamplerConfig::new(
            ComponentConfig {
                minimum_sample_rate_sec: 1,
                project_configs: vec![
                    TEST_CONFIG_P5_1.to_string(),
                    TEST_CONFIG_P5_2.to_string(),
                    TEST_CONFIG_P6.to_string(),
                ],
            },
            inspector.root(),
        )
        .unwrap();

        let SamplerConfig { project_configs, stats: _ } = sc;

        assert_json_diff!(inspector, root: {
            project_5: {
                cobalt_logs_sent: 0,
                metrics_configured: 4,
                events: {},
            },
            project_6: {
                cobalt_logs_sent: 0,
                metrics_configured: 1,
                events: {},
            }
        });

        assert_eq!(project_configs.len(), 2);
        assert_eq!(
            project_configs[0],
            ProjectConfig {
                project_id: ProjectId(5),
                data_sets: vec![
                    DataSetConfig {
                        poll_rate_sec: 3,
                        metrics: vec![
                            MetricConfig {
                                selectors: vec![
                                    parse_verbose("single_counter:root/samples:counter").unwrap(),
                                ],
                                metric_id: MetricId(101),
                                metric_type: MetricType::Occurrence,
                                event_codes: vec![EventCode(0), EventCode(0)],
                                upload_once: false,
                            },
                            MetricConfig {
                                selectors: vec![
                                    parse_verbose("single_counter:root/samples:integer_1").unwrap(),
                                ],
                                metric_id: MetricId(102),
                                metric_type: MetricType::Integer,
                                event_codes: vec![EventCode(0), EventCode(0)],
                                upload_once: false,
                            },
                            MetricConfig {
                                selectors: vec![
                                    parse_verbose("single_counter:root/samples:integer_2").unwrap(),
                                ],
                                metric_id: MetricId(103),
                                metric_type: MetricType::Integer,
                                event_codes: vec![EventCode(0), EventCode(0)],
                                upload_once: true,
                            },
                        ],
                    },
                    DataSetConfig {
                        poll_rate_sec: 3000,
                        metrics: vec![MetricConfig {
                            selectors: vec![
                                parse_verbose("single_counter:root/samples:counter").unwrap(),
                            ],
                            metric_id: MetricId(104),
                            metric_type: MetricType::Occurrence,
                            event_codes: vec![EventCode(0), EventCode(0)],
                            upload_once: false,
                        },],
                    },
                ]
            }
        );
        assert_eq!(
            project_configs[1],
            ProjectConfig {
                project_id: ProjectId(6),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 10,
                    metrics: vec![MetricConfig {
                        selectors: vec![parse_verbose("foo:bar:baz").unwrap(),],
                        metric_id: MetricId(101),
                        metric_type: MetricType::IntHistogram,
                        event_codes: vec![EventCode(0)],
                        upload_once: false,
                    },],
                },],
            },
        );
    }
}
