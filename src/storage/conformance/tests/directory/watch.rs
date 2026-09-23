// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::{WatchEvent, WatchMessage, Watcher};
use futures::StreamExt;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;
use std::path::PathBuf;

#[fuchsia::test]
async fn watch_dir_existing() {
    let harness = TestHarness::new().await;

    let entries = vec![file("foo", b"test".to_vec())];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    let mut watcher = Watcher::new(&dir).await.expect("making watcher");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::EXISTING, filename: PathBuf::from(".") },
    );
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::EXISTING, filename: PathBuf::from("foo") },
    );
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::IDLE, filename: PathBuf::new() },
    );
}

#[fuchsia::test]
async fn watch_dir_added_removed() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    let mut watcher = Watcher::new(&dir).await.expect("making watcher");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::EXISTING, filename: PathBuf::from(".") },
    );
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::IDLE, filename: PathBuf::new() },
    );

    let _ = dir
        .open_node::<fio::FileMarker>(
            "foo",
            fio::Flags::FLAG_MUST_CREATE | fio::Flags::PROTOCOL_FILE,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::ADD_FILE, filename: PathBuf::from("foo") },
    );

    let _ = dir
        .open_node::<fio::DirectoryMarker>(
            "dir",
            fio::Flags::FLAG_MUST_CREATE | fio::Flags::PROTOCOL_DIRECTORY,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::ADD_FILE, filename: PathBuf::from("dir") },
    );

    dir.unlink("foo", &fio::UnlinkOptions::default())
        .await
        .expect("fidl error")
        .expect("unlink error");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::REMOVE_FILE, filename: PathBuf::from("foo") },
    );
}

#[fuchsia::test]
async fn watch_dir_existing_file_create_does_not_generate_new_event() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    let mut watcher = Watcher::new(&dir).await.expect("making watcher");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::EXISTING, filename: PathBuf::from(".") },
    );
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::IDLE, filename: PathBuf::new() },
    );

    let _ = dir
        .open_node::<fio::FileMarker>(
            "foo",
            fio::Flags::FLAG_MUST_CREATE | fio::Flags::PROTOCOL_FILE,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::ADD_FILE, filename: PathBuf::from("foo") },
    );

    // Ensure FLAG_MAYBE_CREATE doesn't trigger a new event as the file should already exist.
    let _ = dir
        .open_node::<fio::FileMarker>(
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_FILE,
            None,
        )
        .await
        .unwrap();
    // Since we are testing that the previous open does _not_ generate an event, do something else
    // that will generate a different event and make sure that is the next event.
    dir.unlink("foo", &fio::UnlinkOptions::default())
        .await
        .expect("fidl error")
        .expect("unlink error");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::REMOVE_FILE, filename: PathBuf::from("foo") },
    );
}

#[fuchsia::test]
async fn watch_dir_rename() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_modify_directory {
        return;
    }

    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    let mut watcher = Watcher::new(&dir).await.expect("making watcher");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::EXISTING, filename: PathBuf::from(".") },
    );
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::IDLE, filename: PathBuf::new() },
    );

    let _ = dir
        .open_node::<fio::FileMarker>(
            "foo",
            fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE | fio::Flags::PROTOCOL_FILE,
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::ADD_FILE, filename: PathBuf::from("foo") },
    );

    let (status, token) = dir.get_token().await.unwrap();
    assert_eq!(status, zx::sys::ZX_OK);
    let token = token.unwrap();
    dir.rename("foo", token.into(), "bar").await.expect("fidl error").expect("rename error");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::REMOVE_FILE, filename: PathBuf::from("foo") },
    );
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::ADD_FILE, filename: PathBuf::from("bar") },
    );

    dir.unlink("bar", &fio::UnlinkOptions::default())
        .await
        .expect("fidl error")
        .expect("unlink error");
    assert_eq!(
        watcher.next().await.expect("watcher stream empty").expect("watch message error"),
        WatchMessage { event: WatchEvent::REMOVE_FILE, filename: PathBuf::from("bar") },
    );
}

#[fuchsia::test]
async fn watch_requires_enumerate_right() {
    let harness = TestHarness::new().await;
    for dir_flags in harness.dir_rights.combinations_without(fio::Operations::ENUMERATE) {
        let dir = harness.get_directory(vec![], dir_flags);

        let (_client, server) = fidl::endpoints::create_endpoints();
        let status = dir.watch(fio::WatchMask::empty(), 0, server).await.expect("FIDL call failed");
        assert_eq!(zx::Status::err_from_raw(status), zx::Status::ACCESS_DENIED);
    }
}
