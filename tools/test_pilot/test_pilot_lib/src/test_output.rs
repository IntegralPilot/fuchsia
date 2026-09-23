// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::BufReader;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::{fs, io};
use thiserror::Error;

/// Distinguished exit status used to indicate the test was run as requested but that the
/// test itself failed.
const FAIL_EXIT_STATUS: i32 = 86;

/// The body of the test output summary.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Summary {
    /// Properties concerning the overall test run.
    #[serde(flatten)]
    pub common: SummaryCommonProperties,

    /// Cases by case name.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub cases: HashMap<String, SummaryCase>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_succeeded: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub teardown_succeeded: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl Summary {
    /// Merges `Summary` read from a file into self. Returns Ok(true) if the file was merged
    /// successfully and `self` changed as a result, Ok(false) if the file did not exist or
    /// no change was made.
    pub fn maybe_merge_file(&mut self, path: &PathBuf) -> Result<bool, TestOutputError> {
        if let Ok(file) = fs::File::open(&path) {
            let mut reader = BufReader::new(file);
            let summary_from_file: Summary = serde_json::from_reader(&mut reader)
                .map_err(|e| TestOutputError::SummaryRead { path: path.clone(), source: e })?;
            Ok(self.merge(summary_from_file))
        } else {
            Ok(false)
        }
    }

    /// Writes this summary to a file.
    pub fn write(&self, path: &PathBuf) -> Result<(), TestOutputError> {
        fs::write(path, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| TestOutputError::SummaryWrite { path: path.clone(), source: e })?;

        Ok(())
    }

    /// Merges `other` into self. This is used for merging test and postprocessor summaries
    /// into the aggregate summary. Returns `true` if `self` changed as a result of the merge.
    fn merge(&mut self, other: Summary) -> bool {
        let mut changed = false;

        changed |= self.common.merge(other.common);
        for (other_case_name, other_case) in other.cases {
            match &mut self.cases.get_mut(other_case_name.as_str()) {
                Some(case_properties) => {
                    changed |= case_properties.merge(other_case);
                }
                None => {
                    self.cases.insert(other_case_name, other_case);
                    changed = true;
                }
            }
        }

        if other.setup_succeeded.is_some() && other.setup_succeeded != self.setup_succeeded {
            self.setup_succeeded = other.setup_succeeded;
            changed = true;
        }
        if other.teardown_succeeded.is_some() && other.teardown_succeeded != self.teardown_succeeded
        {
            self.teardown_succeeded = other.teardown_succeeded;
            changed = true;
        }
        if other.exit_code.is_some() && other.exit_code != self.exit_code {
            self.exit_code = other.exit_code;
            changed = true;
        }

        changed
    }
}

/// Test output regarding a single case.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct SummaryCase {
    #[serde(flatten)]
    pub common: SummaryCommonProperties,
}

impl SummaryCase {
    /// Merges `other` into self. This is used for merging test and postprocessor summaries
    /// into the aggregate summary. Returns `true` if `self` changed as a result of the merge.
    fn merge(&mut self, other: SummaryCase) -> bool {
        self.common.merge(other.common)
    }
}

/// Properties shared by the overall summary and by cases.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct SummaryCommonProperties {
    #[serde(default)]
    #[serde(skip_serializing_if = "is_zero")]
    pub duration: i64,

    pub outcome: SummaryOutcome,

    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub artifacts: HashMap<PathBuf, SummaryArtifact>,

    #[serde(flatten)]
    pub extension: HashMap<String, Value>,
}

impl SummaryCommonProperties {
    /// Merges `other` into self. This is used for merging test and postprocessor summaries
    /// into the aggregate summary. Returns `true` if `self` changed as a result of the merge.
    fn merge(&mut self, other: SummaryCommonProperties) -> bool {
        let mut changed = false;

        if other.duration != 0 && other.duration != self.duration {
            self.duration = other.duration;
            changed = true;
        }

        changed |= self.outcome.merge(other.outcome);

        for (other_artifact_name, other_artifact_properties) in other.artifacts {
            match &mut self.artifacts.get_mut(&other_artifact_name) {
                Some(artifact_properties) => {
                    changed |= artifact_properties.merge(other_artifact_properties);
                }
                None => {
                    self.artifacts.insert(other_artifact_name, other_artifact_properties);
                    changed = true;
                }
            }
        }

        for (other_extension_name, other_extension_properties) in other.extension {
            self.extension.insert(other_extension_name, other_extension_properties);
            changed = true;
        }

        changed
    }
}

