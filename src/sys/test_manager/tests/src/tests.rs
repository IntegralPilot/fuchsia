// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl::endpoints;
use fidl_fuchsia_test_manager as ftest_manager;
use ftest_manager::{CaseStatus, RunOptions, RunSuiteOptions, SuiteStatus};
use fuchsia_async as fasync;
use fuchsia_component::client;
use futures::channel::mpsc;
use futures::{StreamExt, stream};
use pretty_assertions::assert_eq;
use test_case::test_case;
use test_manager_test_lib::{
    AttributedLog, GroupRunEventByTestCase, RunEvent, SuiteRunner, TestBuilder,
    TestRunEventPayload, collect_string_from_socket_helper, collect_suite_events,
    collect_suite_events_with_watch, default_run_option, default_run_suite_options,
};

macro_rules! connect_run_builder {
    () => {
        client::connect_to_protocol::<ftest_manager::RunBuilderMarker>()
            .context("cannot connect to run builder proxy")
    };
}

macro_rules! connect_query_server {
    () => {
        client::connect_to_protocol::<ftest_manager::QueryMarker>()
            .context("cannot connect to query proxy")
    };
}

macro_rules! connect_suite_runner {
    () => {
        client::connect_to_protocol::<ftest_manager::SuiteRunnerMarker>()
            .context("cannot connect to suite runner proxy")
    };
}

macro_rules! connect_test_case_enumerator_server {
    () => {
        client::connect_to_protocol::<ftest_manager::TestCaseEnumeratorMarker>()
            .context("cannot connect to test case numerator proxy")
    };
}

#[allow(dead_code)]
async fn run_single_test(
    test_url: &str,
    run_options: RunOptions,
) -> Result<(Vec<RunEvent>, Vec<AttributedLog>), Error> {
    let builder = TestBuilder::new(connect_run_builder!()?);
    let suite_instance =
        builder.add_suite(test_url, run_options).await.context("Cannot create suite instance")?;
    let builder_run = fasync::Task::spawn(async move { builder.run().await });
    let ret = collect_suite_events(suite_instance).await;
    builder_run.await.context("builder execution failed")?;
    ret
}

#[allow(dead_code)]
async fn run_single_test_for_suite(
    test_url: &str,
    options: RunSuiteOptions,
) -> Result<(Vec<RunEvent>, Vec<AttributedLog>), Error> {
    let runner = SuiteRunner::new(connect_suite_runner!()?);
    let suite_instance = runner.start_suite_run(test_url, options).context("Cannot run suite")?;
    let ret = collect_suite_events_with_watch(suite_instance, false, true).await;
    // Drop the runner here so it stays alive through `collect_suite_events_with_watch`.
    drop(runner);
    ret
}

#[fuchsia::test]
async fn calling_kill_should_kill_test() {
    let proxy = connect_run_builder!().unwrap();
    let builder = TestBuilder::new(proxy);
    let suite = builder
        .add_suite(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/hanging_test.cm",
            default_run_option(),
        )
        .await
        .unwrap();
    let _builder_run = fasync::Task::spawn(async move { builder.run().await });
    let (sender, mut recv) = mpsc::channel(1024);

    let controller = suite.controller();
    let task = fasync::Task::spawn(async move { suite.collect_events(sender).await });
    // let the test start
    let _initial_event = recv.next().await.unwrap();
    controller.kill().unwrap();
    // collect rest of the events
    let events = recv.collect::<Vec<_>>().await;
    task.await.unwrap();
    let events = events
        .into_iter()
        .filter_map(|e| match e.payload {
            test_manager_test_lib::SuiteEventPayload::RunEvent(e) => Some(e),
            _ => None,
        })
        .collect::<Vec<_>>();
    // make sure that test never finished
    for event in events {
        match event {
            RunEvent::SuiteStopped { .. } => {
                panic!("should not receive SuiteStopped event as the test was killed. ")
            }
            _ => {}
        }
    }
}

#[fuchsia::test]
async fn closing_suite_controller_should_kill_test() {
    let proxy = connect_run_builder!().unwrap();
    let builder = TestBuilder::new(proxy);
    let suite = builder
        .add_suite(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/hanging_test.cm",
            default_run_option(),
        )
        .await
        .unwrap();
    let builder_run_task = fasync::Task::spawn(async move { builder.run().await });

    // let the test start
    // Drop events so that we are not holding on to logger channel else archivist will
    // timeout during shutdown.
    let _ = suite.controller().get_events().await.unwrap().unwrap();
    // drop suite, which should also close the suite channel
    drop(suite);

    // We can't verify that the suite didn't complete, but verify that the run
    // completes successfully.
    builder_run_task.await.expect("Run controller failed to collect events");
}

#[fuchsia::test]
async fn calling_builder_kill_should_kill_test() {
    let proxy = connect_run_builder!().unwrap();
    let builder = TestBuilder::new(proxy);
    let suite = builder
        .add_suite(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/hanging_test.cm",
            default_run_option(),
        )
        .await
        .unwrap();
    let proxy = builder.take_proxy();

    let (controller_proxy, controller) = endpoints::create_proxy();
    proxy.build(controller).unwrap();
    let (sender, mut recv) = mpsc::channel(1024);
    let _task = fasync::Task::spawn(async move { suite.collect_events(sender).await });
    // let the test start
    let _initial_event = recv.next().await.unwrap();
    controller_proxy.kill().unwrap();
    loop {
        let events = controller_proxy.get_events().await.unwrap();
        if events.is_empty() {
            break;
        }
        for event in events {
            if let Some(e) = &event.payload {
                match e {
                    // ignore, we sometimes see debugdata artifact
                    ftest_manager::RunEventPayload::Artifact(_) => {}
                    _ => panic!("should not get this event: {:?}", event),
                }
            }
        }
    }
    // collect rest of the events
    let events = recv.collect::<Vec<_>>().await;
    let events = events
        .into_iter()
        .filter_map(|e| match e.payload {
            test_manager_test_lib::SuiteEventPayload::RunEvent(e) => Some(e),
            _ => None,
        })
        .collect::<Vec<_>>();
    // make sure that test never finished
    for event in events {
        match event {
            RunEvent::SuiteStopped { .. } => {
                panic!("should not receive SuiteStopped event as the test was killed. ")
            }
            _ => {}
        }
    }
}

