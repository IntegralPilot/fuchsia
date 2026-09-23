// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

// Validate allowed rights for Directory objects.
#[fuchsia::test]
async fn validate_directory_rights() {
    let harness = TestHarness::new().await;
    // Create a test directory and ensure we can open it with all supported rights.
    let entries = vec![file(TEST_FILE, vec![])];
    let _dir = harness.get_directory(entries, harness.dir_rights.all_flags());
}

// Validate allowed rights for File objects (ensures writable files cannot be opened as executable).
#[fuchsia::test]
async fn validate_file_rights() {
    let harness = TestHarness::new().await;
    // Create a test directory with a single File object, and ensure the directory has all rights.
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    // Opening as READABLE must succeed.
    let _ = dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_READABLE, None).await.unwrap();

    if harness.config.supports_mutable_file {
        // Opening as WRITABLE must succeed.

        let _ =
            dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_WRITABLE, None).await.unwrap();
    } else {
        // Opening as WRITABLE must fail.
        assert_eq!(
            dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_WRITABLE, None)
                .await
                .unwrap_err(),
            zx::Status::ACCESS_DENIED
        );
    }
    // An executable file wasn't created, opening as EXECUTABLE must fail.
    dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_EXECUTABLE, None)
        .await
        .expect_err("open succeeded");
}

// Validate allowed rights for ExecutableFile objects (ensures cannot be opened as writable).
#[fuchsia::test]
async fn validate_executable_file_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_executable_file {
        return;
    }
    // Create a test directory with an ExecutableFile object, and ensure the directory has all rights.
    let entries = vec![executable_file(TEST_FILE)];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    // Opening with READABLE/EXECUTABLE should succeed.
    let _ = dir
        .open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_READABLE | fio::PERM_EXECUTABLE, None)
        .await
        .unwrap();
    // Opening with WRITABLE must fail to ensure W^X enforcement.

    assert_eq!(
        dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_WRITABLE, None).await.unwrap_err(),
        zx::Status::ACCESS_DENIED
    );
}

#[fuchsia::test]
async fn open_rights() {
    let harness = TestHarness::new().await;

    const CONTENT: &[u8] = b"content";
    let dir = harness.get_directory(vec![file(TEST_FILE, CONTENT.to_vec())], fio::PERM_READABLE);
    // Should fail to open the file if the rights exceed those allowed by the directory.
    let status = dir
        .open_node::<fio::NodeMarker>(&TEST_FILE, fio::PERM_WRITABLE, None)
        .await
        .expect_err("open should fail if rights exceed those of the parent connection");
    assert_eq!(status, zx::Status::ACCESS_DENIED);

    // Calling open with no rights set is the same as calling open with empty rights.
    let proxy = dir
        .open_node::<fio::FileMarker>(&TEST_FILE, fio::Flags::PROTOCOL_FILE, None)
        .await
        .unwrap();
    assert_eq!(proxy.get_flags().await.unwrap().unwrap(), fio::Flags::PROTOCOL_FILE);

    // Opening with rights that the connection has should succeed.
    let proxy = dir
        .open_node::<fio::FileMarker>(
            &TEST_FILE,
            fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        proxy.get_flags().await.unwrap().unwrap(),
        fio::Flags::PROTOCOL_FILE | fio::Flags::PERM_GET_ATTRIBUTES | fio::Flags::PERM_READ_BYTES
    );
    // We should be able to read from the file, but not write.
    assert_eq!(&fuchsia_fs::file::read(&proxy).await.expect("read failed"), CONTENT);
    assert_matches!(
        fuchsia_fs::file::write(&proxy, "data").await,
        Err(fuchsia_fs::file::WriteError::WriteError(zx::Status::BAD_HANDLE))
    );
}

