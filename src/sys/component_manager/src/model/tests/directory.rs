// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use assert_matches::assert_matches;
use cm_rust::*;
use cm_rust_testing::*;
use fidl_fuchsia_io as fio;
use futures::channel::mpsc;
use std::sync::LazyLock;
use vfs::ObjectRequestRef;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::execution_scope::ExecutionScope;
use vfs::remote::RemoteLike;

use crate::model::actions::{ActionsManager, DestroyAction};
use crate::model::component::StartReason;
use crate::model::start::Start;
use crate::model::testing::out_dir::OutDir;
use crate::model::testing::routing_test_helpers::RoutingTest;

/// ```
///    a
///   / \
///  b   c
/// ```
///
/// `b` uses `data` from parent at `/data`.
/// `c` exposes `data` from self at `/data`.
/// `a` offers it from `c` to `b`.
async fn build_realm() -> RoutingTest {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .offer(
                    OfferBuilder::directory()
                        .name("data")
                        .source_static_child("c")
                        .target_static_child("b")
                        .rights(fio::R_STAR_DIR),
                )
                .child_default("b")
                .child_default("c")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::directory().name("data").path("/data"))
                .build(),
        ),
        (
            "c",
            ComponentDeclBuilder::new()
                .capability(CapabilityBuilder::directory().name("data").path("/data"))
                .expose(ExposeBuilder::directory().name("data").source(ExposeSource::Self_))
                .build(),
        ),
    ];
    RoutingTest::new("a", components).await
}

/// If component `b` uses a directory from `c`, starting `b` will cause the
/// framework to provision its namespace with a directory connection, but
/// `c` should not be started due to this.
#[fuchsia::test]
async fn starting_directory_using_target_component_does_not_start_source() {
    let test = build_realm().await;
    let b = test.model.root().find_and_maybe_resolve(&"b".parse().unwrap()).await.unwrap();
    let c = test.model.root().find_and_maybe_resolve(&"c".parse().unwrap()).await.unwrap();

    assert!(!c.is_started().await);

    // Start `b` and get a hold of the directory connection.
    b.ensure_started(&StartReason::Debug).await.unwrap();
    test.mock_runner.wait_for_url("test:///b").await;
    let namespace = test.mock_runner.get_namespace("test:///b").unwrap();

    {
        let namespace = namespace.lock().await;
        let client_end = namespace.get(&"/data".parse().unwrap()).unwrap();
        assert_matches!(
            client_end
                .channel()
                .wait_one(zx::Signals::CHANNEL_PEER_CLOSED, zx::MonotonicInstant::INFINITE_PAST)
                .to_result(),
            Err(zx::Status::TIMED_OUT)
        );
    }

    // `c` should remain not started.
    assert!(!c.is_started().await);

    // Make some round-trip calls on the directory.
    {
        let mut namespace = namespace.lock().await;
        let client_end = namespace.remove(&"/data".parse().unwrap()).unwrap();
        let dir = client_end.into_proxy();
        fuchsia_fs::directory::readdir(&dir).await.unwrap();
    }

    // `c` should be started now.
    assert!(c.is_started().await);
}

/// If component `b` uses a directory from `c`, and `b` makes multiple `Open`
/// calls on the directory capability, `c` should see them coming from one
/// connection because the framework would like to hand off the directory
/// connection to `c` and step away from intercepting further `Open` calls.
#[fuchsia::test]
async fn open_requests_go_to_the_same_directory_connection() {
    let test = build_realm().await;

    static OPEN_FLAGS: LazyLock<fio::Flags> =
        LazyLock::new(|| fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY);

    // A directory that notifies via the sender whenever it is opened.
    struct MockDir(mpsc::Sender<()>);
    impl DirectoryEntry for MockDir {
        fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
            request.open_remote(self)
        }
    }
    impl GetEntryInfo for MockDir {
        fn entry_info(&self) -> EntryInfo {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
        }
    }
    impl RemoteLike for MockDir {
        fn open(
            self: Arc<Self>,
            _scope: ExecutionScope,
            relative_path: vfs::path::Path,
            flags: fio::Flags,
            _object_request: ObjectRequestRef<'_>,
        ) -> Result<(), zx::Status> {
            assert_eq!(relative_path.into_string(), "");
            assert_eq!(flags, *OPEN_FLAGS);
            self.0.clone().try_send(()).unwrap();
            Ok(())
        }
    }

    let (open_tx, mut open_rx) = mpsc::channel::<()>(1);
    let mock_data = Arc::new(MockDir(open_tx));
    let mut out_dir = OutDir::new();
    out_dir.add_entry("/data".parse().unwrap(), mock_data.clone());
    test.mock_runner.add_host_fn("test:///c", out_dir.host_fn());

    let b = test.model.root().find_and_maybe_resolve(&"b".parse().unwrap()).await.unwrap();
    let c = test.model.root().find_and_maybe_resolve(&"c".parse().unwrap()).await.unwrap();

    b.ensure_started(&StartReason::Debug).await.unwrap();
    test.mock_runner.wait_for_url("test:///b").await;
    {
        let namespace = test.mock_runner.get_namespace("test:///b").unwrap();
        let mut namespace = namespace.lock().await;
        let client_end = namespace.remove(&"/data".parse().unwrap()).unwrap();
        let dir = client_end.into_proxy();

        // Make a few open calls.
        for _ in 0..10 {
            let (_, server_end) = fidl::endpoints::create_endpoints::<fio::NodeMarker>();
            dir.open(".", *OPEN_FLAGS, &fio::Options::default(), server_end.into_channel())
                .unwrap();
        }
    }
    // Drain routing and open requests.
    b.stop().await.unwrap();
    ActionsManager::register(b.clone(), DestroyAction::new()).await.unwrap();

    // `c` should only get one open call after we drain any requests.
    test.mock_runner.wait_for_url("test:///c").await;
    c.stop().await.unwrap();
    open_rx.try_next().unwrap();
    open_rx.try_next().unwrap_err();
}