/// Expresses the outcome of a test or test case run.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct SummaryOutcome {
    /// General result of the test run. The result of a test that has one
    /// or more cases should typically be the maximum of the results of
    /// the cases.
    #[serde(default)]
    #[serde(skip_serializing_if = "is_not_specified")]
    pub result: SummaryOutcomeResult,

    /// Optional details describing the outcome, used primarily to
    /// describe `Failed` and `Error` outcomes. The format of this field
    /// is not constrained by the ABI. The intent of this field is to offer
    /// a brief description of the problem for the purposes of triage,
    /// debugging, and classification of failures and errors.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SummaryOutcome {
    /// Merges `other` into self. This is used for merging test and postprocessor summaries
    /// into the aggregate summary. Returns `true` if `self` changed as a result of the merge.
    fn merge(&mut self, other: SummaryOutcome) -> bool {
        let mut changed = false;
        if other.result != SummaryOutcomeResult::NotSpecified && other.result != self.result {
            // If other has a specified result, the merge changes self to equal other.
            self.result = other.result;
            self.detail = other.detail;
            changed = true;
        } else if other.detail.is_some() && other.detail != self.detail {
            // Otherwise, if other has detail, the merge changes self's detail to match other's.
            self.detail = other.detail;
            changed = true;
        }

        changed
    }

    /// Merges a case outcome into a test outcome (self).
    pub fn merge_case_outcome(&mut self, case_outcome: SummaryOutcome) {
        if (self.result as u8) < (case_outcome.result as u8) {
            self.result = case_outcome.result;
            self.detail = case_outcome.detail;
        } else if self.result == case_outcome.result {
            if self.detail.is_none() {
                self.detail = case_outcome.detail;
            } else if case_outcome.detail.is_some() {
                let mut detail = self.detail.take().unwrap();
                detail.push('\n');
                detail.push_str(&case_outcome.detail.unwrap());
                self.detail = Some(detail);
            }
        }
    }
}

/// Summarizes the outcome of a test or test case run.
///
/// In general, the result of a test that has many cases will be the
/// maximum of the results of the cases. This constrains the ordering of
/// these values.
#[derive(Serialize, Deserialize, Debug, Default, Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum SummaryOutcomeResult {
    /// Default result value used when a test or postprocessor summary does
    /// not wish to specify an outcome result. This value must never appear
    /// in an aggregate summary.
    #[default]
    NotSpecified = 0x00,

    /// Regarding a case, the case was not fully executed, because the
    /// test configuration specified that it should be skipped or because
    /// the case explicitly requested that it be skipped. In the latter
    /// case, the detail associated with this result might give the reason
    /// that the case was skipped.
    ///
    /// Regarding a test, all cases in the test were skipped.
    #[serde(rename = "skipped")]
    Skipped = 0x10,

    /// Regarding a case, the case was executed as requested and passed.
    ///
    /// Regarding a test, at least one case passed, and all others passed
    /// or were skipped.
    #[serde(rename = "passed")]
    Passed = 0x20,

    /// Regarding a case, the case was not executed to completion as
    /// requested, because test execution was terminated early. This can
    /// occur if the client requests early termination or if the
    /// configuration specifies something like 'break on failure'.
    ///
    /// Regarding a test, at least one case was canceled, and all others
    /// either were canceled, passed or were skipped.
    #[serde(rename = "canceled")]
    Canceled = 0x30,

    /// Regarding a case, the case was executed as requested but did not
    /// terminate before the timeout interval for the case or overall test
    /// elapsed.
    ///
    /// Regarding a test, at least one case timed out, and all others either
    /// timed out, were canceled, passed, or were skipped. Alternatively,
    /// this value may be used if a timeout applied to the overall test
    /// run expired.
    #[serde(rename = "timed_out")]
    TimedOut = 0x40,

    /// Regarding a case, the case was executed as requested and failed.
    /// Failed outcomes typically include a non-empty `detail` field,
    /// formatted in a manner that is specific to the case. That format
    /// is not defined by the test ABI.
    ///
    /// Regarding a test, at least one case failed, and all others either
    /// failed, timed out, were canceled, passed, or were skipped. Failed
    /// test outcomes typically aggregate the details of all the failed
    /// cases.
    #[serde(rename = "failed")]
    Failed = 0x50,

    /// Regarding a case, the case was not executed as requested due to
    /// an error. This can occur due to resource or connectivity problems
    /// or due to a bug in the test framework implementation. The associated
    /// string is in a format that is specific to the test framework
    /// implementation and is not part of the test ABI.
    ///
    /// Regarding a test, at least one case resulted in an error result or
    /// an error that interfered with the execution of the test occurred
    /// outside the context of any given case. Typically, error results for
    /// a test are accompanied by a detail string indicating the nature of
    /// the error.
    #[serde(rename = "error")]
    Error = 0x60,
}

