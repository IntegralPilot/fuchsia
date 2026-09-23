// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_test_manager as ftest_manager;
use ftest_manager::{CaseStatus, SuiteStatus};
use fuchsia_component::client;
use futures::prelude::*;
use pretty_assertions::assert_eq;
use test_manager_test_lib::{
    collect_string_from_socket_helper, collect_suite_events, default_run_option,
    GroupRunEventByTestCase, RunEvent, TestBuilder, TestRunEventPayload,
};

async fn debug_data_stress_test(
    case_name: &str,
    vmo_count: usize,
    vmo_size: usize,
    compressed: bool,
) {
    const TEST_URL: &str =
        "fuchsia-pkg://fuchsia.com/test_manager_stress_test#meta/debug_data_spam_test.cm";

    let builder = TestBuilder::new(
        client::connect_to_protocol::<ftest_manager::RunBuilderMarker>()
            .expect("cannot connect to run builder proxy"),
    );
    let mut options = default_run_option();
    options.case_filters_to_run = Some(vec![case_name.into()]);
    let suite_instance =
        builder.add_suite(TEST_URL, options).await.expect("Cannot create suite instance");
    let (run_events_result, suite_events_result) = futures::future::join(
        builder.run_with_option(compressed),
        collect_suite_events(suite_instance),
    )
    .await;

    let suite_events = suite_events_result.unwrap().0;
    let expected_events = vec![
        RunEvent::suite_started(),
        RunEvent::case_found(case_name),
        RunEvent::case_started(case_name),
        RunEvent::case_stopped(case_name, CaseStatus::Passed),
        RunEvent::case_finished(case_name),
        RunEvent::suite_stopped(SuiteStatus::Passed),
    ];

    assert_eq!(
        suite_events.into_iter().group_by_test_case_unordered(),
        expected_events.into_iter().group_by_test_case_unordered(),
    );

    let test_run_events = stream::iter(run_events_result.unwrap());
    let num_vmos = test_run_events
        .then(|run_event| async move {
            let TestRunEventPayload::DebugData { filename, socket } = run_event.payload;
            let content = collect_string_from_socket_helper(socket, compressed)
                .await
                .unwrap_or_else(|e| panic!("cannot read socket for {}: {:?}", &filename, e));
            content.len() == vmo_size && content.chars().all(|c| c == 'a')
        })
        .filter(|matches_vmo| futures::future::ready(*matches_vmo))
        .count()
        .await;
    assert_eq!(num_vmos, vmo_count);
}

#[fuchsia::test]
async fn debug_data_stress_test_many_vmos() {
    const NUM_EXPECTED_VMOS: usize = 3250;
    const VMO_SIZE: usize = 4096;
    const CASE_NAME: &'static str = "many_small_vmos";
    debug_data_stress_test(CASE_NAME, NUM_EXPECTED_VMOS, VMO_SIZE, true).await;
}

#[fuchsia::test]
async fn debug_data_stress_test_few_large_vmos() {
    const NUM_EXPECTED_VMOS: usize = 2;
    const VMO_SIZE: usize = 1024 * 1024 * 400;
    const CASE_NAME: &'static str = "few_large_vmos";
    debug_data_stress_test(CASE_NAME, NUM_EXPECTED_VMOS, VMO_SIZE, true).await;
}

// Not using test-case crate as this would be deleted once the old API is deleted
#[fuchsia::test]
async fn debug_data_stress_test_many_vmos_uncompressed() {
    const NUM_EXPECTED_VMOS: usize = 3250;
    const VMO_SIZE: usize = 4096;
    const CASE_NAME: &'static str = "many_small_vmos";
    debug_data_stress_test(CASE_NAME, NUM_EXPECTED_VMOS, VMO_SIZE, false).await;
}

// Not using test-case crate as this would be deleted once the old API is deleted
#[fuchsia::test]
async fn debug_data_stress_test_few_large_vmos_uncompressed() {
    const NUM_EXPECTED_VMOS: usize = 2;
    const VMO_SIZE: usize = 1024 * 1024 * 400;
    const CASE_NAME: &'static str = "few_large_vmos";
    debug_data_stress_test(CASE_NAME, NUM_EXPECTED_VMOS, VMO_SIZE, false).await;
}
