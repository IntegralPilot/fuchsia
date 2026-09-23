// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn file_write_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    for flags in harness.file_rights.combinations_containing(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        let _: u64 = file
            .write("".as_bytes())
            .await
            .expect("write failed")
            .map_err(zx::Status::err_from_raw)
            .expect("write error");
    }
}

#[fuchsia::test]
async fn file_write_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    for flags in harness.file_rights.combinations_without(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        let result = file
            .write("".as_bytes())
            .await
            .expect("write failed")
            .map_err(zx::Status::err_from_raw);
        assert_eq!(result, Err(zx::Status::BAD_HANDLE))
    }
}

#[fuchsia::test]
async fn file_write_at_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    for flags in harness.file_rights.combinations_containing(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        let _: u64 = file
            .write_at("".as_bytes(), 0)
            .await
            .expect("write_at failed")
            .map_err(zx::Status::err_from_raw)
            .expect("write_at error");
    }
}

#[fuchsia::test]
async fn file_write_at_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    for flags in harness.file_rights.combinations_without(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        let result = file
            .write_at("".as_bytes(), 0)
            .await
            .expect("write_at failed")
            .map_err(zx::Status::err_from_raw);
        assert_eq!(result, Err(zx::Status::BAD_HANDLE));
    }
}