impl SummaryOutcomeResult {
    /// Determines whether this outcome represents a failure state.
    /// Returns true for outcomes that indicate a failed or incomplete run,
    /// specifically `Failed`, `TimedOut`, `Canceled`, or `Error`, false otherwise.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            SummaryOutcomeResult::Failed
                | SummaryOutcomeResult::TimedOut
                | SummaryOutcomeResult::Canceled
                | SummaryOutcomeResult::Error
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct SummaryArtifact {
    #[serde(rename = "type")]
    pub artifact_type: String,
}

impl SummaryArtifact {
    /// Merges `other` into self. This is used for merging test and postprocessor summaries
    /// into the aggregate summary. Returns `true` if `self` changed as a result of the merge.
    fn merge(&mut self, other: SummaryArtifact) -> bool {
        let mut changed = false;
        if !other.artifact_type.is_empty() && other.artifact_type != self.artifact_type {
            self.artifact_type = other.artifact_type;
            changed = true;
        }
        changed
    }
}

fn is_zero(to_test: &i64) -> bool {
    *to_test == 0
}

fn is_not_specified(to_test: &SummaryOutcomeResult) -> bool {
    *to_test == SummaryOutcomeResult::NotSpecified
}

const TEST_SUBDIR: &str = "test";
const EXECUTION_LOG_FILE_PATH: &str = "execution_log.json";
const TEST_CONFIG_FILE_PATH: &str = "test_config.json";
const OUTPUT_SUMMARY_FILE_PATH: &str = "output_summary.json";
const INVOCATION_STDOUT_FILE_PATH: &str = "invocation_stdout.txt";
const INVOCATION_STDERR_FILE_PATH: &str = "invocation_stderr.txt";

/// Creates path values based on output directory structure.
pub struct OutputDirectory<'a> {
    path: &'a Path,
}

impl<'a> OutputDirectory<'a> {
    /// Creates a new `OutputDirectory` located at `Path`.
    pub fn new(path: &'a Path) -> Self {
        Self { path }
    }

    /// Returns the full path of the specified file or subdirectory in the top-level output
    /// directory.
    pub fn join(&self, subdir: &str) -> PathBuf {
        self.path.join(subdir)
    }

    /// Returns the full path of the execution log.
    pub fn execution_log(&self) -> PathBuf {
        self.join(EXECUTION_LOG_FILE_PATH)
    }

    /// Returns the full path of the main summary.
    pub fn main_summary(&self) -> PathBuf {
        self.join(OUTPUT_SUMMARY_FILE_PATH)
    }

    /// Returns the full path of the test subdirectory, where the test deposits its summary
    /// and artifacts.
    pub fn test_subdir(&self) -> PathBuf {
        self.join(TEST_SUBDIR)
    }

    /// Returns the full path of the summary generated by the test.
    pub fn test_summary(&self) -> PathBuf {
        self.test_subdir().join(OUTPUT_SUMMARY_FILE_PATH)
    }

    /// Returns the full path of the test configuration.
    pub fn test_config(&self) -> PathBuf {
        self.join(TEST_CONFIG_FILE_PATH)
    }

    /// Returns the full path of the file to which test-pilot writes the test's stdout.
    pub fn test_invocation_stdout(&self) -> PathBuf {
        self.test_subdir().join(INVOCATION_STDOUT_FILE_PATH)
    }

    /// Returns the full path of the file to which test-pilot writes the test's stderr.
    pub fn test_invocation_stderr(&self) -> PathBuf {
        self.test_subdir().join(INVOCATION_STDERR_FILE_PATH)
    }

    /// Returns the full path of the subdirectory for the specified postprocessor.
    pub fn postprocessor_subdir(&self, binary: &PathBuf) -> PathBuf {
        self.join(postprocessor_name(binary))
    }

    /// Returns the full path of the summary for a specified postprocessor.
    pub fn postprocessor_summary(&self, binary: &PathBuf) -> PathBuf {
        self.postprocessor_subdir(binary).join(OUTPUT_SUMMARY_FILE_PATH)
    }

    /// Returns the full path of the file to which test-pilot writes a post-processor's stdout.
    pub fn postprocessor_invocation_stdout(&self, binary: &PathBuf) -> PathBuf {
        self.postprocessor_subdir(binary).join(INVOCATION_STDOUT_FILE_PATH)
    }

    /// Returns the full path of the file to which test-pilot writes a post-processor's stderr.
    pub fn postprocessor_invocation_stderr(&self, binary: &PathBuf) -> PathBuf {
        self.postprocessor_subdir(binary).join(INVOCATION_STDERR_FILE_PATH)
    }

