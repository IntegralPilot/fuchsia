// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{helpers, results_parser};
use anyhow::{Context, Error};
use fidl_fuchsia_test::{self as ftest, Result_ as TestResult, Status};
use ftest::CaseListenerProxy;
use futures::AsyncBufReadExt;
use futures::io::BufReader;
use gtest_runner_lib::parser;
use gtest_runner_lib::parser::{IndividualTestOutputStatus, TestSuiteOutput};
use helpers::TestType;
use std::collections::HashMap;
use test_runners_lib::cases::TestCaseInfo;
use {fidl_fuchsia_component_runner as frunner, fuchsia_async as fasync};

const DYNAMIC_SKIP_RESULT: &str = "SKIPPED";

const LIST_TESTS_ARG: &str = "list_tests";
const FILTER_ARG: &str = "filter=";
const OUTPUT_PATH: &str = "{component_path}/test_data/";

/// Runs the test component with `--gunit_list_tests` and returns the parsed test cases list.
pub async fn get_cases_list_for_gtests(
    mut start_info: frunner::ComponentStartInfo,
    component_runner: &frunner::ComponentRunnerProxy,
    test_type: TestType,
) -> Result<Vec<ftest::Case>, Error> {
    log::debug!("getting test cases for gtest");
    // Replace the program args to get test cases in a json file.
    let list_tests_arg = helpers::format_arg(test_type, LIST_TESTS_ARG);
    let output_file_name = helpers::unique_test_result_filename();
    let output_path = format!("output=json:{}{}", OUTPUT_PATH, output_file_name);
    let output_arg = helpers::format_arg(test_type, &output_path);
    helpers::replace_program_args(
        vec![list_tests_arg, output_arg],
        start_info.program.as_mut().context("No program.")?,
    );

    let (outgoing_dir, _outgoing_dir_server) = zx::Channel::create();
    start_info.outgoing_dir = Some(outgoing_dir.into());
    start_info.numbered_handles = Some(vec![]);
    let output_dir = helpers::add_output_dir_to_namespace(&mut start_info)?;

    log::debug!(start_info:?; "starting test component to list test cases");
    let component_controller = helpers::start_test_component(start_info, component_runner)?;
    match helpers::read_result(component_controller.take_event_stream()).await.status {
        Some(ftest::Status::Passed) => (),
        Some(ftest::Status::Failed) => log::error!("Test dry-run to list cases failed."),
        Some(ftest::Status::Skipped) => log::error!("Test dry-run to list cases was skipped."),
        None => log::error!("Test dry-run to list cases didn't provide a status."),
    }

    // Parse tests from output file.
    let read_content = parser::read_file(&output_dir, &output_file_name)
        .await
        .context("reading tests from output file.")?;
    let tests = parser::parse_test_cases(read_content).context("parsing test cases")?;
    Ok(tests
        .iter()
        .map(|TestCaseInfo { name, enabled }| ftest::Case {
            name: Some(name.clone()),
            enabled: Some(*enabled),
            ..Default::default()
        })
        .collect())
}

