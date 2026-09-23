// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::borrow::Cow;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::schema::{FailureReason, TestCaseResult, TestStatus};

static ANSI_ESCAPE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());

/// Matches log timestamp prefixes in square brackets to strip them before parsing.
/// Supported formats include:
/// - Elapsed seconds with decimal precision (e.g. `[ 12.345 ]`, `[00012.100]`)
/// - Clock time (e.g. `[ 12:34:56.789 ]`, `[12:34:56]`)
/// - ISO 8601 timestamps (e.g. `[ 2026-08-28T12:34:56Z ]`, `[2026-08-28 12:34:56.789]`)
static TIMESTAMP_PREFIX_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*\[\s*(?:\d+\.\d+|\d{1,2}:\d{2}(?::\d{2})?(?:\.\d+)?|\d{4}-\d{2}-\d{2}[T\s]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)\s*\]\s*",
    )
    .unwrap()
});

static GTEST_RUN_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[\s*RUN\s*\]\s*(\S+?)\.(\S+)$").unwrap());

static GTEST_COMPLETION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[\s*(OK|FAILED|SKIPPED|DISABLED)\s*\]\s*(\S+?)\.(\S+?)(?:\s*\((\d+)\s*ms\))?$")
        .unwrap()
});

const LEGACY_TEST_STDOUT_PREFIX: &str = "[stdout - legacy_test]";
const GTEST_FORMAT_NAME: &str = "GoogleTest";

fn sanitize_line<'a>(raw_line: &'a str) -> Cow<'a, str> {
    let after_ansi = ANSI_ESCAPE_PATTERN.replace_all(raw_line, "");
    match TIMESTAMP_PREFIX_PATTERN.replace_all(&after_ansi, "") {
        Cow::Borrowed(_) => after_ansi,
        Cow::Owned(s) => Cow::Owned(s),
    }
}

/// Parses stdout from a GoogleTest program (or serial stream) and returns structured results.
pub fn parse(stdout: &[u8]) -> Vec<TestCaseResult> {
    parse_str(&String::from_utf8_lossy(stdout))
}

