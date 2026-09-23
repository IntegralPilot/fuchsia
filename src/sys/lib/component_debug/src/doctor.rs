// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::route::DictionaryEntry;
use anyhow::{Result, format_err};
use flex_fuchsia_sys2 as fsys;
use moniker::Moniker;
use prettytable::format::consts::FORMAT_CLEAN;
use prettytable::{Row, Table, row};

const USE_TITLE: &'static str = "Used Capability";
const EXPOSE_TITLE: &'static str = "Exposed Capability";
const SUCCESS_SUMMARY: &'static str = "Success";
const CAPABILITY_COLUMN_WIDTH: usize = 50;
const SUMMARY_COLUMN_WIDTH: usize = 80;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// Analytical information about a capability.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug)]
pub struct RouteReport {
    pub decl_type: DeclType,

    /// The name of the capability (for DeclType::Expose), or the path of
    /// the capability in the namespace (for DeclType::Use).
    pub capability: String,

    /// The type of the capability (e.g. protocol, directory, resolver, ...)
    pub build_time_capability_type: Option<String>,

    /// If Some, indicates a routing error for this route.
    pub error_summary: Option<String>,

    /// The requested level of availability of the capability.
    pub availability: Option<cm_rust::Availability>,

    /// The contents of the dictionary, if the capability was a dictionary.
    pub dictionary_entries: Option<Vec<DictionaryEntry>>,
}

impl TryFrom<fsys::RouteReport> for RouteReport {
    type Error = anyhow::Error;