/// Runs a gunit test case associated with a single `ftest::SuiteRequest::Run` request.
///
/// Running the test component is delegated to an instance of the starnix kernel.
/// stdout logs are parsed for results and forwarded to the test framework.
///
/// # Parameters
/// - `tests`: The test invocations to run.
/// - `start_info`: The component start info of the test to run.
/// - `run_listener_proxy`: The proxy used to communicate results of the test run to the test
///                         framework.
/// - `component_runner`: The runner that will run the test component.
/// - `test_type`: The type of test to run, used to determine which arguments to pass to the test.
pub async fn run_gunit_cases(
    tests: Vec<ftest::Invocation>,
    mut start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
) -> Result<(), Error> {
    // Before passing in the handles, we'll need to wire up an IO sink so that we can parse results.
    let (numbered_handles, mut std_handles) = helpers::create_numbered_handles();
    let (stdout_reader, stdout_writer) = zx::Socket::create_stream();
    let stdout_sink = std_handles.out.take().unwrap();
    std_handles.out = Some(stdout_reader);

    let top_level_report_proxy = helpers::start_top_level_report(
        &mut start_info,
        run_listener_proxy,
        numbered_handles,
        std_handles,
    )?;

    // Notify test manager of cases starting.
    let mut run_listener_proxies = helpers::start_tests(&tests, run_listener_proxy)?;
    let test_filter_arg = create_tests_filter_arg(&tests, TestType::Gunit);
    helpers::append_program_args(
        vec![test_filter_arg],
        start_info.program.as_mut().context("No program.")?,
    );

    // Start the test component.
    let component_controller = helpers::start_test_component(start_info, component_runner)?;
    let _ = helpers::read_result(component_controller.take_event_stream()).await;
    top_level_report_proxy
        .finished(&TestResult { status: Some(Status::Passed), ..Default::default() })?;

    // Parse stdout.
    let mut reader = BufReader::new(fasync::Socket::from_socket(stdout_sink));
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 {
        // Copy output to test's stdout.
        let _ = stdout_writer.write(line.as_bytes())?;

        let status = if line.starts_with("[       OK ]") {
            Status::Passed
        } else if line.starts_with("[  FAILED  ]") {
            Status::Failed
        } else if line.starts_with("[  SKIPPED ]") {
            Status::Skipped
        } else {
            line.clear();
            continue;
        };

        let test_name = line[13..].split_whitespace().next().unwrap_or_default();
        if let Some(case_listener_proxy) = run_listener_proxies.remove(test_name) {
            case_listener_proxy
                .finished(&TestResult { status: Some(status), ..Default::default() })?;
        }
        line.clear();
    }

    handle_unresolved_test_cases(run_listener_proxies)?;
    Ok(())
}

/// Runs a gtest case associated with a single `ftest::SuiteRequest::Run` request.
///
/// Running the test component is delegated to an instance of the starnix kernel.
/// Results are read from and output file and stdout logs are forwarded to the test framework.
///
/// # Parameters
/// - `tests`: The test invocations to run.
/// - `start_info`: The component start info of the test to run.
/// - `run_listener_proxy`: The proxy used to communicate results of the test run to the test
///                         framework.
/// - `component_runner`: The runner that will run the test component.
pub async fn run_gtest_cases(
    tests: Vec<ftest::Invocation>,
    mut start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
    test_type: TestType,
) -> Result<(), Error> {
    let (numbered_handles, std_handles) = helpers::create_numbered_handles();
    let top_level_report_proxy = helpers::start_top_level_report(
        &mut start_info,
        run_listener_proxy,
        numbered_handles,
        std_handles,
    )?;
    let (run_listener_proxies, test_result_list) =
        execute_gtests(component_runner, start_info, tests, run_listener_proxy, test_type).await?;

    // Mark the overall test suite as completed. The status is always "Passed," but we will parse
    // and report the actual status of individual tests below.
    top_level_report_proxy
        .finished(&TestResult { status: Some(Status::Passed), ..Default::default() })?;

    report_test_results(run_listener_proxies, test_result_list)
}

