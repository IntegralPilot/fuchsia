// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for the [`crate::directory::immutable::Simple`] directory.

use crate::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use crate::directory::helper::DirectlyMutable;
use crate::directory::immutable::Simple;
use crate::directory::serve;
use crate::directory::test_utils::DirentsSameInodeBuilder;
use crate::execution_scope::ExecutionScope;
use crate::file::{self, FidlIoConnection, File, FileIo, FileLike, FileOptions};
use crate::node::Node;
use crate::path::Path;
use crate::{
    assert_close, assert_get_attr, assert_query, assert_read, assert_read_dirents, assert_seek,
    assert_write, pseudo_directory,
};
use assert_matches::assert_matches;
use flex_client::fidl::DiscoverableProtocolMarker as _;
use flex_fuchsia_io as fio;
#[cfg(not(feature = "fdomain"))]
use fuchsia_fs::directory::{
    WatchEvent, WatchMessage, Watcher, open_directory, open_directory_async, open_file,
    open_file_async,
};
#[cfg(feature = "fdomain")]
use fuchsia_fs_fdomain::directory::{
    WatchEvent, WatchMessage, Watcher, open_directory, open_directory_async, open_file,
    open_file_async,
};
use fuchsia_sync::Mutex;
use futures::StreamExt as _;
use libc::{S_IFDIR, S_IRUSR, S_IXUSR};
use static_assertions::assert_eq_size;
use std::path::PathBuf;
use std::sync::Arc;
use zx_status::Status;

async fn assert_open_file_err(
    root: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
    expected_status: Status,
) {
    let file = open_file_async(&root, path, flags).unwrap();
    assert_matches!(
        file.take_event_stream().next().await,
        Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. })) if epitaph == expected_status
    );
}

async fn assert_open_directory_err(
    root: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
    expected_status: Status,
) {
    let file = open_directory_async(&root, path, flags).unwrap();
    assert_matches!(
        file.take_event_stream().next().await,
        Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. })) if epitaph == expected_status
    );
}

#[fuchsia::test]
async fn empty_directory() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = Simple::new();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    assert_close!(root);
}

#[fuchsia::test]
async fn empty_directory_get_attr() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = Simple::new();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    assert_get_attr!(
        root,
        fio::NodeAttributes {
            mode: S_IFDIR | S_IRUSR | S_IXUSR,
            id: fio::INO_UNKNOWN,
            content_size: 0,
            storage_size: 0,
            link_count: 1,
            creation_time: 0,
            modification_time: 0,
        }
    );
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn empty_directory_with_custom_inode_get_attr() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = Simple::new_with_inode(12345);
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    assert_get_attr!(
        root,
        fio::NodeAttributes {
            mode: S_IFDIR | S_IRUSR | S_IXUSR,
            id: 12345,
            content_size: 0,
            storage_size: 0,
            link_count: 1,
            creation_time: 0,
            modification_time: 0,
        }
    );
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn empty_directory_describe() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = Simple::new();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    assert_query!(root, fio::DirectoryMarker::PROTOCOL_NAME);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn open_empty_directory_with_describe() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = Simple::new();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION);
    assert_matches!(
        root.take_event_stream().next().await,
        Some(Ok(fio::DirectoryEvent::OnRepresentation { .. }))
    );
}

#[fuchsia::test]
async fn clone() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file" => file::read_only(b"Content"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let file = open_file(&root, "file", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "Content");
    assert_close!(file);

    #[cfg(feature = "fdomain")]
    let (root_clone, server) = {
        let client = scope.domain();
        client.create_proxy::<fio::DirectoryMarker>()
    };
    #[cfg(not(feature = "fdomain"))]
    let (root_clone, server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    root.clone(server.into_channel().into()).unwrap();
    let file = open_file(&root_clone, "file", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "Content");
    assert_close!(file);

    let _ = &client;
    assert_close!(root);
    let _ = &client;
    assert_close!(root_clone);
}

