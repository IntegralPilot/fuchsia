// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

/// Creates a directory with a remote mount inside of it, and checks that the remote can be opened.
#[fuchsia::test]
async fn open_remote_directory_test() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_remote_dir {
        return;
    }
    let remote_name = "remote_directory";
    let remote_dir = harness.get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Create a directory with the remote directory inside of it.
    let entries = vec![remote_directory(remote_name, remote_dir)];
    let dir = harness.get_directory(entries, fio::PERM_READABLE | fio::PERM_WRITABLE);

    dir.open_node::<fio::DirectoryMarker>(
        remote_name,
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE | fio::PERM_WRITABLE,
        None,
    )
    .await
    .expect("failed to open remote directory");
}

/// Creates a directory with a remote mount containing a file inside of it, and checks that the
/// file can be opened through the remote.
#[fuchsia::test]
async fn open_remote_file_test() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_remote_dir {
        return;
    }

    let remote_name = "remote_directory";
    let remote_entries = vec![file(TEST_FILE, vec![])];
    let remote_dir = harness.get_directory(remote_entries, fio::PERM_READABLE);

    // Create a directory with the remote directory inside of it.
    let entries = vec![remote_directory(remote_name, remote_dir)];
    let dir = harness.get_directory(entries, fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Test opening file by opening the remote directory first and then opening the file.
    let remote_dir_proxy = dir
        .open_node::<fio::DirectoryMarker>(
            remote_name,
            fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
            None,
        )
        .await
        .expect("failed to open remote directory");

    remote_dir_proxy
        .open_node::<fio::NodeMarker>(
            TEST_FILE,
            fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE,
            None,
        )
        .await
        .expect("failed to open file in remote directory");

    // Test opening file directly though local directory by crossing remote automatically.
    dir.open_node::<fio::NodeMarker>(
        [remote_name, "/", TEST_FILE].join("").as_str(),
        fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE,
        None,
    )
    .await
    .expect("failed to open file when traversing a remote mount point");
}

/// Ensure specifying optional rights cannot cause rights escalation. The test sets up the following
/// hierarchy of nodes:
///
/// --------------------- RW   --------------------------
/// |  root_proxy       | ---> |  root                  |
/// --------------------- (a)  |   - /mount_point       | RWX  ---------------------
///                            |     (remote_proxy)     | ---> |  remote_dir       |
///                            -------------------------- (b)  ---------------------
///
/// It then verifies that opening `remote_dir` through `root_proxy` will remove any specified
/// optional rights not present during any intermediate opening steps.
#[fuchsia::test]
async fn open_remote_directory_right_escalation_test() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_remote_dir {
        return;
    }

    let mount_point = "mount_point";

    // Use the test harness to serve a directory with RWX permissions.
    let remote_proxy = harness
        .get_directory(vec![], fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE);

    // Mount the remote directory through root, and ensure that the connection only has RW
    // RW permissions (which is thus a sub-set of the permissions the remote_proxy has).
    let entries = vec![remote_directory(mount_point, remote_proxy)];
    let root_proxy = harness.get_directory(entries, fio::PERM_READABLE | fio::PERM_WRITABLE);

    // Open the remote with read rights as required, but write/execute as optional.
    let proxy = root_proxy
        .open_node::<fio::NodeMarker>(
            mount_point,
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::PERM_READABLE
                | fio::Flags::PERM_INHERIT_WRITE
                | fio::Flags::PERM_INHERIT_EXECUTE,
            None,
        )
        .await
        .expect("failed to open remote node");

    // Ensure the resulting connection expanded write but not execute rights.
    let flags = proxy.get_flags().await.unwrap().unwrap();
    assert!(!flags.contains(fio::Flags::PERM_EXECUTE));
    assert!(flags.contains(fio::PERM_READABLE | fio::PERM_WRITABLE));
}
