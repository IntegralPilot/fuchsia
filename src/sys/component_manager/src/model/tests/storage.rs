// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::{ActionsManager, DestroyAction, ShutdownType};
use crate::model::component::StartReason;
use crate::model::start::Start;
use crate::model::testing::routing_test_helpers::*;
use ::routing_test_helpers::RoutingTestModel;
use ::routing_test_helpers::component_id_index::make_index_file;
use ::routing_test_helpers::storage::CommonStorageTest;
use assert_matches::assert_matches;
use async_utils::PollExt;
use cm_rust::offer::*;
use cm_rust::*;
use cm_rust_testing::*;
use component_id_index::InstanceId;
use errors::{ActionErrorKind, CreateNamespaceError, ModelError, StartActionError};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fuchsia_async::TestExecutor;
use fuchsia_sync as fsync;
use futures::pin_mut;
use moniker::Moniker;
use std::path::Path;
use std::sync::mpsc;
use vfs::ToObjectRequest;
use vfs::directory::entry_container::Directory;
use vfs::execution_scope::ExecutionScope;
use vfs::path::Path as VfsPath;

#[fuchsia::test]
async fn storage_dir_from_cm_namespace() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_storage_dir_from_cm_namespace().await
}

#[fuchsia::test]
async fn storage_from_void() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_storage_from_void().await
}

#[fuchsia::test]
async fn storage_and_dir_from_parent() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_storage_and_dir_from_parent().await
}

#[fuchsia::test]
async fn storage_and_dir_from_parent_with_subdir() {
    CommonStorageTest::<RoutingTestBuilder>::new()
        .test_storage_and_dir_from_parent_with_subdir()
        .await
}

#[fuchsia::test]
async fn storage_and_dir_from_parent_rights_invalid() {
    CommonStorageTest::<RoutingTestBuilder>::new()
        .test_storage_and_dir_from_parent_rights_invalid()
        .await
}

#[fuchsia::test]
async fn storage_from_parent_dir_from_grandparent() {
    CommonStorageTest::<RoutingTestBuilder>::new()
        .test_storage_from_parent_dir_from_grandparent()
        .await
}

#[fuchsia::test]
async fn storage_from_parent_dir_from_grandparent_with_subdirs() {
    CommonStorageTest::<RoutingTestBuilder>::new()
        .test_storage_from_parent_dir_from_grandparent_with_subdirs()
        .await
}

#[fuchsia::test]
async fn storage_from_parent_dir_from_grandparent_with_subdir() {
    CommonStorageTest::<RoutingTestBuilder>::new()
        .test_storage_from_parent_dir_from_grandparent_with_subdir()
        .await
}

#[fuchsia::test]
async fn storage_and_dir_from_grandparent() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_storage_and_dir_from_grandparent().await
}

#[fuchsia::test]
async fn storage_from_parent_dir_from_sibling() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_storage_from_parent_dir_from_sibling().await
}

#[fuchsia::test]
async fn storage_from_parent_dir_from_sibling_with_subdir() {
    CommonStorageTest::<RoutingTestBuilder>::new()
        .test_storage_from_parent_dir_from_sibling_with_subdir()
        .await
}

