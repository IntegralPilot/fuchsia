// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cancel::{Cancelled, NamedFutureExt, OrCancel};
use crate::connector::SuiteRunnerConnector;
use crate::diagnostics::{self, LogDisplayConfiguration};
use crate::outcome::{Outcome, RunTestSuiteError};
use crate::output::{self, RunReporter, Timestamp};
use crate::params::{RunParams, TestParams, TimeoutBehavior};
use crate::running_suite::{RunningSuite, WaitForStartArgs, run_suite_and_collect_logs};
use diagnostics_data::LogTextDisplayOptions;
use flex_client::ProxyHasDomain;
use flex_fuchsia_test_manager::{self as ftest_manager, SuiteRunnerProxy};
use futures::prelude::*;
use log::warn;
use std::io::Write;
use std::path::PathBuf;

struct RunState<'a> {
    run_params: &'a RunParams,
    final_outcome: Option<Outcome>,
    failed_suites: u32,
    timeout_occurred: bool,
    cancel_occurred: bool,
    internal_error_occurred: bool,
}

impl<'a> RunState<'a> {
    fn new(run_params: &'a RunParams) -> Self {
        Self {
            run_params,
            final_outcome: None,
            failed_suites: 0,
            timeout_occurred: false,
            cancel_occurred: false,
            internal_error_occurred: false,
        }
    }

    fn cancel_run(&mut self, final_outcome: Outcome) {
        self.final_outcome = Some(final_outcome);
        self.cancel_occurred = true;
    }

    fn record_next_outcome(&mut self, next_outcome: Outcome) {
        if next_outcome != Outcome::Passed {
            self.failed_suites += 1;
        }
        match &next_outcome {
            Outcome::Timedout => self.timeout_occurred = true,
            Outcome::Cancelled => self.cancel_occurred = true,
            Outcome::Error { origin } if origin.is_internal_error() => {
                self.internal_error_occurred = true;
            }
            Outcome::Passed
            | Outcome::Failed
            | Outcome::Inconclusive
            | Outcome::DidNotFinish
            | Outcome::Error { .. } => (),
        }

        self.final_outcome = match (self.final_outcome.take(), next_outcome) {
            (None, first_outcome) => Some(first_outcome),
            (Some(outcome), Outcome::Passed) => Some(outcome),
            (Some(_), failing_outcome) => Some(failing_outcome),
        };
    }

    fn should_stop_run(&mut self) -> bool {
        let stop_due_to_timeout = self.run_params.timeout_behavior
            == TimeoutBehavior::TerminateRemaining
            && self.timeout_occurred;
        let stop_due_to_failures = match self.run_params.stop_after_failures.as_ref() {
            Some(threshold) => self.failed_suites >= threshold.get(),
            None => false,
        };
        stop_due_to_timeout
            || stop_due_to_failures
            || self.cancel_occurred
            || self.internal_error_occurred
    }

    fn final_outcome(self) -> Outcome {
        self.final_outcome.unwrap_or(Outcome::Passed)
    }
}

