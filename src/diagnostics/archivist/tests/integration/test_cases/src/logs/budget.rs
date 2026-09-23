// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::logs::common::LogFormat;
use crate::{test_topology, utils};
use fidl_fuchsia_archivist_test as ftest;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_diagnostics_types::Severity;
use futures::StreamExt;
use test_case::test_case;

const SPAM_COUNT: usize = 1001;

/// Verify that Archivist correctly budgets and evicts log messages when its
/// cache is full. Archivist is configured with a limited buffer size, and this
/// test ensures that as new logs are ingested, older logs are dropped to stay
/// within the budget. This prevents unbounded memory growth and ensures the
/// system remains stable even under heavy log production.
///
/// The test sets up two log producers: a "victim" that logs a single message
/// and a "spammer" that floods Archivist with enough logs to exceed the
/// configured budget. It then verifies that the victim's initial log message is
/// evicted from the cache, confirming the FIFO (First-In, First-Out) eviction
/// strategy.
#[cfg_attr(
    fuchsia_api_level_at_least = "HEAD",
    test_case(LogFormat::Rust(fdiagnostics::Format::Fxt))
)]
#[cfg_attr(fuchsia_api_level_at_least = "HEAD", test_case(LogFormat::Ffi))]
#[cfg_attr(
    fuchsia_api_level_at_least = "HEAD",
    test_case(LogFormat::Rust(fdiagnostics::Format::LegacyFxt))
)]
#[test_case(LogFormat::Rust(fdiagnostics::Format::Json))]
#[fuchsia::test]
async fn test_budget(reader_format: LogFormat) {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![
            test_topology::PuppetDeclBuilder::new("spammer").into(),
            test_topology::PuppetDeclBuilder::new("victim").into(),
        ]),
        archivist_config: Some(ftest::ArchivistConfig {
            logs_max_cached_original_bytes: Some(98304),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await
    .unwrap();

    let spammer_puppet = test_topology::connect_to_puppet(&realm_proxy, "spammer").await.unwrap();
    let victim_puppet = test_topology::connect_to_puppet(&realm_proxy, "victim").await.unwrap();
    spammer_puppet.wait_for_interest_change().await.unwrap();
    victim_puppet.wait_for_interest_change().await.unwrap();

    let mut counter = 0;
    let mut next_message = || {
        counter += 1;
        format!("{counter:50}")
    };
    let expected = next_message();
    victim_puppet
        .log(&ftest::LogPuppetLogRequest {
            severity: Some(Severity::Info),
            message: Some(expected.clone()),
            ..Default::default()
        })
        .await
        .expect("emitted log");

    let accessor = utils::connect_accessor(&realm_proxy, utils::ALL_PIPELINE).await;
    let reader = reader_format.build(accessor);

    let mut observed_logs = reader.get_test_snapshot_then_subscribe().await;
    let mut observed_logs_2 = reader.get_test_snapshot_then_subscribe().await;

    let msg_a = observed_logs.next().await.unwrap();
    let msg_a_2 = observed_logs_2.next().await.unwrap();
    assert_eq!(expected, msg_a.message);
    assert_eq!(expected, msg_a_2.message);

    // Spam many logs.
    let mut expected_messages = Vec::new();
    for i in 0..SPAM_COUNT {
        let message = next_message();
        spammer_puppet
            .log(&ftest::LogPuppetLogRequest {
                severity: Some(Severity::Info),
                message: Some(message.clone()),
                ..Default::default()
            })
            .await
            .expect("emitted log");
        expected_messages.push(message);

        // Each message is about 136 bytes. Archivist delays rolling out messages to reduce CPU
        // time, so we must take care to observe the messages in batches. If we don't wait,
        // the logs will get dropped when we try and send them. The batch size of 100 should
        // fit in the buffer all at once, and then we sleep to ensure that Archivist will wake up
        // and roll logs out before we write and read the next batch.
        if i.is_multiple_of(100) {
            for message in expected_messages.drain(..) {
                assert_eq!(message, observed_logs.next().await.unwrap().message);
            }
        }
    }

    for message in expected_messages.drain(..) {
        assert_eq!(message, observed_logs.next().await.unwrap().message);
    }

    // We observe some logs were rolled out.
    while !observed_logs_2.next().await.unwrap().message.contains("rolled_out") {}

    let mut observed_logs = reader.get_test_snapshot().await.into_iter();
    let msg_b = observed_logs.next().unwrap();
    assert_ne!(msg_b.moniker_tag, "puppet-victim");

    // Victim logs should have been rolled out.
    let messages =
        observed_logs.filter(|log| log.moniker_tag == "puppet-victim").collect::<Vec<_>>();
    assert!(messages.is_empty());
    assert_ne!(msg_a.message, msg_b.message);
}
