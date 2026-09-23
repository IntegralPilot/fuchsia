// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::debian_guest::DebianGuest;
use crate::{gtest, helpers, results_parser};

use anyhow::{self, Context};
use cm_types::NamespacePath;
use fidl::endpoints;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_test::{self as ftest, CaseListenerProxy, Result_ as TestResult, Status};
use gtest_runner_lib::parser::TestSuiteOutput;
use helpers::TestType;
use namespace::Namespace;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

const HOST_TMP_DIR: &str = "/tmp";
const CML_TARGET_KERNEL_FIELD: &str = "test_target_kernel";

/// Shallow helper struct, simply for encapsulating the various ways that tests report.
struct TestRunnerReport {
    stdout: zx::Socket,
    stderr: zx::Socket,
    top_level_report_proxy: CaseListenerProxy,
    individual_report_proxies: HashMap<String, CaseListenerProxy>,
}

pub async fn run_syscall_gtests(
    tests: Vec<ftest::Invocation>,
    mut test_start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
    debian_guest: Arc<DebianGuest>,
) -> Result<(), anyhow::Error> {
    let program_data = test_start_info.program.as_mut().unwrap();
    let test_target_kernel =
        helpers::take_opt_str_value_from_dict(program_data, CML_TARGET_KERNEL_FIELD)?;

    match test_target_kernel.as_deref() {
        Some("linux") => {
            log::info!(
                "Linux {} specified, bootstrapping the Machina guest.",
                CML_TARGET_KERNEL_FIELD
            );
            run_on_debian_guest(&tests, &mut test_start_info, run_listener_proxy, debian_guest)
                .await
        }
        None => {
            log::info!(
                "No {} specified, defaulting to Starnix environment for execution.",
                CML_TARGET_KERNEL_FIELD
            );

            // Forward Starnix kernel tests over to the vanilla Gtest runner.
            gtest::run_gtest_cases(
                tests,
                test_start_info,
                run_listener_proxy,
                component_runner,
                TestType::Gtest,
            )
            .await
        }
        Some(unexpected_target) => {
            anyhow::bail!(
                "Unexpected and unknown {} specified. Value: {}",
                CML_TARGET_KERNEL_FIELD,
                unexpected_target
            )
        }
    }
}

/// Runs a set of tests within a Debian guest and reports the results. This function handles setup,
/// pushing test dependencies, executing the tests, and retrieving and parsing the output.
async fn run_on_debian_guest(
    tests: &Vec<ftest::Invocation>,
    test_start_info: &mut frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    debian_guest: Arc<DebianGuest>,
) -> Result<(), anyhow::Error> {
    // We need to take() the namespace from the start_info, but will immediately clone it back.
    let mut test_component_ns = Namespace::try_from(test_start_info.ns.take().unwrap())?;
    test_start_info.ns = Some(test_component_ns.clone().try_into()?);
    let test_pkg_dir = test_component_ns
        .remove(&NamespacePath::new("/pkg")?)
        .ok_or_else(|| anyhow::anyhow!("Could not find /pkg in namespace!"))?
        .into_proxy();

    // Initialize the environment.
    let test_runner_report =
        initialize_test_runner_reporting(tests, run_listener_proxy, test_start_info)?;

    let data_mount = helpers::get_syscall_data_mount(&test_pkg_dir).await?;
    debian_guest.configure_data_mount(data_mount);

    let guest_binary_location = helpers::get_guest_syscall_test_binary_path(test_start_info)?;
    let test_binary_file =
        helpers::open_guest_syscall_test_binary(&test_pkg_dir, test_start_info).await?;
    debian_guest.push_data_to_guest(test_binary_file, &guest_binary_location).await?;
    let (exec_command, guest_output_filename) = format_exec_command(tests, &guest_binary_location);

    // Execute the tests and retrieve the results. The command's overall return code is ignored, as
    // the gTest results are parsed from the stdout in get_test_results.
    let _ = debian_guest
        .execute(
            &exec_command,
            &[],
            None,
            Some(test_runner_report.stdout),
            Some(test_runner_report.stderr),
        )
        .await?;
    let test_results = get_test_results(debian_guest.clone(), guest_output_filename).await?;

    // Report results back to the test runner. We mark the overall test suite as "Passed,"
    // but we will parse and report the actual status of individual tests afterwards.
    test_runner_report
        .top_level_report_proxy
        .finished(&TestResult { status: Some(Status::Passed), ..Default::default() })?;
    gtest::report_test_results(test_runner_report.individual_report_proxies, test_results)?;

    Ok(())
}

