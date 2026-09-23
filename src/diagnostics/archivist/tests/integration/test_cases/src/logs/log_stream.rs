// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(fuchsia_api_level_at_least = "HEAD")]

use crate::puppet::PuppetProxyExt;
use crate::test_topology;
use diagnostics_log_encoding::parse::parse_record;
use diagnostics_log_encoding::{Argument, Record};
use futures::{StreamExt, future};
use {
    fidl_fuchsia_archivist_test as ftest, fidl_fuchsia_diagnostics as fdiagnostics,
    fidl_fuchsia_diagnostics_types as fdiagnostics_types,
};

const PUPPET_NAME: &str = "puppet";

struct OwnedLog {
    record: Record<'static>,
    moniker: String,
    url: String,
}

#[fuchsia::test]
async fn listen_with_log_stream() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .expect("create realm");

    let puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();
    puppet
        .log_messages(vec![
            (fdiagnostics_types::Severity::Info, "my msg: 10"),
            (fdiagnostics_types::Severity::Warn, "my other msg: 20"),
        ])
        .await;

    let (socket, s2) = zx::Socket::create_datagram();
    let socket = fuchsia_async::Socket::from_socket(socket);
    let log_stream =
        realm_proxy.connect_to_protocol::<fdiagnostics::LogStreamMarker>().await.unwrap();
    log_stream
        .connect(
            s2,
            &fdiagnostics::LogStreamOptions {
                mode: Some(fdiagnostics::StreamMode::SnapshotThenSubscribe),
                include_moniker: Some(true),
                include_component_url: Some(true),
                ..Default::default()
            },
        )
        .expect("connected socket");

    let mut records = socket
        .into_datagram_stream()
        .filter_map(|result| {
            let bytes = result.unwrap();
            if bytes.is_empty() {
                return future::ready(None);
            }
            let (record, rest) = match parse_record(&bytes) {
                Ok((record, consumed)) => (record.into_owned(), consumed),
                Err(err) => {
                    panic!("Failed to parse record: {bytes:?}: {err:?}")
                }
            };

            let (moniker, url) = if !rest.is_empty() {
                assert!(rest.len() >= 16, "extended data should have a header");
                let moniker_len = u32::from_le_bytes(rest[0..4].try_into().unwrap()) as usize;
                let component_url_len = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;

                // We don't test rolled out here, but we still need to parse it to advance.
                let mut data_offset = 16;
                let moniker_str =
                    std::str::from_utf8(&rest[data_offset..data_offset + moniker_len]).unwrap();
                let moniker_padded_len = (moniker_len + 7) & !7;
                data_offset += moniker_padded_len;

                let url_str =
                    std::str::from_utf8(&rest[data_offset..data_offset + component_url_len])
                        .unwrap();
                let component_url_padded_len = (component_url_len + 7) & !7;
                data_offset += component_url_padded_len;

                assert_eq!(data_offset, rest.len(), "extended data should be fully parsed");
                (moniker_str.to_string(), url_str.to_string())
            } else {
                ("".to_string(), "".to_string())
            };

            future::ready(Some(OwnedLog { record, moniker, url }))
        })
        .take(2);

    let log = records.next().await.unwrap();
    assert_eq!(log.record.severity, fdiagnostics_types::Severity::Info.into_primitive());
    assert_eq!(message(&log.record), "my msg: 10");
    assert_eq!(log.moniker, PUPPET_NAME);
    assert_eq!(log.url, "puppet#meta/puppet.cm");

    let log = records.next().await.unwrap();
    assert_eq!(log.record.severity, fdiagnostics_types::Severity::Warn.into_primitive());
    assert_eq!(message(&log.record), "my other msg: 20");
    assert_eq!(log.moniker, PUPPET_NAME);
    assert_eq!(log.url, "puppet#meta/puppet.cm");
}

fn message<'a>(record: &'a Record<'_>) -> &'a str {
    record
        .arguments
        .iter()
        .filter_map(|arg| match arg {
            Argument::Message(m) => Some(m.as_ref()),
            _ => None,
        })
        .next()
        .unwrap()
}
