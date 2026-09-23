// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use futures::StreamExt as _;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn symlink_describe_prepopulated_target() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_symlinks {
        return;
    }

    let dir = harness.get_directory(
        vec![symlink(TEST_SYMLINK, TEST_SYMLINK_TARGET)],
        harness.dir_rights.all_flags(),
    );
    let symlink = dir
        .open_node::<fio::SymlinkMarker>(
            TEST_SYMLINK,
            fio::Flags::PROTOCOL_SYMLINK | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .unwrap();

    let info = symlink.describe().await.expect("describe failed");
    assert_eq!(info.target.as_deref(), Some(TEST_SYMLINK_TARGET));
}

#[fuchsia::test]
async fn symlink_open_requires_get_attributes() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_symlinks {
        return;
    }

    let dir = harness.get_directory(
        vec![symlink(TEST_SYMLINK, TEST_SYMLINK_TARGET)],
        harness.dir_rights.all_flags(),
    );
    for flags in harness.symlink_rights.combinations_without(fio::Rights::GET_ATTRIBUTES) {
        let status = dir
            .open_node::<fio::SymlinkMarker>(
                TEST_SYMLINK,
                flags | fio::Flags::PROTOCOL_SYMLINK,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(status, zx::Status::INVALID_ARGS);
    }
}

#[fuchsia::test]
async fn symlink_get_attributes() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_symlinks {
        return;
    }

    let dir = harness.get_directory(
        vec![symlink(TEST_SYMLINK, TEST_SYMLINK_TARGET)],
        harness.dir_rights.all_flags(),
    );
    let symlink = dir
        .open_node::<fio::SymlinkMarker>(
            TEST_SYMLINK,
            fio::Flags::PROTOCOL_SYMLINK | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .unwrap();

    let (_, immutable) = symlink
        .get_attributes(
            fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::CONTENT_SIZE,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(immutable.protocols, Some(fio::NodeProtocolKinds::SYMLINK));
    assert_eq!(immutable.content_size, Some(TEST_SYMLINK_TARGET.len() as u64));
}

#[fuchsia::test]
async fn symlink_link_into_rights_enforcement() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_symlinks || !harness.config.supports_link_into {
        return;
    }

    // 1. Connection with full rights (READ + WRITE + GET_ATTRIBUTES) succeeds.
    let full_flags = fio::Flags::PROTOCOL_SYMLINK
        | fio::PERM_READABLE
        | fio::PERM_WRITABLE
        | fio::Flags::PERM_GET_ATTRIBUTES;
    let dir = harness.get_directory(
        vec![symlink(TEST_SYMLINK, TEST_SYMLINK_TARGET)],
        harness.dir_rights.all_flags(),
    );
    let token = get_token(&dir).await.into();
    let symlink =
        dir.open_node::<fio::SymlinkMarker>(TEST_SYMLINK, full_flags, None).await.unwrap();
    symlink.link_into(token, "linked").await.unwrap().expect("link_into should succeed");

    // 2. Connection without full rights (e.g. read-only) fails with ACCESS_DENIED.
    let ro_flags =
        fio::Flags::PROTOCOL_SYMLINK | fio::PERM_READABLE | fio::Flags::PERM_GET_ATTRIBUTES;
    let token = get_token(&dir).await.into();
    let ro_symlink =
        dir.open_node::<fio::SymlinkMarker>(TEST_SYMLINK, ro_flags, None).await.unwrap();
    assert_eq!(
        ro_symlink.link_into(token, "linked2").await.unwrap().unwrap_err(),
        zx::Status::ACCESS_DENIED.into_raw()
    );
}

#[fuchsia::test]
async fn symlink_clone_preserves_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_symlinks || !harness.config.supports_link_into {
        return;
    }

    let ro_flags =
        fio::Flags::PROTOCOL_SYMLINK | fio::PERM_READABLE | fio::Flags::PERM_GET_ATTRIBUTES;
    let dir = harness.get_directory(
        vec![symlink(TEST_SYMLINK, TEST_SYMLINK_TARGET)],
        harness.dir_rights.all_flags(),
    );
    let symlink = dir.open_node::<fio::SymlinkMarker>(TEST_SYMLINK, ro_flags, None).await.unwrap();

    let (clone_proxy, clone_server) = fidl::endpoints::create_proxy::<fio::SymlinkMarker>();
    symlink.clone(clone_server.into_channel().into()).unwrap();

    // Describe works on the clone.
    let info = clone_proxy.describe().await.unwrap();
    assert_eq!(info.target.as_deref(), Some(TEST_SYMLINK_TARGET));

    // LinkInto is denied on the clone because the cloned connection retained read-only rights.
    let token = get_token(&dir).await.into();
    assert_eq!(
        clone_proxy.link_into(token, "linked_from_clone").await.unwrap().unwrap_err(),
        zx::Status::ACCESS_DENIED.into_raw()
    );
}

#[fuchsia::test]
async fn symlink_openable_pipeline_fails_not_dir() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_symlinks {
        return;
    }

    let dir = harness.get_directory(
        vec![symlink(TEST_SYMLINK, TEST_SYMLINK_TARGET)],
        harness.dir_rights.all_flags(),
    );
    let symlink = dir
        .open_node::<fio::SymlinkMarker>(
            TEST_SYMLINK,
            fio::Flags::PROTOCOL_SYMLINK | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .unwrap();

    let (child_proxy, child_server) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    symlink
        .open(
            "child",
            fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE,
            &Default::default(),
            child_server.into_channel().into(),
        )
        .unwrap();

    assert_matches!(
        child_proxy.take_event_stream().next().await,
        Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. }))
            if epitaph == zx::Status::NOT_DIR
    );

    // Symlink connection remains alive and functional.
    let info = symlink.describe().await.expect("describe failed");
    assert_eq!(info.target.as_deref(), Some(TEST_SYMLINK_TARGET));
}
