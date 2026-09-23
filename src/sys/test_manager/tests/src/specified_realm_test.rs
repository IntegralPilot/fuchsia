// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl::endpoints::ClientEnd;
use ftest_manager::{CaseStatus, RunOptions, SuiteStatus};
use fuchsia_component::client;
use futures::prelude::*;
use pretty_assertions::assert_eq;
use test_case::test_case;
use test_manager_test_lib::{
    AttributedLog, GroupRunEventByTestCase, RunEvent, TestBuilder, TestRunEventPayload,
    collect_string_from_socket_helper, collect_suite_events, default_run_option,
};
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_test_manager as ftest_manager, fuchsia_async as fasync,
};

const ECHO_TEST_COL: &str = "echo_test_coll";
const HERMETIC_TEST_COL: &str = "hermetic_test_coll";

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

fn connect_realm() -> Result<ClientEnd<fcomponent::RealmMarker>, Error> {
    let (client_end, server_end) = fidl::endpoints::create_endpoints::<fcomponent::RealmMarker>();
    client::connect_channel_to_protocol::<fcomponent::RealmMarker>(server_end.into_channel())
        .context("could not connect to Realm service")?;
    Ok(client_end)
}

fn default_offers() -> Vec<fdecl::Offer> {
    vec![
        fdecl::Offer::Storage(fdecl::OfferStorage {
            source_name: Some("data".to_string()),
            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
            target: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
            target_name: Some("data".to_string()),
            ..Default::default()
        }),
        fdecl::Offer::EventStream(fdecl::OfferEventStream {
            target_name: Some("capability_requested".to_string()),
            ..Default::default()
        }),
    ]
}

async fn run_test_in_echo_test_realm(
    test_url: &str,
    run_options: RunOptions,
) -> Result<(Vec<RunEvent>, Vec<AttributedLog>), Error> {
    let realm = connect_realm().unwrap();
    let mut offers = default_offers();
    offers.push(fdecl::Offer::Protocol(fdecl::OfferProtocol {
        source_name: Some("fidl.examples.routing.echo.Echo".into()),
        target_name: Some("fidl.examples.routing.echo.Echo".into()),
        source: None,
        target: None,
        dependency_type: None,
        ..Default::default()
    }));
    run_single_test(realm, &offers, ECHO_TEST_COL, test_url, run_options).await
}

async fn run_test_in_hermetic_test_realm(
    test_url: &str,
    run_options: RunOptions,
) -> Result<(Vec<RunEvent>, Vec<AttributedLog>), Error> {
    let realm = connect_realm().unwrap();
    let offers = default_offers();
    run_single_test(realm, &offers, HERMETIC_TEST_COL, test_url, run_options).await
}

async fn run_single_test(
    realm: ClientEnd<fcomponent::RealmMarker>,
    offers: &[fdecl::Offer],
    test_collection: &str,
    test_url: &str,
    run_options: RunOptions,
) -> Result<(Vec<RunEvent>, Vec<AttributedLog>), Error> {
    let builder = TestBuilder::new(connect_run_builder!()?);
    let suite_instance = builder
        .add_suite_in_realm(realm, offers, test_collection, test_url, run_options)
        .await
        .context("Cannot create suite instance")?;
    let builder_run = fasync::Task::spawn(async move { builder.run().await });
    let ret = collect_suite_events(suite_instance).await;
    builder_run.await.context("builder execution failed")?;
    ret
}

#[fuchsia::test]
async fn launch_and_test_echo_test() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/echo_test_client.cm";
    let (events, logs) = run_test_in_echo_test_realm(test_url, default_run_option()).await.unwrap();

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
async fn enumerate_echo_test() {
    let proxy = connect_query_server!().unwrap();
    let realm = connect_realm().unwrap();
    let mut offers = default_offers();
    offers.push(fdecl::Offer::Protocol(fdecl::OfferProtocol {
        source_name: Some("fidl.examples.routing.echo.Echo".into()),
        target_name: Some("fidl.examples.routing.echo.Echo".into()),
        source: None,
        target: None,
        dependency_type: None,
        ..Default::default()
    }));

    let (iterator, server_end) = fidl::endpoints::create_proxy();

    proxy
        .enumerate_in_realm(
            "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/echo_test_realm.cm",
            realm,
            &offers,
            ECHO_TEST_COL,
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
async fn launch_and_test_echo_test_in_hermetic_realm() {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/echo_test_client.cm";
    let (events, _logs) =
        run_test_in_hermetic_test_realm(test_url, default_run_option()).await.unwrap();

    // this will fail because the hermetic realm does not have access to echo service.
    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found("EchoTest"),
        RunEvent::case_started("EchoTest"),
        RunEvent::case_stopped("EchoTest", CaseStatus::Failed),
        RunEvent::case_finished("EchoTest"),
        RunEvent::suite_stopped(SuiteStatus::Failed),
    ];

    //assert_eq!(logs, Vec::<String>::new());
    assert_eq!(&expected_events, &events);
}

#[fuchsia::test]
async fn launch_and_test_hermetic_echo_test_in_hermetic_realm() {
    // This test does not depend on system echo service so should pass in hermetic realm.
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/echo_test_realm.cm";
    let (events, logs) =
        run_test_in_hermetic_test_realm(test_url, default_run_option()).await.unwrap();

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
async fn collect_isolated_logs_using_default_log_iterator() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let (_events, logs) =
        run_test_in_hermetic_test_realm(test_url, default_run_option()).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_using_batch() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let mut options = default_run_option();
    options.log_iterator = Some(ftest_manager::LogsIteratorOption::BatchIterator);
    let (_events, logs) = run_test_in_hermetic_test_realm(test_url, options).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
    );
}