#[fuchsia::test]
async fn one_file_open_existing() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file" => file::read_only(b"Content"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    let file = open_file(&root, "file", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "Content");
    assert_close!(file);

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn one_file_open_missing() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file" => file::read_only("Content"),
    };

    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    assert_open_file_err(&root, "file2", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn one_file_open_missing_not_found_handler() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let last_handler_value = Arc::new(Mutex::new(None));
    let last_handler_value_clone = last_handler_value.clone();
    let dir = Simple::new_with_not_found_handler(move |path| {
        *last_handler_value_clone.lock() = Some(path.to_string());
    });
    dir.add_entry("file", file::read_only("Content")).unwrap();

    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    assert_open_file_err(&root, "file2", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let _ = &client;
    assert_close!(root);
    assert_eq!(Some("file2".to_string()), *last_handler_value.lock());
}

#[fuchsia::test]
async fn small_tree_traversal() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "etc" => pseudo_directory! {
            "fstab" => file::read_only(b"/dev/fs /"),
            "ssh" => pseudo_directory! {
                "sshd_config" => file::read_only(b"# Empty"),
            },
        },
        "uname" => file::read_only(b"Fuchsia"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    async fn assert_contents(root: &fio::DirectoryProxy, path: &str, expected_contents: &str) {
        let file = open_file(&root, path, fio::PERM_READABLE).await.unwrap();
        assert_read!(file, expected_contents);
        assert_close!(file);
    }
    assert_contents(&root, "etc/fstab", "/dev/fs /").await;
    assert_contents(&root, "etc/ssh/sshd_config", "# Empty").await;
    assert_contents(&root, "uname", "Fuchsia").await;

    let ssh_dir = open_directory(&root, "etc/ssh", fio::PERM_READABLE).await.unwrap();
    assert_contents(&ssh_dir, "sshd_config", "# Empty").await;

    let _ = &client;
    assert_close!(ssh_dir);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn open_writable_in_subdir() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = {
        pseudo_directory! {
            "etc" => pseudo_directory! {
                "ssh" => pseudo_directory! {
                    "sshd_config" => Arc::new(MockWritableFile),
                }
            }
        }
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE | fio::PERM_WRITABLE);
    let ssh_dir =
        open_directory(&root, "etc/ssh", fio::PERM_READABLE | fio::PERM_WRITABLE).await.unwrap();
    let file =
        open_file(&ssh_dir, "sshd_config", fio::PERM_READABLE | fio::PERM_WRITABLE).await.unwrap();
    assert_read!(file, MOCK_FILE_CONTENTS);
    assert_seek!(file, 0, Start);
    assert_write!(file, "new content");
    assert_close!(file);
}

#[fuchsia::test]
async fn open_non_existing_path() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "dir" => pseudo_directory! {
            "file1" => file::read_only(b"Content 1"),
        },
        "file2" => file::read_only(b"Content 2"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "non-existing", fio::PERM_READABLE, Status::NOT_FOUND).await;
    assert_open_file_err(&root, "dir/file10", fio::PERM_READABLE, Status::NOT_FOUND).await;
    assert_open_file_err(&root, "dir/dir/file10", fio::PERM_READABLE, Status::NOT_FOUND).await;
    assert_open_file_err(&root, "dir/dir/file1", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn open_empty_path() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file_foo" => file::read_only(b"Content"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    assert_open_file_err(&root, "", fio::PERM_READABLE, Status::INVALID_ARGS).await;
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn open_path_within_a_file() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "dir" => pseudo_directory! {
            "file1" => file::read_only(b"Content 1"),
        },
        "file2" => file::read_only(b"Content 2"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "file2/file1", fio::PERM_READABLE, Status::NOT_DIR).await;
    assert_open_file_err(&root, "dir/file1/file3", fio::PERM_READABLE, Status::NOT_DIR).await;

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn open_file_as_directory() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "dir" => pseudo_directory! {
            "file1" => file::read_only(b"Content 1"),
        },
        "file2" => file::read_only(b"Content 2"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_directory_err(&root, "file2", fio::PERM_READABLE, Status::NOT_DIR).await;
    assert_open_directory_err(&root, "dir/file1", fio::PERM_READABLE, Status::NOT_DIR).await;

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn open_directory_as_file() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "dir" => pseudo_directory! {
            "dir2" => pseudo_directory! {},
        },
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "dir", fio::PERM_READABLE, Status::NOT_FILE).await;
    assert_open_file_err(&root, "dir/dir2", fio::PERM_READABLE, Status::NOT_FILE).await;

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
// TODO(https://fxbug.dev/405151790): open3 doesn't enforce the trailing slash meaning directory.
// Either enable this test when it does or delete/modify it if we decide on a different policy.
#[ignore]
async fn trailing_slash_means_directory() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file" => file::read_only(b"Content"),
        "dir" => pseudo_directory! {},
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "file/", fio::PERM_READABLE, Status::NOT_DIR).await;

    let file = open_file(&root, "file", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "Content");
    assert_close!(file);

    let sub_dir = open_directory(&root, "dir/", fio::PERM_READABLE).await.unwrap();
    let _ = &client;
    assert_close!(sub_dir);

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn no_dots_in_open() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file" => file::read_only(b"Content"),
        "dir" => pseudo_directory! {
            "dir2" => pseudo_directory! {},
        },
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_directory_err(&root, "dir/../dir2", fio::PERM_READABLE, Status::INVALID_ARGS).await;
    assert_open_directory_err(&root, "dir/./dir2", fio::PERM_READABLE, Status::INVALID_ARGS).await;
    assert_open_directory_err(&root, "./dir2", fio::PERM_READABLE, Status::INVALID_ARGS).await;

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn no_consecutive_slashes_in_open() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "dir" => pseudo_directory! {
            "dir2" => pseudo_directory! {},
        },
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_directory_err(&root, "dir//dir2", fio::PERM_READABLE, Status::INVALID_ARGS).await;
    assert_open_directory_err(&root, "dir/dir2//", fio::PERM_READABLE, Status::INVALID_ARGS).await;
    assert_open_directory_err(&root, "//dir/dir2", fio::PERM_READABLE, Status::INVALID_ARGS).await;

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn directories_restrict_nested_read_permissions() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "dir" => pseudo_directory! {
            "file" => file::read_only(b"Content"),
        },
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::Flags::empty());
    assert_open_file_err(&root, "dir/file", fio::PERM_READABLE, Status::ACCESS_DENIED).await;
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn directories_restrict_nested_write_permissions() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "dir" => pseudo_directory! {
            "file" => Arc::new(MockWritableFile),
        },
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::Flags::empty());
    assert_open_file_err(&root, "dir/file", fio::PERM_WRITABLE, Status::ACCESS_DENIED).await;
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn directories_remove_nested() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    // Test dynamic removal of a subdirectory under another directory.
    let root = pseudo_directory! {
        "dir" => pseudo_directory! {
            "subdir" => pseudo_directory! {},   // To be removed below.
        },
    };
    let dir_entry = root.get_entry("dir").expect("Failed to get directory entry!");
    // Remove subdir from dir.
    let downcasted_dir = dir_entry.into_any().downcast::<Simple>().expect("Downcast failed!");
    downcasted_dir.remove_entry("subdir", true).expect("Failed to remove directory entry!");

    // Ensure it was actually removed.
    assert_eq!(downcasted_dir.get_entry("subdir").err(), Some(Status::NOT_FOUND));
}