/// Schedule and run the tests specified in |test_params|, and collect the results.
/// Note this currently doesn't record the result or call finished() on run_reporter,
/// the caller should do this instead.
async fn run_test_chunk<'a, F: 'a + Future<Output = ()> + Unpin>(
    runner_proxy: SuiteRunnerProxy,
    test_params: TestParams,
    run_state: &'a mut RunState<'_>,
    run_params: &'a RunParams,
    run_reporter: &'a RunReporter,
    cancel_fut: F,
) -> Result<(), RunTestSuiteError> {
    let timeout = test_params
        .timeout_seconds
        .map(|seconds| std::time::Duration::from_secs(seconds.get() as u64));

    // If the test spec includes minimum log severity, combine that with any selectors we
    // got from the command line.
    let mut combined_log_interest = run_params.min_severity_logs.clone();
    combined_log_interest.extend(test_params.min_severity_logs.iter().cloned());

    let mut run_options = flex_fuchsia_test_manager::RunSuiteOptions {
        max_concurrent_test_case_runs: test_params.parallel,
        arguments: Some(test_params.test_args),
        timeout: timeout.map(|duration| duration.as_nanos() as i64),
        run_disabled_tests: Some(test_params.also_run_disabled_tests),
        test_case_filters: test_params.test_filters,
        break_on_failure: Some(test_params.break_on_failure),
        logs_iterator_type: Some(
            run_params.log_protocol.unwrap_or_else(diagnostics::get_logs_iterator_type),
        ),
        log_interest: Some(combined_log_interest),
        no_exception_channel: Some(test_params.no_exception_channel),
        ..Default::default()
    };
    let suite = run_reporter.new_suite(&test_params.test_url)?;
    suite.set_tags(test_params.tags);
    if let Some(realm) = test_params.realm.as_ref() {
        run_options.realm_options = Some(flex_fuchsia_test_manager::RealmOptions {
            realm: Some(realm.get_realm_client()?),
            offers: Some(realm.offers()),
            test_collection: Some(realm.collection().to_string()),
            ..Default::default()
        });
    }
    let (suite_controller, suite_server_end) =
        runner_proxy.domain().create_proxy::<ftest_manager::SuiteControllerMarker>();
    let suite_start_fut = RunningSuite::wait_for_start(WaitForStartArgs {
        proxy: suite_controller,
        max_severity_logs: test_params.max_severity_logs,
        timeout,
        timeout_grace: std::time::Duration::from_secs(run_params.timeout_grace_seconds as u64),
        max_pipelined: None,
        no_cases_equals_success: test_params.no_cases_equals_success,
    });

    runner_proxy.run(&test_params.test_url, run_options, suite_server_end)?;

    let cancel_fut = cancel_fut.shared();

    let handle_suite_fut = async move {
        // There's only one suite, but we loop so we can use break for flow control.
        'block: {
            let suite_stop_fut = cancel_fut.clone().map(|_| Outcome::Cancelled);

            let running_suite =
                match suite_start_fut.named("suite_start").or_cancelled(suite_stop_fut).await {
                    Ok(running_suite) => running_suite,
                    Err(Cancelled(final_outcome)) => {
                        run_state.cancel_run(final_outcome);
                        break 'block;
                    }
                };

            let log_display = LogDisplayConfiguration {
                interest: run_params.min_severity_logs.clone(),
                text_options: LogTextDisplayOptions {
                    show_full_moniker: run_params.show_full_moniker,
                    ..Default::default()
                },
            };

            let result =
                run_suite_and_collect_logs(running_suite, &suite, log_display, cancel_fut.clone())
                    .await;
            let suite_outcome = result.unwrap_or_else(|err| Outcome::error(err));
            // We should always persist results, even if something failed.
            suite.finished()?;
            run_state.record_next_outcome(suite_outcome);
            if run_state.should_stop_run() {
                break 'block;
            }
        }
        Result::<_, RunTestSuiteError>::Ok(())
    };

    handle_suite_fut.boxed_local().await.map_err(|e| e.into())
}

async fn run_tests<'a, F: 'a + Future<Output = ()> + Unpin>(
    connector: impl SuiteRunnerConnector,
    test_params: TestParams,
    run_params: RunParams,
    run_reporter: &'a RunReporter,
    cancel_fut: F,
) -> Result<Outcome, RunTestSuiteError> {
    let mut run_state = RunState::new(&run_params);
    let cancel_fut = cancel_fut.shared();
    match run_state.should_stop_run() {
        true => {
            // This indicates we need to terminate early. The suite wasn't recorded at all,
            // so we need to drain and record it wasn't started.
            let suite_reporter = run_reporter.new_suite(&test_params.test_url)?;
            suite_reporter.set_tags(test_params.tags);
            suite_reporter.finished()?;
        }
        false => {
            let runner_proxy = connector.connect().await?;
            run_test_chunk(
                runner_proxy,
                test_params,
                &mut run_state,
                &run_params,
                run_reporter,
                cancel_fut.clone(),
            )
            .await?;
        }
    }

    Ok(run_state.final_outcome())
}