#[fuchsia::test]
async fn open_invalid() {
    let harness = TestHarness::new().await;

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // It's an error to specify more than one protocol when trying to create an object.
    for create_flag in [
        fio::Flags::FLAG_MAYBE_CREATE,
        fio::Flags::FLAG_MUST_CREATE,
        // FLAG_MUST_CREATE takes precedence over FLAG_MAYBE_CREATE.
        fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::FLAG_MUST_CREATE,
    ] {
        let status = dir
            .open_node::<fio::NodeMarker>(
                "file",
                fio::Flags::PROTOCOL_FILE | fio::Flags::PROTOCOL_DIRECTORY | create_flag,
                None,
            )
            .await
            .expect_err("open should fail if multiple protocols are set with FLAG_*_CREATE");
        assert_eq!(status, zx::Status::INVALID_ARGS);
    }
}

#[fuchsia::test]
async fn open_create_dot_fails_with_already_exists() {
    let harness = TestHarness::new().await;

    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    let status = dir
        .open_node::<fio::DirectoryMarker>(
            ".",
            fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_MUST_CREATE,
            None,
        )
        .await
        .expect_err("open should fail when trying to create the dot path");
    assert_eq!(status, zx::Status::ALREADY_EXISTS);
}

#[fuchsia::test]
async fn open_directory() {
    let harness = TestHarness::new().await;

    let dir = harness
        .get_directory(vec![directory("dir", vec![])], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Should be able to open using the directory protocol.
    let (_, representation) = dir
        .open_node_repr::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_DIRECTORY,
            None,
        )
        .await
        .expect("open using directory protocol failed");
    assert_matches!(representation, fio::Representation::Directory(_));

    // Should also be able to open without specifying an exact protocol due to protocol resolution.
    let (_, representation) = dir
        .open_node_repr::<fio::DirectoryMarker>("dir", fio::Flags::FLAG_SEND_REPRESENTATION, None)
        .await
        .expect("open using node protocol resolution failed");
    assert_matches!(representation, fio::Representation::Directory(_));

    // Should be able to open the file specifying multiple protocols as long as one matches.
    let (_, representation) = dir
        .open_node_repr::<fio::FileMarker>(
            "dir",
            fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::PROTOCOL_FILE
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::PROTOCOL_SYMLINK,
            None,
        )
        .await
        .expect("failed to open directory with multiple protocols");
    assert_matches!(representation, fio::Representation::Directory(_));

    // Attempting to open the directory as a file should fail with ZX_ERR_NOT_FILE.
    let status = dir
        .open_node::<fio::NodeMarker>(
            "dir",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_FILE,
            None,
        )
        .await
        .expect_err("opening directory as file should fail");
    assert_eq!(status, zx::Status::NOT_FILE);

    // Attempting to open with file protocols should fail with ZX_ERR_INVALID_ARGS. It is worth
    // noting that the behaviour for opening file flags with directory is not clearly defined. Linux
    // allows opening a directory with `O_APPEND` but not `O_TRUNC`.
    let status = dir
        .open_node::<fio::NodeMarker>(
            "dir",
            fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::FILE_APPEND
                | fio::Flags::FILE_TRUNCATE,
            None,
        )
        .await
        .expect_err("opening directory as file should fail");
    assert_eq!(status, zx::Status::INVALID_ARGS);

    // Attempting to open the directory as a symbolic link should fail with ZX_ERR_WRONG_TYPE.
    let status = dir
        .open_node::<fio::NodeMarker>(
            "dir",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_SYMLINK,
            None,
        )
        .await
        .expect_err("opening directory as symlink should fail");
    assert_eq!(status, zx::Status::WRONG_TYPE);
}

