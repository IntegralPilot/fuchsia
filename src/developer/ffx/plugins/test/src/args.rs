// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::ensure;
use argh::{ArgsInfo, FromArgs};
use diagnostics_data::Severity;
use fidl_fuchsia_diagnostics::LogInterestSelector;
use std::path::PathBuf;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "test",
    description = "Run test suite",
    note = "Run tests or inspect output from a previous test run."
)]
pub struct TestCommand {
    #[argh(subcommand)]
    pub subcommand: TestSubCommand,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum TestSubCommand {
    Run(RunCommand),
    List(ListCommand),
    EarlyBootProfile(EarlyBootProfileCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone, Default)]
#[argh(
    subcommand,
    name = "run",
    description = "Execute test suites on a target device",
    example = "\
Run a test suite:
    $ ffx test run fuchsia-pkg://fuchsia.com/my_test#meta/my_test.cm

Run a test suite in system realm:
    $ ffx test run --realm /core/testing/system-tests fuchsia-pkg://fuchsia.com/my_test#meta/my_test.cm

Run a test suite and pass arguments to the suite:
    $ ffx test run fuchsia-pkg://fuchsia.com/my_test#meta/my_test.cm -- arg1 arg2

Run test suites specified in a JSON file (currently unstable):
    $ ffx test run --test-file test-list.json

Given a suite that contains the test cases 'Foo.Test1', 'Foo.Test2',
'Bar.Test1', and 'Bar.Test2':

Run test cases that start with 'Foo.' ('Foo.Test1', 'Foo.Test2'):
    $ ffx test run <suite-url> --test-filter 'Foo.*'

Run test cases that do not start with 'Foo.' ('Bar.Test1', 'Bar.Test2'):
    $ ffx test run <suite-url> --test-filter '-Foo.*'

Run test cases that start with 'Foo.' and do not end with 'Test1' ('Foo.Test2'):
    $ ffx test run <suite-url> --test-filter 'Foo.*' --test-filter '-*.Test1'",
    note = "Runs test suites implementing the `fuchsia.test.Suite` protocol.

When multiple test suites are run, either through the --count option or through
--test-file, the default behavior of ffx test is:
    If any test suite times out, halt execution and do not attempt to run any
    unstarted suites.
    If any test suite fails for any other reason, continue to run unstarted
    suites."
)]
pub struct RunCommand {
    /// read test parameters from the specified file instead of from the command line.
    /// This is intended to be used by test_pilot.
    #[argh(option)]
    pub pilot: Option<String>,

    /// test suite url, and any arguments passed to tests, following `--`.
    /// When --test-file is specified test_args should not be specified.
    #[argh(positional)]
    pub test_args: Vec<String>,

    // TODO(satsukiu): once stable, document the format
    /// read test url and options from the specified file instead of from the
    /// command line.
    /// May not be used in conjunction with `test_args`, `--count`,
    /// `--test-filter`, `--run-disabled`, `--parallel`, `--max-severity-logs`
    /// This option is currently unstable and the format of the file is subject
    /// to change. Using this option requires setting the
    /// 'test.experimental_json_input' configuration to true.
    ///
    /// For current details, see test-list.json format at
    /// https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/lib/testing/test_list/
    #[argh(option)]
    pub test_file: Option<String>,

    // suite options
    /// test suite timeout in seconds.
    #[argh(option, short = 't')]
    pub timeout: Option<u32>,

    /// test case filter. This filter will match based on glob pattern. This
    /// option may be specified multiple times. Only test cases matching at
    /// least one pattern will be run. Negative filters may be specified by
    /// prepending '-' and will exclude matching test cases.
    #[argh(option)]
    pub test_filter: Vec<String>,

    /// the realm to run the test in. This field is optional and takes the form:
    /// /path/to/realm:test_collection. See https://fuchsia.dev/go/components/non-hermetic-tests
    #[argh(option)]
    pub realm: Option<String>,

    /// also execute test cases that have been disabled by the test author.
    #[argh(switch)]
    pub run_disabled: bool,

    /// maximum number of test cases to run in parallel. Defaults to a value
    /// specified by the test runner.
    #[argh(option)]
    pub parallel: Option<u16>,

    /// when set, fails tests that emit logs with a higher severity.
    ///
    /// For example, if --max-severity-logs WARN is specified, fails any test
    /// that produces an ERROR level log.
    #[argh(option)]
    pub max_severity_logs: Option<Severity>,

    // test run options
    /// continue running unfinished suites if a suite times out.
    /// This option is only relevant when multiple suites are run.
    #[argh(switch)]
    pub continue_on_timeout: bool,

    /// stop running unfinished suites after the number of provided failures
    /// has occurred. This option is only relevant when multiple suites are
    /// run.
    #[argh(option)]
    pub stop_after_failures: Option<u32>,

