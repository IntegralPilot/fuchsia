// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl::endpoints::{Proxy as _, ServerEnd};
use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

const NODE_REFERENCE_FLAGS: fio::Flags =
    fio::Flags::PROTOCOL_NODE.union(fio::Flags::PERM_GET_ATTRIBUTES);

#[fuchsia::test]
async fn clone_file() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file = dir
        .open_node::<fio::FileMarker>(
            &TEST_FILE,
            fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE,
            None,
        )
        .await
        .unwrap();
    const EXPECTED_FILE_FLAGS: fio::Flags = fio::Flags::PROTOCOL_FILE
        .union(fio::Flags::PERM_GET_ATTRIBUTES)
        .union(fio::Flags::PERM_READ_BYTES);
    assert_eq!(file.get_flags().await.unwrap().unwrap(), EXPECTED_FILE_FLAGS);

    let (clone_proxy, clone_server) = fidl::endpoints::create_proxy::<fio::FileMarker>();
    file.clone(ServerEnd::new(clone_server.into_channel())).unwrap();
    assert_eq!(clone_proxy.get_flags().await.unwrap().unwrap(), EXPECTED_FILE_FLAGS);
    // Make sure the file protocol was served by invoking some file methods.
    let _data: Vec<u8> = clone_proxy
        .read(0)
        .await
        .expect("read failed")
        .map_err(zx::Status::err_from_raw)
        .expect("read error");
}

#[fuchsia::test]
async fn clone_file_node_reference() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let node =
        dir.open_node::<fio::NodeMarker>(&TEST_FILE, NODE_REFERENCE_FLAGS, None).await.unwrap();
    assert_eq!(node.get_flags().await.unwrap().unwrap(), NODE_REFERENCE_FLAGS);

    // fuchsia.unknown/Cloneable.Clone
    let (clone_proxy, clone_server) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    node.clone(ServerEnd::new(clone_server.into_channel())).unwrap();
    assert_eq!(clone_proxy.get_flags().await.unwrap().unwrap(), NODE_REFERENCE_FLAGS);

    // We should fail to invoke file methods since this connection doesn't serve the file protocol.
    let wrong_protocol = fio::FileProxy::from_channel(clone_proxy.into_channel().unwrap());
    assert_matches!(
        wrong_protocol.read(0).await,
        Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
    );
}

#[fuchsia::test]
async fn clone_directory() {
    let harness = TestHarness::new().await;

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        dir.get_flags().await.unwrap().unwrap(),
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE
    );

    let (clone_proxy, clone_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    dir.clone(ServerEnd::new(clone_server.into_channel())).unwrap();
    assert_eq!(
        clone_proxy.get_flags().await.unwrap().unwrap(),
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE
    );
    // Make sure the directory protocol was served by invoking a directory method.
    assert_matches!(
        clone_proxy.open_node::<fio::NodeMarker>(".", fio::Flags::PROTOCOL_NODE, None).await,
        Ok(_)
    );
}

#[fuchsia::test]
async fn clone_directory_node_reference() {
    let harness = TestHarness::new().await;

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let node = dir.open_node::<fio::NodeMarker>("dir", NODE_REFERENCE_FLAGS, None).await.unwrap();
    assert_eq!(node.get_flags().await.unwrap().unwrap(), NODE_REFERENCE_FLAGS);

    // fuchsia.unknown/Cloneable.Clone
    let (clone_proxy, clone_server) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    node.clone(ServerEnd::new(clone_server.into_channel())).unwrap();
    assert_eq!(clone_proxy.get_flags().await.unwrap().unwrap(), NODE_REFERENCE_FLAGS);

    // We should fail to invoke directory methods since this isn't a directory connection.
    let wrong_protocol = fio::DirectoryProxy::from_channel(clone_proxy.into_channel().unwrap());
    assert_matches!(
        wrong_protocol.open_node::<fio::NodeMarker>(".", fio::Flags::PROTOCOL_NODE, None).await,
        Err(zx::Status::PEER_CLOSED)
    );
}
