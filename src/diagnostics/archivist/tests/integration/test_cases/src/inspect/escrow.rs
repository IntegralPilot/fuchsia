// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{test_topology, utils};
use diagnostics_assertions::{AnyProperty, assert_data_tree};
use diagnostics_data::{InspectData, InspectHandleName};
use diagnostics_reader::{ArchiveReader, RetryConfig};
use fidl_fuchsia_diagnostics::{
    All, ComponentSelector, Selector, StringSelector, SubtreeSelector, TreeNames, TreeSelector,
};
use fuchsia_inspect::{Inspector, InspectorConfig, reader};
use realm_proxy_client::RealmProxyClient;
use {fidl_fuchsia_archivist_test as ftest, fuchsia_async as fasync};

const PUPPET_NAME: &str = "puppet";

#[fuchsia::test]
async fn escrow_inspect_data() {
    const REALM_NAME: &str = "escrow_inspect_data";

    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        realm_name: Some(REALM_NAME.into()),
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .unwrap();

    let stop_waiter = realm_proxy
        .connect_to_protocol::<ftest::StopWatcherMarker>()
        .await
        .expect("connect to stop watcher")
        .watch_component(PUPPET_NAME, ftest::ExitStatus::Clean)
        .await
        .unwrap()
        .into_proxy();

    // Publish some inspect in the puppet.
    let child_puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();
    let writer = child_puppet
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer.set_health_ok().await.unwrap();

    // Assert the current live data that the component exposes.
    let data =
        read_data(&realm_proxy, RetryConfig::always(), TreeNames::Some(vec!["root".to_string()]))
            .await
            .pop()
            .unwrap();
    assert!(!data.metadata.escrowed);
    assert_data_tree!(data.payload.as_ref().unwrap(), root: {
        "fuchsia.inspect.Health": {
            status: "OK",
            start_timestamp_nanos: AnyProperty,
        }
    });

    // Tell the puppet to escrow the data it's currently exposing.
    let token = writer
        .escrow_and_exit(&ftest::InspectWriterEscrowAndExitRequest {
            name: Some("test-escrow".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    stop_waiter.wait().await.expect("puppet stops");

    // Assert that we can read the escrowed data event after the component has stopped.
    let data = read_data(
        &realm_proxy,
        RetryConfig::always(),
        TreeNames::Some(vec!["test-escrow".to_string()]),
    )
    .await
    .pop()
    .unwrap();
    assert!(data.metadata.escrowed);
    assert_eq!(data.metadata.name, InspectHandleName::Name("test-escrow".into()));
    assert_data_tree!(data.payload.as_ref().unwrap(), root: {
        "fuchsia.inspect.Health": {
            status: "OK",
            start_timestamp_nanos: AnyProperty,
        }
    });

    // Drop token and assert there's no data anymore.
    drop(token);
    loop {
        let data = read_data(&realm_proxy, RetryConfig::never(), TreeNames::All(All {})).await;
        if data.is_empty() {
            break;
        }
        fasync::Timer::new(zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(100)))
            .await;
    }
}

#[fuchsia::test]
async fn republish_escrowed_inspect_data() {
    const REALM_NAME: &str = "republish_escrowed_inspect_data";

    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        realm_name: Some(REALM_NAME.into()),
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new(PUPPET_NAME).into()]),
        ..Default::default()
    })
    .await
    .unwrap();

    let stop_waiter = realm_proxy
        .connect_to_protocol::<ftest::StopWatcherMarker>()
        .await
        .expect("connect to stop watcher")
        .watch_component(PUPPET_NAME, ftest::ExitStatus::Clean)
        .await
        .unwrap()
        .into_proxy();

    // Publish some inspect in the puppet.
    let child_puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();
    let writer = child_puppet
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer.set_health_ok().await.unwrap();
    writer.record_int("instance", 1).await.unwrap();

    // Assert the current live data that the component exposes.
    let data =
        read_data(&realm_proxy, RetryConfig::always(), TreeNames::Some(vec!["root".to_string()]))
            .await
            .pop()
            .unwrap();
    assert!(!data.metadata.escrowed);
    assert_data_tree!(data.payload.as_ref().unwrap(), root: {
        "fuchsia.inspect.Health": {
            status: "OK",
            start_timestamp_nanos: AnyProperty,
        },
        // Will be Uint instead of Int due to JSON limitation. JSON encoding is
        // unable to distinguish between signed and unsigned numbers.
        "instance": 1u64,
    });

    // Tell the puppet to escrow the data it's currently exposing.
    let token = writer
        .escrow_and_exit(&ftest::InspectWriterEscrowAndExitRequest::default())
        .await
        .unwrap()
        .token;
    stop_waiter.wait().await.unwrap();

    // Assert that we can read the escrowed data event after the component has stopped.
    let data =
        read_data(&realm_proxy, RetryConfig::always(), TreeNames::Some(vec!["root".to_string()]))
            .await
            .pop()
            .unwrap();
    assert!(data.metadata.escrowed);
    assert_data_tree!(data.payload.as_ref().unwrap(), root: {
        "fuchsia.inspect.Health": {
            status: "OK",
            start_timestamp_nanos: AnyProperty,
        },
        "instance": 1u64,
    });

    // Restart the puppet and republish from the VMO.
    let child_puppet = test_topology::connect_to_puppet(&realm_proxy, PUPPET_NAME).await.unwrap();
    let (writer, escrowed_vmo) = child_puppet
        .create_inspector_from_escrow(ftest::InspectPuppetCreateInspectorFromEscrowRequest {
            token,
            ..Default::default()
        })
        .await
        .unwrap();
    let writer = writer.into_proxy();

    // Verify the VMO contains the previously escrowed Inspect data.
    let escrowed_inspector = Inspector::new(InspectorConfig::default().vmo(escrowed_vmo));
    let escrowed_data = reader::read(&escrowed_inspector).await.unwrap();
    assert_data_tree!(escrowed_data, root: {
        "fuchsia.inspect.Health": {
            status: "OK",
            start_timestamp_nanos: AnyProperty,
        },
        "instance": 1,
    });

    // Re-record "instance" field but modified based on previous value.
    let instance = escrowed_data.get_property("instance").unwrap().int().unwrap();
    writer.record_int("instance", instance + 1).await.unwrap();

    // Assert the component exposes the escrowed data alongside new data.
    let data =
        read_data(&realm_proxy, RetryConfig::always(), TreeNames::Some(vec!["root".to_string()]))
            .await
            .pop()
            .unwrap();
    assert!(!data.metadata.escrowed);
    assert_data_tree!(data.payload.as_ref().unwrap(), root: {
        // Will be Uint instead of Int due to JSON limitation. JSON encoding is
        // unable to distinguish between signed and unsigned numbers.
        "instance": 2u64,
    });
}

async fn read_data(
    realm_proxy: &RealmProxyClient,
    retry: RetryConfig,
    tree_names: TreeNames,
) -> Vec<InspectData> {
    let accessor = utils::connect_accessor(realm_proxy, utils::ALL_PIPELINE).await;
    ArchiveReader::inspect()
        .with_archive(accessor)
        .retry(retry)
        .add_selector(Selector {
            component_selector: Some(ComponentSelector {
                moniker_segments: Some(vec![StringSelector::ExactMatch(PUPPET_NAME.to_string())]),
                ..Default::default()
            }),
            tree_selector: Some(TreeSelector::SubtreeSelector(SubtreeSelector {
                node_path: vec![StringSelector::ExactMatch("root".to_string())],
            })),
            tree_names: Some(tree_names),
            ..Default::default()
        })
        .snapshot()
        .await
        .expect("got inspect data")
}
