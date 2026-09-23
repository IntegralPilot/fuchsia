// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::logs::common::LogFormat;
use crate::logs::utils::Listener;
use crate::puppet::PuppetProxyExt;
use crate::{test_topology, utils};
use fidl_fuchsia_archivist_test as ftest;
use fidl_fuchsia_diagnostics::Format;
use fidl_fuchsia_diagnostics_types::Severity;
use fidl_fuchsia_logger::{LogFilterOptions, LogLevelFilter, LogMarker, LogMessage, LogProxy};
use fuchsia_async as fasync;
use fuchsia_syslog_listener as syslog_listener;
use futures::channel::mpsc;
use futures::{Stream, StreamExt};
use log::info;
use test_case::test_case;

const PUPPET_NAME: &str = "puppet";

fn run_listener(tag: &str, proxy: LogProxy) -> impl Stream<Item = LogMessage> {
    let options = LogFilterOptions {
        filter_by_pid: false,
        pid: 0,
        min_severity: LogLevelFilter::None,
        verbosity: 0,
        filter_by_tid: false,
        tid: 0,
        tags: vec![tag.to_string()],
    };
    let (send_logs, recv_logs) = mpsc::unbounded();
    let l = Listener { send_logs };
    fasync::Task::spawn(async move {
        let fut = syslog_listener::run_log_listener_with_proxy(&proxy, l, Some(&options));
        if let Err(e) = fut.await {
            panic!("test fail {e:?}");
        }
    })
    .detach();
    recv_logs
}

#[fuchsia::test]
async fn listen_for_syslog() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create realm");

    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();
    puppet
        .log_messages(vec![(Severity::Info, "my msg: 10"), (Severity::Warn, "log crate: 20")])
        .await;

    let log_proxy = realm_proxy.connect_to_protocol::<LogMarker>().await.unwrap();
    let incoming = run_listener(PUPPET_NAME, log_proxy);

    let mut logs: Vec<LogMessage> = incoming.take(2).collect().await;

    // sort logs to account for out-of-order arrival
    logs.sort_by_key(|a| a.time);
    assert_eq!(2, logs.len());
    assert_eq!(logs[1].tags.len(), 1);
    assert_eq!(logs[0].tags[0], PUPPET_NAME);
    assert_eq!(logs[0].severity, LogLevelFilter::Info as i32);
    assert_eq!(logs[0].msg, "my msg: 10");
    assert_eq!(logs[1].tags.len(), 1);
    assert_eq!(logs[1].tags[0], PUPPET_NAME);
    assert_eq!(logs[1].severity, LogLevelFilter::Warn as i32);
    assert_eq!(logs[1].msg, "log crate: 20");
}

#[fuchsia::test]
async fn listen_for_klog() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        archivist_config: Some(ftest::ArchivistConfig {
            enable_klog: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await
    .unwrap();

    let log_proxy = realm_proxy.connect_to_protocol::<LogMarker>().await.unwrap();

    let logs = run_listener("klog", log_proxy);
    let msg = format!("logger_integration_rust test_klog {}", rand::random::<u64>());

    let resource = zx::Resource::from(zx::NullableHandle::invalid());
    let debuglog = zx::DebugLog::create(&resource, zx::DebugLogOpts::empty()).unwrap();
    debuglog.write(msg.as_bytes()).unwrap();

    logs.filter(|m| futures::future::ready(m.msg == msg)).next().await;
}

#[test_case(LogFormat::Rust(Format::Json))]
#[cfg_attr(fuchsia_api_level_at_least = "HEAD", test_case(LogFormat::Rust(Format::Fxt)))]
#[cfg_attr(fuchsia_api_level_at_least = "HEAD", test_case(LogFormat::Ffi))]
#[fuchsia::test]
async fn listen_for_syslog_routed_stdio(format: LogFormat) {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .unwrap();

    let accessor = utils::connect_accessor(&realm_proxy, utils::ALL_PIPELINE).await;
    let mut logs = format.build(accessor).get_test_snapshot_then_subscribe().await;

    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();
    info!("Connected to puppet");

    // Ensure the component already started by waiting for its initial interest change.
    assert_eq!(Some(Severity::Info), puppet.wait_for_interest_change().await.unwrap().severity);

    let msg = format!("logger_integration_rust test_klog stdout {}", rand::random::<u64>());
    puppet.println(&msg).await.unwrap();
    info!("printed '{msg}' to stdout");
    logs.by_ref().filter(|m| futures::future::ready(m.message == msg)).next().await;

    let msg = format!("logger_integration_rust test_klog stderr {}", rand::random::<u64>());
    puppet.eprintln(&msg).await.unwrap();
    info!("Printed '{msg}' to stderr");
    logs.filter(|m| futures::future::ready(m.message == msg)).next().await;

    // TODO(https://fxbug.dev/42126316): add test for multiline log once behavior is defined.
}
