// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use diagnostics_data::Severity;
use fidl_fuchsia_diagnostics::LogInterestSelector;
use fidl_fuchsia_sys2 as fsys;
use fidl_fuchsia_test_manager::{LaunchError, LogsIteratorType, SuiteRunnerMarker};
use run_test_suite_lib::{Outcome, RunTestSuiteError};

#[derive(FromArgs, Default, PartialEq, Debug)]
/// Entry point for executing tests.
struct Args {
    /// test timeout. Exits with -`ZX_ERR_TIMED_OUT` if the test times out.
    #[argh(option, short = 't')]
    timeout: Option<u32>,

    /// test url. Test should implement `fuchsia.test.Suite` protocol.
    #[argh(positional)]
    test_url: String,

    /// test filter. Glob pattern for matching tests. Can be specified multiple
    /// times to pass in multiple patterns. Tests may be excluded by prepending a
    /// '-' to the glob pattern.
    /// example: --test-filter glob1 --test-filter glob2.
    #[argh(option)]
    test_filter: Vec<String>,

    /// count an empty test suite as a success. This is used when CTF tests are disabled
    /// to facilitate a breaking change.
    #[argh(option)]
    no_cases_equals_success: Option<bool>,

    /// the realm to run the test in. This field is optional and takes the form:
    /// /path/to/realm:test_collection. See https://fuchsia.dev/go/components/non-hermetic-tests
    #[argh(option)]
    realm: Option<String>,

    /// whether to also run tests that have been marked disabled/ignored by the test author.
    #[argh(switch)]
    also_run_disabled_tests: bool,

    /// whether to filter ANSI escape sequences from stdout.
    #[argh(switch)]
    filter_ansi: bool,

    /// continue running unfinished suites if a suite times out.
    /// By default, unfinished suites are immediately terminated if a suite times out.
    /// This option is only relevant when multiple suites are run.
    #[argh(switch)]
    continue_on_timeout: bool,

    /// stop running unfinished suites after the number of provided failures has occurred.
    /// By default, all suites are run to completion if a suite fails.
    #[argh(option)]
    stop_after_failures: Option<u32>,

    /// run test cases in parallel, up to the number provided.
    #[argh(option)]
    parallel: Option<u16>,

    /// number of times to run the test. By default run 1 time.
    /// If an iteration of test times out, no further iterations
    /// would be executed.
    #[argh(option)]
    count: Option<u32>,

    /// whether the test creates exception channels, which may conflict with any exception
    /// channels created by test_manager.
    #[argh(switch)]
    no_exception_channel: bool,

    /// when set, only logs with a severity equal to the given one or higher will be printed for
    /// the associated component.
    ///
    /// This modifies the minimum log severity level emitted by components during the test
    /// execution.
    ///
    /// Specify using the format <component-selector>#<log-level>, or just <log-level> (in which
    /// case the severity will apply to all components under the test, including the test component
    /// itself) with level as one of FATAL|ERROR|WARN|INFO|DEBUG|TRACE.
    /// May be repeated.
    #[argh(option, from_str_fn(log_interest_selector_or_severity))]
    min_severity_logs: Vec<LogInterestSelector>,

    /// when set, the test will fail if any log with a higher severity is emitted.
    #[argh(option)]
    max_severity_logs: Option<Severity>,

    /// when set, saves the output directory on the host.
    /// Note - this option is only intended to aid in migrating OOT tests to v2. It will be
    /// removed once existing users stop using it, and new users will not be supported.
    // TODO(https://fxbug.dev/42178399): remove this option once users are migrated to ffx test.
    #[argh(option)]
    deprecated_output_directory: Option<String>,

    #[argh(positional)]
    /// arguments passed to tests following `--`.
    test_args: Vec<String>,
}

fn log_interest_selector_or_severity(input: &str) -> Result<LogInterestSelector, String> {
    selectors::parse_log_interest_selector_or_severity(input).map_err(|s| s.to_string())
}

const REALM_QUERY_PATH: &str = "/svc/fuchsia.sys2.RealmQuery.root";
const LIFECYCLE_CONTROLLER_PATH: &str = "/svc/fuchsia.sys2.LifecycleController.root";

