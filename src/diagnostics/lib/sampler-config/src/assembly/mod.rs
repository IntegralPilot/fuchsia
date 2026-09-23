// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::{EventCode, MetricId, MetricType, ProjectId};
use crate::utils::OneOrMany;
use anyhow::bail;
use component_id_index::InstanceId;
use moniker::ExtendedMoniker;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::marker::PhantomData;
use std::str::FromStr;

pub use crate::runtime::{DataSetConfig, MetricConfig, ProjectConfig};

/// Configuration for a single FIRE project template to map Inspect data to its Cobalt metrics
/// for all components in the ComponentIdInfo.
///
/// This type will get converted to an individual `ProjectConfig` and piped to Sampler.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct ProjectTemplate {
    /// Project ID that metrics are being sampled and forwarded on behalf of.
    pub project_id: ProjectId,

    /// The frequency with which metrics are sampled, in seconds.
    #[serde(deserialize_with = "crate::utils::greater_than_zero")]
    pub poll_rate_sec: i64,

    /// The collection of mappings from Inspect to Cobalt.
    pub metrics: Vec<MetricTemplate>,
}

impl ProjectTemplate {
    pub fn expand(self, components: &ComponentIdInfoList) -> Result<ProjectConfig, anyhow::Error> {
        let ProjectTemplate { project_id, poll_rate_sec, metrics } = self;
        let mut metric_configs = Vec::with_capacity(metrics.len() * components.len());
        for component in components.iter() {
            for metric in metrics.iter() {
                if let Some(metric_config) = metric.expand(component)? {
                    metric_configs.push(metric_config);
                }
            }
        }

        let data_sets = vec![DataSetConfig { poll_rate_sec, metrics: metric_configs }];

        Ok(ProjectConfig { project_id, data_sets })
    }
}

/// Configuration for a single FIRE metric template to map from an Inspect property
/// to a cobalt metric. Unlike MetricConfig, selectors aren't parsed.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct MetricTemplate {
    /// Selector identifying the metric to
    /// sample via the diagnostics platform.
    #[serde(rename = "selector", deserialize_with = "one_or_many_strings")]
    pub selectors: Vec<String>,

    /// Cobalt metric id to map the selector to.
    pub metric_id: MetricId,

    /// Data type to transform the metric to.
    pub metric_type: MetricType,

    /// Event codes defining the dimensions of the
    /// cobalt metric.
    /// Notes:
    /// - Order matters, and must match the order of the defined dimensions
    ///   in the cobalt metric file.
    /// - The FIRE component-ID will be inserted as the first element of event_codes.
    /// - The event_codes field may be omitted from the config file if component-ID is the only
    ///   event code.
    #[serde(default)]
    pub event_codes: Vec<EventCode>,

    /// Optional boolean specifying whether to upload the specified metric only once, the first time
    /// it becomes available to the sampler. Defaults to false.
    #[serde(default)]
    pub upload_once: bool,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct ComponentIdInfo {
    /// The component's moniker
    #[serde(deserialize_with = "moniker_deserialize", serialize_with = "moniker_serialize")]
    pub moniker: ExtendedMoniker,

    /// The Component Instance ID - may not be available
    #[serde(
        default,
        deserialize_with = "instance_id_deserialize",
        serialize_with = "instance_id_serialize"
    )]
    pub instance_id: Option<InstanceId>,

    /// The ID sent to Cobalt as an event code
    #[serde(alias = "id")]
    pub event_id: EventCode,

    /// Human-readable label, not used by Sampler, only for human configs.
    pub label: String,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq)]
pub struct ComponentIdInfoList(Vec<ComponentIdInfo>);

impl ComponentIdInfoList {
    pub fn new(infos: Vec<ComponentIdInfo>) -> Self {
        Self(infos)
    }