#[fuchsia::test]
async fn collect_isolated_logs_using_archive_iterator() {
    let test_url = "fuchsia-pkg://fuchsia.com/test-manager-diagnostics-tests#meta/test-root.cm";
    let options = RunOptions {
        log_iterator: Some(ftest_manager::LogsIteratorOption::SocketBatchIterator),
        ..default_run_option()
    };
    let (_events, logs) = run_test_in_hermetic_test_realm(test_url, options).await.unwrap();

    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec!["Started diagnostics publisher", "Finishing through Stop"],
        "{logs:#?}",
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
    let (_events, logs) = run_test_in_hermetic_test_realm(test_url, options).await.unwrap();
    assert_eq!(
        logs.iter().map(|attributed| attributed.log.as_ref()).collect::<Vec<&str>>(),
        vec![
            "I'm a debug log from a test",
            "Started diagnostics publisher",
            "I'm a debug log from the publisher!",
            "Finishing through Stop",
        ],
        "{logs:#?}",
    );
}

#[test_case(true; "compressed debug_data")]
#[test_case(false; "uncompressed debug_data")]
#[fuchsia::test]
async fn debug_data_test(compressed: bool) {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/debug_data_write_test.cm";

    let builder = TestBuilder::new(connect_run_builder!().unwrap());
    let realm = connect_realm().unwrap();
    let suite_instance = builder
        .add_suite_in_realm(
            realm,
            &default_offers(),
            HERMETIC_TEST_COL,
            test_url,
            default_run_option(),
        )
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

#[fuchsia::test]
async fn debug_data_accumulate_test() {
    let compressed_debug_data = true;
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/debug_data_write_test.cm";

    // If I run the same test again, also accumulating debug_data, I should see two files
    for iteration in 1usize..3 {
        let builder = TestBuilder::new(connect_run_builder!().unwrap());
        builder.set_scheduling_options(true).expect("set scheduling options");
        let realm = connect_realm().unwrap();
        let suite_instance = builder
            .add_suite_in_realm(
                realm,
                &default_offers(),
                HERMETIC_TEST_COL,
                test_url,
                default_run_option(),
            )
            .await
            .expect("Cannot create suite instance");
        let (run_events_result, _) = futures::future::join(
            builder.run_with_option(compressed_debug_data),
            collect_suite_events(suite_instance),
        )
        .await;

        let num_debug_data_events = stream::iter(run_events_result.unwrap())
            .then(|run_event| async move {
                let TestRunEventPayload::DebugData { socket, .. } = run_event.payload;
                let content =
                    collect_string_from_socket_helper(socket, compressed_debug_data).await.unwrap();
                content == "Debug data from test\n"
            })
            .filter(|matches_vmo| futures::future::ready(*matches_vmo))
            .count()
            .await;
        assert_eq!(num_debug_data_events, iteration);
    }
}

#[test_case(true; "compressed debug_data")]
#[test_case(false; "uncompressed debug_data")]
#[fuchsia::test]
async fn debug_data_isolated_test(compressed: bool) {
    let test_url =
        "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/debug_data_write_test.cm";
    // By default, when I run the same test twice, debug data is not accumulated.
    for _ in 0..2 {
        let builder = TestBuilder::new(connect_run_builder!().unwrap());
        let realm = connect_realm().unwrap();
        let suite_instance = builder
            .add_suite_in_realm(
                realm,
                &default_offers(),
                HERMETIC_TEST_COL,
                test_url,
                default_run_option(),
            )
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
    let test_url = "fuchsia-pkg://fuchsia.com/test_manager_specified_realm_test#meta/custom_artifact_realm_test.cm";

    let (events, _) =
        run_test_in_hermetic_test_realm(test_url, default_run_option()).await.unwrap();
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
