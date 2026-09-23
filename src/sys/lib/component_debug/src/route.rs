// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use flex_fuchsia_sys2 as fsys;
use moniker::{ExtendedMoniker, Moniker};
use prettytable::format::consts::FORMAT_CLEAN;
use prettytable::{Table, row};
use std::fmt;

const SUCCESS_SUMMARY: &'static str = "Success";
const VOID_SUMMARY: &'static str = "Routed from void";

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// Analytical information about a capability.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug)]
pub struct RouteReport {
    pub decl_type: DeclType,
    pub capability: String,
    pub error_summary: Option<String>,
    pub source_moniker: Option<String>,
    pub service_instances: Option<Vec<ServiceInstance>>,
    pub dictionary_entries: Option<Vec<DictionaryEntry>>,
    pub outcome: RouteOutcome,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq)]
pub struct ServiceInstance {
    pub instance_name: String,
    pub child_name: String,
    pub child_instance_name: String,
}

impl TryFrom<fsys::ServiceInstance> for ServiceInstance {
    type Error = anyhow::Error;

    fn try_from(value: fsys::ServiceInstance) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            instance_name: value
                .instance_name
                .ok_or_else(|| format_err!("missing instance_name"))?,
            child_name: value.child_name.ok_or_else(|| format_err!("missing child_name"))?,
            child_instance_name: value
                .child_instance_name
                .ok_or_else(|| format_err!("missing child_instance_name"))?,
        })
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq)]
pub struct DictionaryEntry {
    pub name: String,
}

impl TryFrom<fsys::DictionaryEntry> for DictionaryEntry {
    type Error = anyhow::Error;

    fn try_from(value: fsys::DictionaryEntry) -> std::result::Result<Self, Self::Error> {
        Ok(Self { name: value.name.ok_or_else(|| format_err!("missing name"))? })
    }
}

impl TryFrom<fsys::RouteReport> for RouteReport {
    type Error = anyhow::Error;

    fn try_from(report: fsys::RouteReport) -> Result<Self> {
        let decl_type =
            report.decl_type.ok_or_else(|| format_err!("missing decl type"))?.try_into()?;
        let capability = report.capability.ok_or_else(|| format_err!("missing capability name"))?;
        let error_summary = if let Some(error) = report.error { error.summary } else { None };
        let source_moniker = report.source_moniker;
        let service_instances = report
            .service_instances
            .map(|s| s.into_iter().map(|s| s.try_into()).collect())
            .transpose()?;
        let dictionary_entries = report
            .dictionary_entries
            .map(|s| s.into_iter().map(|s| s.try_into()).collect())
            .transpose()?;
        let outcome = match report.outcome {
            Some(o) => o.try_into()?,
            None => {
                // Backward compatibility. `outcome` may be missing if the client (e.g., ffx)
                // is built at a later version than the target.
                if error_summary.is_some() { RouteOutcome::Failed } else { RouteOutcome::Success }
            }
        };
        Ok(RouteReport {
            decl_type,
            capability,
            error_summary,
            source_moniker,
            service_instances,
            dictionary_entries,
            outcome,
        })
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq)]
pub enum DeclType {
    Use,
    Expose,
}

impl fmt::Display for DeclType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            DeclType::Use => "use",
            DeclType::Expose => "expose",
        };
        write!(f, "{}", s)
    }
}

impl TryFrom<fsys::DeclType> for DeclType {
    type Error = anyhow::Error;

    fn try_from(value: fsys::DeclType) -> std::result::Result<Self, Self::Error> {
        match value {
            fsys::DeclType::Use => Ok(DeclType::Use),
            fsys::DeclType::Expose => Ok(DeclType::Expose),
            _ => Err(format_err!("unknown decl type")),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq)]
pub enum RouteOutcome {
    Success,
    Void,
    Failed,
}

impl TryFrom<fsys::RouteOutcome> for RouteOutcome {
    type Error = anyhow::Error;

