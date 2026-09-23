// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use fdomain_fuchsia_diagnostics::{ComponentSelector, LogInterestSelector, StringSelector};
use fdomain_fuchsia_diagnostics_types::Interest;
use fdomain_fuchsia_sys2 as fsys;
use run_test_suite_lib::TestParams;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use test_list::{ExecutionEntry, FuchsiaComponentExecutionEntry, TestList, TestListEntry, TestTag};

/// Test parameters for deserialization when test_pilot is used.
#[derive(Deserialize, Default, PartialEq, Debug)]
struct PilotTestParams {
    /// Directory in which to deposit test output.
    pub output_directory: PathBuf,

    /// URL of the test component to be run.
    pub target_test_url: String,

    /// Arguments to be passed to the test component.
    #[serde(default)]
    pub target_test_args: Vec<String>,

    /// Maximum severity of logs produced by the test when successful. If more severe logs
    /// are produced, the test has failed.
    #[serde(default)]
    pub max_severity_logs: Option<diagnostics_data::Severity>,

    /// Severity below which log messages will not be printed.
    #[serde(default)]
    pub min_severity_logs: Option<diagnostics_data::Severity>,

    /// Realm in which to execute the test in the form <realm path>:<collection>.
    #[serde(default)]
    pub realm: Option<String>,

    /// Client-defined dictionary of name/value pairs that describe the test. Each `HashMap` in
    /// the vector should contain two pairs with keys 'name' and 'value'.
    #[serde(default)]
    pub tags: Vec<HashMap<String, String>>,

    /// Target device to which ffx test has access.
    pub target: String,

    /// Glob patterns specifying which test cases to execute. A case is selected if it matches
    /// any of the specified patterns.
    #[serde(default)]
    pub test_case_filters: Option<Vec<String>>,

    /// Whether to run otherwise disabled cases. By default, disabled cases are not run.
    #[serde(default)]
    pub run_disabled_cases: bool,

    /// Whether the first failure should cause test execution to stop with zxdb attached to the
    /// failing process
    #[serde(default)]
    pub break_on_failure: bool,

    /// Whether the test manager should refrain from creating exception channels, because the
    /// test creates conflicting ones.
    #[serde(default)]
    pub no_exception_channel: bool,

    /// Indicates the number of test cases that may be run in parallel.
    #[serde(default)]
    pub max_concurrent_test_case_runs: Option<u16>,

    /// Indicates the number of test cases that may be run in parallel.
    #[serde(default)]
    pub timeout: Option<std::num::NonZero<u32>>,

    /// Whether the full moniker should be shown in unstructured logs.
    #[serde(default)]
    pub show_full_moniker_in_logs: bool,

    /// Maximum number of seconds the test may run before being terminated due to a timeout.
    #[serde(flatten)]
    pub unknown: HashMap<String, Value>,
}

// Complete set of params needed to run a test.
#[derive(Debug, PartialEq)]
pub struct CombinedParams {
    pub run_params: run_test_suite_lib::RunParams,
    pub test_params: run_test_suite_lib::TestParams,
    pub output_directory: Option<PathBuf>,
}

pub async fn combined_params_from_pilot_reader<R: Read>(
    reader: R,
    lifecycle_controller: &fsys::LifecycleControllerProxy,
    realm_query: &fsys::RealmQueryProxy,
    timeout_grace_seconds: u32,
) -> fho::Result<CombinedParams> {
    let pilot_test_params: PilotTestParams =
        serde_json::from_reader(reader).map_err(anyhow::Error::from)?;

    let mut provided_realm = None;
    if let Some(realm_str) = &pilot_test_params.realm {
        provided_realm = Some(
            run_test_suite_lib::parse_provided_realm(
                &lifecycle_controller,
                &realm_query,
                &realm_str,
            )
            .await
            .map_err(|e| errors::ffx_error!("Error parsing realm '{}': {}", realm_str, e))?,
        );
    }

    let test_params = TestParams {
        test_url: pilot_test_params.target_test_url,
        test_args: pilot_test_params.target_test_args,
        realm: provided_realm.into(),
        timeout_seconds: pilot_test_params.timeout,
        test_filters: pilot_test_params.test_case_filters,
        also_run_disabled_tests: pilot_test_params.run_disabled_cases,
        parallel: pilot_test_params.max_concurrent_test_case_runs,
        max_severity_logs: pilot_test_params.max_severity_logs,
        min_severity_logs: min_severity_param_value_to_selectors(
            pilot_test_params.min_severity_logs,
        ),
        tags: tags_param_value_to_test_tags(pilot_test_params.tags),
        break_on_failure: pilot_test_params.break_on_failure,
        no_exception_channel: pilot_test_params.no_exception_channel,
        ..Default::default()
    };

    let run_params = run_test_suite_lib::RunParams {
        timeout_grace_seconds: timeout_grace_seconds,
        accumulate_debug_data: false, // ffx never accumulates.
        log_protocol: None,
        min_severity_logs: test_params.min_severity_logs.clone(),
        show_full_moniker: pilot_test_params.show_full_moniker_in_logs,

        // These apply only to multiple-suite runs, which are no longer supported.
        timeout_behavior: run_test_suite_lib::TimeoutBehavior::TerminateRemaining,
        stop_after_failures: None,
    };

    let outdir = String::from(
        pilot_test_params.output_directory.to_str().expect("output_directory is a string"),
    );

    Ok(CombinedParams { run_params, test_params, output_directory: Some(PathBuf::from(outdir)) })
}

