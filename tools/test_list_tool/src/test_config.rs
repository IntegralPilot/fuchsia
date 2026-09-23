// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Prototyping test configuration for Fuchsia Test ABI. Will move it to its own tool once MVP is
// complete.

use crate::TestsJsonEntry;
use diagnostics_log_types::Severity;
use serde::{Deserialize, Serialize};
use test_list::{FuchsiaComponentExecutionEntry, TestTag};

/// We only need to ever support generation of a single version of test configuration.
/// Consumed by test executors and test pilot to execute the test.
const TEST_CONFIG_VERSION: &str = "1";
#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestConfig {
    /// Version of the wire format
    pub version: String,

    /// Verbose name of the test. This is same as the one in tests.json. We will remove this once
    /// tests.json starts referring to individual config files
    pub name: String,

    /// Verbose label of the test. This is same as the one in tests.json. We will remove this once
    /// tests.json starts referring to individual config files
    pub label: String,

    /// Path to host test binary.
    pub host_test_binary: String,

    /// Path to a folder (relative to host test binary) with any resources required by the host test
    /// binary to run the test.
    pub resources: Option<String>,

    /// Arbitrary tags to identify and categorize the tests.
    pub tags: Vec<TestTag>,

    /// Specifies requested features for the test.
    pub requested_features: RequestedFeatures,

    /// Execution options
    pub execution: Execution,
}

/// Specifies requested features for the test.
#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestedFeatures {
    /// True if this test requires a path to SDK tools.
    pub sdk_tools_path: bool,

    /// True if this test requires a Fuchsia system.
    pub requires_target: bool,

    /// True if the tests requires a serial connection.
    pub requires_serial: bool,
}

/// Specifies Execution options. This is minimal configuration to run a test. More options would be
/// added later.
#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Execution {
    /// URL of the test component
    pub test_url: String,

    ///  Arguments for the test
    pub test_args: Vec<String>,

    // Optional filters of which test cases to include from the component.
    pub test_filters: Option<Vec<String>>,

    // Whether an empty set of test cases counts as success or failure
    pub no_cases_equals_success: Option<bool>,

    /// Weather to run disabled tests
    pub run_disabled_tests: bool,

    /// Max severity of the logs that the test is
    /// supposed to produce
    pub max_severity_logs: Option<Severity>,

    /// Run tests in parallel, if non-zero
    pub parallel: Option<u16>,

    /// The realm to run the test in
    pub realm: Option<String>,
}

// Create test config entry for a test component.
pub fn create_test_config_entry(
    test_tags: Vec<TestTag>,
    entry: &TestsJsonEntry,
    execution_entry: &FuchsiaComponentExecutionEntry,
) -> TestConfig {
    let config = TestConfig {
        version: TEST_CONFIG_VERSION.to_string(),
        name: entry.test.name.clone(),
        label: entry.test.label.clone(),
        host_test_binary: "path/to/bin".to_string(), // TODO: Fix this path when we have a common binary
        resources: None,
        tags: test_tags,
        requested_features: RequestedFeatures {
            sdk_tools_path: true,
            requires_target: true,
            requires_serial: false,
        },
        execution: Execution {
            test_url: execution_entry.component_url.clone(),
            test_args: execution_entry.test_args.clone(),
            test_filters: execution_entry.test_filters.clone(),
            no_cases_equals_success: execution_entry.no_cases_equals_success,
            run_disabled_tests: execution_entry.also_run_disabled_tests,
            max_severity_logs: execution_entry.max_severity_logs.clone(),
            realm: execution_entry.realm.clone(),
            parallel: execution_entry.parallel.clone(),
        },
    };
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestEntry;

    #[test]
    fn test_create_test_config_entry() {
        let tags = vec![
            TestTag { key: "key1".into(), value: "v1".into() },
            TestTag { key: "key2".into(), value: "v2".into() },
        ];
        let test_tags = tags.clone();
        let entry = TestsJsonEntry {
            test: TestEntry {
                name: "test_name".to_string(),
                label: "test_label".to_string(),
                cpu: "".into(),
                os: "".into(),
                package_url: None,
                component_label: None,
                package_label: None,
                package_manifests: None,
                log_settings: None,
                build_rule: None,
                has_generated_manifest: None,
                create_no_exception_channel: None,
                test_filters: None,
            },
        };
        let execution_entry = FuchsiaComponentExecutionEntry {
            component_url: "test_component_url".to_string(),
            test_args: vec!["arg1".to_string(), "arg2".to_string()],
            also_run_disabled_tests: true,
            max_severity_logs: Severity::Info.into(),
            realm: "test_realm".to_string().into(),
            timeout_seconds: None,
            test_filters: None,
            no_cases_equals_success: None,
            parallel: Some(10),
            min_severity_logs: None,
            create_no_exception_channel: false,
        };

        let expected_config = TestConfig {
            version: TEST_CONFIG_VERSION.to_string(),
            name: "test_name".to_string(),
            label: "test_label".to_string(),
            host_test_binary: "path/to/bin".to_string(),
            resources: None,
            tags: tags,
            requested_features: RequestedFeatures {
                sdk_tools_path: true,
                requires_target: true,
                requires_serial: false,
            },
            execution: Execution {
                test_url: "test_component_url".to_string(),
                test_args: vec!["arg1".to_string(), "arg2".to_string()],
                run_disabled_tests: true,
                test_filters: None,
                no_cases_equals_success: None,
                max_severity_logs: Severity::Info.into(),
                realm: "test_realm".to_string().into(),
                parallel: Some(10),
            },
        };

        let result = create_test_config_entry(test_tags.clone(), &entry, &execution_entry);
        assert_eq!(result, expected_config);
    }
}