#[fuchsia::test]
async fn flag_inherit_write_means_writable() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = {
        pseudo_directory! {
        "nested" => pseudo_directory! {
            "file" => Arc::new(MockWritableFile),
            }
        }
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE | fio::PERM_WRITABLE);
    let sub_dir =
        open_directory(&root, "nested", fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE)
            .await
            .unwrap();
    let file = open_file(&sub_dir, "file", fio::PERM_READABLE | fio::PERM_WRITABLE).await.unwrap();

    assert_read!(file, MOCK_FILE_CONTENTS);
    assert_seek!(file, 0, Start);
    assert_write!(file, "new content");

    assert_close!(file);
    let _ = &client;
    assert_close!(sub_dir);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn flag_inherit_write_does_not_add_writable_to_read_only() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "nested" => pseudo_directory! {
            "file" => Arc::new(MockWritableFile),
        },
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let sub_dir =
        open_directory(&root, "nested", fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE)
            .await
            .unwrap();
    assert_open_file_err(
        &root,
        "file",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
        Status::ACCESS_DENIED,
    )
    .await;

    let file = open_file(&sub_dir, "file", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, MOCK_FILE_CONTENTS);

    assert_close!(file);
    let _ = &client;
    assert_close!(sub_dir);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn read_dirents_large_buffer() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "etc" => pseudo_directory! {
            "fstab" => file::read_only(b"/dev/fs /"),
            "passwd" => file::read_only(b"[redacted]"),
            "shells" => file::read_only(b"/bin/bash"),
            "ssh" => pseudo_directory! {
                "sshd_config" => file::read_only(b"# Empty"),
            },
        },
        "files" => file::read_only(b"Content"),
        "more" => file::read_only(b"Content"),
        "uname" => file::read_only(b"Fuchsia"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected
        .add(fio::DirentType::Directory, ".")
        .add(fio::DirentType::Directory, "etc")
        .add(fio::DirentType::File, "files")
        .add(fio::DirentType::File, "more")
        .add(fio::DirentType::File, "uname");
    assert_read_dirents!(root, 1000, expected.into_vec());

    let etc_dir = open_directory(&root, "etc", fio::PERM_READABLE).await.unwrap();
    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected
        .add(fio::DirentType::Directory, ".")
        .add(fio::DirentType::File, "fstab")
        .add(fio::DirentType::File, "passwd")
        .add(fio::DirentType::File, "shells")
        .add(fio::DirentType::Directory, "ssh");
    assert_read_dirents!(etc_dir, 1000, expected.into_vec());
    let _ = &client;
    assert_close!(etc_dir);

    let ssh_dir = open_directory(&root, "etc/ssh", fio::PERM_READABLE).await.unwrap();
    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected.add(fio::DirentType::Directory, ".").add(fio::DirentType::File, "sshd_config");
    assert_read_dirents!(ssh_dir, 1000, expected.into_vec());
    let _ = &client;
    assert_close!(ssh_dir);

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn read_dirents_small_buffer() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "etc" => pseudo_directory! { },
        "files" => file::read_only(b"Content"),
        "more" => file::read_only(b"Content"),
        "uname" => file::read_only(b"Fuchsia"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    // Entry header is 10 bytes + length of the name in bytes.
    // (10 + 1) = 11
    expected.add(fio::DirentType::Directory, ".");
    assert_read_dirents!(root, 11, expected.into_vec());

    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected
        // (10 + 3) = 13
        .add(fio::DirentType::Directory, "etc")
        // 13 + (10 + 5) = 28
        .add(fio::DirentType::File, "files");
    assert_read_dirents!(root, 28, expected.into_vec());

    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected.add(fio::DirentType::File, "more").add(fio::DirentType::File, "uname");
    assert_read_dirents!(root, 100, expected.into_vec());

    assert_read_dirents!(root, 100, vec![]);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn read_dirents_very_small_buffer() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file" => file::read_only(b"Content"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let (status, entries) = root.read_dirents(8).await.expect("read_dirents fidl error");
    assert_eq!(Status::ok(status), Err(Status::BUFFER_TOO_SMALL));
    assert_eq!(entries.len(), 0);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn read_dirents_rewind() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "etc" => pseudo_directory! { },
        "files" => file::read_only(b"Content"),
        "more" => file::read_only(b"Content"),
        "uname" => file::read_only(b"Fuchsia"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    // Entry header is 10 bytes + length of the name in bytes.
    expected
        // (10 + 1) = 11
        .add(fio::DirentType::Directory, ".")
        // 11 + (10 + 3) = 24
        .add(fio::DirentType::Directory, "etc")
        // 24 + (10 + 5) = 39
        .add(fio::DirentType::File, "files");
    assert_read_dirents!(root, 39, expected.into_vec());

    let status = root.rewind().await.expect("rewind fidl error");
    assert_eq!(status, zx_status::sys::ZX_OK);

    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    // Entry header is 10 bytes + length of the name in bytes.
    expected
        // (10 + 1) = 11
        .add(fio::DirentType::Directory, ".")
        // 11 + (10 + 3) = 24
        .add(fio::DirentType::Directory, "etc");
    assert_read_dirents!(root, 24, expected.into_vec());

    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected
        .add(fio::DirentType::File, "files")
        .add(fio::DirentType::File, "more")
        .add(fio::DirentType::File, "uname");
    assert_read_dirents!(root, 200, expected.into_vec());

    assert_read_dirents!(root, 100, vec![]);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn add_entry_too_long_error() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    assert_eq_size!(u64, usize);

    // It is annoying to have to write `as u64` or `as usize` everywhere.  Converting
    // `MAX_FILENAME` to `usize` aligns the types.
    let max_filename = fio::MAX_NAME_LENGTH as usize;

    let dir = Simple::new();
    let name = {
        let mut name = "This entry name will be longer than the MAX_FILENAME bytes".to_string();

        // Make `name` at least `MAX_FILENAME + 1` bytes long.
        name.reserve(max_filename + 1);
        let filler = " - filler";
        name.push_str(&filler.repeat((max_filename + filler.len()) / filler.len()));

        // And we want exactly `MAX_FILENAME + 1` bytes.  As all the characters are ASCII, we
        // should be able to just cut at any byte.
        name.truncate(max_filename + 1);
        assert!(name.len() == max_filename + 1);

        name
    };
    let name_len = name.len();

    match dir.clone().add_entry(name, file::read_only(b"Should never be used")) {
        Ok(()) => panic!(
            "`add_entry()` succeeded for a name of {} bytes, when MAX_FILENAME is {}",
            name_len, max_filename
        ),
        Err(Status::BAD_PATH) => (),
        Err(status) => panic!(
            "`add_entry()` failed for a name of {} bytes, with status {}.  Expected status is \
             BAD_PATH.  MAX_FILENAME is {}.",
            name_len, status, max_filename
        ),
    }

    // Make sure that after we have seen an error, the entry is not actually inserted.

    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected.add(fio::DirentType::Directory, ".");
    assert_read_dirents!(root, 1000, expected.into_vec());
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn simple_add_file() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = Simple::new();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir.clone(), scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "file", fio::PERM_READABLE, Status::NOT_FOUND).await;

    let file = file::read_only(b"Content");
    dir.add_entry("file", file).unwrap();

    let proxy = open_file(&root, "file", fio::PERM_READABLE).await.unwrap();
    assert_read!(proxy, "Content");
    let _ = &client;
    assert_close!(proxy);
}

#[fuchsia::test]
async fn add_file_to_empty() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let etc = Simple::new();
    let dir = pseudo_directory! {
        "etc" => etc.clone(),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "etc/fstab", fio::PERM_READABLE, Status::NOT_FOUND).await;

    let fstab = file::read_only(b"/dev/fs /");
    etc.add_entry("fstab", fstab).unwrap();

    let proxy = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(proxy, "/dev/fs /");
    let _ = &client;
    assert_close!(proxy);
}

