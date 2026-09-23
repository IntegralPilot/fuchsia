// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::logs::common::{LogFormat, TestLogMessage};
use crate::{test_topology, utils};
use diagnostics_reader::RetryConfig;
use fidl_fuchsia_archivist_test as ftest;
use fidl_fuchsia_archivist_test::LogPuppetLogRequest;
use fidl_fuchsia_diagnostics::Format;
use fidl_fuchsia_diagnostics_types::Severity;
use futures::StreamExt;
use test_case::test_case;

const HELLO_WORLD: &str = "Hello, world!";

#[test_case(LogFormat::Rust(Format::Json))]
#[cfg_attr(fuchsia_api_level_at_least = "HEAD", test_case(LogFormat::Rust(Format::Fxt)))]
#[cfg_attr(fuchsia_api_level_at_least = "HEAD", test_case(LogFormat::Ffi))]
#[fuchsia::test]
async fn test_logs_lifecycle(format: LogFormat) {
    let mut puppets = Vec::with_capacity(12);
    for i in 0..50 {
        puppets.push(test_topology::PuppetDeclBuilder::new(format!("puppet{i}")).into());
    }
    let realm = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(puppets),
        ..Default::default()
    })
    .await
    .expect("create base topology");

    let accessor = utils::connect_accessor(&realm, utils::ALL_PIPELINE).await;
    let mut reader = format.build(accessor);

    // The retry config defaults to never.
    let mut subscription = reader.get_test_snapshot_then_subscribe().await;

    reader.retry_config(RetryConfig::always());
    for i in 0..50 {
        let puppet_name = format!("puppet{i}");
        let puppet = test_topology::connect_to_puppet(&realm, &puppet_name).await.unwrap();
        let request = LogPuppetLogRequest {
            severity: Some(Severity::Info),
            message: Some(HELLO_WORLD.to_string()),
            ..Default::default()
        };
        puppet.log(&request).await.expect("Log succeeds");

        check_message(&puppet_name, subscription.next().await.unwrap()).await;

        reader.retry_config(RetryConfig::MinSchemaCount(i));

        let all_messages = reader.get_test_snapshot().await;

        for message in all_messages {
            check_message("puppet", message).await;
        }
    }
}

async fn check_message(expected_moniker_prefix: &str, message: TestLogMessage) {
    assert!(message.moniker_tag.starts_with(expected_moniker_prefix));
    assert_eq!(message.message, HELLO_WORLD);
}
