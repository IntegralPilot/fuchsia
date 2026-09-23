// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::{DirEntry, DirentKind, parse_dir_entries};
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

async fn read_dirents(
    dir: &fio::DirectoryProxy,
    max_bytes: u64,
) -> Result<Vec<DirEntry>, zx::Status> {
    let (status, buf) = dir.read_dirents(max_bytes).await.expect("FIDL call failed");
    zx::Status::ok(status)?;
    Ok(parse_dir_entries(&buf)
        .into_iter()
        .map(|result| result.expect("invalid directory entry"))
        .collect())
}

#[fuchsia::test]
async fn buffer_too_small() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());
    for buffer_size in 0..=10 {
        assert_eq!(
            read_dirents(&dir, buffer_size).await,
            Err(zx::Status::BUFFER_TOO_SMALL),
            "A buffer of size {} should be too small",
            buffer_size
        );
    }
    // Check that read_dirents still works after getting a BUFFER_TOO_SMALL.
    let entries = read_dirents(&dir, 11).await.expect("'.' should fit in 11 bytes");
    assert_eq!(entries, vec![DirEntry { name: ".".to_string(), kind: DirentKind::Directory }]);
}

#[fuchsia::test]
async fn empty_directory_contains_dot() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());
    let entries = read_dirents(&dir, 11).await.expect("'.' should fit in 11 bytes");
    assert_eq!(entries, vec![DirEntry { name: ".".to_string(), kind: DirentKind::Directory }]);
    let entries = read_dirents(&dir, 4096).await.expect("no more entries");
    assert_eq!(entries, vec![]);
}

#[fuchsia::test]
async fn multiple_read_dirents_calls() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(
        vec![file("a", vec![]), directory("b", vec![]), file("c", vec![])],
        harness.dir_rights.all_flags(),
    );
    let entries1 = read_dirents(&dir, 11).await.expect("read_dirents should succeed");
    assert_eq!(entries1.len(), 1);
    let entries2 = read_dirents(&dir, 25).await.expect("read_dirents should succeed");
    assert_eq!(entries2.len(), 2);
    let entries3 = read_dirents(&dir, 13).await.expect("read_dirents should succeed");
    assert_eq!(entries3.len(), 1);
    let entries4 = read_dirents(&dir, 13).await.expect("read_dirents should succeed");
    assert_eq!(entries4, vec![]);

    let mut entries = entries1;
    entries.extend(entries2);
    entries.extend(entries3);
    entries.sort();
    assert_eq!(
        entries,
        vec![
            DirEntry { name: ".".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "a".to_string(), kind: DirentKind::File },
            DirEntry { name: "b".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "c".to_string(), kind: DirentKind::File }
        ]
    );
}

#[fuchsia::test]
async fn read_dirents_rewind_read_dirents() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(
        vec![file("a", vec![]), directory("b", vec![]), file("c", vec![])],
        harness.dir_rights.all_flags(),
    );

    let entries1 = read_dirents(&dir, 22).await.expect("read_dirents should succeed");
    assert_eq!(entries1.len(), 2);
    dir.rewind().await.expect("rewind should succeed");
    let entries2 = read_dirents(&dir, 44).await.expect("read_dirents should succeed");
    assert_eq!(entries2.len(), 4);
    let entries3 = read_dirents(&dir, 4096).await.expect("read_dirents should succeed");
    assert_eq!(entries3, vec![]);

    // The entries can be returned in any order so we can't verify which ones are in `entries1` but
    // there are at least 2. The second read_dirents call will return all entries. Merging the 2
    // lists, sorting, and deduping shouldn't have extra entries.
    let mut entries = entries1;
    entries.extend(entries2);
    entries.sort();
    entries.dedup();
    assert_eq!(
        entries,
        vec![
            DirEntry { name: ".".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "a".to_string(), kind: DirentKind::File },
            DirEntry { name: "b".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "c".to_string(), kind: DirentKind::File }
        ]
    );
}

#[fuchsia::test]
async fn zero_buffer_after_all_entries_is_ok() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    read_dirents(&dir, 4096).await.expect("read_dirents should succeed");
    // A zero sized buffer isn't large enough to fit any entries but there shouldn't be any entries
    // so the call should still succeed.
    read_dirents(&dir, 0).await.expect("read_dirents should succeed");
}

#[fuchsia::test]
async fn read_dirents_requires_enumerate_right() {
    let harness = TestHarness::new().await;
    for dir_flags in harness.dir_rights.combinations_without(fio::Operations::ENUMERATE) {
        let dir = harness.get_directory(vec![], dir_flags);

        let (status, _buf) = dir.read_dirents(4096).await.expect("FIDL call failed");
        assert_eq!(zx::Status::err_from_raw(status), zx::Status::ACCESS_DENIED);
    }
}

#[fuchsia::test]
async fn rewind_requires_enumerate_right() {
    let harness = TestHarness::new().await;
    for dir_flags in harness.dir_rights.combinations_without(fio::Operations::ENUMERATE) {
        let dir = harness.get_directory(vec![], dir_flags);

        let status = dir.rewind().await.expect("FIDL call failed");
        assert_eq!(zx::Status::err_from_raw(status), zx::Status::ACCESS_DENIED);
    }
}
