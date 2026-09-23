// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "ABORT")]
    Abort,
    #[serde(rename = "SKIP")]
    Skip,
    #[serde(rename = "INFRA_FAIL")]
    InfraFail,
    #[serde(rename = "EXONERATED")]
    Exonerated,
}

impl TestStatus {
    /// Returns true if the status represents a failure condition.
    /// Note: This considers infra failures and aborts (timeouts) as failures,
    /// matching ResultDB ingestion requirements (only PASS and SKIP are non-failures).
    pub fn is_failure(&self) -> bool {
        !matches!(self, TestStatus::Pass | TestStatus::Skip)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureReasonError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureReason {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<FailureReasonError>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_errors_count: Option<i32>,
}

impl FailureReason {
    pub fn from_message(message: &str) -> Option<Self> {
        let trimmed_message = message.trim();
        if trimmed_message.is_empty() {
            None
        } else {
            Some(Self {
                errors: Some(vec![FailureReasonError {
                    message: Some(trimmed_message.to_string()),
                    trace: None,
                }]),
                truncated_errors_count: None,
            })
        }
    }
}

fn serialize_duration_nanos<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_u128(duration.as_nanos())
}

fn deserialize_duration_nanos<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let nanos = u128::deserialize(deserializer)?;
    let secs = (nanos / 1_000_000_000) as u64;
    let subsec_nanos = (nanos % 1_000_000_000) as u32;
    Ok(Duration::new(secs, subsec_nanos))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCaseResult {
    pub display_name: String,
    pub suite_name: String,
    pub case_name: String,
    pub status: TestStatus,
    #[serde(
        rename = "duration_nanos",
        serialize_with = "serialize_duration_nanos",
        deserialize_with = "deserialize_duration_nanos"
    )]
    pub duration: Duration,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<FailureReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub cases: Vec<TestCaseResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_serialization() {
        let case_result = TestCaseResult {
            display_name: "Suite.Case".to_string(),
            suite_name: "Suite".to_string(),
            case_name: "Case".to_string(),
            status: TestStatus::Pass,
            duration: Duration::from_millis(4),
            format: "GoogleTest".to_string(),
            failure_reason: None,
            output_files: None,
            output_dir: None,
        };

        let json_value = serde_json::to_value(&case_result).expect("serialization succeeds");
        assert_eq!(json_value["display_name"], "Suite.Case");
        assert_eq!(json_value["status"], "PASS");
        assert_eq!(json_value["duration_nanos"], 4_000_000);
        assert_eq!(json_value["format"], "GoogleTest");
    }
}