#[fuchsia::test]
async fn open_file() {
    let harness = TestHarness::new().await;

    const CONTENT: &[u8] = b"content";
    let dir = harness.get_directory(
        vec![file("file", CONTENT.to_vec())],
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    );

    // Should be able to open the file specifying just the file protocol.
    let (_, representation) = dir
        .open_node_repr::<fio::FileMarker>(
            "file",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_FILE,
            None,
        )
        .await
        .expect("failed to open file with file protocol");
    assert_matches!(representation, fio::Representation::File(_));

    // Should also be able to open without specifying an exact protocol due to protocol resolution.
    let (_, representation) = dir
        .open_node_repr::<fio::FileMarker>("file", fio::Flags::FLAG_SEND_REPRESENTATION, None)
        .await
        .expect("failed to open file with protocol resolution");
    assert_matches!(representation, fio::Representation::File(_));

    // Should be able to open the file specifying multiple protocols as long as one matches.
    let (_, representation) = dir
        .open_node_repr::<fio::FileMarker>(
            "file",
            fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::PROTOCOL_FILE
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::PROTOCOL_SYMLINK,
            None,
        )
        .await
        .expect("failed to open file with multiple protocols");
    assert_matches!(representation, fio::Representation::File(_));

    // Attempting to open the file as a directory should fail with ZX_ERR_NOT_DIR.
    let status = dir
        .open_node_repr::<fio::NodeMarker>(
            "file",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_DIRECTORY,
            None,
        )
        .await
        .expect_err("should fail to open file as directory");
    assert_eq!(status, zx::Status::NOT_DIR);

    // Attempting to open the file as a symbolic link should fail with ZX_ERR_WRONG_TYPE.
    let status = dir
        .open_node_repr::<fio::NodeMarker>(
            "file",
            fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PROTOCOL_SYMLINK,
            None,
        )
        .await
        .expect_err("should fail to open file as symlink");
    assert_eq!(status, zx::Status::WRONG_TYPE);
}

#[fuchsia::test]
async fn open_file_append() {
    let harness = TestHarness::new().await;

    if !harness.config.supports_append {
        return;
    }

    let dir = harness.get_directory(
        vec![file("file", b"foo".to_vec())],
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    );

    let proxy = dir
        .open_node::<fio::FileMarker>(
            "file",
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FILE_APPEND,
            None,
        )
        .await
        .unwrap();

    // Append to the file.
    assert_matches!(fuchsia_fs::file::write(&proxy, " bar").await, Ok(()));

    // Read back to check.
    proxy.seek(fio::SeekOrigin::Start, 0).await.expect("seek FIDL failed").expect("seek failed");
    assert_eq!(fuchsia_fs::file::read(&proxy).await.expect("read failed"), b"foo bar");
}

#[fuchsia::test]
async fn open_file_truncate_invalid() {
    let harness = TestHarness::new().await;

    if !harness.config.supports_truncate {
        return;
    }

    let dir = harness.get_directory(vec![file("file", b"foo".to_vec())], fio::PERM_READABLE);

    let status = dir
        .open_node::<fio::FileMarker>("file", fio::Flags::FILE_TRUNCATE, None)
        .await
        .expect_err("open with truncate requires rights to write bytes");
    assert_eq!(status, zx::Status::INVALID_ARGS);
}

#[fuchsia::test]
async fn open_file_truncate() {
    let harness = TestHarness::new().await;

    if !harness.config.supports_truncate {
        return;
    }

    let dir = harness.get_directory(
        vec![file("file", b"foo".to_vec())],
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    );

    let proxy = dir
        .open_node::<fio::FileMarker>(
            "file",
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FILE_TRUNCATE,
            None,
        )
        .await
        .unwrap();

    assert_eq!(fuchsia_fs::file::read(&proxy).await.expect("read failed"), b"");
}

