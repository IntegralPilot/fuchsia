// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};
use std::{fs, io};
use test_pilot_lib::test_output::{Summary, SummaryCase, SummaryOutcomeResult};

const FAILED_EXIT_CODE: i32 = 1;
const TEST_CASE_RESULT_FORMAT: &str = "FTF";

fn main() {
    let mut args = std::env::args();
    if args.len() != 4 {
        usage(FAILED_EXIT_CODE);
    }

    let _ = args.next();
    let config = args.next().unwrap();
    let from = args.next().unwrap();
    let to = args.next().unwrap();

    if !validate_existent_file_path(&config) {
        usage(FAILED_EXIT_CODE);
    }
    if !validate_existent_file_path(&from) {
        usage(FAILED_EXIT_CODE);
    }
    if !validate_nonexistent_path(&to) {
        usage(FAILED_EXIT_CODE);
    }

    match fs::File::open(&config) {
        Ok(file) => {
            let mut reader = io::BufReader::new(file);
            let test_config: TestConfig = serde_json::from_reader(&mut reader).unwrap();
            match fs::File::open(&from) {
                Ok(file) => {
                    let mut reader = io::BufReader::new(file);
                    let pilot_summary: Summary = serde_json::from_reader(&mut reader).unwrap();
                    let botanist_summary = convert(pilot_summary, test_config);
                    if let Err(err) =
                        fs::write(&to, serde_json::to_string_pretty(&botanist_summary).unwrap())
                    {
                        eprintln!("failed to write new summary file {to}: {err}");
                        std::process::exit(FAILED_EXIT_CODE);
                    }
                }
                Err(err) => {
                    eprintln!("failed to read summary file {from}: {err}");
                    std::process::exit(FAILED_EXIT_CODE);
                }
            }
        }
        Err(err) => {
            eprintln!("failed to open summary file {from}: {err}");
            std::process::exit(FAILED_EXIT_CODE);
        }
    }
}

fn usage(exit_code: i32) {
    eprintln!("usage: resummarize <config> <from> <to>");
    std::process::exit(exit_code);
}

// Validates `path`, verifying that the path references an existing file. Returns true if and onlyf
// if `path` is valid.
fn validate_existent_file_path(path: &str) -> bool {
    match fs::exists(path) {
        Ok(true) => match fs::metadata(path) {
            Ok(metadata) => {
                if metadata.is_file() {
                    true
                } else {
                    eprintln!("path {path} is a directory, expected a file");
                    false
                }
            }
            Err(err) => {
                eprintln!("failed to determine whether {path} is a file: {err}");
                false
            }
        },
        Ok(false) => {
            eprintln!("input file {path} does not exist");
            false
        }
        Err(err) => {
            eprintln!("failed to determine if {path} exists: {err}");
            false
        }
    }
}

// Validates `path`, verifying that the file/path does not exist.  Returns true if and only if
// `path` is valid.
fn validate_nonexistent_path(path: &str) -> bool {
    match fs::exists(path) {
        Ok(true) => {
            eprintln!("path {path} references existing file or directory, should not exist");
            false
        }
        Ok(false) => true,
        Err(err) => {
            eprintln!("failed to determine if {path} exists: {err}");
            false
        }
    }
}

// Converts a `Summary` into a `TestResult`
fn convert(pilot_summary: Summary, test_config: TestConfig) -> TestResult {
    TestResult {
        output_files: pilot_summary
            .common
            .artifacts
            .into_iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect(),
        output_dir: test_config.output_directory.clone(),
        cases: pilot_summary
            .cases
            .into_iter()
            .map(|(name, case)| convert_case(name, case, &test_config.output_directory))
            .collect(),
        failure_reason: if pilot_summary.common.outcome.result.is_failure() {
            FailureReason::from_message(pilot_summary.common.outcome.detail.unwrap_or_default())
        } else {
            None
        },
        setup_succeeded: pilot_summary.setup_succeeded,
        teardown_succeeded: pilot_summary.teardown_succeeded,
        exit_code: pilot_summary.exit_code,
    }
}

