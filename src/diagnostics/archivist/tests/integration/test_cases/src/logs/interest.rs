// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::assert::assert_logs_sequence;
use crate::puppet::PuppetProxyExt;
use crate::test_topology;
use crate::utils::LogSettingsExt;
use diagnostics_data::ExtendedMoniker;
use fidl_fuchsia_archivist_test as ftest;
use fidl_fuchsia_diagnostics::LogSettingsMarker;
use fidl_fuchsia_diagnostics_types::Severity;
use futures::{FutureExt, select};
use std::pin::pin;
use std::sync::LazyLock;

const PUPPET_NAME: &str = "puppet";
static PUPPET_MONIKER: LazyLock<ExtendedMoniker> =
    LazyLock::new(|| PUPPET_NAME.try_into().unwrap());

#[fuchsia::test]
async fn set_interest_persist() {
    const REALM_NAME: &str = "set_interest_persist";

    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        realm_name: Some(REALM_NAME.to_string()),
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create test topology");

    let mut logs = crate::utils::snapshot_and_stream_logs(&realm_proxy).await;

    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME)
        .await
        .expect("connect to puppet");

    // Use default severity INFO.
    // Wait for the initial interest to be observed.
    let mut response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Info));

    // Log one info message before the first debug message to confirm the debug
    // message isn't skipped because of a race condition.
    puppet
        .log_messages(vec![
            (Severity::Info, "A1"),
            (Severity::Debug, "B1"), // not observed.
            (Severity::Info, "C1"),
            (Severity::Warn, "D1"),
            (Severity::Error, "E1"),
        ])
        .await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![
            (Severity::Info, "A1"),
            (Severity::Info, "C1"),
            (Severity::Warn, "D1"),
            (Severity::Error, "E1"),
        ],
    )
    .await;

    let log_settings = realm_proxy
        .connect_to_protocol::<LogSettingsMarker>()
        .await
        .expect("connect to log settings");

    // Severity: DEBUG
    log_settings.set_interest_for_component(PUPPET_NAME, Severity::Debug, true).await.unwrap();

    response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Debug));

    drop(log_settings);

    let mut wait_for_interest = puppet.wait_for_interest_change().fuse();
    let mut timer =
        pin!(fuchsia_async::Timer::new(fuchsia_async::MonotonicDuration::from_seconds(3)).fuse());
    let got_interest_change_event = select! {
        _ = wait_for_interest => true,
        _ = timer => false,
    };
    assert!(!got_interest_change_event);

    puppet
        .log_messages(vec![
            (Severity::Debug, "A2"),
            (Severity::Info, "B2"),
            (Severity::Warn, "C2"),
            (Severity::Error, "D2"),
        ])
        .await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![
            (Severity::Debug, "A2"),
            (Severity::Info, "B2"),
            (Severity::Warn, "C2"),
            (Severity::Error, "D2"),
        ],
    )
    .await;
}

