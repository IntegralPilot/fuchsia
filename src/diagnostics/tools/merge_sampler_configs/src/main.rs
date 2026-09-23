// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, bail, format_err};
use argh::{ArgsInfo, FromArgs};
use cobalt_registry_proto::cobalt::CobaltRegistry;
use cobalt_registry_proto::cobalt::metric_definition::MetricType as CobaltMetricType;
use fidl_fuchsia_diagnostics::Selector;
use prost::Message;
use sampler_config::MetricType as SamplerMetricType;
use sampler_config::assembly::{MergedSamplerConfig, ProjectTemplate};
use sampler_config::runtime::ProjectConfig as SamplerProjectConfig;
use selectors::SelectorDisplayOptions;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const FUCHSIA_CUSTOMER_ID: u32 = 1;

/// Diagnostics config command
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
pub struct MergeConfigsCommand {
    /// paths to sampler project configs.
    #[argh(option)]
    pub project_config: Vec<PathBuf>,

    /// paths to sampler project templates.
    #[argh(option)]
    pub fire_project_template: Vec<PathBuf>,

    /// paths to sampler component configs.
    #[argh(option)]
    pub fire_component_config: Vec<PathBuf>,

    /// path to cobalt registry binary proto to validate against.
    #[argh(option)]
    pub cobalt_registry: Option<PathBuf>,

    /// path to which the result will be written.
    #[argh(option)]
    pub output: PathBuf,
}

pub fn main() -> Result<(), Error> {
    let args: MergeConfigsCommand = argh::from_env();

    let mut project_configs = Vec::new();
    for project_config_path in args.project_config {
        let parsed = read_file(&project_config_path)?;
        project_configs.push((project_config_path, parsed));
    }
    let mut fire_project_templates = Vec::new();
    for project_template_path in args.fire_project_template {
        let parsed = read_file(&project_template_path)?;
        fire_project_templates.push((project_template_path, parsed));
    }
    let mut fire_component_configs = Vec::new();
    for component_config_path in args.fire_component_config {
        let parsed = read_file(&component_config_path)?;
        fire_component_configs.push(parsed);
    }

    if let Some(cobalt_registry_path) = args.cobalt_registry {
        let registry_bytes = std::fs::read(&cobalt_registry_path).with_context(|| {
            format!("Failed to read cobalt registry from {:?}", cobalt_registry_path)
        })?;
        validate(&registry_bytes, &project_configs, &fire_project_templates)?;
    }

    let config = MergedSamplerConfig {
        project_configs: project_configs.into_iter().map(|(_, c)| c).collect(),
        fire_project_templates: fire_project_templates.into_iter().map(|(_, t)| t).collect(),
        fire_component_configs,
    };

    write_file(args.output, config)?;

    Ok(())
}