///   a
///    \
///     b
///      \
///      [c]
///
/// a: offers directory to b at path /minfs
/// b: has storage decl with name "mystorage" with a source of realm at path /data
/// b: offers storage to collection from "mystorage"
/// [c]: uses storage as /storage
/// [c]: destroyed and storage goes away
#[fuchsia::test]
async fn use_in_collection_from_parent() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("data")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .offer(
                    OfferBuilder::directory()
                        .name("data")
                        .target_name("minfs")
                        .source(OfferSource::Self_)
                        .target_static_child("b")
                        .rights(fio::RW_STAR_DIR),
                )
                .child_default("b")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Self_)
                        .target(OfferTarget::Collection("coll".parse().unwrap())),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("cache")
                        .source(OfferSource::Self_)
                        .target(OfferTarget::Collection("coll".parse().unwrap())),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("data")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Parent)
                        .subdir("data"),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("cache")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Parent)
                        .subdir("cache"),
                )
                .collection_default("coll")
                .build(),
        ),
        (
            "c",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("data").path("/data"))
                .use_(UseBuilder::storage().name("cache").path("/cache"))
                .build(),
        ),
    ];
    let test = RoutingTest::new("a", components).await;
    test.create_dynamic_child(&["b"].try_into().unwrap(), "coll", ChildBuilder::new().name("c"))
        .await;

    // Use storage and confirm its existence.
    test.check_use(
        ["b", "coll:c"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["coll:c"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: Some("data".to_string()),
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;
    test.check_use(
        ["b", "coll:c"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/cache".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["coll:c"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: Some("cache".to_string()),
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;
    // Confirm storage directory exists for component in collection
    assert_eq!(
        test.list_directory_in_storage(Some("data"), Moniker::root(), None, "").await,
        vec!["coll:c:0".to_string()],
    );
    assert_eq!(
        test.list_directory_in_storage(Some("cache"), Moniker::root(), None, "").await,
        vec!["coll:c:0".to_string()],
    );
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "coll", "c").await;

    // Confirm storage no longer exists.
    assert_eq!(
        test.list_directory_in_storage(Some("data"), Moniker::root(), None, "").await,
        Vec::<String>::new(),
    );
    assert_eq!(
        test.list_directory_in_storage(Some("cache"), Moniker::root(), None, "").await,
        Vec::<String>::new(),
    );
}

///   a
///    \
///     b
///      \
///      [c]
///
/// a: has storage decl with name "mystorage" with a source of self at path /data
/// a: offers storage to b from "mystorage"
/// b: offers storage to collection from "mystorage"
/// [c]: uses storage as /storage
/// [c]: destroyed and storage goes away
#[fuchsia::test]
async fn use_in_collection_from_grandparent() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("minfs")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Self_)
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("cache")
                        .source(OfferSource::Self_)
                        .target_static_child("b"),
                )
                .child_default("b")
                .capability(
                    CapabilityBuilder::storage()
                        .name("data")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Self_)
                        .subdir("data"),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("cache")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Self_)
                        .subdir("cache"),
                )
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("coll".parse().unwrap())),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("cache")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("coll".parse().unwrap())),
                )
                .collection_default("coll")
                .build(),
        ),
        (
            "c",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("data").path("/data"))
                .use_(UseBuilder::storage().name("cache").path("/cache"))
                .build(),
        ),
    ];
    let test = RoutingTest::new("a", components).await;
    test.create_dynamic_child(&["b"].try_into().unwrap(), "coll", ChildBuilder::new().name("c"))
        .await;

    // Use storage and confirm its existence.
    test.check_use(
        ["b", "coll:c"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["b", "coll:c"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: Some("data".to_string()),
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;
    test.check_use(
        ["b", "coll:c"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/cache".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["b", "coll:c"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: Some("cache".to_string()),
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;
    assert_eq!(
        test.list_directory_in_storage(
            Some("data"),
            Moniker::try_from(["b"]).unwrap(),
            None,
            "children",
        )
        .await,
        vec!["coll:c:0".to_string()]
    );
    assert_eq!(
        test.list_directory_in_storage(
            Some("cache"),
            Moniker::try_from(["b"]).unwrap(),
            None,
            "children",
        )
        .await,
        vec!["coll:c:0".to_string()]
    );
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "coll", "c").await;

    // Confirm storage no longer exists.
    assert_eq!(
        test.list_directory_in_storage(
            Some("data"),
            Moniker::try_from(["b"]).unwrap(),
            None,
            "children"
        )
        .await,
        Vec::<String>::new(),
    );
    assert_eq!(
        test.list_directory_in_storage(
            Some("cache"),
            Moniker::try_from(["b"]).unwrap(),
            None,
            "children"
        )
        .await,
        Vec::<String>::new(),
    );
}

#[fuchsia::test]
async fn storage_multiple_types() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_storage_multiple_types().await
}

#[fuchsia::test]
async fn use_the_wrong_type_of_storage() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_use_the_wrong_type_of_storage().await
}

#[fuchsia::test]
async fn directories_are_not_storage() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_directories_are_not_storage().await
}

