// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fidl::endpoints::{ClientEnd, ServerEnd, create_endpoints, create_proxy};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component_test::{
    Capability, ChildOptions, DEFAULT_COLLECTION_NAME, LocalComponentHandles, RealmBuilder, Ref,
    Route,
};
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::sink::SinkExt;
use futures::{Future, FutureExt, StreamExt, TryStreamExt};
use maplit::hashset;
use std::collections::HashSet;

const CM_URL: &'static str = "#meta/component_manager.cm";

/// Returns a new mock that writes a file and terminates. The future indicates when writing the file has completed.
fn new_data_user_mock<T: Into<String>, U: Into<String>>(
    filename: T,
    contents: U,
) -> (
    impl Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), anyhow::Error>>
    + Sync
    + Send
    + 'static,
    impl Future<Output = ()>,
) {
    let (send, recv) = mpsc::channel(1);
    let filename = filename.into();
    let contents = contents.into();

    let mock = move |mock_handles: LocalComponentHandles| {
        let mut send_clone = send.clone();
        let filename_clone = filename.clone();
        let contents_clone = contents.clone();
        async move {
            let data_handle =
                mock_handles.clone_from_namespace("data").expect("data directory not available");

            let file = fuchsia_fs::directory::open_file_async(
                &data_handle,
                &filename_clone,
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
            )
            .expect("failed to open file");
            fuchsia_fs::file::write(&file, &contents_clone).await.expect("write file failed");
            send_clone.send(()).await.unwrap();
            Ok(())
        }
        .boxed()
    };
    (mock, recv.into_future().map(|_| ()))
}

/// Collect the set of component instances that use the `data` storage capability.
async fn collect_storage_user_monikers<T: AsRef<str>>(
    admin: &fcomponent::StorageAdminProxy,
    realm_moniker: T,
) -> HashSet<String> {
    let (it_proxy, it_server) = create_proxy::<fcomponent::StorageIteratorMarker>();
    admin
        .list_storage_in_realm(realm_moniker.as_ref(), it_server)
        .await
        .expect("fidl error")
        .expect("list storage error");

    let mut storage_monikers = hashset! {};
    loop {
        let next = it_proxy.next().await.expect("Error calling next on storage iterator");
        match next.is_empty() {
            true => {
                break;
            }
            false => storage_monikers.extend(next.into_iter()),
        }
    }
    storage_monikers
}

#[fuchsia::test]
async fn single_storage_user() {
    let (mock, done_signal) = new_data_user_mock("file", "data");
    let builder = RealmBuilder::new().await.unwrap();
    let storage_user =
        builder.add_local_child("storage-user", mock, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::storage("data").path("/data"))
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&storage_user),
        )
        .await
        .unwrap();
    let instance = builder.build().await.unwrap();
    let _ = instance.root.connect_to_binder().unwrap();
    let instance_moniker = format!("{}:{}", DEFAULT_COLLECTION_NAME, instance.root.child_name());
    let storage_user_moniker = format!("{}/storage-user", &instance_moniker);

    let storage_admin = connect_to_protocol::<fcomponent::StorageAdminMarker>().unwrap();
    let storage_users = collect_storage_user_monikers(&storage_admin, instance_moniker).await;
    assert_eq!(
        storage_users,
        hashset! {
            storage_user_moniker.clone()
        }
    );

    done_signal.await;

    let (node_client_end, node_server) = create_endpoints::<fio::NodeMarker>();
    let directory = ClientEnd::<fio::DirectoryMarker>::new(node_client_end.into_channel());
    let dir_proxy = directory.into_proxy();
    let storage_user_moniker_with_instances = storage_users.into_iter().next().unwrap();
    storage_admin
        .open_storage(&storage_user_moniker_with_instances, node_server)
        .await
        .expect("transport error")
        .expect("open component storage");
    let filenames: HashSet<_> = fuchsia_fs::directory::readdir_recursive(&dir_proxy, None)
        .map_ok(|dir_entry| dir_entry.name)
        .try_collect()
        .await
        .expect("Error reading directory");
    assert_eq!(filenames, hashset! {"file".to_string()});
    let file =
        fuchsia_fs::directory::open_file_async(&dir_proxy, "file", fio::PERM_READABLE).unwrap();
    assert_eq!(fuchsia_fs::file::read_to_string(&file).await.unwrap(), "data".to_string());
}

#[fuchsia::test]
async fn multiple_storage_users() {
    const NUM_MOCKS: usize = 5;
    let builder = RealmBuilder::new().await.unwrap();
    let (mocks, done_signals): (Vec<_>, Vec<_>) =
        (0..NUM_MOCKS).map(|_| new_data_user_mock("file", "data")).unzip();
    for (mock_idx, mock) in mocks.into_iter().enumerate() {
        let mock_name = format!("storage-user-{:?}", mock_idx);
        let mock_ref = builder
            .add_local_child(mock_name.as_str(), mock, ChildOptions::new().eager())
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::storage("data").path("/data"))
                    .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                    .from(Ref::parent())
                    .to(&mock_ref),
            )
            .await
            .unwrap();
    }

    let instance = builder.build().await.unwrap();
    let _ = instance.root.connect_to_binder().unwrap();
    futures::future::join_all(done_signals).await;

    let instance_moniker = format!("{}:{}", DEFAULT_COLLECTION_NAME, instance.root.child_name());
    let expected_storage_users: HashSet<_> = (0..NUM_MOCKS)
        .map(|mock_idx| format!("{}/storage-user-{:?}", &instance_moniker, mock_idx))
        .collect();

    let storage_admin = connect_to_protocol::<fcomponent::StorageAdminMarker>().unwrap();
    let storage_users = collect_storage_user_monikers(&storage_admin, instance_moniker).await;
    assert_eq!(storage_users, expected_storage_users);
}