    pub fn add_instance_ids(&mut self, index: &component_id_index::Index) {
        for component in self.0.iter_mut() {
            if let ExtendedMoniker::ComponentInstance(moniker) = &component.moniker {
                component.instance_id = index.id_for_moniker(moniker).cloned();
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ComponentIdInfo> {
        self.0.iter()
    }
}

impl std::ops::Deref for ComponentIdInfoList {
    type Target = Vec<ComponentIdInfo>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ComponentIdInfoList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl IntoIterator for ComponentIdInfoList {
    type Item = ComponentIdInfo;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl MetricTemplate {
    fn expand(&self, component: &ComponentIdInfo) -> Result<Option<MetricConfig>, anyhow::Error> {
        let MetricTemplate { selectors, metric_id, metric_type, event_codes, upload_once } = self;
        let mut result_selectors = Vec::with_capacity(selectors.len());
        for selector in selectors {
            if let Some(selector_string) = interpolate_template(selector, component)? {
                let selector = crate::utils::parse_selector(&selector_string)?;
                result_selectors.push(selector);
            }
        }
        if result_selectors.is_empty() {
            return Ok(None);
        }
        let mut event_codes = event_codes.to_vec();
        event_codes.insert(0, component.event_id);
        Ok(Some(MetricConfig {
            event_codes,
            selectors: result_selectors,
            metric_id: *metric_id,
            metric_type: *metric_type,
            upload_once: *upload_once,
        }))
    }
}

const MONIKER_INTERPOLATION: &str = "{MONIKER}";
const INSTANCE_ID_INTERPOLATION: &str = "{INSTANCE_ID}";

/// Returns Ok(None) if the template needs a component ID and there's none available for
/// the component. This is not an error and should be handled silently.
fn interpolate_template(
    template: &str,
    component_info: &ComponentIdInfo,
) -> Result<Option<String>, anyhow::Error> {
    let moniker_position = template.find(MONIKER_INTERPOLATION);
    let instance_id_position = template.find(INSTANCE_ID_INTERPOLATION);
    let separator_position = template.find(":");
    // If the insert position is before the first colon, it's the selector's moniker and
    // slashes should not be escaped.
    // Otherwise, treat the moniker string as a single Node or Property name,
    // and escape the appropriate characters.
    // Instance IDs have no special characters and don't need escaping.
    match (moniker_position, separator_position, instance_id_position, &component_info.instance_id)
    {
        (Some(i), Some(s), _, _) if i < s => {
            Ok(Some(template.replace(MONIKER_INTERPOLATION, component_info.moniker.as_ref())))
        }
        (Some(_), Some(_), _, _) => Ok(Some(template.replace(
            MONIKER_INTERPOLATION,
            &selectors::sanitize_string_for_selectors(component_info.moniker.as_ref()),
        ))),
        (_, _, Some(_), Some(id)) => {
            Ok(Some(template.replace(INSTANCE_ID_INTERPOLATION, &id.to_string())))
        }
        (_, _, Some(_), None) => Ok(None),
        (None, _, None, _) => {
            bail!(
                "{} and {} not found in selector template {}",
                MONIKER_INTERPOLATION,
                INSTANCE_ID_INTERPOLATION,
                template
            )
        }
        (Some(_), None, _, _) => {
            bail!("Separator ':' not found in selector template {}", template)
        }
    }
}

fn moniker_deserialize<'de, D>(deserializer: D) -> Result<ExtendedMoniker, D::Error>
where
    D: Deserializer<'de>,
{
    let moniker_str = String::deserialize(deserializer)?;
    ExtendedMoniker::parse_str(&moniker_str).map_err(de::Error::custom)
}

fn instance_id_deserialize<'de, D>(deserializer: D) -> Result<Option<InstanceId>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        None => Ok(None),
        Some(instance_id) => {
            let instance_id = InstanceId::from_str(&instance_id).map_err(de::Error::custom)?;
            Ok(Some(instance_id))
        }
    }
}

pub fn moniker_serialize<S>(moniker: &ExtendedMoniker, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(moniker.as_ref())
}

pub fn instance_id_serialize<S>(
    instance_id: &Option<InstanceId>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match instance_id.as_ref() {
        Some(instance_id) => serializer.serialize_some(&instance_id.to_string()),
        None => serializer.serialize_none(),
    }
}

fn one_or_many_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(OneOrMany(PhantomData::<String>))
}

#[derive(Serialize, Deserialize, Default)]
pub struct MergedSamplerConfig {
    pub fire_project_templates: Vec<ProjectTemplate>,
    pub fire_component_configs: Vec<ComponentIdInfoList>,
    pub project_configs: Vec<ProjectConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_id_index::IndexEntry;
    use moniker::Moniker;
    use selectors::FastError;
    use std::str::FromStr;

    #[fuchsia::test]
    fn parse_project_template() {
        let template = r#"{
            "project_id": 13,
            "poll_rate_sec": 60,
            "metrics": [
                {
                "selector": [
                    "{MONIKER}:root/path2:leaf2",
                    "foo/bar:root/{MONIKER}:leaf3",
                    "asdf/qwer:root/path4:pre-{MONIKER}-post",
                ],
                "metric_id": 2,
                "metric_type": "Occurrence",
                }
            ]
        }"#;
        let config: ProjectTemplate = serde_json5::from_str(template).expect("deserialize");
        assert_eq!(
            config,
            ProjectTemplate {
                project_id: ProjectId(13),
                poll_rate_sec: 60,
                metrics: vec![MetricTemplate {
                    selectors: vec![
                        "{MONIKER}:root/path2:leaf2".into(),
                        "foo/bar:root/{MONIKER}:leaf3".into(),
                        "asdf/qwer:root/path4:pre-{MONIKER}-post".into(),
                    ],
                    event_codes: vec![],
                    upload_once: false,
                    metric_id: MetricId(2),
                    metric_type: MetricType::Occurrence,
                }],
            }
        );
    }

    #[fuchsia::test]
    fn parse_component_config() {
        let components_json = r#"[
            {
                "id": 42,
                "label": "Foo_42",
                "moniker": "core/foo42"
            },
            {
                "id": 10,
                "label": "Hello",
                "instance_id": "8775ff0afe12ca578135014a5d36a7733b0f9982bcb62a888b007cb2c31a7046",
                "moniker": "bootstrap/hello"
            }
        ]"#;
        let config: ComponentIdInfoList =
            serde_json5::from_str(components_json).expect("deserialize");
        assert_eq!(
            config,
            ComponentIdInfoList(vec![
                ComponentIdInfo {
                    moniker: ExtendedMoniker::parse_str("core/foo42").unwrap(),
                    instance_id: None,
                    event_id: EventCode(42),
                    label: "Foo_42".into()
                },
                ComponentIdInfo {
                    moniker: ExtendedMoniker::parse_str("bootstrap/hello").unwrap(),
                    instance_id: Some(
                        InstanceId::from_str(
                            "8775ff0afe12ca578135014a5d36a7733b0f9982bcb62a888b007cb2c31a7046"
                        )
                        .unwrap()
                    ),
                    event_id: EventCode(10),
                    label: "Hello".into()
                },
            ])
        );
    }