fn validate(
    registry_bytes: &[u8],
    project_configs: &[(PathBuf, SamplerProjectConfig)],
    fire_project_templates: &[(PathBuf, ProjectTemplate)],
) -> Result<(), Error> {
    let registry = CobaltRegistry::decode(registry_bytes)
        .context("Failed to decode CobaltRegistry protobuf")?;

    let customer =
        registry.customers.iter().find(|c| c.customer_id == FUCHSIA_CUSTOMER_ID).ok_or_else(
            || {
                format_err!(
                    "Fuchsia customer ID ({}) not found in Cobalt registry",
                    FUCHSIA_CUSTOMER_ID
                )
            },
        )?;

    let mut cobalt_projects = HashMap::new();
    for project in &customer.projects {
        let metrics_by_id: HashMap<u32, _> = project.metrics.iter().map(|m| (m.id, m)).collect();
        cobalt_projects.insert(project.project_id, (&project.project_name, metrics_by_id));
    }

    let mut errors = Vec::new();

    // Validate standard projects
    for (path, project) in project_configs {
        let project_id = *project.project_id;
        let (project_name, cobalt_metrics) = match cobalt_projects.get(&project_id) {
            Some((name, metrics)) => (*name, metrics),
            None => {
                errors.push(format!(
                    "In {}: Sampler project_id {} not found in Cobalt registry",
                    path.display(),
                    project_id
                ));
                continue;
            }
        };

        for dataset in &project.data_sets {
            for metric in &dataset.metrics {
                let metric_id = *metric.metric_id;
                let selector_context = format_selectors_context(&metric.selectors);
                let cobalt_metric = match cobalt_metrics.get(&metric_id) {
                    Some(m) => m,
                    None => {
                        errors.push(format!(
                            "In {}: Metric ID {} not found in Cobalt project {} ({}){}",
                            path.display(),
                            metric_id,
                            project_id,
                            project_name,
                            selector_context
                        ));
                        continue;
                    }
                };

                if let Err(e) = verify_metric_type(metric.metric_type, cobalt_metric.metric_type) {
                    errors.push(format!(
                        "In {}: Metric type mismatch for metric {} ({}) in project {} ({}): {}{}",
                        path.display(),
                        metric_id,
                        cobalt_metric.metric_name,
                        project_id,
                        project_name,
                        e,
                        selector_context
                    ));
                }

                let expected_dim_names: Vec<&str> =
                    cobalt_metric.metric_dimensions.iter().map(|d| d.dimension.as_str()).collect();
                let actual_dims = metric.event_codes.len();
                if actual_dims > expected_dim_names.len() {
                    let actual_codes: Vec<u32> = metric.event_codes.iter().map(|c| c.0).collect();
                    errors.push(format!(
                        "In {}: Dimension count mismatch for metric {} ({}) in project {} ({}): \
                         Sampler config has {} event_codes ({:?}), but Cobalt defines {} dimension(s): {:?}{}",
                        path.display(),
                        metric_id,
                        cobalt_metric.metric_name,
                        project_id,
                        project_name,
                        actual_dims,
                        actual_codes,
                        expected_dim_names.len(),
                        expected_dim_names,
                        selector_context
                    ));
                }
            }
        }
    }

    // Validate FIRE project templates
    for (path, template) in fire_project_templates {
        let project_id = *template.project_id;
        let (project_name, cobalt_metrics) = match cobalt_projects.get(&project_id) {
            Some((name, metrics)) => (*name, metrics),
            None => {
                errors.push(format!(
                    "In {}: FIRE template project_id {} not found in Cobalt registry",
                    path.display(),
                    project_id
                ));
                continue;
            }
        };

        for metric in &template.metrics {
            let metric_id = *metric.metric_id;
            let selector_context = format_template_selectors_context(&metric.selectors);
            let cobalt_metric = match cobalt_metrics.get(&metric_id) {
                Some(m) => m,
                None => {
                    errors.push(format!(
                        "In {}: FIRE Metric ID {} not found in Cobalt project {} ({}){}",
                        path.display(),
                        metric_id,
                        project_id,
                        project_name,
                        selector_context
                    ));
                    continue;
                }
            };

            if let Err(e) = verify_metric_type(metric.metric_type, cobalt_metric.metric_type) {
                errors.push(format!(
                    "In {}: Metric type mismatch for FIRE metric {} ({}) in project {} ({}): {}{}",
                    path.display(),
                    metric_id,
                    cobalt_metric.metric_name,
                    project_id,
                    project_name,
                    e,
                    selector_context
                ));
            }

            // In FIRE templates, component ID is injected as dimension 0, so event_codes.len() + 1
            let expected_dim_names: Vec<&str> =
                cobalt_metric.metric_dimensions.iter().map(|d| d.dimension.as_str()).collect();
            let actual_dims = metric.event_codes.len() + 1;
            if actual_dims > expected_dim_names.len() {
                let actual_codes: Vec<u32> = metric.event_codes.iter().map(|c| c.0).collect();
                errors.push(format!(
                    "In {}: Dimension count mismatch for FIRE metric {} ({}) in project {} ({}): \
                     Sampler has {} event_codes ({:?}) + 1 for component = {actual_dims}, \
                     but Cobalt defines {} dimension(s): {:?}{}",
                    path.display(),
                    metric_id,
                    cobalt_metric.metric_name,
                    project_id,
                    project_name,
                    metric.event_codes.len(),
                    actual_codes,
                    expected_dim_names.len(),
                    expected_dim_names,
                    selector_context
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        let count = errors.len();
        let formatted_errors = errors
            .iter()
            .enumerate()
            .map(|(i, err)| format!("{}. {}", i + 1, err))
            .collect::<Vec<_>>()
            .join("\n\n");
        bail!("{count} validation error(s) found in Sampler configs:\n\n{formatted_errors}")
    }
}

fn verify_metric_type(sampler_type: SamplerMetricType, cobalt_type_raw: i32) -> Result<(), Error> {
    let cobalt_type =
        CobaltMetricType::try_from(cobalt_type_raw).unwrap_or(CobaltMetricType::Unset);
    let expected = match sampler_type {
        SamplerMetricType::Occurrence => CobaltMetricType::Occurrence,
        SamplerMetricType::Integer => CobaltMetricType::Integer,
        SamplerMetricType::IntHistogram => CobaltMetricType::IntegerHistogram,
        SamplerMetricType::String => CobaltMetricType::String,
    };
    if cobalt_type != expected {
        bail!("Sampler specified {:?}, Cobalt defines {:?}", sampler_type, cobalt_type);
    }
    Ok(())
}

fn format_selectors_context(selectors: &[Selector]) -> String {
    let strs: Vec<_> = selectors
        .iter()
        .filter_map(|s| {
            selectors::selector_to_string(s, SelectorDisplayOptions::never_wrap_in_quotes()).ok()
        })
        .collect();
    if strs.is_empty() { String::new() } else { format!("\n  Selector: {}", strs.join(", ")) }
}

fn format_template_selectors_context(selectors: &[String]) -> String {
    if selectors.is_empty() {
        String::new()
    } else {
        format!("\n  Selector: {}", selectors.join(", "))
    }
}

fn read_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> anyhow::Result<T> {
    let file =
        File::open(path.as_ref()).with_context(|| format!("Failed to open {:?}", path.as_ref()))?;
    let mut reader = BufReader::new(file);
    let result: T = serde_json5::from_reader(&mut reader)
        .with_context(|| format!("Failed to parse JSON5 from {:?}", path.as_ref()))?;
    Ok(result)
}

fn write_file<T: Serialize>(path: PathBuf, value: T) -> anyhow::Result<()> {
    let file =
        File::create(&path).with_context(|| format!("Failed to create output file {:?}", path))?;
    let mut writer = BufWriter::new(file);
    serde_json5::to_writer(&mut writer, &value)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cobalt_registry_proto::cobalt::metric_definition::MetricDimension;
    use cobalt_registry_proto::cobalt::{
        CustomerConfig, MetricDefinition, ProjectConfig as CobaltProjectConfig,
    };
    use sampler_config::assembly::MetricTemplate;
    use sampler_config::runtime::{
        DataSetConfig, MetricConfig, ProjectConfig as SamplerProjectConfig,
    };
    use sampler_config::{EventCode, MetricId, ProjectId};

    fn make_test_registry() -> CobaltRegistry {
        CobaltRegistry {
            customers: vec![CustomerConfig {
                customer_name: "fuchsia".into(),
                customer_id: 1,
                projects: vec![CobaltProjectConfig {
                    project_name: "test_project".into(),
                    project_id: 10,
                    metrics: vec![
                        MetricDefinition {
                            id: 100,
                            metric_name: "test_occurrence".into(),
                            metric_type: CobaltMetricType::Occurrence as i32,
                            metric_dimensions: vec![MetricDimension {
                                dimension: "dim1".into(),
                                ..Default::default()
                            }],
                            ..Default::default()
                        },
                        MetricDefinition {
                            id: 101,
                            metric_name: "test_fire_histogram".into(),
                            metric_type: CobaltMetricType::IntegerHistogram as i32,
                            metric_dimensions: vec![
                                MetricDimension {
                                    dimension: "component".into(),
                                    ..Default::default()
                                },
                                MetricDimension {
                                    dimension: "reason".into(),
                                    ..Default::default()
                                },
                            ],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_valid_config() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        let project_configs = vec![(
            PathBuf::from("test/project.json5"),
            SamplerProjectConfig {
                project_id: ProjectId(10),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        metric_id: MetricId(100),
                        metric_type: SamplerMetricType::Occurrence,
                        event_codes: vec![EventCode(1)],
                        selectors: vec![],
                        upload_once: false,
                    }],
                }],
            },
        )];
        let fire_templates = vec![(
            PathBuf::from("test/fire.json5"),
            ProjectTemplate {
                project_id: ProjectId(10),
                poll_rate_sec: 60,
                metrics: vec![MetricTemplate {
                    metric_id: MetricId(101),
                    metric_type: SamplerMetricType::IntHistogram,
                    event_codes: vec![EventCode(2)],
                    selectors: vec![],
                    upload_once: false,
                }],
            },
        )];

        assert!(validate(&bytes, &project_configs, &fire_templates).is_ok());
    }

    #[test]
    fn test_unknown_project() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        let project_configs = vec![(
            PathBuf::from("test/bad_project.json5"),
            SamplerProjectConfig { project_id: ProjectId(999), data_sets: vec![] },
        )];

        let err = validate(&bytes, &project_configs, &[]).unwrap_err();
        assert!(
            err.to_string().contains("In test/bad_project.json5: Sampler project_id 999 not found")
        );
    }

    #[test]
    fn test_unknown_metric() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        let selector = selectors::parse_verbose("core/foo:root:bar").unwrap();
        let project_configs = vec![(
            PathBuf::from("test/bad_metric.json5"),
            SamplerProjectConfig {
                project_id: ProjectId(10),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        metric_id: MetricId(999),
                        metric_type: SamplerMetricType::Occurrence,
                        event_codes: vec![EventCode(1)],
                        selectors: vec![selector],
                        upload_once: false,
                    }],
                }],
            },
        )];

        let err = validate(&bytes, &project_configs, &[]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(
            "In test/bad_metric.json5: Metric ID 999 not found in Cobalt project 10 (test_project)"
        ));
        assert!(msg.contains("Selector: core/foo:root:bar"));
    }

    #[test]
    fn test_metric_type_mismatch() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        let project_configs = vec![(
            PathBuf::from("test/bad_type.json5"),
            SamplerProjectConfig {
                project_id: ProjectId(10),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        metric_id: MetricId(100),
                        metric_type: SamplerMetricType::Integer,
                        event_codes: vec![EventCode(1)],
                        selectors: vec![],
                        upload_once: false,
                    }],
                }],
            },
        )];

        let err = validate(&bytes, &project_configs, &[]).unwrap_err();
        assert!(err.to_string().contains("In test/bad_type.json5: Metric type mismatch for metric 100 (test_occurrence) in project 10 (test_project): Sampler specified Integer, Cobalt defines Occurrence"));
    }

    #[test]
    fn test_dimension_mismatch() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        let project_configs = vec![(
            PathBuf::from("test/bad_dims.json5"),
            SamplerProjectConfig {
                project_id: ProjectId(10),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        metric_id: MetricId(100),
                        metric_type: SamplerMetricType::Occurrence,
                        event_codes: vec![EventCode(1), EventCode(2)],
                        selectors: vec![],
                        upload_once: false,
                    }],
                }],
            },
        )];

        let err = validate(&bytes, &project_configs, &[]).unwrap_err();
        assert!(err.to_string().contains("In test/bad_dims.json5: Dimension count mismatch for metric 100 (test_occurrence) in project 10 (test_project): Sampler config has 2 event_codes ([1, 2]), but Cobalt defines 1 dimension(s): [\"dim1\"]"));
    }

    #[test]
    fn test_fire_dimension_mismatch() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        let fire_templates = vec![(
            PathBuf::from("test/bad_fire.json5"),
            ProjectTemplate {
                project_id: ProjectId(10),
                poll_rate_sec: 60,
                metrics: vec![MetricTemplate {
                    metric_id: MetricId(101),
                    metric_type: SamplerMetricType::IntHistogram,
                    event_codes: vec![EventCode(1), EventCode(2)], // 2 + 1 component = 3 > 2 in Cobalt
                    selectors: vec!["core/fire:root:val".to_string()],
                    upload_once: false,
                }],
            },
        )];

        let err = validate(&bytes, &[], &fire_templates).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("In test/bad_fire.json5: Dimension count mismatch for FIRE metric 101 (test_fire_histogram) in project 10 (test_project): Sampler has 2 event_codes ([1, 2]) + 1 for component = 3, but Cobalt defines 2 dimension(s): [\"component\", \"reason\"]"));
        assert!(msg.contains("Selector: core/fire:root:val"));
    }

    #[test]
    fn test_fewer_dimensions_allowed() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        // Project metric 100 has 1 dimension in Cobalt, but Sampler specifies 0 event codes.
        // FIRE template metric 101 has 2 dimensions in Cobalt, but Sampler specifies 0 event codes
        // (+1 for component = 1 dimension).
        // Both should be allowed since actual_dims <= expected_dims.
        let project_configs = vec![(
            PathBuf::from("test/fewer_dims.json5"),
            SamplerProjectConfig {
                project_id: ProjectId(10),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![MetricConfig {
                        metric_id: MetricId(100),
                        metric_type: SamplerMetricType::Occurrence,
                        event_codes: vec![],
                        selectors: vec![],
                        upload_once: false,
                    }],
                }],
            },
        )];
        let fire_templates = vec![(
            PathBuf::from("test/fewer_fire_dims.json5"),
            ProjectTemplate {
                project_id: ProjectId(10),
                poll_rate_sec: 60,
                metrics: vec![MetricTemplate {
                    metric_id: MetricId(101),
                    metric_type: SamplerMetricType::IntHistogram,
                    event_codes: vec![], // 0 + 1 component = 1 <= 2 dimensions in Cobalt
                    selectors: vec![],
                    upload_once: false,
                }],
            },
        )];

        assert!(validate(&bytes, &project_configs, &fire_templates).is_ok());
    }

    #[test]
    fn test_multiple_errors_accumulated() {
        let registry = make_test_registry();
        let bytes = registry.encode_to_vec();

        let project_configs = vec![
            (
                PathBuf::from("test/bad_project1.json5"),
                SamplerProjectConfig { project_id: ProjectId(999), data_sets: vec![] },
            ),
            (
                PathBuf::from("test/bad_project2.json5"),
                SamplerProjectConfig {
                    project_id: ProjectId(10),
                    data_sets: vec![DataSetConfig {
                        poll_rate_sec: 60,
                        metrics: vec![
                            MetricConfig {
                                metric_id: MetricId(100),
                                metric_type: SamplerMetricType::Integer, // mismatch
                                event_codes: vec![EventCode(1)],
                                selectors: vec![],
                                upload_once: false,
                            },
                            MetricConfig {
                                metric_id: MetricId(999), // unknown metric
                                metric_type: SamplerMetricType::Occurrence,
                                event_codes: vec![],
                                selectors: vec![],
                                upload_once: false,
                            },
                        ],
                    }],
                },
            ),
        ];

        let err = validate(&bytes, &project_configs, &[]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("3 validation error(s) found in Sampler configs:"));
        assert!(msg.contains("1. In test/bad_project1.json5: Sampler project_id 999 not found"));
        assert!(msg.contains("2. In test/bad_project2.json5: Metric type mismatch"));
        assert!(msg.contains("3. In test/bad_project2.json5: Metric ID 999 not found"));
    }
}
