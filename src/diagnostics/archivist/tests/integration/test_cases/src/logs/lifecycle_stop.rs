// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::assert::assert_logs_sequence;
use crate::puppet::PuppetProxyExt;
use crate::test_topology;
use crate::utils::LogSettingsExt;
use fidl_fuchsia_diagnostics_types::Severity;
use fidl_fuchsia_logger::{LogFilterOptions, LogLevelFilter, LogMarker, LogMessage};
use fuchsia_syslog_listener::{LogProcessor, run_log_listener_with_proxy};
use futures::StreamExt;
use futures::channel::mpsc;
use {
    fidl_fuchsia_archivist_test as ftest, fidl_fuchsia_diagnostics as fdiagnostics,
    fuchsia_async as fasync,
};

const PUPPET_NAME: &str = "puppet";

#[fuchsia::test]
async fn embedding_stop_api_for_log_listener() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create realm");

    let options = LogFilterOptions {
        filter_by_pid: false,
        pid: 0,
        min_severity: LogLevelFilter::None,
        verbosity: 0,
        filter_by_tid: false,
        tid: 0,
        tags: vec![PUPPET_NAME.to_owned()],
    };

    let (send_logs, recv_logs) = mpsc::unbounded();
    let log_proxy = realm_proxy.connect_to_protocol::<LogMarker>().await.unwrap();
    let task = fasync::Task::spawn(async move {
        let l = Listener { send_logs };
        run_log_listener_with_proxy(&log_proxy, l, Some(&options)).await
    });
    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();

    // Leverage `wait_for_interest_change` to know that the component started and has already
    // processed its initial interest update.
    assert_eq!(Some(Severity::Info), puppet.wait_for_interest_change().await.unwrap().severity);

    let log_settings = realm_proxy
        .connect_to_protocol::<fdiagnostics::LogSettingsMarker>()
        .await
        .expect("connect to log settings");

    log_settings.set_interest_for_component(PUPPET_NAME, Severity::Debug, false).await.unwrap();
    assert_eq!(Some(Severity::Debug), puppet.wait_for_interest_change().await.unwrap().severity);

    puppet
        .log_messages(vec![
            (Severity::Debug, "my debug message."),
            (Severity::Info, "my info message."),
            (Severity::Warn, "my warn message."),
        ])
        .await;

    // this will trigger Lifecycle.Stop.
    drop(realm_proxy);

    // collect all logs
    let logs = recv_logs.map(|l| (l.severity as i8, l.msg)).collect::<Vec<_>>().await;

    drop(task);

    assert_eq!(
        logs,
        vec![
            (Severity::Debug.into_primitive() as i8, "my debug message.".to_owned()),
            (Severity::Info.into_primitive() as i8, "my info message.".to_owned()),
            (Severity::Warn.into_primitive() as i8, "my warn message.".to_owned()),
        ]
    );
}

#[fuchsia::test]
async fn embedding_stop_api_works_for_batch_iterator() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create realm");

    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();

    // Leverage `wait_for_interest_change` to know that the component started and has already
    // processed its initial interest update.
    assert_eq!(Some(Severity::Info), puppet.wait_for_interest_change().await.unwrap().severity);

    let log_settings = realm_proxy
        .connect_to_protocol::<fdiagnostics::LogSettingsMarker>()
        .await
        .expect("connect to log settings");

    log_settings.set_interest_for_component(PUPPET_NAME, Severity::Debug, false).await.unwrap();
    assert_eq!(Some(Severity::Debug), puppet.wait_for_interest_change().await.unwrap().severity);

    puppet
        .log_messages(vec![
            (Severity::Debug, "my debug message."),
            (Severity::Info, "my info message."),
            (Severity::Warn, "my warn message."),
        ])
        .await;

    let mut logs = crate::utils::snapshot_and_stream_logs(&realm_proxy).await;

    // this will trigger Lifecycle.Stop.
    drop(realm_proxy);

    assert_logs_sequence(
        &mut logs,
        &PUPPET_NAME.try_into().unwrap(),
        vec![
            (Severity::Debug, "my debug message."),
            (Severity::Info, "my info message."),
            (Severity::Warn, "my warn message."),
        ],
    )
    .await;
}

struct Listener {
    send_logs: mpsc::UnboundedSender<LogMessage>,
}

impl LogProcessor for Listener {
    fn log(&mut self, message: LogMessage) {
        self.send_logs.unbounded_send(message).unwrap();
    }

    fn done(&mut self) {
        panic!("this should not be called");
    }
}