#[fuchsia::test]
async fn use_storage_when_not_offered() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_use_storage_when_not_offered().await
}

#[fuchsia::test]
async fn dir_offered_from_nonexecutable() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_dir_offered_from_nonexecutable().await
}

#[fuchsia::test]
async fn storage_dir_from_cm_namespace_prevented_by_policy() {
    CommonStorageTest::<RoutingTestBuilder>::new()
        .test_storage_dir_from_cm_namespace_prevented_by_policy()
        .await
}

#[fuchsia::test]
async fn instance_id_from_index() {
    CommonStorageTest::<RoutingTestBuilder>::new().test_instance_id_from_index().await
}

///   component manager's namespace
///    |
///   provider (provides storage capability, restricted to component ID index)
///    |
///   parent_consumer (in component ID index)
///    |
///   child_consumer (not in component ID index)
///
/// Test that a component cannot start if it uses restricted storage but isn't in the component ID
/// index.
#[fuchsia::test]
async fn use_restricted_storage_start_failure() {
    let parent_consumer_instance_id = InstanceId::new_random(&mut rand::rng());
    let index = {
        let mut index = component_id_index::Index::default();
        index
            .insert(component_id_index::IndexEntry {
                moniker: Moniker::parse_str("/parent_consumer").unwrap(),
                instance_id: parent_consumer_instance_id.clone(),
                ignore_duplicate_id: false,
            })
            .unwrap();
        index
    };
    let component_id_index_path = make_index_file(index).unwrap();
    let components = vec![
        (
            "provider",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("data")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("cache")
                        .backing_dir("data")
                        .source(StorageDirectorySource::Self_)
                        .storage_id(fdecl::StorageId::StaticInstanceId),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("cache")
                        .source(OfferSource::Self_)
                        .target_static_child("parent_consumer"),
                )
                .child_default("parent_consumer")
                .build(),
        ),
        (
            "parent_consumer",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("cache").path("/storage"))
                .offer(
                    OfferBuilder::storage()
                        .name("cache")
                        .source(OfferSource::Parent)
                        .target_static_child("child_consumer"),
                )
                .child_default("child_consumer")
                .build(),
        ),
        (
            "child_consumer",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("cache").path("/storage"))
                .build(),
        ),
    ];
    let test = RoutingTestBuilder::new("provider", components)
        .set_component_id_index_path(component_id_index_path.path().to_owned().try_into().unwrap())
        .build()
        .await;

    test.start_instance(&Moniker::parse_str("/parent_consumer").unwrap())
        .await
        .expect("start /parent_consumer failed");

    let child_bind_result =
        test.start_instance(&Moniker::parse_str("/parent_consumer/child_consumer").unwrap()).await;
    assert_matches!(
        child_bind_result,
        Err(ModelError::ActionError { err }) => {
            assert_matches!(err.kind(), ActionErrorKind::StartError {
                err: StartActionError::CreateNamespaceError(
                    CreateNamespaceError::InstanceNotInInstanceIdIndex(err)
                )
            } if moniker::ExtendedMoniker::from(err.clone()) == moniker::ExtendedMoniker::from(Moniker::try_from([]).unwrap()));
        }
    );
}

