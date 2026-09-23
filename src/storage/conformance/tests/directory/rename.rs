// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn rename_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory || !harness.config.supports_get_token {
        return;
    }
    let contents = "abcdef".as_bytes();

    for flags in harness.file_rights.combinations_containing(fio::W_STAR_DIR) {
        let entries = vec![
            directory("src", vec![file("old.txt", contents.to_vec())]),
            directory("dest", vec![]),
        ];
        let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
        let src_dir = dir.open_node::<fio::DirectoryMarker>("src", flags, None).await.unwrap();
        let dest_dir = dir.open_node::<fio::DirectoryMarker>("dest", flags, None).await.unwrap();
        let dest_token = get_token(&dest_dir).await;

        // Rename src/old.txt -> dest/new.txt.
        let status = src_dir
            .rename("old.txt", zx::Event::from(dest_token), "new.txt")
            .await
            .expect("rename failed");
        assert!(status.is_ok());

        // Check dest/new.txt was created and has correct contents.
        assert_eq!(read_file(&dir, "dest/new.txt").await, contents);

        // Check src/old.txt no longer exists.
        assert_eq!(
            dir.open_node::<fio::FileMarker>("src/old.txt", fio::PERM_READABLE, None)
                .await
                .unwrap_err(),
            zx::Status::NOT_FOUND
        );
    }
}

#[fuchsia::test]
async fn rename_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory || !harness.config.supports_get_token {
        return;
    }
    let contents = "abcdef".as_bytes();
    for flags in harness.file_rights.combinations_without(fio::W_STAR_DIR) {
        let entries = vec![
            directory("src", vec![file("old.txt", contents.to_vec())]),
            directory("dest", vec![]),
        ];
        let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
        let src_dir = dir.open_node::<fio::DirectoryMarker>("src", flags, None).await.unwrap();
        let dest_dir = dir
            .open_node::<fio::DirectoryMarker>("dest", harness.dir_rights.all_flags(), None)
            .await
            .unwrap();
        let dest_token = get_token(&dest_dir).await;

        // Renaming "src/old.txt" to "dest/new.txt" should fail as we lack MODIFY_DIRECTORY rights
        // on src_dir.
        let status = src_dir
            .rename("old.txt", zx::Event::from(dest_token), "new.txt")
            .await
            .expect("rename failed");
        assert!(status.is_err());
        assert_eq!(status.err().unwrap(), zx::Status::ACCESS_DENIED.into_raw());
    }
}

#[fuchsia::test]
async fn rename_with_slash_in_path_fails() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory || !harness.config.supports_get_token {
        return;
    }
    let contents = "abcdef".as_bytes();

    for flags in harness.file_rights.combinations_containing(fio::W_STAR_DIR) {
        let entries = vec![
            directory("src", vec![file("old.txt", contents.to_vec())]),
            directory("dest", vec![]),
        ];
        let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
        let src_dir = dir.open_node::<fio::DirectoryMarker>("src", flags, None).await.unwrap();
        let dest_dir = dir
            .open_node::<fio::DirectoryMarker>(
                "dest",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
                None,
            )
            .await
            .unwrap();

        // Including a slash in the src or dest path should fail.
        let status = dir
            .rename("src/old.txt", zx::Event::from(get_token(&dest_dir).await), "new.txt")
            .await
            .expect("rename failed");
        assert!(status.is_err());
        assert_eq!(status.err().unwrap(), zx::Status::INVALID_ARGS.into_raw());
        let status = src_dir
            .rename("old.txt", zx::Event::from(get_token(&dest_dir).await), "nested/new.txt")
            .await
            .expect("rename failed");
        assert!(status.is_err());
        assert_eq!(status.err().unwrap(), zx::Status::INVALID_ARGS.into_raw());
    }
}
