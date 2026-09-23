// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;

#[fuchsia::test]
async fn get_token_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_get_token {
        return;
    }
    // TODO(https://fxbug.dev/346585458): This should only require the MODIFY_DIRECTORY right
    // however the C++ VFS incorrectly checks for WRITE_BYTES instead.
    for dir_flags in harness
        .dir_rights
        .combinations_containing(fio::Rights::WRITE_BYTES | fio::Rights::MODIFY_DIRECTORY)
    {
        let dir = harness.get_directory(vec![], dir_flags);
        let (status, _handle) = dir.get_token().await.expect("get_token failed");
        assert_eq!(status, zx::sys::ZX_OK);
        // Handle is tested in other test cases.
    }
}

#[fuchsia::test]
async fn get_token_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_get_token {
        return;
    }
    // TODO(https://fxbug.dev/346585458): This should only require the MODIFY_DIRECTORY right
    // however the C++ VFS incorrectly checks for WRITE_BYTES instead.
    for dir_flags in harness
        .dir_rights
        .combinations_without(fio::Rights::WRITE_BYTES | fio::Rights::MODIFY_DIRECTORY)
    {
        let dir = harness.get_directory(vec![], dir_flags);
        let (status, _handle) = dir.get_token().await.expect("get_token failed");
        assert_eq!(zx::Status::ok(status), Err(zx::Status::BAD_HANDLE));
    }
}
