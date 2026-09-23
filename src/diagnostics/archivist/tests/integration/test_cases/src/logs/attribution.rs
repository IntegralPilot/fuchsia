// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::logs::common::LogFormat;
use crate::{test_topology, utils};
use diagnostics_reader::Severity;
use fidl_fuchsia_archivist_test as ftest;
use fidl_fuchsia_archivist_test::LogPuppetLogRequest;
use fidl_fuchsia_diagnostics::Format;
use fidl_fuchsia_diagnostics_types as fdiagnostics;
use futures::StreamExt;
use test_case::test_case;

// This test verifies that Archivist knows about logging from this component.
#[test_case(LogFormat::Rust(Format::Json))]
#[cfg_attr(fuchsia_api_level_at_least = "HEAD", test_case(LogFormat::Rust(Format::Fxt)))]
#[cfg_attr(fuchsia_api_level_at_least = "HEAD", test_case(LogFormat::Ffi))]
#[fuchsia::test]
async fn log_attribution(format: LogFormat) {
    const REALM_NAME: &str = "child";
    let realm = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(REALM_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create base topology");

    let accessor = utils::connect_accessor(&realm, utils::ALL_PIPELINE).await;
    let mut result = format.build(accessor).get_test_snapshot_then_subscribe().await;

    let puppet = test_topology::connect_to_puppet(&realm, REALM_NAME).await.unwrap();
    let messages = ["This is a syslog message", "This is another syslog message"];
    for message in messages {
        puppet
            .log(&LogPuppetLogRequest {
                severity: Some(fdiagnostics::Severity::Info),
                message: Some(message.to_string()),
                ..Default::default()
            })
            .await
            .expect("Log succeeds");
    }

    for log_str in &messages {
        let log_record = result.next().await.expect("received log");
        assert_eq!(log_record.moniker_tag, REALM_NAME);
        assert!(log_record.tags.is_empty());
        assert_eq!(log_record.severity, Severity::Info);
        assert!(log_record.message.contains(log_str));
    }
}
