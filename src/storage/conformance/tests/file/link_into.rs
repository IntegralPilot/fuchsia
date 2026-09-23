// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

const CONTENTS: &[u8] = b"abcdef";

struct Fixture {
    _harness: TestHarness,
    dir: fio::DirectoryProxy,
    file: fio::FileProxy,
}

impl Fixture {
    async fn new(flags: fio::Flags) -> Option<Self> {
        let harness = TestHarness::new().await;
        if !harness.config.supports_link_into || !harness.config.supports_get_token {
            return None;
        }
        let entries = vec![file(TEST_FILE, CONTENTS.to_vec()), file("existing", vec![])];
        let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        Some(Self { _harness: harness, dir, file })
    }

    async fn get_token(&self) -> zx::Event {
        io_conformance_util::get_token(&self.dir).await.into()
    }
}

#[fuchsia::test]
async fn file_link_into() {
    let Some(fixture) = Fixture::new(fio::PERM_READABLE | fio::PERM_WRITABLE).await else {
        return;
    };

    fixture
        .file
        .link_into(fixture.get_token().await, "linked")
        .await
        .expect("link_into (FIDL) failed")
        .expect("link_into failed");

    assert_eq!(read_file(&fixture.dir, "linked").await, CONTENTS);
}

#[fuchsia::test]
async fn file_link_into_bad_name() {
    let Some(fixture) = Fixture::new(fio::PERM_READABLE | fio::PERM_WRITABLE).await else {
        return;
    };

    for bad_name in ["/linked", ".", "..", "\0"] {
        assert_eq!(
            fixture
                .file
                .link_into(fixture.get_token().await, bad_name)
                .await
                .expect("link_into (FIDL) failed")
                .expect_err("link_into succeeded"),
            zx::Status::INVALID_ARGS.into_raw()
        );
    }
}

#[fuchsia::test]
async fn file_link_into_insufficient_rights() {
    for flags in [fio::PERM_READABLE, fio::PERM_WRITABLE] {
        let Some(fixture) = Fixture::new(flags).await else { return };
        assert_eq!(
            fixture
                .file
                .link_into(fixture.get_token().await, "linked")
                .await
                .expect("link_into (FIDL) failed")
                .expect_err("link_into succeeded"),
            zx::Status::ACCESS_DENIED.into_raw()
        );
    }
}

#[fuchsia::test]
async fn file_link_into_for_unlinked_file() {
    let Some(fixture) = Fixture::new(fio::PERM_READABLE | fio::PERM_WRITABLE).await else {
        return;
    };

    fixture
        .dir
        .unlink(TEST_FILE, &fio::UnlinkOptions::default())
        .await
        .expect("unlink (FIDL) failed")
        .expect("unlink failed");

    assert_eq!(
        fixture
            .file
            .link_into(fixture.get_token().await, "linked")
            .await
            .expect("link_into (FIDL) failed")
            .expect_err("link_into succeeded"),
        zx::Status::NOT_FOUND.into_raw()
    );
}

#[fuchsia::test]
async fn file_link_into_existing() {
    let Some(fixture) = Fixture::new(fio::PERM_READABLE | fio::PERM_WRITABLE).await else {
        return;
    };

    assert_eq!(
        fixture
            .file
            .link_into(fixture.get_token().await, "existing")
            .await
            .expect("link_into (FIDL) failed")
            .expect_err("link_into succeeded"),
        zx::Status::ALREADY_EXISTS.into_raw()
    );
}

#[fuchsia::test]
async fn file_link_into_target_unlinked_dir() {
    let Some(fixture) = Fixture::new(fio::PERM_READABLE | fio::PERM_WRITABLE).await else {
        return;
    };

    let target_dir = fixture
        .dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::FLAG_MUST_CREATE
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .unwrap();

    let token = get_token(&target_dir).await.into();

    fixture
        .dir
        .unlink("dir", &fio::UnlinkOptions::default())
        .await
        .expect("unlink (FIDL) failed")
        .expect("unlink failed");

    assert_eq!(
        fixture
            .file
            .link_into(token, "existing")
            .await
            .expect("link_into (FIDL) failed")
            .expect_err("link_into succeeded"),
        zx::Status::ACCESS_DENIED.into_raw()
    );
}

#[fuchsia::test]
async fn unnamed_temporary_file_can_link_into_named_file() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_unnamed_temporary_file {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    let temporary_file = dir
        .open_node::<fio::FileMarker>(
            ".",
            fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .expect("open failed to open unnamed temporary file");
    temporary_file
        .write(CONTENTS)
        .await
        .expect("write (FIDL) failed")
        .expect("write to file failed");

    let token = get_token(&dir).await.into();

    temporary_file
        .link_into(token, "foo")
        .await
        .expect("link_into (FIDL) failed")
        .expect("link_into failed");

    assert_eq!(read_file(&dir, "foo").await, CONTENTS);
}

#[fuchsia::test]
async fn unlinkable_unnamed_temporary_should_fail_link_into() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_unnamed_temporary_file {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    let temporary_file = dir
        .open_node::<fio::FileMarker>(
            ".",
            fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
                | fio::FLAG_TEMPORARY_AS_NOT_LINKABLE
                | fio::PERM_WRITABLE,
            None,
        )
        .await
        .expect("open failed to open unnamed temporary file");

    let token = get_token(&dir).await.into();

    assert_eq!(
        temporary_file
            .link_into(token, "foo")
            .await
            .expect("link_into (FIDL) failed")
            .map_err(zx::Status::err_from_raw)
            .expect_err("link_into passed unexpectedly for unlinkable file"),
        zx::Status::NOT_FOUND
    );
}