///   component manager's namespace
///    |
///   provider (provides storage capability, restricted to component ID index)
///    |
///   parent_consumer (in component ID index)
///
/// Test that a component can open a subdirectory of a storage successfully
#[fuchsia::test]
async fn open_storage_subdirectory() {
    let parent_consumer_instance_id = InstanceId::new_random(&mut rand::rng());
    let index = {
        let mut index = component_id_index::Index::default();
        index
            .insert(component_id_index::IndexEntry {
                moniker: Moniker::parse_str("/child_consumer").unwrap(),
                instance_id: parent_consumer_instance_id.clone(),
                ignore_duplicate_id: false,
            })
            .unwrap();
        index
    };
    let component_id_index_path = make_index_file(index).unwrap();
    let components = vec![
        (
            "provider",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("data")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("cache")
                        .backing_dir("data")
                        .source(StorageDirectorySource::Self_),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("cache")
                        .source(OfferSource::Self_)
                        .target_static_child("consumer"),
                )
                .child_default("consumer")
                .build(),
        ),
        (
            "consumer",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("cache").path("/storage"))
                .build(),
        ),
    ];
    let test = RoutingTestBuilder::new("provider", components)
        .set_component_id_index_path(component_id_index_path.path().to_owned().try_into().unwrap())
        .build()
        .await;

    let consumer_moniker = Moniker::parse_str("/consumer").unwrap();
    let (consumer_instance, _) = test
        .start_and_get_instance(&consumer_moniker, StartReason::Eager, false)
        .await
        .expect("could not resolve state");

    // `consumer` should be able to open its storage dir
    let (root_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    let server_end = server_end.into_channel();
    let scope = ExecutionScope::new();
    let flags = fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY;
    let mut object_request = flags.to_object_request(server_end);
    let consumer_instance_state = consumer_instance.lock_resolved_state().await.unwrap();
    let consumer_namespace = consumer_instance_state.namespace_dir().await.unwrap();
    let path = VfsPath::validate_and_split("/storage").unwrap();
    consumer_namespace.clone().open(scope.clone(), path, flags, &mut object_request).unwrap();

    // Create the subdirectories we will open later
    let bar_dir = fuchsia_fs::directory::create_directory_recursive(
        &root_dir,
        "foo/bar",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await
    .unwrap();
    let entries = fuchsia_fs::directory::readdir(&bar_dir).await.unwrap();
    assert!(entries.is_empty());

    // `consumer` should be able to open its storage at "foo/bar"
    let (bar_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    let scope = ExecutionScope::new();
    let flags = fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY;
    let mut object_request = flags.to_object_request(server_end);
    consumer_namespace
        .open(scope.clone(), "/storage/foo/bar".try_into().unwrap(), flags, &mut object_request)
        .unwrap();

    let entries = fuchsia_fs::directory::readdir(&bar_dir).await.unwrap();
    assert!(entries.is_empty());
}

///   a
///   |
///   b
///   |
///  coll-persistent_storage: "true"
///   |
/// [c]
///
/// Test that storage data persists after destroy for a collection with a moniker-based storage
/// path. The persistent storage data can be deleted through a
/// StorageAdminRequest::DeleteComponentStorage request.
/// The following storage paths are used:
///  - moniker path with instance ids cleared
#[fuchsia::test]
async fn storage_persistence_moniker_path() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("minfs")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("data")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Self_),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Self_)
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.component.StorageAdmin")
                        .source(OfferSource::Capability("data".parse().unwrap()))
                        .target_static_child("b"),
                )
                .child_default("b")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .use_(UseBuilder::protocol().name("fuchsia.component.StorageAdmin"))
                .use_(UseBuilder::storage().name("data").path("/data"))
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("persistent_coll".parse().unwrap())),
                )
                .collection(
                    CollectionBuilder::new().name("persistent_coll").persistent_storage(true),
                )
                .build(),
        ),
        (
            "c",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("data").path("/data"))
                .build(),
        ),
    ];

    let test = RoutingTestBuilder::new("a", components).build().await;

    // create [c] under the storage persistent collection
    test.create_dynamic_child(
        &["b"].try_into().unwrap(),
        "persistent_coll",
        ChildBuilder::new().name("c"),
    )
    .await;

    // write to [c]_1 storage
    test.create_static_file(&Path::new("b:0/children/persistent_coll:c:0/data/c1"), "hippos")
        .await
        .unwrap();

    // destroy [c]_1
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "persistent_coll", "c").await;

    // expect the [c]_1 storage and data to persist
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/data",
        vec!["c1".to_string()],
    )
    .await;

    // recreate dynamic child [c]_1
    test.create_dynamic_child(
        &["b"].try_into().unwrap(),
        "persistent_coll",
        ChildBuilder::new().name("c"),
    )
    .await;

    // write to [c]_2 storage
    test.create_static_file(&Path::new("b:0/children/persistent_coll:c:0/data/c2"), "sharks")
        .await
        .unwrap();

    // destroy [c]_2
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "persistent_coll", "c").await;

    // expect the [c]_1 and [c]_2 storage and data to persist
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/data",
        vec!["c1".to_string(), "c2".to_string()],
    )
    .await;

    // check that the file can be destroyed by storage admin
    let namespace = test.bind_and_get_namespace(["b"].try_into().unwrap()).await;
    let storage_admin_proxy = capability_util::connect_to_svc_in_namespace::<
        fcomponent::StorageAdminMarker,
    >(
        &namespace, &"/svc/fuchsia.component.StorageAdmin".parse().unwrap()
    )
    .await;
    let _ = storage_admin_proxy
        // StorageAdmin::DeleteComponentStorage tolerates both regular old monikers and instanced
        // monikers ("b:0/persistent_col:c:0"). Use the regular moniker here since the IDs would be
        // ignored anyway.
        .delete_component_storage("b/persistent_coll:c")
        .await
        .unwrap()
        .unwrap();

    // expect persistent_coll storage to be destroyed
    capability_util::confirm_storage_is_deleted_for_component(
        None,
        Moniker::try_from(["b", "persistent_coll:c"]).unwrap(),
        None,
        &test.test_dir_proxy,
    )
    .await;
}