#[fuchsia::main]
async fn main() {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    let args = argh::from_env();

    let Args {
        timeout,
        test_url,
        test_filter,
        no_cases_equals_success,
        realm,
        also_run_disabled_tests,
        continue_on_timeout,
        stop_after_failures,
        parallel,
        count,
        no_exception_channel,
        min_severity_logs,
        max_severity_logs,
        deprecated_output_directory,
        test_args,
        filter_ansi,
    } = args;
    let count = count.unwrap_or(1);
    if count == 0 {
        println!("--count should be greater than zero.");
        std::process::exit(1);
    }

    if filter_ansi {
        println!("Note: Filtering out ANSI escape sequences.");
    }

    let mut provided_realm = None;
    let hermetic_test = realm.is_none();
    if let Some(realm) = realm {
        let lifecycle_controller = fuchsia_component::client::connect_to_protocol_at_path::<
            fsys::LifecycleControllerMarker,
        >(LIFECYCLE_CONTROLLER_PATH)
        .expect("connecting to LifecycleController");
        let realm_query = fuchsia_component::client::connect_to_protocol_at_path::<
            fsys::RealmQueryMarker,
        >(REALM_QUERY_PATH)
        .expect("connecting to RealmQuery");
        match run_test_suite_lib::parse_provided_realm(&lifecycle_controller, &realm_query, &realm)
            .await
        {
            Ok(r) => {
                provided_realm = Some(r);
            }
            Err(e) => {
                println!("Error parsing realm '{}': {:?}", realm, e);
                std::process::exit(1);
            }
        }
    }

    let test_filters = if test_filter.len() == 0 { None } else { Some(test_filter) };
    let shell_reporter = run_test_suite_lib::output::ShellReporter::new(std::io::stdout());
    let dir_reporter = match deprecated_output_directory {
        Some(path) => match run_test_suite_lib::output::DirectoryReporter::new(
            path.into(),
            run_test_suite_lib::output::SchemaVersion::V1,
        ) {
            Ok(reporter) => Some(reporter),
            Err(e) => {
                println!("Failed to make directory reporter: {:?}", e);
                std::process::exit(1);
            }
        },
        None => None,
    };
    let run_reporter = match (filter_ansi, dir_reporter) {
        (true, None) => run_test_suite_lib::output::RunReporter::new_ansi_filtered(shell_reporter),
        (false, None) => run_test_suite_lib::output::RunReporter::new(shell_reporter),
        (true, Some(dir_reporter)) => run_test_suite_lib::output::RunReporter::new_ansi_filtered(
            run_test_suite_lib::output::MultiplexedReporter::new(shell_reporter, dir_reporter),
        ),
        (false, Some(dir_reporter)) => run_test_suite_lib::output::RunReporter::new(
            run_test_suite_lib::output::MultiplexedReporter::new(shell_reporter, dir_reporter),
        ),
    };

    let run_params = run_test_suite_lib::RunParams {
        timeout_behavior: match continue_on_timeout {
            false => run_test_suite_lib::TimeoutBehavior::TerminateRemaining,
            true => run_test_suite_lib::TimeoutBehavior::Continue,
        },
        timeout_grace_seconds: 0,
        stop_after_failures: match stop_after_failures.map(std::num::NonZeroU32::new) {
            None => None,
            Some(None) => {
                println!("--stop-after-failures should be greater than zero.");
                std::process::exit(1);
            }
            Some(Some(stop_after)) => Some(stop_after),
        },
        accumulate_debug_data: true, // must be true to support coverage via scp
        log_protocol: Some(LogsIteratorType::Batch),
        min_severity_logs: min_severity_logs.clone(),
        // TODO(https://fxbug.dev/42059408): make this configurable
        show_full_moniker: true,
    };

    let proxy = fuchsia_component::client::connect_to_protocol::<SuiteRunnerMarker>()
        .expect("connecting to SuiteRunner");
    let start_time = std::time::Instant::now();
    let mut outcome = run_test_suite_lib::run_test_and_get_outcome(
        run_test_suite_lib::SingleRunConnector::new(proxy),
        run_test_suite_lib::TestParams {
            test_url,
            realm: provided_realm.into(),
            timeout_seconds: timeout.and_then(std::num::NonZeroU32::new),
            test_filters,
            no_cases_equals_success,
            also_run_disabled_tests,
            parallel,
            test_args,
            max_severity_logs,
            min_severity_logs,
            tags: vec![],
            no_exception_channel,
            break_on_failure: false,
        },
        run_params,
        run_reporter,
        futures::future::pending(),
    )
    .await;
    log::info!("run test suite duration: {:?}", start_time.elapsed().as_secs_f32());
    let is_no_matching_cases = match outcome {
        run_test_suite_lib::Outcome::Error { ref origin } => match **origin {
            RunTestSuiteError::Launch(LaunchError::NoMatchingCases) => true,
            _ => false,
        },
        _ => false,
    };
    if is_no_matching_cases && no_cases_equals_success == Some(true) {
        outcome = Outcome::Passed;
    }
    if outcome != run_test_suite_lib::Outcome::Passed {
        println!("One or more test runs failed.");
    }
    let show_realm_warning = outcome == run_test_suite_lib::Outcome::Timedout
        || outcome == run_test_suite_lib::Outcome::Failed
        || outcome == run_test_suite_lib::Outcome::DidNotFinish;
    if show_realm_warning && hermetic_test {
        println!(
            "The test was executed in the hermetic realm. If your test depends on system \
capabilities, pass in correct realm. See https://fuchsia.dev/go/components/non-hermetic-tests"
        );
    }
    match outcome {
        run_test_suite_lib::Outcome::Passed => {}
        run_test_suite_lib::Outcome::Timedout => {
            std::process::exit(-zx::Status::TIMED_OUT.into_raw());
        }
        run_test_suite_lib::Outcome::Failed
        | run_test_suite_lib::Outcome::Cancelled
        | run_test_suite_lib::Outcome::DidNotFinish
        | run_test_suite_lib::Outcome::Inconclusive => {
            std::process::exit(1);
        }
        run_test_suite_lib::Outcome::Error { origin } => {
            println!("Encountered error trying to run tests: {}", *origin);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // As we have custom parsing when user passes "--", making sure everything works fine.
    fn test_args() {
        let url = "foo.cm";
        let mut expected_args = Args { test_url: url.to_string(), ..Default::default() };
        expected_args.test_url = url.to_string();

        let args = Args::from_args(&["cmd"], &[url]).unwrap();
        assert_eq!(args, expected_args);

        let args = Args::from_args(&["cmd"], &[url, "--"]).unwrap();
        expected_args.test_args = vec![];
        assert_eq!(args, expected_args);

        // make sure we can parse --help flag when user passes "--"
        let early_exit = Args::from_args(&["cmd"], &[url, "--help", "--"]).unwrap_err();
        assert_eq!(early_exit.status, Ok(()));

        // make sure we can parse --help flag without "--"
        let early_exit = Args::from_args(&["cmd"], &[url, "--help"]).unwrap_err();
        assert_eq!(early_exit.status, Ok(()));

        // make sure we can catch arg errors when user passes "--"
        let early_exit = Args::from_args(&["cmd"], &[url, "--timeout", "a", "--"]).unwrap_err();
        assert_eq!(early_exit.status, Err(()));

        // make sure we can catch arg errors without "--"
        let early_exit = Args::from_args(&["cmd"], &[url, "--timeout", "a"]).unwrap_err();
        assert_eq!(early_exit.status, Err(()));

        // make sure we can parse args when user passes "--"
        let args = Args::from_args(&["cmd"], &[url, "--timeout", "2", "--"]).unwrap();
        expected_args.timeout = Some(2);
        expected_args.test_args = vec![];
        assert_eq!(args, expected_args);

        // make sure we can parse args without "--"
        let args = Args::from_args(&["cmd"], &[url, "--timeout", "2"]).unwrap();
        assert_eq!(args, expected_args);

        // make sure we can parse args after "--"
        let args = Args::from_args(
            &["cmd"],
            &[url, "--timeout", "2", "--", "--arg1", "some_random_str", "-arg2"],
        )
        .unwrap();
        expected_args.test_args =
            vec!["--arg1".to_owned(), "some_random_str".to_owned(), "-arg2".to_owned()];
        assert_eq!(args, expected_args);

        // Args::from_args works with multiple "--"
        let args = Args::from_args(
            &["cmd"],
            &[url, "--timeout", "2", "--", "--", "--arg1", "some_random_str", "--", "-arg2"],
        )
        .unwrap();
        expected_args.test_args = vec![
            "--".to_owned(),
            "--arg1".to_owned(),
            "some_random_str".to_owned(),
            "--".to_owned(),
            "-arg2".to_owned(),
        ];
        assert_eq!(args, expected_args);
    }
}