    #[fuchsia::test]
    fn template_expansion_basic() {
        let project_template = ProjectTemplate {
            project_id: ProjectId(13),
            poll_rate_sec: 60,
            metrics: vec![MetricTemplate {
                selectors: vec!["{MONIKER}:root/path:leaf".into()],
                metric_id: MetricId(1),
                metric_type: MetricType::Occurrence,
                event_codes: vec![EventCode(1), EventCode(2)],
                upload_once: true,
            }],
        };
        let component_info = ComponentIdInfoList(vec![
            ComponentIdInfo {
                moniker: ExtendedMoniker::parse_str("core/foo42").unwrap(),
                instance_id: None,
                event_id: EventCode(42),
                label: "Foo_42".into(),
            },
            ComponentIdInfo {
                moniker: ExtendedMoniker::parse_str("bootstrap/hello").unwrap(),
                instance_id: Some(
                    InstanceId::from_str(
                        "8775ff0afe12ca578135014a5d36a7733b0f9982bcb62a888b007cb2c31a7046",
                    )
                    .unwrap(),
                ),
                event_id: EventCode(43),
                label: "Hello".into(),
            },
        ]);
        let config = project_template.expand(&component_info).expect("expanded template");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(13),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![
                        MetricConfig {
                            selectors: vec![
                                selectors::parse_selector::<FastError>("core/foo42:root/path:leaf")
                                    .unwrap()
                            ],
                            metric_id: MetricId(1),
                            metric_type: MetricType::Occurrence,
                            event_codes: vec![EventCode(42), EventCode(1), EventCode(2)],
                            upload_once: true,
                        },
                        MetricConfig {
                            selectors: vec![
                                selectors::parse_selector::<FastError>(
                                    "bootstrap/hello:root/path:leaf"
                                )
                                .unwrap()
                            ],
                            metric_id: MetricId(1),
                            metric_type: MetricType::Occurrence,
                            event_codes: vec![EventCode(43), EventCode(1), EventCode(2)],
                            upload_once: true,
                        },
                    ],
                }],
            }
        );
    }

    #[fuchsia::test]
    fn template_expansion_many_selectors() {
        let project_template = ProjectTemplate {
            project_id: ProjectId(13),
            poll_rate_sec: 60,
            metrics: vec![MetricTemplate {
                selectors: vec![
                    "{MONIKER}:root/path2:leaf2".into(),
                    "foo/bar:root/{MONIKER}:leaf3".into(),
                    "asdf/qwer:root/path4:pre-{MONIKER}-post".into(),
                ],
                metric_id: MetricId(2),
                metric_type: MetricType::Occurrence,
                event_codes: vec![],
                upload_once: false,
            }],
        };
        let component_info = ComponentIdInfoList(vec![
            ComponentIdInfo {
                moniker: ExtendedMoniker::parse_str("core/foo42").unwrap(),
                instance_id: None,
                event_id: EventCode(42),
                label: "Foo_42".into(),
            },
            ComponentIdInfo {
                moniker: ExtendedMoniker::parse_str("bootstrap/hello").unwrap(),
                instance_id: Some(
                    InstanceId::from_str(
                        "8775ff0afe12ca578135014a5d36a7733b0f9982bcb62a888b007cb2c31a7046",
                    )
                    .unwrap(),
                ),
                event_id: EventCode(43),
                label: "Hello".into(),
            },
        ]);
        let config = project_template.expand(&component_info).expect("expanded template");
        assert_eq!(
            config,
            ProjectConfig {
                project_id: ProjectId(13),
                data_sets: vec![DataSetConfig {
                    poll_rate_sec: 60,
                    metrics: vec![
                        MetricConfig {
                            selectors: vec![
                                selectors::parse_selector::<FastError>(
                                    "core/foo42:root/path2:leaf2"
                                )
                                .unwrap(),
                                selectors::parse_selector::<FastError>(
                                    "foo/bar:root/core\\/foo42:leaf3"
                                )
                                .unwrap(),
                                selectors::parse_selector::<FastError>(
                                    "asdf/qwer:root/path4:pre-core\\/foo42-post"
                                )
                                .unwrap()
                            ],
                            metric_id: MetricId(2),
                            metric_type: MetricType::Occurrence,
                            event_codes: vec![EventCode(42)],
                            upload_once: false,
                        },
                        MetricConfig {
                            selectors: vec![
                                selectors::parse_selector::<FastError>(
                                    "bootstrap/hello:root/path2:leaf2"
                                )
                                .unwrap(),
                                selectors::parse_selector::<FastError>(
                                    "foo/bar:root/bootstrap\\/hello:leaf3"
                                )
                                .unwrap(),
                                selectors::parse_selector::<FastError>(
                                    "asdf/qwer:root/path4:pre-bootstrap\\/hello-post"
                                )
                                .unwrap()
                            ],
                            metric_id: MetricId(2),
                            metric_type: MetricType::Occurrence,
                            event_codes: vec![EventCode(43)],
                            upload_once: false,
                        },
                    ],
                }],
            }
        );
    }

    #[fuchsia::test]
    fn index_substitution_works() {
        let mut ids = component_id_index::Index::default();
        let foo_bar_moniker = Moniker::parse_str("foo/bar").unwrap();
        let qwer_asdf_moniker = Moniker::parse_str("qwer/asdf").unwrap();
        ids.insert(IndexEntry {
            moniker: foo_bar_moniker,
            instance_id: "1234123412341234123412341234123412341234123412341234123412341234"
                .parse::<InstanceId>()
                .unwrap(),
            ignore_duplicate_id: false,
        })
        .unwrap();
        ids.insert(IndexEntry {
            moniker: qwer_asdf_moniker,
            instance_id: "1234abcd1234abcd123412341234123412341234123412341234123412341234"
                .parse::<InstanceId>()
                .unwrap(),
            ignore_duplicate_id: false,
        })
        .unwrap();
        let mut components = ComponentIdInfoList(vec![
            ComponentIdInfo {
                moniker: "baz/quux".try_into().unwrap(),
                event_id: EventCode(101),
                label: "bq".into(),
                instance_id: None,
            },
            ComponentIdInfo {
                moniker: "foo/bar".try_into().unwrap(),
                event_id: EventCode(102),
                label: "fb".into(),
                instance_id: None,
            },
        ]);
        components.add_instance_ids(&ids);
        let moniker_template = "fizz/buzz:root/info/{MONIKER}/data";
        let id_template = "fizz/buzz:root/info/{INSTANCE_ID}/data";
        assert_eq!(
            interpolate_template(moniker_template, &components[0]).unwrap().unwrap(),
            "fizz/buzz:root/info/baz\\/quux/data".to_string()
        );
        assert_eq!(
            interpolate_template(moniker_template, &components[1]).unwrap().unwrap(),
            "fizz/buzz:root/info/foo\\/bar/data".to_string()
        );
        assert_eq!(interpolate_template(id_template, &components[0]).unwrap(), None);
        assert_eq!(
            interpolate_template(id_template, &components[1]).unwrap().unwrap(),
            "fizz/buzz:root/info/1234123412341234123412341234123412341234123412341234123412341234/data"
        );
    }

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct TestString(#[serde(deserialize_with = "super::one_or_many_strings")] Vec<String>);

    #[fuchsia::test]
    fn parse_valid_single_string() {
        let json = "\"whatever-1982035*()$*H\"";
        let data: TestString = serde_json5::from_str(json).expect("deserialize");
        assert_eq!(data, TestString(vec!["whatever-1982035*()$*H".into()]));
    }

    #[fuchsia::test]
    fn parse_valid_multiple_strings() {
        let json = "[ \"core/foo:not:a:selector:root/branch:leaf\", \"core/bar:root/twig:leaf\"]";
        let data: TestString = serde_json5::from_str(json).expect("deserialize");
        assert_eq!(
            data,
            TestString(vec![
                "core/foo:not:a:selector:root/branch:leaf".into(),
                "core/bar:root/twig:leaf".into()
            ])
        );
    }

    #[fuchsia::test]
    fn refuse_invalid_strings() {
        let not_string = "42";
        let bad_list = "[ 42, \"core/bar:not:a:selector:root/twig:leaf\"]";
        serde_json5::from_str::<TestString>(not_string).expect_err("should fail");
        serde_json5::from_str::<TestString>(bad_list).expect_err("should fail");
    }
}
