// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for the remote node.

use super::{RemoteLike, remote_dir};

use crate::{assert_close, assert_read, assert_read_dirents, pseudo_directory};

use crate::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use crate::directory::entry_container::Directory;
use crate::directory::test_utils::DirentsSameInodeBuilder;
use crate::execution_scope::ExecutionScope;
use crate::object_request::ObjectRequest;
use crate::path::Path;
use crate::{ObjectRequestRef, file};

use flex_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use std::sync::Arc;
use zx_status::Status;

fn test_scope() -> ExecutionScope {
    #[cfg(feature = "fdomain")]
    let client = flex_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    return ExecutionScope::new(client);
    #[cfg(not(feature = "fdomain"))]
    return ExecutionScope::new();
}

fn set_up_remote(scope: ExecutionScope) -> fio::DirectoryProxy {
    let r = pseudo_directory! {
        "a" => file::read_only("a content"),
        "dir" => pseudo_directory! {
            "b" => file::read_only("b content"),
        }
    };

    let (remote_proxy, remote_server_end) = scope.domain().create_proxy::<fio::DirectoryMarker>();
    let flags = fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE | fio::PERM_WRITABLE;
    ObjectRequest::new(flags, &fio::Options::default(), remote_server_end.into_channel().into())
        .handle(|request| r.open(scope, Path::dot(), flags, request));

    remote_proxy
}

#[fuchsia::test]
async fn test_set_up_remote() {
    let scope = test_scope();
    let remote_proxy = set_up_remote(scope.clone());
    assert_close!(remote_proxy);
}

// Tests for opening a remote node with the NODE_REFERENCE flag. The remote node uses the existing
// Service connection type after construction, which is tested in service/tests/node_reference.rs.
#[fuchsia::test]
async fn remote_dir_construction_open_node_ref() {
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();

    let remote_proxy = set_up_remote(scope.clone());
    let server = remote_dir(remote_proxy);

    #[cfg(feature = "fdomain")]
    let (proxy, server_end) = {
        let client = scope.domain();
        client.create_proxy::<fio::NodeMarker>()
    };
    #[cfg(not(feature = "fdomain"))]
    let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    let flags = fio::Flags::PROTOCOL_NODE;
    ObjectRequest::new(flags, &fio::Options::default(), server_end.into_channel().into())
        .handle(|request| server.open(scope.clone(), Path::dot(), flags, request));
    assert_close!(proxy);
}

#[fuchsia::test]
async fn remote_dir_node_ref_with_path() {
    let scope = test_scope();

    let remote_proxy = set_up_remote(scope.clone());
    let server = remote_dir(remote_proxy);
    let path = Path::validate_and_split("dir/b").unwrap();

    #[cfg(feature = "fdomain")]
    let (proxy, server_end) = {
        let client = scope.domain();
        client.create_proxy::<fio::NodeMarker>()
    };
    #[cfg(not(feature = "fdomain"))]
    let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    let flags = fio::Flags::PROTOCOL_NODE;
    ObjectRequest::new(flags, &fio::Options::default(), server_end.into_channel().into())
        .handle(|request| server.open(scope.clone(), path, flags, request));
    assert_close!(proxy);
}

// Tests for opening a remote node where we actually want the open request to be forwarded.
#[fuchsia::test]
async fn remote_dir_direct_connection() {
    let scope = test_scope();

    let remote_proxy = set_up_remote(scope.clone());
    let server = remote_dir(remote_proxy);

    #[cfg(feature = "fdomain")]
    let (proxy, server_end) = {
        let client = scope.domain();
        client.create_proxy::<fio::DirectoryMarker>()
    };
    #[cfg(not(feature = "fdomain"))]
    let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    let flags = fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE;
    ObjectRequest::new(flags, &fio::Options::default(), server_end.into_channel().into())
        .handle(|request| server.open(scope.clone(), Path::dot(), flags, request));
    let mut expected = DirentsSameInodeBuilder::new(fio::INO_UNKNOWN);
    expected
        // (10 + 1) = 11
        .add(fio::DirentType::Directory, ".")
        // 11 + (10 + 1) = 22
        .add(fio::DirentType::File, "a");
    assert_read_dirents!(proxy, 22, expected.into_vec());
    assert_close!(proxy);
}

#[fuchsia::test]
async fn remote_dir_direct_connection_dir_contents() {
    let scope = test_scope();

    let remote_proxy = set_up_remote(scope.clone());
    let server = remote_dir(remote_proxy);
    let path = Path::validate_and_split("a").unwrap();

    #[cfg(feature = "fdomain")]
    let (proxy, server_end) = {
        let client = scope.domain();
        client.create_proxy::<fio::FileMarker>()
    };
    #[cfg(not(feature = "fdomain"))]
    let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::FileMarker>();
    let flags = fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE;
    ObjectRequest::new(flags, &fio::Options::default(), server_end.into_channel().into())
        .handle(|request| server.open(scope.clone(), path, flags, request));
    assert_read!(proxy, "a content");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn lazy_remote() {
    struct Remote(Mutex<Option<oneshot::Sender<()>>>);
    impl DirectoryEntry for Remote {
        fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
            request.open_remote(self)
        }
    }
    impl GetEntryInfo for Remote {
        fn entry_info(&self) -> EntryInfo {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Unknown)
        }
    }
    impl RemoteLike for Remote {
        fn open(
            self: Arc<Self>,
            _scope: ExecutionScope,
            _path: Path,
            _flags: fio::Flags,
            _object_request: ObjectRequestRef<'_>,
        ) -> Result<(), Status> {
            self.0.lock().take().unwrap().send(()).unwrap();
            Ok(())
        }

        fn lazy(&self, _path: &Path) -> bool {
            true
        }
    }
    let (sender, mut receiver) = oneshot::channel();

    let root = pseudo_directory! {
        "remote" => Arc::new(Remote(Mutex::new(Some(sender)))),
    };

    let scope = test_scope();

    #[cfg(feature = "fdomain")]
    let (client, server_end) = {
        let client = scope.domain();
        client.create_proxy::<fio::NodeMarker>()
    };
    #[cfg(not(feature = "fdomain"))]
    let (client, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    let path = Path::validate_and_split("remote").unwrap();
    let flags = fio::Flags::empty();
    ObjectRequest::new(flags, &fio::Options::default(), server_end.into_channel().into())
        .handle(|request| root.open(scope, path, flags, request));

    // The open shouldn't get forwarded until we write something to client.
    assert_eq!(receiver.try_recv().unwrap(), None);

    // Sending get_attributes should cause the open to trigger.
    let _ = client.get_attributes(fio::NodeAttributesQuery::default());

    receiver.await.expect("Open not sent");
}