#[fuchsia::test]
async fn in_tree_open() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let ssh = pseudo_directory! {
        "sshd_config" => file::read_only(b"# Empty"),
    };
    let _root = pseudo_directory! {
        "etc" => pseudo_directory! {
            "ssh" => ssh.clone(),
        },
    };

    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let ssh_dir = serve(ssh, scope.clone(), fio::PERM_READABLE);
    let file = open_file(&ssh_dir, "sshd_config", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "# Empty");
    assert_close!(file);
    let _ = &client;
    assert_close!(ssh_dir);
}

#[fuchsia::test]
async fn in_tree_open_path_one_component() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let etc = pseudo_directory! {
        "ssh" => pseudo_directory! {
            "sshd_config" => file::read_only(b"# Empty"),
        },
    };
    let _root = pseudo_directory! {
        "etc" => etc.clone(),
    };

    let path = Path::validate_and_split("ssh").unwrap();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let ssh_dir = crate::serve_directory(etc, path, scope.clone(), fio::PERM_READABLE);
    let file = open_file(&ssh_dir, "sshd_config", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "# Empty");
    assert_close!(file);
    let _ = &client;
    assert_close!(ssh_dir);
}

#[fuchsia::test]
async fn in_tree_open_path_two_components() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let etc = pseudo_directory! {
        "ssh" => pseudo_directory! {
            "sshd_config" => file::read_only(b"# Empty"),
        },
    };
    let _root = pseudo_directory! {
        "etc" => etc.clone(),
    };

    let path = Path::validate_and_split("ssh/sshd_config").unwrap();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let file = crate::serve_file(etc, path, scope.clone(), fio::PERM_READABLE);
    assert_read!(file, "# Empty");
    assert_close!(file);
}