/// Formats the guest exec command, handling the appropriate gtest filters as well
/// as the JSON output file for test results. Returns the (exec_command, output_filepath)
fn format_exec_command(
    tests: &Vec<ftest::Invocation>,
    guest_binary_location: &Path,
) -> (String, String) {
    let test_filter_arg = gtest::create_tests_filter_arg(tests, TestType::Gtest);
    let guest_output_filename = helpers::unique_test_result_filename();
    let guest_output_path = helpers::get_guest_syscall_test_output_path(&guest_output_filename);
    let output_arg = helpers::format_arg(
        TestType::Gtest,
        &format!("output={}:{}", "json", guest_output_path.display()),
    );
    let exec_command =
        format!("{} {} {}", guest_binary_location.display(), test_filter_arg, output_arg);

    (exec_command, guest_output_filename)
}

/// Transfers a test results file from a guest and parses its contents. In the case of transfer or
/// parsing errors, it logs a warning and returns an empty vector. The test reporting mechanism
/// should handle interpreting the empty vector appropriately.
async fn get_test_results(
    debian_guest: Arc<DebianGuest>,
    guest_output_filename: String,
) -> Result<Vec<TestSuiteOutput>, anyhow::Error> {
    // Firstly, transfer the results file from the guest back to the host.
    let host_test_output_path = Path::new(HOST_TMP_DIR).join(&guest_output_filename);
    let guest_test_output_path =
        helpers::get_guest_syscall_test_output_path(&guest_output_filename);
    let host_test_output_file =
        OpenOptions::new().write(true).create_new(true).open(&host_test_output_path)?;
    let file_channel: zx::Channel = fdio::transfer_fd(host_test_output_file)?.into();
    let file_client_end = endpoints::ClientEnd::from(file_channel);
    let transfer_result = debian_guest.get_file(&guest_test_output_path, file_client_end).await;

    match transfer_result {
        Ok(_) => {
            // Read and parse the results. Since the backing file handle is consumed by the transfer_fd
            // call, we need to reopen the file to read the contents.
            let mut host_test_output_file =
                OpenOptions::new().read(true).open(&host_test_output_path)?;
            let mut gtest_output_buffer = String::new();
            let test_results = host_test_output_file
                .read_to_string(&mut gtest_output_buffer)
                .with_context(|| format!("Failed to read {}", host_test_output_path.display()))
                .and_then(|_| {
                    results_parser::parse_results(TestType::Gtest, gtest_output_buffer.trim())
                        .with_context(|| {
                            format!("Failed to parse {}", host_test_output_path.display())
                        })
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

            Ok(test_result_list)
        }
        Err(e) => {
            // Transfer errors could be an issue with the Machina guest, but are most likely
            // indicative of the test result file not being present. That is most likely indicative
            // of the tests crashing while running. In any case, we'll return an empty list. The
            // parsing contract will handle reporting these as failures.
            log::warn!(
                "Failed to transfer results file, which likely indicates the tests crashed while running: {}",
                e
            );
            Ok(vec![])
        }
    }
}

/// Initializes the necessary plumbing for capturing stdout and stderr,
/// and for reporting test results to the framework's run listener(s).
fn initialize_test_runner_reporting(
    tests: &Vec<ftest::Invocation>,
    run_listener_proxy: &ftest::RunListenerProxy,
    test_start_info: &frunner::ComponentStartInfo,
) -> Result<TestRunnerReport, anyhow::Error> {
    let (test_stdout, stdout_client) = zx::Socket::create_stream();
    let (test_stderr, stderr_client) = zx::Socket::create_stream();
    let std_handles = ftest::StdHandles {
        out: Some(stdout_client),
        err: Some(stderr_client),
        ..Default::default()
    };
    let (top_level_report_proxy, overall_test_listener) =
        endpoints::create_proxy::<ftest::CaseListenerMarker>();
    run_listener_proxy.on_test_case_started(
        &ftest::Invocation {
            name: Some(test_start_info.resolved_url.clone().unwrap_or_default()),
            tag: None,
            ..Default::default()
        },
        std_handles,
        overall_test_listener,
    )?;

    let test_report_proxies = helpers::start_tests(tests, run_listener_proxy)?;
    Ok(TestRunnerReport {
        stdout: test_stdout,
        stderr: test_stderr,
        top_level_report_proxy,
        individual_report_proxies: test_report_proxies,
    })
}
