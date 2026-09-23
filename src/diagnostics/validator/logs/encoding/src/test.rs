// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_validate_logs::{
    EncodingValidatorMarker, TestFailure, TestSuccess, ValidateResult,
    ValidateResultsIteratorGetNextResponse,
};
use fuchsia_component::client;
use log::info;

#[fuchsia::test]
async fn validate_rust_log_encoding() {
    let validator =
        client::connect_to_protocol::<EncodingValidatorMarker>().expect("connect to validator");
    let (result_iterator, server_end) = fidl::endpoints::create_proxy();
    validator.validate(server_end).expect("call validate");

    let mut failed_results = vec![];
    let mut tests_executed = 0;
    while let Ok(response) = result_iterator.get_next().await {
        let ValidateResultsIteratorGetNextResponse { result, .. } = response;
        let Some(result) = result else {
            break;
        };
        tests_executed += 1;
        match result {
            ValidateResult::Success(TestSuccess { test_name }) => {
                info!("Test {test_name} passed.");
            }
            ValidateResult::Failure(TestFailure { test_name, reason }) => {
                failed_results.push(format!("Test {test_name} failed: {reason}"));
            }
        }
    }

    assert!(tests_executed > 0);
    info!("Ran: {tests_executed} tests.");
    if !failed_results.is_empty() {
        panic!("{}", failed_results.join("\n"));
    }
}