#[fuchsia::test]
async fn in_tree_add_file() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let etc = pseudo_directory! {
        "ssh" => pseudo_directory! {
            "sshd_config" => file::read_only(b"# Empty"),
        },
        "passwd" => file::read_only(b"[redacted]"),
    };
    let dir = pseudo_directory! {
        "etc" => etc.clone()
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "etc/fstab", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let file = open_file(&root, "etc/passwd", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "[redacted]");
    assert_close!(file);

    let fstab = file::read_only(b"/dev/fs /");
    etc.add_entry("fstab", fstab).unwrap();

    let file = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);
    let file = open_file(&root, "etc/passwd", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "[redacted]");
    assert_close!(file);

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn in_tree_remove_file() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let etc = pseudo_directory! {
        "fstab" => file::read_only(b"/dev/fs /"),
        "passwd" => file::read_only(b"[redacted]"),
    };
    let dir = pseudo_directory! {
        "etc" => etc.clone(),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    let file = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);
    let file = open_file(&root, "etc/passwd", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "[redacted]");
    assert_close!(file);

    let o_passwd = etc.remove_entry("passwd", false).unwrap();
    match o_passwd {
        None => panic!("remove_entry() did not find 'passwd'"),
        Some(passwd) => {
            let entry_info = passwd.entry_info();
            assert_eq!(entry_info, EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File));
        }
    }

    assert_open_file_err(&root, "etc/passwd", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let file = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn in_tree_move_file() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let etc = pseudo_directory! {
        "fstab" => file::read_only(b"/dev/fs /"),
    };
    let dir = pseudo_directory! {
        "etc" => etc.clone()
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "etc/passwd", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let file = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);

    let fstab = etc
        .clone()
        .remove_entry("fstab", false)
        .unwrap()
        .expect("remove_entry() did not find 'fstab'");

    etc.add_entry("passwd", fstab).unwrap();

    assert_open_file_err(&root, "etc/fstab", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let file = open_file(&root, "etc/passwd", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn watch_empty() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = Simple::new();
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let mut watcher = Watcher::new(&root).await.unwrap();

    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from(".")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::IDLE, filename })) if filename == PathBuf::new()
    );

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn watch_non_empty() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "etc" => pseudo_directory! {
            "fstab" => file::read_only(b"/dev/fs /"),
            "ssh" => pseudo_directory! {
                "sshd_config" => file::read_only(b"# Empty"),
            },
        },
        "files" => file::read_only(b"Content"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let mut watcher = Watcher::new(&root).await.unwrap();

    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from(".")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("etc")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("files")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::IDLE, filename })) if filename == PathBuf::new()
    );

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn watch_two_watchers() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "etc" => pseudo_directory! {
            "fstab" => file::read_only(b"/dev/fs /"),
            "ssh" => pseudo_directory! {
                "sshd_config" => file::read_only(b"# Empty"),
            },
        },
        "files" => file::read_only(b"Content"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let mut watcher = Watcher::new(&root).await.unwrap();

    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from(".")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("etc")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("files")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::IDLE, filename })) if filename == PathBuf::new()
    );

    let mut watcher2 = Watcher::new(&root).await.unwrap();

    assert_matches!(
        watcher2.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from(".")
    );
    assert_matches!(
        watcher2.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("etc")
    );
    assert_matches!(
        watcher2.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("files")
    );
    assert_matches!(
        watcher2.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::IDLE, filename })) if filename == PathBuf::new()
    );

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn watch_addition() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let etc = pseudo_directory! {
        "ssh" => pseudo_directory! {
            "sshd_config" => file::read_only(b"# Empty"),
        },
        "passwd" => file::read_only(b"[redacted]"),
    };
    let dir = pseudo_directory! {
        "etc" => etc.clone(),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    assert_open_file_err(&root, "etc/fstab", fio::PERM_READABLE, Status::NOT_FOUND).await;
    let file = open_file(&root, "etc/passwd", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "[redacted]");
    assert_close!(file);

    let etc_proxy = open_directory(&root, "etc", fio::PERM_READABLE).await.unwrap();
    let mut watcher = Watcher::new(&etc_proxy).await.unwrap();

    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from(".")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("passwd")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("ssh")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::IDLE, filename })) if filename == PathBuf::new()
    );

    let fstab = file::read_only(b"/dev/fs /");
    etc.add_entry("fstab", fstab).unwrap();

    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::ADD_FILE, filename })) if filename == PathBuf::from("fstab")
    );

    let file = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);
    let file = open_file(&root, "etc/passwd", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "[redacted]");
    assert_close!(file);

    let _ = &client;
    assert_close!(etc_proxy);
    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn watch_removal() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    let etc = pseudo_directory! {
        "fstab" => file::read_only(b"/dev/fs /"),
        "passwd" => file::read_only(b"[redacted]"),
    };
    let dir = pseudo_directory! {
        "etc" => etc.clone(),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    let file = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);
    let file = open_file(&root, "etc/passwd", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "[redacted]");
    assert_close!(file);

    let etc_proxy = open_directory(&root, "etc", fio::PERM_READABLE).await.unwrap();
    let mut watcher = Watcher::new(&etc_proxy).await.unwrap();

    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from(".")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("fstab")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::EXISTING, filename })) if filename == PathBuf::from("passwd")
    );
    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::IDLE, filename })) if filename == PathBuf::new()
    );

    let o_passwd = etc.remove_entry("passwd", false).unwrap();
    match o_passwd {
        None => panic!("remove_entry() did not find 'passwd'"),
        Some(passwd) => {
            let entry_info = passwd.entry_info();
            assert_eq!(entry_info, EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File));
        }
    }

    assert_matches!(
        watcher.next().await,
        Some(Ok(WatchMessage { event: WatchEvent::REMOVE_FILE, filename })) if filename == PathBuf::from("passwd")
    );

    let file = open_file(&root, "etc/fstab", fio::PERM_READABLE).await.unwrap();
    assert_read!(file, "/dev/fs /");
    assert_close!(file);
    assert_open_file_err(&root, "etc/passwd", fio::PERM_READABLE, Status::NOT_FOUND).await;

    assert_close!(etc_proxy);
    assert_close!(root);
}