pub async fn test_params_from_reader<R: Read>(
    reader: R,
    lifecycle_controller: &fsys::LifecycleControllerProxy,
    realm_query: &fsys::RealmQueryProxy,
) -> Result<TestParams> {
    let test_list: TestList = serde_json::from_reader(reader).map_err(anyhow::Error::from)?;
    let TestList::Experimental { mut data } = test_list;
    if data.len() != 1 {
        return Err(format_err!(
            "Expected only a single test run per invocation, got {}",
            data.len()
        ));
    }

    let TestListEntry { tags, execution, name, .. } = data.remove(0);
    match execution {
        Some(ExecutionEntry::FuchsiaComponent(component_execution)) => {
            let FuchsiaComponentExecutionEntry {
                component_url,
                test_args,
                timeout_seconds,
                test_filters,
                no_cases_equals_success,
                also_run_disabled_tests,
                parallel,
                max_severity_logs,
                min_severity_logs,
                realm,
                create_no_exception_channel,
            } = component_execution;
            let mut provided_realm = None;
            if let Some(realm_str) = &realm {
                provided_realm = Some(
                    run_test_suite_lib::parse_provided_realm(
                        &lifecycle_controller,
                        &realm_query,
                        &realm_str,
                    )
                    .await
                    .map_err(|e| {
                        errors::ffx_error!("Error parsing realm '{}': {}", realm_str, e)
                    })?,
                );
            }
            Ok(TestParams {
                test_url: component_url,
                realm: provided_realm.into(),
                test_args,
                timeout_seconds,
                test_filters,
                no_cases_equals_success,
                also_run_disabled_tests,
                parallel,
                max_severity_logs,
                min_severity_logs: min_severity_param_value_to_selectors(min_severity_logs),
                tags,
                break_on_failure: false,
                no_exception_channel: create_no_exception_channel,
            })
        }
        _ => Err(format_err!(
            "Cannot execute {name}, only \"fuchsia_component\" test execution is supported."
        )),
    }
}

