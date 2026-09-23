// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::binder_latency::run_binder_latency;
use crate::debian_guest::DebianGuest;
use crate::gbenchmark::*;
use crate::gtest::*;
use crate::helpers::*;
use crate::selinux::*;
use crate::syscalls_gtest::*;
use crate::syscalls_libtest::*;
use crate::vts_binary::*;
use anyhow::{Error, anyhow};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_test::{self as ftest};
use frunner::{ComponentRunnerMarker, ComponentRunnerProxy, ComponentStartInfo};
use futures::TryStreamExt;
use log::debug;
use namespace::Namespace;
use rust_measure_tape_for_case::Measurable as _;
use std::sync::Arc;
use test_runners_lib::elf::SuiteServerError;
use zx::sys::ZX_CHANNEL_MAX_MSG_BYTES;

/// Determines what type of tests the program is.
///
/// Removes this from the original start info because the start info is later passed on to the
/// Starnix kernel.
fn remove_test_type(program: &mut fdata::Dictionary) -> Result<TestType, Error> {
    match take_opt_str_value_from_dict(program, "test_type")?.as_deref() {
        Some("binder_latency") => Ok(TestType::BinderLatency),
        Some("gbenchmark") => Ok(TestType::Gbenchmark),
        Some("gtest") => Ok(TestType::Gtest),
        Some("gtest_xml_output") => Ok(TestType::GtestXmlOutput),
        Some("gunit") => Ok(TestType::Gunit),
        Some("vts_binary") => Ok(TestType::VtsBinary),
        Some("selinux") => Ok(TestType::SeLinux),
        Some("syscall_gtest") => Ok(TestType::SyscallGtest),
        Some("syscall_libtest") => Ok(TestType::SyscallLibtest),
        Some("ltp") => Ok(TestType::VtsBinary),
        Some(value) => Err(anyhow!("Unrecognized test_type: {}", value)),

        // If test_type is not specified, then just run the component as a single test case.
        None => Ok(TestType::SingleTest),
    }
}

async fn component_runner_from_start_info(
    start_info: ComponentStartInfo,
) -> Result<ComponentRunnerProxy, Error> {
    debug!("use component runner from namespace");
    let ns = start_info.ns.ok_or_else(|| anyhow!("start info does not have namespace"))?;
    let ns = Namespace::try_from(ns)?;
    let svc = ns
        .get(&"/svc".parse().unwrap())
        .ok_or_else(|| anyhow!("test component namespace does not have /svc"))?;
    return Ok(
        fuchsia_component::client::connect_to_protocol_at_dir_root::<ComponentRunnerMarker>(svc)?,
    );
}

/// Handles a single `ftest::SuiteRequestStream`.
///
/// # Parameters
/// - `start_info`: The start info to use when running the test component.
/// - `stream`: The request stream to handle.
pub async fn handle_suite_requests(
    mut start_info: frunner::ComponentStartInfo,
    mut stream: ftest::SuiteRequestStream,
    debian_guest: Arc<DebianGuest>,
) -> Result<(), Error> {
    debug!(start_info:?; "got suite request stream");
    let test_type = remove_test_type(start_info.program.as_mut().unwrap())?;

    // The kernel start info is largely the same as that of the test component. The main difference
    // is that the `container_start_info` does not contain the outgoing directory of the test component.
    let container_start_info = clone_start_info(&mut start_info)?;
    let component_runner = component_runner_from_start_info(container_start_info).await?;

    while let Some(event) = stream.try_next().await? {
        let mut test_start_info = clone_start_info(&mut start_info)?;
        debug!("got suite request");
        match event {
            ftest::SuiteRequest::GetTests { iterator, .. } => {
                debug!("enumerating test cases");
                let stream = iterator.into_stream();

                let test_cases = match test_type {
                    TestType::Gtest
                    | TestType::Gunit
                    | TestType::GtestXmlOutput
                    | TestType::SyscallGtest => {
                        get_cases_list_for_gtests(test_start_info, &component_runner, test_type)
                            .await?
                    }
                    TestType::BinderLatency | TestType::Gbenchmark | TestType::SingleTest => {
                        let name = test_start_info
                            .resolved_url
                            .as_ref()
                            .ok_or_else(|| anyhow!("Missing resolved URL"))?;
                        vec![ftest::Case {
                            name: Some(name.clone()),
                            enabled: Some(true),
                            ..Default::default()
                        }]
                    }
                    TestType::VtsBinary | TestType::SeLinux => {
                        get_cases_list_for_vts_binary(test_start_info).await?
                    }
                    TestType::SyscallLibtest => {
                        get_cases_list_for_libtests(test_start_info, &component_runner).await?
                    }
                };

                handle_case_iterator(test_cases, stream).await?
            }
            ftest::SuiteRequest::Run { tests, options, listener, .. } => {
                debug!(tests:?; "running tests");
                let run_listener_proxy = listener.into_proxy();

                if tests.is_empty() {
                    debug!("no tests listed, returning");
                    run_listener_proxy.on_finished()?;
                    break;
                }

                // Replace tests with program arguments if they were passed in.
                let mut program =
                    test_start_info.program.clone().ok_or_else(|| anyhow!("Missing program."))?;
                if let Some(test_args) = &options.arguments {
                    replace_program_args(test_args.clone(), &mut program);
                }
                test_start_info.program = Some(program);
                debug!(test_start_info:?; "running tests with info");

                match test_type {
                    TestType::Gunit => {
                        run_gunit_cases(
                            tests,
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                        )
                        .await?;
                    }
                    TestType::Gtest | TestType::GtestXmlOutput => {
                        run_gtest_cases(
                            tests,
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                            test_type,
                        )
                        .await?;
                    }
                    TestType::Gbenchmark => {
                        run_gbenchmark(
                            tests.get(0).unwrap().clone(),
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                        )
                        .await?
                    }
                    TestType::BinderLatency => {
                        run_binder_latency(
                            tests.get(0).unwrap().clone(),
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                        )
                        .await?
                    }
                    TestType::VtsBinary => {
                        run_vts_binary_cases(
                            tests,
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                        )
                        .await?
                    }
                    TestType::SeLinux => {
                        run_selinux_test_suite_cases(
                            tests,
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                        )
                        .await?
                    }
                    TestType::SingleTest => {
                        run_test_case(
                            tests.get(0).unwrap().clone(),
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                        )
                        .await?
                    }
                    TestType::SyscallGtest => {
                        run_syscall_gtests(
                            tests,
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                            debian_guest.clone(),
                        )
                        .await?;
                    }
                    TestType::SyscallLibtest => {
                        run_syscall_libtests(
                            tests,
                            test_start_info,
                            &run_listener_proxy,
                            &component_runner,
                            debian_guest.clone(),
                            options,
                        )
                        .await?;
                    }
                }

                run_listener_proxy.on_finished()?;
            }
        }
    }

    Ok(())
}