#[fuchsia::test]
async fn watch_with_mask() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "etc" => pseudo_directory! {
            "fstab" => file::read_only(b"/dev/fs /"),
            "ssh" => pseudo_directory! {
                "sshd_config" => file::read_only(b"# Empty"),
            },
        },
        "files" => file::read_only(b"Content"),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    let mask = fio::WatchMask::IDLE | fio::WatchMask::ADDED | fio::WatchMask::REMOVED;
    let mut watcher = Watcher::new_with_mask(&root, mask).await.unwrap();
    assert_eq!(
        watcher.next().await.unwrap().unwrap(),
        WatchMessage { event: WatchEvent::IDLE, filename: PathBuf::new() }
    );

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn watch_remove_all_entries() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {
        "file1" => file::read_only(""),
        "file2" => file::read_only(""),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir.clone(), scope.clone(), fio::PERM_READABLE);
    let mut watcher = Watcher::new_with_mask(&root, fio::WatchMask::REMOVED).await.unwrap();

    dir.remove_all_entries();

    assert_eq!(
        watcher.next().await.unwrap().unwrap(),
        WatchMessage { event: WatchEvent::REMOVE_FILE, filename: PathBuf::from("file1") }
    );
    assert_eq!(
        watcher.next().await.unwrap().unwrap(),
        WatchMessage { event: WatchEvent::REMOVE_FILE, filename: PathBuf::from("file2") }
    );

    let _ = &client;
    assert_close!(root);
}

