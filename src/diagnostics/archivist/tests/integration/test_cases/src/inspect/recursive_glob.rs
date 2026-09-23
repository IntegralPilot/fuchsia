// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{test_topology, utils};
use diagnostics_assertions::{AnyProperty, assert_data_tree};
use diagnostics_reader::ArchiveReader;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_archivist_test as ftest;
use realm_proxy_client::RealmProxyClient;
use std::collections::HashSet;

#[fuchsia::test]
async fn read_components_recursive_glob() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![
            test_topology::PuppetDeclBuilder::new("child_a").into(),
            test_topology::PuppetDeclBuilder::new("child_b").into(),
        ]),
        ..Default::default()
    })
    .await
    .unwrap();

    // Only inspect from descendants of child_a should be reported
    let expected_monikers = HashSet::from_iter(vec![
        "child_a/nested_one".try_into().unwrap(),
        "child_a/nested_two".try_into().unwrap(),
    ]);

    let puppet_a = test_topology::connect_to_puppet(&realm_proxy, "child_a").await.unwrap();
    let writer_a = puppet_a
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer_a.set_health_ok().await.unwrap();

    let puppet_b = test_topology::connect_to_puppet(&realm_proxy, "child_b").await.unwrap();
    let writer_b = puppet_b
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer_b.set_health_ok().await.unwrap();

    let mut exposed_inspect = vec![];
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_a", "nested_one").await);
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_a", "nested_two").await);
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_b", "nested_one").await);
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_b", "nested_two").await);

    let accessor = utils::connect_accessor(&realm_proxy, utils::ALL_PIPELINE).await;
    let data_vec = ArchiveReader::inspect()
        .add_selector("child_a/**:root")
        .with_archive(accessor)
        .with_minimum_schema_count(expected_monikers.len())
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data_vec.len(), expected_monikers.len());
    let mut found_monikers = HashSet::new();
    for data in data_vec {
        assert_data_tree!(data.payload.as_ref().unwrap(), root: {
            "fuchsia.inspect.Health": {
                status: "OK",
                start_timestamp_nanos: AnyProperty,
            }
        });
        found_monikers.insert(data.moniker);
    }
    assert_eq!(expected_monikers, found_monikers);
}

#[fuchsia::test]
async fn read_components_subtree_with_recursive_glob() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![
            test_topology::PuppetDeclBuilder::new("child_a").into(),
            test_topology::PuppetDeclBuilder::new("child_b").into(),
        ]),
        ..Default::default()
    })
    .await
    .unwrap();

    let puppet_a = test_topology::connect_to_puppet(&realm_proxy, "child_a").await.unwrap();
    let writer_a = puppet_a
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer_a.set_health_ok().await.unwrap();

    let puppet_b = test_topology::connect_to_puppet(&realm_proxy, "child_b").await.unwrap();
    let writer_b = puppet_b
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer_b.set_health_ok().await.unwrap();

    let mut exposed_inspect = vec![];
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_a", "nested_one").await);
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_a", "nested_two").await);
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_b", "nested_one").await);
    exposed_inspect.push(expose_nested_inspect(&realm_proxy, "child_b", "nested_two").await);

    // Only inspect from test_app_a, and descendants of test_app_a should be reported
    let expected_monikers = HashSet::from_iter(vec![
        "child_a".try_into().unwrap(),
        "child_a/nested_one".try_into().unwrap(),
        "child_a/nested_two".try_into().unwrap(),
    ]);

    let accessor = utils::connect_accessor(&realm_proxy, utils::ALL_PIPELINE).await;
    let data_vec = ArchiveReader::inspect()
        .add_selector("child_a/**:root")
        .add_selector("child_a:root")
        .with_archive(accessor)
        .with_minimum_schema_count(expected_monikers.len())
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data_vec.len(), expected_monikers.len());
    let mut found_monikers = HashSet::new();
    for data in data_vec {
        if data.payload.is_none() {
            log::error!("UNEXPECTED EMPTY PAYLOAD: {data:?}");
        }
        assert_data_tree!(data.payload.as_ref().unwrap(), root: {
            "fuchsia.inspect.Health": {
                status: "OK",
                start_timestamp_nanos: AnyProperty,
            }
        });
        found_monikers.replace(data.moniker);
    }
    assert_eq!(expected_monikers, found_monikers);
}

async fn expose_nested_inspect(
    realm_proxy: &RealmProxyClient,
    puppet_name: &str,
    nested_puppet_name: &str,
) -> ftest::InspectWriterProxy {
    let puppet_protocol_alias =
        format!("{}.{puppet_name}.{nested_puppet_name}", ftest::InspectPuppetMarker::PROTOCOL_NAME);
    let puppet_inspect = realm_proxy
        .connect_to_named_protocol::<ftest::InspectPuppetMarker>(&puppet_protocol_alias)
        .await
        .expect("failed to connect to nested inspect puppet");

    let writer = puppet_inspect
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer.set_health_ok().await.unwrap();
    writer
}