/// Runs test specified in |test_params| and reports the results to
/// |run_reporter|.
///
/// Options specifying how the test run is executed are passed in via |run_params|.
/// Options specific to how a single suite is run are passed in via the entry for
/// the suite in |test_params|.
/// |cancel_fut| is used to gracefully stop execution of tests. Tests are
/// terminated and recorded when the future resolves. The caller can control when the
/// future resolves by passing in the receiver end of a `future::channel::oneshot`
/// channel.
pub async fn run_test_and_get_outcome<F>(
    connector: impl SuiteRunnerConnector,
    test_params: TestParams,
    run_params: RunParams,
    run_reporter: RunReporter,
    cancel_fut: F,
) -> Outcome
where
    F: Future<Output = ()>,
{
    match run_reporter.started(Timestamp::Unknown) {
        Ok(()) => (),
        Err(e) => return Outcome::error(e),
    }
    let test_outcome = match run_tests(
        connector,
        test_params,
        run_params,
        &run_reporter,
        cancel_fut.boxed_local(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            return Outcome::error(e);
        }
    };

    let report_result = match run_reporter.stopped(&test_outcome.clone().into(), Timestamp::Unknown)
    {
        Ok(()) => run_reporter.finished(),
        Err(e) => Err(e),
    };
    if let Err(e) = report_result {
        warn!("Failed to record results: {:?}", e);
    }

    test_outcome
}

pub struct DirectoryReporterOptions {
    /// Root path of the directory.
    pub root_path: PathBuf,
}

