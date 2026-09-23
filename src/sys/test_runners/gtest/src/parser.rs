// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_io as fio;
use serde::{Deserialize, Serialize};
use test_runners_lib::cases::TestCaseInfo;
use test_runners_lib::errors::*;

/// In `gtest_list_test` output, provides info about individual test cases.
/// Example: For test FOO.Bar, this contains info about Bar.
/// Please refer to documentation of `ListTestResult` for details.
#[derive(Serialize, Deserialize, Debug)]
struct IndividualTestInfo {
    pub name: String,
    pub file: String,
    pub line: u64,
}

/// In `gtest_list_test` output, provides info about individual test suites.
/// Example: For test FOO.Bar, this contains info about FOO.
/// Please refer to documentation of `ListTestResult` for details.
#[derive(Serialize, Deserialize, Debug)]
struct TestSuiteResult {
    pub tests: usize,
    pub name: String,
    pub testsuite: Vec<IndividualTestInfo>,
}

/// Structure of the output of `<test binary> --gtest_list_test`.
///
/// Sample json will look like
/// ```
/// {
/// "tests": 6,
/// "name": "AllTests",
/// "testsuites": [
///    {
///      "name": "SampleTest1",
///      "tests": 2,
///      "testsuite": [
///        {
///          "name": "Test1",
///          "file": "../../src/sys/test_runners/gtest/test_data/sample_tests.cc",
///          "line": 7
///        },
///        {
///          "name": "Test2",
///          "file": "../../src/sys/test_runners/gtest/test_data/sample_tests.cc",
///          "line": 9
///        }
///      ]
///    },
///  ]
///}
///```
#[derive(Serialize, Deserialize, Debug)]
struct ListTestResult {
    pub tests: usize,
    pub name: String,
    pub testsuites: Vec<TestSuiteResult>,
}

/// Provides info about test case failure if any.
#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
pub struct Failure {
    pub failure: String,
}

/// Provides info about individual test executions.
/// Example: For test FOO.Bar, this contains info about Bar.
/// Please refer to documentation of `TestOutput` for details.
#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
pub struct IndividualTestOutput {
    pub name: String,
    pub status: IndividualTestOutputStatus,
    pub time: String,
    pub failures: Option<Vec<Failure>>,
    /// This field is not documented, so using String. We can use serde_enum_str to convert it to
    /// enum, but that is not in our third party crates.
    /// Most common values seen in the output are COMPLETED, SKIPPED, SUPPRESSED
    pub result: String,
}

/// Describes whether a test was run or skipped.
///
/// Refer to [`TestSuiteOutput`] documentation for schema details.
#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum IndividualTestOutputStatus {
    #[default]
    Run,
    NotRun,
}

/// Provides info about individual test suites.
/// Refer to [gtest documentation] for output structure.
/// [gtest documentation]: https://github.com/google/googletest/blob/2002f267f05be6f41a3d458954414ba2bfa3ff1d/googletest/docs/advanced.md#generating-a-json-report
#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
pub struct TestSuiteOutput {
    pub name: String,
    pub tests: usize,
    pub failures: usize,
    pub disabled: usize,
    pub time: String,
    pub testsuite: Vec<IndividualTestOutput>,
}

/// Provides info test and the its run result.
/// Example: For test FOO.Bar, this contains info about FOO.
/// Please refer to documentation of `TestSuiteOutput` for details.
#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
pub struct TestOutput {
    pub testsuites: Vec<TestSuiteOutput>,
}

/// Opens and reads file defined by `path` in `dir`.
pub async fn read_file(dir: &fio::DirectoryProxy, path: &str) -> Result<String, Error> {
    // Open the file in read-only mode.
    let result_file_proxy = fuchsia_fs::directory::open_file(dir, path, fio::PERM_READABLE).await?;
    return fuchsia_fs::file::read_to_string(&result_file_proxy).await.map_err(Into::into);
}

pub fn parse_test_cases(test_string: String) -> Result<Vec<TestCaseInfo>, EnumerationError> {
    let test_list: ListTestResult =
        serde_json::from_str(&test_string).map_err(EnumerationError::from)?;

    let mut tests = Vec::<TestCaseInfo>::with_capacity(test_list.tests);

    for suite in &test_list.testsuites {
        for test in &suite.testsuite {
            let name = format!("{}.{}", suite.name, test.name);
            let enabled = is_test_case_enabled(&name);
            tests.push(TestCaseInfo { name, enabled })
        }
    }

    Ok(tests)
}

/// Returns `true` if the test case is disabled, based on its name. (This is apparently the only
/// way that gtest tests can be disabled.)
/// See
/// https://github.com/google/googletest/blob/HEAD/googletest/docs/advanced.md#temporarily-disabling-tests
fn is_test_case_enabled(case_name: &str) -> bool {
    !case_name.contains("DISABLED_")
}