///   a
///   |
///   b
///   |
///  coll-persistent_storage: "true" / instance_id
///   |
/// [c:1]
///
/// Test that storage data persists after destroy for a collection with an instance-id-based
/// storage path. The persistent storage data can be deleted through a
/// StorageAdminRequest::DeleteComponentStorage request.
/// The following storage paths are used:
///   - indexed path
#[fuchsia::test]
async fn storage_persistence_instance_id_path() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("minfs")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("data")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Self_),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Self_)
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.component.StorageAdmin")
                        .source(OfferSource::Capability("data".parse().unwrap()))
                        .target_static_child("b"),
                )
                .child_default("b")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .use_(UseBuilder::protocol().name("fuchsia.component.StorageAdmin"))
                .use_(UseBuilder::storage().name("data").path("/data"))
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("persistent_coll".parse().unwrap())),
                )
                .collection(
                    CollectionBuilder::new().name("persistent_coll").persistent_storage(true),
                )
                .build(),
        ),
        (
            "c",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("data").path("/data"))
                .build(),
        ),
    ];

    // set instance_id for "b/persistent_coll:c" components
    let instance_id = InstanceId::new_random(&mut rand::rng());
    let index = {
        let mut index = component_id_index::Index::default();
        index
            .insert(component_id_index::IndexEntry {
                moniker: ["b", "persistent_coll:c"].try_into().unwrap(),
                instance_id: instance_id.clone(),
                ignore_duplicate_id: false,
            })
            .unwrap();
        index
    };
    let component_id_index_path = make_index_file(index).unwrap();

    let test = RoutingTestBuilder::new("a", components)
        .set_component_id_index_path(component_id_index_path.path().to_owned().try_into().unwrap())
        .build()
        .await;

    // create [c]_1 under the storage persistent collection
    test.create_dynamic_child(
        &["b"].try_into().unwrap(),
        "persistent_coll",
        ChildBuilder::new().name("c"),
    )
    .await;

    // write to [c]_1 storage
    test.create_static_file(&Path::new(&format!("{}/c1", instance_id)), "hippos").await.unwrap();

    // destroy [c]_1
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "persistent_coll", "c").await;

    // expect the [c]_1 storage and data to persist
    test.check_test_subdir_contents(&instance_id.to_string(), vec!["c1".to_string()]).await;

    // recreate dynamic child [c]
    test.create_dynamic_child(
        &["b"].try_into().unwrap(),
        "persistent_coll",
        ChildBuilder::new().name("c"),
    )
    .await;

    // write to [c]_2 storage
    test.create_static_file(&Path::new(&format!("{}/c2", instance_id)), "sharks").await.unwrap();

    // destroy [c]_2
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "persistent_coll", "c").await;

    // expect the [c]_1 and [c]_2 storage and data to persist
    test.check_test_subdir_contents(
        &instance_id.to_string(),
        vec!["c1".to_string(), "c2".to_string()],
    )
    .await;

    // destroy the persistent storage with a storage admin request
    let namespace = test.bind_and_get_namespace(["b"].try_into().unwrap()).await;
    let storage_admin_proxy = capability_util::connect_to_svc_in_namespace::<
        fcomponent::StorageAdminMarker,
    >(
        &namespace, &"/svc/fuchsia.component.StorageAdmin".parse().unwrap()
    )
    .await;
    let _ = storage_admin_proxy.delete_component_storage("./b/persistent_coll:c").await.unwrap();

    // expect persistent_coll storage to be destroyed
    capability_util::confirm_storage_is_deleted_for_component(
        None,
        Moniker::try_from(["b", "persistent_coll:c"]).unwrap(),
        Some(&instance_id),
        &test.test_dir_proxy,
    )
    .await;
}