/// Create a reporter for use with |run_tests_and_get_outcome|.
pub fn create_reporter<W: 'static + Write + Send + Sync>(
    filter_ansi: bool,
    dir: Option<DirectoryReporterOptions>,
    writer: W,
) -> Result<output::RunReporter, anyhow::Error> {
    let stdout_reporter = output::ShellReporter::new(writer);
    let dir_reporter = dir
        .map(|dir| {
            output::DirectoryWithStdoutReporter::new(dir.root_path, output::SchemaVersion::V1)
        })
        .transpose()?;
    let reporter = match (dir_reporter, filter_ansi) {
        (Some(dir_reporter), false) => output::RunReporter::new(output::MultiplexedReporter::new(
            stdout_reporter,
            dir_reporter,
        )),
        (Some(dir_reporter), true) => output::RunReporter::new_ansi_filtered(
            output::MultiplexedReporter::new(stdout_reporter, dir_reporter),
        ),
        (None, false) => output::RunReporter::new(stdout_reporter),
        (None, true) => output::RunReporter::new_ansi_filtered(stdout_reporter),
    };
    Ok(reporter)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::connector::SingleRunConnector;
    use crate::output::{EntityId, InMemoryReporter};
    use assert_matches::assert_matches;
    use fidl::endpoints::{Proxy, create_proxy_and_stream};
    use flex_fuchsia_test_manager as ftest_manager;
    use futures::future::join;
    use futures::stream::futures_unordered::FuturesUnordered;
    use maplit::hashmap;
    use std::collections::HashMap;
    #[cfg(target_os = "fuchsia")]
    use {
        flex_fuchsia_io as fio, futures::future::join3, vfs::file::vmo::read_only,
        vfs::pseudo_directory, zx,
    };

    // TODO(https://fxbug.dev/42180532): add unit tests for suite artifacts too.

    async fn fake_running_all_suites(
        mut stream: ftest_manager::SuiteRunnerRequestStream,
        mut suite_events: HashMap<&str, Vec<ftest_manager::Event>>,
    ) {
        let mut suite_streams = vec![];

        if let Ok(Some(req)) = stream.try_next().await {
            match req {
                ftest_manager::SuiteRunnerRequest::Run { test_suite_url, controller, .. } => {
                    let events = suite_events
                        .remove(test_suite_url.as_str())
                        .expect("Got a request for an unexpected test URL");
                    suite_streams.push((controller.into_stream(), events));
                }
                ftest_manager::SuiteRunnerRequest::_UnknownMethod { ordinal, .. } => {
                    panic!("Not expecting unknown request: {}", ordinal)
                }
            }
        }
        assert!(suite_events.is_empty(), "Expected AddSuite to be called for all specified suites");

        // Each suite just reports that it started and passed.
        let mut suite_streams = suite_streams
            .into_iter()
            .map(|(mut stream, events)| {
                async move {
                    let mut maybe_events = Some(events);
                    while let Ok(Some(req)) = stream.try_next().await {
                        match req {
                            ftest_manager::SuiteControllerRequest::WatchEvents {
                                responder,
                                ..
                            } => {
                                let send_events = maybe_events.take().unwrap_or(vec![]);
                                let _ = responder.send(Ok(send_events));
                            }
                            _ => {
                                // ignore all other requests
                            }
                        }
                    }
                }
            })
            .collect::<FuturesUnordered<_>>();

        async move { while let Some(_) = suite_streams.next().await {} }.await;
    }

    struct ParamsForRunTests {
        runner_proxy: ftest_manager::SuiteRunnerProxy,
        test_params: TestParams,
        run_reporter: RunReporter,
    }

    fn create_empty_suite_events() -> Vec<ftest_manager::Event> {
        vec![
            ftest_manager::Event {
                timestamp: Some(1000),
                details: Some(ftest_manager::EventDetails::SuiteStarted(
                    ftest_manager::SuiteStartedEventDetails { ..Default::default() },
                )),
                ..Default::default()
            },
            ftest_manager::Event {
                timestamp: Some(2000),
                details: Some(ftest_manager::EventDetails::SuiteStopped(
                    ftest_manager::SuiteStoppedEventDetails {
                        result: Some(ftest_manager::SuiteResult::Finished),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        ]
    }

    async fn call_run_tests(params: ParamsForRunTests) -> Outcome {
        run_test_and_get_outcome(
            SingleRunConnector::new(params.runner_proxy),
            params.test_params,
            RunParams {
                timeout_behavior: TimeoutBehavior::Continue,
                timeout_grace_seconds: 0,
                stop_after_failures: None,
                accumulate_debug_data: false,
                log_protocol: None,
                min_severity_logs: vec![],
                show_full_moniker: false,
            },
            params.run_reporter,
            futures::future::pending(),
        )
        .await
    }

    #[fuchsia::test]
    async fn single_run_no_events() {
        let (runner_proxy, suite_runner_stream) =
            create_proxy_and_stream::<ftest_manager::SuiteRunnerMarker>();

        let reporter = InMemoryReporter::new();
        let run_reporter = RunReporter::new(reporter.clone());
        let run_fut = call_run_tests(ParamsForRunTests {
            runner_proxy,
            test_params: TestParams {
                test_url: "fuchsia-pkg://fuchsia.com/nothing#meta/nothing.cm".to_string(),
                ..TestParams::default()
            },
            run_reporter,
        });
        let fake_fut = fake_running_all_suites(
            suite_runner_stream,
            hashmap! {
                "fuchsia-pkg://fuchsia.com/nothing#meta/nothing.cm" => create_empty_suite_events()
            },
        );

        assert_eq!(join(run_fut, fake_fut).await.0, Outcome::Passed,);

        let reports = reporter.get_reports();
        assert_eq!(2usize, reports.len());
        assert!(reports[0].report.artifacts.is_empty());
        assert!(reports[0].report.directories.is_empty());
        assert!(reports[1].report.artifacts.is_empty());
        assert!(reports[1].report.directories.is_empty());
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    async fn single_run_custom_directory() {
        let (runner_proxy, suite_runner_stream) =
            create_proxy_and_stream::<ftest_manager::SuiteRunnerMarker>();

        let reporter = InMemoryReporter::new();
        let run_reporter = RunReporter::new(reporter.clone());
        let run_fut = call_run_tests(ParamsForRunTests {
            runner_proxy,
            test_params: TestParams {
                test_url: "fuchsia-pkg://fuchsia.com/nothing#meta/nothing.cm".to_string(),
                ..TestParams::default()
            },
            run_reporter,
        });

        let dir = pseudo_directory! {
            "test_file.txt" => read_only("Hello, World!"),
        };

        let directory_proxy = vfs::directory::serve(
            dir,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );

        let directory_client =
            fidl::endpoints::ClientEnd::new(directory_proxy.into_channel().unwrap().into());

        let (_pair_1, pair_2) = zx::EventPair::create();

        let events = vec![
            ftest_manager::Event {
                timestamp: Some(1000),
                details: Some(ftest_manager::EventDetails::SuiteStarted(
                    ftest_manager::SuiteStartedEventDetails { ..Default::default() },
                )),
                ..Default::default()
            },
            ftest_manager::Event {
                details: Some(ftest_manager::EventDetails::SuiteArtifactGenerated(
                    ftest_manager::SuiteArtifactGeneratedEventDetails {
                        artifact: Some(ftest_manager::Artifact::Custom(
                            ftest_manager::CustomArtifact {
                                directory_and_token: Some(ftest_manager::DirectoryAndToken {
                                    directory: directory_client,
                                    token: pair_2,
                                }),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            ftest_manager::Event {
                timestamp: Some(2000),
                details: Some(ftest_manager::EventDetails::SuiteStopped(
                    ftest_manager::SuiteStoppedEventDetails {
                        result: Some(ftest_manager::SuiteResult::Finished),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        ];

        let fake_fut = fake_running_all_suites(
            suite_runner_stream,
            hashmap! {
                "fuchsia-pkg://fuchsia.com/nothing#meta/nothing.cm" => events
            },
        );

        assert_eq!(join(run_fut, fake_fut).await.0, Outcome::Passed,);

        let reports = reporter.get_reports();
        assert_eq!(2usize, reports.len());
        let run = reports.iter().find(|e| e.id == EntityId::Suite).expect("find run report");
        assert_eq!(1usize, run.report.directories.len());
        let dir = &run.report.directories[0];
        let files = dir.1.files.lock();
        assert_eq!(1usize, files.len());
        let (name, file) = &files[0];
        assert_eq!(name.to_string_lossy(), "test_file.txt".to_string());
        assert_eq!(file.get_contents(), b"Hello, World!");
    }

    #[fuchsia::test]
    async fn record_output_after_internal_error() {
        let (runner_proxy, suite_runner_stream) =
            create_proxy_and_stream::<ftest_manager::SuiteRunnerMarker>();

        let reporter = InMemoryReporter::new();
        let run_reporter = RunReporter::new(reporter.clone());
        let run_fut = call_run_tests(ParamsForRunTests {
            runner_proxy,
            test_params: TestParams {
                test_url: "fuchsia-pkg://fuchsia.com/invalid#meta/invalid.cm".to_string(),
                ..TestParams::default()
            },
            run_reporter,
        });

        let fake_fut = fake_running_all_suites(
            suite_runner_stream,
            hashmap! {
                // return an internal error from the test.
                "fuchsia-pkg://fuchsia.com/invalid#meta/invalid.cm" => vec![
                    ftest_manager::Event {
                        timestamp: Some(1000),
                        details: Some(ftest_manager::EventDetails::SuiteStarted(ftest_manager::SuiteStartedEventDetails {..Default::default()})),
                        ..Default::default()
                    },
                    ftest_manager::Event {
                        timestamp: Some(2000),
                        details: Some(ftest_manager::EventDetails::SuiteStopped(ftest_manager::SuiteStoppedEventDetails {
                            result: Some(ftest_manager::SuiteResult::InternalError),
                            ..Default::default()
                        },
                        )),
                        ..Default::default()
                    },
                ],
            },
        );

        assert_matches!(join(run_fut, fake_fut).await.0, Outcome::Error { .. });

        let reports = reporter.get_reports();
        assert_eq!(2usize, reports.len());
        let invalid_suite = reports
            .iter()
            .find(|e| e.report.name == "fuchsia-pkg://fuchsia.com/invalid#meta/invalid.cm")
            .expect("find run report");
        assert_eq!(invalid_suite.report.outcome, Some(output::ReportedOutcome::Error));
        assert!(invalid_suite.report.is_finished);

        // The results for the run should also be saved.
        let run = reports.iter().find(|e| e.id == EntityId::TestRun).expect("find run report");
        assert_eq!(run.report.outcome, Some(output::ReportedOutcome::Error));
        assert!(run.report.is_finished);
        assert!(run.report.started_time.is_some());
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    async fn single_run_debug_data() {
        let (runner_proxy, suite_runner_stream) =
            create_proxy_and_stream::<ftest_manager::SuiteRunnerMarker>();

        let reporter = InMemoryReporter::new();
        let run_reporter = RunReporter::new(reporter.clone());
        let run_fut = call_run_tests(ParamsForRunTests {
            runner_proxy,
            test_params: TestParams {
                test_url: "fuchsia-pkg://fuchsia.com/nothing#meta/nothing.cm".to_string(),
                ..TestParams::default()
            },
            run_reporter,
        });

        let (debug_client, debug_service) =
            fidl::endpoints::create_endpoints::<ftest_manager::DebugDataIteratorMarker>();
        let debug_data_fut = async move {
            let (client, server) = zx::Socket::create_stream();
            let mut compressor = zstd::bulk::Compressor::new(0).unwrap();
            let bytes = compressor.compress(b"Not a real profile").unwrap();
            let _ = server.write(bytes.as_slice()).unwrap();
            let mut service = debug_service.into_stream();
            let mut data = vec![ftest_manager::DebugData {
                name: Some("test_file.profraw".to_string()),
                socket: Some(client.into()),
                ..Default::default()
            }];
            drop(server);
            while let Ok(Some(request)) = service.try_next().await {
                match request {
                    ftest_manager::DebugDataIteratorRequest::GetNext { .. } => {
                        panic!("Not Implemented");
                    }
                    ftest_manager::DebugDataIteratorRequest::GetNextCompressed {
                        responder,
                        ..
                    } => {
                        let _ = responder.send(std::mem::take(&mut data));
                    }
                }
            }
        };

        let events = vec![
            ftest_manager::Event {
                timestamp: Some(1000),
                details: Some(ftest_manager::EventDetails::SuiteStarted(
                    ftest_manager::SuiteStartedEventDetails { ..Default::default() },
                )),
                ..Default::default()
            },
            ftest_manager::Event {
                details: Some(ftest_manager::EventDetails::SuiteArtifactGenerated(
                    ftest_manager::SuiteArtifactGeneratedEventDetails {
                        artifact: Some(ftest_manager::Artifact::DebugData(debug_client)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            ftest_manager::Event {
                timestamp: Some(2000),
                details: Some(ftest_manager::EventDetails::SuiteStopped(
                    ftest_manager::SuiteStoppedEventDetails {
                        result: Some(ftest_manager::SuiteResult::Finished),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        ];

        let fake_fut = fake_running_all_suites(
            suite_runner_stream,
            hashmap! {
                "fuchsia-pkg://fuchsia.com/nothing#meta/nothing.cm" => events,
            },
        );

        assert_eq!(join3(run_fut, debug_data_fut, fake_fut).await.0, Outcome::Passed);

        let reports = reporter.get_reports();
        assert_eq!(2usize, reports.len());
        let run = reports.iter().find(|e| e.id == EntityId::Suite).expect("find run report");
        assert_eq!(1usize, run.report.directories.len());
        let dir = &run.report.directories[0];
        let files = dir.1.files.lock();
        assert_eq!(1usize, files.len());
        let (name, file) = &files[0];
        assert_eq!(name.to_string_lossy(), "test_file.profraw".to_string());
        assert_eq!(file.get_contents(), b"Not a real profile");
    }
}