#[fuchsia::test]
async fn calling_stop_should_stop_test() {
    let proxy = connect_run_builder!().unwrap();
    let builder = TestBuilder::new(proxy);
    // can't use hanging test here as stop will only return once current running test completes.
    let suite = builder
        .add_suite(
            "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/huge_gtest.cm",
            default_run_option(),
        )
        .await
        .unwrap();
    let _builder_run = fasync::Task::spawn(async move { builder.run().await });
    let (sender, mut recv) = mpsc::channel(1024);

    let controller = suite.controller();
    let task = fasync::Task::spawn(async move { suite.collect_events(sender).await });
    // let the test start
    let _initial_event = recv.next().await.unwrap();
    controller.stop().unwrap();
    // collect rest of the events
    let events = recv.collect::<Vec<_>>().await;
    task.await.unwrap();
    // get suite finished event
    let events = events
        .into_iter()
        .filter_map(|e| match e.payload {
            test_manager_test_lib::SuiteEventPayload::RunEvent(e) => match e {
                RunEvent::SuiteStopped { .. } => Some(e),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(events, vec![RunEvent::suite_stopped(SuiteStatus::Stopped)]);
}

#[fuchsia::test]
async fn launch_and_test_subpackaged_test() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/subpackaged_echo_integration_test_cpp#meta/default.cm";
    let (events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("EchoIntegrationTest.TestEcho"),
        RunEvent::case_started("EchoIntegrationTest.TestEcho"),
        RunEvent::case_stopped("EchoIntegrationTest.TestEcho", CaseStatus::Passed),
        RunEvent::case_finished("EchoIntegrationTest.TestEcho"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);

    let test_url =
        "fuchsia-pkg://fuchsia.com/subpackaged_echo_integration_test_rust#meta/default.cm";
    let (events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("echo_integration_test"),
        RunEvent::case_started("echo_integration_test"),
        RunEvent::case_stopped("echo_integration_test", CaseStatus::Passed),
        RunEvent::case_finished("echo_integration_test"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn launch_and_test_echo_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/echo_test_realm.cm";
    let (events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("EchoTest"),
        RunEvent::case_started("EchoTest"),
        RunEvent::case_stopped("EchoTest", CaseStatus::Passed),
        RunEvent::case_finished("EchoTest"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn launch_and_test_no_on_finished() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_test#meta/no-onfinished-after-test-example.cm";

    let (events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let test_cases = ["Example.Test1", "Example.Test2", "Example.Test3"];
    let mut expected_events = vec![RunEvent::suite_started()];
    for case in test_cases {
        expected_events.push(RunEvent::case_found(case));
        expected_events.push(RunEvent::case_started(case));

        for i in 1..=3 {
            expected_events.push(RunEvent::case_stdout(case, format!("log{} for {}", i, case)));
        }
        expected_events.push(RunEvent::case_stopped(case, CaseStatus::Passed));
        expected_events.push(RunEvent::case_finished(case));
    }
    expected_events.push(RunEvent::suite_stopped(SuiteStatus::DidNotFinish));
    let expected_events = expected_events.into_iter().group_by_test_case_unordered();

    assert_eq!(&expected_events, &events);
    assert_eq!(logs, Vec::new());
}

#[fuchsia::test]
async fn launch_and_test_gtest_runner_sample_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";

    let (events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let expected_events =
        include!("../../../test_runners/gtest/test_data/sample_tests_golden_events.rsf")
            .into_iter()
            .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn launch_and_test_isolated_data() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/data_storage_test.cm";

    let (events, _logs) = run_single_test(test_url, default_run_option()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("test_data_storage"),
        RunEvent::case_started("test_data_storage"),
        RunEvent::case_stopped("test_data_storage", CaseStatus::Passed),
        RunEvent::case_finished("test_data_storage"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn positive_filter_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_option();
    options.case_filters_to_run =
        Some(vec!["SampleTest2.SimplePass".into(), "SampleFixture*".into()]);
    let (events, logs) = run_single_test(test_url, options).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("SampleTest2.SimplePass"),
        RunEvent::case_started("SampleTest2.SimplePass"),
        RunEvent::case_stopped("SampleTest2.SimplePass", CaseStatus::Passed),
        RunEvent::case_finished("SampleTest2.SimplePass"),
        RunEvent::case_found("SampleFixture.Test1"),
        RunEvent::case_started("SampleFixture.Test1"),
        RunEvent::case_stopped("SampleFixture.Test1", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test1"),
        RunEvent::case_found("SampleFixture.Test2"),
        RunEvent::case_started("SampleFixture.Test2"),
        RunEvent::case_stopped("SampleFixture.Test2", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test2"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn negative_filter_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_option();
    options.case_filters_to_run = Some(vec![
        "-SampleTest1.*".into(),
        "-Tests/SampleParameterizedTestFixture.*".into(),
        "-SampleDisabled.*".into(),
        "-WriteToStd.*".into(),
    ]);
    let (events, logs) = run_single_test(test_url, options).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("SampleTest2.SimplePass"),
        RunEvent::case_started("SampleTest2.SimplePass"),
        RunEvent::case_stopped("SampleTest2.SimplePass", CaseStatus::Passed),
        RunEvent::case_finished("SampleTest2.SimplePass"),
        RunEvent::case_found("SampleFixture.Test1"),
        RunEvent::case_started("SampleFixture.Test1"),
        RunEvent::case_stopped("SampleFixture.Test1", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test1"),
        RunEvent::case_found("SampleFixture.Test2"),
        RunEvent::case_started("SampleFixture.Test2"),
        RunEvent::case_stopped("SampleFixture.Test2", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test2"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn positive_and_negative_filter_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_option();
    options.case_filters_to_run =
        Some(vec!["SampleFixture.*".into(), "SampleTest2.*".into(), "-*Test1".into()]);
    let (events, logs) = run_single_test(test_url, options).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("SampleTest2.SimplePass"),
        RunEvent::case_started("SampleTest2.SimplePass"),
        RunEvent::case_stopped("SampleTest2.SimplePass", CaseStatus::Passed),
        RunEvent::case_finished("SampleTest2.SimplePass"),
        RunEvent::case_found("SampleFixture.Test2"),
        RunEvent::case_started("SampleFixture.Test2"),
        RunEvent::case_stopped("SampleFixture.Test2", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test2"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn parallel_tests() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_option();
    options.parallel = Some(10);
    let (events, logs) = run_single_test(test_url, options).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let expected_events =
        include!("../../../test_runners/gtest/test_data/sample_tests_golden_events.rsf")
            .into_iter()
            .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn multiple_test() {
    let gtest_test_url =
        "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";

    let builder = TestBuilder::new(connect_run_builder!().unwrap());
    let gtest_suite_instance1 = builder
        .add_suite(gtest_test_url, default_run_option())
        .await
        .expect("Cannot create suite instance");
    let gtest_suite_instance2 = builder
        .add_suite(gtest_test_url, default_run_option())
        .await
        .expect("Cannot create suite instance");

    let builder_run = fasync::Task::spawn(async move { builder.run().await });
    let fut1 = collect_suite_events(gtest_suite_instance1);
    let fut2 = collect_suite_events(gtest_suite_instance2);
    let (ret1, ret2) = futures::join!(fut1, fut2);
    let (gtest_events1, gtest_log1) = ret1.unwrap();
    let (gtest_events2, gtest_log2) = ret2.unwrap();

    builder_run.await.expect("builder execution failed");

    let gtest_events1 = gtest_events1.into_iter().group_by_test_case_unordered();
    let gtest_events2 = gtest_events2.into_iter().group_by_test_case_unordered();

    let expected_events =
        include!("../../../test_runners/gtest/test_data/sample_tests_golden_events.rsf")
            .into_iter()
            .group_by_test_case_unordered();

    assert_eq!(gtest_log1, Vec::new());
    assert_eq!(gtest_log2, Vec::new());
    assert_eq!(&expected_events, &gtest_events1);
    assert_eq!(&expected_events, &gtest_events2);
}

#[fuchsia::test]
async fn no_suite_service_test() {
    let proxy = connect_run_builder!().unwrap();
    let builder = TestBuilder::new(proxy);
    let suite = builder
        .add_suite(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/no_suite_service.cm",
            default_run_option(),
        )
        .await
        .unwrap();
    let _builder_run = fasync::Task::spawn(async move { builder.run().await });
    let (sender, _recv) = mpsc::channel(1024);
    let err = suite
        .collect_events(sender)
        .await
        .expect_err("this should return instance not found error");
    let err = err.downcast::<test_manager_test_lib::SuiteLaunchError>().unwrap();
    // as test doesn't expose suite service, enumeration of test cases will fail.
    assert_eq!(err, test_manager_test_lib::SuiteLaunchError::CaseEnumeration);
}

#[fuchsia::test]
async fn test_not_resolved() {
    let proxy = connect_run_builder!().unwrap();
    let builder = TestBuilder::new(proxy);
    let suite = builder
        .add_suite(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/invalid_cml.cm",
            default_run_option(),
        )
        .await
        .unwrap();
    let _builder_run = fasync::Task::spawn(async move { builder.run().await });
    let (sender, _recv) = mpsc::channel(1024);
    let err = suite
        .collect_events(sender)
        .await
        .expect_err("this should return instance not found error");
    let err = err.downcast::<test_manager_test_lib::SuiteLaunchError>().unwrap();
    assert_eq!(err, test_manager_test_lib::SuiteLaunchError::InstanceCannotResolve);
}

#[fuchsia::test]
async fn collect_isolated_logs_using_default_log_iterator() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let (_events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_from_generated_component_manifest() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/logger-test-generated-manifest.cm";
    let (_events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["I'm a info log from a test"],
        "{logs:#?}",
    );
}

fn get_stdmsgs_and_status(events: &Vec<RunEvent>) -> (Vec<String>, Option<SuiteStatus>) {
    let mut stdmsgs = vec![];
    let mut s = None;
    for e in events {
        match e {
            RunEvent::CaseStdout { name, stdout_message } => {
                stdmsgs.push(format!("{} - {}", name, stdout_message));
            }
            RunEvent::CaseStderr { name, stderr_message } => {
                stdmsgs.push(format!("{} - {}", name, stderr_message));
            }

            RunEvent::SuiteStopped { status } => s = Some(status.clone()),
            _ => {
                continue;
            }
        }
    }
    (stdmsgs, s)
}

#[fuchsia::test]
async fn collect_isolated_logs_using_batch() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let mut options = default_run_option();
    options.log_iterator = Some(ftest_manager::LogsIteratorOption::BatchIterator);
    let (events, logs) = run_single_test(test_url, options).await.unwrap();

    let (stdmsgs, status) = get_stdmsgs_and_status(&events);
    assert_eq!(status, Some(SuiteStatus::Passed), "{logs:#?}\n{stdmsgs:#?}");
    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}\n{stdmsgs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_using_host_socket() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunOptions {
        log_iterator: Some(ftest_manager::LogsIteratorOption::SocketBatchIterator),
        ..default_run_option()
    };
    let (events, logs) = run_single_test(test_url, options).await.unwrap();
    let (stdmsgs, status) = get_stdmsgs_and_status(&events);
    assert_eq!(status, Some(SuiteStatus::Passed), "{logs:#?}\n{stdmsgs:#?}");
    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}\n{stdmsgs:#?}",
    );
}

#[fuchsia::test]
async fn update_log_severity_for_all_components() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunOptions {
        log_iterator: Some(ftest_manager::LogsIteratorOption::SocketBatchIterator),
        log_interest: Some(vec![
            selectors::parse_log_interest_selector_or_severity("DEBUG").unwrap(),
        ]),
        ..default_run_option()
    };
    let (_events, logs) = run_single_test(test_url, options).await.unwrap();
    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec![
            "I'm a debug log from a test",
            "Started diagnostics publisher",
            "I'm a debug log from the publisher!",
            "Finishing through Stop",
        ]
    );
}

#[fuchsia::test]
async fn update_log_severity_for_the_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunOptions {
        log_iterator: Some(ftest_manager::LogsIteratorOption::SocketBatchIterator),
        log_interest: Some(vec![
            selectors::parse_log_interest_selector_or_severity("<root>#DEBUG").unwrap(),
        ]),
        ..default_run_option()
    };
    let (_events, logs) = run_single_test(test_url, options).await.unwrap();
    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec![
            "I'm a debug log from a test",
            "Started diagnostics publisher",
            "Finishing through Stop",
        ]
    );
}

#[fuchsia::test]
async fn custom_artifact_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/custom_artifact_user.cm";

    let (events, _) = run_single_test(test_url, default_run_option()).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("use_artifact"),
        RunEvent::case_started("use_artifact"),
        RunEvent::case_stopped("use_artifact", CaseStatus::Passed),
        RunEvent::case_finished("use_artifact"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
        RunEvent::suite_custom(".", "artifact.txt", "Hello, world!"),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(&expected_events, &events);
}

#[test_case(true; "compressed debug_data")]
#[test_case(false; "uncompressed debug_data")]
#[fuchsia::test]
async fn debug_data_test(compressed: bool) {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/debug_data_write_test.cm";

    let builder = TestBuilder::new(connect_run_builder!().unwrap());
    let suite_instance = builder
        .add_suite(test_url, default_run_option())
        .await
        .expect("Cannot create suite instance");
    let (run_events_result, suite_events_result) = futures::future::join(
        builder.run_with_option(compressed),
        collect_suite_events(suite_instance),
    )
    .await;

    let suite_events = suite_events_result.unwrap().0;
    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("publish_debug_data"),
        RunEvent::case_started("publish_debug_data"),
        RunEvent::case_stopped("publish_debug_data", CaseStatus::Passed),
        RunEvent::case_finished("publish_debug_data"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(
        suite_events.into_iter().group_by_test_case_unordered(),
        expected_events.into_iter().group_by_test_case_unordered(),
    );

    let num_debug_data_events = stream::iter(run_events_result.unwrap())
        .then(|run_event| async move {
            let TestRunEventPayload::DebugData { socket, .. } = run_event.payload;
            let content = collect_string_from_socket_helper(socket, compressed).await.unwrap();
            content == "Debug data from test\n"
        })
        .filter(|matches_vmo| futures::future::ready(*matches_vmo))
        .count()
        .await;
    assert_eq!(num_debug_data_events, 1);
}

// Test that we can hook up DebugDataIterator connection for EarlyBootPrfile.
#[fuchsia::test]
async fn early_boot_profile_test() {
    let proxy = client::connect_to_protocol::<ftest_manager::EarlyBootProfileMarker>()
        .expect("cannot connect to run builder proxy");
    let (debug_data_proxy, iterator) = endpoints::create_proxy();
    proxy.register_watcher(iterator).unwrap();

    // We cannot check the returned value as it can be empty vector or bunch of socket connections.
    // But we can check that our call to DebugDataIterator goes through.
    let _ = debug_data_proxy.get_next_compressed().await.unwrap();
}

#[fuchsia::test]
async fn debug_data_accumulate_test() {
    // Two tests are combined here, because one would otherwise break the other due to the
    // accumulated files left behind.

    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/debug_data_write_test.cm";

    // This is the old `TestBuilder` test.
    {
        for iteration in 1usize..3 {
            let builder = TestBuilder::new(connect_run_builder!().unwrap());
            builder.set_scheduling_options(true).expect("set scheduling options");
            let suite_instance = builder
                .add_suite(test_url, default_run_option())
                .await
                .expect("Cannot create suite instance");
            let (run_events_result, _) = futures::future::join(
                builder.run_with_option(true),
                collect_suite_events(suite_instance),
            )
            .await;

            let num_debug_data_events = stream::iter(run_events_result.unwrap())
                .then(|run_event| async move {
                    let TestRunEventPayload::DebugData { socket, .. } = run_event.payload;
                    let content = collect_string_from_socket_helper(socket, true).await.unwrap();
                    content == "Debug data from test\n"
                })
                .filter(|matches_vmo| futures::future::ready(*matches_vmo))
                .count()
                .await;
            assert_eq!(num_debug_data_events, iteration);
        }
    }

    // This is the new `SuiteRunner` test.
    {
        for iteration in 1usize..3 {
            let runner = SuiteRunner::new(connect_suite_runner!().unwrap());
            let suite_run_instance = runner
                .start_suite_run(
                    test_url,
                    ftest_manager::RunSuiteOptions {
                        run_disabled_tests: Some(false),
                        accumulate_debug_data: Some(true),
                        ..Default::default()
                    },
                )
                .context("Cannot run suite")
                .unwrap();
            let suite_events_result =
                collect_suite_events_with_watch(suite_run_instance, false, true).await;
            let suite_events = suite_events_result.unwrap().0;

            let debug_event_count = stream::iter(suite_events)
                .then(|suite_event| async move {
                    if let RunEvent::DebugData { filename, socket } = suite_event {
                        if filename.starts_with("data_sink/data_sink.") {
                            let content =
                                collect_string_from_socket_helper(socket, true).await.unwrap();
                            assert_eq!(content, "Debug data from test\n");
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                })
                .filter(|matches_vmo| futures::future::ready(*matches_vmo))
                .count()
                .await;
            // Add 2 here to account for the files left behind by the `TestBuilder` test above.
            assert_eq!(debug_event_count, iteration + 2);
        }
    }
}

#[test_case(true; "compressed debug_data")]
#[test_case(false; "uncompressed debug_data")]
#[fuchsia::test]
async fn debug_data_isolated_test(compressed: bool) {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/debug_data_write_test.cm";
    // By default, when I run the same test twice, debug data is not accumulated.
    for _ in 0..2 {
        let builder = TestBuilder::new(connect_run_builder!().unwrap());
        let suite_instance = builder
            .add_suite(test_url, default_run_option())
            .await
            .expect("Cannot create suite instance");
        let (run_events_result, _) = futures::future::join(
            builder.run_with_option(compressed),
            collect_suite_events(suite_instance),
        )
        .await;

        let num_debug_data_events = stream::iter(run_events_result.unwrap())
            .then(|run_event| async move {
                let TestRunEventPayload::DebugData { socket, .. } = run_event.payload;
                let content = collect_string_from_socket_helper(socket, compressed).await.unwrap();
                content == "Debug data from test\n"
            })
            .filter(|matches_vmo| futures::future::ready(*matches_vmo))
            .count()
            .await;
        assert_eq!(num_debug_data_events, 1);
    }
}

#[fuchsia::test]
async fn custom_artifact_realm_test() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/custom_artifact_realm_test.cm";

    let (events, _) = run_single_test(test_url, default_run_option()).await.unwrap();
    let events = events.into_iter().group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("use_artifact"),
        RunEvent::case_started("use_artifact"),
        RunEvent::case_stopped("use_artifact", CaseStatus::Passed),
        RunEvent::case_finished("use_artifact"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
        RunEvent::suite_custom("test_driver", "artifact.txt", "Hello, world!"),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn enumerate_invalid_test() {
    let proxy = connect_query_server!().unwrap();
    let (_iterator, server_end) = endpoints::create_proxy();
    let err = proxy
        .enumerate("fuchsia-pkg://fuchsia.com/test_manager_test#meta/invalid_cml.cm", server_end)
        .await
        .unwrap()
        .expect_err("This should error out as we have invalid test");
    assert_eq!(err, ftest_manager::LaunchError::InstanceCannotResolve);
}

#[fuchsia::test]
async fn enumerate_echo_test() {
    let proxy = connect_query_server!().unwrap();
    let (iterator, server_end) = endpoints::create_proxy();
    proxy
        .enumerate(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/echo_test_realm.cm",
            server_end,
        )
        .await
        .unwrap()
        .expect("This should not fail");

    let mut cases = vec![];
    loop {
        let mut c = iterator.get_next().await.unwrap();
        if c.is_empty() {
            break;
        }
        cases.append(&mut c);
    }
    assert_eq!(
        cases.into_iter().map(|c| c.name.unwrap()).collect::<Vec<_>>(),
        vec!["EchoTest".to_string()]
    );
}

#[fuchsia::test]
async fn enumerate_huge_test() {
    let proxy = connect_query_server!().unwrap();
    let (iterator, server_end) = endpoints::create_proxy();
    proxy
        .enumerate(
            "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/huge_gtest.cm",
            server_end,
        )
        .await
        .unwrap()
        .expect("This should not fail");

    let mut cases = vec![];
    loop {
        let mut c = iterator.get_next().await.unwrap();
        if c.is_empty() {
            break;
        }
        cases.append(&mut c);
    }
    let expected_cases = (0..=999)
        .into_iter()
        .map(|n| format!("HugeStress/HugeTest.Test/{}", n))
        .collect::<Vec<_>>();

    assert_eq!(cases.into_iter().map(|c| c.name.unwrap()).collect::<Vec<_>>(), expected_cases);
}

#[fuchsia::test]
async fn capability_access_test() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_test#meta/nonhermetic_capability_test.cm";
    let (events, _) = run_single_test(test_url, default_run_option()).await.unwrap();

    let failed_suite = RunEvent::suite_stopped(SuiteStatus::Failed);

    assert!(events.contains(&failed_suite));
}

#[fuchsia::test]
async fn launch_chromium_test() {
    // TODO(https://fxbug.dev/42173552): This test is launched in the chromium
    // realm. Once we support out of tree realm definitions we should move the
    // definition and test to chromium.
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/simple_chromium_realm_test.cm";
    let (events, logs) = run_single_test(test_url, default_run_option()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("noop_test"),
        RunEvent::case_started("noop_test"),
        RunEvent::case_stopped("noop_test", CaseStatus::Passed),
        RunEvent::case_finished("noop_test"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn calling_kill_should_kill_test_for_suite() {
    let runner = SuiteRunner::new(connect_suite_runner!().unwrap());
    let suite_run_instance = runner
        .start_suite_run(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/hanging_test.cm",
            default_run_suite_options(),
        )
        .context("Cannot run suite")
        .unwrap();
    let (sender, mut recv) = mpsc::channel(1024);

    let controller = suite_run_instance.controller();
    let task = fasync::Task::spawn(async move {
        suite_run_instance.collect_events_with_watch(sender, false, false).await
    });
    // let the test start
    let _initial_event = recv.next().await.unwrap();
    controller.kill().unwrap();
    // collect rest of the events
    let events = recv.collect::<Vec<_>>().await;
    task.await.unwrap();
    let events = events
        .into_iter()
        .filter_map(|e| match e.payload {
            test_manager_test_lib::SuiteEventPayload::RunEvent(e) => Some(e),
            _ => None,
        })
        .collect::<Vec<_>>();
    // make sure that test never finished
    for event in events {
        match event {
            RunEvent::SuiteStopped { .. } => {
                panic!("should not receive SuiteStopped event as the test was killed. ")
            }
            _ => {}
        }
    }

    drop(runner);
}

#[fuchsia::test]
async fn calling_stop_should_stop_test_for_suite() {
    let runner = SuiteRunner::new(connect_suite_runner!().unwrap());
    let suite_run_instance = runner
        .start_suite_run(
            "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/huge_gtest.cm",
            default_run_suite_options(),
        )
        .context("Cannot run suite")
        .unwrap();
    let (sender, mut recv) = mpsc::channel(1024);

    let controller = suite_run_instance.controller();
    let task = fasync::Task::spawn(async move {
        suite_run_instance.collect_events_with_watch(sender, false, false).await
    });
    // let the test start
    let _initial_event = recv.next().await.unwrap();
    controller.stop().unwrap();
    // collect rest of the events
    let events = recv.collect::<Vec<_>>().await;
    task.await.unwrap();
    // get suite finished event
    let events = events
        .into_iter()
        .filter_map(|e| match e.payload {
            test_manager_test_lib::SuiteEventPayload::RunEvent(e) => match e {
                RunEvent::SuiteStopped { .. } => Some(e),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(events, vec![RunEvent::suite_stopped(SuiteStatus::Stopped)]);
}

#[fuchsia::test]
async fn launch_and_test_subpackaged_test_for_suite() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/subpackaged_echo_integration_test_cpp#meta/default.cm";
    let (events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("EchoIntegrationTest.TestEcho"),
        RunEvent::case_started("EchoIntegrationTest.TestEcho"),
        RunEvent::case_stopped("EchoIntegrationTest.TestEcho", CaseStatus::Passed),
        RunEvent::case_finished("EchoIntegrationTest.TestEcho"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events.into_iter().filter(not_debug_data).collect::<Vec<_>>());

    let test_url =
        "fuchsia-pkg://fuchsia.com/subpackaged_echo_integration_test_rust#meta/default.cm";
    let (events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("echo_integration_test"),
        RunEvent::case_started("echo_integration_test"),
        RunEvent::case_stopped("echo_integration_test", CaseStatus::Passed),
        RunEvent::case_finished("echo_integration_test"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events.into_iter().filter(not_debug_data).collect::<Vec<_>>());
}

#[fuchsia::test]
async fn launch_and_test_echo_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/echo_test_realm.cm";
    let (events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("EchoTest"),
        RunEvent::case_started("EchoTest"),
        RunEvent::case_stopped("EchoTest", CaseStatus::Passed),
        RunEvent::case_finished("EchoTest"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events.into_iter().filter(not_debug_data).collect::<Vec<_>>());
}

#[fuchsia::test]
async fn launch_and_test_no_on_finished_for_suite() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_test#meta/no-onfinished-after-test-example.cm";

    let (events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let test_cases = ["Example.Test1", "Example.Test2", "Example.Test3"];
    let mut expected_events = vec![RunEvent::suite_started()];
    for case in test_cases {
        expected_events.push(RunEvent::case_found(case));
        expected_events.push(RunEvent::case_started(case));

        for i in 1..=3 {
            expected_events.push(RunEvent::case_stdout(case, format!("log{} for {}", i, case)));
        }
        expected_events.push(RunEvent::case_stopped(case, CaseStatus::Passed));
        expected_events.push(RunEvent::case_finished(case));
    }
    expected_events.push(RunEvent::suite_stopped(SuiteStatus::DidNotFinish));
    let expected_events = expected_events.into_iter().group_by_test_case_unordered();

    assert_eq!(&expected_events, &events);
    assert_eq!(logs, Vec::new());
}

#[fuchsia::test]
async fn launch_and_test_gtest_runner_sample_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";

    let (events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();
    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let expected_events =
        include!("../../../test_runners/gtest/test_data/sample_tests_golden_events.rsf")
            .into_iter()
            .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn launch_and_test_isolated_data_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/data_storage_test.cm";

    let (events, _logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("test_data_storage"),
        RunEvent::case_started("test_data_storage"),
        RunEvent::case_stopped("test_data_storage", CaseStatus::Passed),
        RunEvent::case_finished("test_data_storage"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(&expected_events, &events.into_iter().filter(not_debug_data).collect::<Vec<_>>());
}

#[fuchsia::test]
async fn positive_filter_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_suite_options();
    options.test_case_filters =
        Some(vec!["SampleTest2.SimplePass".into(), "SampleFixture*".into()]);
    let (events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();
    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("SampleTest2.SimplePass"),
        RunEvent::case_started("SampleTest2.SimplePass"),
        RunEvent::case_stopped("SampleTest2.SimplePass", CaseStatus::Passed),
        RunEvent::case_finished("SampleTest2.SimplePass"),
        RunEvent::case_found("SampleFixture.Test1"),
        RunEvent::case_started("SampleFixture.Test1"),
        RunEvent::case_stopped("SampleFixture.Test1", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test1"),
        RunEvent::case_found("SampleFixture.Test2"),
        RunEvent::case_started("SampleFixture.Test2"),
        RunEvent::case_stopped("SampleFixture.Test2", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test2"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn negative_filter_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_suite_options();
    options.test_case_filters = Some(vec![
        "-SampleTest1.*".into(),
        "-Tests/SampleParameterizedTestFixture.*".into(),
        "-SampleDisabled.*".into(),
        "-WriteToStd.*".into(),
    ]);
    let (events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();
    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("SampleTest2.SimplePass"),
        RunEvent::case_started("SampleTest2.SimplePass"),
        RunEvent::case_stopped("SampleTest2.SimplePass", CaseStatus::Passed),
        RunEvent::case_finished("SampleTest2.SimplePass"),
        RunEvent::case_found("SampleFixture.Test1"),
        RunEvent::case_started("SampleFixture.Test1"),
        RunEvent::case_stopped("SampleFixture.Test1", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test1"),
        RunEvent::case_found("SampleFixture.Test2"),
        RunEvent::case_started("SampleFixture.Test2"),
        RunEvent::case_stopped("SampleFixture.Test2", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test2"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn positive_and_negative_filter_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_suite_options();
    options.test_case_filters =
        Some(vec!["SampleFixture.*".into(), "SampleTest2.*".into(), "-*Test1".into()]);
    let (events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();
    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("SampleTest2.SimplePass"),
        RunEvent::case_started("SampleTest2.SimplePass"),
        RunEvent::case_stopped("SampleTest2.SimplePass", CaseStatus::Passed),
        RunEvent::case_finished("SampleTest2.SimplePass"),
        RunEvent::case_found("SampleFixture.Test2"),
        RunEvent::case_started("SampleFixture.Test2"),
        RunEvent::case_stopped("SampleFixture.Test2", CaseStatus::Passed),
        RunEvent::case_finished("SampleFixture.Test2"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn parallel_tests_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/sample_tests.cm";
    let mut options = default_run_suite_options();
    options.max_concurrent_test_case_runs = Some(10);
    let (events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();
    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let expected_events =
        include!("../../../test_runners/gtest/test_data/sample_tests_golden_events.rsf")
            .into_iter()
            .group_by_test_case_unordered();

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn no_suite_service_test_for_suite() {
    let runner = SuiteRunner::new(connect_suite_runner!().unwrap());
    let suite_run_instance = runner
        .start_suite_run(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/no_suite_service.cm",
            default_run_suite_options(),
        )
        .context("Cannot run suite")
        .unwrap();
    let (sender, _recv) = mpsc::channel(1024);
    let err = suite_run_instance
        .collect_events(sender)
        .await
        .expect_err("this should return instance not found error");
    let err = err.downcast::<test_manager_test_lib::SuiteLaunchError>().unwrap();
    // as test doesn't expose suite service, enumeration of test cases will fail.
    assert_eq!(err, test_manager_test_lib::SuiteLaunchError::CaseEnumeration);
}

#[fuchsia::test]
async fn test_not_resolved_for_suite() {
    let runner = SuiteRunner::new(connect_suite_runner!().unwrap());
    let suite_run_instance = runner
        .start_suite_run(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/invalid_cml.cm",
            default_run_suite_options(),
        )
        .context("Cannot run suite")
        .unwrap();
    let (sender, _recv) = mpsc::channel(1024);
    let err = suite_run_instance
        .collect_events(sender)
        .await
        .expect_err("this should return instance not found error");
    let err = err.downcast::<test_manager_test_lib::SuiteLaunchError>().unwrap();
    assert_eq!(err, test_manager_test_lib::SuiteLaunchError::InstanceCannotResolve);
}

#[fuchsia::test]
async fn collect_isolated_logs_using_default_log_iterator_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let (_events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_from_generated_component_manifest_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/logger-test-generated-manifest.cm";
    let (_events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["I'm a info log from a test"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_using_invalid_iterator_type_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunSuiteOptions {
        logs_iterator_type: Some(ftest_manager::LogsIteratorType::__SourceBreaking {
            unknown_ordinal: 666,
        }),
        ..default_run_suite_options()
    };
    let err = run_single_test_for_suite(test_url, options)
        .await
        .expect_err("this should return invalid args error");

    let err = err.downcast::<fidl::Error>().expect("expected fidl::Error");
    match err {
        fidl::Error::ClientChannelClosed { epitaph, protocol_name: _ } => {
            assert_eq!(epitaph, fidl::Status::PEER_CLOSED);
        }
        _ => {
            assert!(false, "expected ClientChannelClosed");
        }
    }
}

#[fuchsia::test]
async fn collect_isolated_logs_using_batch_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let mut options = default_run_suite_options();
    options.logs_iterator_type = Some(ftest_manager::LogsIteratorType::Batch);
    let (_events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_using_archive_iterator_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunSuiteOptions {
        logs_iterator_type: Some(ftest_manager::LogsIteratorType::Socket),
        ..default_run_suite_options()
    };
    let (_events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_using_host_socket_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunSuiteOptions {
        logs_iterator_type: Some(ftest_manager::LogsIteratorType::Socket),
        ..default_run_suite_options()
    };
    let (_events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn update_log_severity_for_all_components_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunSuiteOptions {
        logs_iterator_type: Some(ftest_manager::LogsIteratorType::Socket),
        log_interest: Some(vec![
            selectors::parse_log_interest_selector_or_severity("DEBUG").unwrap(),
        ]),
        ..default_run_suite_options()
    };
    let (_events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();
    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec![
            "I'm a debug log from a test",
            "Started diagnostics publisher",
            "I'm a debug log from the publisher!",
            "Finishing through Stop",
        ]
    );
}

#[fuchsia::test]
async fn update_log_severity_for_the_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunSuiteOptions {
        logs_iterator_type: Some(ftest_manager::LogsIteratorType::Socket),
        log_interest: Some(vec![
            selectors::parse_log_interest_selector_or_severity("<root>#DEBUG").unwrap(),
        ]),
        ..default_run_suite_options()
    };
    let (_events, logs) = run_single_test_for_suite(test_url, options).await.unwrap();
    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec![
            "I'm a debug log from a test",
            "Started diagnostics publisher",
            "Finishing through Stop",
        ]
    );
}

#[fuchsia::test]
async fn custom_artifact_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/custom_artifact_user.cm";

    let (events, _) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();
    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("use_artifact"),
        RunEvent::case_started("use_artifact"),
        RunEvent::case_stopped("use_artifact", CaseStatus::Passed),
        RunEvent::case_finished("use_artifact"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
        RunEvent::suite_custom(".", "artifact.txt", "Hello, world!"),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(&expected_events, &events);
}

#[test_case(true; "compressed debug_data")]
#[test_case(false; "uncompressed debug_data")]
#[fuchsia::test]
async fn debug_data_test_for_suite(compressed: bool) {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/debug_data_write_test.cm";

    let runner = SuiteRunner::new(connect_suite_runner!().unwrap());
    let suite_run_instance = runner
        .start_suite_run(test_url, default_run_suite_options())
        .context("Cannot run suite")
        .unwrap();
    let suite_events_result =
        collect_suite_events_with_watch(suite_run_instance, false, compressed).await;
    let suite_events = suite_events_result.unwrap().0;

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("publish_debug_data"),
        RunEvent::case_started("publish_debug_data"),
        RunEvent::case_stopped("publish_debug_data", CaseStatus::Passed),
        RunEvent::case_finished("publish_debug_data"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    // Split `suite_events` into `DebugData` events and others.
    let mut debug_data_events = vec![];
    let suite_events: Vec<RunEvent> = suite_events
        .into_iter()
        .filter_map(|e| {
            if let RunEvent::DebugData { .. } = e {
                debug_data_events.push(e);
                None
            } else {
                Some(e)
            }
        })
        .collect();

    // Verify the `DebugData` events.
    let got_summary = false;
    let got_data_sink = false;
    stream::iter(debug_data_events).for_each(|debug_data_event| async move {
        if let RunEvent::DebugData { filename, socket } = debug_data_event {
            if filename == "summary.json" {
                assert!(!got_summary, "received summary.json DebugData more than once");
                let content = collect_string_from_socket_helper(socket, compressed).await.unwrap();
                assert_eq!(content[..150], r#"{"tests":[{"name":"fuchsia-pkg://fuchsia.com/test_manager_test#meta/debug_data_write_test.cm","data_sinks":{"data_sink":[{"file":"data_sink/data_sink."#[..150]);
            } else if filename.starts_with("data_sink/data_sink.") {
                assert!(!got_data_sink, "received data sink DebugData more than once");
                let content = collect_string_from_socket_helper(socket, compressed).await.unwrap();
                assert_eq!(content, "Debug data from test\n");
            }
        }
    }).await;

    // Verify the other events.
    assert_eq!(
        suite_events.into_iter().group_by_test_case_unordered(),
        expected_events.into_iter().group_by_test_case_unordered(),
    );
}

// Test that we can hook up DebugDataIterator connection for EarlyBootPrfile.
#[fuchsia::test]
async fn early_boot_profile_test_for_suite() {
    let proxy = client::connect_to_protocol::<ftest_manager::EarlyBootProfileMarker>()
        .expect("cannot connect to run builder proxy");
    let (debug_data_proxy, iterator) = endpoints::create_proxy();
    proxy.register_watcher(iterator).unwrap();

    // We cannot check the returned value as it can be empty vector or bunch of socket connections.
    // But we can check that our call to DebugDataIterator goes through.
    let _ = debug_data_proxy.get_next_compressed().await.unwrap();
}

#[test_case(true; "compressed debug_data")]
#[test_case(false; "uncompressed debug_data")]
#[fuchsia::test]
async fn debug_data_isolated_test_for_suite(compressed: bool) {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/debug_data_write_test.cm";

    // By default, when I run the same test twice, debug data is not accumulated.
    for _ in 0..2 {
        let runner = SuiteRunner::new(connect_suite_runner!().unwrap());
        let suite_run_instance = runner
            .start_suite_run(test_url, default_run_suite_options())
            .context("Cannot run suite")
            .unwrap();
        let suite_events_result =
            collect_suite_events_with_watch(suite_run_instance, false, compressed).await;
        let suite_events = suite_events_result.unwrap().0;

        let debug_event_count = stream::iter(suite_events)
            .then(|suite_event| async move {
                if let RunEvent::DebugData { filename, socket } = suite_event {
                    if filename.starts_with("data_sink/data_sink.") {
                        let content =
                            collect_string_from_socket_helper(socket, compressed).await.unwrap();
                        assert_eq!(content, "Debug data from test\n");
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
            .filter(|matches_vmo| futures::future::ready(*matches_vmo))
            .count()
            .await;
        assert_eq!(debug_event_count, 1);
    }
}

#[fuchsia::test]
async fn custom_artifact_realm_test_for_suite() {
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/custom_artifact_realm_test.cm";

    let (events, _) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();
    let events = events.into_iter().filter(not_debug_data).group_by_test_case_unordered();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("use_artifact"),
        RunEvent::case_started("use_artifact"),
        RunEvent::case_stopped("use_artifact", CaseStatus::Passed),
        RunEvent::case_finished("use_artifact"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
        RunEvent::suite_custom("test_driver", "artifact.txt", "Hello, world!"),
    ]
    .into_iter()
    .group_by_test_case_unordered();

    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn enumerate_invalid_test_for_suite() {
    let proxy = connect_test_case_enumerator_server!().unwrap();
    let (_iterator, server_end) = endpoints::create_proxy();
    let err = proxy
        .enumerate(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/invalid_cml.cm",
            ftest_manager::EnumerateTestCasesOptions { ..Default::default() },
            server_end,
        )
        .await
        .unwrap()
        .expect_err("This should error out as we have invalid test");
    assert_eq!(err, ftest_manager::LaunchError::InstanceCannotResolve);
}

#[fuchsia::test]
async fn enumerate_echo_test_for_suite() {
    let proxy = connect_test_case_enumerator_server!().unwrap();
    let (iterator, server_end) = endpoints::create_proxy();
    proxy
        .enumerate(
            "fuchsia-pkg://fuchsia.com/test_manager_test#meta/echo_test_realm.cm",
            ftest_manager::EnumerateTestCasesOptions { ..Default::default() },
            server_end,
        )
        .await
        .unwrap()
        .expect("This should not fail");

    let mut cases = vec![];
    loop {
        let mut c = iterator.get_next().await.unwrap();
        if c.is_empty() {
            break;
        }
        cases.append(&mut c);
    }
    assert_eq!(
        cases.into_iter().map(|c| c.name.unwrap()).collect::<Vec<_>>(),
        vec!["EchoTest".to_string()]
    );
}

#[fuchsia::test]
async fn enumerate_huge_test_for_suite() {
    let proxy = connect_test_case_enumerator_server!().unwrap();
    let (iterator, server_end) = endpoints::create_proxy();
    proxy
        .enumerate(
            "fuchsia-pkg://fuchsia.com/gtest-runner-example-tests#meta/huge_gtest.cm",
            ftest_manager::EnumerateTestCasesOptions { ..Default::default() },
            server_end,
        )
        .await
        .unwrap()
        .expect("This should not fail");

    let mut cases = vec![];
    loop {
        let mut c = iterator.get_next().await.unwrap();
        if c.is_empty() {
            break;
        }
        cases.append(&mut c);
    }
    let expected_cases = (0..=999)
        .into_iter()
        .map(|n| format!("HugeStress/HugeTest.Test/{}", n))
        .collect::<Vec<_>>();

    assert_eq!(cases.into_iter().map(|c| c.name.unwrap()).collect::<Vec<_>>(), expected_cases);
}

#[fuchsia::test]
async fn capability_access_test_for_suite() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_test#meta/nonhermetic_capability_test.cm";
    let (events, _) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    let failed_suite = RunEvent::suite_stopped(SuiteStatus::Failed);

    assert!(events.contains(&failed_suite));
}

#[fuchsia::test]
async fn launch_chromium_test_for_suite() {
    // TODO(https://fxbug.dev/42173552): This test is launched in the chromium
    // realm. Once we support out of tree realm definitions we should move the
    // definition and test to chromium.
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_test#meta/simple_chromium_realm_test.cm";
    let (events, logs) =
        run_single_test_for_suite(test_url, default_run_suite_options()).await.unwrap();

    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("noop_test"),
        RunEvent::case_started("noop_test"),
        RunEvent::case_stopped("noop_test", CaseStatus::Passed),
        RunEvent::case_finished("noop_test"),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(logs, Vec::new());
    assert_eq!(&expected_events, &events.into_iter().filter(not_debug_data).collect::<Vec<_>>());
}

fn not_debug_data(event: &RunEvent) -> bool {
    match event {
        RunEvent::DebugData { .. } => false,
        _ => true,
    }
}