#[fuchsia::test]
async fn destroyed_storage_user() {
    let (mock, done_signal) = new_data_user_mock("file", "data");
    let builder = RealmBuilder::new().await.unwrap();
    let storage_user =
        builder.add_local_child("storage-user", mock, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::storage("data").path("/data"))
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&storage_user),
        )
        .await
        .unwrap();
    let instance = builder.build().await.unwrap();
    let instance_moniker = format!("{}:{}", DEFAULT_COLLECTION_NAME, instance.root.child_name());
    let storage_user_moniker = format!("{}/storage-user", &instance_moniker);

    done_signal.await;

    let storage_admin = connect_to_protocol::<fcomponent::StorageAdminMarker>().unwrap();
    let storage_users = collect_storage_user_monikers(&storage_admin, &instance_moniker).await;
    assert_eq!(
        storage_users,
        hashset! {
            storage_user_moniker.clone()
        }
    );

    let mut event_stream = EventStream::open().await.unwrap();
    instance.destroy().await.unwrap();

    EventMatcher::ok()
        .moniker(storage_user_moniker)
        .wait::<Destroyed>(&mut event_stream)
        .await
        .unwrap();

    let storage_users = collect_storage_user_monikers(&storage_admin, ".").await;
    assert!(!storage_users.contains(&instance_moniker));
}

/// Tests that the storage admin protocol can open and write a file to the storage for a given
/// component and that the `delete_all_storage_contents` function then successfully deletes that
/// file.
#[fuchsia::test]
async fn storage_admin_with_subdir() {
    let builder = RealmBuilder::new().await.unwrap();
    let provider_child = builder
        .add_local_child(
            "storage_provider",
            |handles| {
                async move {
                    let data_dir = handles.clone_from_namespace("tmp_storage").unwrap();
                    let _ = fuchsia_fs::directory::create_directory_recursive(
                        &data_dir,
                        "data/subdir",
                        fio::PERM_WRITABLE,
                    )
                    .await
                    .unwrap();
                    fuchsia_fs::directory::clone_onto(&data_dir, handles.outgoing_dir).unwrap();
                    futures::future::pending().await
                }
                .boxed()
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder.add_storage("tmp_storage", vec![&provider_child], None).await.unwrap();
    builder
        .add_capability(cm_rust::CapabilityDecl::Storage(cm_rust::StorageDecl {
            name: cm_types::Name::new("data").unwrap(),
            source: cm_rust::StorageDirectorySource::Child("storage_provider".to_string()),
            backing_dir: cm_types::Name::new("data_dir").unwrap(),
            subdir: cm_types::RelativePath::new("subdir").unwrap(),
            storage_id: fdecl::StorageId::StaticInstanceIdOrMoniker,
        }))
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::directory("data_dir").rights(fio::RW_STAR_DIR).path("/data"),
                )
                .from(&provider_child)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fcomponent::StorageAdminMarker>())
                .from(Ref::capability("data"))
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let user_parent = builder.add_child_realm("storage_parent", ChildOptions::new()).await.unwrap();
    let user_child = user_parent
        .add_local_child(
            "storage_user",
            |_handles| futures::future::pending().boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::storage("data"))
                .from(Ref::self_())
                .to(&user_parent),
        )
        .await
        .unwrap();
    user_parent
        .add_route(
            Route::new()
                .capability(Capability::storage("data").path("/data"))
                .from(Ref::parent())
                .to(&user_child),
        )
        .await
        .unwrap();

    let instance = builder.build_in_nested_component_manager(CM_URL).await.unwrap();

    let storage_admin_proxy = instance
        .root
        .connect_to_protocol_at_exposed_dir::<fcomponent::StorageAdminProxy>()
        .unwrap();
    let (dir_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
    storage_admin_proxy
        .open_storage("storage_parent/storage_user", ServerEnd::new(server_end.into_channel()))
        .await
        .unwrap()
        .unwrap();
    let file_proxy = fuchsia_fs::directory::open_file(
        &dir_proxy,
        "example_file",
        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
    )
    .await
    .unwrap();
    fuchsia_fs::file::write(&file_proxy, "example file contents").await.unwrap();

    let exposed_dir = instance.root.get_exposed_dir();
    let data_dir = fuchsia_fs::directory::open_directory(
        exposed_dir,
        "data_dir",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await
    .unwrap();

    async fn readdir_for_files(proxy: &fio::DirectoryProxy) -> Vec<String> {
        let mut readdir = fuchsia_fs::directory::readdir_recursive(proxy, None);
        let mut file_names = vec![];
        while let Some(res) = readdir.next().await {
            match res {
                Ok(e) if e.kind == fuchsia_fs::directory::DirentKind::File => {
                    file_names.push(e.name);
                }
                Ok(_) => (),
                Err(e) => panic!("failed to readdir: {e:?}"),
            }
        }
        file_names
    }

    assert_eq!(
        readdir_for_files(&data_dir).await,
        vec!["subdir/storage_parent:0/children/storage_user:0/data/example_file".to_string()],
    );

    storage_admin_proxy.delete_all_storage_contents().await.unwrap().unwrap();

    assert_eq!(readdir_for_files(&data_dir).await, Vec::<String>::new(),);
}