    /// Converts a full path into a path relative to the output directory.
    pub fn make_relative(&self, path: &PathBuf) -> Option<PathBuf> {
        path.strip_prefix(self.path).ok().map(|p| p.to_path_buf())
    }
}

/// Returns the name of a postprocessor subdirectory given a path to the postprocessor's binary.
fn postprocessor_name<'b>(binary: &'b PathBuf) -> &'b str {
    binary.file_name().expect("binary path has file name").to_str().expect("file name is ansi")
}

/// Determines whether `exit_status` indicates the test run ran correctly, but the test itself
/// failed.
pub fn is_fail_exit_status(exit_status: std::process::ExitStatus) -> bool {
    exit_status.code() == Some(FAIL_EXIT_STATUS) || exit_status.into_raw() == FAIL_EXIT_STATUS
}

/// Returns an outcome string from an exit status.
pub fn outcome_from_exit_status(exit_status: std::process::ExitStatus) -> SummaryOutcome {
    let code = exit_status.code().unwrap_or_else(|| exit_status.into_raw());
    match code {
        0 => SummaryOutcome { result: SummaryOutcomeResult::Passed, detail: None },
        FAIL_EXIT_STATUS => SummaryOutcome { result: SummaryOutcomeResult::Failed, detail: None },
        status => SummaryOutcome {
            result: SummaryOutcomeResult::Error,
            detail: Some(format!("test binary failed with exit status {status}")),
        },
    }
}

