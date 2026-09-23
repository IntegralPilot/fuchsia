// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn dir_get_flags() {
    let harness = TestHarness::new().await;

    let dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    let flags = dir.get_flags().await.expect("get_flags failed").expect("Failed to get node flags");
    assert_eq!(flags, fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE | fio::PERM_WRITABLE);
}

#[fuchsia::test]
async fn file_get_flags() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, fio::PERM_READABLE);

    let file = dir
        .open_node::<fio::FileMarker>(
            &TEST_FILE,
            fio::PERM_READABLE | fio::Flags::FILE_APPEND,
            None,
        )
        .await
        .expect("open failed");

    let flags =
        file.get_flags().await.expect("get_flags failed").expect("Failed to get node flags");
    assert_eq!(
        flags,
        fio::Flags::PROTOCOL_FILE
            | fio::Flags::PERM_READ_BYTES
            | fio::Flags::PERM_GET_ATTRIBUTES
            | fio::Flags::FILE_APPEND
    );
}

#[fuchsia::test]
async fn node_reference_get_flags() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, fio::PERM_READABLE);

    let node_reference = dir
        .open_node::<fio::NodeMarker>(
            &TEST_FILE,
            fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .expect("open failed");

    let flags = node_reference
        .get_flags()
        .await
        .expect("get_flags failed")
        .expect("Failed to get node flags");
    assert_eq!(flags, fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES);

    // Unless `fio::Flags::PERM_GET_ATTRIBUTES` is specified, the opened connection will not have
    // that right.
    let node_reference = dir
        .open_node::<fio::NodeMarker>(&TEST_FILE, fio::Flags::PROTOCOL_NODE, None)
        .await
        .expect("open failed");

    let flags = node_reference
        .get_flags()
        .await
        .expect("get_flags failed")
        .expect("Failed to get node flags");
    assert_eq!(flags, fio::Flags::PROTOCOL_NODE);
}

#[fuchsia::test]
async fn file_set_flags() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, fio::PERM_READABLE);
    let file = dir
        .open_node::<fio::FileMarker>(&TEST_FILE, fio::Flags::empty(), None)
        .await
        .expect("open failed");

    // Check that no rights were set.
    let flags =
        file.get_flags().await.expect("get_flags failed").expect("Failed to get node flags");
    assert_eq!(flags, fio::Flags::PROTOCOL_FILE);

    // We should be able to do this without any rights
    file.set_flags(fio::Flags::FILE_APPEND)
        .await
        .expect("set_flags failed")
        .expect("Failed to set node flags");

    // Check that `fio::Flags::FILE_APPEND` was set.
    let flags =
        file.get_flags().await.expect("get_flags failed").expect("Failed to get node flags");
    assert_eq!(flags, fio::Flags::PROTOCOL_FILE | fio::Flags::FILE_APPEND);
}

#[fuchsia::test]
async fn file_set_flags_empty_clears_append_mode() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, fio::PERM_READABLE);
    let file = dir
        .open_node::<fio::FileMarker>(&TEST_FILE, fio::Flags::FILE_APPEND, None)
        .await
        .expect("open failed");
    let flags =
        file.get_flags().await.expect("get_flags failed").expect("Failed to get node flags");
    assert_eq!(flags, fio::Flags::PROTOCOL_FILE | fio::Flags::FILE_APPEND);

    file.set_flags(fio::Flags::empty())
        .await
        .expect("set_flags failed")
        .expect("Failed to set node flags");

    let flags =
        file.get_flags().await.expect("get_flags failed").expect("Failed to get node flags");
    assert_eq!(flags, fio::Flags::PROTOCOL_FILE);
}

#[fuchsia::test]
async fn file_set_flags_invalid_flags() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, fio::PERM_READABLE);
    let file = dir
        .open_node::<fio::FileMarker>(&TEST_FILE, fio::Flags::empty(), None)
        .await
        .expect("open failed");

    // The only valid flag to set with fuchsia.io/Node.SetFlags is fuchsia.io/Flags.FILE_APPEND.
    let err = file
        .set_flags(fio::Flags::FILE_APPEND | fio::Flags::PERM_GET_ATTRIBUTES)
        .await
        .expect("set_flags failed")
        .map_err(zx::Status::err_from_raw)
        .expect_err("set_flags only supports setting Flags.FILE_APPEND");
    assert_eq!(err, zx::Status::INVALID_ARGS);
}

#[fuchsia::test]
async fn dir_set_flags_not_supported() {
    let harness = TestHarness::new().await;

    let dir = harness.get_directory(vec![], fio::Flags::empty());

    let err = dir
        .set_flags(fio::Flags::empty())
        .await
        .expect("set_flags failed")
        .map_err(zx::Status::err_from_raw)
        .expect_err("set_flags should be unsupported for directory nodes");
    assert_eq!(err, zx::Status::NOT_SUPPORTED);
}

#[fuchsia::test]
async fn node_reference_set_flags_not_supported() {
    let harness = TestHarness::new().await;

    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, fio::PERM_READABLE);

    let node_reference = dir
        .open_node::<fio::NodeMarker>(&TEST_FILE, fio::Flags::PROTOCOL_NODE, None)
        .await
        .expect("open failed");

    let err = node_reference
        .set_flags(fio::Flags::empty())
        .await
        .expect("set_flags failed")
        .map_err(zx::Status::err_from_raw)
        .expect_err("set_flags should be unsupported for directory nodes");
    assert_eq!(err, zx::Status::NOT_SUPPORTED);
}