#[fuchsia::test]
async fn open_directory_get_representation() {
    let harness = TestHarness::new().await;

    let dir = harness.get_directory(vec![], fio::PERM_READABLE);

    let (_, representation) = dir
        .open_node_repr::<fio::DirectoryMarker>(
            ".",
            fio::Flags::FLAG_SEND_REPRESENTATION,
            Some(fio::Options {
                attributes: Some(
                    fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::ABILITIES,
                ),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    assert_matches!(
        representation,
        fio::Representation::Directory(fio::DirectoryInfo {
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

#[fuchsia::test]
async fn open_file_get_representation() {
    let harness = TestHarness::new().await;

    let dir = harness.get_directory(vec![file("file", vec![])], fio::PERM_READABLE);

    let (_, representation) = dir
        .open_node_repr::<fio::FileMarker>(
            "file",
            fio::Flags::FLAG_SEND_REPRESENTATION
                | if harness.config.supports_append {
                    fio::Flags::FILE_APPEND
                } else {
                    fio::Flags::empty()
                },
            Some(fio::Options {
                attributes: Some(
                    fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::ABILITIES,
                ),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    assert_matches!(
        representation,
        fio::Representation::File(fio::FileInfo {
            is_append,
            attributes: Some(fio::NodeAttributes2 { mutable_attributes, immutable_attributes }),
            ..
        })
        if mutable_attributes == fio::MutableNodeAttributes::default()
            && immutable_attributes
                == fio::ImmutableNodeAttributes {
                    protocols: Some(fio::NodeProtocolKinds::FILE),
                    abilities: Some(harness.supported_file_abilities()),
                    ..Default::default()
                }
            && is_append == Some(harness.config.supports_append)
    );
}

#[fuchsia::test]
async fn open_dir_inherit_rights() {
    let harness = TestHarness::new().await;

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    let proxy = dir
        .open_node::<fio::DirectoryMarker>(
            ".",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::PERM_READABLE
                | fio::Flags::PERM_INHERIT_WRITE
                | fio::Flags::PERM_INHERIT_EXECUTE,
            None,
        )
        .await
        .unwrap();
    // We should inherit only write rights since the parent connection lacks executable rights.
    assert_eq!(
        proxy.get_flags().await.unwrap().unwrap(),
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE | fio::PERM_WRITABLE,
    );
}

#[fuchsia::test]
async fn open_request_attributes_rights_failure() {
    let harness = TestHarness::new().await;

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Open with no rights.
    let proxy =
        dir.open_node::<fio::DirectoryMarker>(".", fio::Flags::empty(), None).await.unwrap();

    // Requesting attributes when re-opening via `proxy` should fail without `PERM_GET_ATTRIBUTES`.
    assert_matches!(
        proxy
            .open_node::<fio::DirectoryMarker>(
                ".",
                fio::Flags::FLAG_SEND_REPRESENTATION,
                Some(fio::Options {
                    attributes: Some(fio::NodeAttributesQuery::PROTOCOLS),
                    ..Default::default()
                })
            )
            .await,
        Err(zx::Status::ACCESS_DENIED)
    );
}

#[fuchsia::test]
async fn open_existing_directory() {
    let harness = TestHarness::new().await;

    let dir = harness
        .get_directory(vec![directory("dir", vec![])], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Should not be able to open non-existing directory entry without `FLAG_*_CREATE`.
    let status = dir
        .open_node::<fio::NodeMarker>(
            "this_path_does_not_exist",
            fio::Flags::PROTOCOL_DIRECTORY,
            None,
        )
        .await
        .expect_err("should fail to open non-existing entry when OpenExisting is set");
    assert_eq!(status, zx::Status::NOT_FOUND);

    dir.open_node::<fio::NodeMarker>(
        "dir",
        fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_MAYBE_CREATE,
        None,
    )
    .await
    .expect("failed to open existing entry");
}

#[fuchsia::test]
async fn open_directory_as_node_reference() {
    let harness = TestHarness::new().await;

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let directory_proxy = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::PROTOCOL_NODE
                | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .expect("open failed");

    // We are allowed to call `get_attributes` on a node reference
    directory_proxy
        .get_attributes(fio::NodeAttributesQuery::empty())
        .await
        .unwrap()
        .expect("get_attributes failed");

    // Make sure that the directory protocol *was not* served by calling a directory-only method.
    // It should fail with PEER_CLOSED as this method is unknown.
    let err = directory_proxy
        .read_dirents(1)
        .await
        .expect_err("calling a directory-specific method on a node reference is not be allowed");
    assert!(err.is_closed());
}

#[fuchsia::test]
async fn open_file_as_node_reference() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy = dir
        .open_node::<fio::FileMarker>(
            TEST_FILE,
            fio::Flags::PROTOCOL_FILE | fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .expect("open failed");

    // We are allowed to call `get_attributes` on a node reference
    file_proxy
        .get_attributes(fio::NodeAttributesQuery::empty())
        .await
        .unwrap()
        .expect("get_attributes failed");

    // Make sure that the directory protocol *was not* served by calling a file-only method.
    // It should fail with PEER_CLOSED as this method is unknown.
    let err = file_proxy
        .read(0)
        .await
        .expect_err("calling a file-specific method on a node reference is not be allowed");
    assert!(err.is_closed());
}