/// Runs a test case associated with a single `ftest::SuiteRequest::Run` request.
///
/// Running the test component is delegated to an instance of the starnix kernel.
///
/// # Parameters
/// - `tests`: The tests that are to be run. Each test executes an independent run of the test
/// component.
/// - `test_url`: The URL of the test component.
/// - `program`: The program data associated with the runner request for the test component.
/// - `run_listener_proxy`: The listener proxy for the test run.
/// - `component_runner`: The runner that will run the test component.
async fn run_test_case(
    test: ftest::Invocation,
    mut start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
) -> Result<(), Error> {
    debug!("running generic fallback test suite");
    let (case_listener_proxy, case_listener) = create_proxy::<ftest::CaseListenerMarker>();
    let (numbered_handles, std_handles) = create_numbered_handles();
    start_info.numbered_handles = Some(numbered_handles);

    debug!("notifying client test case started");
    run_listener_proxy.on_test_case_started(&test, std_handles, case_listener)?;

    debug!("starting test component");
    let component_controller = start_test_component(start_info, component_runner)?;

    let result = read_result(component_controller.take_event_stream()).await;
    debug!(result:?; "notifying client test case finished");
    case_listener_proxy.finished(&result)?;

    Ok(())
}

/// Lists all the available test cases and returns them in response to
/// `ftest::CaseIteratorRequest::GetNext`.
async fn handle_case_iterator(
    cases: Vec<ftest::Case>,
    mut stream: ftest::CaseIteratorRequestStream,
) -> Result<(), Error> {
    let mut remaining_cases = &cases[..];

    while let Some(event) = stream.try_next().await? {
        match event {
            ftest::CaseIteratorRequest::GetNext { responder } => {
                // Paginate cases
                // Page overhead of message header + vector
                let mut bytes_used: usize = 32;
                let mut case_count = 0;
                for case in remaining_cases {
                    bytes_used += case.measure().num_bytes;
                    if bytes_used > ZX_CHANNEL_MAX_MSG_BYTES as usize {
                        break;
                    }
                    case_count += 1;
                }
                responder
                    .send(&remaining_cases[..case_count])
                    .map_err(SuiteServerError::Response)?;
                remaining_cases = &remaining_cases[case_count..];
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{ClientEnd, create_request_stream};
    use fuchsia_async as fasync;

    /// Returns a `ftest::CaseIteratorProxy` that is served by `super::handle_case_iterator`.
    ///
    /// # Parameters
    /// - `test_name`: The name of the test case that is provided to `handle_case_iterator`.
    fn set_up_iterator(test_name: &str) -> ftest::CaseIteratorProxy {
        let cases = vec![ftest::Case { name: Some(test_name.to_string()), ..Default::default() }];
        let (iterator_client_end, iterator_stream) =
            create_request_stream::<ftest::CaseIteratorMarker>();
        fasync::Task::local(async move {
            let _ = handle_case_iterator(cases, iterator_stream).await;
        })
        .detach();

        iterator_client_end.into_proxy()
    }

    /// Spawns a `ComponentRunnerRequestStream` server that immediately closes all incoming
    /// component controllers with the epitaph specified in `component_controller_epitaph`.
    ///
    /// This function can be used to mock the starnix kernel in a way that simulates a component
    /// exiting with or without error.
    ///
    /// # Parameters
    /// - `component_controller_epitaph`: The epitaph used to close the component controller.
    ///
    /// # Returns
    /// A `ComponentRunnerProxy` that serves each run request by closing the component with the
    /// provided epitaph.
    fn spawn_runner(
        component_controller_epitaph: Result<(), zx::Status>,
    ) -> frunner::ComponentRunnerProxy {
        let (proxy, mut request_stream) =
            fidl::endpoints::create_proxy_and_stream::<frunner::ComponentRunnerMarker>();
        fasync::Task::local(async move {
            while let Some(event) =
                request_stream.try_next().await.expect("Error in test runner request stream")
            {
                match event {
                    frunner::ComponentRunnerRequest::Start {
                        start_info: _start_info,
                        controller,
                        ..
                    } => {
                        controller
                            .close_with_epitaph(component_controller_epitaph)
                            .expect("Could not close with epitaph");
                    }
                    frunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                        log::warn!(ordinal:%; "Unknown ComponentRunner request");
                    }
                }
            }
        })
        .detach();
        proxy
    }

    /// Returns the status from the first test case reported to `run_listener_stream`.
    ///
    /// This is done by listening to the first `CaseListener` provided via `OnTestCaseStarted`.
    ///
    /// # Parameters
    /// - `run_listener_stream`: The run listener stream to extract the test status from.
    ///
    /// # Returns
    /// The status of the first test case that is run, or `None` if no such status is reported.
    async fn listen_to_test_result(
        mut run_listener_stream: ftest::RunListenerRequestStream,
    ) -> Option<ftest::Status> {
        match run_listener_stream.try_next().await.expect("..") {
            Some(ftest::RunListenerRequest::OnTestCaseStarted {
                invocation: _,
                std_handles: _,
                listener,
                ..
            }) => match listener
                .into_stream()
                .try_next()
                .await
                .expect("Failed to get case listener stream request")
            {
                Some(ftest::CaseListenerRequest::Finished { result, .. }) => result.status,
                _ => None,
            },
            _ => None,
        }
    }

    /// Spawns a task that calls `super::run_test_cases` with the provided `run_listener` and
    /// `runner_proxy`. The call is made with a mock test case.
    fn spawn_run_test_cases(
        run_listener: ClientEnd<ftest::RunListenerMarker>,
        component_runner: frunner::ComponentRunnerProxy,
    ) {
        fasync::Task::local(async move {
            let _ = run_test_case(
                ftest::Invocation {
                    name: Some("".to_string()),
                    tag: Some("".to_string()),
                    ..Default::default()
                },
                frunner::ComponentStartInfo { ns: Some(vec![]), ..Default::default() },
                &run_listener.into_proxy(),
                &component_runner,
            )
            .await;
        })
        .detach();
    }

    #[fuchsia::test]
    async fn test_number_of_test_cases() {
        let iterator_proxy = set_up_iterator("test");
        let first_result = iterator_proxy.get_next().await.expect("Didn't get first result");
        let second_result = iterator_proxy.get_next().await.expect("Didn't get second result");

        assert_eq!(first_result.len(), 1);
        assert_eq!(second_result.len(), 0);
    }

    #[fuchsia::test]
    async fn test_case_name() {
        let test_name = "test_name";
        let iterator_proxy = set_up_iterator(test_name);
        let result = iterator_proxy.get_next().await.expect("Didn't get first result");
        assert_eq!(result[0].name, Some(test_name.to_string()));
    }

    /// Tests that when starnix closes the component controller with an `OK` status, the test case
    /// passes.
    #[fuchsia::test]
    async fn test_component_controller_epitaph_ok() {
        let component_runner = spawn_runner(Ok(()));
        let (run_listener, run_listener_stream) =
            create_request_stream::<ftest::RunListenerMarker>();
        spawn_run_test_cases(run_listener, component_runner);
        assert_eq!(listen_to_test_result(run_listener_stream).await, Some(ftest::Status::Passed));
    }

    /// Tests that when starnix closes the component controller with an error status, the test case
    /// fails.
    #[fuchsia::test]
    async fn test_component_controller_epitaph_not_ok() {
        let component_runner = spawn_runner(Err(zx::Status::INTERNAL));
        let (run_listener, run_listener_stream) =
            create_request_stream::<ftest::RunListenerMarker>();
        spawn_run_test_cases(run_listener, component_runner);
        assert_eq!(listen_to_test_result(run_listener_stream).await, Some(ftest::Status::Failed));
    }
}