    fn try_from(value: fsys::RouteOutcome) -> std::result::Result<Self, Self::Error> {
        match value {
            fsys::RouteOutcome::Success => Ok(RouteOutcome::Success),
            fsys::RouteOutcome::Void => Ok(RouteOutcome::Void),
            fsys::RouteOutcome::Failed => Ok(RouteOutcome::Failed),
            _ => Err(format_err!("unknown route outcome")),
        }
    }
}

/// Call `RouteValidator/Route` with `moniker` and `targets`.
pub async fn route(
    route_validator: &fsys::RouteValidatorProxy,
    moniker: Moniker,
    targets: Vec<fsys::RouteTarget>,
) -> Result<Vec<RouteReport>> {
    let reports = match route_validator.route(moniker.as_ref(), &targets).await? {
        Ok(reports) => reports,
        Err(e) => {
            return Err(format_err!(
                "Component manager returned an unexpected error during routing: {:?}\n\
                 The state of the component instance may have changed.\n\
                 Please report this to the Component Framework team.",
                e
            ));
        }
    };

    reports.into_iter().map(|r| r.try_into()).collect()
}

/// Construct a table of routes from the given route reports.
pub fn create_table(reports: Vec<RouteReport>) -> Table {
    let mut table = Table::new();
    table.set_format(*FORMAT_CLEAN);

    let mut first = true;
    for report in reports {
        if first {
            first = false;
        } else {
            table.add_empty_row();
        }
        add_report(report, &mut table);
    }
    table
}

fn add_report(report: RouteReport, table: &mut Table) {
    table
        .add_row(row!(r->"Capability: ", &format!("{} ({})", report.capability, report.decl_type)));
    let (mark, summary) = match report.outcome {
        RouteOutcome::Success => {
            let mark = ansi_term::Color::Green.paint("[✓]");
            (mark, SUCCESS_SUMMARY)
        }
        RouteOutcome::Void => {
            let mark = ansi_term::Color::Yellow.paint("[~]");
            (mark, VOID_SUMMARY)
        }
        RouteOutcome::Failed => {
            let mark = ansi_term::Color::Red.paint("[✗]");
            let summary = report
                .error_summary
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or("Missing error summary. This is a bug.");
            (mark, summary)
        }
    };
    table.add_row(row!(r->"Result: ", &format!("{} {}", mark, summary)));
    if let Some(source_moniker) = report.source_moniker {
        let source_moniker = match ExtendedMoniker::parse_str(&source_moniker) {
            Ok(m) => m.to_string(),
            Err(e) => format!("<invalid moniker>: {}: {}", e, source_moniker),
        };
        table.add_row(row!(r->"Source: ", source_moniker));
    }
    if let Some(service_instances) = report.service_instances {
        let mut service_table = Table::new();
        let mut format = *FORMAT_CLEAN;
        format.padding(0, 0);
        service_table.set_format(format);
        let mut first = true;
        for service_instance in service_instances {
            if first {
                first = false;
            } else {
                service_table.add_empty_row();
            }
            service_table.add_row(row!(r->"Name: ", &service_instance.instance_name));
            service_table.add_row(row!(r->"Source child: ", &service_instance.child_name));
            service_table.add_row(row!(r->"Name in child: ",
                &service_instance.child_instance_name));
        }
        table.add_row(row!(r->"Service instances: ", service_table));
    }
    if let Some(dictionary_entries) = report.dictionary_entries {
        let mut dict_table = Table::new();
        let mut format = *FORMAT_CLEAN;
        format.padding(0, 0);
        dict_table.set_format(format);
        for e in dictionary_entries {
            dict_table.add_row(row!(&e.name));
        }
        table.add_row(row!(r->"Contents: ", dict_table));
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;

    fn route_validator(
        expected_moniker: &'static str,
        expected_targets: Vec<fsys::RouteTarget>,
        reports: Vec<fsys::RouteReport>,
    ) -> fsys::RouteValidatorProxy {
        let (route_validator, mut stream) =
            endpoints::create_proxy_and_stream::<fsys::RouteValidatorMarker>();
        fasync::Task::local(async move {
            match stream.try_next().await.unwrap().unwrap() {
                fsys::RouteValidatorRequest::Validate { .. } => {
                    panic!("unexpected Validate request");
                }
                fsys::RouteValidatorRequest::Route { moniker, targets, responder } => {
                    assert_eq!(
                        Moniker::parse_str(expected_moniker).unwrap(),
                        Moniker::parse_str(&moniker).unwrap()
                    );
                    assert_eq!(expected_targets, targets);
                    responder.send(Ok(&reports)).unwrap();
                }
            }
        })
        .detach();
        route_validator
    }

    #[fuchsia::test]
    async fn test_errors() {
        let targets =
            vec![fsys::RouteTarget { decl_type: fsys::DeclType::Use, name: "fuchsia.foo".into() }];
        let validator = route_validator(
            "/test",
            targets.clone(),
            vec![fsys::RouteReport {
                capability: Some("fuchsia.foo.bar".into()),
                decl_type: Some(fsys::DeclType::Use),
                error: Some(fsys::RouteError {
                    summary: Some("Access denied".into()),
                    ..Default::default()
                }),
                // Test inference of Failed
                outcome: None,
                ..Default::default()
            }],
        );

        let mut reports =
            route(&validator, Moniker::parse_str("./test").unwrap(), targets).await.unwrap();
        assert_eq!(reports.len(), 1);

        let report = reports.remove(0);
        assert_matches!(
            report,
            RouteReport {
                capability,
                decl_type: DeclType::Use,
                error_summary: Some(s),
                source_moniker: None,
                service_instances: None,
                dictionary_entries: None,
                outcome: RouteOutcome::Failed,
            } if capability == "fuchsia.foo.bar" && s == "Access denied"
        );
    }

    #[fuchsia::test]
    async fn test_no_errors() {
        let targets =
            vec![fsys::RouteTarget { decl_type: fsys::DeclType::Use, name: "fuchsia.foo".into() }];
        let validator = route_validator(
            "/test",
            targets.clone(),
            vec![
                fsys::RouteReport {
                    capability: Some("fuchsia.foo.bar".into()),
                    decl_type: Some(fsys::DeclType::Use),
                    source_moniker: Some("<component manager>".into()),
                    error: None,
                    outcome: Some(fsys::RouteOutcome::Void),
                    ..Default::default()
                },
                fsys::RouteReport {
                    capability: Some("fuchsia.foo.baz".into()),
                    decl_type: Some(fsys::DeclType::Expose),
                    source_moniker: Some("/test/src".into()),
                    service_instances: Some(vec![
                        fsys::ServiceInstance {
                            instance_name: Some("1234abcd".into()),
                            child_name: Some("a".into()),
                            child_instance_name: Some("default".into()),
                            ..Default::default()
                        },
                        fsys::ServiceInstance {
                            instance_name: Some("abcd1234".into()),
                            child_name: Some("b".into()),
                            child_instance_name: Some("other".into()),
                            ..Default::default()
                        },
                    ]),
                    dictionary_entries: Some(vec![
                        fsys::DictionaryEntry { name: Some("k1".into()), ..Default::default() },
                        fsys::DictionaryEntry { name: Some("k2".into()), ..Default::default() },
                    ]),
                    error: None,
                    // Test inference of Success
                    outcome: None,
                    ..Default::default()
                },
            ],
        );

        let mut reports =
            route(&validator, Moniker::parse_str("./test").unwrap(), targets).await.unwrap();
        assert_eq!(reports.len(), 2);

        let report = reports.remove(0);
        assert_matches!(
            report,
            RouteReport {
                capability,
                decl_type: DeclType::Use,
                error_summary: None,
                source_moniker: Some(m),
                service_instances: None,
                dictionary_entries: None,
                outcome: RouteOutcome::Void,
            } if capability == "fuchsia.foo.bar" && m == "<component manager>"
        );

        let report = reports.remove(0);
        assert_matches!(
            report,
            RouteReport {
                capability,
                decl_type: DeclType::Expose,
                error_summary: None,
                source_moniker: Some(m),
                service_instances: Some(s),
                dictionary_entries: Some(d),
                outcome: RouteOutcome::Success,
            } if capability == "fuchsia.foo.baz" && m == "/test/src"
                && s == vec![
                    ServiceInstance {
                        instance_name: "1234abcd".into(),
                        child_name: "a".into(),
                        child_instance_name: "default".into(),
                    },
                    ServiceInstance {
                        instance_name: "abcd1234".into(),
                        child_name: "b".into(),
                        child_instance_name: "other".into(),
                    },
                ]
                && d == vec![
                    DictionaryEntry {
                        name: "k1".into(),
                    },
                    DictionaryEntry {
                        name: "k2".into(),
                    },
                ]
        );
    }

    #[fuchsia::test]
    async fn test_no_routes() {
        let validator = route_validator("/test", vec![], vec![]);

        let reports =
            route(&validator, Moniker::parse_str("./test").unwrap(), vec![]).await.unwrap();
        assert!(reports.is_empty());
    }

    #[fuchsia::test]
    async fn test_parse_error() {
        let validator = route_validator(
            "/test",
            vec![],
            vec![
                // Don't set any fields
                fsys::RouteReport::default(),
            ],
        );

        let result = route(&validator, Moniker::parse_str("./test").unwrap(), vec![]).await;
        assert_matches!(result, Err(_));
    }
}