fn convert_case(name: String, case: SummaryCase, output_directory: &str) -> TestCaseResult {
    let failure_reason = if case.common.outcome.result.is_failure() {
        FailureReason::from_message(case.common.outcome.detail.unwrap_or_default())
    } else {
        None
    };
    TestCaseResult {
        display_name: name.clone(),
        suite_name: "".to_string(),
        case_name: name,
        status: convert_result(case.common.outcome.result),
        duration_nanos: case.common.duration * 1000000,
        format: TEST_CASE_RESULT_FORMAT.to_string(),
        failure_reason,
        output_files: case
            .common
            .artifacts
            .into_iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect(),
        output_dir: output_directory.to_string(),
        tags: vec![TestTag {
            key: "test_outcome".to_string(),
            value: convert_result_for_tag(case.common.outcome.result),
        }],
    }
}

fn convert_result(result: SummaryOutcomeResult) -> String {
    match result {
        SummaryOutcomeResult::NotSpecified => "NOT_SPECIFIED".to_string(),
        SummaryOutcomeResult::Skipped => "SKIP".to_string(),
        SummaryOutcomeResult::Passed => "PASS".to_string(),
        SummaryOutcomeResult::Canceled => "ABORT".to_string(),
        SummaryOutcomeResult::TimedOut => "FAIL".to_string(),
        SummaryOutcomeResult::Failed => "FAIL".to_string(),
        SummaryOutcomeResult::Error => "CRASH".to_string(),
    }
}

fn convert_result_for_tag(result: SummaryOutcomeResult) -> String {
    match result {
        SummaryOutcomeResult::NotSpecified => "NOT_SPECIFIED".to_string(),
        SummaryOutcomeResult::Skipped => "SKIPPED".to_string(),
        SummaryOutcomeResult::Passed => "PASSED".to_string(),
        SummaryOutcomeResult::Canceled => "CANCELED".to_string(),
        SummaryOutcomeResult::TimedOut => "TIMED_OUT".to_string(),
        SummaryOutcomeResult::Failed => "FAILED".to_string(),
        SummaryOutcomeResult::Error => "ERROR".to_string(),
    }
}

/// Parameters describing a test to be run.
#[derive(Deserialize, Debug)]
struct TestConfig {
    pub output_directory: String,
}

/// Body of the summary in the format used by botanist.
#[derive(Serialize, Debug)]
struct TestResult {
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_files: Vec<String>,

    pub output_dir: String,

