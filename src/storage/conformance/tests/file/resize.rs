// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn file_resize_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_truncate {
        return;
    }
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    for flags in harness.file_rights.combinations_containing(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        file.resize(0)
            .await
            .expect("resize failed")
            .map_err(zx::Status::err_from_raw)
            .expect("resize error")
    }
}

#[fuchsia::test]
async fn file_resize_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_truncate {
        return;
    }
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    for flags in harness.file_rights.combinations_without(fio::Rights::WRITE_BYTES) {
        let file = dir.open_node::<fio::FileMarker>(TEST_FILE, flags, None).await.unwrap();
        let result = file.resize(0).await.expect("resize failed").map_err(zx::Status::err_from_raw);
        assert_eq!(result, Err(zx::Status::BAD_HANDLE));
    }
}