/// Executes the given tests and returns a map of the tests to their listener proxy, as well as the
/// test execution results. Callers can iterate through results, and report state to the proxies.
/// If the given tests fail to execute gracefully (e.g. crash), an empty vector will be returned.
pub async fn execute_gtests(
    component_runner: &frunner::ComponentRunnerProxy,
    mut start_info: frunner::ComponentStartInfo,
    tests: Vec<ftest::Invocation>,
    run_listener_proxy: &ftest::RunListenerProxy,
    test_type: TestType,
) -> Result<(HashMap<String, CaseListenerProxy>, Vec<TestSuiteOutput>), Error> {
    // Notify test manager of cases starting.
    let run_listener_proxies = helpers::start_tests(&tests, run_listener_proxy)?;
    let test_filter_arg = create_tests_filter_arg(&tests, test_type);

    let output_file_name = helpers::unique_test_result_filename();
    let output_format = match test_type {
        TestType::Gtest => "json",
        TestType::GtestXmlOutput => "xml",
        _ => panic!("unexpected test type"),
    };

    let output_arg = helpers::format_arg(
        test_type,
        &format!("output={}:{}{}", output_format, OUTPUT_PATH, output_file_name),
    );

    log::info!("writing json output to {:?}", output_arg);
    helpers::append_program_args(
        vec![test_filter_arg, output_arg],
        start_info.program.as_mut().expect("No program."),
    );

    let output_dir = helpers::add_output_dir_to_namespace(&mut start_info)?;

    // Start the test component.
    let component_controller = helpers::start_test_component(start_info, component_runner)?;
    let _ = helpers::read_result(component_controller.take_event_stream()).await;

    // Parse test results.
    let test_results = parser::read_file(&output_dir, &output_file_name)
        .await
        .with_context(|| format!("Failed to read {}", output_file_name))
        .and_then(|x| {
            results_parser::parse_results(test_type, x.trim())
                .with_context(|| format!("Failed to parse {}", output_file_name))
        });

    // If tests crashes then we may fail to read or parse the output file. We should handle that
    // edge case gracefully so that callers can decide how to proceed.
    let test_result_list = match test_results {
        Ok(results) => results.testsuites,
        Err(e) => {
            log::error!("Tests crashed whilst running: {}", e);
            vec![]
        }
    };

    return Ok((run_listener_proxies, test_result_list));
}

/// Reports the test results back to the given proxies. If any proxies are left unanswered by the
/// given results, they will be cross-checked for disabled status. If they were not disabled, then
/// it will be assumed that the tests failed (e.g. crashed and results not reported).
pub fn report_test_results(
    mut run_listener_proxies: HashMap<String, CaseListenerProxy>,
    test_suite_output: Vec<TestSuiteOutput>,
) -> Result<(), Error> {
    for suite in &test_suite_output {
        for test in &suite.testsuite {
            let name = format!("{}.{}", suite.name, test.name);
            let status = match &test.status {
                IndividualTestOutputStatus::NotRun => Status::Skipped,
                IndividualTestOutputStatus::Run => match test.result.as_str() {
                    DYNAMIC_SKIP_RESULT => Status::Skipped,
                    _ => match &test.failures {
                        Some(_failures) => Status::Failed,
                        None => Status::Passed,
                    },
                },
            };

            let case_listener_proxy = run_listener_proxies
                .remove(&name)
                .unwrap_or_else(|| panic!("No case listener for test case {name}"));
            case_listener_proxy
                .finished(&TestResult { status: Some(status), ..Default::default() })?;
        }
    }

    handle_unresolved_test_cases(run_listener_proxies)?;
    Ok(())
}

pub fn create_tests_filter_arg(tests: &Vec<ftest::Invocation>, test_type: TestType) -> String {
    let mut test_filter_arg = helpers::format_arg(test_type, FILTER_ARG);
    for test in tests {
        let test_name = test.name.clone().expect("No test name.");
        test_filter_arg = format!("{}{}:", test_filter_arg, &test_name);
    }

    test_filter_arg
}

/// We may have unresolved cases if the test was explicitly disabled, but also if running the test
/// crashed or otherwise behaved in an unexpected way. This function resolves all hanging test cases
/// and returns the number of tests which were marked as failing.
fn handle_unresolved_test_cases(
    run_listener_proxies: HashMap<String, CaseListenerProxy>,
) -> Result<i32, Error> {
    let mut failing_count = 0;
    for (name, case_listener_proxy) in run_listener_proxies {
        log::warn!("Did not receive result for {name}. Marking as failed if not a disabled test.");
        let status = if name.contains("DISABLED") {
            Status::Skipped
        } else {
            failing_count += 1;
            Status::Failed
        };
        case_listener_proxy.finished(&TestResult { status: Some(status), ..Default::default() })?;
    }
    Ok(failing_count)
}