    pub cases: Vec<TestCaseResult>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<FailureReason>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_succeeded: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub teardown_succeeded: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
struct FailureReason {
    pub errors: Vec<FailureReasonError>,
}

impl FailureReason {
    /// Constructs a FailureReason from a string message if non-empty after trimming,
    /// containing a single error message.
    pub fn from_message(message: String) -> Option<Self> {
        if message.trim().is_empty() {
            None
        } else {
            Some(Self { errors: vec![FailureReasonError { message, trace: String::new() }] })
        }
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
struct FailureReasonError {
    pub message: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trace: String,
}

/// Case summary in the format used by botanist.
#[derive(Serialize, Debug)]
struct TestCaseResult {
    pub display_name: String,

    pub suite_name: String,

    pub case_name: String,

    pub status: String,

    pub duration_nanos: i64,

    pub format: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<FailureReason>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_files: Vec<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output_dir: String,

    pub tags: Vec<TestTag>,
}

#[derive(Serialize, Deserialize, Debug)]
struct TestTag {
    pub key: String,

    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::{NamedTempFile, tempdir};
    use test_pilot_lib::test_output::{SummaryCommonProperties, SummaryOutcome};

    #[test]
    fn test_convert() {
        let pilot_summary = Summary {
            common: SummaryCommonProperties {
                duration: 123, // milliseconds
                outcome: SummaryOutcome {
                    result: SummaryOutcomeResult::Failed,
                    detail: Some("Overall test failed".to_string()),
                },
                artifacts: [(PathBuf::from("path/to/artifact1.txt"), Default::default())].into(),
                ..Default::default()
            },
            cases: HashMap::from([
                (
                    "case1".to_string(),
                    SummaryCase {
                        common: SummaryCommonProperties {
                            duration: 50, // milliseconds
                            outcome: SummaryOutcome {
                                result: SummaryOutcomeResult::Passed,
                                detail: None,
                            },
                            ..Default::default()
                        },
                    },
                ),
                (
                    "case2".to_string(),
                    SummaryCase {
                        common: SummaryCommonProperties {
                            duration: 73, // milliseconds
                            outcome: SummaryOutcome {
                                result: SummaryOutcomeResult::Failed,
                                detail: Some("assertion failed".to_string()),
                            },
                            artifacts: [(PathBuf::from("case2/log.txt"), Default::default())]
                                .into(),
                            ..Default::default()
                        },
                    },
                ),
                (
                    "case3".to_string(),
                    SummaryCase {
                        common: SummaryCommonProperties {
                            duration: 10, // milliseconds
                            outcome: SummaryOutcome {
                                result: SummaryOutcomeResult::Passed,
                                detail: Some("info message".to_string()),
                            },
                            ..Default::default()
                        },
                    },
                ),
            ]),
            ..Default::default()
        };

        let test_config = TestConfig { output_directory: "/path/to/output/directory".to_string() };

        let botanist_summary = convert(pilot_summary, test_config);

        assert_eq!(botanist_summary.output_files, vec!["path/to/artifact1.txt"]);
        assert_eq!(botanist_summary.output_dir, "/path/to/output/directory".to_string());
        assert_eq!(botanist_summary.cases.len(), 3);
        assert_eq!(
            botanist_summary.failure_reason,
            FailureReason::from_message("Overall test failed".to_string())
        );
        assert_eq!(botanist_summary.setup_succeeded, None);
        assert_eq!(botanist_summary.teardown_succeeded, None);
        assert_eq!(botanist_summary.exit_code, None);

        // The order of cases from a HashMap is not guaranteed, so we find them.
        let case1 = botanist_summary.cases.iter().find(|c| c.case_name == "case1").unwrap();
        assert_eq!(case1.display_name, "case1");
        assert_eq!(case1.suite_name, "");
        assert_eq!(case1.status, "PASS");
        assert_eq!(case1.duration_nanos, 50_000_000);
        assert_eq!(case1.format, "FTF");
        assert_eq!(case1.failure_reason, None);
        assert!(case1.output_files.is_empty());
        assert_eq!(case1.output_dir, "/path/to/output/directory");

        let case2 = botanist_summary.cases.iter().find(|c| c.case_name == "case2").unwrap();
        assert_eq!(case2.display_name, "case2");
        assert_eq!(case1.suite_name, "");
        assert_eq!(case2.status, "FAIL");
        assert_eq!(case2.duration_nanos, 73_000_000);
        assert_eq!(case2.format, "FTF");
        assert_eq!(
            case2.failure_reason,
            FailureReason::from_message("assertion failed".to_string())
        );
        assert_eq!(case2.output_files, vec!["case2/log.txt"]);
        assert_eq!(case2.output_dir, "/path/to/output/directory");

        let case3 = botanist_summary.cases.iter().find(|c| c.case_name == "case3").unwrap();
        assert_eq!(case3.display_name, "case3");
        assert_eq!(case3.status, "PASS");
        assert_eq!(case3.failure_reason, None);
    }

    #[test]
    fn test_convert_lifecycle_fields() {
        let pilot_summary = Summary {
            setup_succeeded: Some(true),
            teardown_succeeded: Some(false),
            exit_code: Some(0),
            ..Default::default()
        };

        let test_config = TestConfig { output_directory: "/path/to/output/directory".to_string() };

        let botanist_summary = convert(pilot_summary, test_config);

        assert_eq!(botanist_summary.setup_succeeded, Some(true));
        assert_eq!(botanist_summary.teardown_succeeded, Some(false));
        assert_eq!(botanist_summary.exit_code, Some(0));

        let botanist_summary_json = serde_json::to_value(&botanist_summary).unwrap();
        assert_eq!(botanist_summary_json.get("setup_succeeded"), Some(&serde_json::json!(true)));
        assert_eq!(
            botanist_summary_json.get("teardown_succeeded"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(botanist_summary_json.get("exit_code"), Some(&serde_json::json!(0)));
    }

    #[test]
    fn test_convert_lifecycle_fields_failure() {
        let pilot_summary = Summary {
            setup_succeeded: Some(false),
            teardown_succeeded: Some(true),
            exit_code: Some(86),
            ..Default::default()
        };

        let test_config = TestConfig { output_directory: "/path/to/output/directory".to_string() };

        let botanist_summary = convert(pilot_summary, test_config);

        assert_eq!(botanist_summary.setup_succeeded, Some(false));
        assert_eq!(botanist_summary.teardown_succeeded, Some(true));
        assert_eq!(botanist_summary.exit_code, Some(86));

        let botanist_summary_json = serde_json::to_value(&botanist_summary).unwrap();
        assert_eq!(botanist_summary_json.get("setup_succeeded"), Some(&serde_json::json!(false)));
        assert_eq!(botanist_summary_json.get("teardown_succeeded"), Some(&serde_json::json!(true)));
        assert_eq!(botanist_summary_json.get("exit_code"), Some(&serde_json::json!(86)));
    }

    #[test]
    fn test_failure_reason_from_message() {
        assert_eq!(FailureReason::from_message("".to_string()), None);
        assert_eq!(FailureReason::from_message("   \n".to_string()), None);
        assert!(FailureReason::from_message("test failure".to_string()).is_some());
    }

    #[test]
    fn test_convert_result() {
        assert_eq!(convert_result(SummaryOutcomeResult::Passed), "PASS");
        assert_eq!(convert_result(SummaryOutcomeResult::Failed), "FAIL");
        assert_eq!(convert_result(SummaryOutcomeResult::Skipped), "SKIP");
        assert_eq!(convert_result(SummaryOutcomeResult::TimedOut), "FAIL");
        assert_eq!(convert_result(SummaryOutcomeResult::Canceled), "ABORT");
        assert_eq!(convert_result(SummaryOutcomeResult::Error), "CRASH");
        assert_eq!(convert_result(SummaryOutcomeResult::NotSpecified), "NOT_SPECIFIED");
    }

    #[test]
    fn test_convert_result_for_tag() {
        assert_eq!(convert_result_for_tag(SummaryOutcomeResult::Passed), "PASSED");
        assert_eq!(convert_result_for_tag(SummaryOutcomeResult::Failed), "FAILED");
        assert_eq!(convert_result_for_tag(SummaryOutcomeResult::Skipped), "SKIPPED");
        assert_eq!(convert_result_for_tag(SummaryOutcomeResult::TimedOut), "TIMED_OUT");
        assert_eq!(convert_result_for_tag(SummaryOutcomeResult::Canceled), "CANCELED");
        assert_eq!(convert_result_for_tag(SummaryOutcomeResult::Error), "ERROR");
        assert_eq!(convert_result_for_tag(SummaryOutcomeResult::NotSpecified), "NOT_SPECIFIED");
    }

    #[test]
    fn test_validate_existent_file_path() {
        // Existent file.
        let temp_file = NamedTempFile::new().expect("to create temporary file");
        let temp_file_path = temp_file.path().to_str().unwrap().to_string();
        assert!(validate_existent_file_path(temp_file_path.as_str()));
        temp_file.close().expect("to close temporary file");

        // Non-existent file.
        assert!(!validate_existent_file_path("/non_existent_file"));

        // Existent directory.
        let temp_dir = tempdir().expect("to create temporary directory");
        let temp_dir_path = temp_dir.path().to_str().unwrap().to_string();
        assert!(!validate_existent_file_path(temp_dir_path.as_str()));
        temp_dir.close().expect("to close temporary directory");
    }

    #[test]
    fn test_validate_nonexistent_path() {
        // Existent file.
        let temp_file = NamedTempFile::new().expect("to create temporary file");
        let temp_file_path = temp_file.path().to_str().unwrap().to_string();
        assert!(!validate_nonexistent_path(temp_file_path.as_str()));
        temp_file.close().expect("to close temporary file");

        // Non-existent file.
        assert!(validate_nonexistent_path("/non_existent_file"));

        // Existent directory.
        let temp_dir = tempdir().expect("to create temporary directory");
        let temp_dir_path = temp_dir.path().to_str().unwrap().to_string();
        assert!(!validate_nonexistent_path(temp_dir_path.as_str()));
        temp_dir.close().expect("to close temporary directory");
    }
}
