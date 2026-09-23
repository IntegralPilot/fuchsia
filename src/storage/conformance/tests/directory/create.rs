// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn create_directory() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // A request to create a new object requires that the parent connection has the right to modify.
    let dir_with_sufficient_rights = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PERM_MODIFY_DIRECTORY
                | fio::Flags::PERM_TRAVERSE
                | fio::Flags::PERM_ENUMERATE,
            None,
        )
        .await
        .expect("open failed.");

    // Trying to create the file again with MUST_CREATE should fail with ALREADY_EXISTS.
    assert_eq!(
        dir.open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PERM_MODIFY_DIRECTORY,
            None,
        )
        .await
        .unwrap_err(),
        zx::Status::ALREADY_EXISTS
    );

    let (_, representation) = dir_with_sufficient_rights
        .open_node_repr::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MAYBE_CREATE,
            None,
        )
        .await
        .expect("open with directory protocol failed.");
    assert_matches!(representation, fio::Representation::Directory(_));

    assert_matches!(
        dir_with_sufficient_rights
            .open_node::<fio::DirectoryMarker>("dir", fio::Flags::FLAG_MAYBE_CREATE, None,)
            .await
            .expect_err("open should fail when creating directory without specifiying a protocol."),
        zx::Status::INVALID_ARGS
    );
}

#[fuchsia::test]
async fn create_directory_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // A request to create a new object requires that the parent connection has the right to modify.
    let dir_with_insufficient_rights = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PERM_TRAVERSE
                | fio::Flags::PERM_ENUMERATE,
            None,
        )
        .await
        .expect("open failed.");

    // Should fail without update_attribues
    assert_matches!(
        dir_with_insufficient_rights
            .open_node::<fio::DirectoryMarker>(
                "dir",
                fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_MAYBE_CREATE,
                None,
            )
            .await
            .expect_err(
                "create directory should fail when parent node does not have modify rights."
            ),
        zx::Status::ACCESS_DENIED
    );
}

#[fuchsia::test]
async fn create_directory_with_create_attributes() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory
        || !harness.config.supported_attributes.contains(fio::NodeAttributesQuery::MODE)
    {
        return;
    }

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // A request to create create a new object requires that the parent directory has the right to
    // modify directory.
    let dir_with_sufficient_rights = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PERM_MODIFY_DIRECTORY
                | fio::Flags::PERM_GET_ATTRIBUTES
                | fio::Flags::PERM_TRAVERSE
                | fio::Flags::PERM_ENUMERATE,
            None,
        )
        .await
        .expect("open failed.");

    let (_, representation) = dir_with_sufficient_rights
        .open_node_repr::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MAYBE_CREATE,
            Some(fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    mode: Some(111),
                    ..Default::default()
                }),
                attributes: Some(fio::NodeAttributesQuery::MODE),
                ..Default::default()
            }),
        )
        .await
        .expect("create directory failed.");
    assert_matches!(representation, fio::Representation::Directory(_));
    assert_matches!(
        representation,
        fio::Representation::Directory(fio::DirectoryInfo {
            attributes: Some(fio::NodeAttributes2 { mutable_attributes, immutable_attributes }),
            ..
        })
        if immutable_attributes == fio::ImmutableNodeAttributes::default()
            && mutable_attributes == fio::MutableNodeAttributes {
                mode: Some(111),
                ..Default::default()
            }
    );
}

#[fuchsia::test]
async fn open_directory_with_never_create_and_create_attributes() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory
        || !harness.config.supported_attributes.contains(fio::NodeAttributesQuery::MODE)
    {
        return;
    }

    let dir = harness
        .get_directory(vec![directory("dir", vec![])], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Only open an existing object, never create a new object.
    let flags = fio::Flags::FLAG_SEND_REPRESENTATION;

    let status = dir
        .open_node_repr::<fio::DirectoryMarker>(
            "dir",
            flags,
            Some(fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    mode: Some(111),
                    ..Default::default()
                }),
                attributes: Some(fio::NodeAttributesQuery::MODE),
                ..Default::default()
            }),
        )
        .await
        .expect_err("open should fail when mode is never create and setting create attributes.");
    assert_matches!(status, zx::Status::INVALID_ARGS);
}