/// Creates a vector of `TestTag` from the `tags` value generated by the JSON parser, which
/// consists of a vector of hashmaps. Each hashmap is intended to contain two pairs, one with
/// the name 'name' and the other with the name 'value'.
fn tags_param_value_to_test_tags(value: Vec<HashMap<String, String>>) -> Vec<TestTag> {
    value
        .into_iter()
        .filter_map(|map| {
            if let Some(key) = map.get("key") {
                if let Some(value) = map.get("value") {
                    Some(TestTag { key: String::from(key), value: String::from(value) })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Converts an optional `Severity` produced by the JSON parser into a vector of
/// `LogInterestSelector`. This conversion accepts only a simple severity, which applies to
/// all relevant components. Multiple severities and explicit component selectors are not
/// supported.
fn min_severity_param_value_to_selectors(
    value: Option<diagnostics_data::Severity>,
) -> Vec<LogInterestSelector> {
    let mut min_severity_selectors = vec![];

    if let Some(min_severity) = value {
        min_severity_selectors.push(LogInterestSelector {
            selector: ComponentSelector {
                moniker_segments: Some(vec![StringSelector::StringPattern("**".into())]),
                ..Default::default()
            },
            interest: Interest { min_severity: Some(min_severity.into()), ..Default::default() },
        });
    }

    min_severity_selectors
}

#[cfg(test)]
mod test {
    use super::*;

    const TEST_LIST_VALID: &'static str = r#"
    {
        "schema_id": "experimental",
        "data": [
            {
                "name": "test",
                "labels": [],
                "tags": [],
                "execution": {
                    "type": "fuchsia_component",
                    "component_url": "fuchsia.com"
                }
            }
        ]
    }
    "#;

    const TEST_LIST_INVALID: &'static str = r#"
    {
        "schema_id": "experimental",
        "data": [
            {
                "name": "test",
                "labels": [],
                "tags": [],
                "execution": {
                    "type": "fuchsia_component",
                    "component_url": "fuchsia.com"
                }
            },
            {
                "name": "test3",
                "labels": [],
                "tags": []
            }
        ]
    }
    "#;

    #[fuchsia::test]
    async fn test_params_from_reader_valid() {
        let client = fdomain_local::local_client_empty();
        let reader = TEST_LIST_VALID.as_bytes();
        let (lifecycle_controller, _server_end1) =
            client.create_proxy::<fsys::LifecycleControllerMarker>();
        let (realm_query, _server_end2) = client.create_proxy::<fsys::RealmQueryMarker>();
        let test_params = test_params_from_reader(reader, &lifecycle_controller, &realm_query)
            .await
            .expect("read file");
        assert_eq!(
            TestParams {
                test_url: String::from("fuchsia.com"),
                realm: None.into(),
                timeout_seconds: None,
                test_filters: None,
                no_cases_equals_success: None,
                also_run_disabled_tests: false,
                parallel: None,
                test_args: vec![],
                max_severity_logs: None,
                min_severity_logs: vec![],
                tags: vec![],
                break_on_failure: false,
                no_exception_channel: false,
            },
            test_params
        );
    }

    #[fuchsia::test]
    async fn test_params_from_reader_invalid() {
        let client = fdomain_local::local_client_empty();
        let reader = TEST_LIST_INVALID.as_bytes();
        let (lifecycle_controller, _server_end1) =
            client.create_proxy::<fsys::LifecycleControllerMarker>();
        let (realm_query, _server_end2) = client.create_proxy::<fsys::RealmQueryMarker>();
        let test_params =
            test_params_from_reader(reader, &lifecycle_controller, &realm_query).await;
        assert!(test_params.is_err());
    }

    const PILOT_CONFIG_VALID: &'static str = r#"
    {
        "output_directory": "test/output/directory",
        "target_test_url": "test_target_test_url",
        "target_test_args": [ "test", "args" ],
        "max_severity_logs": "error",
        "min_severity_logs": "warn",
        "tags": [ { "key": "test", "value": "tags" } ],
        "target": "test_target",
        "test_case_filters": [ "test", "case", "filters" ],
        "run_disabled_cases": true,
        "break_on_failure": true,
        "no_exception_channel": true,
        "max_concurrent_test_case_runs": 12,
        "timeout": 1000,
        "show_full_moniker_in_logs": true
    }
    "#;

    const PILOT_CONFIG_INVALID: &'static str = r#"
    {
        "output_directory": "test/output/directory",
        "target_test_url": "test_target_test_url",
        "target_test_args": [ "test", "args" ],
        "max_severity_logs": "error",
        "min_severity_logs": "warn",
        "tags": [ { "key": "test", "value": "tags" } ],
        "target": "test_target",
        "test_case_filters": [ "test", "case", "filters" ],
        "run_disabled_cases": true,
        "break_on_failure": true,
        "no_exception_channel": true,
        "max_concurrent_test_case_runs": 12,
        "timeout": 1000,
        "show_full_moniker_in_logs": true,
    }
    "#;

    #[fuchsia::test]
    async fn test_combined_params_from_pilot_reader() {
        let client = fdomain_local::local_client_empty();
        let reader = PILOT_CONFIG_VALID.as_bytes();
        let (lifecycle_controller, _server_end1) =
            client.create_proxy::<fsys::LifecycleControllerMarker>();
        let (realm_query, _server_end2) = client.create_proxy::<fsys::RealmQueryMarker>();
        let combined_params =
            combined_params_from_pilot_reader(reader, &lifecycle_controller, &realm_query, 10)
                .await
                .expect("read file");

        assert_eq!(
            CombinedParams {
                run_params: run_test_suite_lib::RunParams {
                    timeout_behavior: run_test_suite_lib::TimeoutBehavior::TerminateRemaining,
                    timeout_grace_seconds: 10,
                    stop_after_failures: None,
                    accumulate_debug_data: false,
                    log_protocol: None,
                    min_severity_logs: vec![LogInterestSelector {
                        selector: ComponentSelector {
                            moniker_segments: Some(vec![StringSelector::StringPattern(
                                "**".into()
                            )]),
                            ..Default::default()
                        },
                        interest: Interest {
                            min_severity: Some(diagnostics_data::Severity::Warn.into()),
                            ..Default::default()
                        },
                    }],
                    show_full_moniker: true,
                },
                test_params: run_test_suite_lib::TestParams {
                    test_url: String::from("test_target_test_url"),
                    realm: None.into(),
                    timeout_seconds: std::num::NonZero::new(1000),
                    test_filters: Some(vec![
                        String::from("test"),
                        String::from("case"),
                        String::from("filters")
                    ]),
                    no_cases_equals_success: None,
                    also_run_disabled_tests: true,
                    parallel: Some(12),
                    test_args: vec![String::from("test"), String::from("args")],
                    max_severity_logs: Some(diagnostics_data::Severity::Error),
                    min_severity_logs: vec![LogInterestSelector {
                        selector: ComponentSelector {
                            moniker_segments: Some(vec![StringSelector::StringPattern(
                                "**".into()
                            )]),
                            ..Default::default()
                        },
                        interest: Interest {
                            min_severity: Some(diagnostics_data::Severity::Warn.into()),
                            ..Default::default()
                        },
                    }],
                    tags: vec![test_list::TestTag {
                        key: String::from("test"),
                        value: String::from("tags")
                    }],
                    break_on_failure: true,
                    no_exception_channel: true,
                },
                output_directory: Some(PathBuf::from("test/output/directory")),
            },
            combined_params
        );
    }

    #[fuchsia::test]
    async fn test_combined_params_from_pilot_reader_invalid() {
        let client = fdomain_local::local_client_empty();
        let reader = PILOT_CONFIG_INVALID.as_bytes();
        let (lifecycle_controller, _server_end1) =
            client.create_proxy::<fsys::LifecycleControllerMarker>();
        let (realm_query, _server_end2) = client.create_proxy::<fsys::RealmQueryMarker>();
        let combined_params_result =
            combined_params_from_pilot_reader(reader, &lifecycle_controller, &realm_query, 10)
                .await;
        assert!(combined_params_result.is_err());
    }

    const PILOT_CONFIG_SEVERITY_UPPER: &'static str = r#"
    {
        "output_directory": "test/output/directory",
        "target_test_url": "test_target_test_url",
        "max_severity_logs": "ERROR",
        "min_severity_logs": "WARN",
        "target": "test_target"
    }
    "#;

    const PILOT_CONFIG_SEVERITY_LOWER: &'static str = r#"
    {
        "output_directory": "test/output/directory",
        "target_test_url": "test_target_test_url",
        "max_severity_logs": "error",
        "min_severity_logs": "warn",
        "target": "test_target"
    }
    "#;

    const PILOT_CONFIG_SEVERITY_PASCAL: &'static str = r#"
    {
        "output_directory": "test/output/directory",
        "target_test_url": "test_target_test_url",
        "max_severity_logs": "Error",
        "min_severity_logs": "Warn",
        "target": "test_target"
    }
    "#;

    // Ensures that `Severity` deserialization accepts UPPER, lower and Pascal case values
    // for the variants. The enum is marked rename_all = "UPPERCASE", but this test passes
    // anyway. This test might start to fail if a new version of serde is stricter about
    // enum deserialization, in which case a custom deserializer or use of new attributes
    // may be required.
    #[fuchsia::test]
    async fn test_pilot_reader_severity_cases() {
        let client = fdomain_local::local_client_empty();
        let reader = PILOT_CONFIG_SEVERITY_UPPER.as_bytes();
        let (lifecycle_controller, _server_end1) =
            client.create_proxy::<fsys::LifecycleControllerMarker>();
        let (realm_query, _server_end2) = client.create_proxy::<fsys::RealmQueryMarker>();
        assert!(
            combined_params_from_pilot_reader(reader, &lifecycle_controller, &realm_query, 10)
                .await
                .is_ok()
        );

        let reader = PILOT_CONFIG_SEVERITY_LOWER.as_bytes();
        let (lifecycle_controller, _server_end1) =
            client.create_proxy::<fsys::LifecycleControllerMarker>();
        let (realm_query, _server_end2) = client.create_proxy::<fsys::RealmQueryMarker>();
        assert!(
            combined_params_from_pilot_reader(reader, &lifecycle_controller, &realm_query, 10)
                .await
                .is_ok()
        );

        let reader = PILOT_CONFIG_SEVERITY_PASCAL.as_bytes();
        let (lifecycle_controller, _server_end1) =
            client.create_proxy::<fsys::LifecycleControllerMarker>();
        let (realm_query, _server_end2) = client.create_proxy::<fsys::RealmQueryMarker>();
        assert!(
            combined_params_from_pilot_reader(reader, &lifecycle_controller, &realm_query, 10)
                .await
                .is_ok()
        );
    }
}