#[fuchsia::test]
async fn open_directory_containing_itself() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;
    let dir = pseudo_directory! {};
    dir.add_entry("dir", dir.clone()).unwrap();

    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir.clone(), scope.clone(), fio::PERM_READABLE);
    let sub_dir = open_directory(&root, "dir/dir/dir/dir", fio::PERM_READABLE).await.unwrap();

    let _ = &client;
    assert_close!(sub_dir);
    let _ = &client;
    assert_close!(root);

    dir.remove_entry("dir", true).unwrap();
}

struct MockWritableFile;
const MOCK_FILE_CONTENTS: &str = "mock-file-contents";

impl GetEntryInfo for MockWritableFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)
    }
}

impl DirectoryEntry for MockWritableFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl Node for MockWritableFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::FILE,
                abilities: fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::UPDATE_ATTRIBUTES
                    | fio::Operations::READ_BYTES
                    | fio::Operations::WRITE_BYTES,
                content_size: 0,
                storage_size: 0,
                link_count: 1,
                id: fio::INO_UNKNOWN,
            }
        ))
    }
}

impl FileLike for MockWritableFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: crate::ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        FidlIoConnection::create_sync(scope, self, options, object_request.take());
        Ok(())
    }
}

impl File for MockWritableFile {
    fn writable(&self) -> bool {
        true
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, _: u64) -> Result<(), Status> {
        unimplemented!()
    }