// This test verifies that a component only emits messages at or above its
// current interest severity level, even when the interest changes while the
// component is running.
#[fuchsia::test]
async fn set_interest() {
    const REALM_NAME: &str = "set_interest";

    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        realm_name: Some(REALM_NAME.to_string()),
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create test topology");

    let mut logs = crate::utils::snapshot_and_stream_logs(&realm_proxy).await;

    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME)
        .await
        .expect("connect to puppet");

    // Use default severity INFO.
    // Wait for the initial interest to be observed.
    let mut response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Info));

    // Log one info message before the first debug message to confirm the debug
    // message isn't skipped because of a race condition.
    puppet
        .log_messages(vec![
            (Severity::Info, "A1"),
            (Severity::Debug, "B1"), // not observed.
            (Severity::Info, "C1"),
            (Severity::Warn, "D1"),
            (Severity::Error, "E1"),
        ])
        .await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![
            (Severity::Info, "A1"),
            (Severity::Info, "C1"),
            (Severity::Warn, "D1"),
            (Severity::Error, "E1"),
        ],
    )
    .await;

    let log_settings = realm_proxy
        .connect_to_protocol::<LogSettingsMarker>()
        .await
        .expect("connect to log settings");

    // Severity: DEBUG
    log_settings.set_interest_for_component(PUPPET_NAME, Severity::Debug, false).await.unwrap();
    response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Debug));
    puppet
        .log_messages(vec![
            (Severity::Debug, "A2"),
            (Severity::Info, "B2"),
            (Severity::Warn, "C2"),
            (Severity::Error, "D2"),
        ])
        .await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![
            (Severity::Debug, "A2"),
            (Severity::Info, "B2"),
            (Severity::Warn, "C2"),
            (Severity::Error, "D2"),
        ],
    )
    .await;

    // Severity: WARN
    log_settings.set_interest_for_component(PUPPET_NAME, Severity::Warn, false).await.unwrap();
    response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Warn));
    puppet
        .log_messages(vec![
            (Severity::Debug, "A3"), // Not observed.
            (Severity::Info, "B3"),  // Not observed.
            (Severity::Warn, "C3"),
            (Severity::Error, "D3"),
        ])
        .await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![(Severity::Warn, "C3"), (Severity::Error, "D3")],
    )
    .await;

    // Severity: ERROR
    log_settings.set_interest_for_component(PUPPET_NAME, Severity::Error, false).await.unwrap();
    response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Error));
    puppet
        .log_messages(vec![
            (Severity::Debug, "A4"), // Not observed.
            (Severity::Info, "B4"),  // Not observed.
            (Severity::Warn, "C4"),  // Not observed.
            (Severity::Error, "D4"),
        ])
        .await;

    assert_logs_sequence(&mut logs, &PUPPET_MONIKER, vec![(Severity::Error, "D4")]).await;

    // Disconnecting the protocol, brings back an EMPTY interest, which defaults to Severity::Info.
    drop(log_settings);
    response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Info));

    // Again, log one info message before the first debug message to confirm the
    // debug message isn't skipped because of a race condition.
    puppet
        .log_messages(vec![
            (Severity::Debug, "A5"), // Not observed.
            (Severity::Info, "B5"),
            (Severity::Info, "C5"),
            (Severity::Warn, "D5"),
            (Severity::Error, "E5"),
        ])
        .await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![
            (Severity::Info, "B5"),
            (Severity::Info, "C5"),
            (Severity::Warn, "D5"),
            (Severity::Error, "E5"),
        ],
    )
    .await;
}

// This test verifies that a component only emits messages at or above its
// current interest severity level, where the interest is inherited from the
// parent realm, having been configured before the component was launched.
#[fuchsia::test]
async fn set_interest_before_startup() {
    const REALM_NAME: &str = "set_interest_before_startup";

    // Create the test realm.
    // We won't connect to the puppet until after we've configured logging interest.
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        realm_name: Some(REALM_NAME.to_string()),
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create test topology");

    let log_settings = realm_proxy
        .connect_to_protocol::<LogSettingsMarker>()
        .await
        .expect("connect to log settings");

    // Set the minimum severity to Severity::Debug.
    log_settings.set_interest_for_component(PUPPET_NAME, Severity::Debug, false).await.unwrap();

    // Connect to the component under test to start it.
    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME)
        .await
        .expect("connect to puppet");

    let response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Debug));
    puppet
        .log_messages(vec![(Severity::Debug, "debugging world"), (Severity::Info, "Hello, world!")])
        .await;

    // Assert logs include the Severity::Debug log.
    let mut logs = crate::utils::snapshot_and_stream_logs(&realm_proxy).await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![(Severity::Debug, "debugging world"), (Severity::Info, "Hello, world!")],
    )
    .await;
}

// This test verifies that the initial severity set for a a component only emits messages at or above its
// current interest severity level, where the interest is inherited from the
// parent realm, having been configured before the component was launched.
#[fuchsia::test]
async fn initial_interest() {
    const REALM_NAME: &str = "initial_interest";

    // Create the test realm.
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        archivist_config: Some(ftest::ArchivistConfig {
            initial_interests: Some(vec![ftest::ComponentInitialInterest {
                moniker: Some(PUPPET_NAME.into()),
                log_severity: Some(Severity::Debug),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        realm_name: Some(REALM_NAME.to_string()),
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create test topology");

    // Connect to the component under test to start it.
    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME)
        .await
        .expect("connect to puppet");

    let response = puppet.wait_for_interest_change().await.unwrap();
    assert_eq!(response.severity, Some(Severity::Debug));
    puppet
        .log_messages(vec![
            (Severity::Trace, "tracing world"),
            (Severity::Debug, "debugging world"),
            (Severity::Info, "Hello, world!"),
        ])
        .await;

    // Assert logs include the Severity::Debug log.
    let mut logs = crate::utils::snapshot_and_stream_logs(&realm_proxy).await;

    assert_logs_sequence(
        &mut logs,
        &PUPPET_MONIKER,
        vec![(Severity::Debug, "debugging world"), (Severity::Info, "Hello, world!")],
    )
    .await;
}
