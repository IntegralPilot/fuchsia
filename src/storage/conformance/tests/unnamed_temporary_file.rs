// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn open_file_as_unnamed_temporary() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_unnamed_temporary_file {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());
    let _file_proxy = dir
        .open_node::<fio::FileMarker>(
            ".",
            fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .expect("open failed to open unnamed temporary file");
}

#[fuchsia::test]
async fn open_non_file_as_unnamed_temporary_should_fail() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_unnamed_temporary_file {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    for unallowed_protocol in [
        fio::Flags::empty(),
        fio::Flags::PROTOCOL_DIRECTORY,
        fio::Flags::PROTOCOL_NODE,
        fio::Flags::PROTOCOL_SYMLINK,
    ] {
        assert_eq!(
            dir.open_node::<fio::NodeMarker>(
                ".",
                unallowed_protocol
                    | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                    | fio::PERM_WRITABLE,
                None,
            )
            .await
            .expect_err("opening an unnamed temporary file passed unexpectedly"),
            zx::Status::NOT_SUPPORTED
        );
    }
}

#[fuchsia::test]
async fn open_file_as_unnamed_temporary_fail_when_not_supported() {
    let harness = TestHarness::new().await;
    if harness.config.supports_unnamed_temporary_file {
        return;
    }

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    assert_eq!(
        dir.open_node::<fio::FileMarker>(
            "dir",
            fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .expect_err("opening an unnamed temporary file passed unexpectedly"),
        zx::Status::NOT_SUPPORTED
    );

    // Also check for the case of creating an unnamed temporary file at the current directory. Some
    // filesystems return earlier at empty path (assuming opening current directory),
    assert_eq!(
        dir.open_node::<fio::FileMarker>(
            ".",
            fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .expect_err("opening an unnamed temporary file passed unexpectedly"),
        zx::Status::NOT_SUPPORTED
    );
}

#[fuchsia::test]
async fn open_file_as_unnamed_temporary_in_nonexistent_directory_should_fail() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_unnamed_temporary_file {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());
    assert_eq!(
        dir.open_node::<fio::FileMarker>(
            "foo",
            fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .expect_err("opening an unnamed temporary file in nonexistent directory should fail"),
        zx::Status::NOT_FOUND
    );

    // It should also fail when we pass in "must create" flag.
    assert_eq!(
        dir.open_node::<fio::FileMarker>(
            "foo",
            fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                | fio::Flags::FLAG_MUST_CREATE
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .expect_err("opening an unnamed temporary file in nonexistent directory should fail"),
        zx::Status::NOT_FOUND
    );
}