/// Error encountered while executing test binary
#[derive(Debug, Error)]
pub enum TestOutputError {
    #[error("Error reading output summary file {path}: {source}")]
    SummaryRead {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Error writing output summary file {path}: {source}")]
    SummaryWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs::File;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use tempfile::tempdir;

    fn outcome(result: SummaryOutcomeResult, detail: &str) -> SummaryOutcome {
        SummaryOutcome { result, detail: Some(String::from(detail)) }
    }

    fn case(result: SummaryOutcomeResult, detail: &str) -> SummaryCase {
        SummaryCase {
            common: SummaryCommonProperties {
                outcome: outcome(result, detail),
                ..Default::default()
            },
        }
    }

    fn artifact(artifact_type: &str) -> SummaryArtifact {
        SummaryArtifact { artifact_type: String::from(artifact_type) }
    }

    #[fuchsia::test]
    fn test_merge_common_properties() {
        let mut under_test = SummaryCommonProperties {
            duration: 1,
            outcome: outcome(SummaryOutcomeResult::Failed, "test outcome a"),
            artifacts: [
                (PathBuf::from("a"), artifact("a_type")),
                (PathBuf::from("b"), artifact("b_type")),
            ]
            .into(),
            extension: [
                (String::from("x"), Value::from("x_value")),
                (String::from("y"), Value::from("y_value")),
            ]
            .into(),
        };

        // Non-trivial values from argument take precedent. New artifacts are inserted.
        // Existing artifacts with non-trivial values get updated.
        assert!(
            under_test.merge(SummaryCommonProperties {
                duration: 3,
                outcome: outcome(SummaryOutcomeResult::Failed, "test outcome b"),
                artifacts: [
                    (PathBuf::from("a"), artifact("new_a_type")),
                    (PathBuf::from("c"), artifact("c_type")),
                ]
                .into(),
                extension: [
                    (String::from("x"), Value::from("new_x_value")),
                    (String::from("z"), Value::from("z_value")),
                ]
                .into(),
            })
        );
        assert_eq!(
            under_test,
            SummaryCommonProperties {
                duration: 3,
                outcome: outcome(SummaryOutcomeResult::Failed, "test outcome b"),
                artifacts: [
                    (PathBuf::from("a"), artifact("new_a_type")),
                    (PathBuf::from("b"), artifact("b_type")),
                    (PathBuf::from("c"), artifact("c_type")),
                ]
                .into(),
                extension: [
                    (String::from("x"), Value::from("new_x_value")),
                    (String::from("y"), Value::from("y_value")),
                    (String::from("z"), Value::from("z_value")),
                ]
                .into(),
            }
        );

        // Trivial values from argument are not copied. For existing artifacts, trivial
        // values from argument are not copied.
        assert!(under_test.merge(SummaryCommonProperties {
            duration: 0,
            outcome: outcome(SummaryOutcomeResult::NotSpecified, "test outcome c"),
            artifacts: [(PathBuf::from("a"), artifact(""))].into(),
            extension: HashMap::new(),
        }));
        assert_eq!(
            under_test,
            SummaryCommonProperties {
                duration: 3,
                outcome: outcome(SummaryOutcomeResult::Failed, "test outcome c"),
                artifacts: [
                    (PathBuf::from("a"), artifact("new_a_type")),
                    (PathBuf::from("b"), artifact("b_type")),
                    (PathBuf::from("c"), artifact("c_type")),
                ]
                .into(),
                extension: [
                    (String::from("x"), Value::from("new_x_value")),
                    (String::from("y"), Value::from("y_value")),
                    (String::from("z"), Value::from("z_value")),
                ]
                .into(),
            }
        );

        // Merging with trivial or identical values does not change self and returns false.
        let before = under_test.clone();
        assert!(!under_test.merge(SummaryCommonProperties {
            duration: 0,
            outcome: outcome(SummaryOutcomeResult::NotSpecified, "test outcome c"),
            artifacts: [(PathBuf::from("a"), artifact(""))].into(),
            extension: HashMap::new(),
        }));
        assert_eq!(under_test, before);

        assert!(!under_test.merge(SummaryCommonProperties {
            duration: 3,
            outcome: SummaryOutcome::default(),
            artifacts: [(PathBuf::from("a"), artifact("new_a_type"))].into(),
            extension: HashMap::new(),
        }));
        assert_eq!(under_test, before);
    }

    #[fuchsia::test]
    fn test_merge_cases() {
        let mut under_test = Summary {
            common: SummaryCommonProperties::default(),
            cases: [
                (String::from("case_a"), case(SummaryOutcomeResult::Failed, "case_a_outcome")),
                (String::from("case_b"), case(SummaryOutcomeResult::Failed, "case_b_outcome")),
            ]
            .into(),
            ..Default::default()
        };

        // Non-trivial values from argument take precedent. New artifacts are inserted.
        // Existing artifacts with non-trivial values get updated.
        assert!(
            under_test.merge(Summary {
                common: SummaryCommonProperties::default(),
                cases: [
                    (
                        String::from("case_a"),
                        case(SummaryOutcomeResult::Failed, "case_a_new_outcome")
                    ),
                    (String::from("case_c"), case(SummaryOutcomeResult::Failed, "case_c_outcome")),
                ]
                .into(),
                ..Default::default()
            })
        );
        let expected = Summary {
            common: SummaryCommonProperties::default(),
            cases: [
                (String::from("case_a"), case(SummaryOutcomeResult::Failed, "case_a_new_outcome")),
                (String::from("case_b"), case(SummaryOutcomeResult::Failed, "case_b_outcome")),
                (String::from("case_c"), case(SummaryOutcomeResult::Failed, "case_c_outcome")),
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(under_test, expected);

        // Merging unchanged cases returns false and leaves self unchanged.
        assert!(
            !under_test.merge(Summary {
                common: SummaryCommonProperties::default(),
                cases: [(
                    String::from("case_a"),
                    case(SummaryOutcomeResult::Failed, "case_a_new_outcome")
                ),]
                .into(),
                ..Default::default()
            })
        );
        assert_eq!(under_test, expected);
    }

    #[fuchsia::test]
    fn test_merge_lifecycle_fields() {
        // When self fields are None and other has Some, other's fields take precedent.
        let mut under_test = Summary {
            setup_succeeded: None,
            teardown_succeeded: None,
            exit_code: None,
            ..Default::default()
        };
        assert!(under_test.merge(Summary {
            setup_succeeded: Some(true),
            teardown_succeeded: Some(false),
            exit_code: Some(0),
            ..Default::default()
        }));
        assert_eq!(under_test.setup_succeeded, Some(true));
        assert_eq!(under_test.teardown_succeeded, Some(false));
        assert_eq!(under_test.exit_code, Some(0));

        // When other has None, self fields are preserved.
        assert!(!under_test.merge(Summary {
            setup_succeeded: None,
            teardown_succeeded: None,
            exit_code: None,
            ..Default::default()
        }));
        assert_eq!(under_test.setup_succeeded, Some(true));
        assert_eq!(under_test.teardown_succeeded, Some(false));
        assert_eq!(under_test.exit_code, Some(0));

        // When other has Some with identical values, self fields are preserved and merge returns false.
        assert!(!under_test.merge(Summary {
            setup_succeeded: Some(true),
            teardown_succeeded: Some(false),
            exit_code: Some(0),
            ..Default::default()
        }));
        assert_eq!(under_test.setup_succeeded, Some(true));
        assert_eq!(under_test.teardown_succeeded, Some(false));
        assert_eq!(under_test.exit_code, Some(0));

        // When other has Some, it overwrites existing self fields.
        assert!(under_test.merge(Summary {
            setup_succeeded: Some(false),
            teardown_succeeded: Some(true),
            exit_code: Some(86),
            ..Default::default()
        }));
        assert_eq!(under_test.setup_succeeded, Some(false));
        assert_eq!(under_test.teardown_succeeded, Some(true));
        assert_eq!(under_test.exit_code, Some(86));
    }

    #[fuchsia::test]
    fn test_merge_outcomes() {
        let results = [
            SummaryOutcomeResult::NotSpecified,
            SummaryOutcomeResult::Skipped,
            SummaryOutcomeResult::Passed,
            SummaryOutcomeResult::Canceled,
            SummaryOutcomeResult::TimedOut,
            SummaryOutcomeResult::Failed,
            SummaryOutcomeResult::Error,
        ];
        let specified_results = [
            SummaryOutcomeResult::Skipped,
            SummaryOutcomeResult::Passed,
            SummaryOutcomeResult::Canceled,
            SummaryOutcomeResult::TimedOut,
            SummaryOutcomeResult::Failed,
            SummaryOutcomeResult::Error,
        ];

        // When other is unspecified and detail is none, merging leaves self unchanged.
        for result in &results {
            let mut under_test = outcome(result.clone(), "self");
            assert!(!under_test.merge(SummaryOutcome {
                result: SummaryOutcomeResult::NotSpecified,
                detail: None
            }));
            assert_eq!(under_test, outcome(result.clone(), "self"));
        }

        // When other's result is specified:
        for self_result in &results {
            for other_result in &specified_results {
                // If other has a different result, or same result with a different detail,
                // merging changes self to equal other.
                let mut under_test = outcome(self_result.clone(), "self");
                let other = outcome(other_result.clone(), "other");
                assert!(under_test.merge(other.clone()));
                assert_eq!(under_test, other);

                // If other's result is different, self takes other's result and None detail.
                // If other's result is the same and other's detail is None, self is unchanged.
                let mut under_test = outcome(self_result.clone(), "self");
                let other = SummaryOutcome { result: other_result.clone(), detail: None };
                if self_result != other_result {
                    assert!(under_test.merge(other.clone()));
                    assert_eq!(under_test, other);
                } else {
                    assert!(!under_test.merge(other));
                    assert_eq!(under_test, outcome(self_result.clone(), "self"));
                }
            }
        }

        // When other's result and detail are identical, merging leaves self unchanged.
        for result in &specified_results {
            let mut under_test = outcome(result.clone(), "self");
            let other = outcome(result.clone(), "self");
            assert!(!under_test.merge(other));
            assert_eq!(under_test, outcome(result.clone(), "self"));
        }

        // When other's result is unspecified:
        for result in &results {
            // Different detail changes self's detail to other's.
            let mut under_test = outcome(result.clone(), "self");
            assert!(under_test.merge(outcome(SummaryOutcomeResult::NotSpecified, "other")));
            assert_eq!(under_test, outcome(result.clone(), "other"));

            // Same detail leaves self unchanged and returns false.
            let mut under_test = outcome(result.clone(), "self");
            assert!(!under_test.merge(outcome(SummaryOutcomeResult::NotSpecified, "self")));
            assert_eq!(under_test, outcome(result.clone(), "self"));
        }
    }

    #[fuchsia::test]
    fn test_merge_case_outcomes() {
        let results = [
            SummaryOutcomeResult::NotSpecified,
            SummaryOutcomeResult::Skipped,
            SummaryOutcomeResult::Passed,
            SummaryOutcomeResult::Canceled,
            SummaryOutcomeResult::TimedOut,
            SummaryOutcomeResult::Failed,
            SummaryOutcomeResult::Error,
        ];

        // Merging an outcome with a greater result copies the greater result.
        let selves_iter = &mut results.into_iter();
        while let Some(self_result) = selves_iter.next() {
            let mut others_iter = selves_iter.clone();
            while let Some(other_result) = others_iter.next() {
                let mut self_outcome = SummaryOutcome {
                    result: self_result,
                    detail: Some(String::from("self detail")),
                };
                let other_outcome = SummaryOutcome {
                    result: other_result,
                    detail: Some(String::from("other detail")),
                };
                self_outcome.merge_case_outcome(other_outcome.clone());
                assert_eq!(other_outcome, self_outcome);
            }
        }

        // Merging an outcome with a lesser result has no effect.
        let others_iter = &mut results.into_iter();
        while let Some(other_result) = others_iter.next() {
            let mut selves_iter = others_iter.clone();
            while let Some(self_result) = selves_iter.next() {
                let mut self_outcome = SummaryOutcome {
                    result: self_result,
                    detail: Some(String::from("self detail")),
                };
                let other_outcome = SummaryOutcome {
                    result: other_result,
                    detail: Some(String::from("other detail")),
                };
                self_outcome.merge_case_outcome(other_outcome.clone());
                assert_eq!(
                    SummaryOutcome {
                        result: self_result,
                        detail: Some(String::from("self detail"))
                    },
                    self_outcome
                );
            }
        }

        // Merging an outcome with an equal result has no effect on the result and merges the
        // detail.
        for result in results {
            // If both outcomes have no detail, the merge will have no detail.
            let mut self_outcome = SummaryOutcome { result: result, detail: None };
            let other_outcome = SummaryOutcome { result: result, detail: None };
            self_outcome.merge_case_outcome(other_outcome.clone());
            assert_eq!(self_outcome, SummaryOutcome { result: result, detail: None });

            // If both outcomes have detail, the detail is concatenated with a newline separator.
            let mut self_outcome =
                SummaryOutcome { result: result, detail: Some(String::from("self detail")) };
            let other_outcome =
                SummaryOutcome { result: result, detail: Some(String::from("other detail")) };
            self_outcome.merge_case_outcome(other_outcome.clone());
            assert_eq!(
                self_outcome,
                SummaryOutcome {
                    result: result,
                    detail: Some(String::from("self detail\nother detail"))
                }
            );

            // If only the self outcome has detail, that detail is used.
            let mut self_outcome =
                SummaryOutcome { result: result, detail: Some(String::from("self detail")) };
            let other_outcome = SummaryOutcome { result: result, detail: None };
            self_outcome.merge_case_outcome(other_outcome.clone());
            assert_eq!(
                self_outcome,
                SummaryOutcome { result: result, detail: Some(String::from("self detail")) }
            );

            // If only the other outcome has detail, that detail is used.
            let mut self_outcome = SummaryOutcome { result: result, detail: None };
            let other_outcome =
                SummaryOutcome { result: result, detail: Some(String::from("other detail")) };
            self_outcome.merge_case_outcome(other_outcome.clone());
            assert_eq!(
                self_outcome,
                SummaryOutcome { result: result, detail: Some(String::from("other detail")) }
            );
        }
    }

    #[fuchsia::test]
    fn test_outcome_from_exit_status() {
        assert_eq!(
            outcome_from_exit_status(ExitStatus::from_raw(0)),
            SummaryOutcome { result: SummaryOutcomeResult::Passed, detail: None }
        );
        assert_eq!(
            outcome_from_exit_status(ExitStatus::from_raw(FAIL_EXIT_STATUS)),
            SummaryOutcome { result: SummaryOutcomeResult::Failed, detail: None }
        );
        assert_eq!(
            outcome_from_exit_status(ExitStatus::from_raw(1)),
            SummaryOutcome {
                result: SummaryOutcomeResult::Error,
                detail: Some(String::from("test binary failed with exit status 1"))
            }
        );
    }

    #[fuchsia::test]
    fn test_summary_write() {
        let under_test = Summary {
            common: SummaryCommonProperties {
                duration: 3,
                outcome: outcome(SummaryOutcomeResult::Failed, "test outcome b"),
                artifacts: [
                    (PathBuf::from("a"), artifact("a_type")),
                    (PathBuf::from("b"), artifact("b_type")),
                    (PathBuf::from("c"), artifact("c_type")),
                ]
                .into(),
                extension: [
                    (String::from("x"), Value::from("x_value")),
                    (String::from("y"), Value::from("y_value")),
                    (String::from("z"), Value::from("z_value")),
                ]
                .into(),
            },
            cases: [
                (String::from("case_a"), case(SummaryOutcomeResult::Failed, "case_a_outcome")),
                (String::from("case_b"), case(SummaryOutcomeResult::Failed, "case_b_outcome")),
            ]
            .into(),
            ..Default::default()
        };

        let temp_dir = tempdir().expect("to create temporary directory");
        let file_path = temp_dir.path().join("test_output_summary.json");

        under_test.write(&file_path).expect("to write summary");

        let file = File::open(&file_path).expect("to open summary");
        let mut reader = BufReader::new(file);
        let file_contents: Value = serde_json5::from_reader(&mut reader).expect("to read summary");
        assert_eq!(
            file_contents,
            json!({
                "artifacts": {
                    "a": {"type": "a_type"},
                    "b": {"type": "b_type"},
                    "c": {"type": "c_type"}
                },
                "cases": {
                    "case_a": {"outcome": {"result": "failed", "detail": "case_a_outcome"}},
                    "case_b": {"outcome": {"result": "failed", "detail": "case_b_outcome"}}
                },
                "duration": 3,
                "outcome": {"result": "failed", "detail": "test outcome b"},
                "x": "x_value",
                "y": "y_value",
                "z": "z_value"
            })
        );

        temp_dir.close().expect("to close temporary directory");
    }

    #[fuchsia::test]
    fn test_summary_maybe_merge_file() {
        let temp_dir = tempdir().expect("to create temporary directory");
        let mut under_test = Summary::default();

        assert_eq!(
            false,
            under_test
                .maybe_merge_file(&temp_dir.path().join("does_not_exist.json"))
                .expect("success"),
        );

        assert_eq!(Summary::default(), under_test);

        let file_path = temp_dir.path().join("test_output_summary.json");
        fs::write(
            &file_path,
            serde_json::to_string_pretty(&json!({
                "artifacts": {
                    "a": {"type": "a_type"},
                    "b": {"type": "b_type"},
                    "c": {"type": "c_type"}
                },
                "cases": {
                    "case_a": {"outcome": {"result": "failed", "detail": "case_a_outcome"}},
                    "case_b": {"outcome": {"result": "failed", "detail": "case_b_outcome"}}
                },
                "duration": 3,
                "outcome": {"result": "failed", "detail": "test outcome b"},
                "x": "x_value",
                "y": "y_value",
                "z": "z_value"
            }))
            .unwrap(),
        )
        .expect("json write succeeds");

        assert_eq!(true, under_test.maybe_merge_file(&file_path).expect("success"),);

        assert_eq!(
            Summary {
                common: SummaryCommonProperties {
                    duration: 3,
                    outcome: outcome(SummaryOutcomeResult::Failed, "test outcome b"),
                    artifacts: [
                        (PathBuf::from("a"), artifact("a_type")),
                        (PathBuf::from("b"), artifact("b_type")),
                        (PathBuf::from("c"), artifact("c_type")),
                    ]
                    .into(),
                    extension: [
                        (String::from("x"), Value::from("x_value")),
                        (String::from("y"), Value::from("y_value")),
                        (String::from("z"), Value::from("z_value")),
                    ]
                    .into(),
                },
                cases: [
                    (String::from("case_a"), case(SummaryOutcomeResult::Failed, "case_a_outcome")),
                    (String::from("case_b"), case(SummaryOutcomeResult::Failed, "case_b_outcome")),
                ]
                .into(),
                ..Default::default()
            },
            under_test
        );

        // Merging a file with no new information returns Ok(false).
        let no_change_file_path = temp_dir.path().join("no_change_summary.json");
        fs::write(
            &no_change_file_path,
            serde_json::to_string_pretty(&json!({
                "duration": 3,
                "outcome": {"result": "failed", "detail": "test outcome b"},
            }))
            .unwrap(),
        )
        .expect("json write succeeds");

        assert_eq!(false, under_test.maybe_merge_file(&no_change_file_path).expect("success"));

        temp_dir.close().expect("to close temporary directory");
    }

    #[fuchsia::test]
    fn test_summary_maybe_merge_file_with_lifecycle_fields() {
        let temp_dir = tempdir().expect("to create temporary directory");
        let file_path = temp_dir.path().join("test_output_summary.json");
        fs::write(
            &file_path,
            serde_json::to_string_pretty(&json!({
                "outcome": {},
                "setup_succeeded": false,
                "teardown_succeeded": true,
                "exit_code": 1,
            }))
            .unwrap(),
        )
        .expect("json write succeeds");

        let mut under_test = Summary::default();
        assert!(under_test.maybe_merge_file(&file_path).expect("success"));
        assert_eq!(under_test.setup_succeeded, Some(false));
        assert_eq!(under_test.teardown_succeeded, Some(true));
        assert_eq!(under_test.exit_code, Some(1));

        // Merging the same file again produces no changes and returns false.
        assert!(!under_test.maybe_merge_file(&file_path).expect("success"));

        temp_dir.close().expect("to close temporary directory");
    }

    #[fuchsia::test]
    fn test_merge_artifact() {
        let mut under_test = artifact("old_type");

        // Empty type does not change artifact and returns false.
        assert!(!under_test.merge(artifact("")));
        assert_eq!(under_test, artifact("old_type"));

        // Same type does not change artifact and returns false.
        assert!(!under_test.merge(artifact("old_type")));
        assert_eq!(under_test, artifact("old_type"));

        // Different non-empty type updates artifact and returns true.
        assert!(under_test.merge(artifact("new_type")));
        assert_eq!(under_test, artifact("new_type"));
    }

    #[test]
    fn test_is_failure() {
        assert!(!SummaryOutcomeResult::NotSpecified.is_failure());
        assert!(!SummaryOutcomeResult::Skipped.is_failure());
        assert!(!SummaryOutcomeResult::Passed.is_failure());
        assert!(SummaryOutcomeResult::Canceled.is_failure());
        assert!(SummaryOutcomeResult::TimedOut.is_failure());
        assert!(SummaryOutcomeResult::Failed.is_failure());
        assert!(SummaryOutcomeResult::Error.is_failure());
    }
}
