// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn test_open_node_on_directory() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    let (_proxy, on_representation) = dir
        .open_node_repr::<fio::NodeMarker>(
            ".",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_NODE,
            None,
        )
        .await
        .unwrap();
    assert_matches!(on_representation, fio::Representation::Node(fio::NodeInfo { .. }));

    // If other protocol types are specified, the target must match at least one.
    let error: zx::Status = dir
        .open_node::<fio::NodeMarker>(
            ".",
            fio::Flags::PROTOCOL_NODE | fio::Flags::PROTOCOL_FILE,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, zx::Status::NOT_FILE);
}

#[fuchsia::test]
async fn test_open_node_on_file() {
    let harness = TestHarness::new().await;

    let entries = vec![file("file", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    let (_proxy, representation) = dir
        .open_node_repr::<fio::NodeMarker>(
            "file",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_NODE,
            None,
        )
        .await
        .unwrap();
    assert_matches!(representation, fio::Representation::Node(fio::NodeInfo { .. }));

    // If other protocol types are specified, the target must match at least one.
    let error: zx::Status = dir
        .open_node::<fio::NodeMarker>(
            "file",
            fio::Flags::PROTOCOL_NODE | fio::Flags::PROTOCOL_DIRECTORY,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, zx::Status::NOT_DIR);

    let error: zx::Status = dir
        .open_node::<fio::NodeMarker>(
            "file",
            fio::Flags::PROTOCOL_NODE | fio::Flags::PROTOCOL_SYMLINK,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, zx::Status::WRONG_TYPE);
}

#[fuchsia::test]
async fn test_set_flags_on_node() {
    let harness = TestHarness::new().await;
    let entries = vec![file("file", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let proxy =
        dir.open_node::<fio::NodeMarker>("file", fio::Flags::PROTOCOL_NODE, None).await.unwrap();
    assert_eq!(
        zx::Status::ok(
            proxy.set_flags(fio::Flags::FILE_APPEND).await.expect("set_flags failed").unwrap_err()
        ),
        Err(zx::Status::NOT_SUPPORTED)
    );
}

#[fuchsia::test]
async fn test_open_node_with_attributes() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    let (_proxy, representation) = dir
        .open_node_repr::<fio::NodeMarker>(
            ".",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_NODE,
            Some(fio::Options {
                attributes: Some(
                    fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::ABILITIES,
                ),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    assert_matches!(representation,
        fio::Representation::Node(fio::NodeInfo {
            attributes: Some(fio::NodeAttributes2 { mutable_attributes, immutable_attributes }),
            ..
        })
        if mutable_attributes == fio::MutableNodeAttributes::default()
            && immutable_attributes
                == fio::ImmutableNodeAttributes {
                    protocols: Some(fio::NodeProtocolKinds::DIRECTORY),
                    abilities: Some(harness.supported_dir_abilities()),
                    ..Default::default()
                }
    );
}