    fn try_from(report: fsys::RouteReport) -> Result<Self> {
        let decl_type =
            report.decl_type.ok_or_else(|| format_err!("missing decl type"))?.try_into()?;
        let capability = report.capability.ok_or_else(|| format_err!("missing capability name"))?;
        let availability: Option<cm_rust::Availability> =
            report.availability.map(cm_rust::Availability::from);
        let dictionary_entries = report
            .dictionary_entries
            .map(|e| e.into_iter().map(DictionaryEntry::try_from).collect())
            .transpose()?;
        let error_summary = if let Some(error) = report.error { error.summary } else { None };
        Ok(RouteReport {
            decl_type,
            capability,
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            build_time_capability_type: report.build_time_capability_type,
            #[cfg(fuchsia_api_level_less_than = "HEAD")]
            capability_type: None,
            error_summary,
            availability,
            dictionary_entries,
        })
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq)]
pub enum DeclType {
    Use,
    Expose,
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

/// Returns a list of individual RouteReports for use and expose declarations
/// for the component. Any individual report with `error_summary` set to Some()
/// indicates a routing error.
pub async fn validate_routes(
    route_validator: &fsys::RouteValidatorProxy,
    moniker: &Moniker,
) -> Result<Vec<RouteReport>> {
    let reports = match route_validator.validate(moniker.as_ref()).await? {
        Ok(reports) => reports,
        Err(e) => {
            return Err(format_err!(
                "Component manager returned an unexpected error during validation: {:?}\n\
                 The state of the component instance may have changed.\n\
                 Please report this to the Component Framework team.",
                e
            ));
        }
    };

    reports.into_iter().map(|r| r.try_into()).collect()
}

fn format(report: &RouteReport) -> Vec<Row> {
    let capability = match report.dictionary_entries {
        Some(_) => format!("{} (Dictionary)", report.capability),
        None => report.capability.clone(),
    };
    let capability = match &report.build_time_capability_type {
        Some(capability_type) => format!("{} {}", capability_type, capability),
        None => capability,
    };
    let capability = match report.availability {
        Some(cm_rust::Availability::Required) | None => capability,
        Some(availability) => format!("{} ({})", capability, availability),
    };
    let capability = textwrap::fill(&capability, CAPABILITY_COLUMN_WIDTH);
    let (mark, summary) = if let Some(summary) = &report.error_summary {
        let mark = ansi_term::Color::Red.paint("[✗]");
        let summary = textwrap::fill(summary, SUMMARY_COLUMN_WIDTH);
        (mark, summary)
    } else {
        let mark = ansi_term::Color::Green.paint("[✓]");
        let summary = textwrap::fill(SUCCESS_SUMMARY, SUMMARY_COLUMN_WIDTH);
        (mark, summary)
    };
    let mut rows = vec![row!(mark, capability, summary)];
    if let Some(dictionary_entries) = &report.dictionary_entries {
        let mut table = Table::new();
        let mut format = *FORMAT_CLEAN;
        format.padding(0, 0);
        table.set_format(format);
        for e in dictionary_entries {
            table.add_row(row!(&e.name));
        }
        rows.push(row!("", table))
    }
    rows
}

// Construct the used and exposed capability tables from the given route reports.
pub fn create_tables(reports: &Vec<RouteReport>) -> (Table, Table) {
    let mut use_table = new_table(USE_TITLE);
    let mut expose_table = new_table(EXPOSE_TITLE);

    for report in reports {
        match &report.decl_type {
            DeclType::Use => {
                for r in format(&report) {
                    use_table.add_row(r);
                }
            }
            DeclType::Expose => {
                for r in format(&report) {
                    expose_table.add_row(r);
                }
            }
        };
    }
    (use_table, expose_table)
}

// Create a new table with the given title.
fn new_table(title: &str) -> Table {
    let mut table = Table::new();
    table.set_format(*FORMAT_CLEAN);
    table.set_titles(row!("", title.to_string(), "Result"));
    table
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use futures::TryStreamExt;

    fn route_validator(
        expected_moniker: &'static str,
        reports: Vec<fsys::RouteReport>,
    ) -> fsys::RouteValidatorProxy {
        let (route_validator, mut stream) = create_proxy_and_stream::<fsys::RouteValidatorMarker>();
        fuchsia_async::Task::local(async move {
            match stream.try_next().await.unwrap().unwrap() {
                fsys::RouteValidatorRequest::Validate { moniker, responder, .. } => {
                    assert_eq!(Moniker::parse_str(expected_moniker), Moniker::parse_str(&moniker));
                    responder.send(Ok(&reports)).unwrap();
                }
                fsys::RouteValidatorRequest::Route { .. } => {
                    panic!("unexpected Route request");
                }
            }
        })
        .detach();
        route_validator
    }

    #[fuchsia::test]
    async fn test_errors() {
        let validator = route_validator(
            "/test",
            vec![fsys::RouteReport {
                capability: Some("fuchsia.foo.bar".to_string()),
                decl_type: Some(fsys::DeclType::Use),
                error: Some(fsys::RouteError {
                    summary: Some("Access denied".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        );

        let mut reports =
            validate_routes(&validator, &Moniker::parse_str("test").unwrap()).await.unwrap();
        assert_eq!(reports.len(), 1);

        let report = reports.remove(0);
        assert_eq!(report.capability, "fuchsia.foo.bar");
        assert_eq!(report.decl_type, DeclType::Use);

        let error = report.error_summary.unwrap();
        assert_eq!(error, "Access denied");
    }

    #[fuchsia::test]
    async fn test_no_errors() {
        let validator = route_validator(
            "/test",
            vec![fsys::RouteReport {
                capability: Some("fuchsia.foo.bar".to_string()),
                decl_type: Some(fsys::DeclType::Use),
                dictionary_entries: Some(vec![fsys::DictionaryEntry {
                    name: Some("k1".into()),
                    ..Default::default()
                }]),
                error: None,
                ..Default::default()
            }],
        );

        let mut reports =
            validate_routes(&validator, &Moniker::parse_str("test").unwrap()).await.unwrap();
        assert_eq!(reports.len(), 1);

        let report = reports.remove(0);
        assert_eq!(report.capability, "fuchsia.foo.bar");
        assert_eq!(report.decl_type, DeclType::Use);
        assert_eq!(report.dictionary_entries.unwrap(), [DictionaryEntry { name: "k1".into() }]);
        assert!(report.error_summary.is_none());
    }

    #[fuchsia::test]
    async fn test_no_routes() {
        let validator = route_validator("test", vec![]);

        let reports =
            validate_routes(&validator, &Moniker::parse_str("test").unwrap()).await.unwrap();
        assert!(reports.is_empty());
    }

    #[fuchsia::test]
    async fn test_parse_error() {
        let validator = route_validator(
            "/test",
            vec![
                // Don't set any fields
                fsys::RouteReport::default(),
            ],
        );

        let result = validate_routes(&validator, &Moniker::parse_str("test").unwrap()).await;
        assert!(result.is_err());
    }
}
