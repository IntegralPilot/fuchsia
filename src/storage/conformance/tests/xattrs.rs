// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn test_xattr_read_permissions() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_xattrs {
        return;
    }

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    // 1. Setup: Open with all rights and set an xattr.
    let rw_file = dir
        .open_node::<fio::FileMarker>(TEST_FILE, harness.file_rights.all_flags(), None)
        .await
        .unwrap();

    rw_file
        .set_extended_attribute(
            b"user.test",
            fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
            fio::SetExtendedAttributeMode::Set,
        )
        .await
        .expect("FIDL call failed")
        .expect("set_extended_attribute failed");

    // 2. Test connections with READ_BYTES.
    for flags in harness.file_rights.combinations_containing(fio::Rights::READ_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();

        assert_eq!(
            file.get_extended_attribute(b"user.test")
                .await
                .expect("FIDL call failed")
                .expect("get_extended_attribute failed"),
            fio::ExtendedAttributeValue::Bytes(b"value".to_vec())
        );

        let (iterator_client, iterator_server) =
            fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
        file.list_extended_attributes(iterator_server).expect("list_extended_attributes failed");
        assert_matches!(
            iterator_client.get_next().await,
            Ok(Ok((entries, false))) if entries == vec![b"user.test".to_vec()]
        );
    }

    // 3. Test connections without READ_BYTES.
    for flags in harness.file_rights.combinations_without(fio::Rights::READ_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();

        assert_matches!(
            file.get_extended_attribute(b"user.test")
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );

        let (iterator_client, iterator_server) =
            fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
        file.list_extended_attributes(iterator_server).expect("list_extended_attributes failed");
        assert_matches!(
            iterator_client.get_next().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::BAD_HANDLE
        );
    }
}

#[fuchsia::test]
async fn test_xattr_write_permissions() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_xattrs {
        return;
    }

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    // 1. Test connections with WRITE_BYTES.
    for flags in harness.file_rights.combinations_containing(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();

        file.set_extended_attribute(
            b"user.test",
            fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
            fio::SetExtendedAttributeMode::Set,
        )
        .await
        .expect("FIDL call failed")
        .expect("set_extended_attribute failed");

        file.remove_extended_attribute(b"user.test")
            .await
            .expect("FIDL call failed")
            .expect("remove_extended_attribute failed");
    }

    // 2. Test connections without WRITE_BYTES.
    for flags in harness.file_rights.combinations_without(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();

        assert_matches!(
            file.set_extended_attribute(
                b"user.test",
                fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
                fio::SetExtendedAttributeMode::Set,
            )
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );

        assert_matches!(
            file.remove_extended_attribute(b"user.test")
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );
    }
}

#[fuchsia::test]
async fn test_xattr_node_reference() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_xattrs {
        return;
    }

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    // Open as Node Reference.
    let node_ref = dir
        .open_node::<fio::NodeMarker>(
            TEST_FILE,
            fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_READ_BYTES | fio::Flags::PERM_WRITE_BYTES,
            None,
        )
        .await
        .unwrap();

    // Node reference should return NOT_SUPPORTED for all xattr operations.
    assert_matches!(
        node_ref
            .get_extended_attribute(b"user.test")
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw),
        Err(zx::Status::NOT_SUPPORTED)
    );

    assert_matches!(
        node_ref
            .set_extended_attribute(
                b"user.test",
                fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
                fio::SetExtendedAttributeMode::Set,
            )
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw),
        Err(zx::Status::NOT_SUPPORTED)
    );

    assert_matches!(
        node_ref
            .remove_extended_attribute(b"user.test")
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw),
        Err(zx::Status::NOT_SUPPORTED)
    );

    let (iterator_client, iterator_server) =
        fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
    node_ref.list_extended_attributes(iterator_server).expect("list_extended_attributes failed");
    assert_matches!(
        iterator_client.get_next().await,
        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
            if epitaph == zx::Status::NOT_SUPPORTED
    );
}

