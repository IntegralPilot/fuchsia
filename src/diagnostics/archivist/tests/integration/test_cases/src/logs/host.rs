// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(fuchsia_api_level_at_least = "HEAD")]

use crate::logs::common::LogFormat;
use crate::logs::common::ffi_format::StreamArchiveAccessor;
use crate::puppet::PuppetProxyExt;
use crate::{test_topology, utils};
use diagnostics_data::Severity;
use fidl_fuchsia_archivist_test as ftest;
use fidl_fuchsia_diagnostics::Format;
use fidl_fuchsia_diagnostics_types as fdiagnostics_types;
use futures::StreamExt;
use test_case::test_case;

const PUPPET_NAME: &str = "puppet";

#[test_case(LogFormat::Rust(Format::Json))]
#[test_case(LogFormat::Rust(Format::Fxt))]
#[fuchsia::test]
async fn can_read_using_the_socket_accessor(format: LogFormat) {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create realm");

    let messages = vec![
        (fdiagnostics_types::Severity::Info, "my msg: 10"),
        (fdiagnostics_types::Severity::Warn, "my other msg: 20"),
    ];
    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();
    puppet.log_messages(messages.clone()).await;

    let accessor = utils::connect_accessor(&realm_proxy, utils::ALL_PIPELINE).await;
    let reader = format.build(StreamArchiveAccessor(accessor));
    let mut stream = reader.get_test_snapshot_then_subscribe().await;

    let mut pending = messages.into_iter().peekable();
    while pending.peek().is_some() {
        let log = stream.next().await.unwrap();
        let (severity, msg) = pending.next().unwrap();
        assert_eq!(log.severity, Severity::from(severity));
        assert_eq!(log.message, msg);
    }
}
