// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn unlink_file_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }
    let contents = "abcdef".as_bytes();

    for flags in harness.dir_rights.combinations_containing(fio::W_STAR_DIR) {
        let entries = vec![directory("src", vec![file("file.txt", contents.to_vec())])];
        let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
        let src_dir = dir
            .open_node::<fio::DirectoryMarker>("src", flags | fio::PERM_READABLE, None)
            .await
            .unwrap();

        let file = src_dir
            .open_node::<fio::FileMarker>("file.txt", fio::PERM_READABLE, None)
            .await
            .unwrap();

        src_dir
            .unlink("file.txt", &fio::UnlinkOptions::default())
            .await
            .expect("unlink fidl failed")
            .expect("unlink failed");

        // Check file is gone.
        assert_eq!(
            dir.open_node::<fio::FileMarker>("src/file.txt", fio::PERM_READABLE, None)
                .await
                .unwrap_err(),
            zx::Status::NOT_FOUND
        );

        // Ensure file connection remains usable.
        let read_result = file
            .read(contents.len() as u64)
            .await
            .expect("read failed")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");

        assert_eq!(read_result, contents);
    }
}

#[fuchsia::test]
async fn unlink_file_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }
    let contents = "abcdef".as_bytes();

    for flags in harness.dir_rights.combinations_without(fio::W_STAR_DIR) {
        let entries = vec![directory("src", vec![file("file.txt", contents.to_vec())])];
        let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
        let src_dir = dir.open_node::<fio::DirectoryMarker>("src", flags, None).await.unwrap();

        assert_eq!(
            src_dir
                .unlink("file.txt", &fio::UnlinkOptions::default())
                .await
                .expect("unlink fidl failed")
                .expect_err("unlink succeeded"),
            zx::sys::ZX_ERR_BAD_HANDLE
        );

        // Check file still exists.
        let _ = dir
            .open_node::<fio::NodeMarker>("src/file.txt", fio::Flags::PROTOCOL_NODE, None)
            .await
            .unwrap();
    }
}

#[fuchsia::test]
async fn unlink_directory_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    for flags in harness.dir_rights.combinations_containing(fio::W_STAR_DIR) {
        let entries = vec![directory("src", vec![])];
        let dir = harness.get_directory(entries, flags);
        dir.unlink("src", &fio::UnlinkOptions::default())
            .await
            .expect("unlink fidl failed")
            .expect("unlink failed");
    }
}

#[fuchsia::test]
async fn unlink_directory_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    for flags in harness.dir_rights.combinations_without(fio::W_STAR_DIR) {
        let entries = vec![directory("src", vec![])];
        let dir = harness.get_directory(entries, flags);
        assert_eq!(
            dir.unlink("src", &fio::UnlinkOptions::default())
                .await
                .expect("unlink fidl failed")
                .expect_err("unlink succeeded"),
            zx::sys::ZX_ERR_BAD_HANDLE
        );
    }
}

#[fuchsia::test]
async fn unlink_must_be_directory() {
    let harness = TestHarness::new().await;

    if !harness.config.supports_modify_directory {
        return;
    }

    let entries = vec![directory("dir", vec![]), file("file", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    let must_be_directory = fio::UnlinkOptions {
        flags: Some(fio::UnlinkFlags::MUST_BE_DIRECTORY),
        ..Default::default()
    };
    dir.unlink("dir", &must_be_directory)
        .await
        .expect("unlink fidl failed")
        .expect("unlink dir failed");
    assert_eq!(
        dir.unlink("file", &must_be_directory)
            .await
            .expect("unlink fidl failed")
            .expect_err("unlink file succeeded"),
        zx::sys::ZX_ERR_NOT_DIR
    );
}