#[fuchsia::test]
async fn test_xattr_symlink_permissions() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_xattrs {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    // Create a symlink.
    let (symlink_client, symlink_server) = fidl::endpoints::create_proxy::<fio::SymlinkMarker>();
    let create_result = dir
        .create_symlink("symlink", b"target", symlink_server)
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw);

    if let Err(status) = create_result {
        if status == zx::Status::NOT_SUPPORTED {
            // Symlinks not supported by this filesystem.
            return;
        }
        panic!("create_symlink failed: {:?}", status);
    }

    // Set an xattr on the symlink (harness should have granted WRITABLE to symlink_client).
    symlink_client
        .set_extended_attribute(
            b"user.test",
            fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
            fio::SetExtendedAttributeMode::Set,
        )
        .await
        .expect("FIDL call failed")
        .expect("set_extended_attribute failed");

    // 1. Test connections with READ_BYTES.
    for flags in harness.file_rights.combinations_containing(fio::Rights::READ_BYTES) {
        let symlink = dir
            .open_node::<fio::SymlinkMarker>("symlink", flags | fio::Flags::PROTOCOL_SYMLINK, None)
            .await
            .unwrap();

        assert_eq!(
            symlink
                .get_extended_attribute(b"user.test")
                .await
                .expect("FIDL call failed")
                .expect("get_extended_attribute failed"),
            fio::ExtendedAttributeValue::Bytes(b"value".to_vec())
        );

        let (iterator_client, iterator_server) =
            fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
        symlink.list_extended_attributes(iterator_server).expect("list_extended_attributes failed");
        assert_matches!(
            iterator_client.get_next().await,
            Ok(Ok((entries, false))) if entries == vec![b"user.test".to_vec()]
        );
    }

    // 2. Test connections without READ_BYTES.
    for flags in harness.file_rights.combinations_without(fio::Rights::READ_BYTES) {
        let symlink = dir
            .open_node::<fio::SymlinkMarker>("symlink", flags | fio::Flags::PROTOCOL_SYMLINK, None)
            .await
            .unwrap();

        assert_matches!(
            symlink
                .get_extended_attribute(b"user.test")
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );

        let (iterator_client, iterator_server) =
            fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
        symlink.list_extended_attributes(iterator_server).expect("list_extended_attributes failed");
        assert_matches!(
            iterator_client.get_next().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::BAD_HANDLE
        );
    }

    // 3. Test connections with WRITE_BYTES.
    for flags in harness.file_rights.combinations_containing(fio::Rights::WRITE_BYTES) {
        let symlink = dir
            .open_node::<fio::SymlinkMarker>("symlink", flags | fio::Flags::PROTOCOL_SYMLINK, None)
            .await
            .unwrap();

        symlink
            .set_extended_attribute(
                b"user.test",
                fio::ExtendedAttributeValue::Bytes(b"value2".to_vec()),
                fio::SetExtendedAttributeMode::Set,
            )
            .await
            .expect("FIDL call failed")
            .expect("set_extended_attribute failed");

        symlink
            .remove_extended_attribute(b"user.test")
            .await
            .expect("FIDL call failed")
            .expect("remove_extended_attribute failed");
    }

    // 4. Test connections without WRITE_BYTES.
    for flags in harness.file_rights.combinations_without(fio::Rights::WRITE_BYTES) {
        let symlink = dir
            .open_node::<fio::SymlinkMarker>("symlink", flags | fio::Flags::PROTOCOL_SYMLINK, None)
            .await
            .unwrap();

        assert_matches!(
            symlink
                .set_extended_attribute(
                    b"user.test",
                    fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
                    fio::SetExtendedAttributeMode::Set,
                )
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );

        assert_matches!(
            symlink
                .remove_extended_attribute(b"user.test")
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );
    }
}

#[fuchsia::test]
async fn test_xattr_unsupported() {
    let harness = TestHarness::new().await;
    if harness.config.supports_xattrs {
        return;
    }

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    // 1. With sufficient rights, expect NOT_SUPPORTED.
    let file = dir
        .open_node::<fio::FileMarker>(TEST_FILE, harness.file_rights.all_flags(), None)
        .await
        .unwrap();

    assert_matches!(
        file.get_extended_attribute(b"user.test")
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw),
        Err(zx::Status::NOT_SUPPORTED)
    );

    assert_matches!(
        file.set_extended_attribute(
            b"user.test",
            fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
            fio::SetExtendedAttributeMode::Set,
        )
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw),
        Err(zx::Status::NOT_SUPPORTED)
    );

    assert_matches!(
        file.remove_extended_attribute(b"user.test")
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw),
        Err(zx::Status::NOT_SUPPORTED)
    );

    let (iterator_client, iterator_server) =
        fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
    file.list_extended_attributes(iterator_server).expect("list_extended_attributes failed");
    assert_matches!(
        iterator_client.get_next().await,
        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
            if epitaph == zx::Status::NOT_SUPPORTED
    );

    // 2. Without rights, expect BAD_HANDLE (rights check happens first).
    for flags in harness.file_rights.combinations_without(fio::Rights::READ_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        assert_matches!(
            file.get_extended_attribute(b"user.test")
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );
    }

    for flags in harness.file_rights.combinations_without(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        assert_matches!(
            file.set_extended_attribute(
                b"user.test",
                fio::ExtendedAttributeValue::Bytes(b"value".to_vec()),
                fio::SetExtendedAttributeMode::Set,
            )
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw),
            Err(zx::Status::BAD_HANDLE)
        );
    }
}