///   a
///   |
///   b
///   |
///  coll-persistent_storage: "true" / instance_id
///   |
/// [c:1]
///  / \
/// d  [coll]
///      |
///     [e:1]
///
/// Test that storage persistence behavior is inherited by descendents with a different storage
/// path.
/// The following storage paths are used:
///   - indexed path
///   - moniker path with instance ids cleared
#[fuchsia::test]
async fn storage_persistence_inheritance() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("minfs")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("data")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Self_),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Self_)
                        .target_static_child("b"),
                )
                .child_default("b")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .use_(UseBuilder::storage().name("data").path("/data"))
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("persistent_coll".parse().unwrap())),
                )
                .collection(
                    CollectionBuilder::new().name("persistent_coll").persistent_storage(true),
                )
                .build(),
        ),
        (
            "c",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .use_(UseBuilder::storage().name("data").path("/data"))
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target_static_child("d"),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("lower_coll".parse().unwrap())),
                )
                .child_default("d")
                .collection_default("lower_coll")
                .build(),
        ),
        (
            "d",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("data").path("/data"))
                .build(),
        ),
        (
            "e",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("data").path("/data"))
                .build(),
        ),
    ];

    // set instance_id for "b/persistent_coll:c" components
    let instance_id = InstanceId::new_random(&mut rand::rng());
    let index = {
        let mut index = component_id_index::Index::default();
        index
            .insert(component_id_index::IndexEntry {
                moniker: ["b", "persistent_coll:c"].try_into().unwrap(),
                instance_id: instance_id.clone(),
                ignore_duplicate_id: false,
            })
            .unwrap();
        index
    };
    let component_id_index_path = make_index_file(index).unwrap();

    let test = RoutingTestBuilder::new("a", components)
        .set_component_id_index_path(component_id_index_path.path().to_owned().try_into().unwrap())
        .build()
        .await;

    // create [c:1] under the storage persistent collection
    test.create_dynamic_child(
        &["b"].try_into().unwrap(),
        "persistent_coll",
        ChildBuilder::new().name("c"),
    )
    .await;

    // write to [c:1] storage
    test.check_use(
        ["b", "persistent_coll:c"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["b", "persistent_coll:c"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: None,
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;

    // start d:0 and write to storage
    test.check_use(
        ["b", "persistent_coll:c", "d"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["b", "persistent_coll:c", "d"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: None,
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;

    // create [e:1] under the lower collection
    test.create_dynamic_child(
        &["b", "persistent_coll:c"].try_into().unwrap(),
        "lower_coll",
        ChildBuilder::new().name("e"),
    )
    .await;

    // write to [e:1] storage
    test.check_use(
        ["b", "persistent_coll:c", "lower_coll:e"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(
                Moniker::try_from(["b", "persistent_coll:c", "lower_coll:e"]).unwrap(),
            ),
            from_cm_namespace: false,
            storage_subdir: None,
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;

    // test that [c:1] wrote to instance id path
    test.check_test_subdir_contents(&instance_id.to_string(), vec!["hippos".to_string()]).await;
    // test that d:0 wrote to moniker based path with instance ids cleared
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/children/d:0/data",
        vec!["hippos".to_string()],
    )
    .await;
    // test that [e:1] wrote to moniker based path with instance ids cleared
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/children/lower_coll:e:0/data",
        vec!["hippos".to_string()],
    )
    .await;

    // destroy [c:1], which will also shutdown d:0 and lower_coll:e:1
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "persistent_coll", "c").await;

    // expect [c:1], d:0, and [e:1] storage and data to persist
    test.check_test_subdir_contents(&instance_id.to_string(), vec!["hippos".to_string()]).await;
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/children/d:0/data",
        vec!["hippos".to_string()],
    )
    .await;
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/children/lower_coll:e:0/data",
        vec!["hippos".to_string()],
    )
    .await;
}

///    a
///    |
///    b
///    |
///   coll-persistent_storage: "true" / instance_id
///   |
///  [c:1]
///   / \
///  d  coll-persistent_storage: "false"
///      |
///     [e:1]
///
///  Test that storage persistence can be disabled by a lower-level collection.
///  The following storage paths are used:
///   - indexed path
///   - moniker path
#[fuchsia::test]
async fn storage_persistence_disablement() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .capability(
                    CapabilityBuilder::directory()
                        .name("minfs")
                        .path("/data")
                        .rights(fio::RW_STAR_DIR),
                )
                .capability(
                    CapabilityBuilder::storage()
                        .name("data")
                        .backing_dir("minfs")
                        .source(StorageDirectorySource::Self_),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Self_)
                        .target_static_child("b"),
                )
                .child_default("b")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .use_(UseBuilder::storage().name("data").path("/data"))
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("persistent_coll".parse().unwrap())),
                )
                .collection(
                    CollectionBuilder::new().name("persistent_coll").persistent_storage(true),
                )
                .build(),
        ),
        (
            "c",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .use_(UseBuilder::storage().name("data").path("/data"))
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target_static_child("d"),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("data")
                        .source(OfferSource::Parent)
                        .target(OfferTarget::Collection("non_persistent_coll".parse().unwrap())),
                )
                .child_default("d")
                .collection(
                    CollectionBuilder::new().name("non_persistent_coll").persistent_storage(false),
                )
                .build(),
        ),
        (
            "d",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .use_(UseBuilder::storage().name("data").path("/data"))
                .build(),
        ),
        (
            "e",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::storage().name("data").path("/data"))
                .build(),
        ),
    ];

    // set instance_id for "b/persistent_coll:c" components
    let instance_id = InstanceId::new_random(&mut rand::rng());
    let index = {
        let mut index = component_id_index::Index::default();
        index
            .insert(component_id_index::IndexEntry {
                moniker: ["b", "persistent_coll:c"].try_into().unwrap(),
                instance_id: instance_id.clone(),
                ignore_duplicate_id: false,
            })
            .unwrap();
        index
    };
    let component_id_index_path = make_index_file(index).unwrap();

    let test = RoutingTestBuilder::new("a", components)
        .set_component_id_index_path(component_id_index_path.path().to_owned().try_into().unwrap())
        .build()
        .await;

    // create [c] under the storage persistent collection
    test.create_dynamic_child(
        &["b"].try_into().unwrap(),
        "persistent_coll",
        ChildBuilder::new().name("c"),
    )
    .await;

    // write to [c] storage
    test.check_use(
        ["b", "persistent_coll:c"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["b", "persistent_coll:c"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: None,
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;

    // start d and write to storage
    test.check_use(
        ["b", "persistent_coll:c", "d"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(Moniker::try_from(["b", "persistent_coll:c", "d"]).unwrap()),
            from_cm_namespace: false,
            storage_subdir: None,
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;

    // create [e] under the non persistent collection
    test.create_dynamic_child(
        &["b", "persistent_coll:c"].try_into().unwrap(),
        "non_persistent_coll",
        ChildBuilder::new().name("e"),
    )
    .await;

    // write to [e] storage
    test.check_use(
        ["b", "persistent_coll:c", "non_persistent_coll:e"].try_into().unwrap(),
        CheckUse::Storage {
            path: "/data".parse().unwrap(),
            storage_relation: Some(
                Moniker::try_from(["b", "persistent_coll:c", "non_persistent_coll:e"]).unwrap(),
            ),
            from_cm_namespace: false,
            storage_subdir: None,
            expected_res: ExpectedResult::Ok,
        },
    )
    .await;

    // test that [c:1] wrote to instance id path
    test.check_test_subdir_contents(&instance_id.to_string(), vec!["hippos".to_string()]).await;
    // test that b children includes persistent_coll:c
    test.check_test_subdir_contents("b:0/children", vec!["persistent_coll:c:0".to_string()]).await;
    // test that d wrote to moniker based path for d and [e]
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/children/d:0/data",
        vec!["hippos".to_string()],
    )
    .await;
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/children/non_persistent_coll:e:0/data",
        vec!["hippos".to_string()],
    )
    .await;

    // destroy [c], which will shutdown d and destroy [e]
    test.destroy_dynamic_child(["b"].try_into().unwrap(), "persistent_coll", "c").await;

    // expect [c], d storage and data to persist
    test.check_test_subdir_contents(&instance_id.to_string(), vec!["hippos".to_string()]).await;
    test.check_test_subdir_contents(
        "b:0/children/persistent_coll:c:0/children/d:0/data",
        vec!["hippos".to_string()],
    )
    .await;

    // expect non_persistent_coll storage and data to be destroyed (only persistent_coll exists)
    capability_util::confirm_storage_is_deleted_for_component(
        None,
        Moniker::try_from(["b", "persistent_coll:c", "non_persistent_coll:e"]).unwrap(),
        None,
        &test.test_dir_proxy,
    )
    .await;
}

/// This is a regression test for https://fxbug.dev/332414801.
///
/// When a component connects to a capability where routing potentially requires unbounded work,
/// those should not block shutdown. If those unbounded work were spawned in the namespace scope,
/// that would block stop and shutdown.
#[fuchsia::test]
fn storage_does_not_block_shutdown_when_backing_dir_hangs() {
    // Building the `RoutingTest` may stall so we run it outside of `run_until_stalled`.
    //
    //   a
    //    \
    //     b
    //
    // a: has storage decl with name "cache" with a source of self at path /data
    // a: offers cache storage to b from "cache"
    // b: uses cache storage as /storage
    let mut executor = TestExecutor::new();
    let test = executor.run_singlethreaded(async move {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("data")
                            .source(StorageDirectorySource::Self_),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        RoutingTestBuilder::new("a", components).build().await
    });

    let mut test_body = Box::pin(async {
        // Setup a backing_dir that hangs.
        let (out_dir_tx, out_dir_rx) = mpsc::sync_channel(1);
        let out_dir_tx = fsync::Mutex::new(out_dir_tx);
        let url = "test:///a";
        test.mock_runner.add_host_fn(
            url,
            Box::new(move |server_end: ServerEnd<fio::DirectoryMarker>| {
                out_dir_tx.lock().try_send(server_end).unwrap();
            }),
        );
        let root = test.model.root();
        root.ensure_started(&StartReason::Debug).await.unwrap();
        test.mock_runner.wait_for_url(url).await;

        // Connect to storage. This will hang because `a` does not respond to outgoing dir requests.
        let storage_user_fut = async {
            test.check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: None,
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx::Status::NOT_FOUND),
                },
            )
            .await;
        };
        pin_mut!(storage_user_fut);
        TestExecutor::poll_until_stalled(&mut storage_user_fut)
            .await
            .expect_pending("Storage connection should hang");

        // We should receive the open request though.
        let out_dir = out_dir_rx.recv().unwrap();

        // Shutdown the component. This should not hang despite an in-progress storage
        // provisioning operation.
        root.shutdown(ShutdownType::Instance).await.unwrap();

        // Drop the `out_dir` which causes the `storage_user_fut` to proceed (with an error).
        drop(out_dir);
        TestExecutor::poll_until_stalled(&mut storage_user_fut)
            .await
            .expect("Storage connection should finish");

        ActionsManager::register(root.clone(), DestroyAction::new()).await.expect("destroy failed");
    });
    executor.run_until_stalled(&mut test_body).unwrap();
}