#[fuchsia::test]
async fn create_file() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // A request to create a new object requires that the parent connection has the right to modify.
    let dir_with_sufficient_rights = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PERM_MODIFY_DIRECTORY
                | fio::Flags::PERM_TRAVERSE
                | fio::Flags::PERM_ENUMERATE,
            None,
        )
        .await
        .expect("open failed.");

    let (_, representation) = dir_with_sufficient_rights
        .open_node_repr::<fio::FileMarker>(
            TEST_FILE,
            fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_MAYBE_CREATE,
            None,
        )
        .await
        .expect("open with file protocol failed.");
    assert_matches!(representation, fio::Representation::File(_));
}

#[fuchsia::test]
async fn create_file_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // A request to create a new object requires that the parent connection has the right to modify.
    let dir_with_insufficient_rights = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PERM_TRAVERSE
                | fio::Flags::PERM_ENUMERATE,
            None,
        )
        .await
        .expect("open failed.");

    // Should fail without update_attribues
    assert_matches!(
        dir_with_insufficient_rights
            .open_node::<fio::FileMarker>(
                TEST_FILE,
                fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_MAYBE_CREATE,
                None,
            )
            .await
            .expect_err("create file should fail when parent node does not have modify rights."),
        zx::Status::ACCESS_DENIED
    );
}

#[fuchsia::test]
async fn create_file_with_create_attributes() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory
        || !harness.config.supported_attributes.contains(fio::NodeAttributesQuery::MODE)
    {
        return;
    }
    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // A request to create create a new object requires that the parent directory has the right to
    // modify directory.
    let dir_with_sufficient_rights = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PERM_MODIFY_DIRECTORY
                | fio::Flags::PERM_GET_ATTRIBUTES
                | fio::Flags::PERM_TRAVERSE
                | fio::Flags::PERM_ENUMERATE,
            None,
        )
        .await
        .expect("open failed.");

    let (_, representation) = dir_with_sufficient_rights
        .open_node_repr::<fio::FileMarker>(
            TEST_FILE,
            fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_MAYBE_CREATE,
            Some(fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    mode: Some(123),
                    ..Default::default()
                }),
                attributes: Some(fio::NodeAttributesQuery::MODE),
                ..Default::default()
            }),
        )
        .await
        .expect("create file failed.");
    assert_matches!(representation, fio::Representation::File(_));
    assert_matches!(
        representation,
        fio::Representation::File(fio::FileInfo {
            attributes: Some(fio::NodeAttributes2 { mutable_attributes, immutable_attributes }),
            ..
        })
        if immutable_attributes == fio::ImmutableNodeAttributes::default()
            && mutable_attributes == fio::MutableNodeAttributes {
                mode: Some(123),
                ..Default::default()
            }
    );
}

#[fuchsia::test]
async fn open_file_with_never_create_and_create_attributes() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory
        || !harness.config.supported_attributes.contains(fio::NodeAttributesQuery::MODE)
    {
        return;
    }

    let dir = harness
        .get_directory(vec![file(TEST_FILE, vec![])], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Only open an existing object, never create a new object.
    let flags = fio::Flags::FLAG_SEND_REPRESENTATION;

    let status = dir
        .open_node_repr::<fio::FileMarker>(
            TEST_FILE,
            flags,
            Some(fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    mode: Some(123),
                    ..Default::default()
                }),
                attributes: Some(fio::NodeAttributesQuery::MODE),
                ..Default::default()
            }),
        )
        .await
        .expect_err("open should fail when mode is never create and setting create attributes.");
    assert_matches!(status, zx::Status::INVALID_ARGS);
}