/// Parses stdout string from a GoogleTest program and returns structured results.
pub fn parse_str(stdout: &str) -> Vec<TestCaseResult> {
    let mut results = Vec::new();
    let mut current_suite = String::new();
    let mut current_case = String::new();
    let mut output_for_case = Vec::new();

    let flush_unfinished_test = |results: &mut Vec<TestCaseResult>,
                                 suite: &mut String,
                                 case: &mut String,
                                 output: &mut Vec<String>| {
        if !case.is_empty() {
            let display_name = format!("{suite}.{case}");
            let failure_reason = if !output.is_empty() {
                FailureReason::from_message(&output.join("\n"))
            } else {
                None
            };
            results.push(TestCaseResult {
                display_name,
                suite_name: std::mem::take(suite),
                case_name: std::mem::take(case),
                status: TestStatus::Fail,
                duration: Duration::ZERO,
                format: GTEST_FORMAT_NAME.to_string(),
                failure_reason,
                output_files: None,
                output_dir: None,
            });
            output.clear();
        }
    };

    for raw_line in stdout.lines() {
        let line = sanitize_line(raw_line);
        let trimmed_line = line.trim();

        if trimmed_line.starts_with('[') {
            if let Some(captures) = GTEST_RUN_PATTERN.captures(trimmed_line) {
                flush_unfinished_test(
                    &mut results,
                    &mut current_suite,
                    &mut current_case,
                    &mut output_for_case,
                );
                current_suite = captures[1].to_string();
                current_case = captures[2].to_string();
                output_for_case.clear();
                continue;
            }

            if let Some(captures) = GTEST_COMPLETION_PATTERN.captures(trimmed_line) {
                let duration_match = captures.get(4);
                let has_duration = duration_match.is_some();
                let is_disabled = &captures[1] == "DISABLED";

                if current_case.is_empty() && !has_duration && !is_disabled {
                    continue;
                }

                if is_disabled {
                    flush_unfinished_test(
                        &mut results,
                        &mut current_suite,
                        &mut current_case,
                        &mut output_for_case,
                    );
                }

                let status = match &captures[1] {
                    "OK" => TestStatus::Pass,
                    "FAILED" => TestStatus::Fail,
                    "SKIPPED" | "DISABLED" => TestStatus::Skip,
                    _ => TestStatus::Fail,
                };

                let suite_name = captures[2].to_string();
                let case_name = captures[3].to_string();
                let display_name = format!("{suite_name}.{case_name}");

                let duration = duration_match
                    .and_then(|m| m.as_str().parse::<u64>().ok())
                    .map(Duration::from_millis)
                    .unwrap_or(Duration::ZERO);

                let failure_reason = if status.is_failure() && !output_for_case.is_empty() {
                    FailureReason::from_message(&output_for_case.join("\n"))
                } else {
                    None
                };

                results.push(TestCaseResult {
                    display_name,
                    suite_name,
                    case_name,
                    status,
                    duration,
                    format: GTEST_FORMAT_NAME.to_string(),
                    failure_reason,
                    output_files: None,
                    output_dir: None,
                });

                current_suite.clear();
                current_case.clear();
                output_for_case.clear();
                continue;
            }
        }

        if !current_case.is_empty() && !trimmed_line.starts_with(LEGACY_TEST_STDOUT_PREFIX) {
            output_for_case.push(line.into_owned());
        }
    }

    flush_unfinished_test(
        &mut results,
        &mut current_suite,
        &mut current_case,
        &mut output_for_case,
    );

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_parsed_cases(stdout: &str, expected: Vec<TestCaseResult>) {
        let results = parse(stdout.as_bytes());
        assert_eq!(results, expected);
    }

    #[test]
    fn test_parse_empty() {
        assert_parsed_cases("", vec![]);
    }

    #[test]
    fn test_parse_no_test_cases() {
        assert_parsed_cases("non-test output without any markers", vec![]);
    }

    #[test]
    fn test_parse_google_test() {
        let stdout = r#"
Sometimes there is weird stuff in stdout.
[==========] Running 4 tests from 1 test suite.
[----------] Global test environment set-up.
[----------] 4 tests from ChannelTest
[ RUN      ] ChannelTest.ReadWrite
[       OK ] ChannelTest.ReadWrite (4 ms)
[ RUN      ] ChannelTest.InvalidHandle
../../src/zircon/tests/channel_test.cc:45: Failure
Expected equality of these values:
  status
    Which is: ZX_ERR_BAD_HANDLE (-11)
  ZX_OK
    Which is: 0
[  FAILED  ] ChannelTest.InvalidHandle (5 ms)
[ RUN      ] ChannelTest.InterleavedLogs
Sometimes tests print to stdout.
Their prints get interleaved with the results.
[       OK ] ChannelTest.InterleavedLogs (3 ms)
[ RUN      ] ChannelTest.SkipFeature
[  SKIPPED ] ChannelTest.SkipFeature (1 ms)
[----------] 4 tests from ChannelTest (13 ms total)
[----------] Global test environment tear-down
[==========] 4 tests from 1 test suite ran. (15 ms total)
[  PASSED  ] 2 tests.
[  SKIPPED ] 1 test.
[  FAILED  ] 1 test.
"#;

        let expected = vec![
            TestCaseResult {
                display_name: "ChannelTest.ReadWrite".to_string(),
                suite_name: "ChannelTest".to_string(),
                case_name: "ReadWrite".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(4),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "ChannelTest.InvalidHandle".to_string(),
                suite_name: "ChannelTest".to_string(),
                case_name: "InvalidHandle".to_string(),
                status: TestStatus::Fail,
                duration: Duration::from_millis(5),
                format: "GoogleTest".to_string(),
                failure_reason: FailureReason::from_message(
                    "../../src/zircon/tests/channel_test.cc:45: Failure\nExpected equality of these values:\n  status\n    Which is: ZX_ERR_BAD_HANDLE (-11)\n  ZX_OK\n    Which is: 0",
                ),
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "ChannelTest.InterleavedLogs".to_string(),
                suite_name: "ChannelTest".to_string(),
                case_name: "InterleavedLogs".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(3),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "ChannelTest.SkipFeature".to_string(),
                suite_name: "ChannelTest".to_string(),
                case_name: "SkipFeature".to_string(),
                status: TestStatus::Skip,
                duration: Duration::from_millis(1),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
        ];

        assert_parsed_cases(stdout, expected);
    }

    #[test]
    fn test_parse_serial_google_test() {
        let stdout = "\n[00012.100] [==========] Running 2 tests from 1 test suite.\n\
[00012.105] [----------] Global test environment set-up.\n\
[00012.110] [----------] 2 tests from SerialTest\n\
[00012.120] \x1b[0;32m[ RUN      ]\x1b[m SerialTest.PassTest\n\
[00012.125] \x1b[0;32m[       OK ]\x1b[m SerialTest.PassTest (5 ms)\n\
[00012.130] \x1b[0;31m[ RUN      ]\x1b[m SerialTest.FailTest\n\
[00012.135] assertion failed: expected 42 but got 0\n\
[00012.140] \x1b[0;31m[  FAILED  ]\x1b[m SerialTest.FailTest (10 ms)\n\
[00012.145] [----------] 2 tests from SerialTest (15 ms total)\n\
[00012.150] [==========] 2 tests ran.\n";

        let expected = vec![
            TestCaseResult {
                display_name: "SerialTest.PassTest".to_string(),
                suite_name: "SerialTest".to_string(),
                case_name: "PassTest".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(5),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "SerialTest.FailTest".to_string(),
                suite_name: "SerialTest".to_string(),
                case_name: "FailTest".to_string(),
                status: TestStatus::Fail,
                duration: Duration::from_millis(10),
                format: "GoogleTest".to_string(),
                failure_reason: FailureReason::from_message(
                    "assertion failed: expected 42 but got 0",
                ),
                output_files: None,
                output_dir: None,
            },
        ];

        assert_parsed_cases(stdout, expected);
    }

    #[test]
    fn test_parse_disabled_google_test() {
        let stdout = r#"
[==========] Running 3 tests from 1 test suite.
[----------] 3 tests from DisabledSuite
[ DISABLED ] DisabledSuite.FeatureOff
[ RUN      ] DisabledSuite.FeatureActive
[       OK ] DisabledSuite.FeatureActive (8 ms)
[ DISABLED ] DisabledSuite.AnotherOff
[----------] 3 tests from DisabledSuite (8 ms total)
"#;

        let expected = vec![
            TestCaseResult {
                display_name: "DisabledSuite.FeatureOff".to_string(),
                suite_name: "DisabledSuite".to_string(),
                case_name: "FeatureOff".to_string(),
                status: TestStatus::Skip,
                duration: Duration::ZERO,
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "DisabledSuite.FeatureActive".to_string(),
                suite_name: "DisabledSuite".to_string(),
                case_name: "FeatureActive".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(8),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "DisabledSuite.AnotherOff".to_string(),
                suite_name: "DisabledSuite".to_string(),
                case_name: "AnotherOff".to_string(),
                status: TestStatus::Skip,
                duration: Duration::ZERO,
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
        ];

        assert_parsed_cases(stdout, expected);
    }

    #[test]
    fn test_parse_flexible_timestamps() {
        let stdout = r#"
[2026-08-28 03:47:19.123] [==========] Running 2 tests from 1 test suite.
[2026-08-28 03:47:19.124] [ RUN      ] TimeSuite.IsoTest
[2026-08-28 03:47:19.129] [       OK ] TimeSuite.IsoTest (5 ms)
[12:34:56.789] [ RUN      ] TimeSuite.ClockTest
[12:34:56.790] [       OK ] TimeSuite.ClockTest (1 ms)
"#;

        let expected = vec![
            TestCaseResult {
                display_name: "TimeSuite.IsoTest".to_string(),
                suite_name: "TimeSuite".to_string(),
                case_name: "IsoTest".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(5),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "TimeSuite.ClockTest".to_string(),
                suite_name: "TimeSuite".to_string(),
                case_name: "ClockTest".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(1),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
        ];

        assert_parsed_cases(stdout, expected);
    }

    #[test]
    fn test_parse_parameterized_google_test() {
        let stdout = r#"
[==========] Running 2 tests from 1 test suite.
[----------] 2 tests from SocketSuite/ParamTest
[ RUN      ] SocketSuite/ParamTest.DataTransfer/0
[       OK ] SocketSuite/ParamTest.DataTransfer/0 (12 ms)
[ RUN      ] SocketSuite/ParamTest.DataTransfer/1
[       OK ] SocketSuite/ParamTest.DataTransfer/1 (11 ms)
[----------] 2 tests from SocketSuite/ParamTest (23 ms total)
"#;

        let expected = vec![
            TestCaseResult {
                display_name: "SocketSuite/ParamTest.DataTransfer/0".to_string(),
                suite_name: "SocketSuite/ParamTest".to_string(),
                case_name: "DataTransfer/0".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(12),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "SocketSuite/ParamTest.DataTransfer/1".to_string(),
                suite_name: "SocketSuite/ParamTest".to_string(),
                case_name: "DataTransfer/1".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(11),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
        ];

        assert_parsed_cases(stdout, expected);
    }

    #[test]
    fn test_parse_crash_google_test() {
        let stdout = r#"
[==========] Running 2 tests from 1 test suite.
[----------] 2 tests from CrashSuite
[ RUN      ] CrashSuite.TestPass
[       OK ] CrashSuite.TestPass (2 ms)
[ RUN      ] CrashSuite.TestCrash
kernel panic: segmentation fault at 0xdeadbeef
{{{bt:0:0x123456}}}
"#;

        let expected = vec![
            TestCaseResult {
                display_name: "CrashSuite.TestPass".to_string(),
                suite_name: "CrashSuite".to_string(),
                case_name: "TestPass".to_string(),
                status: TestStatus::Pass,
                duration: Duration::from_millis(2),
                format: "GoogleTest".to_string(),
                failure_reason: None,
                output_files: None,
                output_dir: None,
            },
            TestCaseResult {
                display_name: "CrashSuite.TestCrash".to_string(),
                suite_name: "CrashSuite".to_string(),
                case_name: "TestCrash".to_string(),
                status: TestStatus::Fail,
                duration: Duration::ZERO,
                format: "GoogleTest".to_string(),
                failure_reason: FailureReason::from_message(
                    "kernel panic: segmentation fault at 0xdeadbeef\n{{{bt:0:0x123456}}}",
                ),
                output_files: None,
                output_dir: None,
            },
        ];

        assert_parsed_cases(stdout, expected);
    }

    #[test]
    fn test_parse_bracketed_non_timestamp_logs() {
        let stdout = r#"
[==========] Running 1 test from 1 test suite.
[----------] 1 test from BracketSuite
[ RUN      ] BracketSuite.BracketTest
[0] array element value
[1/5] progress step
[42] answer
[       OK ] BracketSuite.BracketTest (3 ms)
[----------] 1 test from BracketSuite (3 ms total)
"#;

        let expected = vec![TestCaseResult {
            display_name: "BracketSuite.BracketTest".to_string(),
            suite_name: "BracketSuite".to_string(),
            case_name: "BracketTest".to_string(),
            status: TestStatus::Pass,
            duration: Duration::from_millis(3),
            format: "GoogleTest".to_string(),
            failure_reason: None,
            output_files: None,
            output_dir: None,
        }];

        assert_parsed_cases(stdout, expected);
    }
}