    /// number of times to run the test suite. By default run the suite 1 time.
    #[argh(option)]
    pub count: Option<u32>,

    /// enables experimental parallel test scheduler. The provided number
    /// specifies the max number of test suites to run in parallel.
    /// If the value provided is 0, a default value will be chosen by the
    /// server implementation.
    #[argh(option)]
    pub experimental_parallel_execution: Option<u16>,

    /// enable break_on_failure for supported test runners. Any test case failure causes the test
    /// execution to stop and wait for zxdb to attach to debug the failure. When the debugger
    /// exits, the process will be released and the suite will be continued.
    ///
    /// Note: test runners may or may not have the ability to halt a test suite after it has
    /// started executing. If there isn't a way to raise an exception for a debugger to catch, the
    /// test will run and exit as normal, and will not wait for any debugger interaction.
    #[argh(switch)]
    pub break_on_failure: bool,

    // output options
    /// filter ANSI escape sequences from output.
    #[argh(switch)]
    pub filter_ansi: bool,

    /// set the minimum log severity printed.
    ///
    /// This modifies the minimum log severity level emitted by components during the test
    /// execution.
    ///
    /// Specify using the format <component-selector>#<log-level>, or just <log-level> (in which
    /// case the severity will apply to all components under the test, including the test component
    /// itself) with level as one of FATAL|ERROR|WARN|INFO|DEBUG|TRACE.
    /// May be repeated.
    #[argh(option, from_str_fn(log_interest_selector_or_severity))]
    pub min_severity_logs: Vec<LogInterestSelector>,

    /// show the full moniker in unstructured log output.
    #[argh(switch)]
    pub show_full_moniker_in_logs: bool,

    /// output test results to the specified directory. The produced output
    /// is in the format described in
    /// https://fuchsia.dev/fuchsia-src/reference/platform-spec/testing/test-output-format
    #[argh(option)]
    pub output_directory: Option<String>,

    /// disable structured output to a directory. Note structured output is
    /// disabled by default, unless --output-directory is specified. This
    /// option supported an experiment which has been removed. It is now a
    /// no-op and will soon be removed.
    #[argh(switch)]
    pub disable_output_directory: bool,

    /// when set, prevents test_manager from creating exception channels that may confilict
    /// with those created by the test.
    #[argh(switch)]
    pub no_exception_channel: bool,

    /// when set, the test_filter selecting an empty set of cases to run is treated
    /// as success rather than failure.
    #[argh(switch)]
    pub no_cases_equals_success: bool,

    /// indicates whether a syslog that covers the duration of the test run should be captured in
    /// the test output.
    #[argh(switch)]
    pub capture_syslog: bool,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list-cases",
    description = "List test suite cases",
    note = "Lists the set of test cases available in a test suite"
)]
pub struct ListCommand {
    /// test url
    #[argh(positional)]
    pub test_url: String,

    /// the realm to enumerate the test in. This field is optional and takes the form:
    /// /path/to/realm:test_collection.
    #[argh(option)]
    pub realm: Option<String>,
}

fn log_interest_selector_or_severity(input: &str) -> Result<LogInterestSelector, String> {
    selectors::parse_log_interest_selector_or_severity(input).map_err(|s| s.to_string())
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "early-boot-profile", description = "Manage early boot profiles")]

pub struct EarlyBootProfileCommand {
    /// output early boot profile to the specified directory. The produced output
    /// is in the format described in
    /// https://fuchsia.dev/fuchsia-src/reference/platform-spec/testing/test-output-format
    #[argh(option, from_str_fn(dir_parse_path))]
    pub output_directory: PathBuf,
}

fn dir_parse_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    let validation = move || {
        ensure!(path.exists(), "{:?} does not exist", path);
        let metadata = std::fs::metadata(&path)?;
        ensure!(metadata.is_dir(), "{:?} should be a directory", path);
        Ok(path)
    };
    validation().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::dir_parse_path;
    use tempfile::{NamedTempFile, tempdir};

    #[test]
    fn test_dir_parse_path() {
        // Create a temporary directory
        let temp_dir = tempdir().expect("Failed to create temporary directory");
        let temp_dir_path = temp_dir.path();

        // Valid directory path
        assert!(dir_parse_path(temp_dir_path.to_str().unwrap()).is_ok());
        assert!(dir_parse_path(".").is_ok());

        // Clean up temporary directory after the test
        temp_dir.close().expect("Failed to close temporary directory");

        // Invalid directory path
        assert!(dir_parse_path("/non_existent_path").is_err());
        // Create a temporary file
        let temp_file = NamedTempFile::new().expect("Failed to create temporary file in the test");
        let temp_file_path = temp_file.path().to_str().unwrap().to_string();

        assert!(dir_parse_path(temp_file_path.as_str()).is_err()); // File path
    }
}