    async fn get_size(&self) -> Result<u64, Status> {
        unimplemented!()
    }

    async fn update_attributes(&self, _: fio::MutableNodeAttributes) -> Result<(), Status> {
        unimplemented!()
    }

    async fn sync(&self, _: file::SyncMode) -> Result<(), Status> {
        Ok(())
    }
}

impl FileIo for MockWritableFile {
    async fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<u64, Status> {
        assert_eq!(offset, 0);
        assert!(bytes.len() >= MOCK_FILE_CONTENTS.len());
        bytes[..MOCK_FILE_CONTENTS.len()].copy_from_slice(MOCK_FILE_CONTENTS.as_bytes());
        Ok(MOCK_FILE_CONTENTS.len() as u64)
    }

    async fn write_at(&self, _: u64, bytes: &[u8]) -> Result<u64, Status> {
        Ok(bytes.len() as u64)
    }

    async fn append(&self, _: &[u8]) -> Result<(u64, u64), Status> {
        unimplemented!()
    }
}

#[fuchsia::test]
async fn test_open_without_traverse() {
    #[cfg(feature = "fdomain")]
    let client = fdomain_local::local_client_empty();
    #[cfg(not(feature = "fdomain"))]
    let client = flex_client::fidl::ZirconClient;
    let _ = &client;

    let dir = pseudo_directory! {
        "file" => file::read_only(b"Content"),
    };

    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();

    // Serve with no rights (PROTOCOL_DIRECTORY only).
    // This creates a connection with empty rights (no TRAVERSE).
    let root = serve(dir, scope.clone(), fio::Flags::PROTOCOL_DIRECTORY);

    assert_open_file_err(&root, "file", fio::Flags::PROTOCOL_NODE, Status::ACCESS_DENIED).await;
}
